//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Mount point and namespace management
//!
//!
//! Core concepts:
//! - `VfsMount`: Per-mount metadata (used by procfs/rootfs for local state)
//! - `MntFlags`: Mount flags (read-only, noexec, etc.)
//! - `do_mount()`: Unified mount entry point for sys_mount

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use crate::errno;
use crate::sync::spinlock::Spinlock;

/// Global mount registry: (device, mount_point, fs_type, flags_str)
static MOUNT_TABLE: Spinlock<Vec<(String, String, String, String)>> = Spinlock::new(Vec::new());

/// Register a mount in the global table
pub fn register_mount(device: &str, mount_point: &str, fs_type: &str, flags: &str) {
    let mut table = MOUNT_TABLE.lock_irqsave();
    table.retain(|(_, mp, _, _)| mp != mount_point);
    table.push((
        String::from(device),
        String::from(mount_point),
        String::from(fs_type),
        String::from(flags),
    ));
}

/// Get all registered mounts (includes hardcoded rootfs)
pub fn get_mounts() -> Vec<(String, String, String, String)> {
    let mut result = Vec::new();
    // Always include rootfs (registered before allocator is fully stable)
    result.push((String::from("rootfs"), String::from("/"), String::from("rootfs"), String::from("rw")));
    // Add dynamically registered mounts
    let table = MOUNT_TABLE.lock_irqsave();
    for (d, m, f, fl) in table.iter() {
        result.push((d.clone(), m.clone(), f.clone(), fl.clone()));
    }
    result
}

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
// Unified mount entry point
// ============================================================================

/// Perform a real mount: call the filesystem's mount callback, then build
/// the dentry tree via `vfs_mount()`.
///
/// This is the single entry point for both boot-time mounts and sys_mount().
pub fn do_mount(target: &str, fs_type: &str, _flags: u64) -> Result<(), i32> {
    let mnt_flags = MntFlags::new(0);

    match fs_type {
        "ext4" => {
            let fs = crate::fs::ext4::get_ext4_fs()
                .ok_or(errno::Errno::NoSuchDevice.as_neg_i32())?;
            // Mount ext4 if not already mounted
            if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
                crate::fs::ext4::mount_ext4(disk as *const _)
                    .map_err(|e| e as i32)?;
            } else if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
                let disk_ptr = &virtio_dev.disk as *const crate::drivers::blkdev::GenDisk;
                crate::fs::ext4::mount_ext4(disk_ptr)
                    .map_err(|e| e as i32)?;
            } else {
                return Err(errno::Errno::NoSuchDevice.as_neg_i32());
            }
            let _ = fs; // already mounted
            crate::fs::vfs::vfs_mount(target, crate::fs::ext4::create_root_inode(), mnt_flags);
            register_mount("/dev/vda", target, fs_type, "rw");
        }
        "proc" | "procfs" => {
            crate::fs::procfs::mount_procfs()
                .map_err(|e| e as i32)?;
            crate::fs::vfs::vfs_mount(target, crate::fs::procfs::create_root_inode(), mnt_flags);
            register_mount(fs_type, target, "proc", "rw");
        }
        "devfs" | "devtmpfs" => {
            crate::fs::devfs::init();
            if let Some(root_entry) = crate::fs::devfs::get_root_entry() {
                crate::fs::vfs::vfs_mount(target,
                    crate::fs::devfs::create_root_inode(&root_entry), mnt_flags);
                register_mount(fs_type, target, "devtmpfs", "rw");
            } else {
                return Err(errno::Errno::NoSuchDevice.as_neg_i32());
            }
        }
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    }

    Ok(())
}
