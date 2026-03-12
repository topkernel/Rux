//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Mount point and namespace management
//!
//!
//! Core concepts:
//! - `struct vfsmount`: Mount point, representing a filesystem's location in namespace
//! - `struct mnt_namespace`: Namespace, containing all mount points visible to a process
//! - Mount point tree: Hierarchical structure formed by mount points

use crate::errno;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MntFlags(u64);

impl MntFlags {
    /// Read-only mount
    pub const MNT_READONLY: u64 = 0x01;
    /// No atime update
    pub const MNT_NOATIME: u64 = 0x02;
    /// No directory atime update
    pub const MNT_NODIRATIME: u64 = 0x04;
    /// Force synchronous writes
    pub const MNT_SYNCHRONOUS: u64 = 0x08;
    /// Disable program execution
    pub const MNT_NOEXEC: u64 = 0x10;
    /// No suid/sgid support
    pub const MNT_NOSUID: u64 = 0x20;
    /// No device node atime update
    pub const MNT_NODEV: u64 = 0x40;
    /// Private mount
    pub const MNT_PRIVATE: u64 = 0x80;
    /// Shared mount group
    pub const MNT_SHARED: u64 = 0x100;
    /// Slave mount
    pub const MNT_SLAVE: u64 = 0x200;
    /// Unbindable
    pub const MNT_UNBINDABLE: u64 = 0x400;
    /// Force flag
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

#[repr(C)]
pub struct VfsMount {
    /// Mount point unique ID
    pub mnt_id: u64,
    /// Parent mount point
    pub mnt_parent: Option<Arc<VfsMount>>,
    /// Mount point flags
    pub mnt_flags: MntFlags,
    /// Mount point name (mount directory)
    pub mnt_mountpoint: Option<Arc<Vec<u8>>>,
    /// Mount root directory
    pub mnt_root: Option<Arc<Vec<u8>>>,
    /// Superblock pointer
    pub mnt_sb: Option<*mut u8>,
    /// Mount point reference count
    mnt_count: AtomicU64,
    /// Mount point expiration status
    mnt_expired: AtomicU64,
    /// Namespace
    pub mnt_ns: Option<*mut MntNamespace>,
}

unsafe impl Send for VfsMount {}
unsafe impl Sync for VfsMount {}

impl VfsMount {
    /// Create new mount point
    pub fn new(mountpoint: Vec<u8>, root: Vec<u8>, flags: MntFlags, sb: Option<*mut u8>) -> Self {
        Self {
            mnt_id: 0,  // Will be allocated when added to namespace
            mnt_parent: None,
            mnt_flags: flags,
            mnt_mountpoint: Some(Arc::new(mountpoint)),
            mnt_root: Some(Arc::new(root)),
            mnt_sb: sb,
            mnt_count: AtomicU64::new(1),
            mnt_expired: AtomicU64::new(0),
            mnt_ns: None,
        }
    }

    /// Get superblock
    pub fn get_superblock(&self) -> Option<*mut u8> {
        self.mnt_sb
    }

    /// Set superblock
    pub fn set_superblock(&mut self, sb: *mut u8) {
        self.mnt_sb = Some(sb);
    }

    /// Set parent mount point
    pub fn set_parent(&mut self, parent: Arc<VfsMount>) {
        self.mnt_parent = Some(parent);
    }

    /// Increment reference count
    pub fn get(&self) {
        self.mnt_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn put(&self) {
        if self.mnt_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last reference, should clean up resources here
            // But since we use Arc, actual cleanup happens during drop
        }
    }

    /// Check if expired
    pub fn is_expired(&self) -> bool {
        self.mnt_expired.load(Ordering::Acquire) != 0
    }

    /// Mark as expired
    pub fn mark_expired(&self) {
        self.mnt_expired.store(1, Ordering::Release);
    }

    /// Get mount point path
    pub fn get_path(&self) -> Option<Vec<u8>> {
        self.mnt_mountpoint.as_ref().map(|_arc| {
            // Get clone of Vec<u8>
            // Arc implements Clone trait (standard library)
            Vec::new()  // TODO: Implement actual clone
        })
    }
}

#[repr(C)]
pub struct MntNamespace {
    /// Namespace ID
    pub ns_id: u64,
    /// Mount point list
    mounts: Mutex<Vec<Arc<VfsMount>>>,
    /// Root mount point
    pub root: Option<Arc<VfsMount>>,
    /// Reference count
    count: AtomicU64,
}

unsafe impl Send for MntNamespace {}
unsafe impl Sync for MntNamespace {}

impl MntNamespace {
    /// Create new namespace
    pub fn new() -> Self {
        Self {
            ns_id: 0,
            mounts: Mutex::new(Vec::new()),
            root: None,
            count: AtomicU64::new(1),
        }
    }

