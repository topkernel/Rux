//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Mount point and namespace management
//!
//!
//! Core concepts:
//! - `MountTable`: Global mount table with longest-prefix-match lookup
//! - `VfsMount`: Per-mount metadata (used by procfs/rootfs for local state)
//! - `MntFlags`: Mount flags (read-only, noexec, etc.)

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::vfs::FsType;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MntFlags(u64);

impl MntFlags {
    pub const MNT_READONLY: u64 = 0x01;
    pub const MNT_NOATIME: u64 = 0x02;
    pub const MNT_NODIRATIME: u64 = 0x04;
    pub const MNT_SYNCHRONOUS: u64 = 0x08;
    pub const MNT_NOEXEC: u64 = 0x10;
    pub const MNT_NOSUID: u64 = 0x20;
    pub const MNT_NODEV: u64 = 0x40;
    pub const MNT_PRIVATE: u64 = 0x80;
    pub const MNT_SHARED: u64 = 0x100;
    pub const MNT_SLAVE: u64 = 0x200;
    pub const MNT_UNBINDABLE: u64 = 0x400;
    pub const MNT_FORCE: u64 = 0x800;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::MNT_READONLY) != 0
    }

    pub fn is_noexec(&self) -> bool {
        (self.0 & Self::MNT_NOEXEC) != 0
    }

    pub fn is_nosuid(&self) -> bool {
        (self.0 & Self::MNT_NOSUID) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// Per-mount metadata (used by individual filesystems for local state).
/// This coexists with the global MountTable for routing.
#[repr(C)]
pub struct VfsMount {
    pub mnt_id: u64,
    pub mnt_flags: MntFlags,
    pub mnt_mountpoint: Option<Arc<Vec<u8>>>,
    pub mnt_root: Option<Arc<Vec<u8>>>,
    pub mnt_sb: Option<*mut u8>,
    mnt_count: core::sync::atomic::AtomicU64,
    mnt_expired: core::sync::atomic::AtomicU64,
}

unsafe impl Send for VfsMount {}
unsafe impl Sync for VfsMount {}

impl VfsMount {
    pub fn new(mountpoint: Vec<u8>, root: Vec<u8>, flags: MntFlags, sb: Option<*mut u8>) -> Self {
        Self {
            mnt_id: 0,
            mnt_flags: flags,
            mnt_mountpoint: Some(Arc::new(mountpoint)),
            mnt_root: Some(Arc::new(root)),
            mnt_sb: sb,
            mnt_count: core::sync::atomic::AtomicU64::new(1),
            mnt_expired: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn get_superblock(&self) -> Option<*mut u8> {
        self.mnt_sb
    }

    pub fn set_superblock(&mut self, sb: *mut u8) {
        self.mnt_sb = Some(sb);
    }
}

// ============================================================================
// Mount Table — longest-prefix-match routing
// ============================================================================

/// A single mount entry in the global mount table.
pub struct MountEntry {
    /// Mount point path (e.g., "/", "/dev", "/proc")
    pub mountpoint: String,
    /// Filesystem type
    pub fs_type: FsType,
    /// Filesystem-specific data pointer
    pub fs_data: *mut u8,
    /// Mount flags
    pub flags: u64,
}

unsafe impl Send for MountEntry {}
unsafe impl Sync for MountEntry {}

/// Global mount table with longest-prefix-match lookup.
pub struct MountTable {
    entries: Mutex<Vec<MountEntry>>,
}

unsafe impl Send for MountTable {}
unsafe impl Sync for MountTable {}

impl MountTable {
    pub const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Mount a filesystem at the given path.
    /// If a mount already exists at this path, it is replaced (overlay mount).
    pub fn mount(&self, mountpoint: &str, fs_type: FsType, fs_data: *mut u8, flags: u64) {
        let mut entries = self.entries.lock();

        // Replace existing mount at same path
        for entry in entries.iter_mut() {
            if entry.mountpoint == mountpoint {
                entry.fs_type = fs_type;
                entry.fs_data = fs_data;
                entry.flags = flags;
                return;
            }
        }

        entries.push(MountEntry {
            mountpoint: String::from(mountpoint),
            fs_type,
            fs_data,
            flags,
        });
    }

    /// Unmount a filesystem at the given path.
    /// Returns the removed entry's fs_data pointer, or None if not found.
    pub fn umount(&self, mountpoint: &str) -> Option<*mut u8> {
        let mut entries = self.entries.lock();

        let idx = entries.iter().position(|e| e.mountpoint == mountpoint)?;
        let removed = entries.remove(idx);
        Some(removed.fs_data)
    }

    /// Find the best matching mount for a path using longest-prefix match.
    /// Returns the filesystem type and mount point length.
    ///
    /// Example: path="/dev/null" matches mount "/dev" → returns (DevFS, 4)
    ///          path="/proc" matches mount "/proc" → returns (ProcFS, 5)
    ///          path="/bin/ls" matches mount "/" → returns (Ext4, 1)
    pub fn lookup(&self, path: &str) -> Option<(FsType, usize)> {
        let entries = self.entries.lock();

        let mut best_idx: Option<usize> = None;
        let mut best_len: usize = 0;
        let mut best_fs_type = FsType::RootFS;

        for (i, entry) in entries.iter().enumerate() {
            let mnt_len = entry.mountpoint.len();
            // Mount point must be a prefix of path
            if path.len() >= mnt_len && &path[..mnt_len] == entry.mountpoint.as_str() {
                // Root mount "/" matches all absolute paths.
                // Other mounts must be exact match or followed by '/'.
                let is_match = if mnt_len == 1 && entry.mountpoint.as_bytes()[0] == b'/' {
                    true
                } else {
                    path.len() == mnt_len
                        || path.as_bytes().get(mnt_len) == Some(&b'/')
                };
                if is_match && mnt_len > best_len {
                    best_idx = Some(i);
                    best_len = mnt_len;
                    best_fs_type = entry.fs_type;
                }
            }
        }

        best_idx.map(|_| (best_fs_type, best_len))
    }

    /// List all mount entries.
    pub fn list(&self) -> Vec<(String, FsType)> {
        let entries = self.entries.lock();
        entries.iter().map(|e| (e.mountpoint.clone(), e.fs_type)).collect()
    }

    /// Check if any mount with the given fs_type exists.
    pub fn has_mount_of_type(&self, fs_type: FsType) -> bool {
        let entries = self.entries.lock();
        entries.iter().any(|e| e.fs_type == fs_type)
    }
}

/// Global mount table.
static MOUNT_TABLE: MountTable = MountTable::new();

/// Get the global mount table.
pub fn get_mount_table() -> &'static MountTable {
    &MOUNT_TABLE
}

/// Register a filesystem at the given mount point.
pub fn mount_at(mountpoint: &str, fs_type: FsType, fs_data: *mut u8, flags: u64) {
    get_mount_table().mount(mountpoint, fs_type, fs_data, flags);
}

/// Unmount a filesystem at the given mount point.
/// Returns the removed entry's fs_data pointer, or None if not found.
pub fn umount_at(mountpoint: &str) -> Option<*mut u8> {
    get_mount_table().umount(mountpoint)
}
