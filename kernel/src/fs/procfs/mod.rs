//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ProcFS - Process information filesystem
//!
//! ## Overview
//!
//! ProcFS provides a filesystem interface to kernel and process information.
//! It is mounted at /proc.
//!
//! ## Directory Structure
//!
//! ```text
//! /proc/
//! ├── meminfo      - Memory information
//! ├── cpuinfo      - CPU information
//! ├── version      - Kernel version
//! ├── uptime       - System uptime
//! ├── cmdline      - Kernel boot parameters
//! ├── loadavg      - System load average
//! ├── mounts       - Mounted filesystems
//! ├── filesystems  - Supported filesystem types
//! ├── self         - Symlink to current process directory
//! └── [pid]/       - Process-specific directories
//!     ├── status   - Process status
//!     ├── cmdline  - Command line arguments
//!     ├── stat     - Process statistics
//!     ├── exe      - Symlink to executable
//!     ├── cwd      - Symlink to current working directory
//!     ├── environ  - Environment variables
//!     └── fd/      - File descriptors
//! ```

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::superblock::{SuperBlock, SuperBlockFlags, FileSystemType};
use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps};
use crate::fs::mount::{VfsMount, MntFlags};
use crate::errno;

// Sub-modules for individual procfs entries
pub mod meminfo;
pub mod cpuinfo;
pub mod version;
pub mod uptime;
pub mod cmdline;
pub mod mounts;
pub mod loadavg;
pub mod self_proc;
pub mod pid;
pub mod interrupts;

// Re-export uptime functions for other modules
pub use uptime::get_uptime_seconds;
pub use uptime::get_uptime_ms;

/// ProcFS magic number
const PROCFS_MAGIC: u32 = 0x9fa0;

/// ProcFS node type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcFSType {
    /// Directory
    Directory,
    /// Regular file (dynamically generated content)
    RegularFile,
    /// Symbolic link
    SymbolicLink,
}

/// Dynamic content generator function type
pub type ContentGenerator = fn() -> Vec<u8>;

/// ProcFS node
pub struct ProcFSNode {
    /// Node name
    pub name: Vec<u8>,
    /// Node type
    pub node_type: ProcFSType,
    /// Dynamic content generator (for regular files)
    pub content_generator: Option<ContentGenerator>,
    /// Static content (if no content generator)
    pub static_content: Option<Vec<u8>>,
    /// Symbolic link target generator (for symlinks)
    pub link_generator: Option<fn() -> Vec<u8>>,
    /// Symbolic link target (static)
    pub link_target: Option<Vec<u8>>,
    /// Child nodes (if directory)
    pub children: Mutex<Vec<Arc<ProcFSNode>>>,
    /// Reference count
    ref_count: AtomicU64,
    /// Node ID
    pub ino: u64,
}