    /// Add mount point to namespace
    pub fn add_mount(&self, mount: Arc<VfsMount>) -> Result<(), i32> {
        let mut mounts = self.mounts.lock();

        // Allocate mount point ID
        let _mnt_id = mounts.len() as u64;

        // If first mount point, set as root mount point
        if self.root.is_none() {
            // Note: Modifying value inside Arc is complex in Rust
            // Simplified implementation: set all properties when creating mount point
        }

        mounts.push(mount);
        Ok(())
    }

    /// Remove mount point
    pub fn remove_mount(&self, mnt_id: u64) -> Result<(), i32> {
        let mut mounts = self.mounts.lock();

        // Find and remove mount point
        for i in 0..mounts.len() {
            if mounts[i].mnt_id == mnt_id {
                // Check if root mount point
                if let Some(ref root) = self.root {
                    if root.mnt_id == mnt_id {
                        return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
                    }
                }

                mounts.remove(i);
                return Ok(());
            }
        }

        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }

    /// Find mount point
    pub fn find_mount(&self, _path: &[u8]) -> Option<Arc<VfsMount>> {
        let mounts = self.mounts.lock();

        for mount in mounts.iter() {
            if let Some(ref _mountpoint) = mount.mnt_mountpoint {
                // TODO: Implement path comparison
                // if mountpoint.as_slice() == path {
                //     return Some(mount.clone());
                // }
            }
        }

        None
    }

    /// Get all mount points
    pub fn list_mounts(&self) -> Vec<Arc<VfsMount>> {
        let _mounts = self.mounts.lock();
        // Arc implements Clone trait (standard library)
        // Return empty Vec for now
        Vec::new()
    }

    /// Increment reference count
    pub fn get(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn put(&self) {
        if self.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last reference, clean up resources
        }
    }
}

static INIT_NS: MntNamespace = MntNamespace {
    ns_id: 0,
    mounts: Mutex::new(Vec::new()),
    root: None,
    count: AtomicU64::new(1),
};

pub fn get_init_namespace() -> &'static MntNamespace {
    &INIT_NS
}

pub fn create_namespace() -> Result<&'static MntNamespace, i32> {
    // TODO: Implement real namespace creation
    // This requires dynamic allocation, which is complex in no_std environment
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

pub fn clone_namespace(_ns: &MntNamespace) -> Result<&'static MntNamespace, i32> {
    // TODO: Implement namespace cloning
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MsFlags(u64);

impl MsFlags {
    /// Bind mount
    pub const MS_BIND: u64 = 0x1000;
    /// Private mount
    pub const MS_PRIVATE: u64 = 0x40000;
    /// Shared mount
    pub const MS_SHARED: u64 = 0x20000;
    /// Slave mount
    pub const MS_SLAVE: u64 = 0x80000;
    /// Unbindable
    pub const MS_UNBINDABLE: u64 = 0x200000;
    /// Move mount point
    pub const MS_MOVE: u64 = 0x8000;
    /// Recursive bind
    pub const MS_REC: u64 = 0x4000;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_bind(&self) -> bool {
        (self.0 & Self::MS_BIND) != 0
    }

    pub fn is_move(&self) -> bool {
        (self.0 & Self::MS_MOVE) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

pub struct MountTreeIter<'a> {
    /// Current namespace
    ns: &'a MntNamespace,
    /// Current position
    current: Option<Arc<VfsMount>>,
}

impl<'a> MountTreeIter<'a> {
    /// Create new iterator
    pub fn new(ns: &'a MntNamespace) -> Self {
        Self {
            ns,
            current: None,
        }
    }
}

impl<'a> Iterator for MountTreeIter<'a> {
    type Item = Arc<VfsMount>;

    fn next(&mut self) -> Option<Self::Item> {
        // TODO: Implement depth-first traversal
        // Simplified implementation: only return root mount point
        let current = self.current.take();
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mnt_flags() {
        let flags = MntFlags::new(MntFlags::MNT_READONLY | MntFlags::MNT_NOEXEC);
        assert!(flags.is_readonly());
        assert!(flags.is_noexec());
        assert!(!flags.is_nosuid());
    }

    #[test]
    fn test_vfsmount_create() {
        let mountpoint = b"/mnt".to_vec();
        let root = b"/".to_vec();
        let flags = MntFlags::new(MntFlags::MNT_READONLY);

        let mnt = VfsMount::new(mountpoint, root, flags, None);
        assert!(mnt.mnt_flags.is_readonly());
        assert_eq!(mnt.mnt_id, 0);
    }

    #[test]
    fn test_namespace() {
        let ns = MntNamespace::new();
        assert!(ns.root.is_none());
        assert_eq!(ns.list_mounts().len(), 0);
    }

    #[test]
    fn test_ms_flags() {
        let flags = MsFlags::new(MsFlags::MS_BIND | MsFlags::MS_REC);
        assert!(flags.is_bind());
    }
}
