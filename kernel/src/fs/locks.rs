//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File locks: BSD flock(2) + POSIX fcntl(2) record locks
//! (P0-5 — sqlite / package-manager data safety).
//!
//! ## Ownership model (mirrors Linux)
//!
//! **flock(2)** locks are owned by the OPEN FILE DESCRIPTION
//! (`File::file_id`, the R36-B1 never-repeating generation) and keyed by
//! the file's `(fs_id, ino)` identity:
//! - two separate open()s of the same path hold SEPARATE locks that
//!   conflict with each other (flock is not per-process);
//! - dup'd fds (and fork's fd-table copies) share the description and
//!   therefore the lock;
//! - the lock is released when the description dies — hooked from
//!   `File::drop`, which covers close(2), dup2(2) displacement and
//!   process exit in one place;
//! - a re-lock through the same description CONVERTS the lock (SH<->EX),
//!   possibly blocking/EWOULDBLOCKing against the other holders.
//!
//! **POSIX record locks** (F_SETLK/F_SETLKW/F_GETLK) are owned by the
//! PROCESS (tgid) and keyed by `(fs_id, ino)`:
//! - locks of one process never conflict with each other; a set request
//!   REPLACES the owner's overlapping locks (split/merge per POSIX);
//! - read locks are shared, write locks exclusive, across processes;
//! - closing ANY fd referring to the file releases ALL of the closing
//!   process's record locks on that file — the classic POSIX semantic —
//!   hooked from `FdTable::close_fd`/`dup2_fd`/`Drop`;
//! - as belt-and-braces against a missed hook, a conflicting lock whose
//!   owner process no longer exists is treated as stale and reaped
//!   (no permanent EAGAIN / F_SETLKW hang from dead owners).
//!
//! Both tables are global `Spinlock<Vec<..>>`s: lock counts are small
//! (sqlite uses a handful per database), O(n) scans are fine, and a
//! global wait queue per lock class keeps the wake-all-and-recheck
//! discipline simple (thundering herd at worst).

use alloc::vec::Vec;
use crate::fs::file::File;
use crate::fs::inode::Inode;
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

/// flock(2) operation bits (Linux UAPI).
pub mod flock_ops {
    pub const LOCK_SH: i32 = 1;
    pub const LOCK_EX: i32 = 2;
    pub const LOCK_NB: i32 = 4;
    pub const LOCK_UN: i32 = 8;
}

/// File identity key for locking: the inode's `(fs_id, ino)`. For files
/// without an inode (pipes, sockets — flock(2) is legal on them on Linux)
/// the description id stands in; a pipe has no other flock domain anyway.
fn file_lock_key(file: &File) -> (u64, u64) {
    // SAFETY: inode is written once at open time and never mutated;
    // read-only access.
    let inode_opt = unsafe { &*file.inode.get() };
    match inode_opt.as_ref() {
        Some(inode) => (inode.fs_id, inode.ino),
        None => (u64::MAX, file.file_id),
    }
}

/// Lock identity key of an Inode.
fn inode_lock_key(inode: &Inode) -> (u64, u64) {
    (inode.fs_id, inode.ino)
}

/// tgid of the current task (record-lock owner). Falls back to pid, and
/// to 0 in non-task context (kernel threads never hold user record locks).
fn current_tgid() -> u32 {
    match crate::sched::current() {
        Some(task) => task.tgid(),
        None => 0,
    }
}

// ============================================================================
// flock(2)
// ============================================================================

/// One held flock lock. `file_id` identifies the owning open file
/// description; `owner_pid` is informational (who took it — reserved
/// for a future /proc/locks, and useful in crash dumps).
#[allow(dead_code)]
struct FlockEntry {
    key: (u64, u64),
    file_id: u64,
    owner_pid: u32,
    exclusive: bool,
}

/// Global flock table (design note in module header).
static FILE_LOCK_TABLE: Spinlock<Vec<FlockEntry>> = Spinlock::new(Vec::new());
/// All flock waiters (wake-all-and-recheck on every release/conversion).
static FLOCK_WAIT_QUEUE: WaitQueueHead = WaitQueueHead::new();

/// Would a lock request conflict, ignoring the caller's own description?
fn flock_conflicts(key: (u64, u64), file_id: u64, exclusive: bool) -> bool {
    let table = FILE_LOCK_TABLE.lock();
    table.iter().any(|e| {
        e.key == key && e.file_id != file_id && (exclusive || e.exclusive)
    })
}

