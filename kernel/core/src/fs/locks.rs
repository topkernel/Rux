//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File locks: BSD flock(2) + POSIX fcntl(2) record locks
//! (P0-5 — sqlite / package-manager data safety).
//!
//! ## The fs-rest split: THIS file is the framework half
//!
//! The flock/POSIX/lease LOGIC and its tables live in the fs crate
//! (`kernel/fs/src/locks.rs`, under crate-level forbid); THIS
//! module keeps everything the kernel's open/close/exec paths touch
//! directly, so the hot legs pay zero indirection (crate-plan §3a
//! rule 2 — no fn-pointer rows on data paths):
//!
//! - the live-entry count mirrors (`FLOCK_COUNT` / `RECORD_LOCK_COUNT`
//!   / `LEASE_COUNT`), read by the per-open/per-close fast-path gates;
//!   the crate's table critical sections STORE them (they are `pub`
//!   for exactly that);
//! - the open/close bookkeeping: `inode_open_register` (per-open inode
//!   atomics), `inode_open_unregister` / `flock_release_file` /
//!   `ofd_release_file` / `posix_release_for_file` (the close-path
//!   legs: fast gate here, table work through a COLD row that only
//!   fires when locks/leases are actually held);
//! - `break_lease`'s no-lease fast gate (every write-mode open);
//! - the exec text-hold family (`TEXT_HOLDS` + acquire/release/count —
//!   mm_struct and the exec path call these directly);
//! - the shared owner/type vocabulary (`LockOwner`, `LockConflict`,
//!   `F_*_KIND`) the VFS fcntl arms construct and the crate consumes.
//!
//! The cold rows live in `lock_hooks::LocksHooks` (the registration
//! slot model this file's seam predicted at rung 3); the crate
//! registers them at boot alongside the other tables.
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
//! Both halves' tables are global `Spinlock<Vec<..>>`s (crate side):
//! lock counts are small (sqlite uses a handful per database), O(n)
//! scans are fine, and a global wait queue per lock class keeps the
//! wake-all-and-recheck discipline simple (thundering herd at worst).

// (INV-LOCKS-KEY-1: the file-lock family — lock keys and the open/
// lease registries live in locked maps; the raw file/inode words in
// the release/lease rows are identity keys into those maps (never
// dereferenced), read under INV-FILE-CELL-1's cell discipline.)
#![forbid(unsafe_code)]
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::fs::file::File;
use crate::fs::inode::Inode;
use crate::sync::spinlock::Spinlock;

/// Record-lock ownership token mirror (the tables storing it live in
/// the fs crate's locks module).
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
    pub fn report_pid(&self) -> u32 {
        match *self {
            LockOwner::Pid(p) => p,
            LockOwner::Ofd(_) => 0,
        }
    }
}

/// A conflicting lock reported to F_GETLK (constructed crate-side from
/// the posix_test_lock row, consumed by the VFS fcntl arm).
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

// ============================================================================
// The live-entry count mirrors (framework-side: the open/close fast-path
// gates read them; the crate's critical sections store them)
// ============================================================================

