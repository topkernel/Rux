//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Superblock and Filesystem Type Management
//!
//!
//! Core concepts:
//! - `struct super_block`: Superblock, represents a mounted filesystem
//! - `struct file_system_type`: Filesystem type, used for registration and mounting
//! - `struct vfsmount`: Mount point, represents filesystem position in namespace

use crate::errno;
use alloc::sync::Arc;
use spin::Mutex;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SuperBlockFlags(u64);

impl SuperBlockFlags {
    /// Read-only mount
    pub const SB_RDONLY: u64 = 1;
    /// Don't update atime
    pub const SB_NOATIME: u64 = 1 << 5;
    /// Don't update atime/mtime/ctime
    pub const SB_NODIRATIME: u64 = 1 << 6;
    /// Force synchronous writes
    pub const SB_SYNCHRONOUS: u64 = 1 << 7;
    /// Disallow mandatory locking
    pub const SB_MANDLOCK: u64 = 1 << 8;
    /// Don't write to device
    pub const SB_DIRSYNC: u64 = 1 << 9;
    /// Don't update atime
    pub const SB_NOSEC: u64 = 1 << 10;
    /// Active mount
    pub const SB_ACTIVE: u64 = 1 << 11;
    /// Currently writing
    pub const SB_WRITERS: u64 = 1 << 12;

    pub fn new(flags: u64) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::SB_RDONLY) != 0
    }

    pub fn is_active(&self) -> bool {
        (self.0 & Self::SB_ACTIVE) != 0
    }

    pub fn bits(&self) -> u64 {
        self.0
    }
}

#[repr(C)]
pub struct SuperBlock {
    /// Filesystem flags
    pub s_flags: SuperBlockFlags,
    /// Block size
    pub s_blocksize: usize,
    /// Block size bits
    pub s_blocksize_bits: u8,
    /// Filesystem magic number
    pub s_magic: u32,
    /// Maximum links
    pub s_max_links: u32,
    /// Root inode
    pub s_root: Option<Arc<()>>,
    /// Filesystem type
    pub s_type: Option<&'static FileSystemType>,
    /// Mount options
    pub s_options: Option<Arc<()>>,
    /// Private data (for specific filesystem)
    pub s_fs_info: Option<*mut u8>,
}

unsafe impl Send for SuperBlock {}
unsafe impl Sync for SuperBlock {}

impl SuperBlock {
    /// Create new superblock
    pub fn new(blocksize: usize, magic: u32) -> Self {
        // Calculate block size bits
        let mut bits = 0u8;
        let mut size = blocksize;
        while size > 1 {
            size >>= 1;
            bits += 1;
        }

        Self {
            s_flags: SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY),
            s_blocksize: blocksize,
            s_blocksize_bits: bits,
            s_magic: magic,
            s_max_links: 0,
            s_root: None,
            s_type: None,
            s_options: None,
            s_fs_info: None,
        }
    }

    /// Set filesystem type
    pub fn set_type(&mut self, fs_type: &'static FileSystemType) {
        self.s_type = Some(fs_type);
    }

    /// Set private data
    pub fn set_fs_info(&mut self, info: *mut u8) {
        self.s_fs_info = Some(info);
    }

    /// Set flags
    pub fn set_flags(&mut self, flags: SuperBlockFlags) {
        self.s_flags = flags;
    }
}

pub struct FsContext<'a> {
    /// Source device
    pub source: Option<&'a str>,
    /// Mount target
    pub target: Option<&'a str>,
    /// Mount flags
    pub ms_flags: u64,
    /// Data options
    pub data: Option<&'a str>,
}

impl<'a> FsContext<'a> {
    /// Create new mount context
    pub fn new(
        source: Option<&'a str>,
        target: Option<&'a str>,
        ms_flags: u64,
    ) -> Self {
        Self {
            source,
            target,
            ms_flags,
            data: None,
        }
    }
}

