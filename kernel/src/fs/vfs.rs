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

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use crate::sync::spinlock::Spinlock;

use crate::errno;
use crate::fs::file::{File, FileFlags, FileOps, get_file_fd, close_file_fd, get_file_fd_install};
use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps, VfsDirEntry, setattr_attr};
use crate::fs::dentry::{Dentry, VfsMountInternal};
use crate::fs::mount::MntFlags;
use crate::fs::Stat;
use crate::fs::path::path_normalize;

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
    /// Global VFS root dentry — the top of the dentry tree
    root_dentry: Option<Arc<Dentry>>,
    initialized: bool,
}

static VFS_STATE: Spinlock<VfsState> = Spinlock::new(VfsState {
    root_dentry: None,
    initialized: false,
});

/// Get the global VFS root dentry.
pub fn get_vfs_root() -> Option<Arc<Dentry>> {
    VFS_STATE.lock().root_dentry.clone()
}

/// If `dentry` is a mount point, return the mounted filesystem's root dentry.
/// Otherwise return `dentry` itself.
pub fn follow_mount(dentry: Arc<Dentry>) -> Arc<Dentry> {
    let mount = dentry.get_mount();
    match mount {
        Some(mnt) => mnt.root.clone(),
        None => dentry,
    }
}

/// Mount a filesystem at the given path, building the dentry tree.
///
/// This replaces the old `mount_at()` string-based routing with dentry tree
/// construction. The dentry tree allows `path_lookup()` to walk the tree
/// and cross mount points via `follow_mount()`.
pub fn vfs_mount(
    mountpoint: &str,
    root_inode: Arc<Inode>,
    mnt_flags: MntFlags,
) {
    let mut state = VFS_STATE.lock();

    // Ensure VFS root dentry exists
    let vfs_root = match state.root_dentry.clone() {
        Some(d) => d,
        None => {
            let d = Arc::new(Dentry::new(String::from("/")));
            d.set_hashed();
            state.root_dentry = Some(d.clone());
            d
        }
    };

    // Create the mounted filesystem's root dentry
    let mounted_root = Arc::new(Dentry::new(String::from("/")));
    mounted_root.set_inode(root_inode);
    mounted_root.set_hashed();

    // Walk from VFS root to the mount point, creating intermediate dentries
    // Special case: mounting at "/" means we overlay the VFS root's inode.
    // We do NOT set a VfsMountInternal here because that would cause follow_mount
    // to jump to a new dentry without children. Instead, we directly replace the
    // root dentry's inode, so children (proc, dev, etc.) remain accessible.
    if mountpoint == "/" {
        vfs_root.set_inode(mounted_root.get_inode().unwrap());
        return;
    }

    // For non-root mountpoints (e.g., "/dev", "/proc"), walk from VFS root
    let components: Vec<&str> = mountpoint
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let mut current = vfs_root.clone();
    current = follow_mount(current);

    for (i, component) in components.iter().enumerate() {
        let is_last = i == components.len() - 1;
        let name = String::from(*component);

        if is_last {
            // This is the mount point — create/replace the dentry and attach mount
            let child = match current.lookup_child(&name) {
                Some(existing) => existing,
                None => {
                    let d = Arc::new(Dentry::new(name.clone()));
                    d.set_parent(current.clone());
                    current.add_child(name.clone(), d.clone());
                    d
                }
            };

            // Create mount descriptor
            let mnt_desc = Arc::new(VfsMountInternal {
                root: mounted_root.clone(),
                flags: mnt_flags,
            });
            child.set_mount(mnt_desc);
        } else {
            // Intermediate component — create if not exists
            let child = match current.lookup_child(&name) {
                Some(existing) => follow_mount(existing),
                None => {
                    let d = Arc::new(Dentry::new(name.clone()));
                    d.set_parent(current.clone());
                    current.add_child(name.clone(), d.clone());
                    d
                }
            };
            current = child;
        }
    }
}

// ============================================================================
// Filesystem Type Enumeration
// ============================================================================