/// flock(2) core. `operation` is LOCK_SH/LOCK_EX/LOCK_UN optionally ORed
/// with LOCK_NB. Returns negative errno on failure.
pub fn flock_lock(file: &File, operation: i32) -> Result<(), i32> {
    use flock_ops::*;

    let nonblock = operation & LOCK_NB != 0;
    let key = file_lock_key(file);
    let file_id = file.file_id;
    let owner_pid = match crate::sched::current() {
        Some(task) => task.pid(),
        None => 0,
    };

    let op = operation & !LOCK_NB;
    match op {
        LOCK_UN => {
            let mut table = FILE_LOCK_TABLE.lock();
            let before = table.len();
            table.retain(|e| !(e.key == key && e.file_id == file_id));
            if table.len() != before {
                drop(table);
                FLOCK_WAIT_QUEUE.wake_up_all();
            }
            Ok(())
        }
        LOCK_SH | LOCK_EX => {
            let exclusive = op == LOCK_EX;
            loop {
                // Try to install/convert under one lock hold: our own
                // previous entry (same description) is replaced — a
                // conversion never conflicts with itself.
                let acquired = {
                    let mut table = FILE_LOCK_TABLE.lock();
                    if flock_conflicts_locked(&table, key, file_id, exclusive) {
                        false
                    } else {
                        table.retain(|e| !(e.key == key && e.file_id == file_id));
                        table.push(FlockEntry {
                            key,
                            file_id,
                            owner_pid,
                            exclusive,
                        });
                        true
                    }
                };
                if acquired {
                    return Ok(());
                }
                if nonblock {
                    // EWOULDBLOCK == EAGAIN on Linux.
                    return Err(-crate::errno::constants::EAGAIN);
                }
                // Blocking: wait until no other description holds a
                // conflicting lock (interruptible, prepare_to_wait
                // discipline via the house macro).
                let ret = crate::wait_event_interruptible!(
                    &FLOCK_WAIT_QUEUE,
                    !flock_conflicts(key, file_id, exclusive)
                );
                if ret != 0 {
                    return Err(-crate::errno::constants::EINTR);
                }
                // Loop: re-check AND install atomically (another waiter
                // may have won the race between the wake and our lock).
            }
        }
        _ => Err(-crate::errno::constants::EINVAL),
    }
}

/// Conflict check against an already-held table guard (avoids double
/// locking on the install path).
fn flock_conflicts_locked(
    table: &Vec<FlockEntry>,
    key: (u64, u64),
    file_id: u64,
    exclusive: bool,
) -> bool {
    table.iter().any(|e| {
        e.key == key && e.file_id != file_id && (exclusive || e.exclusive)
    })
}

/// Release every flock held by a dying open file description. Called from
/// `File::drop` — the exact moment the last fd (possibly across fork
/// copies and in-flight syscall clones) goes away.
pub fn flock_release_file(file_id: u64) {
    let mut table = FILE_LOCK_TABLE.lock();
    let before = table.len();
    table.retain(|e| e.file_id != file_id);
    if table.len() != before {
        drop(table);
        FLOCK_WAIT_QUEUE.wake_up_all();
    }
}

// ============================================================================
// POSIX record locks (fcntl F_GETLK / F_SETLK / F_SETLKW)
// ============================================================================

/// Record-lock request/entry. `end` is EXCLUSIVE; `u64::MAX` means
/// "to EOF" (and tracks EOF as it grows, POSIX).
#[derive(Clone, Copy)]
pub struct RecordLock {
    pub key: (u64, u64),
    pub owner_pid: u32,
    pub start: u64,
    pub end: u64,
    pub exclusive: bool,
}

/// A conflicting lock reported to F_GETLK.
pub struct LockConflict {
    pub exclusive: bool,
    pub start: u64,
    pub end: u64,
    pub pid: u32,
}

/// Lock type for requests: 0 = unlock, 1 = read (shared), 2 = write (exclusive).
pub const F_UNLCK_KIND: u8 = 0;
pub const F_RDLCK_KIND: u8 = 1;
pub const F_WRLCK_KIND: u8 = 2;

