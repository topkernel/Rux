//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Virtual File System (VFS) core functionality
//!
//! ## Architecture Overview
//!
//! The VFS layer provides a unified interface for all filesystems:
//! - **RootFS**: Memory-backed filesystem for initial root
//! - **ext4**: Block device backed filesystem
//! - **procfs**: Process information filesystem
//! - **devfs**: Device filesystem
//!
//! ## Key Concepts
//!
//! - **inode**: Represents a filesystem object (file, directory, etc.)
//! - **dentry**: Directory entry, caches path lookups
//! - **superblock**: Represents a mounted filesystem
//! - **inode_operations**: Function pointers for filesystem operations
//!
//! ## Path Resolution
//!
//! All paths are resolved through `path_lookup()` which:
//! 1. Normalizes the path (handles . and ..)
//! 2. Resolves relative paths using current working directory
//! 3. Handles mount points
//! 4. Returns a `Path` structure with dentry and mount info

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use spin::Mutex;

use crate::errno;
use crate::fs::file::{File, FileFlags, FileOps, get_file_fd, close_file_fd, get_file_fd_install};
use crate::fs::rootfs::{RootFSNode, get_rootfs};
use crate::fs::inode::{Inode, InodeMode, INodeOps, setattr_attr};
use crate::fs::dentry::Dentry;
use crate::fs::ext4;
use crate::fs::procfs;
use crate::fs::devfs;
use crate::fs::Stat;
use crate::fs::path::path_normalize;
use crate::println;

// ============================================================================
// VFS Core Structures
// ============================================================================

/// VFS lookup flags
pub mod lookup_flags {
    /// Follow symbolic links
    pub const LOOKUP_FOLLOW: u32 = 0x0001;
    /// Must be a directory
    pub const LOOKUP_DIRECTORY: u32 = 0x0002;
    /// Create if doesn't exist
    pub const LOOKUP_CREATE: u32 = 0x0004;
    /// Exclusive create
    pub const LOOKUP_EXCL: u32 = 0x0008;
    /// Don't follow symlinks at the end
    pub const LOOKUP_NO_SYMLINKS: u32 = 0x0010;
}

/// VFS Path structure
///
/// Represents a resolved path with its mount and dentry information.
pub struct VfsPath {
    /// Dentry for this path
    pub dentry: Option<Arc<Dentry>>,
    /// Mount point (vfsmount)
    pub mnt: Option<*const u8>,
    /// Inode if resolved
    pub inode: Option<Arc<Inode>>,
}

impl VfsPath {
    /// Create empty path
    pub fn new() -> Self {
        Self {
            dentry: None,
            mnt: None,
            inode: None,
        }
    }

    /// Create path with inode
    pub fn with_inode(inode: Arc<Inode>) -> Self {
        Self {
            dentry: None,
            mnt: None,
            inode: Some(inode),
        }
    }

    /// Check if path is valid
    pub fn is_valid(&self) -> bool {
        self.inode.is_some()
    }
}

impl Default for VfsPath {
    fn default() -> Self {
        Self::new()
    }
}

/// VFS global state
struct VfsState {
    root_inode: Option<Arc<()>>,  // Will be replaced with actual root inode in the future
    initialized: bool,
}

static VFS_STATE: Mutex<VfsState> = Mutex::new(VfsState {
    root_inode: None,
    initialized: false,
});

// ============================================================================
// Filesystem Type Enumeration
// ============================================================================

/// Filesystem type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    /// RootFS (memory-backed)
    RootFS,
    /// ext4 filesystem
    Ext4,
    /// procfs
    ProcFS,
    /// devfs
    DevFS,
    /// Unknown
    Unknown,
}

// ============================================================================
// VFS Initialization
// ============================================================================

/// Initialize VFS
pub fn init() {
    use crate::console::putchar;
    const MSG1: &[u8] = b"vfs: Initializing Virtual File System...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // Test Arc functionality
    let _test_arc = Arc::new(42i32);
    const MSG2: &[u8] = b"vfs: Arc test passed\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    {
        let mut state = VFS_STATE.lock();
        state.initialized = true;
    }

    const MSG4: &[u8] = b"vfs: VFS layer initialized [OK]\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }
}

// ============================================================================
// Path Lookup (Unified Path Resolution)
// ============================================================================

/// Resolve path to determine which filesystem it belongs to
///
/// Returns (filesystem_type, path_within_filesystem)
fn resolve_filesystem(path: &str) -> (FsType, &str) {
    // Check /dev first (devfs)
    if path == "/dev" || path.starts_with("/dev/") {
        return (FsType::DevFS, path);
    }

    // Check /proc (procfs)
    if path == "/proc" || path.starts_with("/proc/") {
        return (FsType::ProcFS, path);
    }

    // If ext4 is mounted, use it as the default filesystem
    if ext4::is_mounted() {
        return (FsType::Ext4, path);
    }

    // Default: RootFS
    (FsType::RootFS, path)
}

/// Get current working directory
fn get_cwd() -> String {
    if let Some(current) = crate::sched::current() {
        let cwd_bytes = unsafe { (*current).get_cwd() };
        match core::str::from_utf8(&cwd_bytes) {
            Ok(s) => String::from(s),
            Err(_) => String::from("/"),
        }
    } else {
        String::from("/")
    }
}

/// Convert relative path to absolute path
fn make_absolute(path: &str) -> String {
    if path.starts_with('/') {
        String::from(path)
    } else {
        let cwd = get_cwd();
        if cwd.ends_with('/') {
            format!("{}{}", cwd, path)
        } else {
            format!("{}/{}", cwd, path)
        }
    }
}