/// Unmount a filesystem from the dentry tree.
///
/// Removes the mount point dentry (and its `VfsMountInternal`) from the parent.
/// The root "/" cannot be unmounted.
pub fn vfs_umount(mountpoint: &str) -> Result<(), i32> {
    if mountpoint == "/" {
        return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
    }

    let (parent_path, name) = path_parent_and_name(mountpoint)?;
    let parent_vpath = path_lookup(&parent_path, 0)?;
    let parent_dentry = parent_vpath.dentry
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Remove the mount point dentry (drops VfsMountInternal)
    parent_dentry.remove_child(&name);
    Ok(())
}

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

impl FsType {
    /// Parse filesystem type from a string (e.g., "ext4", "proc", "devfs").
    pub fn from_str(s: &str) -> Result<Self, i32> {
        match s {
            "ext4" => Ok(FsType::Ext4),
            "proc" | "procfs" => Ok(FsType::ProcFS),
            "devfs" | "devtmpfs" => Ok(FsType::DevFS),
            "rootfs" | "ramfs" => Ok(FsType::RootFS),
            _ => Err(errno::Errno::InvalidArgument.as_neg_i32()),
        }
    }
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
        // Create VFS root dentry (inode will be set by first vfs_mount("/"))
        let root = Arc::new(Dentry::new(String::from("/")));
        root.set_hashed();
        state.root_dentry = Some(root);
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
/// Returns (filesystem_type, relative_path_within_filesystem)
///
/// The relative path preserves the leading "/" separator after the mount point.
/// For example:
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

/// Unified path lookup — dentry tree traversal with mount point crossing.
///
/// This function resolves a pathname by walking the dentry tree.
/// At each level, it checks for mount points via `follow_mount()`.
///
/// # Arguments
/// - `pathname`: Path to resolve (absolute or relative)
/// - `flags`: Lookup flags (LOOKUP_FOLLOW, LOOKUP_DIRECTORY, etc.)
///
/// # Returns
/// - `Ok(VfsPath)`: Resolved path with dentry and inode
/// - `Err(errno)`: Error code
pub fn path_lookup(pathname: &str, _flags: u32) -> Result<VfsPath, i32> {
    // Empty path is invalid
    if pathname.is_empty() {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // Convert to absolute path and normalize
    let abs_path = make_absolute(pathname);
    let normalized = path_normalize(&abs_path);

    // Get VFS root dentry
    let vfs_root = VFS_STATE.lock().root_dentry.clone()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Start from root, follow mount
    let mut current = follow_mount(vfs_root);
    let mut symlink_depth: usize = 0;

    // Split into path components, skip empty ones
    let components: Vec<&str> = normalized
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    for (ci, component) in components.iter().enumerate() {
        // Skip "." — current directory
        if *component == "." {
            continue;
        }

        // Handle ".." — parent directory
        if *component == ".." {
            let parent_name = current.get_name();
            if parent_name == "/" {
                // Already at root, stay
                continue;
            }
            let parent_opt = current.parent.lock().clone();
            match parent_opt {
                Some(p) => {
                    // Go to parent, then follow mount (for mount point traversal)
                    current = follow_mount(p);
                }
                None => {
                    // No parent (shouldn't happen), stay
                    continue;
                }
            }
            continue;
        }

        // Look up child in dentry tree
        let child = match current.lookup_child(component) {
            Some(c) => {
                // Negative dentry — file known not to exist
                if c.is_negative() {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
                // Check icache for fresh inode data
                if let Some(ref cached_inode) = c.get_inode() {
                    if let Some(fresh) = crate::fs::inode::icache_lookup(cached_inode.ino, cached_inode.fs_id) {
                        c.set_inode(fresh);
                    }
                }
                c
            }
            None => {
                // Not in dentry cache — ask the filesystem to look it up
                let dir_inode = current.get_inode()
                    .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
                let ops = dir_inode.ops.as_ref()
                    .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

                unsafe {
                    // Call lookup to get inode number
                    let ino = match ops.lookup {
                        Some(lookup_fn) => {
                            match lookup_fn(&*dir_inode, component.as_bytes()) {
                                Ok(ino) => ino,
                                Err(e) => {
                                    // Cache negative dentry on ENOENT
                                    if e == -(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()) {
                                        let name = String::from(*component);
                                        let d = Arc::new(Dentry::new(name.clone()));
                                        d.set_negative();
                                        d.set_parent(current.clone());
                                        current.add_child(name, d);
                                    }
                                    return Err(e);
                                }
                            }
                        }
                        None => return Err(errno::Errno::NotADirectory.as_neg_i32()),
                    };

                    // Call iget to instantiate the VFS Inode
                    let child_inode = match ops.iget {
                        Some(iget_fn) => iget_fn(&*dir_inode, component.as_bytes(), ino)?,
                        None => return Err(errno::Errno::NotADirectory.as_neg_i32()),
                    };

                    // Create new dentry and cache it
                    let name = String::from(*component);
                    let d = Arc::new(Dentry::new(name.clone()));
                    d.set_inode(child_inode.clone());
                    d.set_parent(current.clone());
                    current.add_child(name.clone(), d.clone());

                    // Add to icache
                    crate::fs::inode::icache_add(child_inode);

                    // Move to child, follow mount, follow symlink
                    current = follow_mount(d);
                    current = follow_symlink(current, &components, &mut symlink_depth)?;
                    continue;
                }
            }
        };

        // Follow mount point at child, then follow symlink
        current = follow_mount(child);
        current = follow_symlink(current, &components, &mut symlink_depth)?;
    }

    // Build VfsPath from final dentry
    let dentry = current.clone();
    let inode = dentry.get_inode()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    Ok(VfsPath {
        dentry: Some(dentry),
        mnt: None,
        inode: Some(inode),
    })
}

/// Follow symbolic link: if dentry's inode is a symlink, resolve its target.
/// `remaining` is the remaining path components (not yet processed).
/// `depth` tracks nesting to prevent loops (max 8).
fn follow_symlink(
    dentry: alloc::sync::Arc<Dentry>,
    remaining: &Vec<&str>,
    depth: &mut usize,
) -> Result<alloc::sync::Arc<Dentry>, i32> {
    let inode = match dentry.get_inode() {
        Some(i) => i,
        None => return Ok(dentry),
    };

    if !inode.mode.is_symlink() {
        return Ok(dentry);
    }



    *depth += 1;
    if *depth > 8 {
        return Err(errno::Errno::TooManySymbolicLinks.as_neg_i32());
    }

    // Read symlink target
    let mut target_buf = [0u8; 4096];
    let target_len = inode.op_readlink(&mut target_buf);
    if target_len <= 0 {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    let target = core::str::from_utf8(&target_buf[..target_len as usize])
        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    // Resolve target path relative to the symlink's parent directory
    let vfs_root = VFS_STATE.lock().root_dentry.clone()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let base = if target.starts_with('/') {
        // Absolute symlink — start from VFS root
        follow_mount(vfs_root)
    } else {
        // Relative symlink — start from symlink's parent
        let parent_opt = dentry.parent.lock().clone();
        match parent_opt {
            Some(p) => follow_mount(p),
            None => follow_mount(vfs_root),
        }
    };

    // Parse target path components
    let target_components: Vec<&str> = target
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect();

    let mut current = base;
    for component in target_components.iter() {
        if *component == ".." {
            let parent_opt = current.parent.lock().clone();
            match parent_opt {
                Some(p) => current = follow_mount(p),
                None => {}
            }
            continue;
        }

        // Look up in dentry cache or ask filesystem
        let child = match current.lookup_child(component) {
            Some(c) => {
                // Negative dentry — file known not to exist
                if c.is_negative() {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
                // Check icache for fresh inode data
                if let Some(ref cached_inode) = c.get_inode() {
                    if let Some(fresh) = crate::fs::inode::icache_lookup(cached_inode.ino, cached_inode.fs_id) {
                        c.set_inode(fresh);
                    }
                }
                c
            }
            None => {
                let dir_inode = current.get_inode()
                    .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
                let ops = dir_inode.ops.as_ref()
                    .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

                unsafe {
                    let ino = match ops.lookup {
                        Some(lookup_fn) => {
                            match lookup_fn(&*dir_inode, component.as_bytes()) {
                                Ok(ino) => ino,
                                Err(e) => {
                                    if e == -(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()) {
                                        let name = String::from(*component);
                                        let d = alloc::sync::Arc::new(Dentry::new(name.clone()));
                                        d.set_negative();
                                        d.set_parent(current.clone());
                                        current.add_child(name, d);
                                    }
                                    return Err(e);
                                }
                            }
                        }
                        None => return Err(errno::Errno::NotADirectory.as_neg_i32()),
                    };
                    let child_inode = match ops.iget {
                        Some(iget_fn) => iget_fn(&*dir_inode, component.as_bytes(), ino)?,
                        None => return Err(errno::Errno::NotADirectory.as_neg_i32()),
                    };
                    let name = String::from(*component);
                    let d = alloc::sync::Arc::new(Dentry::new(name.clone()));
                    d.set_inode(child_inode.clone());
                    d.set_parent(current.clone());
                    current.add_child(name.clone(), d.clone());
                    crate::fs::inode::icache_add(child_inode);
                    current = follow_mount(d);
                    current = follow_symlink(current, remaining, depth)?;
                    continue;
                }
            }
        };
        current = follow_mount(child);
        current = follow_symlink(current, remaining, depth)?;
    }

    Ok(current)
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
            let new_inode = mkdir_fn(parent_inode.as_ref(), name.as_bytes(), inode_mode)?;

            // Invalidate negative dentry and cache the new one
            if let Some(ref parent_dentry) = parent_vpath.dentry {
                parent_dentry.remove_child(&name);
                let d = Arc::new(Dentry::new(name.clone()));
                d.set_inode(new_inode);
                d.set_parent(parent_dentry.clone());
                parent_dentry.add_child(name, d);
            }

            Ok(())
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Create symbolic link - unified implementation using inode_operations
pub fn vfs_symlink(pathname: &str, target: &str) -> Result<(), i32> {
    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;

    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    unsafe {
        if let Some(symlink_fn) = ops.symlink {
            let new_inode = symlink_fn(parent_inode.as_ref(), name.as_bytes(), target.as_bytes())?;

            // Invalidate negative dentry and cache the new one
            if let Some(ref parent_dentry) = parent_vpath.dentry {
                parent_dentry.remove_child(&name);
                let d = Arc::new(Dentry::new(name.clone()));
                d.set_inode(new_inode);
                d.set_parent(parent_dentry.clone());
                parent_dentry.add_child(name, d);
            }

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
                // Replace dentry with negative entry
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    if let Some(child) = parent_dentry.lookup_child(&name) {
                        child.set_negative();
                        *child.inode.lock() = None;
                    }
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
                // Replace dentry with negative entry
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    if let Some(child) = parent_dentry.lookup_child(&name) {
                        child.set_negative();
                        *child.inode.lock() = None;
                    }
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
                // Invalidate stale/negative dentry at new path
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    parent_dentry.remove_child(&name);
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
        // Invalidate stale dentries at both old and new paths
        if let Some(ref old_pd) = old_parent_vpath.dentry {
            old_pd.remove_child(&old_name);
        }
        if let Some(ref new_pd) = new_parent_vpath.dentry {
            new_pd.remove_child(&new_name);
        }
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
        let o_creat = (flags & FileFlags::O_CREAT) != 0;
        let o_excl = (flags & FileFlags::O_EXCL) != 0;
        let o_trunc = (flags & FileFlags::O_TRUNC) != 0;

        // Step 1: Resolve path through dentry tree
        let inode = match path_lookup(filename, 0) {
            Ok(vpath) => {
                if o_excl && o_creat {
                    return Err(errno::Errno::FileExists.as_neg_i32());
                }
                vpath.inode.ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?
            }
            Err(_e) if o_creat => {
                let (parent_path, child_name) = path_parent_and_name(filename)?;
                let parent_vpath = path_lookup(&parent_path, 0)?;
                let parent_inode = parent_vpath.inode
                    .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
                let ops = parent_inode.ops.as_ref()
                    .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
                let new_inode = {
                    let create_fn = ops.create
                        .ok_or(errno::Errno::PermissionDenied.as_neg_i32())?;
                    create_fn(&*parent_inode, child_name.as_bytes(), crate::fs::inode::InodeMode::new(mode))?
                };
                // Cache new dentry (replace stale/negative dentry)
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    let name = String::from(child_name.as_str());
                    parent_dentry.remove_child(&name);
                    let d = Arc::new(Dentry::new(name.clone()));
                    d.set_inode(Arc::clone(&new_inode));
                    d.set_parent(parent_dentry.clone());
                    parent_dentry.add_child(name, d);
                }
                new_inode
            }
            Err(e) => return Err(e),
        };

        // Directory -> redirect to opendir
        if inode.mode.is_directory() {
            return file_opendir(filename, flags | 0o00200000);
        }

        // Create File object
        let file_flags = FileFlags::new(flags);
        let file = Arc::new(File::new(file_flags));
        file.set_inode(Arc::clone(&inode));

        // Get FileOps from inode callback
        if let Some(ops) = inode.ops {
            if let Some(get_file_ops_fn) = ops.get_file_ops {
                if let Some(file_ops) = get_file_ops_fn(&*inode) {
                    file.set_ops(file_ops);
                }
            }
            // Call open callback (e.g., procfs pre-read)
            if let Some(open_fn) = ops.open {
                let result = open_fn(&*inode, &*file);
                if result != 0 {
                    return Err(result);
                }
            }
        }

        // Handle O_TRUNC via setattr
        if o_trunc && inode.mode.is_regular_file() {
            let result = inode.op_setattr(
                crate::fs::inode::setattr_attr::ATTR_SIZE, 0, 0
            );
            if result != 0 {
                return Err(result);
            }
        }

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

/// Get file status by fd (fstat)
pub fn file_stat(fd: usize, stat: &mut Stat) -> Result<(), i32> {
    unsafe {
        let file = match get_file_fd(fd) {
            Some(f) => f,
            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
        };
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt.as_ref() {
            Some(i) => i,
            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
        };
        let result = inode.op_getattr(stat);
        if result == 0 {
            Ok(())
        } else {
            Err(result)
        }
    }
}

/// Get file status by path (for fstatat)
pub fn stat_file_by_path(path: &str, stat: &mut Stat) -> Result<(), i32> {
    vfs_stat(path, stat)
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
// Directory operations (for getdents64 system call)
// ============================================================================

/// Open directory (for getdents64)
///
/// # Arguments
/// - pathname: directory path
/// - flags: open flags
///
/// # Returns
/// Returns file descriptor on success, error code on failure
pub fn file_opendir(pathname: &str, flags: u32) -> Result<usize, i32> {
    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    if !inode.mode.is_directory() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    let file_flags = FileFlags::new(flags);
    let file = Arc::new(File::new(file_flags));
    file.set_inode(Arc::clone(&inode));

    unsafe {
        // Get directory file ops from inode callback
        if let Some(ops) = inode.ops {
            if let Some(get_file_ops_fn) = ops.get_file_ops {
                if let Some(dir_ops) = get_file_ops_fn(&*inode) {
                    file.set_ops(dir_ops);
                }
            }
        }

        match get_file_fd_install(file) {
            Some(fd) => Ok(fd),
            None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
        }
    }
}

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
        let file = match get_file_fd(fd) {
            Some(f) => f,
            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
        };

        let inode_opt = &*file.inode.get();
        let inode = match inode_opt.as_ref() {
            Some(i) => i,
            None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
        };

        if !inode.mode.is_directory() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        // Call readdir through inode.ops
        let entries = if let Some(ops) = inode.ops {
            if let Some(readdir_fn) = ops.readdir {
                readdir_fn(&*inode)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?
            } else {
                return Err(errno::Errno::NotADirectory.as_neg_i32());
            }
        } else {
            return Err(errno::Errno::BadFileNumber.as_neg_i32());
        };

        let start_pos = file.get_pos() as usize;
        let mut bytes_written = 0usize;
        let mut current_idx = 0usize;

        for entry in entries.iter().skip(start_pos) {
            let name = &entry.name;
            let name_len = name.len();
            let dirent_size = (19 + name_len + 1 + 7) & !7;

            if bytes_written + dirent_size > count {
                break;
            }

            let buf_offset = bytes_written;
            buf[buf_offset..buf_offset + 8].copy_from_slice(&entry.ino.to_le_bytes());
            let d_off = (bytes_written + dirent_size) as u64;
            buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());
            buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());
            buf[buf_offset + 18] = entry.file_type;
            buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name);
            buf[buf_offset + 19 + name_len] = 0;

            bytes_written += dirent_size;
            current_idx += 1;
        }

        file.set_pos((start_pos + current_idx) as u64);
        Ok(bytes_written)
    }
}

// ============================================================================
// Memory File (for procfs shortcut)
// ============================================================================

/// In-memory file content (stored in File's private_data)
struct MemFileContent {
    data: alloc::vec::Vec<u8>,
    offset: usize,
}

/// Read operation for memory files
fn mem_file_read(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            let content = &*(content_ptr as *const MemFileContent);
            let offset = file.get_pos() as usize;
            let remaining = content.data.len().saturating_sub(offset);
            let to_read = remaining.min(buf.len());
            buf[..to_read].copy_from_slice(&content.data[offset..offset + to_read]);
            file.set_pos((offset + to_read) as u64);
            to_read as isize
        } else {
            0
        }
    }
}

/// Lseek operation for memory files
fn mem_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            let content = &*(content_ptr as *const MemFileContent);
            let file_size = content.data.len() as isize;
            let new_offset = match whence {
                0 => offset,
                1 => file.get_pos() as isize + offset,
                2 => file_size + offset,
                _ => return -22,
            };
            if new_offset < 0 || new_offset > file_size {
                return -22;
            }
            file.set_pos(new_offset as u64);
            new_offset
        } else {
            -9 // EBADF
        }
    }
}

