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
use core::sync::atomic::{AtomicU32, Ordering};
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
///
/// `owner` mirrors Linux's `fl_owner` token: `Pid` for classic POSIX
/// locks (all threads of one process share them), `Ofd(file_id)` for
/// open-file-description locks (F_OFD_SETLK — one description per
/// `File::file_id`). Two locks never conflict when their owner tokens
/// are EQUAL, regardless of kind — exactly Linux's `posix_locks_conflict`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LockOwner {
    Pid(u32),
    Ofd(u64),
}

impl LockOwner {
    /// PID reported by F_GETLK for a conflicting lock: the process for
    /// Pid-owned locks; 0 for OFD locks (they have no process owner —
    /// POSIX/linux behavior, LTP fcntl34/36 expect 0).
    fn report_pid(&self) -> u32 {
        match *self {
            LockOwner::Pid(p) => p,
            LockOwner::Ofd(_) => 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct RecordLock {
    pub key: (u64, u64),
    pub owner: LockOwner,
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

/// True if the owner refers to no live task — such a lock is stale (the
/// owner exited and a release hook was missed) and must not block anyone.
/// Only Pid-owned locks can go stale; OFD locks die with their File.
fn owner_is_dead(owner: LockOwner) -> bool {
    match owner {
        LockOwner::Pid(pid) => pid != 0 && crate::process::find_task_by_pid(pid).is_none(),
        LockOwner::Ofd(_) => false,
    }
}

/// Reap stale locks (owner exited) from the table. Returns true if any
/// were removed. Caller holds the table lock.
fn reap_stale_locked(table: &mut Vec<RecordLock>) -> bool {
    let before = table.len();
    table.retain(|l| !owner_is_dead(l.owner));
    table.len() != before
}

/// Find the first lock that conflicts with `(key, owner, start, end,
/// exclusive)`. Locks whose owner token EQUALS the requester's never
/// conflict (own-process POSIX locks, same-description OFD locks).
///
/// Table order: Linux keeps each inode's lock list ordered by start
/// offset (new locks are inserted before the first entry that starts
/// after the new one ends) and F_GETLK reports the FIRST conflicting
/// entry — i.e. the one with the lowest start. `apply_owner_lock`
/// re-sorts after every mutation, so `find` reproduces that order
/// (LTP fcntl11 blocks 1-10 depend on it).
fn find_conflict(
    key: (u64, u64),
    owner: LockOwner,
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
                && l.owner != owner
                && (exclusive || l.exclusive)
                && ranges_overlap(l.start, l.end, start, end)
        })
        .copied()
}

/// F_GETLK: report the first lock that would conflict with the request,
/// or None.
pub fn posix_test_lock(
    file: &File,
    owner: LockOwner,
    exclusive: bool,
    start: u64,
    end: u64,
) -> Option<LockConflict> {
    find_conflict(file_lock_key(file), owner, start, end, exclusive).map(|l| LockConflict {
        exclusive: l.exclusive,
        start: l.start,
        end: l.end,
        pid: l.owner.report_pid(),
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
    owner: LockOwner,
    kind: u8,
    start: u64,
    end: u64,
) -> bool {
    let mut removed = false;
    let mut pieces: Vec<RecordLock> = Vec::new();
    let mut i = 0;
    while i < table.len() {
        let l = &table[i];
        if l.key == key && l.owner == owner && ranges_overlap(l.start, l.end, start, end) {
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
            owner,
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
                && l.owner == p.owner
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
    // Keep the table ordered by (key, start, end): F_GETLK reports the
    // FIRST conflicting lock, and Linux's per-inode lists are ordered by
    // start offset (see find_conflict). Stable sort keeps same-start
    // ties in their existing relative order.
    table.sort_by(|a, b| (a.key, a.start, a.end).cmp(&(b.key, b.start, b.end)));
    removed
}

/// F_SETLK / F_SETLKW / F_OFD_SETLK / F_OFD_SETLKW core. `owner` is the
/// requesting owner token (process for POSIX, description for OFD);
/// `kind` is one of F_*_KIND; `wait` selects blocking behaviour.
/// Returns negative errno on failure.
pub fn posix_set_lock(
    file: &File,
    owner: LockOwner,
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
                        && l.owner != owner
                        && (exclusive || l.exclusive)
                        && ranges_overlap(l.start, l.end, start, end)
                }) {
                    // Stale (exited-owner) locks never block: reap and
                    // retry instead of returning EAGAIN.
                    if owner_is_dead(conflict.owner) {
                        reap_stale_locked(&mut table);
                        drop(table);
                        RECORD_LOCK_WAIT.wake_up_all();
                        continue;
                    }
                    drop(table);
                    Outcome::Conflict
                } else {
                    let removed =
                        apply_owner_lock(&mut table, key, owner, kind, start, end);
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
                let removed = apply_owner_lock(&mut table, key, owner, kind, start, end);
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
                    find_conflict(key, owner, start, end, exclusive).is_none()
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
    table.retain(|l| !(l.key == key && l.owner == LockOwner::Pid(pid)));
    if table.len() != before {
        drop(table);
        RECORD_LOCK_WAIT.wake_up_all();
    }
}

/// Release every OFD record lock held by a dying open file description
/// (F_OFD_SETLK locks live and die with the description, like flock).
/// Called from `File::drop`.
pub fn ofd_release_file(file_id: u64) {
    let mut table = RECORD_LOCK_TABLE.lock();
    let before = table.len();
    table.retain(|l| l.owner != LockOwner::Ofd(file_id));
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

// ============================================================================
// File leases (fcntl F_SETLEASE / F_GETLEASE — Linux file leases)
// ============================================================================
//
// Simplified lease model (documented divergence): a lease is remembered
// per open file description and keyed by the inode; F_SETLEASE grant
// checks mirror Linux's (fcntl_setlease → generic_add_lease):
//   - read lease (F_RDLCK): refused with EAGAIN if ANY open description
//     of the inode (including the caller's own) has write access —
//     Linux's i_writecount test (LTP fcntl27);
//   - write lease (F_WRLCK): refused with EAGAIN if ANY OTHER open
//     description of the inode exists, whatever its mode (LTP fcntl32),
//     or another description already holds a lease;
//   - two read leases from different descriptions coexist (readers do
//     not conflict with each other).
// Lease BREAK: a conflicting open/truncate blocks on LEASE_WAIT, the
// holder is signalled (SIGIO), and after fs.lease-break-time seconds
// the kernel force-breaks the lease and lets the opener through —
// Linux's break_lease discipline in miniature (LTP fcntl33).

/// /proc/sys/fs/lease-break-time — seconds the kernel waits for a lease
/// holder to release after SIGIO before breaking the lease by force.
/// Linux default 45; clamp [0, i32::MAX] on sysctl write.
pub static LEASE_BREAK_TIME: AtomicU32 = AtomicU32::new(45);

/// One held lease. `owner_pid` receives the break signal; `owner_fd` is
/// the holder's fd number, reported in /proc/locks-style info (and handy
/// for debugging).
struct LeaseEntry {
    key: (u64, u64),
    file_id: u64,
    owner_pid: u32,
    exclusive: bool,
}

static LEASE_TABLE: Spinlock<Vec<LeaseEntry>> = Spinlock::new(Vec::new());
/// Openers blocked in a lease break.
static LEASE_WAIT: WaitQueueHead = WaitQueueHead::new();

/// Live open-file-description counts per inode key: (total, write-open,
/// breaking_total, breaking_writers) — the last two count openers BLOCKED
/// in break_lease (so a lease holder cannot "downgrade" past a pending
/// write breaker the way Linux's lease-break state machine prevents).
/// Registered from File::set_inode, dropped from File::drop — this is
/// our i_readcount/i_writecount analogue.
type OpenCounts = (u32, u32, u32, u32);
static INODE_OPENS: Spinlock<alloc::collections::BTreeMap<(u64, u64), OpenCounts>> =
    Spinlock::new(alloc::collections::BTreeMap::new());

/// Register a new open file description on `inode` (File::set_inode).
/// `write_access` is the description's access mode.
pub fn inode_open_register(inode: &Inode, write_access: bool) {
    let key = inode_lock_key(inode);
    let mut m = INODE_OPENS.lock();
    let e = m.entry(key).or_insert((0, 0, 0, 0));
    e.0 = e.0.saturating_add(1);
    if write_access {
        e.1 = e.1.saturating_add(1);
    }
}

/// Unregister a dying open file description (File::drop) and release its
/// lease (if any), waking blocked lease breakers.
pub fn inode_open_unregister(file: &File) {
    // SAFETY: inode is written once at open time; read-only access.
    let inode_opt = unsafe { &*file.inode.get() };
    let key = match inode_opt.as_ref() {
        Some(inode) => inode_lock_key(inode),
        None => return,
    };
    // O_ACCMODE = 3; O_WRONLY(1) and O_RDWR(2) count as write access.
    let write_open = file.flags().bits() & 0o3 != 0;
    {
        let mut m = INODE_OPENS.lock();
        if let Some(e) = m.get_mut(&key) {
            e.0 = e.0.saturating_sub(1);
            if write_open {
                e.1 = e.1.saturating_sub(1);
            }
            if e.0 == 0 && e.2 == 0 {
                m.remove(&key);
            }
        }
    }
    // Release the dying description's lease and wake lease breakers.
    let mut table = LEASE_TABLE.lock();
    let before = table.len();
    table.retain(|e| e.file_id != file.file_id);
    let released = table.len() != before;
    drop(table);
    if released {
        LEASE_WAIT.wake_up_all();
    }
}

/// F_SETLEASE core. `lease_type` is the user's flock l_type value:
/// F_RDLCK(0) / F_WRLCK(1) / F_UNLCK(2). Returns negative errno.
pub fn set_lease(file: &File, lease_type: i16) -> Result<(), i32> {
    const EINVAL: i32 = -crate::errno::constants::EINVAL;
    const EAGAIN: i32 = -crate::errno::constants::EAGAIN;

    // Only regular files can carry leases (Linux fcntl_setlease).
    // SAFETY: inode written once at open; read-only access.
    let inode = match unsafe { &*file.inode.get() }.as_ref() {
        Some(i) => i,
        None => return Err(EINVAL),
    };
    if !inode.mode.is_regular_file() {
        return Err(EINVAL);
    }
    let key = inode_lock_key(inode);
    let file_id = file.file_id;

    if lease_type == flock_types_late::F_UNLCK {
        let mut table = LEASE_TABLE.lock();
        table.retain(|e| !(e.key == key && e.file_id == file_id));
        drop(table);
        // Wake blocked breakers: the conflict just disappeared.
        LEASE_WAIT.wake_up_all();
        return Ok(());
    }
    let exclusive = match lease_type {
        flock_types_late::F_RDLCK => false,
        flock_types_late::F_WRLCK => true,
        _ => return Err(EINVAL),
    };

    // Grant checks under both tables (open counts + leases).
    {
        let opens = INODE_OPENS.lock();
        let (total, writers, breaking_total, breaking_writers) =
            opens.get(&key).copied().unwrap_or((0, 0, 0, 0));
        if !exclusive {
            // Read lease: nobody (not even ourselves) may hold the file
            // open for write — Linux's i_writecount gate — and a WRITE
            // breaker must not be pending (Linux refuses downgrades that
            // would leave the breaker unsatisfied; LTP fcntl33 expects
            // the WRLCK→RDLCK downgrade against an O_WRONLY/O_RDWR open
            // to fail with EAGAIN).
            if writers > 0 || breaking_writers > 0 {
                return Err(EAGAIN);
            }
        } else {
            // Write lease: we must be the ONLY open description and no
            // breaker may be in flight.
            if total > 1 || breaking_total > 0 {
                return Err(EAGAIN);
            }
        }
        drop(opens);
        let table = LEASE_TABLE.lock();
        // Another description's lease blocks a new lease of either kind
        // when it would conflict (read+read coexist).
        for e in table.iter() {
            if e.key != key || e.file_id == file_id {
                continue;
            }
            if exclusive || e.exclusive {
                return Err(EAGAIN);
            }
        }
    }
    let owner_pid = current_tgid();
    let mut table = LEASE_TABLE.lock();
    table.retain(|e| !(e.key == key && e.file_id == file_id));
    table.push(LeaseEntry {
        key,
        file_id,
        owner_pid,
        exclusive,
    });
    drop(table);
    // A downgrade or replacement changes the conflict set — wake blocked
    // breakers so they re-evaluate (LTP fcntl33 OP_OPEN_RDONLY: the
    // WRLCK→RDLCK downgrade must unblock the pending reader).
    LEASE_WAIT.wake_up_all();
    Ok(())
}

/// flock l_type values for the lease path (uapi F_RDLCK/F_WRLCK/F_UNLCK).
mod flock_types_late {
    pub const F_RDLCK: i16 = 0;
    pub const F_WRLCK: i16 = 1;
    pub const F_UNLCK: i16 = 2;
}

/// F_GETLEASE: the lease type held by THIS description (F_RDLCK=0,
/// F_WRLCK=1, F_UNLCK=2 when none).
pub fn get_lease(file: &File) -> i16 {
    // SAFETY: inode written once at open; read-only access.
    let key = match unsafe { &*file.inode.get() }.as_ref() {
        Some(i) => inode_lock_key(i),
        // No-inode files can never have a lease — F_UNLCK.
        None => return flock_types_late::F_UNLCK,
    };
    let table = LEASE_TABLE.lock();
    for e in table.iter() {
        if e.key == key && e.file_id == file.file_id {
            return if e.exclusive {
                flock_types_late::F_WRLCK
            } else {
                flock_types_late::F_RDLCK
            };
        }
    }
    flock_types_late::F_UNLCK
}

/// Does the about-to-happen operation conflict with a lease on `inode`?
/// `writer` is true for write-mode opens and truncate; a write lease
/// conflicts with ANY new open, a read lease only with writers
/// (and with truncate).
pub fn lease_conflicts(inode: &Inode, writer: bool) -> bool {
    let key = inode_lock_key(inode);
    let table = LEASE_TABLE.lock();
    table
        .iter()
        .any(|e| e.key == key && (e.exclusive || writer))
}

/// Break leases on `inode` conflicting with the pending operation
/// (`writer` = write-mode open / truncate). Sends SIGIO to every lease
/// holder and blocks until they release/downgrade, the caller is
/// signalled, or fs.lease-break-time elapses (then the kernel breaks
/// the lease by force, as Linux does). Returns negative errno on
/// interrupt; Ok(()) when the open may proceed.
pub fn break_lease(inode: &Inode, writer: bool) -> Result<(), i32> {
    let key = inode_lock_key(inode);
    // Fast path: no conflicting lease.
    if !lease_conflicts(inode, writer) {
        return Ok(());
    }
    // Signal every holder whose lease conflicts with this open.
    {
        let table = LEASE_TABLE.lock();
        for e in table.iter() {
            if e.key == key && (e.exclusive || writer) {
                let _ = crate::signal::send_signal(
                    e.owner_pid,
                    crate::signal::Signal::SIGIO as i32,
                );
            }
        }
    }
    // Count this pending breaker so set_lease grant checks can refuse a
    // lease that would strand it (see INODE_OPENS).
    {
        let mut m = INODE_OPENS.lock();
        let e = m.entry(key).or_insert((0, 0, 0, 0));
        e.2 = e.2.saturating_add(1);
        if writer {
            e.3 = e.3.saturating_add(1);
        }
    }
    let break_secs = LEASE_BREAK_TIME.load(Ordering::Relaxed);
    let break_msecs = (break_secs as u64).saturating_mul(1000).max(1);
    let deadline = crate::drivers::timer::get_jiffies()
        + crate::drivers::timer::msecs_to_jiffies(break_msecs);

    // One-shot timer so a holder that never releases cannot wedge us.
    let my_pid = current_tgid();
    let timer_id = crate::timer::add_timer_wakeup(deadline, my_pid);

    let ret = loop {
        if !lease_conflicts(inode, writer) {
            break Ok(());
        }
        if crate::drivers::timer::get_jiffies() >= deadline {
            // Lease-break-time expired: break the conflicting leases by
            // force (Linux break_lease → generic_break_lease).
            let mut table = LEASE_TABLE.lock();
            table.retain(|e| !(e.key == key && (e.exclusive || writer)));
            drop(table);
            LEASE_WAIT.wake_up_all();
            break Ok(());
        }
        if crate::signal::signal_pending() {
            // No global ERESTARTSYS restart machinery — report EINTR
            // like the F_SETLKW waiter path does.
            break Err(-crate::errno::constants::EINTR);
        }
        crate::wait_event_interruptible!(&LEASE_WAIT, {
            !lease_conflicts(inode, writer)
                || crate::drivers::timer::get_jiffies() >= deadline
        });
        // wait_event_interruptible returns 0/-ERESTARTSYS; loop re-checks.
    };
    crate::timer::del_timer(timer_id);
    // Uncount the pending breaker (every exit path lands here).
    {
        let mut m = INODE_OPENS.lock();
        if let Some(e) = m.get_mut(&key) {
            e.2 = e.2.saturating_sub(1);
            if writer {
                e.3 = e.3.saturating_sub(1);
            }
            if e.0 == 0 && e.2 == 0 {
                m.remove(&key);
            }
        }
    }
    ret
}
