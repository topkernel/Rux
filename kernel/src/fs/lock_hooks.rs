//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 RELOCATION SEAM (locks family), converted at the fs-rest
//! split: the lock/lease LOGIC and its tables live in the fs crate
//! (`crate::locks` there), while the framework's fs/locks.rs keeps
//! the open/close fast-path gates, the count mirrors and the exec
//! text-hold family. The LockOwner enum and lock-kind constants are
//! re-exported so vfs.rs/file.rs keep their spellings; the per-open/
//! per-close legs and the text-hold census stay DIRECT calls (they hit
//! the framework half), and the COLD paths — the fcntl lock arms and
//! the slow release bodies the gates arm when locks/leases exist —
//! dispatch through the LocksHooks table below (the mount and notify
//! seams' model, as this file's original doc predicted).
//!
//! NOTHING here runs at rust_main time: registration only publishes
//! the plain-Copy table; every row body runs when the framework calls
//! it. The hot open/close paths never dispatch a row on a default boot
//! (the count-mirror gates return first — crate-plan §3a rule 2).

use crate::safety::slot::OnceSlot;

/// The lock-owner identity (re-export; vfs fcntl arms construct it).
pub(crate) use crate::fs::locks::LockOwner;

/// The lock-kind constants (re-export; zero-diff for the fcntl arms).
pub(crate) use crate::fs::locks::{F_RDLCK_KIND, F_UNLCK_KIND, F_WRLCK_KIND};

/// The locks-family cold rows: the fcntl lock arms plus the slow
/// release bodies the framework half's count gates arm. Signatures
/// carry the family's native types (crate-plan §3a rule 3).
#[derive(Clone, Copy)]
pub struct LocksHooks {
    // ---- slow release bodies (armed by the framework half's count
    // gates; run only while locks/leases are actually held) ----
    /// flock table release for a dying description (FLOCK_COUNT != 0).
    pub flock_release_slow: fn(u64),
    /// OFD record-lock release for a dying description.
    pub ofd_release_slow: fn(u64),
    /// POSIX close-event release for the closing tgid on an inode key.
    pub posix_release_slow: fn((u64, u64)),
    /// Lease release for a dying description (LEASE_COUNT != 0).
    pub lease_release_slow: fn(u64),
    /// The lease-break machinery (a conflicting lease exists).
    pub break_lease_slow: fn(&crate::fs::inode::Inode, bool) -> Result<(), crate::errno::Errno>,
    // ---- fcntl arms (cold: only lock-taking syscalls reach them) ----
    /// F_GETLK conflict probe.
    pub posix_test_lock: fn(&crate::fs::file::File, LockOwner, bool, u64, u64) -> Option<crate::fs::locks::LockConflict>,
    /// F_SETLK/W core (kind is a F_*_KIND byte).
    pub posix_set_lock: fn(&crate::fs::file::File, LockOwner, u8, u64, u64, bool) -> Result<(), crate::errno::Errno>,
    /// F_SETLEASE / F_GETLEASE.
    pub set_lease: fn(&crate::fs::file::File, i16) -> Result<(), crate::errno::Errno>,
    pub get_lease: fn(&crate::fs::file::File) -> i16,
}

impl LocksHooks {
    /// The pre-registration table: no locks exist without the crate, so
    /// every slow body is a no-op and every setter reports EINVAL (the
    /// pre-flip behavior of an unregistered family).
    pub const EMPTY: LocksHooks = LocksHooks {
        flock_release_slow: |_| {},
        ofd_release_slow: |_| {},
        posix_release_slow: |_| {},
        lease_release_slow: |_| {},
        break_lease_slow: |_, _| Ok(()),
        posix_test_lock: |_, _, _, _, _| None,
        posix_set_lock: |_, _, _, _, _, _| Err(crate::errno::Errno::InvalidArgument),
        set_lease: |_, _| Err(crate::errno::Errno::InvalidArgument),
        // F_UNLCK (i16): no lease is ever held.
        get_lease: |_| 2,
    };
}

pub static LOCKS_HOOKS: OnceSlot<LocksHooks> = OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins).
pub fn register_locks_hooks(hooks: LocksHooks) {
    let _ = LOCKS_HOOKS.publish(hooks);
}

#[inline]
pub fn lock_rows() -> &'static LocksHooks {
    LOCKS_HOOKS.get().unwrap_or(&LocksHooks::EMPTY)
}

/// Register an inode-open for lease tracking (File::open paths).
/// Direct call: the framework half's per-open inode atomics.
pub(crate) fn inode_open_register(inode: &crate::fs::inode::Inode, write: bool) {
    crate::fs::locks::inode_open_register(inode, write);
}

/// Unregister on close (File::drop path). Direct call: the framework
/// half's atomics + count gate; the lease-release leg rides the row.
pub(crate) fn inode_open_unregister(file: &crate::fs::file::File) {
    crate::fs::locks::inode_open_unregister(file);
}

/// Release flock-owned locks on close. Direct call: the framework
/// half's FLOCK_COUNT gate; the table work rides the row.
pub(crate) fn flock_release_file(file_id: u64) {
    crate::fs::locks::flock_release_file(file_id);
}

/// Release OFD-owned locks on close. Same shape as flock_release_file.
pub(crate) fn ofd_release_file(file_id: u64) {
    crate::fs::locks::ofd_release_file(file_id);
}

/// Break a lease on an inode (write-open conflict paths). Direct call:
/// the framework half's LEASE_COUNT gate; the break machinery rides
/// the row.
pub(crate) fn break_lease(
    inode: &crate::fs::inode::Inode,
    writer: bool,
) -> Result<(), crate::errno::Errno> {
    crate::fs::locks::break_lease(inode, writer)
}

/// Posix fcntl test-lock path (F_GETLK conflict probe). Cold row.
pub(crate) fn posix_test_lock(
    file: &crate::fs::file::File,
    owner: LockOwner,
    exclusive: bool,
    start: u64,
    end: u64,
) -> Option<crate::fs::locks::LockConflict> {
    (lock_rows().posix_test_lock)(file, owner, exclusive, start, end)
}

/// Posix fcntl set-lock path (F_SETLK/W core; kind is a F_*_KIND byte).
/// Cold row.
pub(crate) fn posix_set_lock(
    file: &crate::fs::file::File,
    owner: LockOwner,
    kind: u8,
    start: u64,
    end: u64,
    wait: bool,
) -> Result<(), crate::errno::Errno> {
    (lock_rows().posix_set_lock)(file, owner, kind, start, end, wait)
}

/// Lease set/query (F_SETLEASE/F_GETLEASE). Cold rows.
pub(crate) fn set_lease(
    file: &crate::fs::file::File,
    lease_type: i16,
) -> Result<(), crate::errno::Errno> {
    (lock_rows().set_lease)(file, lease_type)
}
pub(crate) fn get_lease(file: &crate::fs::file::File) -> i16 {
    (lock_rows().get_lease)(file)
}

/// Text-hold census (the write-open conflict check). Direct call: the
/// exec text-hold family never left the framework.
pub(crate) fn inode_text_hold_count(inode: &crate::fs::inode::Inode) -> u32 {
    crate::fs::locks::inode_text_hold_count(inode)
}