/// Close operation for memory files
fn mem_file_close(file: &File) -> i32 {
    unsafe {
        let data_opt = &mut *file.private_data.get();
        if let Some(content_ptr) = data_opt.take() {
            let _ = alloc::boxed::Box::from_raw(content_ptr as *mut MemFileContent);
        }
    }
    0
}

static MEM_FILE_OPS: FileOps = FileOps {
    read: Some(mem_file_read),
    write: None,
    lseek: Some(mem_file_lseek),
    close: Some(mem_file_close),
    poll: None,
};

// ============================================================================
// ProcFS Directory (for /proc/[pid]/fd/ shortcut)
// ============================================================================

/// Synthetic INodeOps for procfs directory shortcuts (e.g., /proc/[pid]/fd/)
static PROCFS_DIR_OPS: INodeOps = INodeOps {
    lookup: None,
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: None,
    readdir: Some(procfs_dir_readdir),
    open: None,
    permission: None,
    getattr: None,
    setattr: None,
    iget: None,
};

/// Readdir callback for synthetic procfs directories.
/// Reads PID from inode.private_data, calls procfs::pid::list_fds().
unsafe fn procfs_dir_readdir(inode: &Inode) -> Option<alloc::vec::Vec<VfsDirEntry>> {
    let pid = inode.private_data? as u64;
    let fds = crate::fs::procfs::pid::list_fds(pid);

    let entries: alloc::vec::Vec<VfsDirEntry> = fds.iter().map(|(fd, _path)| {
        VfsDirEntry {
            ino: *fd as u64,
            name: alloc::format!("{}", fd).into_bytes(),
            file_type: crate::fs::inode::file_type::DT_LNK,
        }
    }).collect();

    Some(entries)
}

