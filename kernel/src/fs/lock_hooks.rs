//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 RELOCATION SEAM (locks family): the file-lease/lock lifecycle
//! hooks. The LockOwner enum and lock-kind constants are re-exported so
//! vfs.rs/file.rs keep their spellings; the close-path release fns ride
//! this table. At the crate flip, locks' implementations move into its
//! crate and these trampolines become registration slots (the mount and
//! notify seams' model).

/// The lock-owner identity (re-export; vfs fcntl arms construct it).
pub(crate) use crate::fs::locks::LockOwner;

/// The lock-kind constants (re-export; zero-diff for the fcntl arms).
pub(crate) use crate::fs::locks::{F_RDLCK_KIND, F_UNLCK_KIND, F_WRLCK_KIND};

/// Register an inode-open for lease tracking (File::open paths).
pub(crate) fn inode_open_register(inode: &crate::fs::inode::Inode, write: bool) {
    crate::fs::locks::inode_open_register(inode, write);
}

/// Unregister on close (File::drop path).
pub(crate) fn inode_open_unregister(file: &crate::fs::file::File) {
    crate::fs::locks::inode_open_unregister(file);
}

/// Release flock-owned locks on close.
pub(crate) fn flock_release_file(file_id: u64) {
    crate::fs::locks::flock_release_file(file_id);
}

/// Release OFD-owned locks on close.
pub(crate) fn ofd_release_file(file_id: u64) {
    crate::fs::locks::ofd_release_file(file_id);
}

/// Break a lease on an inode (write-open conflict paths).
pub(crate) fn break_lease(
    inode: &crate::fs::inode::Inode,
    writer: bool,
) -> Result<(), crate::errno::Errno> {
    crate::fs::locks::break_lease(inode, writer)
}

/// Posix fcntl test-lock path (F_GETLK conflict probe).
pub(crate) fn posix_test_lock(
    file: &crate::fs::file::File,
    owner: LockOwner,
    exclusive: bool,
    start: u64,
    end: u64,
) -> Option<crate::fs::locks::LockConflict> {
    crate::fs::locks::posix_test_lock(file, owner, exclusive, start, end)
}

/// Posix fcntl set-lock path (F_SETLK/W core; kind is a F_*_KIND byte).
pub(crate) fn posix_set_lock(
    file: &crate::fs::file::File,
    owner: LockOwner,
    kind: u8,
    start: u64,
    end: u64,
    wait: bool,
) -> Result<(), crate::errno::Errno> {
    crate::fs::locks::posix_set_lock(file, owner, kind, start, end, wait)
}

/// Lease set/query (F_SETLEASE/F_GETLEASE).
pub(crate) fn set_lease(
    file: &crate::fs::file::File,
    lease_type: i16,
) -> Result<(), crate::errno::Errno> {
    crate::fs::locks::set_lease(file, lease_type)
}
pub(crate) fn get_lease(file: &crate::fs::file::File) -> i16 {
    crate::fs::locks::get_lease(file)
}

/// Text-hold census (the write-open conflict check).
pub(crate) fn inode_text_hold_count(inode: &crate::fs::inode::Inode) -> u32 {
    crate::fs::locks::inode_text_hold_count(inode)
}