#[repr(C)]
pub struct FileSystemType {
    /// Filesystem name
    pub name: &'static str,
    /// Get superblock (called on mount)
    pub mount: Option<unsafe extern "C" fn(&FsContext<'_>) -> Result<*mut SuperBlock, i32>>,
    /// Kill superblock (called on unmount)
    pub kill_sb: Option<unsafe extern "C" fn(*mut SuperBlock)>,
    /// Filesystem flags
    pub fs_flags: u64,
}

impl FileSystemType {
    /// Create new filesystem type
    pub const fn new(
        name: &'static str,
        mount: Option<unsafe extern "C" fn(&FsContext<'_>) -> Result<*mut SuperBlock, i32>>,
        kill_sb: Option<unsafe extern "C" fn(*mut SuperBlock)>,
        fs_flags: u64,
    ) -> Self {
        Self {
            name,
            mount,
            kill_sb,
            fs_flags,
        }
    }

    /// Mount filesystem
    ///
    pub unsafe fn mount_fs(
        &self,
        source: Option<&str>,
        target: Option<&str>,
        flags: u64,
    ) -> Result<*mut SuperBlock, i32> {
        // Create mount context
        let fc = FsContext::new(source, target, flags);

        // Call filesystem-specific mount function
        if let Some(mount_fn) = self.mount {
            mount_fn(&fc)
        } else {
            Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
        }
    }

    /// Unmount filesystem
    ///
    pub unsafe fn kill_super(&self, sb: *mut SuperBlock) {
        if let Some(kill_fn) = self.kill_sb {
            kill_fn(sb);
        }
    }
}

struct FsRegistry {
    /// Filesystem type list
    fs_types: Mutex<[Option<&'static FileSystemType>; 32]>,
}

unsafe impl Send for FsRegistry {}
unsafe impl Sync for FsRegistry {}

impl FsRegistry {
    pub const fn new() -> Self {
        Self {
            fs_types: Mutex::new([None; 32]),
        }
    }

    /// Register filesystem type
    ///
    pub fn register(&self, fs_type: &'static FileSystemType) -> Result<(), i32> {
        let mut registry = self.fs_types.lock();

        // Find free slot
        for i in 0..32 {
            if registry[i].is_none() {
                registry[i] = Some(fs_type);
                return Ok(());
            }
        }

        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// Unregister filesystem type
    ///
    pub fn unregister(&self, fs_type: &'static FileSystemType) -> Result<(), i32> {
        let mut registry = self.fs_types.lock();

        // Find and remove filesystem type
        for i in 0..32 {
            if let Some(ft) = registry[i] {
                if core::ptr::eq(ft, fs_type) {
                    registry[i] = None;
                    return Ok(());
                }
            }
        }

        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }

    /// Find filesystem type
    ///
    pub fn get(&self, name: &str) -> Option<&'static FileSystemType> {
        let registry = self.fs_types.lock();

        for i in 0..32 {
            if let Some(fs_type) = registry[i] {
                if fs_type.name == name {
                    return Some(fs_type);
                }
            }
        }

        None
    }
}

static FS_REGISTRY: FsRegistry = FsRegistry::new();

pub fn register_filesystem(fs_type: &'static FileSystemType) -> Result<(), i32> {
    FS_REGISTRY.register(fs_type)
}

pub fn unregister_filesystem(fs_type: &'static FileSystemType) -> Result<(), i32> {
    FS_REGISTRY.unregister(fs_type)
}

pub fn get_fs_type(name: &str) -> Option<&'static FileSystemType> {
    FS_REGISTRY.get(name)
}

pub unsafe fn do_mount(
    dev_name: Option<&str>,
    dir_name: Option<&str>,
    type_name: &str,
    flags: u64,
    _data: Option<&str>,
) -> Result<(), i32> {
    // Find filesystem type
    let fs_type = get_fs_type(type_name).ok_or(-2_i32)?;  // ENOENT

    // Mount filesystem
    let _sb = fs_type.mount_fs(dev_name, dir_name, flags)?;

    // TODO: Create vfsmount structure
    // TODO: Add mount point to namespace

    Ok(())
}

pub unsafe fn do_umount(_target: &str, _flags: u64) -> Result<(), i32> {
    // TODO: Find mount point
    // TODO: Check if mount point is in use
    // TODO: Call filesystem's kill_sb

    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test filesystem type
    extern "C" fn test_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
        // Simply return a new superblock
        let sb = Box::new(SuperBlock::new(4096, 0x1234));
        Ok(Box::into_raw(sb) as *mut SuperBlock)
    }

    extern "C" fn test_kill_sb(_sb: *mut SuperBlock) {
        // Simply do nothing
    }

    #[test]
    fn test_fs_registry() {
        // Create test filesystem type
        let test_fs = FileSystemType::new(
            "testfs",
            Some(test_mount),
            Some(test_kill_sb),
            0,
        );

        // Register filesystem
        assert!(register_filesystem(&test_fs).is_ok());

        // Find filesystem
        assert!(get_fs_type("testfs").is_some());
        assert!(get_fs_type("nonexistent").is_none());

        // Unregister filesystem
        assert!(unregister_filesystem(&test_fs).is_ok());
        assert!(get_fs_type("testfs").is_none());
    }

    #[test]
    fn test_superblock_flags() {
        let flags = SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY | SuperBlockFlags::SB_ACTIVE);
        assert!(flags.is_readonly());
        assert!(flags.is_active());

        let flags2 = SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY);
        assert!(flags2.is_readonly());
        assert!(!flags2.is_active());
    }
}