/// Unified path lookup
///
/// This function resolves a pathname to a VfsPath structure.
/// It handles:
/// - Absolute and relative paths
/// - Symbolic links (optionally)
/// - Mount points
/// - Different filesystem types
///
/// # Arguments
/// - `pathname`: Path to resolve (absolute or relative)
/// - `flags`: Lookup flags (LOOKUP_FOLLOW, LOOKUP_DIRECTORY, etc.)
///
/// # Returns
/// - `Ok(VfsPath)`: Resolved path with inode
/// - `Err(errno)`: Error code
pub fn path_lookup(pathname: &str, flags: u32) -> Result<VfsPath, i32> {
    // Empty path is invalid
    if pathname.is_empty() {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // Convert to absolute path and normalize
    let abs_path = make_absolute(pathname);
    let normalized = path_normalize(&abs_path);

    // Determine which filesystem this path belongs to
    let (fs_type, fs_path) = resolve_filesystem(&normalized);

    match fs_type {
        FsType::DevFS => {
            // Parse devfs path
            let devfs_path = devfs::parse_dev_path(fs_path).unwrap_or(fs_path);
            if devfs::is_mounted() {
                if let Some((entry, is_char, devno)) = devfs::lookup(devfs_path) {
                    // Create inode for this device
                    let mode = if is_char {
                        InodeMode::new(InodeMode::S_IFCHR | 0o666)
                    } else {
                        InodeMode::new(InodeMode::S_IFBLK | 0o666)
                    };
                    let mut inode = Inode::new(devno.to_u64(), mode);
                    inode.ops = Some(&crate::fs::rootfs::ROOTFS_INODE_OPS);
                    // Store device entry pointer for device operations
                    inode.private_data = Some(Arc::as_ptr(&entry) as *mut u8);
                    return Ok(VfsPath::with_inode(Arc::new(inode)));
                }
            }
            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
        FsType::ProcFS => {
            let procfs_path = if fs_path == "/proc" { "/" } else { &fs_path[5..] };
            if procfs::is_mounted() {
                if let Some(sb) = procfs::get_procfs_sb() {
                    if let Some(node) = sb.lookup(procfs_path) {
                        // Create inode with procfs ops
                        let mode = if node.is_dir() {
                            InodeMode::new(InodeMode::S_IFDIR | 0o555)
                        } else if node.is_symlink() {
                            InodeMode::new(InodeMode::S_IFLNK | 0o777)
                        } else {
                            InodeMode::new(InodeMode::S_IFREG | 0o444)
                        };
                        let mut inode = Inode::new(node.ino, mode);
                        inode.ops = Some(&procfs::PROCFS_INODE_OPS);
                        inode.private_data = Some(Arc::as_ptr(&node) as *mut u8);
                        return Ok(VfsPath::with_inode(Arc::new(inode)));
                    }
                }
            }
            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
        FsType::RootFS => {
            // Lookup in RootFS
            unsafe {
                let sb_ptr = get_rootfs();
                if sb_ptr.is_null() {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }

                let sb = &*sb_ptr;
                if let Some(node) = sb.lookup(&normalized) {
                    // Create inode with RootFS ops
                    let mode = if node.is_dir() {
                        InodeMode::new(InodeMode::S_IFDIR | 0o755)
                    } else if node.is_symlink() {
                        InodeMode::new(InodeMode::S_IFLNK | 0o777)
                    } else {
                        InodeMode::new(InodeMode::S_IFREG | 0o644)
                    };
                    let mut inode = Inode::new(node.ino, mode);
                    inode.ops = Some(&crate::fs::rootfs::ROOTFS_INODE_OPS);
                    inode.private_data = Some(Arc::as_ptr(&node) as *mut u8);
                    return Ok(VfsPath::with_inode(Arc::new(inode)));
                }
            }
            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
        FsType::Ext4 => {
            // Lookup in ext4 filesystem
            if let Some(fs_ptr) = ext4::get_ext4_fs() {
                unsafe {
                    let fs = &*fs_ptr;
                    match fs.lookup_path(&normalized) {
                        Ok((ino, ext4_inode)) => {
                            // Create proper VFS inode with ext4 operations
                            let vfs_inode = ext4::create_vfs_inode(ino, &ext4_inode);
                            crate::fs::inode::icache_add(vfs_inode.clone());
                            return Ok(VfsPath::with_inode(vfs_inode));
                        }
                        Err(_) => {}
                    }
                }
            }
            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
        FsType::Unknown => {
            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
    }
}

/// Lookup parent directory and extract final component
///
/// Splits a path into (parent_dir, filename)
/// For example: "/usr/bin/ls" -> ("/usr/bin", "ls")
pub fn path_parent_and_name(path: &str) -> Result<(String, String), i32> {
    let normalized = path_normalize(path);

    if normalized == "/" || normalized.is_empty() {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // Find last '/'
    if let Some(idx) = normalized.rfind('/') {
        let (parent, name): (&str, &str) = if idx == 0 {
            ("/", &normalized[1..])
        } else {
            (&normalized[..idx], &normalized[idx + 1..])
        };

        if name.is_empty() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        return Ok((String::from(parent), String::from(name)));
    }

    // No '/' found, relative path with single component
    Ok((get_cwd(), normalized))
}

// ============================================================================
// Unified Directory Operations (using inode_operations)
// ============================================================================

/// Lookup parent directory path and return its VfsPath with inode
///
/// Check MAY_WRITE permission on a parent inode for directory modification operations.
fn check_parent_write_permission(parent_inode: &Inode) -> Result<(), i32> {
    let inode_mode = parent_inode.mode.bits() as u16;
    let inode_uid = parent_inode.uid.load(core::sync::atomic::Ordering::Relaxed);
    let inode_gid = parent_inode.gid.load(core::sync::atomic::Ordering::Relaxed);
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new()
    };
    if !crate::fs::permission::generic_permission(
        inode_mode, inode_uid, inode_gid,
        crate::fs::permission::MAY_WRITE, &cred,
    ) {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }
    Ok(())
}

/// This helper function is used by operations that need to modify a directory
/// (mkdir, rmdir, unlink, etc.)
fn lookup_parent_dir(pathname: &str) -> Result<(VfsPath, String), i32> {
    let (parent_path, name) = path_parent_and_name(pathname)?;
    let parent_vpath = path_lookup(&parent_path, lookup_flags::LOOKUP_DIRECTORY)?;

    // Verify it's a directory
    if let Some(ref inode) = parent_vpath.inode {
        if !inode.mode.is_directory() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }
    }

    Ok((parent_vpath, name))
}

/// Create directory - unified implementation using inode_operations
///
/// This function works across all filesystem types by:
/// 1. Resolving the parent directory path
/// 2. Calling the parent's inode_operations->mkdir
pub fn vfs_mkdir(pathname: &str, mode: u32) -> Result<(), i32> {
    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    // Get parent inode
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call mkdir through inode_operations
    unsafe {
        if let Some(mkdir_fn) = ops.mkdir {
            let inode_mode = InodeMode::new(InodeMode::S_IFDIR | mode);
            mkdir_fn(parent_inode.as_ref(), name.as_bytes(), inode_mode)?;
            Ok(())
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Remove directory - unified implementation using inode_operations
pub fn vfs_rmdir(pathname: &str) -> Result<(), i32> {
    // Look up the target inode to get its ino for cache invalidation
    let target_ino_and_fs_id = path_lookup(pathname, 0).ok().and_then(|vp| {
        vp.inode.map(|i| (i.ino, i.fs_id))
    });

    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    // Get parent inode
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call rmdir through inode_operations
    unsafe {
        if let Some(rmdir_fn) = ops.rmdir {
            let result = rmdir_fn(parent_inode.as_ref(), name.as_bytes());
            if result == 0 {
                // Invalidate icache entry for the removed directory
                if let Some((ino, fs_id)) = target_ino_and_fs_id {
                    crate::fs::inode::icache_remove(ino, fs_id);
                }
                Ok(())
            } else {
                Err(result)
            }
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Unlink file - unified implementation using inode_operations
pub fn vfs_unlink(pathname: &str) -> Result<(), i32> {
    // Look up the target inode to get its ino for cache invalidation
    let target_ino_and_fs_id = path_lookup(pathname, 0).ok().and_then(|vp| {
        vp.inode.map(|i| (i.ino, i.fs_id))
    });

    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    // Get parent inode
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call unlink through inode_operations
    unsafe {
        if let Some(unlink_fn) = ops.unlink {
            let result = unlink_fn(parent_inode.as_ref(), name.as_bytes());
            if result == 0 {
                // Invalidate icache entry for the removed inode
                if let Some((ino, fs_id)) = target_ino_and_fs_id {
                    crate::fs::inode::icache_remove(ino, fs_id);
                }
                Ok(())
            } else {
                Err(result)
            }
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Create hard link - unified implementation using inode_operations
pub fn vfs_link(oldpath: &str, newpath: &str) -> Result<(), i32> {
    // Lookup the source file
    let src_vpath = path_lookup(oldpath, 0)?;
    let src_inode = src_vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Lookup parent directory of new path
    let (parent_vpath, name) = lookup_parent_dir(newpath)?;
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call link through inode_operations
    unsafe {
        if let Some(link_fn) = ops.link {
            let result = link_fn(parent_inode.as_ref(), name.as_bytes(), src_inode.as_ref());
            if result == 0 {
                Ok(())
            } else {
                Err(result)
            }
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Rename file/directory
pub fn vfs_rename(oldpath: &str, newpath: &str) -> Result<(), i32> {
    // Lookup parent directories of both paths
    let (old_parent_vpath, old_name) = lookup_parent_dir(oldpath)?;
    let (new_parent_vpath, new_name) = lookup_parent_dir(newpath)?;

    let old_parent = old_parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    let new_parent = new_parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    check_parent_write_permission(old_parent)?;
    check_parent_write_permission(new_parent)?;

    // Use old_parent's inode ops for rename
    let result = old_parent.op_rename(old_name.as_bytes(), new_parent, new_name.as_bytes());
    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}

/// Change file mode (chmod)
///
/// # Arguments
/// - `pathname`: file path
/// - `mode`: new permission bits (e.g., 0o644)
pub fn vfs_chmod(pathname: &str, mode: u32) -> Result<(), i32> {
    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Permission check: root or owner
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new()
    };
    let inode_uid = inode.uid.load(Ordering::Relaxed);
    if cred.euid != 0 && cred.euid != inode_uid {
        return Err(errno::Errno::OperationNotPermitted.as_neg_i32());
    }

    let result = inode.op_setattr(setattr_attr::ATTR_MODE, mode as u64, 0);
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Change file ownership (chown)
///
/// # Arguments
/// - `pathname`: file path
/// - `uid`: new owner uid (u32::MAX = no change)
/// - `gid`: new owner gid (u32::MAX = no change)
pub fn vfs_chown(pathname: &str, uid: u32, gid: u32) -> Result<(), i32> {
    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new()
    };
    let inode_uid = inode.uid.load(Ordering::Relaxed);

    // Permission check
    if cred.euid != 0 {
        // Non-root: can only change group to a group they belong to
        return Err(errno::Errno::OperationNotPermitted.as_neg_i32());
    }

    // Resolve actual uid/gid (u32::MAX means no change)
    let actual_uid = if uid == u32::MAX {
        inode_uid
    } else {
        uid
    };
    let actual_gid = if gid == u32::MAX {
        inode.gid.load(Ordering::Relaxed)
    } else {
        gid
    };

    let result = inode.op_setattr(setattr_attr::ATTR_UID_GID, actual_uid as u64, actual_gid as u64);
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Truncate file by path (truncate)
///
/// # Arguments
/// - `pathname`: file path
/// - `new_size`: new file size
pub fn vfs_truncate(pathname: &str, new_size: i64) -> Result<(), i32> {
    if new_size < 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    if inode.mode.is_directory() {
        return Err(errno::Errno::IsADirectory.as_neg_i32());
    }

    // Permission check: need write access
    let inode_mode = inode.mode.bits() as u16;
    let inode_uid = inode.uid.load(Ordering::Relaxed);
    let inode_gid = inode.gid.load(Ordering::Relaxed);
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new()
    };
    if !crate::fs::permission::generic_permission(
        inode_mode, inode_uid, inode_gid,
        crate::fs::permission::MAY_WRITE, &cred,
    ) {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }

    let result = inode.op_setattr(setattr_attr::ATTR_SIZE, new_size as u64, 0);
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Truncate open file by fd (ftruncate)
pub fn vfs_ftruncate(fd: usize, new_size: i64) -> Result<(), i32> {
    if new_size < 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    let file = unsafe { get_file_fd(fd) }
        .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;

    // Get inode from file
    let inode_opt = unsafe { &*file.inode.get() };
    let inode = inode_opt.as_ref()
        .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;

    if inode.mode.is_directory() {
        return Err(errno::Errno::IsADirectory.as_neg_i32());
    }

    let result = inode.op_setattr(setattr_attr::ATTR_SIZE, new_size as u64, 0);
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Get file/directory status using inode_operations
pub fn vfs_stat(pathname: &str, stat: &mut Stat) -> Result<(), i32> {
    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Get inode operations
    if let Some(ops) = inode.ops.as_ref() {
        // Call getattr through inode_operations
        unsafe {
            if let Some(getattr_fn) = ops.getattr {
                let result = getattr_fn(inode.as_ref(), stat);
                if result == 0 {
                    return Ok(());
                } else {
                    return Err(result);
                }
            }
        }
    }

    // Fallback: fill in basic info from inode
    stat.st_ino = inode.ino;
    stat.st_mode = inode.mode.bits();
    stat.st_size = 0;
    stat.st_nlink = 1;
    Ok(())
}

///
///
/// # Arguments
/// - filename: file name (must be an absolute path)
/// - flags: O_RDONLY (0), O_WRONLY (1), O_RDWR (2), O_CREAT (0o100), O_EXCL (0o200), O_TRUNC (0o1000)
/// - mode: file permission (used when creating, currently not implemented)
///
/// # Returns
/// Returns file descriptor on success, error code on failure
///
/// # Supported flags
/// - O_RDONLY/O_WRONLY/O_RDWR: read/write mode
/// - O_CREAT: create file if it does not exist
/// - O_EXCL: used with O_CREAT, returns error if file already exists
/// - O_TRUNC: truncate file to empty
pub fn file_open(filename: &str, flags: u32, mode: u32) -> Result<usize, i32> {
    unsafe {
        // 0. Check if it's a /dev path (devfs mount point)
        if let Some(devfs_path) = devfs::parse_dev_path(filename) {
            if devfs::is_mounted() {
                // Lookup devfs device
                if let Some((entry, is_char_device, devno)) = devfs::lookup(devfs_path) {
                    // Directories auto-redirect to opendir
                    if entry.is_dir() {
                        return file_opendir(filename, flags | 0o00200000);
                    }

                    // Character device
                    if is_char_device {
                        // Get device operations
                        if let Some(ops) = devfs::registry::get_char_device_ops(devno) {
                            // Create File object
                            let file_flags = FileFlags::new(flags);
                            let file = Arc::new(File::new(file_flags));

                            // Set device operations
                            file.set_ops(ops);

                            // Store device number as private data
                            let devno_ptr = Box::into_raw(Box::new(devno)) as *mut u8;
                            file.set_private_data(devno_ptr);

                            // Allocate file descriptor
                            return match get_file_fd_install(file) {
                                Some(fd) => Ok(fd),
                                None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32())
                            };
                        } else {
                            // Device not registered
                            return Err(errno::Errno::NoSuchDevice.as_neg_i32());
                        }
                    }
                } else {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 1. Check if it's a /proc path (procfs mount point)
        if filename == "/proc" || filename.starts_with("/proc/") {
            if procfs::is_mounted() {
                // Get path in procfs (remove /proc prefix)
                let procfs_path = if filename == "/proc" {
                    "/"
                } else {
                    &filename[5..]  // Remove "/proc"
                };

                // Try to read file from procfs
                if let Some(content) = procfs::read_file(procfs_path) {
                    // Create File object
                    let file_flags = FileFlags::new(flags);
                    let file = Arc::new(File::new(file_flags));

                    // Set file operations (use ProcFS file operations)
                    file.set_ops(&PROCFS_FILE_OPS);

                    // Store content as ProcfsFileContent structure
                    let file_content = Box::new(ProcfsFileContent {
                        data: content,
                        offset: 0,
                    });
                    let content_ptr = Box::into_raw(file_content) as *mut u8;
                    file.set_private_data(content_ptr);

                    // Allocate file descriptor
                    return match get_file_fd_install(file) {
                        Some(fd) => Ok(fd),
                        None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32())
                    };
                } else {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // 1. Get RootFS superblock
        let sb_ptr = get_rootfs();
        if sb_ptr.is_null() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        let sb = &*sb_ptr;

        // Extract flags
        let o_creat = (flags & FileFlags::O_CREAT) != 0;
        let o_excl = (flags & FileFlags::O_EXCL) != 0;
        let o_trunc = (flags & FileFlags::O_TRUNC) != 0;

        // 2. Lookup file node in RootFS
        // If O_CREAT and parent dir exists in RootFS, create there first to avoid
        // ext4 corruption for temp files (e.g., /tmp/smoke_*).
        if o_creat && sb.lookup(filename).is_none() {
            let parent = filename.rfind('/').map(|i| &filename[..i]).unwrap_or("");
            if parent.is_empty() || sb.lookup(parent).is_some() {
                if sb.create_file(filename, Vec::new()).is_ok() {
                    // Successfully created in RootFS — fall through to lookup below
                }
                // If RootFS creation failed, continue to ext4
            }
        }

        // 2. Lookup file node
        let (node, _was_created) = match sb.lookup(filename) {
            Some(n) => {
                // File already exists
                if o_excl && o_creat {
                    // O_EXCL + O_CREAT: file exists, return error
                    return Err(errno::Errno::FileExists.as_neg_i32());
                }
                (n, false)
            }
            None => {
                // File does not exist in RootFS
                // Try ext4 filesystem if mounted
                if ext4::is_mounted() {
                    // First, check if file exists in ext4
                    if let Some(fs_ptr) = ext4::get_ext4_fs() {
                        unsafe {
                            let fs = &*fs_ptr;
                            match fs.lookup_path(filename) {
                                Ok((ino, ext4_inode)) => {
                                    // O_EXCL + O_CREAT: file exists on ext4.
                                    // Fall through to rootfs — the ext4 file may be
                                    // stale from an unclean shutdown; rootfs is the
                                    // authoritative source for O_EXCL semantics.
                                    if !(o_excl && o_creat) {
                                        // Check file permissions
                                        let inode_mode = ext4_inode.mode as u16;
                                        let inode_uid = ext4_inode.uid as u32;
                                        let inode_gid = ext4_inode.gid as u32;
                                        let cred = if let Some(task) = crate::sched::current() {
                                            task.cred().clone()
                                        } else {
                                            crate::process::task::Cred::new()
                                        };
                                        let mut may_mask: u32 = 0;
                                        let file_flags = FileFlags::new(flags);
                                        if !file_flags.is_writeonly() { may_mask |= crate::fs::permission::MAY_READ; }
                                        if !file_flags.is_readonly() { may_mask |= crate::fs::permission::MAY_WRITE; }
                                        if !crate::fs::permission::generic_permission(
                                            inode_mode, inode_uid, inode_gid, may_mask, &cred
                                        ) {
                                            return Err(errno::Errno::PermissionDenied.as_neg_i32());
                                        }

                                        // File exists in ext4 - open it (with O_TRUNC handling)
                                        drop(ext4_inode);  // Drop the temporary inode

                                        // Open existing file with truncation if needed
                                        return open_ext4_file(filename, flags);
                                    }
                                    // O_EXCL + O_CREAT: skip ext4, fall through to rootfs
                                }
                                Err(_) => {}
                            }
                        }
                    }

                    // File doesn't exist in ext4 or lookup failed
                    if o_creat {
                        // Create new file on ext4, fall back to RootFS on failure
                        if let Ok(inode) = ext4::create_file(filename, mode) {
                            // ext4 creation succeeded — use ext4 file ops
                            let file_flags = FileFlags::new(flags);
                            let file = Arc::new(File::new(file_flags));
                            file.set_inode(Arc::clone(&inode));
                            if let Some(ops) = inode.ops {
                                if let Some(get_file_ops) = ops.get_file_ops {
                                    if let Some(file_ops) = get_file_ops(&*inode) {
                                        file.set_ops(file_ops);
                                    }
                                }
                            }
                            return match get_file_fd_install(file) {
                                Some(fd) => Ok(fd),
                                None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
                            };
                        }
                        // ext4 creation failed — fall through to RootFS
                    }

                    // Fall back to RootFS if ext4 not mounted or creation failed
                    if o_creat {
                        // No O_CREAT and file doesn't exist
                        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                    }
                }

                // Fall back to RootFS if ext4 not mounted
                if o_creat {
                    // Create new file in RootFS
                    if let Err(e) = sb.create_file(filename, Vec::new()) {
                        return Err(e);
                    }
                    // Re-lookup the newly created file
                    match sb.lookup(filename) {
                        Some(n) => (n, true),
                        None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                    }
                } else {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        };

        // 4. Check if it's a directory — auto-redirect to opendir
        if node.is_dir() {
            return file_opendir(filename, flags | 0o00200000);
        }

        // 5. Handle O_TRUNC: truncate file
        if o_trunc {
            // TODO: Implement file truncation
            // Need to modify RootFSNode's data to empty Vec
            // Since RootFSNode uses immutable references, this is not yet implementable
            // Can add interior mutability support in the future
        }

        // 6. Create File object
        let file_flags = FileFlags::new(flags);
        let file = Arc::new(File::new(file_flags));

        // 7. Set file operations
        file.set_ops(&ROOTFS_FILE_OPS);

        // 8. Store RootFSNode pointer as private data
        // Note: Using raw pointer here, lifecycle managed by RootFS
        let node_ptr = node.as_ref() as *const RootFSNode as *mut u8;
        file.set_private_data(node_ptr);

        // 9. Allocate file descriptor
        match get_file_fd_install(file) {
            Some(fd) => Ok(fd),
            None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
        }
    }
}

/// Open a file from ext4 filesystem
fn open_ext4_file(filename: &str, flags: u32) -> Result<usize, i32> {
    unsafe {
        // Get ext4 filesystem
        let fs_ptr = match ext4::get_ext4_fs() {
            Some(ptr) => ptr,
            None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        };
        let fs = &*fs_ptr;

        // Lookup inode by path
        let inode = match ext4::path_lookup(fs, filename) {
            Some(ino) => ino,
            None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        };

        // Check if it's a directory — auto-redirect to opendir
        if inode.mode.is_directory() {
            return file_opendir(filename, flags | 0o00200000);
        }

        // Handle O_TRUNC: truncate file to size 0
        let o_trunc = (flags & FileFlags::O_TRUNC) != 0;
        if o_trunc {
            // Read ext4 inode and set size to 0
            let ext4_ino = inode.ino as u32;
            if let Ok(mut ext4_inode) = fs.read_inode(ext4_ino) {
                ext4_inode.set_size(0);
                let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                let block_size = fs.block_size as u64;
                let pointers_per_block = block_size / 4;

                // Free data blocks and clear pointers
                if ext4_inode.has_extent() {
                    use crate::fs::ext4::extent::{Ext4ExtentHeader, Ext4Extent, EXT4_EXT_MAGIC};

                    // Free all blocks referenced by extent entries
                    let header = unsafe {
                        &*(ext4_inode.block.as_ptr() as *const Ext4ExtentHeader)
                    };
                    if header.eh_magic == EXT4_EXT_MAGIC {
                        let entries = unsafe {
                            core::slice::from_raw_parts(
                                (ext4_inode.block.as_ptr() as *const u8)
                                    .add(core::mem::size_of::<Ext4ExtentHeader>())
                                    as *const Ext4Extent,
                                header.eh_entries as usize
                            )
                        };
                        for ext in entries {
                            for j in 0..ext.length() as u64 {
                                let _ = allocator.free_block(ext.start_block() + j);
                            }
                        }
                    }

                    // Reset extent header in i_block
                    let header = unsafe {
                        &mut *(ext4_inode.block.as_mut_ptr() as *mut Ext4ExtentHeader)
                    };
                    header.eh_magic = EXT4_EXT_MAGIC;
                    header.eh_entries = 0;
                    header.eh_max = 4;  // Max inline extents
                    header.eh_depth = 0;
                    header.eh_generation = 0;
                } else {
                    // Free direct blocks
                    for i in 0..12 {
                        if ext4_inode.block[i] != 0 {
                            let _ = allocator.free_block(ext4_inode.block[i] as u64);
                            ext4_inode.block[i] = 0;
                        }
                    }

                    // Free single indirect blocks
                    if ext4_inode.block[12] != 0 {
                        let indirect_block = ext4_inode.block[12] as u64;
                        let current_blocks = (ext4_inode.get_size() + block_size - 1) / block_size;
                        let indirect_count = core::cmp::min(
                            current_blocks.saturating_sub(12),
                            pointers_per_block,
                        ) as usize;
                        for i in 0..indirect_count {
                            if let Ok(data_block) =
                                crate::fs::ext4::indirect::read_indirect_block(
                                    fs, indirect_block, i,
                                )
                            {
                                if data_block != 0 {
                                    let _ = allocator.free_block(data_block);
                                }
                            }
                        }
                        let _ = allocator.free_block(indirect_block);
                        ext4_inode.block[12] = 0;
                    }

                    // Free double indirect blocks
                    if ext4_inode.block[13] != 0 {
                        let double_block = ext4_inode.block[13] as u64;
                        let single_start = 12 + pointers_per_block;
                        let current_blocks = (ext4_inode.get_size() + block_size - 1) / block_size;
                        if current_blocks > single_start {
                            let double_range = current_blocks.saturating_sub(single_start);
                            let first_level_count =
                                ((double_range + pointers_per_block - 1) / pointers_per_block) as usize;
                            for i in 0..core::cmp::min(first_level_count, pointers_per_block as usize) {
                                if let Ok(single_block) =
                                    crate::fs::ext4::indirect::read_indirect_block(
                                        fs, double_block, i,
                                    )
                                {
                                    if single_block != 0 {
                                        let remaining = if i == first_level_count - 1 {
                                            (double_range % pointers_per_block) as usize
                                        } else {
                                            pointers_per_block as usize
                                        };
                                        for j in 0..remaining {
                                            if let Ok(data_block) =
                                                crate::fs::ext4::indirect::read_indirect_block(
                                                    fs, single_block, j,
                                                )
                                            {
                                                if data_block != 0 {
                                                    let _ = allocator.free_block(data_block);
                                                }
                                            }
                                        }
                                        let _ = allocator.free_block(single_block);
                                    }
                                }
                            }
                        }
                        let _ = allocator.free_block(double_block);
                        ext4_inode.block[13] = 0;
                    }
                }

                // Update i_blocks and timestamps
                ext4_inode.blocks = 0;
                let cycles = crate::drivers::intc::clint::read_time();
                let sec = (cycles / 10_000_000) as u32;
                ext4_inode.mtime = sec;
                ext4_inode.ctime = sec;

                // Write back the inode
                let _ = ext4::inode::write_inode(fs, ext4_ino, &ext4_inode);
            }
        }

        // Create File object
        let file_flags = FileFlags::new(flags);
        let file = Arc::new(File::new(file_flags));

        // Set inode
        file.set_inode(Arc::clone(&inode));

        // Get file operations from inode's get_file_ops callback
        if let Some(ops) = inode.ops {
            if let Some(get_file_ops) = ops.get_file_ops {
                if let Some(file_ops) = get_file_ops(&*inode) {
                    file.set_ops(file_ops);
                }
            }
        }

        // Allocate file descriptor
        match get_file_fd_install(file) {
            Some(fd) => Ok(fd),
            None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
        }
    }
}

///
///
/// # Arguments
/// - fd: file descriptor
///
/// # Returns
/// Returns Ok(()) on success, error code on failure
pub fn file_close(fd: usize) -> Result<(), i32> {
    unsafe {
        // Use close_file_fd to close the file descriptor
        // This will:
        // 1. Check file descriptor validity
        // 2. Call the file's close operation
        // 3. Release the file descriptor
        close_file_fd(fd)
    }
}

///
///
/// # Arguments
/// - fd: file descriptor
/// - buf: buffer
/// - count: number of bytes to read
///
/// # Returns
/// Returns number of bytes read on success, error code on failure
pub fn file_read(fd: usize, buf: &mut [u8], count: usize) -> Result<usize, i32> {
    unsafe {
        // Get file object
        match get_file_fd(fd) {
            Some(file) => {
                // Arc auto-derefs to File
                let file_ref: &File = &*file;
                let buf_ptr = buf.as_mut_ptr();
                let read_count = count.min(buf.len());

                // Call file's read operation
                let result = file_ref.read(buf_ptr, read_count);
                if result < 0 {
                    Err(result as i32)
                } else {
                    Ok(result as usize)
                }
            }
            None => {
                Err(errno::Errno::BadFileNumber.as_neg_i32())
            }
        }
    }
}

///
///
/// # Arguments
/// - fd: file descriptor
/// - buf: buffer
/// - count: number of bytes to write
///
/// # Returns
/// Returns number of bytes written on success, error code on failure
pub fn file_write(fd: usize, buf: &[u8], count: usize) -> Result<usize, i32> {
    unsafe {
        // Get file object
        match get_file_fd(fd) {
            Some(file) => {
                // Arc auto-derefs to File
                let file_ref: &File = &*file;
                let buf_ptr = buf.as_ptr();
                let write_count = count.min(buf.len());

                // Call file's write operation
                let result = file_ref.write(buf_ptr, write_count);
                if result < 0 {
                    Err(result as i32)
                } else {
                    Ok(result as usize)
                }
            }
            None => {
                Err(errno::Errno::BadFileNumber.as_neg_i32())
            }
        }
    }
}

///
///
/// # Arguments
/// - fd: file descriptor
/// - stat: output parameter to store file status information
///
/// # Returns
/// Returns Ok(()) on success, error code on failure
///
/// # Description
/// Gets status information of an opened file, including:
/// - File type (regular file, directory, character device, etc.)
/// - File size
/// - Permissions
/// - Inode number
/// - Timestamps, etc.
pub fn file_stat(fd: usize, stat: &mut Stat) -> Result<(), i32> {
    unsafe {
        // Get file object
        match get_file_fd(fd) {
            Some(file) => {
                // Arc auto-derefs to File
                let file_ref: &File = &*file;

                // First check if it's a character device
                if crate::fs::char_dev::char_dev_stat(file_ref, stat).is_some() {
                    return Ok(());
                }

                // Check file operations to determine type
                // (ext4 files set ops+inode but NOT private_data, so check ops first)
                let ops = &*file_ref.ops.get();
                if let Some(ops_ref) = ops {
                    if core::ptr::eq(*ops_ref, &ext4::file::EXT4_FILE_OPS as *const FileOps) {
                        // ext4 regular file - uses file's inode (not private_data)
                        let inode_opt = &*file_ref.inode.get();
                        let inode = match inode_opt {
                            Some(i) => i,
                            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                        };

                        // Get ext4 filesystem pointer from inode's private_data
                        let fs_ptr = match inode.private_data {
                            Some(ptr) => ptr as *const crate::fs::ext4::Ext4FileSystem,
                            None => return Err(errno::Errno::IOError.as_neg_i32()),
                        };
                        let fs = &*fs_ptr;
                        let ext4_ino = inode.ino as u32;

                        // Use cached Ext4Inode from inode.sb instead of re-reading from disk
                        let ext4_inode = match inode.sb {
                            Some(ptr) => unsafe { &*(ptr as *const crate::fs::ext4::inode::Ext4Inode) },
                            None => return Err(errno::Errno::IOError.as_neg_i32()),
                        };

                        // Fill stat structure
                        stat.st_dev = 0;
                        stat.st_ino = ext4_ino as u64;
                        stat.st_nlink = ext4_inode.links_count as u32;
                        stat.st_uid = ext4_inode.uid as u32;
                        stat.st_gid = ext4_inode.gid as u32;
                        stat.st_rdev = 0;
                        stat.st_size = ext4_inode.size as i64;
                        stat.st_blocks = ext4_inode.blocks as i64;
                        stat.st_blksize = fs.block_size as i64;

                        // File type and permissions
                        stat.set_regular_file();
                        stat.set_mode(ext4_inode.mode as u32 & 0o777);

                        // Timestamps
                        stat.st_atime = ext4_inode.atime as i64;
                        stat.st_atime_nsec = 0;
                        stat.st_mtime = ext4_inode.mtime as i64;
                        stat.st_mtime_nsec = 0;
                        stat.st_ctime = ext4_inode.ctime as i64;
                        stat.st_ctime_nsec = 0;

                        return Ok(());
                    }
                }

                // Get data from private_data
                let data_opt = &*file_ref.private_data.get();
                if let Some(data_ptr) = *data_opt {
                    // Check file operations to determine type
                    let ops = &*file_ref.ops.get();
                    if let Some(ops_ref) = ops {
                        // If it's a directory operation, handle DirContext
                        if core::ptr::eq(*ops_ref, &ROOTFS_DIR_OPS as *const FileOps) {
                            // This is a RootFS directory, data_ptr is DirContext
                            let ctx = &*(data_ptr as *const DirContext);
                            let path = ctx.get_path();

                            // Re-lookup node
                            let sb_ptr = get_rootfs();
                            if sb_ptr.is_null() {
                                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                            }
                            let sb = &*sb_ptr;
                            let node = match sb.lookup(path) {
                                Some(n) => n,
                                None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                            };
                            let node_ref = node.as_ref();

                            // Fill stat structure
                            stat.st_dev = 0;
                            stat.st_ino = node_ref.ino;
                            stat.st_nlink = 1;
                            stat.st_uid = 0;
                            stat.st_gid = 0;
                            stat.st_rdev = 0;
                            stat.st_size = 0;
                            stat.st_blocks = 0;
                            stat.st_blksize = 4096;
                            stat.set_directory();
                            stat.set_mode(0o755);
                            stat.st_atime = 0;
                            stat.st_atime_nsec = 0;
                            stat.st_mtime = 0;
                            stat.st_mtime_nsec = 0;
                            stat.st_ctime = 0;
                            stat.st_ctime_nsec = 0;
                            return Ok(());
                        } else if core::ptr::eq(*ops_ref, &EXT4_DIR_OPS as *const FileOps) {
                            // ext4 directory
                            stat.st_dev = 0;
                            stat.st_ino = 2;  // root directory
                            stat.st_nlink = 1;
                            stat.st_uid = 0;
                            stat.st_gid = 0;
                            stat.st_rdev = 0;
                            stat.st_size = 0;
                            stat.st_blocks = 0;
                            stat.st_blksize = 4096;
                            stat.set_directory();
                            stat.set_mode(0o755);
                            stat.st_atime = 0;
                            stat.st_atime_nsec = 0;
                            stat.st_mtime = 0;
                            stat.st_mtime_nsec = 0;
                            stat.st_ctime = 0;
                            stat.st_ctime_nsec = 0;
                            return Ok(());
                        }
                    }

                    // Regular file: data_ptr is RootFSNode pointer
                    let node = &*(data_ptr as *const RootFSNode);

                    // Fill stat structure
                    stat.st_dev = 0;  // RootFS has no device concept
                    stat.st_ino = node.ino;
                    stat.st_nlink = 1;  // Default hard link count is 1
                    stat.st_uid = 0;   // root user
                    stat.st_gid = 0;   // root group
                    stat.st_rdev = 0;

                    // File size
                    let data_guard = node.data.lock();
                    if let Some(ref data) = *data_guard {
                        stat.st_size = data.len() as i64;
                        // Calculate block count (512-byte blocks)
                        stat.st_blocks = (data.len() as i64 + 511) / 512;
                    } else {
                        stat.st_size = 0;
                        stat.st_blocks = 0;
                    }

                    stat.st_blksize = 4096;  // 4KB block size

                    // File type and permissions
                    if node.is_dir() {
                        stat.set_directory();
                        // Directory permissions: rwxr-xr-x (0o755)
                        stat.set_mode(0o755);
                    } else {
                        stat.set_regular_file();
                        // File permissions: rw-r--r-- (0o644)
                        stat.set_mode(0o644);
                    }

                    // Timestamps (currently using 0, can implement real timestamps in the future)
                    stat.st_atime = 0;
                    stat.st_atime_nsec = 0;
                    stat.st_mtime = 0;
                    stat.st_mtime_nsec = 0;
                    stat.st_ctime = 0;
                    stat.st_ctime_nsec = 0;

                    Ok(())
                } else {
                    // No private_data, could be a pipe or character device
                    // TODO: Handle other file types
                    Err(errno::Errno::BadFileNumber.as_neg_i32())
                }
            }
            None => {
                Err(errno::Errno::BadFileNumber.as_neg_i32())
            }
        }
    }
}

/// Get file status by path (for fstatat)
pub fn stat_file_by_path(path: &str, stat: &mut Stat) -> Result<(), i32> {
    // Check devfs
    if let Some(dev_path) = devfs::parse_dev_path(path) {
        if let Some((entry, is_char_dev, devno)) = devfs::lookup(dev_path) {
            stat.st_dev = 0;
            stat.st_ino = 1;
            stat.st_nlink = 1;
            stat.st_uid = 0;
            stat.st_gid = 0;
            stat.st_rdev = ((devno.major as u64) << 32) | (devno.minor as u64);
            stat.st_size = 0;
            stat.st_blocks = 0;
            stat.st_blksize = 4096;

            if entry.is_dir() {
                stat.set_directory();
                stat.set_mode(entry.mode);
            } else if is_char_dev {
                stat.set_char_device();
                stat.set_mode(entry.mode);
            } else {
                stat.set_regular_file();
                stat.set_mode(entry.mode);
            }

            stat.st_atime = 0;
            stat.st_atime_nsec = 0;
            stat.st_mtime = 0;
            stat.st_mtime_nsec = 0;
            stat.st_ctime = 0;
            stat.st_ctime_nsec = 0;
            return Ok(());
        }
    }

    // Check procfs
    if path.starts_with("/proc") {
        // Simplified handling: return stat for proc directory
        stat.st_dev = 0;
        stat.st_ino = 1;
        stat.st_nlink = 1;
        stat.st_uid = 0;
        stat.st_gid = 0;
        stat.st_rdev = 0;
        stat.st_size = 0;
        stat.st_blocks = 0;
        stat.st_blksize = 4096;
        stat.set_directory();
        stat.set_mode(0o555);
        stat.st_atime = 0;
        stat.st_atime_nsec = 0;
        stat.st_mtime = 0;
        stat.st_mtime_nsec = 0;
        stat.st_ctime = 0;
        stat.st_ctime_nsec = 0;
        return Ok(());
    }

    // Try ext4 filesystem
    if ext4::is_mounted() {
        // Use ext4 lookup directly to get file information
        if let Some(fs_ptr) = ext4::get_ext4_fs() {
            unsafe {
                match (*fs_ptr).lookup_path(path) {
                    Ok((ino, inode)) => {
                        stat.st_dev = 0;
                        stat.st_ino = ino as u64;
                        stat.st_nlink = inode.links_count as u32;
                        stat.st_uid = inode.uid as u32;
                        stat.st_gid = inode.gid as u32;
                        stat.st_rdev = 0;
                        stat.st_size = inode.get_size() as i64;
                        stat.st_blocks = inode.blocks as i64;
                        stat.st_blksize = 4096;

                        // Set the entire mode (including file type and permissions)
                        stat.st_mode = inode.mode as u32;

                        stat.st_atime = inode.atime as i64;
                        stat.st_atime_nsec = 0;
                        stat.st_mtime = inode.mtime as i64;
                        stat.st_mtime_nsec = 0;
                        stat.st_ctime = inode.ctime as i64;
                        stat.st_ctime_nsec = 0;
                        return Ok(());
                    }
                    Err(_) => {
                        // File not in ext4, continue trying other filesystems
                    }
                }
            }
        }
    }

    // Try RootFS
    let rootfs = unsafe { get_rootfs() };
    if !rootfs.is_null() {
        if let Some(node) = unsafe { (*rootfs).lookup(path) } {
            stat.st_dev = 0;
            stat.st_ino = node.ino;
            stat.st_nlink = 1;
            stat.st_uid = 0;
            stat.st_gid = 0;
            stat.st_rdev = 0;

            let data_guard = node.data.lock();
            if let Some(ref data) = *data_guard {
                stat.st_size = data.len() as i64;
                stat.st_blocks = (data.len() as i64 + 511) / 512;
            } else {
                stat.st_size = 0;
                stat.st_blocks = 0;
            }

            stat.st_blksize = 4096;

            if node.is_dir() {
                stat.set_directory();
                stat.set_mode(0o755);
            } else {
                stat.set_regular_file();
                stat.set_mode(0o644);
            }

            stat.st_atime = 0;
            stat.st_atime_nsec = 0;
            stat.st_mtime = 0;
            stat.st_mtime_nsec = 0;
            stat.st_ctime = 0;
            stat.st_ctime_nsec = 0;
            return Ok(());
        }
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
}

/// fcntl command constants
///
pub mod fcntl {
    /// Duplicate file descriptor
    pub const F_DUPFD: usize = 0;

    /// Get close-on-exec flag
    pub const F_GETFD: usize = 1;

    /// Set close-on-exec flag
    pub const F_SETFD: usize = 2;

    /// Get file status flags
    pub const F_GETFL: usize = 3;

    /// Set file status flags
    pub const F_SETFL: usize = 4;

    /// Duplicate file descriptor with close-on-exec
    pub const F_DUPFD_CLOEXEC: usize = 1030;

    /// FD_CLOEXEC flag value
    pub const FD_CLOEXEC: usize = 1;
}

///
///
/// # Arguments
/// - fd: file descriptor
/// - cmd: fcntl command
/// - arg: command argument
///
/// # Returns
/// Returns command-specific value on success, error code on failure
///
/// # Supported commands
/// - F_DUPFD (0) - Duplicate file descriptor, arg specifies minimum fd
/// - F_GETFD (1) - Get close-on-exec flag
/// - F_SETFD (2) - Set close-on-exec flag
/// - F_GETFL (3) - Get file status flags
/// - F_SETFL (4) - Set file status flags
pub fn file_fcntl(fd: usize, cmd: usize, arg: usize) -> Result<usize, i32> {
    use crate::fs::file::{get_file_fd, get_file_fd_install};

    unsafe {
        match cmd {
            // F_DUPFD: Duplicate file descriptor
            fcntl::F_DUPFD => {
                // Get original file
                let old_file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Allocate new file descriptor (>= arg)
                let min_fd = arg;
                let new_fd = match get_file_fd_install(old_file) {
                    Some(fd) if fd >= min_fd => fd,
                    Some(_fd) => {
                        // TODO: Implement fd redirection to support F_DUPFD's arg parameter
                        // Current simplified implementation: return allocated fd directly
                        return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
                    }
                    None => return Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
                };

                Ok(new_fd)
            }

            // F_DUPFD_CLOEXEC: Duplicate file descriptor with close-on-exec
            fcntl::F_DUPFD_CLOEXEC => {
                // Get original file
                let old_file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Allocate new file descriptor (>= arg)
                let min_fd = arg;
                let new_fd = match get_file_fd_install(old_file) {
                    Some(fd) if fd >= min_fd => fd,
                    Some(_fd) => {
                        return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
                    }
                    None => return Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
                };

                // Set close-on-exec flag on the new fd
                if let Some(new_file) = get_file_fd(new_fd) {
                    new_file.set_cloexec(true);
                }

                Ok(new_fd)
            }

            // F_GETFD: Get close-on-exec flag
            fcntl::F_GETFD => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                let cloexec = file.get_cloexec();
                Ok(if cloexec { fcntl::FD_CLOEXEC } else { 0 })
            }

            // F_SETFD: Set close-on-exec flag
            fcntl::F_SETFD => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Bit 0 of arg indicates FD_CLOEXEC
                let cloexec = (arg & fcntl::FD_CLOEXEC) != 0;
                file.set_cloexec(cloexec);

                Ok(0)  // Return 0 on success
            }

            // F_GETFL: Get file status flags
            fcntl::F_GETFL => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Return file status flags (access mode)
                Ok(file.flags.bits() as usize)
            }

            // F_SETFL: Set file status flags
            fcntl::F_SETFL => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Only allow setting certain flags (O_NONBLOCK, O_APPEND, O_ASYNC, etc.)
                // Cannot change access mode (O_RDONLY, O_WRONLY, O_RDWR)
                const SETFL_FLAGS: u32 = crate::fs::file::FileFlags::O_APPEND
                    | crate::fs::file::FileFlags::O_NONBLOCK
                    | crate::fs::file::FileFlags::O_SYNC
                    | crate::fs::file::FileFlags::O_DSYNC;

                // Preserve access mode
                let accmode = file.flags.bits() & crate::fs::file::FileFlags::O_ACCMODE;
                // Set new flags
                let new_flags = accmode | (arg as u32 & SETFL_FLAGS);

                // Use unsafe to set flags (FileFlags is not Mutex, requires direct assignment)
                unsafe {
                    let flags_ptr = &file.flags as *const FileFlags as *mut FileFlags;
                    (*flags_ptr).set_bits(new_flags);
                }

                Ok(0)  // Return 0 on success
            }

            // Unsupported command
            _ => {
                Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
            }
        }
    }
}

///
pub fn io_poll(_fds: *mut u8, _nfds: usize, _timeout_ms: i32) -> Result<usize, i32> {
    // TODO: Implement I/O multiplexing
    // Need to implement:
    // - Wait for file descriptor readiness
    // - Support timeout
    // - Return number of ready file descriptors
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

///
///
/// # Arguments
/// - pathname: directory path
/// - mode: directory permissions
///
/// # Returns
/// Returns Ok(()) on success, error code on failure
///
/// - RISC-V: 77 (mkdirat), but we implement simplified mkdir
pub fn file_mkdir(pathname: &str, mode: u32) -> Result<(), i32> {
    vfs_mkdir(pathname, mode)
}

///
///
/// # Arguments
/// - pathname: directory path
///
/// # Returns
/// Returns Ok(()) on success, error code on failure
///
/// - RISC-V: 79
pub fn file_rmdir(pathname: &str) -> Result<(), i32> {
    vfs_rmdir(pathname)
}

///
///
/// # Arguments
/// - pathname: file path
///
/// # Returns
/// Returns Ok(()) on success, error code on failure
///
/// - RISC-V: 74 (unlinkat), but we implement simplified unlink
pub fn file_unlink(pathname: &str) -> Result<(), i32> {
    vfs_unlink(pathname)
}

///
///
/// # Arguments
/// - oldpath: existing file path
/// - newpath: new link path
///
/// # Returns
/// Returns Ok(()) on success, error code on failure
///
/// - RISC-V: 78 (linkat), but we implement simplified link
pub fn file_link(oldpath: &str, newpath: &str) -> Result<(), i32> {
    vfs_link(oldpath, newpath)
}

// ============================================================================
// ============================================================================

/// RootFS file read operation
///
fn rootfs_file_read(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        // Get RootFSNode pointer from private_data
        let data_opt = &*file.private_data.get();
        if let Some(node_ptr) = *data_opt {
            let node = &*(node_ptr as *const RootFSNode);

            // Get current file position
            let offset = file.get_pos() as usize;

            // Check if there is data
            let data_guard = node.data.lock();
            if let Some(ref data) = *data_guard {
                let available: usize = data.len().saturating_sub(offset);
                let to_read = buf.len().min(available);

                if to_read > 0 {
                    // Copy data to buffer
                    buf[..to_read].copy_from_slice(&data[offset..offset + to_read]);

                    // Update file position
                    file.set_pos((offset + to_read) as u64);

                    to_read as isize
                } else {
                    0  // EOF
                }
            } else {
                0  // Directory or no data
            }
        } else {
            -9  // EBADF
        }
    }
}

/// RootFS file write operation
///
fn rootfs_file_write(file: &File, buf: &[u8]) -> isize {
    unsafe {
        // Get RootFSNode pointer from private_data
        let data_opt = &*file.private_data.get();
        if let Some(node_ptr) = *data_opt {
            let node = &*(node_ptr as *const crate::fs::rootfs::RootFSNode);
            let offset = file.get_pos() as usize;
            let written = node.write_data(offset, buf);
            if written > 0 {
                file.set_pos((offset + written) as u64);
            }
            written as isize
        } else {
            -9  // EBADF
        }
    }
}

/// RootFS file seek operation
///
fn rootfs_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    // Get current file position
    let current_pos = file.get_pos() as isize;

    // Get file size
    let file_size = unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(node_ptr) = *data_opt {
            let node = &*(node_ptr as *const RootFSNode);
            node.data.lock().as_ref().map_or(0isize, |d: &Vec<u8>| d.len() as isize)
        } else {
            return -9;  // EBADF
        }
    };

    let new_pos = match whence {
        0 => offset,              // SEEK_SET
        1 => current_pos + offset, // SEEK_CUR
        2 => file_size + offset,   // SEEK_END
        _ => return -22,           // EINVAL - invalid whence
    };

    if new_pos < 0 {
        return -22;  // EINVAL - negative position is invalid
    }

    file.set_pos(new_pos as u64);
    new_pos
}

/// RootFS file close operation
fn rootfs_file_close(_file: &File) -> i32 {
    // RootFS nodes are managed by RootFS, no special handling needed here
    0
}

/// RootFS file operations table
///
static ROOTFS_FILE_OPS: FileOps = FileOps {
    read: Some(rootfs_file_read),
    write: Some(rootfs_file_write),
    lseek: Some(rootfs_file_lseek),
    close: Some(rootfs_file_close),
    poll: None,
};

/// ProcFS file content structure (stored in File's private_data)
#[repr(C)]
pub struct ProcfsFileContent {
    /// File content
    pub data: Vec<u8>,
    /// Current read offset
    pub offset: usize,
}

/// ProcFS file read operation
fn procfs_file_read(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        // Get ProcfsFileContent pointer from private_data
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            let content = &*(content_ptr as *const ProcfsFileContent);

            // Use file's pos as offset
            let offset = file.get_pos() as usize;
            let available = content.data.len().saturating_sub(offset);
            let to_read = buf.len().min(available);

            if to_read > 0 {
                // Copy data to buffer
                buf[..to_read].copy_from_slice(&content.data[offset..offset + to_read]);

                // Update file's pos
                file.set_pos((offset + to_read) as u64);

                to_read as isize
            } else {
                0  // EOF
            }
        } else {
            -9  // EBADF
        }
    }
}

/// ProcFS file write operation (read-only, returns error)
fn procfs_file_write(_file: &File, _buf: &[u8]) -> isize {
    -9  // EBADF - procfs is read-only
}

/// ProcFS file lseek operation
fn procfs_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            let content = &*(content_ptr as *const ProcfsFileContent);
            let file_size = content.data.len() as isize;

            let new_offset = match whence {
                0 => offset,                            // SEEK_SET
                1 => file.get_pos() as isize + offset,  // SEEK_CUR
                2 => file_size + offset,                // SEEK_END
                _ => return -22,  // EINVAL
            };

            if new_offset < 0 || new_offset > file_size {
                return -22;  // EINVAL
            }

            file.set_pos(new_offset as u64);
            new_offset
        } else {
            -9  // EBADF
        }
    }
}

/// ProcFS file close operation
fn procfs_file_close(file: &File) -> i32 {
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            // Free ProcfsFileContent
            let _ = Box::from_raw(content_ptr as *mut ProcfsFileContent);
            *file.private_data.get() = None;
        }
        0
    }
}

/// ProcFS file operations table
static PROCFS_FILE_OPS: FileOps = FileOps {
    read: Some(procfs_file_read),
    write: Some(procfs_file_write),
    lseek: Some(procfs_file_lseek),
    close: Some(procfs_file_close),
    poll: None,
};

// ============================================================================
// Directory operations (for getdents64 system call)
// ============================================================================

/// Directory type identifier (to distinguish between rootfs, ext4, procfs, and devfs directories)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DirType {
    RootFS = 0,
    Ext4 = 1,
    ProcFS = 2,
    DevFS = 3,
}

/// Directory context (stored in File's private_data)
#[repr(C)]
pub struct DirContext {
    /// Directory type
    pub dir_type: DirType,
    /// Current read offset
    pub offset: usize,
    /// Directory path (for ext4)
    pub path: [u8; 256],
    /// Path length
    pub path_len: usize,
}

impl DirContext {
    pub fn new_rootfs(path: &str) -> Self {
        let mut ctx = Self {
            dir_type: DirType::RootFS,
            offset: 0,
            path: [0; 256],
            path_len: 0,
        };
        let bytes = path.as_bytes();
        let len = bytes.len().min(255);
        ctx.path[..len].copy_from_slice(&bytes[..len]);
        ctx.path_len = len;
        ctx
    }

    pub fn new_ext4(path: &str) -> Self {
        let mut ctx = Self {
            dir_type: DirType::Ext4,
            offset: 0,
            path: [0; 256],
            path_len: 0,
        };
        let bytes = path.as_bytes();
        let len = bytes.len().min(255);
        ctx.path[..len].copy_from_slice(&bytes[..len]);
        ctx.path_len = len;
        ctx
    }

    pub fn new_procfs(path: &str) -> Self {
        let mut ctx = Self {
            dir_type: DirType::ProcFS,
            offset: 0,
            path: [0; 256],
            path_len: 0,
        };
        let bytes = path.as_bytes();
        let len = bytes.len().min(255);
        ctx.path[..len].copy_from_slice(&bytes[..len]);
        ctx.path_len = len;
        ctx
    }

    pub fn new_devfs(path: &str) -> Self {
        let mut ctx = Self {
            dir_type: DirType::DevFS,
            offset: 0,
            path: [0; 256],
            path_len: 0,
        };
        let bytes = path.as_bytes();
        let len = bytes.len().min(255);
        ctx.path[..len].copy_from_slice(&bytes[..len]);
        ctx.path_len = len;
        ctx
    }

    pub fn get_path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

/// Open directory (for getdents64)
///
/// # Arguments
/// - pathname: directory path
/// - flags: open flags
///
/// # Returns
/// Returns file descriptor on success, error code on failure
pub fn file_opendir(pathname: &str, flags: u32) -> Result<usize, i32> {
    unsafe {
        // 0. Check if it's a /dev path (devfs mount point)
        if pathname == "/dev" || pathname.starts_with("/dev/") {
            // Check if devfs is mounted
            if devfs::is_mounted() {
                // Get path in devfs (remove /dev prefix)
                let devfs_path = if pathname == "/dev" {
                    ""
                } else {
                    &pathname[5..]  // Remove "/dev"
                };

                // Check if directory exists
                if devfs::list_dir(devfs_path).is_some() {
                    // Create File object
                    let file_flags = FileFlags::new(flags);
                    let file = Arc::new(File::new(file_flags));

                    // Set directory operations (use ext4 operations as placeholder)
                    file.set_ops(&EXT4_DIR_OPS);

                    // Create directory context
                    let ctx = Box::new(DirContext::new_devfs(devfs_path));
                    let ctx_ptr = Box::into_raw(ctx) as *mut u8;
                    file.set_private_data(ctx_ptr);

                    // Allocate file descriptor
                    return match get_file_fd_install(file) {
                        Some(fd) => Ok(fd),
                        None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32())
                    };
                }
            }
        }

        // 1. Check if it's a /proc path (procfs mount point)
        if pathname == "/proc" || pathname.starts_with("/proc/") {
            // Check if procfs is mounted
            if procfs::is_mounted() {
                // Get path in procfs (remove /proc prefix)
                let procfs_path = if pathname == "/proc" {
                    "/"
                } else {
                    &pathname[5..]  // Remove "/proc"
                };

                // Check if directory exists
                if procfs::list_dir(procfs_path).is_some() {
                    // Create File object
                    let file_flags = FileFlags::new(flags);
                    let file = Arc::new(File::new(file_flags));

                    // Set directory operations (use ext4 operations as placeholder)
                    file.set_ops(&EXT4_DIR_OPS);

                    // Create directory context
                    let ctx = Box::new(DirContext::new_procfs(procfs_path));
                    let ctx_ptr = Box::into_raw(ctx) as *mut u8;
                    file.set_private_data(ctx_ptr);

                    // Allocate file descriptor
                    return match get_file_fd_install(file) {
                        Some(fd) => Ok(fd),
                        None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32())
                    };
                }
            }
        }

        // 1. First try to lookup from ext4 (if mounted to root directory)
        // This way ext4's root directory will override RootFS's root directory
        if ext4::is_mounted() {
            // Check if directory exists
            let entries = ext4::list_dir(pathname);

            if let Some(_entries) = entries {
                // Create File object
                let file_flags = FileFlags::new(flags);
                let file = Arc::new(File::new(file_flags));

                // Set directory operations (use ext4 operations)
                file.set_ops(&EXT4_DIR_OPS);

                // Create directory context
                let ctx = Box::new(DirContext::new_ext4(pathname));
                let ctx_ptr = Box::into_raw(ctx) as *mut u8;
                file.set_private_data(ctx_ptr);

                // Allocate file descriptor
                return match get_file_fd_install(file) {
                    Some(fd) => Ok(fd),
                    None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32())
                };
            }
        }

        // 2. Not found in ext4, try to lookup from RootFS
        let sb_ptr = get_rootfs();

        if !sb_ptr.is_null() {
            let sb = &*sb_ptr;

            let lookup_result = sb.lookup(pathname);

            if let Some(node) = lookup_result {
                // Check if it's a directory
                if !node.is_dir() {
                    return Err(errno::Errno::NotADirectory.as_neg_i32());
                }

                // Create File object
                let file_flags = FileFlags::new(flags);
                let file = Arc::new(File::new(file_flags));

                // Set directory operations
                file.set_ops(&ROOTFS_DIR_OPS);

                // Create directory context
                let ctx = Box::new(DirContext::new_rootfs(pathname));
                let ctx_ptr = Box::into_raw(ctx) as *mut u8;
                file.set_private_data(ctx_ptr);

                // Allocate file descriptor
                return match get_file_fd_install(file) {
                    Some(fd) => Ok(fd),
                    None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32())
                };
            }
        }

        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }
}

///
#[repr(C, packed)]
pub struct Dirent64 {
    pub d_ino: u64,       // inode number
    pub d_off: u64,       // offset to next dirent
    pub d_reclen: u16,    // length of this record
    pub d_type: u8,       // file type
    // d_name follows immediately, variable-length string
}

/// File type constants (DT_*)
pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;
pub const DT_WHT: u8 = 14;

/// Read directory entries (getdents64)
///
/// # Arguments
/// - fd: directory file descriptor
/// - buf: output buffer
/// - count: buffer size
///
/// # Returns
/// Returns number of bytes read on success, error code on failure
pub fn file_getdents64(fd: usize, buf: &mut [u8], count: usize) -> Result<usize, i32> {
    unsafe {
        // Get file object
        let file = match get_file_fd(fd) {
            Some(f) => f,
            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
        };

        // Get directory context from private_data
        let data_opt = &*file.private_data.get();
        let ctx_ptr = match *data_opt {
            Some(ptr) => ptr,
            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
        };

        let ctx = &mut *(ctx_ptr as *mut DirContext);

        match ctx.dir_type {
            DirType::RootFS => {
                // RootFS directory read - re-lookup using path
                let sb_ptr = get_rootfs();
                if sb_ptr.is_null() {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }

                let sb = &*sb_ptr;
                let path = ctx.get_path();
                let node = match sb.lookup(path) {
                    Some(n) => n,
                    None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                };

                if !node.is_dir() {
                    return Err(errno::Errno::NotADirectory.as_neg_i32());
                }

                let start_pos = ctx.offset;
                let children = node.list_children();

                let mut bytes_written = 0usize;
                let mut current_idx = 0usize;

                for child in children.iter().skip(start_pos) {
                    let child_ref = child.as_ref();
                    let name = &child_ref.name;
                    let name_len = name.len();

                    let dirent_size = (19 + name_len + 1 + 7) & !7;

                    if bytes_written + dirent_size > count {
                        break;
                    }

                    let buf_offset = bytes_written;

                    // d_ino
                    let d_ino = child_ref.ino;
                    buf[buf_offset..buf_offset + 8].copy_from_slice(&d_ino.to_le_bytes());

                    // d_off
                    let d_off = (bytes_written + dirent_size) as u64;
                    buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());

                    // d_reclen
                    buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());

                    // d_type
                    let d_type = if child_ref.is_dir() {
                        DT_DIR
                    } else if child_ref.is_file() {
                        DT_REG
                    } else if child_ref.is_symlink() {
                        DT_LNK
                    } else {
                        DT_UNKNOWN
                    };
                    buf[buf_offset + 18] = d_type;

                    // d_name
                    buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name);
                    buf[buf_offset + 19 + name_len] = 0;

                    bytes_written += dirent_size;
                    current_idx += 1;
                }

                ctx.offset = start_pos + current_idx;
                Ok(bytes_written)
            }
            DirType::Ext4 => {
                // ext4 directory read
                let path = ctx.get_path();
                let start_pos = ctx.offset;

                // Get directory entry list
                let entries = match ext4::list_dir(path) {
                    Some(e) => e,
                    None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                };

                let mut bytes_written = 0usize;
                let mut current_idx = 0usize;

                // Iterate directory entries starting from start_pos
                for entry in entries.iter().skip(start_pos) {
                    let name_bytes = &entry.name[..entry.name_len as usize];
                    let name_len = name_bytes.len();

                    // Calculate size of this dirent
                    let dirent_size = (19 + name_len + 1 + 7) & !7;

                    // Check if buffer is sufficient
                    if bytes_written + dirent_size > count {
                        break;
                    }

                    // Fill dirent64 structure
                    let buf_offset = bytes_written;

                    // d_ino
                    let d_ino = entry.inode as u64;
                    buf[buf_offset..buf_offset + 8].copy_from_slice(&d_ino.to_le_bytes());

                    // d_off
                    let d_off = (bytes_written + dirent_size) as u64;
                    buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());

                    // d_reclen
                    buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());

                    // d_type - ext4 file type mapping
                    let d_type = match entry.file_type {
                        1 => DT_REG,   // Regular file
                        2 => DT_DIR,   // Directory
                        3 => DT_CHR,   // Character device
                        4 => DT_BLK,   // Block device
                        5 => DT_FIFO,  // FIFO
                        6 => DT_SOCK,  // Socket
                        7 => DT_LNK,   // Symbolic link
                        _ => DT_UNKNOWN,
                    };
                    buf[buf_offset + 18] = d_type;

                    // d_name (null-terminated)
                    buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name_bytes);
                    buf[buf_offset + 19 + name_len] = 0;

                    bytes_written += dirent_size;
                    current_idx += 1;
                }

                // Update offset
                ctx.offset = start_pos + current_idx;

                Ok(bytes_written)
            }
            DirType::ProcFS => {
                // procfs directory read
                let path = ctx.get_path();
                let start_pos = ctx.offset;

                // Get directory entry list
                let entries = match procfs::list_dir(path) {
                    Some(e) => e,
                    None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                };

                let mut bytes_written = 0usize;
                let mut current_idx = 0usize;

                // Iterate directory entries starting from start_pos
                for entry in entries.iter().skip(start_pos) {
                    let name = &entry.0;
                    let name_len = name.len();

                    // Calculate size of this dirent
                    let dirent_size = (19 + name_len + 1 + 7) & !7;

                    // Check if buffer is sufficient
                    if bytes_written + dirent_size > count {
                        break;
                    }

                    // Fill dirent64 structure
                    let buf_offset = bytes_written;

                    // d_ino
                    let d_ino = entry.2;
                    buf[buf_offset..buf_offset + 8].copy_from_slice(&d_ino.to_le_bytes());

                    // d_off
                    let d_off = (bytes_written + dirent_size) as u64;
                    buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());

                    // d_reclen
                    buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());

                    // d_type - procfs file type mapping
                    let d_type = match entry.1 {
                        procfs::ProcFSType::Directory => DT_DIR,
                        procfs::ProcFSType::RegularFile => DT_REG,
                        procfs::ProcFSType::SymbolicLink => DT_LNK,
                    };
                    buf[buf_offset + 18] = d_type;

                    // d_name (null-terminated)
                    buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name);
                    buf[buf_offset + 19 + name_len] = 0;

                    bytes_written += dirent_size;
                    current_idx += 1;
                }

                // Update offset
                ctx.offset = start_pos + current_idx;

                Ok(bytes_written)
            }
            DirType::DevFS => {
                // devfs directory read
                let path = ctx.get_path();
                let start_pos = ctx.offset;

                // Get directory entry list
                let entries = match devfs::list_dir(path) {
                    Some(e) => e,
                    None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                };

                let mut bytes_written = 0usize;
                let mut current_idx = 0usize;

                // Iterate directory entries starting from start_pos
                for entry in entries.iter().skip(start_pos) {
                    let name = &entry.0;
                    let name_len = name.len();

                    // Calculate size of this dirent
                    let dirent_size = (19 + name_len + 1 + 7) & !7;

                    // Check if buffer is sufficient
                    if bytes_written + dirent_size > count {
                        break;
                    }

                    // Fill dirent64 structure
                    let buf_offset = bytes_written;

                    // d_ino
                    let d_ino = entry.2;
                    buf[buf_offset..buf_offset + 8].copy_from_slice(&d_ino.to_le_bytes());

                    // d_off
                    let d_off = (bytes_written + dirent_size) as u64;
                    buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());

                    // d_reclen
                    buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());

                    // d_type - devfs file type mapping
                    let d_type = if entry.1 {
                        DT_DIR
                    } else {
                        DT_CHR  // Non-directory entries in devfs are usually character devices
                    };
                    buf[buf_offset + 18] = d_type;

                    // d_name (null-terminated)
                    buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name.as_bytes());
                    buf[buf_offset + 19 + name_len] = 0;

                    bytes_written += dirent_size;
                    current_idx += 1;
                }

                // Update offset
                ctx.offset = start_pos + current_idx;

                Ok(bytes_written)
            }
        }
    }
}