/// Open a synthetic procfs directory (e.g., /proc/[pid]/fd/), return fd.
///
/// Creates a File backed by a synthetic Inode with readdir support.
pub fn open_procfs_dir(pid: u64, flags: u32) -> Result<usize, i32> {
    unsafe {
        let mut inode = Inode::new(
            pid,
            InodeMode::new(InodeMode::S_IFDIR | 0o555),
        );
        inode.ops = Some(&PROCFS_DIR_OPS);
        inode.private_data = Some(pid as *mut u8);

        let file = alloc::sync::Arc::new(File::new(FileFlags::new(flags)));
        *file.inode.get() = Some(alloc::sync::Arc::new(inode));

        get_file_fd_install(file).ok_or(errno::Errno::TooManyOpenFiles.as_neg_i32())
    }
}

/// Open a memory-backed file with given content, return fd
pub fn open_mem_file(data: alloc::vec::Vec<u8>, flags: u32) -> Result<usize, i32> {
    unsafe {
        let file = Arc::new(File::new(FileFlags::new(flags)));
        file.set_ops(&MEM_FILE_OPS);
        let content = alloc::boxed::Box::new(MemFileContent { data, offset: 0 });
        file.set_private_data(alloc::boxed::Box::into_raw(content) as *mut u8);
        get_file_fd_install(file).ok_or(errno::Errno::TooManyOpenFiles.as_neg_i32())
    }
}