static RECORD_LOCK_TABLE: Spinlock<Vec<RecordLock>> = Spinlock::new(Vec::new());
/// All blocked F_SETLKW waiters (wake-all-and-recheck on every change).
static RECORD_LOCK_WAIT: WaitQueueHead = WaitQueueHead::new();

/// Ranges [s1,e1) and [s2,e2) overlap. A "to EOF" end of u64::MAX works
/// out naturally. Zero-length requests overlap nothing (POSIX).
fn ranges_overlap(s1: u64, e1: u64, s2: u64, e2: u64) -> bool {
    s1 < e2 && s2 < e1
}

/// True if `pid` refers to no live task — such a lock is stale (the
/// owner exited and a release hook was missed) and must not block anyone.
fn owner_is_dead(pid: u32) -> bool {
    pid != 0 && crate::process::find_task_by_pid(pid).is_none()
}

/// Reap stale locks (owner exited) from the table. Returns true if any
/// were removed. Caller holds the table lock.
fn reap_stale_locked(table: &mut Vec<RecordLock>) -> bool {
    let before = table.len();
    table.retain(|l| !owner_is_dead(l.owner_pid));
    table.len() != before
}

/// Find the first lock (POSIX: in unspecified order; we use acquisition
/// order) that conflicts with `(key, pid, start, end, exclusive)`.
/// Locks held by `pid` itself never conflict.
fn find_conflict(
    key: (u64, u64),
    pid: u32,
    start: u64,
    end: u64,
    exclusive: bool,
) -> Option<RecordLock> {
    let mut table = RECORD_LOCK_TABLE.lock();
    // Belt-and-braces: reap locks of exited owners so a missed release
    // hook can never wedge a database forever.
    if reap_stale_locked(&mut table) {
        drop(table);
        RECORD_LOCK_WAIT.wake_up_all();
        table = RECORD_LOCK_TABLE.lock();
    }
    table
        .iter()
        .find(|l| {
            l.key == key
                && l.owner_pid != pid
                && (exclusive || l.exclusive)
                && ranges_overlap(l.start, l.end, start, end)
        })
        .copied()
}

/// F_GETLK: report the first lock that would conflict with the request,
/// or None.
pub fn posix_test_lock(
    file: &File,
    pid: u32,
    exclusive: bool,
    start: u64,
    end: u64,
) -> Option<LockConflict> {
    find_conflict(file_lock_key(file), pid, start, end, exclusive).map(|l| LockConflict {
        exclusive: l.exclusive,
        start: l.start,
        end: l.end,
        pid: l.owner_pid,
    })
}

/// Remove/split the owner's locks overlapping `[start, end)`, inserting
/// the requested lock when `exclusive`/shared kind != unlock. POSIX
/// replacement semantics: an owner's overlapping locks are split around
/// the new region and adjacent same-type pieces coalesce.
/// Caller holds the table lock. Returns true if any entry was REMOVED
/// (wake-worthy).
fn apply_owner_lock(
    table: &mut Vec<RecordLock>,
    key: (u64, u64),
    pid: u32,
    kind: u8,
    start: u64,
    end: u64,
) -> bool {
    let mut removed = false;
    let mut pieces: Vec<RecordLock> = Vec::new();
    let mut i = 0;
    while i < table.len() {
        let l = &table[i];
        if l.key == key && l.owner_pid == pid && ranges_overlap(l.start, l.end, start, end) {
            let l = table.remove(i); // does not shift the unvisited tail
            removed = true;
            // Leading fragment: [l.start, min(l.end, start))
            if l.start < start {
                pieces.push(RecordLock { end: start, ..l });
            }
            // Trailing fragment: [max(l.start, end), l.end)
            if end < l.end {
                pieces.push(RecordLock { start: end, ..l });
            }
        } else {
            i += 1;
        }
    }
    if kind != F_UNLCK_KIND {
        pieces.push(RecordLock {
            key,
            owner_pid: pid,
            start,
            end,
            exclusive: kind == F_WRLCK_KIND,
        });
    }
    // Coalesce adjacent/overlapping same-owner same-type fragments and
    // merge with surviving locks of the same type (POSIX merging).
    for p in pieces {
        let mut merged = false;
        for l in table.iter_mut() {
            if l.key == p.key
                && l.owner_pid == p.owner_pid
                && l.exclusive == p.exclusive
                && l.start <= p.end
                && p.start <= l.end
            {
                l.start = l.start.min(p.start);
                l.end = l.end.max(p.end);
                merged = true;
                break;
            }
        }
        if !merged {
            table.push(p);
        }
    }
    removed
}