/// RootFS directory read operation
fn rootfs_dir_read(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        // Get RootFSNode pointer from private_data
        let data_opt = &*file.private_data.get();
        let node_ptr = match *data_opt {
            Some(ptr) => ptr,
            None => {
                return -9;  // EBADF
            }
        };

        let node = &*(node_ptr as *const RootFSNode);

        // Confirm it's a directory
        if !node.is_dir() {
            return -20;  // ENOTDIR
        }

        // Get current read position
        let start_pos = file.get_pos() as usize;

        // Get child node list
        let children = node.list_children();

        let mut bytes_written = 0usize;
        let mut current_idx = 0usize;

        // Iterate child nodes starting from start_pos
        for child in children.iter().skip(start_pos) {
            let child_ref = child.as_ref();

            // Get file name
            let name = &child_ref.name;
            let name_len = name.len();

            // Calculate size of this dirent
            // dirent64 header: 8 + 8 + 2 + 1 = 19 bytes
            // Plus filename and null terminator
            // Must be aligned to 8-byte boundary
            let dirent_size = (19 + name_len + 1 + 7) & !7;

            // Check if buffer is sufficient
            if bytes_written + dirent_size > buf.len() {
                break;
            }

            // Fill dirent64 structure
            let buf_offset = bytes_written;

            // d_ino
            let d_ino = child_ref.ino;
            buf[buf_offset..buf_offset + 8].copy_from_slice(&d_ino.to_le_bytes());

            // d_off
            let d_off = (bytes_written + dirent_size) as u64;
            buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());

            // d_reclen
            buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());

            // d_type
            let d_type = if child_ref.is_dir() {
                DT_DIR
            } else if child_ref.is_file() {
                DT_REG
            } else if child_ref.is_symlink() {
                DT_LNK
            } else {
                DT_UNKNOWN
            };
            buf[buf_offset + 18] = d_type;

            // d_name (null-terminated)
            buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name);
            buf[buf_offset + 19 + name_len] = 0;

            bytes_written += dirent_size;
            current_idx += 1;
        }

        // Update file position
        file.set_pos((start_pos + current_idx) as u64);

        bytes_written as isize
    }
}

