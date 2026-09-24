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

/// NAME_MAX (Linux): longest single path component we will ever look up.
pub const NAME_MAX: usize = 255;

/// Coarse-grained VFS namespace mutation lock (review 5.1: O_CREAT two-step
/// race — SMP double-create of the same name).
///
/// Design note (minimal correct scheme): a single global lock serializes the
/// lookup-then-mutate window of directory-modifying operations (create via
/// O_CREAT, mkdir, symlink, link, unlink, rmdir, rename). Per-parent hashing
/// would reduce contention but requires stable parent identity across the
/// dentry/mount walk; until dentries are fully refcount-pinned for the whole
/// operation, a single lock is the only scheme that cannot be defeated by a
/// rename of an ancestor between hash and use. ext4 additionally takes
/// EXT4_BIG_LOCK inside its ops, so mutual exclusion composes.
pub static VFS_MUTATION_LOCK: Spinlock<()> = Spinlock::new(());


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
    crate::pr_info!("vfs: Initializing Virtual File System...");

    {
        let mut state = VFS_STATE.lock();
        // Create VFS root dentry (inode will be set by first vfs_mount("/"))
        let root = Arc::new(Dentry::new(String::from("/")));
        root.set_hashed();
        state.root_dentry = Some(root);
        state.initialized = true;
    }

    crate::pr_info!("vfs: VFS layer initialized [OK]");
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
        // Verify alignment of the underlying pointer (fixes H62).
        let ptr = current as *const crate::process::task::Task;
        if (ptr as usize) % core::mem::align_of::<crate::process::task::Task>() != 0 {
            return String::from("/");
        }
        let cwd_bytes = current.get_cwd();
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

// ============================================================================
// chroot (P1): per-task root prefix in path resolution
// ============================================================================

/// The current task's root path ("/" when not chrooted). The root is stored
/// as a REAL (global-namespace) path in fs_struct; a chrooted task resolves
/// absolute paths by prefixing it. Kernel threads / early boot see "/".
pub fn get_process_root() -> String {
    if let Some(current) = crate::sched::current() {
        let root = current.get_root();
        match core::str::from_utf8(&root) {
            Ok(s) if !s.is_empty() => {
                // Normalize (the stored root was normalized at chroot time,
                // but be defensive against a stale FDT-shared value).
                let n = path_normalize(s);
                String::from(n)
            }
            _ => String::from("/"),
        }
    } else {
        String::from("/")
    }
}

/// Whether the current task runs under a chroot (root != "/").
pub fn chrooted() -> bool {
    get_process_root() != "/"
}

/// Interpret `path` in the current task's namespace: absolute paths get the
/// chroot prefix prepended; relative paths stay cwd-joined (cwd is stored
/// as a real path, so the result is always a real-namespace path).
fn make_absolute_rooted(path: &str) -> String {
    if !path.starts_with('/') {
        return make_absolute(path);
    }
    let root = get_process_root();
    if root == "/" {
        return String::from(path);
    }
    let trimmed = root.trim_end_matches('/');
    debug_assert!(!trimmed.is_empty());
    format!("{}{}", trimmed, path)
}

/// Is `abs` (an unnormalized real-namespace path) at or under `root`?
fn is_under_root(abs: &str, root: &str) -> bool {
    if root == "/" {
        return true;
    }
    let trimmed = root.trim_end_matches('/');
    abs == trimmed || abs.starts_with(&format!("{}/", trimmed))
}

/// Lexical normalization with a chroot floor: ".." never pops above
/// `root`'s component depth (Linux follow_dotdot stops at nd->root). When
/// the path is not under the root (cwd outside the jail — allowed after
/// chroot in Linux too, the classic fchdir escape) the floor is the real
/// root, i.e. plain path_normalize semantics.
fn normalize_with_root(abs: &str, root: &str) -> String {
    if root == "/" || !abs.starts_with('/') {
        return path_normalize(abs);
    }

    let floor_len = if is_under_root(abs, root) {
        root.split('/').filter(|s| !s.is_empty()).count()
    } else {
        0
    };

    let mut stack: Vec<&str> = Vec::new();
    for comp in abs.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if comp == ".." {
            if stack.len() > floor_len {
                stack.pop();
            }
            // else: at the chroot floor — ".." stays put (the clamp that
            // makes `chroot("/jail"); open("/..") == "/jail"` hold).
        } else {
            stack.push(comp);
        }
    }

    let mut out = String::from("/");
    for (i, c) in stack.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(c);
    }
    out
}

/// Walk the dentry tree from the VFS root to `path` (a REAL-namespace,
/// already-normalized path), no symlink following, ".." clamped at the
/// process root. Used to find the task's root dentry for absolute-symlink
/// resolution under chroot.
fn dentry_walk_no_symlink(path: &str) -> Option<Arc<Dentry>> {
    let mut current = follow_mount(
        VFS_STATE.lock().root_dentry.clone()?,
    );
    for comp in path.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if comp == ".." {
            let parent = current.parent.lock().clone()?;
            current = follow_mount(parent);
            continue;
        }
        let child = current.lookup_child(comp)?;
        if child.is_negative() {
            return None;
        }
        current = follow_mount(child);
    }
    Some(current)
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
/// Lookup flags
pub const LOOKUP_FOLLOW: u32 = 0x01;    // Follow symlinks (default)
pub const LOOKUP_NOFOLLOW: u32 = 0x02;  // Don't follow final symlink