/// F_SETLK / F_SETLKW core. `kind` is one of F_*_KIND; `wait` selects
/// blocking behaviour. Returns negative errno on failure.
pub fn posix_set_lock(
    file: &File,
    pid: u32,
    kind: u8,
    start: u64,
    end: u64,
    wait: bool,
) -> Result<(), i32> {
    if start >= end {
        // Zero/negative-length region: POSIX EINVAL for malformed input
        // (empty regions can never hold a lock; callers already reject
        // l_len < 0 making start>end, this catches the residual).
        return Err(-crate::errno::constants::EINVAL);
    }
    let key = file_lock_key(file);
    let exclusive = kind == F_WRLCK_KIND;

    loop {
        let outcome = {
            let mut table = RECORD_LOCK_TABLE.lock();
            if exclusive || kind == F_RDLCK_KIND {
                if let Some(conflict) = table.iter().find(|l| {
                    l.key == key
                        && l.owner_pid != pid
                        && (exclusive || l.exclusive)
                        && ranges_overlap(l.start, l.end, start, end)
                }) {
                    // Stale (exited-owner) locks never block: reap and
                    // retry instead of returning EAGAIN.
                    if owner_is_dead(conflict.owner_pid) {
                        reap_stale_locked(&mut table);
                        drop(table);
                        RECORD_LOCK_WAIT.wake_up_all();
                        continue;
                    }
                    drop(table);
                    Outcome::Conflict
                } else {
                    let removed =
                        apply_owner_lock(&mut table, key, pid, kind, start, end);
                    drop(table);
                    if removed {
                        // Our own replaced/split locks may unblock others
                        // holding F_SETLKW on overlapping regions.
                        RECORD_LOCK_WAIT.wake_up_all();
                    }
                    Outcome::Granted
                }
            } else {
                // F_UNLCK never conflicts.
                let removed = apply_owner_lock(&mut table, key, pid, kind, start, end);
                drop(table);
                if removed {
                    RECORD_LOCK_WAIT.wake_up_all();
                }
                Outcome::Granted
            }
        };
        match outcome {
            Outcome::Granted => return Ok(()),
            Outcome::Conflict => {
                if !wait {
                    // Linux returns EAGAIN (== EACCES on some systems).
                    return Err(-crate::errno::constants::EAGAIN);
                }
                let ret = crate::wait_event_interruptible!(
                    &RECORD_LOCK_WAIT,
                    find_conflict(key, pid, start, end, exclusive).is_none()
                );
                if ret != 0 {
                    return Err(-crate::errno::constants::EINTR);
                }
                // Loop: re-check and grant atomically.
            }
        }
    }
}

enum Outcome {
    Granted,
    Conflict,
}

/// Release ALL record locks `pid` holds on `inode` — the POSIX
/// "close any fd of the file drops the process's locks on it" rule.
/// Called from FdTable::close_fd / dup2_fd / Drop.
pub fn posix_release_on_close(inode: &Inode) {
    let pid = current_tgid();
    if pid == 0 {
        return;
    }
    let key = inode_lock_key(inode);
    let mut table = RECORD_LOCK_TABLE.lock();
    let before = table.len();
    table.retain(|l| !(l.key == key && l.owner_pid == pid));
    if table.len() != before {
        drop(table);
        RECORD_LOCK_WAIT.wake_up_all();
    }
}

/// FdTable close-hook wrapper: releases the closing task's record locks
/// on the inode behind `file` (no-op for inode-less files such as pipes).
pub fn posix_release_for_file(file: &File) {
    // SAFETY: inode is written once at open time and never mutated;
    // read-only access.
    let inode_opt = unsafe { &*file.inode.get() };
    if let Some(inode) = inode_opt.as_ref() {
        posix_release_on_close(inode);
    }
}

/// Debug/testing helper: total record locks held.
#[allow(dead_code)]
pub fn record_lock_count() -> usize {
    RECORD_LOCK_TABLE.lock().len()
}

/// Debug/testing helper: total flock locks held.
#[allow(dead_code)]
pub fn flock_lock_count() -> usize {
    FILE_LOCK_TABLE.lock().len()
}