/// Live-entry mirror of the flock table, stored inside every critical
/// section that mutates it (the crate side does the storing). File::drop's
/// release path reads this first and skips the slow row + table scan when
/// it is 0 — no process holds flocks on a default boot, so every close
/// saves one row dispatch + irqsave lock pair + O(table) scan. Soundness
/// of the 0-read against a racing FIRST-ever insert: an insert runs inside
/// flock_lock(&File) which holds a File reference; File::drop (the gate
/// reader) only runs after the LAST reference died, and the Arc refcount's
/// Release/Acquire chain orders the dropping CPU's Acquire load of this
/// counter after the inserter's Release store of the new length.
pub static FLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Live-entry mirror of the POSIX record-lock table (same protocol as
/// FLOCK_COUNT). The two per-CLOSE release paths (posix_release_for_file
/// via fdreap::retire_complete, ofd_release_file via File::drop) read
/// this first and skip the slow row when it is 0 — the overwhelmingly
/// common case (no process on the box holds record locks), and one row
/// dispatch + irqsave lock pair + O(table) scan off every close (the
/// close side of lat_syscall,open; perf-path lane measured the whole
/// close at 300-400 ns). Exactness: 0 proves the table was empty at the
/// storing critical section; a concurrent FIRST-ever insert racing the
/// read can leave one lock unreleased by that close — a window that
/// needs a task inserting the box's first record lock in the exact
/// interval another thread of the same process closes a same-inode
/// file; the stale entry is still reaped by reap_stale_locked
/// (belt-and-braces, crate side) and by that owner's next release path.
pub static RECORD_LOCK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Live-entry mirror of the lease table (same protocol). Every
/// regular-file open runs break_lease's conflict probe and every close
/// runs the unregister lease leg; with no leases on the box (the
/// default) both collapse to one Acquire load. Soundness against a
/// racing FIRST-ever set_lease: the setter runs inside fcntl on a held
/// File reference; File::drop only runs after the last reference died,
/// and the Arc refcount Release/Acquire chain orders the dropping CPU's
/// Acquire read after the setter's Release length store.
pub static LEASE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Lock identity key of an Inode (shared by both halves).
pub fn inode_lock_key(inode: &Inode) -> (u64, u64) {
    (inode.fs_id, inode.ino)
}

// ============================================================================
// Open/close bookkeeping (the hot legs — direct calls; the table work
// rides COLD rows that only fire when the count mirrors are non-zero)
// ============================================================================

/// Register a new open file description on `inode` (File::set_inode).
/// `write_access` is the description's access mode.
pub fn inode_open_register(inode: &Inode, write_access: bool) {
    inode
        .open_total
        .fetch_add(1, Ordering::AcqRel);
    if write_access {
        inode
            .open_write
            .fetch_add(1, Ordering::AcqRel);
    }
}

/// Unregister a dying open file description (File::drop) and release its
/// lease (if any), waking blocked lease breakers.
pub fn inode_open_unregister(file: &File) {
    // SAFETY: inode is written once at open time; read-only access.
    let inode_opt = crate::fs::r#unsafe::file_inode_arc(file);
    let inode = match inode_opt.as_ref() {
        Some(inode) => inode,
        None => return,
    };
    // O_ACCMODE = 3; O_WRONLY(1) and O_RDWR(2) count as write access.
    let write_open = file.flags().bits() & 0o3 != 0;
    inode
        .open_total
        .fetch_sub(1, Ordering::AcqRel);
    if write_open {
        inode
            .open_write
            .fetch_sub(1, Ordering::AcqRel);
    }
    // Release the dying description's lease and wake lease breakers.
    // closegap: empty-table fast path — see LEASE_COUNT. The table work
    // rides the COLD row: a box with zero leases (the default boot)
    // never crosses the crate boundary here.
    if LEASE_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    (super::lock_hooks::lock_rows().lease_release_slow)(file.file_id);
}

/// Release every flock held by a dying open file description. Called from
/// `File::drop` — the exact moment the last fd (possibly across fork
/// copies and in-flight syscall clones) goes away.
pub fn flock_release_file(file_id: u64) {
    // closegap: empty-table fast path — see FLOCK_COUNT. One Acquire
    // load replaces the row dispatch + lock pair + retain scan on every
    // File::drop.
    if FLOCK_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    (super::lock_hooks::lock_rows().flock_release_slow)(file_id);
}

/// Release every OFD record lock held by a dying open file description
/// (F_OFD_SETLK locks live and die with the description, like flock).
/// Called from `File::drop`.
pub fn ofd_release_file(file_id: u64) {
    // closegap: empty-table fast path — see RECORD_LOCK_COUNT.
    if RECORD_LOCK_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    (super::lock_hooks::lock_rows().ofd_release_slow)(file_id);
}

/// FdTable close-hook wrapper: releases the closing task's record locks
/// on the inode behind `file` (no-op for inode-less files such as pipes).
pub fn posix_release_for_file(file: &File) {
    // closegap: empty-table fast path — see RECORD_LOCK_COUNT. The gate
    // runs before the inode fetch (the old body paid the cell read
    // first; with no record locks on the box — the default — close(2)
    // now skips it).
    if RECORD_LOCK_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    // SAFETY: inode is written once at open time and never mutated;
    // read-only access.
    let inode_opt = crate::fs::r#unsafe::file_inode_arc(file);
    if let Some(inode) = inode_opt.as_ref() {
        (super::lock_hooks::lock_rows().posix_release_slow)(inode_lock_key(inode));
    }
}

// ============================================================================
// File leases (fcntl F_SETLEASE / F_GETLEASE — Linux file leases)
// ============================================================================
//
// The lease TABLE, the grant checks and the break machinery live
// crate-side; this side keeps the per-write-open fast gate. The model
// (documented divergence): a lease is remembered per open file
// description and keyed by the inode; F_SETLEASE grant checks mirror
// Linux's (fcntl_setlease → generic_add_lease):
//   - read lease (F_RDLCK): refused with EAGAIN if ANY open description
//     of the inode (including the caller's own) has write access —
//     Linux's i_writecount test (LTP fcntl27);
//   - write lease (F_WRLCK): refused with EAGAIN if ANY OTHER open
//     description of the inode exists, whatever its mode (LTP fcntl32),
//     or another description already holds a lease;
//   - two read leases from different descriptions coexist (readers do
//     not conflict with each other).
// Lease BREAK: a conflicting open/truncate blocks on the crate-side
// LEASE_WAIT, the holder is signalled (SIGIO), and after
// fs.lease-break-time seconds the kernel force-breaks the lease and
// lets the opener through — Linux's break_lease discipline in miniature
// (LTP fcntl33).

/// Break leases on `inode` conflicting with the pending operation
/// (`writer` = write-mode open / truncate). Framework half: the
/// no-lease fast gate every write-mode open pays (one Acquire load —
/// the old lease_conflicts probe collapsed to exactly this when the
/// table was empty); the signal/wait machinery rides the COLD row.
pub fn break_lease(inode: &Inode, writer: bool) -> Result<(), crate::errno::Errno> {
    if LEASE_COUNT.load(Ordering::Acquire) == 0 {
        return Ok(());
    }
    (super::lock_hooks::lock_rows().break_lease_slow)(inode, writer)
}

/// Number of open file descriptions holding `inode` for write access
/// (the i_writecount analogue). execve's ETXTBSY gate reads this
/// (Linux deny_write_access in do_open_execat; LTP execve04): executing
/// a file some open description still writes would alias the text
/// against a concurrent writer's bytes.
pub fn inode_write_open_count(inode: &Inode) -> u32 {
    inode.open_write.load(Ordering::Acquire)
}

// ============================================================================
// Exec text holds (the ETXTBSY negative i_writecount — mm_struct and
// the exec path call these directly; never crosses the crate boundary)
// ============================================================================

/// Live exec text holds per inode key — the NEGATIVE half of the
/// reference i_writecount. deny_write_access() at exec commit takes one
/// hold for the image; allow_write_access() drops it when the last mm of
/// that exec lineage dies (our MmStruct::drop stand-in for the
/// mm->exe_file fput). While any hold is live, a write-mode (or
/// O_TRUNC) open of the file fails ETXTBSY (reference may_open ->
/// get_write_access; LTP creat07).
static TEXT_HOLDS: Spinlock<BTreeMap<(u64, u64), u32>> =
    Spinlock::new(BTreeMap::new());

/// Text-hold key of `inode` (same identity as the lease/open tables).
pub fn inode_text_hold_key(inode: &Inode) -> (u64, u64) {
    inode_lock_key(inode)
}

/// Take one text hold on `key` (exec commit / fork's exe_file dup).
pub fn inode_text_hold_acquire_key(key: (u64, u64)) {
    let mut m = TEXT_HOLDS.lock();
    let c = m.entry(key).or_insert(0);
    *c = c.saturating_add(1);
}

/// Drop one text hold on `key` (last-mm death).
pub fn inode_text_hold_release(key: (u64, u64)) {
    let mut m = TEXT_HOLDS.lock();
    if let Some(c) = m.get_mut(&key) {
        *c = c.saturating_sub(1);
        if *c == 0 {
            m.remove(&key);
        }
    }
}

/// Number of live exec text holds on `inode`. may_open's write leg
/// reads this and fails ETXTBSY while any process still executes the
/// file.
pub fn inode_text_hold_count(inode: &Inode) -> u32 {
    let key = inode_lock_key(inode);
    TEXT_HOLDS.lock().get(&key).copied().unwrap_or(0)
}