pub fn path_lookup(pathname: &str, flags: u32) -> Result<VfsPath, i32> {
    // Empty path is invalid
    if pathname.is_empty() {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // Convert to absolute path and normalize. P1 chroot: absolute paths are
    // interpreted in the CURRENT TASK'S namespace (root prefix), and the
    // normalization floor clamps ".." at that root — a chrooted process
    // cannot lexically climb out of the jail.
    let abs_path = make_absolute_rooted(pathname);
    let normalized = normalize_with_root(&abs_path, &get_process_root());

    // Get VFS root dentry
    let vfs_root = VFS_STATE.lock().root_dentry.clone()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Start from root, follow mount. P1 chroot: also resolve the task's
    // root dentry (when chrooted) for the ".." clamp below and for absolute
    // symlink bases in follow_symlink.
    let mut current = follow_mount(vfs_root.clone());
    let mut symlink_depth: usize = 0;
    let task_root_dentry: Option<Arc<Dentry>> = {
        let root = get_process_root();
        if root == "/" {
            None
        } else {
            dentry_walk_no_symlink(&root)
        }
    };

    // Split into path components, skip empty ones
    let components: Vec<&str> = normalized
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    for (ci, component) in components.iter().enumerate() {
        // NAME_MAX check (review 5.5): components longer than 255 bytes can
        // never exist on any of our filesystems — fail with ENAMETOOLONG
        // instead of truncating into u8 name_len fields downstream.
        if component.len() > NAME_MAX {
            return Err(-(errno::constants::ENAMETOOLONG));
        }

        // DAC: search (x) permission is required on every directory we
        // traverse through (Linux checks MAY_EXEC per component).
        if let Some(ref dir_inode) = current.get_inode() {
            if !crate::fs::permission::inode_permission(dir_inode, crate::fs::permission::MAY_EXEC) {
                return Err(errno::Errno::PermissionDenied.as_neg_i32());
            }
        }

        // Skip "." — current directory
        if *component == "." {
            continue;
        }

        // Handle ".." — parent directory. The floor normalization above
        // already removed ".." components, so this arm is defense-in-depth;
        // it additionally clamps at the PROCESS root dentry (chroot) so a
        // stray ".." can never walk out of the jail via the dentry tree.
        if *component == ".." {
            if let Some(ref root_d) = task_root_dentry {
                if Arc::ptr_eq(&current, root_d) {
                    continue; // at (task) root, stay
                }
            }
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

                // SAFETY: ops.lookup is a VFS callback with a well-defined contract;
                // dir_inode Arc dereference is valid for the scope of this block
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
                    // Respect LOOKUP_NOFOLLOW on the final component here as
                    // well — lstat must not depend on dentry cache state.
                    let is_last = ci == components.len() - 1;
                    if is_last && (flags & LOOKUP_NOFOLLOW) != 0 {
                        // Don't follow symlink on the final component
                    } else {
                        current = follow_symlink(current, &components, &mut symlink_depth)?;
                    }
                    continue;
                }
            }
        };

        // Follow mount point at child, then follow symlink
        current = follow_mount(child);
        // If LOOKUP_NOFOLLOW and this is the last component, skip symlink resolution
        let is_last = ci == components.len() - 1;
        if is_last && (flags & LOOKUP_NOFOLLOW) != 0 {
            // Don't follow symlink on the final component
        } else {
            current = follow_symlink(current, &components, &mut symlink_depth)?;
        }
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
    // Linux MAXSYMLINKS = 40 (review 5.1: was 8, rejecting deep-but-legal
    // symlink chains that glibc/coreutils happily create).
    if *depth > crate::config::MAX_SYMLINKS {
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

    // Resolve target path relative to the symlink's parent directory.
    let vfs_root = VFS_STATE.lock().root_dentry.clone()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // P1 chroot: an ABSOLUTE symlink target is interpreted in the CURRENT
    // TASK'S namespace (Linux: absolute symlinks resolve from nd->root, not
    // the global root). Walk the (normalized) root prefix to its dentry;
    // ".." inside the target is clamped at that dentry below.
    let task_root_dentry: Arc<Dentry> = {
        let root = get_process_root();
        if root == "/" {
            vfs_root.clone()
        } else {
            let rooted = normalize_with_root(&root, &root);
            dentry_walk_no_symlink(&rooted).unwrap_or_else(|| vfs_root.clone())
        }
    };

    let base = if target.starts_with('/') {
        // Absolute symlink — start from the task's root
        follow_mount(task_root_dentry.clone())
    } else {
        // Relative symlink — start from symlink's parent
        let parent_opt = dentry.parent.lock().clone();
        match parent_opt {
            Some(p) => follow_mount(p),
            None => follow_mount(task_root_dentry.clone()),
        }
    };

    // Parse target path components
    let target_components: Vec<&str> = target
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    let mut current = base;
    for component in target_components.iter() {
        // DAC (B4): resolving a symlink body walks directories the caller
        // never named.  Linux checks search (MAY_EXEC) permission on every
        // directory traversed during symlink expansion, exactly like the
        // path_lookup main loop above; without this, a world-readable
        // symlink pointing into a 0700 directory bypassed the traversal
        // check entirely (open /link succeeded where open /dir/file got
        // EACCES).  Nested symlinks re-enter follow_symlink and hit the
        // same check on their own walks.
        if let Some(ref dir_inode) = current.get_inode() {
            if !crate::fs::permission::inode_permission(dir_inode, crate::fs::permission::MAY_EXEC) {
                return Err(errno::Errno::PermissionDenied.as_neg_i32());
            }
        }

        if *component == ".." {
            // Clamp at the task root (chroot) — an absolute symlink like
            // "/../../etc" must not climb out of the jail.
            if Arc::ptr_eq(&current, &task_root_dentry) {
                continue;
            }
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

                // SAFETY: VFS callback contract; dir_inode Arc is valid in scope
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
        crate::process::task::Cred::new_init()
    };
    if !crate::fs::permission::generic_permission(
        inode_mode, inode_uid, inode_gid,
        crate::fs::permission::MAY_WRITE, &cred,
    ) {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }
    Ok(())
}

/// Check MAY_EXEC on a parent inode: directory modification ops on Linux
/// require search permission on the parent in addition to write permission
/// (may_create/may_delete check MAY_WRITE|MAY_EXEC). Review 5.1 (low):
/// check_parent only checked MAY_WRITE.
fn check_parent_exec_permission(parent_inode: &Inode) -> Result<(), i32> {
    let inode_mode = parent_inode.mode.bits() as u16;
    let inode_uid = parent_inode.uid.load(core::sync::atomic::Ordering::Relaxed);
    let inode_gid = parent_inode.gid.load(core::sync::atomic::Ordering::Relaxed);
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new_init()
    };
    if !crate::fs::permission::generic_permission(
        inode_mode, inode_uid, inode_gid,
        crate::fs::permission::MAY_EXEC, &cred,
    ) {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }
    Ok(())
}

/// Sticky bit (S_ISVTX) check for deleting/renaming an entry in a sticky
/// directory (review 5.1 high: without this any user with write permission
/// can unlink other users' files in /tmp).
///
/// Mirrors Linux may_delete() → check_sticky(): when the parent directory
/// has the sticky bit, the caller may only remove/rename an entry it owns
/// (entry uid == euid), or the entry's owner is the caller, or the caller
/// holds CAP_FOWNER.
fn check_sticky(parent_inode: &Inode, target_inode: Option<&Inode>) -> Result<(), i32> {
    const S_ISVTX: u32 = 0o1000;
    if parent_inode.mode.bits() & S_ISVTX == 0 {
        return Ok(());
    }

    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new_init()
    };

    if crate::security::has_capability(&cred, crate::security::CAP_FOWNER) {
        return Ok(());
    }

    // Owner of the parent directory may delete anything in it.
    let parent_uid = parent_inode.uid.load(core::sync::atomic::Ordering::Relaxed);
    if cred.euid == parent_uid {
        return Ok(());
    }

    // Otherwise the caller must own the entry itself.
    if let Some(target) = target_inode {
        let target_uid = target.uid.load(core::sync::atomic::Ordering::Relaxed);
        if cred.euid == target_uid {
            return Ok(());
        }
    }

    Err(errno::Errno::PermissionDenied.as_neg_i32())
}

// ============================================================================
// inotify notification helpers (P0-4)
//
// Every VFS change point below calls inotify::notify at its tail so
// registered watches see {IN_CREATE, IN_DELETE, IN_MOVED_FROM/TO,
// IN_MODIFY, IN_ATTRIB, IN_DELETE_SELF, IN_MOVE_SELF} events. Directory
// watches receive the events tagged with the entry name; direct inode
// watches receive them name-less. IN_OPEN/IN_ACCESS/IN_CLOSE_* come from
// the open/read/write/close paths (file.rs / file_open / file_opendir).
// ============================================================================

use crate::fs::inotify::inotify_events as ino;

/// Parent inode of a dentry (for directory-watch name events).
fn dentry_parent_inode(d: &Dentry) -> Option<Arc<Inode>> {
    d.parent.lock().clone().and_then(|p| p.get_inode())
}

/// Notify a change on `inode`, also reaching watches on the parent
/// directory of `dentry` with the entry `name`.
fn notify_about(
    dentry: Option<&Arc<Dentry>>,
    inode: Option<&Inode>,
    mask: u32,
    name: Option<&[u8]>,
    cookie: u32,
) {
    let parent = dentry.and_then(|d| dentry_parent_inode(d));
    crate::fs::inotify::notify(parent.as_deref(), inode, mask, name, cookie);
}