impl ProcFSNode {
    /// Create directory node
    pub fn new_dir(name: Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::Directory,
            content_generator: None,
            static_content: None,
            link_generator: None,
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// Create dynamic content file node
    pub fn new_dynamic_file(name: Vec<u8>, generator: ContentGenerator, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::RegularFile,
            content_generator: Some(generator),
            static_content: None,
            link_generator: None,
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// Create static content file node
    pub fn new_static_file(name: Vec<u8>, content: Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::RegularFile,
            content_generator: None,
            static_content: Some(content),
            link_generator: None,
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// Create dynamic symlink node
    pub fn new_dynamic_symlink(name: Vec<u8>, link_gen: fn() -> Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::SymbolicLink,
            content_generator: None,
            static_content: None,
            link_generator: Some(link_gen),
            link_target: None,
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// Create static symlink node
    pub fn new_symlink(name: Vec<u8>, target: Vec<u8>, ino: u64) -> Self {
        Self {
            name,
            node_type: ProcFSType::SymbolicLink,
            content_generator: None,
            static_content: None,
            link_generator: None,
            link_target: Some(target),
            children: Mutex::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// Check if it's a directory
    pub fn is_dir(&self) -> bool {
        self.node_type == ProcFSType::Directory
    }

    /// Check if it's a regular file
    pub fn is_file(&self) -> bool {
        self.node_type == ProcFSType::RegularFile
    }

    /// Check if it's a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.node_type == ProcFSType::SymbolicLink
    }

    /// Get file content
    pub fn get_content(&self) -> Vec<u8> {
        if let Some(generator) = self.content_generator {
            generator()
        } else if let Some(ref content) = self.static_content {
            content.clone()
        } else {
            Vec::new()
        }
    }

    /// Get symlink target
    pub fn get_link_target(&self) -> Vec<u8> {
        if let Some(generator) = self.link_generator {
            generator()
        } else if let Some(ref target) = self.link_target {
            target.clone()
        } else {
            Vec::new()
        }
    }

    /// Get file size
    pub fn size(&self) -> usize {
        if self.is_symlink() {
            self.get_link_target().len()
        } else {
            self.get_content().len()
        }
    }

    /// Find child node
    pub fn find_child(&self, name: &[u8]) -> Option<Arc<ProcFSNode>> {
        // Check if it's a PID directory request
        if self.name == b"/" && pid::is_pid_dir(name) {
            if let Some(_pid) = pid::parse_pid(name) {
                // Create a virtual PID directory node
                // This is a dynamic lookup
                return None;  // TODO: Implement dynamic PID node creation
            }
        }

        let children = self.children.lock();
        for child in children.iter() {
            if child.name.as_slice() == name {
                return Some(child.clone());
            }
        }
        None
    }

    /// Add child node
    pub fn add_child(&self, child: Arc<ProcFSNode>) {
        let mut children = self.children.lock();
        children.push(child);
    }

    /// List child nodes
    pub fn list_children(&self) -> Vec<(Vec<u8>, ProcFSType, u64)> {
        let children = self.children.lock();
        let mut result: Vec<(Vec<u8>, ProcFSType, u64)> = children
            .iter()
            .map(|c| (c.name.clone(), c.node_type, c.ino))
            .collect();

        // Add dynamic PID directories
        // TODO: Get actual running process list
        // For now, just add current process
        use crate::process::current_pid;
        let pid = current_pid() as u64;
        let pid_str = format!("{}", pid);
        result.push((pid_str.into_bytes(), ProcFSType::Directory, pid));

        result
    }

    /// Increment reference count
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement reference count
    pub fn put(&self) -> u64 {
        self.ref_count.fetch_sub(1, Ordering::Relaxed)
    }
}

unsafe impl Send for ProcFSNode {}
unsafe impl Sync for ProcFSNode {}

/// ProcFS superblock
pub struct ProcFSSuperBlock {
    /// Base superblock
    pub sb: SuperBlock,
    /// Root node
    pub root_node: Arc<ProcFSNode>,
    /// Next inode ID
    next_ino: AtomicU64,
}

impl ProcFSSuperBlock {
    /// Create new ProcFS superblock
    pub fn new() -> Self {
        let sb = SuperBlock::new(4096, PROCFS_MAGIC);
        let root_node = Arc::new(ProcFSNode::new_dir(b"/".to_vec(), 1));

        Self {
            sb,
            root_node,
            next_ino: AtomicU64::new(2),
        }
    }

    /// Allocate new inode number
    pub fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::Relaxed)
    }

    /// Initialize default files
    pub fn init_default_files(&self) {
        // System information files
        self.create_dynamic_file("meminfo", meminfo::generate);
        self.create_dynamic_file("cpuinfo", cpuinfo::generate);
        self.create_dynamic_file("version", version::generate);
        self.create_dynamic_file("uptime", uptime::generate);
        self.create_dynamic_file("cmdline", cmdline::generate);
        self.create_dynamic_file("loadavg", loadavg::generate);
        self.create_dynamic_file("mounts", mounts::generate);
        self.create_dynamic_file("filesystems", mounts::generate_filesystems);
        self.create_dynamic_file("mountinfo", mounts::generate_mountinfo);
        self.create_dynamic_file("interrupts", interrupts::generate);

        // /proc/self - symlink to current process directory
        self.create_dynamic_symlink("self", self_proc::get_self_link);

        // /proc/[pid] directories are handled dynamically
    }

    /// Create dynamic content file
    fn create_dynamic_file(&self, name: &str, generator: ContentGenerator) {
        let ino = self.alloc_ino();
        let file = Arc::new(ProcFSNode::new_dynamic_file(
            name.as_bytes().to_vec(),
            generator,
            ino,
        ));
        self.root_node.add_child(file);
    }

    /// Create static content file
    fn create_static_file(&self, name: &str, content: Vec<u8>) {
        let ino = self.alloc_ino();
        let file = Arc::new(ProcFSNode::new_static_file(
            name.as_bytes().to_vec(),
            content,
            ino,
        ));
        self.root_node.add_child(file);
    }

    /// Create dynamic symlink
    fn create_dynamic_symlink(&self, name: &str, link_gen: fn() -> Vec<u8>) {
        let ino = self.alloc_ino();
        let link = Arc::new(ProcFSNode::new_dynamic_symlink(
            name.as_bytes().to_vec(),
            link_gen,
            ino,
        ));
        self.root_node.add_child(link);
    }

    /// Create static symlink
    fn create_symlink(&self, name: &str, target: &str) {
        let ino = self.alloc_ino();
        let link = Arc::new(ProcFSNode::new_symlink(
            name.as_bytes().to_vec(),
            target.as_bytes().to_vec(),
            ino,
        ));
        self.root_node.add_child(link);
    }

    /// Lookup file
    pub fn lookup(&self, path: &str) -> Option<Arc<ProcFSNode>> {
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Some(self.root_node.clone());
        }

        let mut current = self.root_node.clone();
        for component in components {
            if component == "." {
                continue;
            }
            if component == ".." {
                current = self.root_node.clone();
                continue;
            }

            // Check for PID directory
            let component_bytes = component.as_bytes();
            if pid::is_pid_dir(component_bytes) {
                // Create virtual PID directory node
                // For now, return None - TODO: implement
                return None;
            }

            match current.find_child(component_bytes) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    /// Read file content
    pub fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let node = self.lookup(path)?;
        if node.is_file() {
            Some(node.get_content())
        } else if node.is_symlink() {
            Some(node.get_link_target())
        } else {
            None
        }
    }

    /// List directory contents
    pub fn list_dir(&self, path: &str) -> Option<Vec<(Vec<u8>, ProcFSType, u64)>> {
        let node = self.lookup(path)?;
        if node.is_dir() {
            Some(node.list_children())
        } else {
            None
        }
    }
}

// ============================================================================
// Filesystem Type Registration
// ============================================================================

/// ProcFS filesystem type
pub static PROCFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "proc",
    Some(procfs_mount),
    Some(procfs_kill_sb),
    0,
);

/// Global ProcFS superblock pointer
static GLOBAL_PROCFS_SB: core::sync::atomic::AtomicPtr<ProcFSSuperBlock> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Global ProcFS mount point pointer
static GLOBAL_PROC_MOUNT: core::sync::atomic::AtomicPtr<VfsMount> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// ProcFS mount function
unsafe extern "C" fn procfs_mount(_fs_context: &crate::fs::superblock::FsContext<'_>) -> Result<*mut SuperBlock, i32> {
    let procfs_sb = alloc::boxed::Box::new(ProcFSSuperBlock::new());
    let procfs_sb_ptr = alloc::boxed::Box::into_raw(procfs_sb) as *mut SuperBlock;
    Ok(procfs_sb_ptr)
}

/// ProcFS unmount function
unsafe extern "C" fn procfs_kill_sb(sb: *mut SuperBlock) {
    if !sb.is_null() {
        let _ = alloc::boxed::Box::from_raw(sb as *mut ProcFSSuperBlock);
    }
}

/// Get ProcFS superblock
pub fn get_procfs_sb() -> Option<&'static ProcFSSuperBlock> {
    let ptr = GLOBAL_PROCFS_SB.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

/// Read file from /proc
pub fn read_file(path: &str) -> Option<Vec<u8>> {
    get_procfs_sb()?.read_file(path)
}

/// Check if procfs is mounted
pub fn is_mounted() -> bool {
    let mount_ptr = GLOBAL_PROC_MOUNT.load(Ordering::Acquire);
    !mount_ptr.is_null()
}

/// List /proc directory
pub fn list_dir(path: &str) -> Option<Vec<(Vec<u8>, ProcFSType, u64)>> {
    get_procfs_sb()?.list_dir(path)
}

/// Check if path exists in procfs
pub fn exists(path: &str) -> bool {
    if let Some(sb) = get_procfs_sb() {
        if path == "/" || path.is_empty() {
            return true;
        }
        sb.read_file(path).is_some() || sb.list_dir(path).is_some()
    } else {
        false
    }
}

/// Initialize ProcFS
pub fn init_procfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;

    // 1. Register filesystem type
    register_filesystem(&PROCFS_FS_TYPE)?;

    // 2. Create superblock
    let procfs_sb = alloc::boxed::Box::new(ProcFSSuperBlock::new());
    let procfs_sb_ptr = alloc::boxed::Box::into_raw(procfs_sb) as *mut ProcFSSuperBlock;

    // 3. Initialize default files
    unsafe {
        (*procfs_sb_ptr).init_default_files();
    }

    // 4. Store global pointer
    GLOBAL_PROCFS_SB.store(procfs_sb_ptr, Ordering::Release);

    Ok(())
}

/// Mount ProcFS to /proc
pub fn mount_procfs() -> Result<(), i32> {
    // Get RootFS superblock
    let rootfs_sb = match crate::fs::rootfs::get_rootfs_sb() {
        Some(sb) => sb,
        None => return Err(-1),
    };

    // Try to create /proc directory in RootFS (ignore error if already exists)
    unsafe {
        let _ = (*rootfs_sb).create_dir("/proc", 0o755);
    }

    // Create mount point
    let procfs_sb_ptr = GLOBAL_PROCFS_SB.load(Ordering::Acquire);
    if procfs_sb_ptr.is_null() {
        return Err(-1);
    }

    let mount = alloc::boxed::Box::new(VfsMount::new(
        b"/proc".to_vec(),
        b"/proc".to_vec(),
        MntFlags::new(0),
        Some(procfs_sb_ptr as *mut u8),
    ));
    let mount_ptr = alloc::boxed::Box::into_raw(mount) as *mut VfsMount;
    GLOBAL_PROC_MOUNT.store(mount_ptr, Ordering::Release);

    Ok(())
}

// ============================================================================
// ProcFS Inode Operations
// ============================================================================

/// ProcFS inode lookup operation
unsafe fn procfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let node_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let node = &*(node_ptr as *const ProcFSNode);

    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Check for PID directory
    if pid::is_pid_dir(name) {
        if let Some(pid_val) = pid::parse_pid(name) {
            // PID directories are dynamic - check if process exists
            use crate::process::{current_pid, find_task_by_pid};
            if current_pid() as u64 == pid_val || find_task_by_pid(pid_val as u32).is_some() {
                return Ok(pid_val);  // Use PID as inode number
            }
        }
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    match node.find_child(name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

/// ProcFS getattr operation
unsafe fn procfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let node_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let node = &*(node_ptr as *const ProcFSNode);

    stat.st_ino = node.ino;
    stat.st_mode = if node.is_dir() {
        InodeMode::S_IFDIR | 0o555  // read-only directory
    } else if node.is_symlink() {
        InodeMode::S_IFLNK | 0o777
    } else {
        InodeMode::S_IFREG | 0o444  // read-only file
    };
    stat.st_size = node.size() as i64;
    stat.st_nlink = 1;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = (stat.st_size as i64 + 511) / 512;
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;

    0
}

/// ProcFS readlink operation
unsafe fn procfs_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let node_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let node = &*(node_ptr as *const ProcFSNode);

    if !node.is_symlink() {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }

    let target = node.get_link_target();
    let len = target.len().min(buf.len());
    buf[..len].copy_from_slice(&target[..len]);
    len as isize
}

/// ProcFS inode operations table
/// ProcFS is a read-only filesystem, so most operations are not supported
pub static PROCFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(procfs_lookup),
    create: None,      // ProcFS is read-only
    link: None,        // ProcFS is read-only
    unlink: None,      // ProcFS is read-only
    symlink: None,     // ProcFS is read-only
    mkdir: None,       // ProcFS is read-only
    rmdir: None,       // ProcFS is read-only
    mknod: None,       // ProcFS is read-only
    rename: None,      // ProcFS is read-only
    readlink: Some(procfs_readlink),
    get_file_ops: None,
    permission: None,  // Default: allow all
    getattr: Some(procfs_getattr),
    setattr: None,     // ProcFS is read-only
};