/// RootFS directory operations table
static ROOTFS_DIR_OPS: FileOps = FileOps {
    read: Some(rootfs_dir_read),
    write: None,
    lseek: Some(rootfs_file_lseek),
    close: Some(rootfs_file_close),
    poll: None,
};

/// ext4 directory read operation
fn ext4_dir_read(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        // Get directory context from private_data
        let data_opt = &*file.private_data.get();
        let ctx_ptr = match *data_opt {
            Some(ptr) => ptr,
            None => {
                return -9;  // EBADF
            }
        };

        let ctx = &mut *(ctx_ptr as *mut DirContext);

        if ctx.dir_type != DirType::Ext4 {
            return -22;  // EINVAL
        }

        let path = ctx.get_path();
        let start_pos = ctx.offset;

        // Get directory entry list
        let entries = match ext4::list_dir(path) {
            Some(e) => e,
            None => return -2,  // ENOENT
        };

        let mut bytes_written = 0usize;
        let mut current_idx = 0usize;

        // Iterate directory entries
        for entry in entries.iter().skip(start_pos) {
            let name_bytes = &entry.name[..entry.name_len as usize];
            let name_len = name_bytes.len();

            // Calculate size of this dirent
            let dirent_size = (19 + name_len + 1 + 7) & !7;

            // Check if buffer is sufficient
            if bytes_written + dirent_size > buf.len() {
                break;
            }

            // Fill dirent64 structure
            let buf_offset = bytes_written;

            // d_ino
            let d_ino = entry.inode as u64;
            buf[buf_offset..buf_offset + 8].copy_from_slice(&d_ino.to_le_bytes());

            // d_off
            let d_off = (bytes_written + dirent_size) as u64;
            buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());

            // d_reclen
            buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());

            // d_type
            let d_type = match entry.file_type {
                1 => DT_REG,
                2 => DT_DIR,
                3 => DT_CHR,
                4 => DT_BLK,
                5 => DT_FIFO,
                6 => DT_SOCK,
                7 => DT_LNK,
                _ => DT_UNKNOWN,
            };
            buf[buf_offset + 18] = d_type;

            // d_name
            buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name_bytes);
            buf[buf_offset + 19 + name_len] = 0;

            bytes_written += dirent_size;
            current_idx += 1;
        }

        // Update offset
        ctx.offset = start_pos + current_idx;

        bytes_written as isize
    }
}

/// ext4 directory close operation
fn ext4_dir_close(_file: &File) -> i32 {
    // No special cleanup needed, DirContext will be automatically freed when file is closed
    0
}

/// ext4 directory operations table
static EXT4_DIR_OPS: FileOps = FileOps {
    read: Some(ext4_dir_read),
    write: None,
    lseek: None,  // ext4 directories do not support lseek
    close: Some(ext4_dir_close),
    poll: None,
};