/// Link count of an inode (for IN_DELETE_SELF suppression on hard-linked
/// files: deleting one link does not delete the inode).
fn inode_nlink(inode: &Inode) -> u32 {
    let mut st = crate::fs::Stat::default();
    if inode.op_getattr(&mut st) == 0 {
        st.st_nlink
    } else {
        1
    }
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
    // Serialize the lookup+mutate window (see VFS_MUTATION_LOCK note).
    let _mutation_guard = VFS_MUTATION_LOCK.lock();

    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    // Get parent inode
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;
    check_parent_exec_permission(parent_inode)?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call mkdir through inode_operations
    // SAFETY: ops.mkdir is a VFS callback; parent_inode Arc is valid in scope
    unsafe {
        if let Some(mkdir_fn) = ops.mkdir {
            let inode_mode = InodeMode::new(InodeMode::S_IFDIR | mode);
            let new_inode = mkdir_fn(parent_inode.as_ref(), name.as_bytes(), inode_mode)?;

            // Invalidate negative dentry and cache the new one
            if let Some(ref parent_dentry) = parent_vpath.dentry {
                parent_dentry.remove_child(&name);
                let d = Arc::new(Dentry::new(name.clone()));
                d.set_inode(new_inode.clone());
                d.set_parent(parent_dentry.clone());
                parent_dentry.add_child(name.clone(), d);
            }

            // inotify: IN_CREATE|IN_ISDIR on the parent (named).
            crate::fs::inotify::notify(
                Some(parent_inode),
                Some(&new_inode),
                ino::IN_CREATE | ino::IN_ISDIR,
                Some(name.as_bytes()),
                0,
            );

            Ok(())
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Create symbolic link - unified implementation using inode_operations
pub fn vfs_symlink(pathname: &str, target: &str) -> Result<(), i32> {
    // Serialize the lookup+mutate window (see VFS_MUTATION_LOCK note).
    let _mutation_guard = VFS_MUTATION_LOCK.lock();

    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;
    check_parent_exec_permission(parent_inode)?;

    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // SAFETY: ops.symlink is a VFS callback; parent_inode Arc is valid in scope
    unsafe {
        if let Some(symlink_fn) = ops.symlink {
            let new_inode = symlink_fn(parent_inode.as_ref(), name.as_bytes(), target.as_bytes())?;

            // Invalidate negative dentry and cache the new one
            if let Some(ref parent_dentry) = parent_vpath.dentry {
                parent_dentry.remove_child(&name);
                let d = Arc::new(Dentry::new(name.clone()));
                d.set_inode(new_inode);
                d.set_parent(parent_dentry.clone());
                parent_dentry.add_child(name.clone(), d);
            }

            // inotify: IN_CREATE on the parent (named).
            crate::fs::inotify::notify(
                Some(parent_inode),
                None,
                ino::IN_CREATE,
                Some(name.as_bytes()),
                0,
            );

            Ok(())
        } else {
            Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
        }
    }
}

/// Remove directory - unified implementation using inode_operations
pub fn vfs_rmdir(pathname: &str) -> Result<(), i32> {
    // Serialize the lookup+mutate window (see VFS_MUTATION_LOCK note).
    let _mutation_guard = VFS_MUTATION_LOCK.lock();

    // Look up the target inode to get its ino for cache invalidation
    // (and its owner for the sticky-bit check).
    let target_vpath = path_lookup(pathname, 0).ok();
    let target_ino_and_fs_id = target_vpath.as_ref().and_then(|vp| {
        vp.inode.as_ref().map(|i| (i.ino, i.fs_id))
    });

    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    // Get parent inode
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;
    check_parent_exec_permission(parent_inode)?;

    // Sticky bit: rmdir of another user's entry in a +t directory.
    check_sticky(
        parent_inode,
        target_vpath.as_ref().and_then(|vp| vp.inode.as_ref().map(|a| a.as_ref())),
    )?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call rmdir through inode_operations
    // SAFETY: ops.rmdir is a VFS callback; parent_inode Arc is valid in scope
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
                // inotify: IN_DELETE|IN_ISDIR on the parent (named) and
                // IN_DELETE_SELF on the directory itself (auto-removes its
                // watches, delivering IN_IGNORED).
                let target_inode = target_vpath.as_ref().and_then(|vp| vp.inode.clone());
                crate::fs::inotify::notify(
                    Some(parent_inode),
                    None,
                    ino::IN_DELETE | ino::IN_ISDIR,
                    Some(name.as_bytes()),
                    0,
                );
                if let Some(ref ti) = target_inode {
                    crate::fs::inotify::notify(
                        None,
                        Some(ti),
                        ino::IN_DELETE_SELF | ino::IN_ISDIR,
                        None,
                        0,
                    );
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
    // Serialize the lookup+mutate window (see VFS_MUTATION_LOCK note).
    let _mutation_guard = VFS_MUTATION_LOCK.lock();

    // Look up the target inode to get its ino for cache invalidation
    // (and its owner for the sticky-bit check).
    let target_vpath = path_lookup(pathname, 0).ok();
    let target_ino_and_fs_id = target_vpath.as_ref().and_then(|vp| {
        vp.inode.as_ref().map(|i| (i.ino, i.fs_id))
    });

    let (parent_vpath, name) = lookup_parent_dir(pathname)?;

    // Get parent inode
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    check_parent_write_permission(parent_inode)?;
    check_parent_exec_permission(parent_inode)?;

    // Sticky bit: unlink of another user's entry in a +t directory
    // (e.g. /tmp) must fail with EACCES (review 5.1 high).
    check_sticky(
        parent_inode,
        target_vpath.as_ref().and_then(|vp| vp.inode.as_ref().map(|a| a.as_ref())),
    )?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call unlink through inode_operations
    // SAFETY: ops.unlink is a VFS callback; parent_inode Arc is valid in scope
    unsafe {
        if let Some(unlink_fn) = ops.unlink {
            let result = unlink_fn(parent_inode.as_ref(), name.as_bytes());
            if result == 0 {
                // inotify: capture the target inode before the caches drop
                // it — IN_DELETE on the parent (named); IN_DELETE_SELF on
                // the inode itself only when the last link went away.
                let target_inode = target_vpath.as_ref().and_then(|vp| vp.inode.clone());
                let last_link = target_inode
                    .as_ref()
                    .map(|i| inode_nlink(i) <= 1)
                    .unwrap_or(true);

                // Invalidate icache entry for the removed inode
                if let Some((ino, fs_id)) = target_ino_and_fs_id {
                    crate::fs::inode::icache_remove(ino, fs_id);
                }
                // P1 FIFO: drop the named-pipe peer registered for this
                // inode — the path is going away, and a future mknod of
                // the same name allocates a NEW inode (fresh pipe). Fds
                // that still hold the old pipe keep it alive via their
                // private_data Arc (POSIX unlinked-FIFO semantics).
                if let Some(ref vp) = target_vpath {
                    if let Some(ref i) = vp.inode {
                        if i.mode.is_fifo() {
                            crate::fs::fifo::forget(i.fs_id, i.ino);
                        }
                    }
                }
                // Replace dentry with negative entry
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    if let Some(child) = parent_dentry.lookup_child(&name) {
                        child.set_negative();
                        *child.inode.lock() = None;
                    }
                }

                // IN_DELETE goes to the parent watch (named); a direct
                // watch on the file gets IN_DELETE_SELF below.
                crate::fs::inotify::notify(
                    Some(parent_inode),
                    None,
                    ino::IN_DELETE,
                    Some(name.as_bytes()),
                    0,
                );
                if last_link {
                    if let Some(ref ti) = target_inode {
                        crate::fs::inotify::notify(
                            None,
                            Some(ti),
                            ino::IN_DELETE_SELF,
                            None,
                            0,
                        );
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
    // Serialize the lookup+mutate window (see VFS_MUTATION_LOCK note).
    let _mutation_guard = VFS_MUTATION_LOCK.lock();

    // Lookup the source file
    let src_vpath = path_lookup(oldpath, 0)?;
    let src_inode = src_vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Lookup parent directory of new path
    let (parent_vpath, name) = lookup_parent_dir(newpath)?;
    let parent_inode = parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    // Cross-device link is not allowed: source and destination must live on
    // the same filesystem (compare inode ops tables). Without this, the
    // parent's link callback would type-confuse a foreign inode.
    if !core::ptr::eq(
        src_inode.ops.as_ref().map(|o| *o as *const _).unwrap_or(core::ptr::null()),
        parent_inode.ops.as_ref().map(|o| *o as *const _).unwrap_or(core::ptr::null()),
    ) {
        return Err(errno::Errno::CrossDeviceLink.as_neg_i32());
    }

    check_parent_write_permission(parent_inode)?;
    check_parent_exec_permission(parent_inode)?;

    // Get inode operations
    let ops = parent_inode.ops.as_ref()
        .ok_or(errno::Errno::ReadOnlyFileSystem.as_neg_i32())?;

    // Call link through inode_operations
    // SAFETY: ops.link is a VFS callback; parent_inode and src_inode Arcs are valid in scope
    unsafe {
        if let Some(link_fn) = ops.link {
            let result = link_fn(parent_inode.as_ref(), name.as_bytes(), src_inode.as_ref());
            if result == 0 {
                // Invalidate stale/negative dentry at new path
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    parent_dentry.remove_child(&name);
                }
                // inotify: a hard link appearing is IN_CREATE in the
                // destination directory (named); the linked inode itself
                // sees IN_ATTRIB (its link count changed).
                crate::fs::inotify::notify(
                    Some(parent_inode),
                    None,
                    ino::IN_CREATE,
                    Some(name.as_bytes()),
                    0,
                );
                crate::fs::inotify::notify(None, Some(src_inode), ino::IN_ATTRIB, None, 0);
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
    // Serialize the lookup+mutate window (see VFS_MUTATION_LOCK note):
    // both the source and destination sequences must be atomic against a
    // concurrent creator/unlinker.
    let _mutation_guard = VFS_MUTATION_LOCK.lock();
    vfs_rename_locked(oldpath, newpath)
}

/// Lock-free rename core — caller must hold VFS_MUTATION_LOCK (either
/// directly via vfs_rename, or across a multi-step sequence such as
/// vfs_rename_exchange that must not interleave with other mutations).
fn vfs_rename_locked(oldpath: &str, newpath: &str) -> Result<(), i32> {
    // Lookup parent directories of both paths
    let (old_parent_vpath, old_name) = lookup_parent_dir(oldpath)?;
    let (new_parent_vpath, new_name) = lookup_parent_dir(newpath)?;

    let old_parent = old_parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    let new_parent = new_parent_vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Rename across filesystems is not allowed: both parents must share the
    // same inode ops table, otherwise the rename callback would type-confuse
    // a foreign parent inode (e.g. rootfs_rename on a procfs inode).
    if !core::ptr::eq(
        old_parent.ops.as_ref().map(|o| *o as *const _).unwrap_or(core::ptr::null()),
        new_parent.ops.as_ref().map(|o| *o as *const _).unwrap_or(core::ptr::null()),
    ) {
        return Err(errno::Errno::CrossDeviceLink.as_neg_i32());
    }

    check_parent_write_permission(old_parent)?;
    check_parent_exec_permission(old_parent)?;
    check_parent_write_permission(new_parent)?;
    check_parent_exec_permission(new_parent)?;

    // Sticky-bit checks on BOTH parents: renaming out of a sticky dir
    // requires owning the source entry; renaming into a sticky dir
    // requires owning the replaced entry (if any) — mirrors Linux
    // may_delete() on both ends (review 5.1 high).
    let source_vpath = path_lookup(oldpath, 0).ok();
    check_sticky(old_parent, source_vpath.as_ref().and_then(|vp| vp.inode.as_ref().map(|a| a.as_ref())))?;
    let replaced_vpath = path_lookup(newpath, 0).ok();
    if replaced_vpath.is_some() {
        check_sticky(new_parent, replaced_vpath.as_ref().and_then(|vp| vp.inode.as_ref().map(|a| a.as_ref())))?;
    }

    // Rename a directory into itself or its own subdirectory would create a
    // ".." cycle — check the ancestor chain of the new parent (review 5.5:
    // rename 环检查错). Only directories can form cycles.
    if let Some(ref src_inode) = source_vpath.as_ref().and_then(|vp| vp.inode.as_ref()) {
        if src_inode.mode.is_directory() {
            // Renaming a dir to the same place is a no-op.
            if let (Some(ref old_d), Some(ref new_d)) = (&old_parent_vpath.dentry, &new_parent_vpath.dentry) {
                if Arc::ptr_eq(old_d, new_d) && old_name == new_name {
                    return Ok(());
                }
            }
            let new_parent_ino = new_parent.ino;
            if new_parent_ino == src_inode.ino
                || is_ancestor_of(Arc::clone(new_parent), src_inode.ino)
            {
                return Err(errno::Errno::InvalidArgument.as_neg_i32());
            }
        }
    }

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

        // inotify: IN_MOVED_FROM on the old parent and IN_MOVED_TO on the
        // new parent share a nonzero cookie so watchers can correlate the
        // halves; the inode itself gets IN_MOVE_SELF.
        let src_inode = source_vpath.as_ref().and_then(|vp| vp.inode.clone());
        let isdir = src_inode
            .as_ref()
            .map(|i| i.mode.is_directory())
            .unwrap_or(false);
        let dir_bit = if isdir { ino::IN_ISDIR } else { 0 };
        let cookie = crate::fs::inotify::alloc_cookie();
        // MOVED_FROM/MOVED_TO go to parent-directory watches (named); a
        // direct watch on the file sees only IN_MOVE_SELF below.
        crate::fs::inotify::notify(
            Some(old_parent),
            None,
            ino::IN_MOVED_FROM | dir_bit,
            Some(old_name.as_bytes()),
            cookie,
        );
        crate::fs::inotify::notify(
            Some(new_parent),
            None,
            ino::IN_MOVED_TO | dir_bit,
            Some(new_name.as_bytes()),
            cookie,
        );
        if let Some(ref si) = src_inode {
            crate::fs::inotify::notify(
                None,
                Some(si),
                ino::IN_MOVE_SELF | dir_bit,
                None,
                0,
            );
        }
        Ok(())
    } else {
        Err(result)
    }
}

/// Walk the ".." chain of `dir` and report whether `candidate_ino` is an
/// ancestor of `dir` (bounded to 64 hops to survive corrupt trees).
fn is_ancestor_of(start: Arc<Inode>, candidate_ino: u64) -> bool {
    let mut current = start;
    let mut guard_count = 0;
    while guard_count < 64 {
        guard_count += 1;
        // Read ".." through the filesystem's lookup op.
        match current.op_lookup(b"..") {
            Ok(parent_ino) => {
                if parent_ino == candidate_ino {
                    return true;
                }
                if parent_ino == current.ino {
                    return false; // reached filesystem root
                }
                // Materialize the parent inode through iget.
                let ops = match current.ops {
                    Some(o) => o,
                    None => return false,
                };
                let iget = match ops.iget {
                    Some(f) => f,
                    None => return false,
                };
                // SAFETY: VFS callback contract; `current` is a valid Inode.
                let parent_inode = match unsafe { iget(&current, b"..", parent_ino) } {
                    Ok(i) => i,
                    Err(_) => return false,
                };
                current = parent_inode;
            }
            Err(_) => return false,
        }
    }
    false
}

/// renameat2(RENAME_EXCHANGE): atomically swap the two paths.
///
/// Both paths must exist (ENOENT otherwise). The swap runs under one
/// VFS_MUTATION_LOCK hold as a three-rename ballet through a unique
/// hidden temp name in the NEW path's parent directory:
///   A → tmp, B → A, tmp → B
/// The whole sequence is serialized against every other VFS mutation
/// (create/unlink/rename take the same lock), so no concurrent mutation
/// can interleave. Concurrent READS can observe the intermediate
/// A-missing window — a full directory-entry-level swap would need
/// per-filesystem support; documented approximation, final state and
/// crash-window-mates are correct.
pub fn vfs_rename_exchange(oldpath: &str, newpath: &str) -> Result<(), i32> {
    use alloc::format;
    use alloc::string::ToString;

    if oldpath == newpath {
        // Exchanging a path with itself is a no-op (Linux returns 0).
        // But it must still exist.
        let mut st = crate::fs::Stat::new();
        return stat_file_by_path(oldpath, &mut st);
    }

    let _mutation_guard = VFS_MUTATION_LOCK.lock();

    // Both endpoints must exist.
    let old_vpath = path_lookup(oldpath, 0)
        .map_err(|_| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    let new_vpath = path_lookup(newpath, 0)
        .map_err(|_| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // A directory swap across an ancestor relationship would create a
    // ".." cycle — reject like plain rename does.
    if let (Some(ref oi), Some(ref ni)) = (&old_vpath.inode, &new_vpath.inode) {
        if oi.mode.is_directory() || ni.mode.is_directory() {
            if is_ancestor_of(Arc::clone(oi), ni.ino) || is_ancestor_of(Arc::clone(ni), oi.ino) {
                return Err(errno::Errno::InvalidArgument.as_neg_i32());
            }
        }
    }

    // Unique hidden temp name in the new path's parent directory (same
    // filesystem as B by construction, so the renames cannot hit EXDEV).
    static EXCHANGE_COUNTER: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let n = EXCHANGE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let tmp_name = format!(
        ".rux-rex-{:x}-{:x}",
        crate::drivers::timer::get_jiffies(),
        n
    );
    let new_parent_str = match newpath.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => newpath[..idx].to_string(),
        None => ".".to_string(),
    };
    let tmp_path = format!("{}/{}", new_parent_str.trim_end_matches('/'), tmp_name);

    // A → tmp (B's directory), B → A, tmp → B.
    vfs_rename_locked(oldpath, &tmp_path)?;
    if let Err(e) = vfs_rename_locked(newpath, oldpath) {
        // Roll back: tmp → A restores the original state.
        let _ = vfs_rename_locked(&tmp_path, oldpath);
        return Err(e);
    }
    if let Err(e) = vfs_rename_locked(&tmp_path, newpath) {
        // B already sits at A's name; restore by moving it back and
        // unwinding the first rename.
        let _ = vfs_rename_locked(oldpath, newpath);
        let _ = vfs_rename_locked(&tmp_path, oldpath);
        return Err(e);
    }
    Ok(())
}
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
        crate::process::task::Cred::new_init()
    };
    let inode_uid = inode.uid.load(Ordering::Relaxed);
    if cred.euid != 0 && cred.euid != inode_uid {
        return Err(errno::Errno::OperationNotPermitted.as_neg_i32());
    }

    let result = inode.op_setattr(setattr_attr::ATTR_MODE, mode as u64, 0);
    if result == 0 {
        // inotify: mode change → IN_ATTRIB.
        let name = vpath.dentry.as_ref().map(|d| d.get_name().into_bytes());
        notify_about(vpath.dentry.as_ref(), Some(inode), ino::IN_ATTRIB, name.as_deref(), 0);
        Ok(())
    } else {
        Err(result)
    }
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
        crate::process::task::Cred::new_init()
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

    // POSIX: clear setuid/setgid bits on owner change (per Linux notify_change)
    let mode = inode.mode.bits();
    if uid != u32::MAX || gid != u32::MAX {
        let new_mode = mode & !(0o4000u32 | 0o2000u32); // clear S_ISUID | S_ISGID
        let _ = inode.op_setattr(setattr_attr::ATTR_MODE, new_mode as u64, 0);
    }

    let result = inode.op_setattr(setattr_attr::ATTR_UID_GID, actual_uid as u64, actual_gid as u64);
    if result == 0 {
        // inotify: owner change → IN_ATTRIB.
        let name = vpath.dentry.as_ref().map(|d| d.get_name().into_bytes());
        notify_about(vpath.dentry.as_ref(), Some(inode), ino::IN_ATTRIB, name.as_deref(), 0);
        Ok(())
    } else {
        Err(result)
    }
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
        crate::process::task::Cred::new_init()
    };
    if !crate::fs::permission::generic_permission(
        inode_mode, inode_uid, inode_gid,
        crate::fs::permission::MAY_WRITE, &cred,
    ) {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }

    let result = inode.op_setattr(setattr_attr::ATTR_SIZE, new_size as u64, 0);
    if result == 0 {
        // inotify: truncate is a size change → IN_MODIFY (and metadata
        // change → IN_ATTRIB) on the file and on parent watches (named).
        let name = vpath.dentry.as_ref().map(|d| d.get_name().into_bytes());
        notify_about(
            vpath.dentry.as_ref(),
            Some(inode),
            ino::IN_MODIFY | ino::IN_ATTRIB,
            name.as_deref(),
            0,
        );
        Ok(())
    } else {
        Err(result)
    }
}

/// Truncate open file by fd (ftruncate)
pub fn vfs_ftruncate(fd: usize, new_size: i64) -> Result<(), i32> {
    if new_size < 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // SAFETY: get_file_fd returns a valid Arc<File> for the given fd
    let file = unsafe { get_file_fd(fd) }
        .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;

    // Linux: check that fd was opened for writing (FMODE_WRITE)
    let file_flags = file.flags();
    if file_flags.is_readonly() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Get inode from file
    // SAFETY: file.inode is an UnsafeCell<Option<Arc<Inode>>>; accessing under current task's fd table lock
    let inode_opt = unsafe { &*file.inode.get() };
    let inode = inode_opt.as_ref()
        .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;

    if inode.mode.is_directory() {
        return Err(errno::Errno::IsADirectory.as_neg_i32());
    }

    // Linux do_ftruncate: only regular files (and dirs, which we already
    // rejected with EISDIR above) are truncatable. A FIFO, socket, or
    // device node must fail with EINVAL instead of running a generic
    // setattr that would fake success.
    if !inode.mode.is_regular_file() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    let result = inode.op_setattr(setattr_attr::ATTR_SIZE, new_size as u64, 0);
    if result == 0 {
        // inotify: ftruncate → IN_MODIFY | IN_ATTRIB (through the File so
        // parent watches get the named event).
        crate::fs::inotify::notify_file(&file, ino::IN_MODIFY | ino::IN_ATTRIB);
        Ok(())
    } else {
        Err(result)
    }
}

/// Get file/directory status using inode_operations
pub fn vfs_stat(pathname: &str, stat: &mut Stat) -> Result<(), i32> {
    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Get inode operations
    if let Some(ops) = inode.ops.as_ref() {
        // SAFETY: ops.getattr is a VFS callback; inode Arc is valid in scope
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
    // SAFETY: file descriptor operations use well-defined VFS callbacks and Arc-based refcounting
    unsafe {
        let o_creat = (flags & FileFlags::O_CREAT) != 0;
        let o_excl = (flags & FileFlags::O_EXCL) != 0;
        let o_trunc = (flags & FileFlags::O_TRUNC) != 0;

        // O_CREAT atomicity (review 5.1 high): the existence check and the
        // create below must be one atomic step on SMP, or two CPUs racing on
        // the same non-existent name both allocate an inode / directory
        // entry. Bracket the whole sequence with the coarse VFS mutation
        // lock (see VFS_MUTATION_LOCK note for the design choice).
        let _mutation_guard = if o_creat {
            Some(VFS_MUTATION_LOCK.lock())
        } else {
            None
        };

        // Step 1: Resolve path through dentry tree
        // O_NOFOLLOW (POSIX): refuse to follow a symlink FINAL component —
        // privileged callers rely on this against symlink tricks, and the
        // flag was previously accepted and silently ignored (the link was
        // followed anyway).
        let lookup_flags = if flags & FileFlags::O_NOFOLLOW != 0 {
            LOOKUP_NOFOLLOW
        } else {
            0
        };
        let (inode, opened_dentry) = match path_lookup(filename, lookup_flags) {
            Ok(vpath) => {
                if o_excl && o_creat {
                    return Err(errno::Errno::FileExists.as_neg_i32());
                }
                let inode = vpath.inode.ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
                if flags & FileFlags::O_NOFOLLOW != 0 && inode.mode.is_symlink() {
                    return Err(errno::Errno::TooManySymbolicLinks.as_neg_i32()); // ELOOP
                }
                (inode, vpath.dentry)
            }
            Err(_e) if o_creat => {
                let (parent_path, child_name) = path_parent_and_name(filename)?;
                let parent_vpath = path_lookup(&parent_path, 0)?;
                let parent_inode = parent_vpath.inode
                    .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
                // DAC: creating an entry requires write+search on the parent
                if !crate::fs::permission::inode_permission(
                    &parent_inode,
                    crate::fs::permission::MAY_WRITE | crate::fs::permission::MAY_EXEC,
                ) {
                    return Err(errno::Errno::PermissionDenied.as_neg_i32());
                }
                let ops = parent_inode.ops.as_ref()
                    .ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
                let new_inode = {
                    let create_fn = ops.create
                        .ok_or(errno::Errno::PermissionDenied.as_neg_i32())?;
                    // Apply umask to mode (POSIX requirement)
                    let effective_mode = if let Some(task) = crate::sched::current() {
                        unsafe { (*task).get_umask() }
                    } else {
                        0o022
                    };
                    let filtered_mode = mode & !effective_mode;
                    create_fn(&*parent_inode, child_name.as_bytes(), crate::fs::inode::InodeMode::new(filtered_mode))?
                };
                // Cache new dentry (replace stale/negative dentry)
                let mut new_d = None;
                if let Some(ref parent_dentry) = parent_vpath.dentry {
                    let name = String::from(child_name.as_str());
                    parent_dentry.remove_child(&name);
                    let d = Arc::new(Dentry::new(name.clone()));
                    d.set_inode(Arc::clone(&new_inode));
                    d.set_parent(parent_dentry.clone());
                    parent_dentry.add_child(name, d.clone());
                    new_d = Some(d);
                }
                // inotify: file creation via open(O_CREAT) → IN_CREATE on
                // the parent directory (named).
                crate::fs::inotify::notify(
                    Some(&parent_inode),
                    None,
                    ino::IN_CREATE,
                    Some(child_name.as_bytes()),
                    0,
                );
                (new_inode, new_d)
            }
            Err(e) => return Err(e),
        };

        // Directory -> redirect to opendir
        if inode.mode.is_directory() {
            return file_opendir(filename, flags | 0o00200000);
        }

        // Device nodes with no bound driver: Linux fails the open with
        // ENXIO (no such device or address) — not a silent ops-less File
        // whose read/write would return EBADF.
        if inode.mode.is_char_device() || inode.mode.is_block_device() {
            let has_ops = match inode.ops.and_then(|o| o.get_file_ops) {
                Some(f) => unsafe { f(&*inode) }.is_some(),
                None => false,
            };
            if !has_ops {
                return Err(errno::Errno::NoSuchDeviceOrAddress.as_neg_i32());
            }
        }

        // DAC: check access mode against the target inode (Linux may_open).
        // O_RDONLY/O_RDWR → MAY_READ; O_WRONLY/O_RDWR/O_TRUNC → MAY_WRITE.
        {
            use crate::fs::permission::{inode_permission, MAY_READ, MAY_WRITE};
            let accmode = flags & FileFlags::O_ACCMODE;
            let mut mask = 0;
            if accmode == FileFlags::O_RDONLY || accmode == FileFlags::O_RDWR {
                mask |= MAY_READ;
            }
            if accmode == FileFlags::O_WRONLY || accmode == FileFlags::O_RDWR || o_trunc {
                mask |= MAY_WRITE;
            }
            if mask != 0 && !inode_permission(&inode, mask) {
                return Err(errno::Errno::PermissionDenied.as_neg_i32());
            }
        }

        // FIFO (P1 mknod/mkfifo): named-pipe open semantics — a blocking
        // O_RDONLY waits for a writer, a blocking O_WRONLY waits for a
        // reader, O_NONBLOCK applies per POSIX (ENXIO for a writer with no
        // reader). Dispatch AFTER the DAC check, BEFORE the regular-file
        // path, so the pipe data path (not the inode data) serves I/O.
        if inode.mode.is_fifo() {
            let peer = crate::fs::fifo::peer_for(inode.fs_id, inode.ino);
            let file = crate::fs::fifo::fifo_open_file(&peer, flags)?;
            file.set_inode(Arc::clone(&inode));
            if let Some(d) = opened_dentry {
                file.set_dentry(d);
            }
            match get_file_fd_install(Arc::clone(&file)) {
                Some(fd) => {
                    crate::fs::inotify::notify_file(&file, ino::IN_OPEN);
                    return Ok(fd);
                }
                None => return Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
            }
        }

        // Create File object
        let file_flags = FileFlags::new(flags);
        let file = Arc::new(File::new(file_flags));
        file.set_inode(Arc::clone(&inode));
        // Track the dentry so /proc/self/fd and dirfd-relative lookups can
        // recover the path (fixes File.dentry never being set).
        if let Some(d) = opened_dentry {
            file.set_dentry(d);
        }

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
            // inotify: O_TRUNC truncation → IN_MODIFY | IN_ATTRIB.
            crate::fs::inotify::notify_file(&file, ino::IN_MODIFY | ino::IN_ATTRIB);
        }

        match get_file_fd_install(Arc::clone(&file)) {
            Some(fd) => {
                // inotify: successful open → IN_OPEN (parent watches get
                // the named event through the File's dentry).
                crate::fs::inotify::notify_file(&file, ino::IN_OPEN);
                Ok(fd)
            }
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
    // SAFETY: close_file_fd safely handles fd validity check and cleanup
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
    // SAFETY: get_file_fd returns valid Arc<File>; File::read handles bounds
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
    // SAFETY: get_file_fd returns valid Arc<File>; File::write handles bounds
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
    // SAFETY: get_file_fd returns valid Arc<File>; inode UnsafeCell accessed under fd table lock
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

/// Get file status by path with lookup flags (for fstatat with AT_SYMLINK_NOFOLLOW)
pub fn stat_file_by_path_with_flags(path: &str, stat: &mut Stat, flags: u32) -> Result<(), i32> {
    let vpath = path_lookup(path, flags)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    if let Some(ops) = inode.ops.as_ref() {
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
    Err(errno::Errno::InvalidArgument.as_neg_i32())
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

    /// Get record lock info (struct flock *)
    pub const F_GETLK: usize = 5;

    /// Set record lock (non-blocking)
    pub const F_SETLK: usize = 6;

    /// Set record lock (blocking)
    pub const F_SETLKW: usize = 7;

    /// 64-bit aliases — on RV64 off_t is already 64-bit and the struct
    /// layout is identical (glibc may use either number).
    pub const F_GETLK64: usize = 12;
    pub const F_SETLK64: usize = 13;
    pub const F_SETLKW64: usize = 14;

    /// Duplicate file descriptor with close-on-exec
    pub const F_DUPFD_CLOEXEC: usize = 1030;

    /// FD_CLOEXEC flag value
    pub const FD_CLOEXEC: usize = 1;
}

/// flock lock types (struct flock::l_type).
mod flock_types {
    pub const F_RDLCK: i16 = 0;
    pub const F_WRLCK: i16 = 1;
    pub const F_UNLCK: i16 = 2;
}

/// Resolve a `struct flock` region (l_whence/l_start/l_len) into an
/// absolute byte range `[start, end)`; `end == u64::MAX` means "to EOF"
/// (and tracks EOF as it grows, per POSIX). Returns negative errno on
/// malformed input.
fn resolve_lock_region(
    file: &File,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
) -> Result<(u64, u64), i32> {
    let base: i64 = match l_whence {
        0 => 0, // SEEK_SET
        1 => file.get_pos() as i64, // SEEK_CUR
        2 => {
            // SAFETY: inode is written once at open time; read-only access.
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt.as_ref() {
                Some(inode) => inode.get_size() as i64, // SEEK_END
                None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
            }
        }
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    };

    let start = match base.checked_add(l_start) {
        Some(s) if s >= 0 => s as u64,
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    };

    let end = if l_len == 0 {
        // l_len == 0: lock from start to EOF (and beyond, if it grows).
        u64::MAX
    } else if l_len > 0 {
        match start.checked_add(l_len as u64) {
            Some(e) => e,
            None => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
        }
    } else {
        // Negative l_len: region ends at l_start and extends backwards.
        match (start as i64).checked_add(l_len) {
            Some(s) if s >= 0 => {
                let end = start;
                let start = s as u64;
                if start >= end {
                    return Err(errno::Errno::InvalidArgument.as_neg_i32());
                }
                return Ok((start, end));
            }
            _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
        }
    };

    if end <= start {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    Ok((start, end))
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

    // SAFETY: fcntl operations use valid fd from get_file_fd; no raw pointer dereference
    unsafe {
        match cmd {
            // F_DUPFD: Duplicate file descriptor
            fcntl::F_DUPFD => {
                // Get original file
                let old_file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Allocate new file descriptor >= arg
                let fdtable = crate::sched::get_current_fdtable()
                    .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;
                let new_fd = fdtable.alloc_fd_from(arg)
                    .ok_or(errno::Errno::TooManyOpenFiles.as_neg_i32())?;
                fdtable.install_fd(new_fd, old_file)
                    .map_err(|_| errno::Errno::TooManyOpenFiles.as_neg_i32())?;

                Ok(new_fd)
            }

            // F_DUPFD_CLOEXEC: Duplicate file descriptor with close-on-exec
            fcntl::F_DUPFD_CLOEXEC => {
                // Get original file
                let old_file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Allocate new file descriptor >= arg
                let fdtable = crate::sched::get_current_fdtable()
                    .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;
                let new_fd = fdtable.alloc_fd_from(arg)
                    .ok_or(errno::Errno::TooManyOpenFiles.as_neg_i32())?;
                fdtable.install_fd(new_fd, old_file)
                    .map_err(|_| errno::Errno::TooManyOpenFiles.as_neg_i32())?;

                // Set close-on-exec flag on the new fd (per-descriptor)
                fdtable.set_fd_cloexec(new_fd, true);

                Ok(new_fd)
            }

            // F_GETFD: Get close-on-exec flag
            fcntl::F_GETFD => {
                if get_file_fd(fd).is_none() {
                    return Err(errno::Errno::BadFileNumber.as_neg_i32());
                }
                let ft = crate::sched::get_current_fdtable()
                    .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;
                Ok(if ft.get_fd_cloexec(fd) { fcntl::FD_CLOEXEC } else { 0 })
            }

            // F_SETFD: Set close-on-exec flag
            fcntl::F_SETFD => {
                if get_file_fd(fd).is_none() {
                    return Err(errno::Errno::BadFileNumber.as_neg_i32());
                }
                let ft = crate::sched::get_current_fdtable()
                    .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;
                // Bit 0 of arg indicates FD_CLOEXEC (per-descriptor)
                ft.set_fd_cloexec(fd, (arg & fcntl::FD_CLOEXEC) != 0);
                Ok(0)  // Return 0 on success
            }

            // F_GETFL: Get file status flags
            fcntl::F_GETFL => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Return file status flags (access mode)
                Ok(file.flags().bits() as usize)
            }

            // F_SETFL: Set file status flags
            fcntl::F_SETFL => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // Linux semantics: F_SETFL can only modify O_NONBLOCK,
                // O_APPEND, O_ASYNC and O_DIRECT/O_SYNC. EVERY other flag
                // (access mode, O_DIRECTORY, O_NOFOLLOW, ...) keeps its
                // current value — the old code rebuilt the word from just
                // accmode|arg and cleared O_DIRECTORY et al (review 5.1).
                // P2 O_DIRECT: the flag round-trips through F_SETFL (the
                // I/O path does not differentiate buffered vs direct —
                // flush semantics approximate it; documented limitation).
                const SETFL_FLAGS: u32 = crate::fs::file::FileFlags::O_APPEND
                    | crate::fs::file::FileFlags::O_NONBLOCK
                    | crate::fs::file::FileFlags::O_SYNC
                    | crate::fs::file::FileFlags::O_DSYNC
                    | crate::fs::file::FileFlags::O_DIRECT;

                let current = file.flags().bits();
                let new_flags = (current & !SETFL_FLAGS) | (arg as u32 & SETFL_FLAGS);

                file.set_flags(crate::fs::file::FileFlags::new(new_flags));

                Ok(0)  // Return 0 on success
            }

            // ==================== POSIX record locks (P0-5) ====================
            //
            // struct flock (LP64): { i16 l_type; i16 l_whence; off_t
            // l_start; off_t l_len; pid_t l_pid; } = 32 bytes.
            fcntl::F_GETLK | fcntl::F_GETLK64 => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };
                if arg == 0 || !crate::arch::riscv64::uaccess::access_ok(arg, 32) {
                    return Err(errno::Errno::BadAddress.as_neg_i32());
                }
                let mut fl = [0u8; 32];
                if unsafe {
                    crate::arch::riscv64::uaccess::copy_from_user(
                        fl.as_mut_ptr(),
                        arg as *const u8,
                        32,
                    )
                } > 0
                {
                    return Err(errno::Errno::BadAddress.as_neg_i32());
                }
                let l_type = i16::from_le_bytes([fl[0], fl[1]]);
                let l_whence = i16::from_le_bytes([fl[2], fl[3]]);
                let l_start = i64::from_le_bytes(fl[8..16].try_into().unwrap());
                let l_len = i64::from_le_bytes(fl[16..24].try_into().unwrap());

                let exclusive = match l_type {
                    flock_types::F_RDLCK => false,
                    flock_types::F_WRLCK => true,
                    _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
                };
                // Access-mode check mirrors F_SETLK's (Linux may_setlk).
                if exclusive && file.flags().is_readonly()
                    || !exclusive && file.flags().is_writeonly()
                {
                    return Err(errno::Errno::BadFileNumber.as_neg_i32());
                }

                let (start, end) = resolve_lock_region(&file, l_whence, l_start, l_len)?;
                let owner_pid = crate::sched::current()
                    .map(|t| t.tgid())
                    .unwrap_or(0);

                let out = {
                    let mut out = fl;
                    match crate::fs::locks::posix_test_lock(
                        &file, owner_pid, exclusive, start, end,
                    ) {
                        // First conflicting lock: report it with ABSOLUTE
                        // start (l_whence = SEEK_SET) and l_len = 0 for a
                        // to-EOF tail (POSIX).
                        Some(c) => {
                            let ltype = if c.exclusive {
                                flock_types::F_WRLCK
                            } else {
                                flock_types::F_RDLCK
                            };
                            let len: i64 = if c.end == u64::MAX {
                                0
                            } else {
                                (c.end - c.start) as i64
                            };
                            out[0..2].copy_from_slice(&ltype.to_le_bytes());
                            out[2..4].copy_from_slice(&0i16.to_le_bytes()); // SEEK_SET
                            out[8..16].copy_from_slice(&(c.start as i64).to_le_bytes());
                            out[16..24].copy_from_slice(&len.to_le_bytes());
                            out[24..28].copy_from_slice(&(c.pid as i32).to_le_bytes());
                        }
                        // No conflict: l_type = F_UNLCK, other fields
                        // untouched (POSIX); l_pid = 0 to be explicit.
                        None => {
                            out[0..2].copy_from_slice(&flock_types::F_UNLCK.to_le_bytes());
                            out[24..28].copy_from_slice(&0i32.to_le_bytes());
                        }
                    }
                    out
                };
                if unsafe {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        arg as *mut u8,
                        out.as_ptr(),
                        32,
                    )
                } > 0
                {
                    return Err(errno::Errno::BadAddress.as_neg_i32());
                }
                Ok(0)
            }

            fcntl::F_SETLK | fcntl::F_SETLK64 | fcntl::F_SETLKW | fcntl::F_SETLKW64 => {
                let wait = cmd == fcntl::F_SETLKW || cmd == fcntl::F_SETLKW64;
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };
                if arg == 0 || !crate::arch::riscv64::uaccess::access_ok(arg, 32) {
                    return Err(errno::Errno::BadAddress.as_neg_i32());
                }
                let mut fl = [0u8; 32];
                if unsafe {
                    crate::arch::riscv64::uaccess::copy_from_user(
                        fl.as_mut_ptr(),
                        arg as *const u8,
                        32,
                    )
                } > 0
                {
                    return Err(errno::Errno::BadAddress.as_neg_i32());
                }
                let l_type = i16::from_le_bytes([fl[0], fl[1]]);
                let l_whence = i16::from_le_bytes([fl[2], fl[3]]);
                let l_start = i64::from_le_bytes(fl[8..16].try_into().unwrap());
                let l_len = i64::from_le_bytes(fl[16..24].try_into().unwrap());

                let kind = match l_type {
                    flock_types::F_RDLCK => {
                        // Read lock needs a readable fd.
                        if file.flags().is_writeonly() {
                            return Err(errno::Errno::BadFileNumber.as_neg_i32());
                        }
                        crate::fs::locks::F_RDLCK_KIND
                    }
                    flock_types::F_WRLCK => {
                        // Write lock needs a writeable fd.
                        if file.flags().is_readonly() {
                            return Err(errno::Errno::BadFileNumber.as_neg_i32());
                        }
                        crate::fs::locks::F_WRLCK_KIND
                    }
                    flock_types::F_UNLCK => crate::fs::locks::F_UNLCK_KIND,
                    _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
                };

                let (start, end) = resolve_lock_region(&file, l_whence, l_start, l_len)?;
                let owner_pid = crate::sched::current()
                    .map(|t| t.tgid())
                    .unwrap_or(0);

                crate::fs::locks::posix_set_lock(&file, owner_pid, kind, start, end, wait)
                    .map(|_| 0)
            }

            // Unsupported command
            _ => {
                Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
            }
        }
    }
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

    // DAC: listing a directory requires read permission on it
    if !crate::fs::permission::inode_permission(inode, crate::fs::permission::MAY_READ) {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }

    let file_flags = FileFlags::new(flags);
    let file = Arc::new(File::new(file_flags));
    file.set_inode(Arc::clone(&inode));
    // Track the dentry (path recovery for /proc/self/fd, dirfd lookups)
    if let Some(ref d) = vpath.dentry {
        file.set_dentry(d.clone());
    }

    // SAFETY: inode.ops callbacks are well-defined; inode Arc is valid in scope
    unsafe {
        // Get directory file ops from inode callback
        if let Some(ops) = inode.ops {
            if let Some(get_file_ops_fn) = ops.get_file_ops {
                if let Some(dir_ops) = get_file_ops_fn(&*inode) {
                    file.set_ops(dir_ops);
                }
            }
        }

        match get_file_fd_install(Arc::clone(&file)) {
            Some(fd) => {
                // inotify: directory open → IN_OPEN|IN_ISDIR (notify_file
                // adds the IN_ISDIR modifier for directories).
                crate::fs::inotify::notify_file(&file, ino::IN_OPEN);
                Ok(fd)
            }
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
    // SAFETY: get_file_fd returns valid Arc<File>; inode UnsafeCell accessed under fd table lock
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

        for (idx, entry) in entries.iter().enumerate().skip(start_pos) {
            let name = &entry.name;
            let name_len = name.len();
            let dirent_size = (19 + name_len + 1 + 7) & !7;

            if bytes_written + dirent_size > count {
                break;
            }

            let buf_offset = bytes_written;
            buf[buf_offset..buf_offset + 8].copy_from_slice(&entry.ino.to_le_bytes());
            // d_off: absolute offset from directory start (for seekdir/telldir)
            let d_off = (start_pos + current_idx + 1) as u64;
            buf[buf_offset + 8..buf_offset + 16].copy_from_slice(&d_off.to_le_bytes());
            buf[buf_offset + 16..buf_offset + 18].copy_from_slice(&(dirent_size as u16).to_le_bytes());
            buf[buf_offset + 18] = entry.file_type;
            buf[buf_offset + 19..buf_offset + 19 + name_len].copy_from_slice(name);
            buf[buf_offset + 19 + name_len] = 0;

            bytes_written += dirent_size;
            current_idx += 1;
        }

        // Linux behaviour: a buffer too small to hold even ONE entry is
        // EINVAL, not a silent Ok(0) — the old code made telldir-style
        // loops spin forever on a short buffer (review 5.1).
        if bytes_written == 0
            && current_idx == 0
            && start_pos < entries.len()
        {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
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
    // SAFETY: private_data contains a valid MemFileContent pointer set by open_mem_file
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
    // SAFETY: private_data contains a valid MemFileContent pointer set by open_mem_file
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
    // SAFETY: private_data contains a valid MemFileContent pointer set by open_mem_file
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
// fallocate / utimensat (fs-side helpers)
// ============================================================================

/// fallocate flags (UAPI)
pub const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
pub const FALLOC_FL_PUNCH_HOLE: i32 = 0x02;

/// Filesystem-side fallocate by fd. The syscall entry
/// (sys_fallocate in syscall/file.rs) delegates here.
///
/// NOTE (wave-2 boundary): sys_fallocate currently returns success without
/// calling this — wiring it is owned by the syscall-layer agent; the
/// behaviour lives here so the change is one call away.
#[allow(dead_code)]
pub fn vfs_fallocate(fd: usize, mode: i32, offset: u64, len: u64) -> Result<(), i32> {
    // SAFETY: get_file_fd returns a valid Arc<File> for the given fd
    let file = unsafe { get_file_fd(fd) }
        .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;

    if !file.flags().is_writeonly() && file.flags().is_readonly() {
        return Err(errno::Errno::BadFileNumber.as_neg_i32());
    }

    // SAFETY: inode is written once at open time; read-only access here.
    let inode_opt = unsafe { &*file.inode.get() };
    let inode = inode_opt.as_ref()
        .ok_or(errno::Errno::BadFileNumber.as_neg_i32())?;

    if !inode.mode.is_regular_file() {
        return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
    }

    // Route to the filesystem implementation (ext4).
    let fs_ptr = inode.private_data
        .ok_or(errno::Errno::FunctionNotImplemented.as_neg_i32())?;
    // Only ext4 inodes carry a filesystem pointer we can dispatch through;
    // verify via the ops table identity.
    if !core::ptr::eq(inode.ops.unwrap_or(&crate::fs::ext4::EXT4_INODE_OPS) as *const _, &crate::fs::ext4::EXT4_INODE_OPS as *const _) {
        return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
    }
    crate::fs::ext4::ext4_fallocate(inode.ino as u32, mode, offset, len)
}

/// Filesystem-side utimensat: update atime/mtime of a path. `times` carries
/// (atime_sec, mtime_sec); UTIME_NOW is expressed by passing None for a
/// component. Minimal mtime-first implementation (review 5.5: utimensat
/// 实现——至少 mtime 更新); the syscall entry
/// (sys_futimesat/sys_utimensat in syscall/file.rs) currently ignores
/// timestamps — wiring is owned by the syscall-layer agent.
#[allow(dead_code)]
pub fn vfs_utimensat(pathname: &str, atime: Option<u64>, mtime: Option<u64>) -> Result<(), i32> {
    let vpath = path_lookup(pathname, 0)?;
    let inode = vpath.inode.as_ref()
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Permission: owner or CAP_FOWNER (Linux do_utimes).
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new_init()
    };
    let inode_uid = inode.uid.load(Ordering::Relaxed);
    if cred.euid != 0 && cred.euid != inode_uid
        && !crate::security::has_capability(&cred, crate::security::CAP_FOWNER)
    {
        return Err(errno::Errno::PermissionDenied.as_neg_i32());
    }

    if let Some(m) = mtime {
        let _ = inode.op_setattr(crate::fs::inode::setattr_attr::ATTR_MTIME, m, 0);
    }
    if let Some(a) = atime {
        let _ = inode.op_setattr(crate::fs::inode::setattr_attr::ATTR_ATIME, a, 0);
    }
    // inotify: timestamp change → IN_ATTRIB.
    let name = vpath.dentry.as_ref().map(|d| d.get_name().into_bytes());
    notify_about(vpath.dentry.as_ref(), Some(inode), ino::IN_ATTRIB, name.as_deref(), 0);
    Ok(())
}

// ============================================================================
// ProcFS Directory (for /proc/[pid]/fd/ shortcut)
// ============================================================================

/// Synthetic INodeOps for procfs directory shortcuts (e.g., /proc/[pid]/fd/)
/// Synthetic INodeOps for procfs directory shortcuts (e.g. /proc/[pid]/fd/).
/// pub(crate): procfs reuses it for the /proc/[pid]/fd lookup-layer inode.
pub(crate) static PROCFS_DIR_OPS: INodeOps = INodeOps {
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
    destroy_inode: None,
};

/// Readdir callback for synthetic procfs directories.
/// Reads PID from inode.private_data, calls procfs::pid::list_fds().
// SAFETY: private_data stores a valid PID value; list_fds is a safe fallible function
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
    // SAFETY: PROCFS_DIR_OPS is a static INodeOps with well-defined callbacks; fd install is safe
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
    // SAFETY: MEM_FILE_OPS is a static FileOps; Box::into_raw ownership is transferred to File
    unsafe {
        let file = Arc::new(File::new(FileFlags::new(flags)));
        file.set_ops(&MEM_FILE_OPS);
        let content = alloc::boxed::Box::new(MemFileContent { data, offset: 0 });
        file.set_private_data(alloc::boxed::Box::into_raw(content) as *mut u8);
        get_file_fd_install(file).ok_or(errno::Errno::TooManyOpenFiles.as_neg_i32())
    }
}
