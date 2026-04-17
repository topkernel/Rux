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
use crate::sync::spinlock::Spinlock;
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

/// Kind of file inside a /proc/[pid] directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidFileKind {
    Status,
    Stat,
    Cmdline,
    Exe,
    Cwd,
    Maps,
    Environ,
    OomScore,
    OomScoreAdj,
}

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
    pub children: Spinlock<Vec<Arc<ProcFSNode>>>,
    /// Reference count
    ref_count: AtomicU64,
    /// Node ID
    pub ino: u64,
    /// Cached content size (set when content is first generated)
    pub cached_size: AtomicU64,
    /// PID for per-process files
    pub pid: Option<u64>,
    /// File kind for per-process files
    pub pid_file_kind: Option<PidFileKind>,
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
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(0),
            pid: None,
            pid_file_kind: None,
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
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(0),
            pid: None,
            pid_file_kind: None,
        }
    }

    /// Create static content file node
    pub fn new_static_file(name: Vec<u8>, content: Vec<u8>, ino: u64) -> Self {
        let sz = content.len() as u64;
        Self {
            name,
            node_type: ProcFSType::RegularFile,
            content_generator: None,
            static_content: Some(content),
            link_generator: None,
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(sz),
            pid: None,
            pid_file_kind: None,
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
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(0),
            pid: None,
            pid_file_kind: None,
        }
    }

    /// Create static symlink node
    pub fn new_symlink(name: Vec<u8>, target: Vec<u8>, ino: u64) -> Self {
        let sz = target.len() as u64;
        Self {
            name,
            node_type: ProcFSType::SymbolicLink,
            content_generator: None,
            static_content: None,
            link_generator: None,
            link_target: Some(target),
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(sz),
            pid: None,
            pid_file_kind: None,
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
        // Dispatch per-PID files
        if let (Some(pid), Some(kind)) = (self.pid, self.pid_file_kind) {
            let content = match kind {
                PidFileKind::Status => pid::generate_status(pid),
                PidFileKind::Stat => pid::generate_stat(pid),
                PidFileKind::Cmdline => pid::generate_cmdline(pid),
                PidFileKind::Maps => pid::generate_maps(pid),
                PidFileKind::Environ => pid::generate_environ(pid),
                PidFileKind::OomScore => pid::generate_oom_score(pid),
                PidFileKind::OomScoreAdj => pid::generate_oom_score_adj(pid),
                // Symlinks handled by get_link_target()
                PidFileKind::Exe | PidFileKind::Cwd => Vec::new(),
            };
            self.cached_size.store(content.len() as u64, Ordering::Relaxed);
            return content;
        }

        if let Some(generator) = self.content_generator {
            let content = generator();
            self.cached_size.store(content.len() as u64, Ordering::Relaxed);
            content
        } else if let Some(ref content) = self.static_content {
            content.clone()
        } else {
            Vec::new()
        }
    }

    /// Get symlink target
    pub fn get_link_target(&self) -> Vec<u8> {
        // Dispatch per-PID symlinks
        if let (Some(pid), Some(kind)) = (self.pid, self.pid_file_kind) {
            return match kind {
                PidFileKind::Exe => pid::generate_exe_link(pid),
                PidFileKind::Cwd => pid::generate_cwd_link(pid),
                _ => Vec::new(),
            };
        }

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

// SAFETY: ProcFSNode's mutable fields (children, data) are protected by Spinlocks;
// name is a &'static str and other fields are atomic or read-only after creation.
unsafe impl Send for ProcFSNode {}
// SAFETY: all shared mutable state is protected by internal Spinlocks;
// no data races are possible across threads/CPUs.
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

        // Kernel log buffer
        self.create_dynamic_file("kmsg", crate::printk::generate_kmsg);

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
// SAFETY: _fs_context is a valid FsContext reference from the VFS mount call;
// ProcFSSuperBlock is a simple wrapper with no unsafe invariants.
unsafe extern "C" fn procfs_mount(_fs_context: &crate::fs::superblock::FsContext<'_>) -> Result<*mut SuperBlock, i32> {
    let procfs_sb = alloc::boxed::Box::new(ProcFSSuperBlock::new());
    let procfs_sb_ptr = alloc::boxed::Box::into_raw(procfs_sb) as *mut SuperBlock;
    Ok(procfs_sb_ptr)
}

/// ProcFS unmount function
// SAFETY: sb is a valid SuperBlock pointer from a previous procfs_mount call;
// Box::from_raw reclaims the ProcFSSuperBlock created during mount.
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
    use crate::process::current_pid;

    // Handle /proc/[pid]/xxx paths directly (lookup() doesn't support PID dirs)
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // Match /proc/[pid]/file or /proc/self/file
    if components.len() == 3 && components[0] == "proc" {
        let first = if components[1] == "self" {
            alloc::format!("{}", current_pid())
        } else {
            alloc::string::String::from(components[1])
        };
        let filename = components[2];

        if let Some(pid) = pid::parse_pid(first.as_bytes()) {
            return match filename {
                "status" => Some(pid::generate_status(pid)),
                "cmdline" => Some(pid::generate_cmdline(pid)),
                "stat" => Some(pid::generate_stat(pid)),
                "maps" => Some(pid::generate_maps(pid)),
                "exe" => Some(pid::generate_exe_link(pid)),
                "cwd" => Some(pid::generate_cwd_link(pid)),
                "environ" => Some(pid::generate_environ(pid)),
                "oom_score" => Some(pid::generate_oom_score(pid)),
                "oom_score_adj" => Some(pid::generate_oom_score_adj(pid)),
                _ => None,
            };
        }
    }

    // Match /proc/[pid]/fd/N (fd symlink target)
    if components.len() == 4 && components[0] == "proc" && components[2] == "fd" {
        let first = if components[1] == "self" {
            alloc::format!("{}", current_pid())
        } else {
            alloc::string::String::from(components[1])
        };

        if let Some(pid) = pid::parse_pid(first.as_bytes()) {
            if let Ok(fd) = components[3].parse::<u32>() {
                let target = pid::generate_fd_link(pid, fd);
                if !target.is_empty() {
                    return Some(target);
                }
            }
        }
    }

    // Legacy 2-component paths (e.g. from internal procfs code)
    if components.len() == 2 {
        let first = if components[0] == "self" {
            alloc::format!("{}", current_pid())
        } else {
            alloc::string::String::from(components[0])
        };

        if let Some(pid) = pid::parse_pid(first.as_bytes()) {
            return match components[1] {
                "status" => Some(pid::generate_status(pid)),
                "cmdline" => Some(pid::generate_cmdline(pid)),
                "stat" => Some(pid::generate_stat(pid)),
                "maps" => Some(pid::generate_maps(pid)),
                "exe" => Some(pid::generate_exe_link(pid)),
                "cwd" => Some(pid::generate_cwd_link(pid)),
                "environ" => Some(pid::generate_environ(pid)),
                "oom_score" => Some(pid::generate_oom_score(pid)),
                "oom_score_adj" => Some(pid::generate_oom_score_adj(pid)),
                _ => None,
            };
        }
    }

    get_procfs_sb()?.read_file(path)
}

/// Check if procfs is mounted
pub fn is_mounted() -> bool {
    let mount_ptr = GLOBAL_PROC_MOUNT.load(Ordering::Acquire);
    !mount_ptr.is_null()
}

/// Create a VFS inode for the procfs root directory.
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
    let sb = match get_procfs_sb() {
        Some(sb) => sb,
        None => {
            // Fallback: create a minimal root node
            let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o555));
            inode.ops = Some(&PROCFS_INODE_OPS);
            return alloc::sync::Arc::new(inode);
        }
    };
    let root_node = sb.root_node.clone();
    let mut inode = Inode::new(root_node.ino, InodeMode::new(InodeMode::S_IFDIR | 0o555));
    inode.ops = Some(&PROCFS_INODE_OPS);
    inode.private_data = Some(alloc::sync::Arc::as_ptr(&root_node) as *mut u8);
    alloc::sync::Arc::new(inode)
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
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
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
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
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
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    // PID directory parent: looking up files inside /proc/[pid]/
    if dir.private_data.is_none() && pid::is_valid_pid(dir.ino) {
        let pid_val = dir.ino;
        // Files inside PID directory
        let kind = match name {
            b"status" => PidFileKind::Status,
            b"stat" => PidFileKind::Stat,
            b"cmdline" => PidFileKind::Cmdline,
            b"exe" => PidFileKind::Exe,
            b"cwd" => PidFileKind::Cwd,
            b"maps" => PidFileKind::Maps,
            b"environ" => PidFileKind::Environ,
            b"oom_score" => PidFileKind::OomScore,
            b"oom_score_adj" => PidFileKind::OomScoreAdj,
            _ => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        };
        // Use a hash of pid + file kind as inode number
        let file_ino = pid_val * 1000 + (kind as u64) + 100;
        return Ok(file_ino);
    }

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
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    // PID directory: private_data is None, inode.ino stores the PID
    if inode.private_data.is_none() && pid::is_valid_pid(inode.ino) {
        let pid = inode.ino;
        stat.st_ino = pid;
        stat.st_mode = InodeMode::S_IFDIR | 0o555;
        stat.st_size = 0;
        stat.st_nlink = 2;
        stat.st_uid = 0;
        stat.st_gid = 0;
        stat.st_rdev = 0;
        stat.st_blksize = 4096;
        stat.st_blocks = 0;
        stat.st_atime = 0;
        stat.st_atime_nsec = 0;
        stat.st_mtime = 0;
        stat.st_mtime_nsec = 0;
        stat.st_ctime = 0;
        stat.st_ctime_nsec = 0;
        return 0;
    }

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
    // Avoid calling node.size() here — it regenerates full file content
    // (via content_generator) just to get the length, which is expensive
    // and can cause slab allocator issues in stat context.
    // Use a cached/approximate size instead.
    stat.st_size = node.cached_size.load(Ordering::Relaxed) as i64;
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
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
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

/// ProcFS iget: instantiate a VFS Inode from (parent_inode, name, child_ino).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, i32> {
    // PID directory parent: creating inode for files inside /proc/[pid]/
    if parent.private_data.is_none() && pid::is_valid_pid(parent.ino) {
        let pid_val = parent.ino;
        let kind = match name {
            b"status" => PidFileKind::Status,
            b"stat" => PidFileKind::Stat,
            b"cmdline" => PidFileKind::Cmdline,
            b"exe" => PidFileKind::Exe,
            b"cwd" => PidFileKind::Cwd,
            b"maps" => PidFileKind::Maps,
            b"environ" => PidFileKind::Environ,
            b"oom_score" => PidFileKind::OomScore,
            b"oom_score_adj" => PidFileKind::OomScoreAdj,
            _ => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        };

        let is_symlink = matches!(kind, PidFileKind::Exe | PidFileKind::Cwd);
        let mode = if is_symlink {
            InodeMode::new(InodeMode::S_IFLNK | 0o777)
        } else {
            InodeMode::new(InodeMode::S_IFREG | 0o444)
        };

        // Create a ProcFSNode for this PID file and leak it (never freed).
        let proc_node = Arc::new(ProcFSNode {
            name: name.to_vec(),
            node_type: if is_symlink { ProcFSType::SymbolicLink } else { ProcFSType::RegularFile },
            content_generator: None,
            static_content: None,
            link_generator: None,
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(0),
            pid: Some(pid_val),
            pid_file_kind: Some(kind),
        });
        let raw_ptr = Arc::into_raw(proc_node) as *mut u8;

        let mut inode = Inode::new(ino, mode);
        inode.ops = Some(&PROCFS_INODE_OPS);
        inode.private_data = Some(raw_ptr);
        return Ok(Arc::new(inode));
    }

    let node_ptr = parent.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let parent_node = &*(node_ptr as *const ProcFSNode);

    // Check for PID directory
    if pid::is_pid_dir(name) {
        if let Some(pid_val) = pid::parse_pid(name) {
            use crate::process::{current_pid, find_task_by_pid};
            if current_pid() as u64 == pid_val || find_task_by_pid(pid_val as u32).is_some() {
                let mode = InodeMode::new(InodeMode::S_IFDIR | 0o555);
                let mut inode = Inode::new(pid_val, mode);
                inode.ops = Some(&PROCFS_INODE_OPS);
                // private_data for PID dirs is set lazily when accessed
                return Ok(Arc::new(inode));
            }
        }
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    let child = parent_node.find_child(name)
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let mode = if child.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | 0o555)
    } else if child.is_symlink() {
        InodeMode::new(InodeMode::S_IFLNK | 0o777)
    } else {
        InodeMode::new(InodeMode::S_IFREG | 0o444)
    };

    let mut inode = Inode::new(child.ino, mode);
    inode.ops = Some(&PROCFS_INODE_OPS);
    inode.private_data = Some(Arc::as_ptr(&child) as *mut u8);
    Ok(Arc::new(inode))
}

/// ProcFS file content structure (stored in File's private_data)
#[repr(C)]
pub struct ProcfsFileContent {
    /// File content
    pub data: alloc::vec::Vec<u8>,
    /// Current read offset
    pub offset: usize,
}

/// ProcFS file read operation
fn procfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            let content = &*(content_ptr as *const ProcfsFileContent);
            let offset = file.get_pos() as usize;
            let available = content.data.len().saturating_sub(offset);
            let to_read = buf.len().min(available);
            if to_read > 0 {
                buf[..to_read].copy_from_slice(&content.data[offset..offset + to_read]);
                file.set_pos((offset + to_read) as u64);
                to_read as isize
            } else {
                0
            }
        } else {
            -9  // EBADF
        }
    }
}

/// ProcFS file write operation (read-only)
fn procfs_file_write(_file: &crate::fs::File, _buf: &[u8]) -> isize {
    -9  // EBADF
}

/// ProcFS file lseek operation
fn procfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            let content = &*(content_ptr as *const ProcfsFileContent);
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
            -9
        }
    }
}

/// ProcFS file close operation
fn procfs_file_close(file: &crate::fs::File) -> i32 {
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(content_ptr) = *data_opt {
            // Null out pointer first to prevent use-after-free by concurrent readers
            *file.private_data.get() = None;
            let _ = alloc::boxed::Box::from_raw(content_ptr as *mut ProcfsFileContent);
        }
        0
    }
}

/// ProcFS file operations table
pub static PROCFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(procfs_file_read),
    write: Some(procfs_file_write),
    lseek: Some(procfs_file_lseek),
    close: Some(procfs_file_close),
    poll: None,
};

/// ProcFS get_file_ops: return ops based on inode type
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&PROCFS_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// ProcFS open: pre-read content for regular files
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_open(inode: &Inode, file: &crate::fs::File) -> i32 {
    if !inode.mode.is_regular_file() {
        return 0;
    }
    let node_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return 0,
    };
    let node = &*(node_ptr as *const ProcFSNode);
    let content = node.get_content();
    let file_content = alloc::boxed::Box::new(ProcfsFileContent {
        data: content,
        offset: 0,
    });
    let content_ptr = alloc::boxed::Box::into_raw(file_content) as *mut u8;
    file.set_private_data(content_ptr);
    0
}

/// Generate directory entries for a /proc/[pid]/ directory
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn generate_pid_dir_entries(pid: u64) -> alloc::vec::Vec<crate::fs::inode::VfsDirEntry> {
    use crate::fs::inode::file_type;

    let mut entries = alloc::vec::Vec::new();

    // . and ..
    entries.push(crate::fs::inode::VfsDirEntry {
        ino: pid,
        name: alloc::vec![b'.'],
        file_type: file_type::DT_DIR,
    });
    entries.push(crate::fs::inode::VfsDirEntry {
        ino: 1, // proc root ino
        name: alloc::vec![b'.', b'.'],
        file_type: file_type::DT_DIR,
    });

    // Static files
    let files: &[(&[u8], u8)] = &[
        (b"status", file_type::DT_REG),
        (b"cmdline", file_type::DT_REG),
        (b"stat", file_type::DT_REG),
        (b"maps", file_type::DT_REG),
        (b"environ", file_type::DT_REG),
        (b"exe", file_type::DT_LNK),
        (b"cwd", file_type::DT_LNK),
        (b"fd", file_type::DT_DIR),
        (b"oom_score", file_type::DT_REG),
        (b"oom_score_adj", file_type::DT_REG),
    ];

    for (name, ft) in files.iter() {
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: pid,
            name: name.to_vec(),
            file_type: *ft,
        });
    }

    entries
}

/// ProcFS readdir: list directory entries
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;

    // PID directory: private_data is None, inode.ino stores the PID
    // Only match when private_data is absent (distinguishes from root procfs inode)
    if inode.private_data.is_none() && pid::is_valid_pid(inode.ino) {
        let pid = inode.ino;
        return Some(generate_pid_dir_entries(pid));
    }

    let node_ptr = inode.private_data?;
    let node = &*(node_ptr as *const ProcFSNode);
    if !node.is_dir() {
        return None;
    }
    let children = node.list_children();
    let mut entries = alloc::vec::Vec::new();
    for (name, ptype, ino) in children.iter() {
        let dt = match ptype {
            ProcFSType::Directory => file_type::DT_DIR,
            ProcFSType::RegularFile => file_type::DT_REG,
            ProcFSType::SymbolicLink => file_type::DT_LNK,
        };
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: *ino,
            name: name.clone(),
            file_type: dt,
        });
    }

    // For procfs root (ino == 1), also list all active PID directories
    if inode.ino == 1 {
        use crate::process::pid_hash;
        let (pids, count, _truncated) = pid_hash::pid_hash_collect_all();
        for i in 0..count {
            entries.push(crate::fs::inode::VfsDirEntry {
                ino: pids[i] as u64,
                name: alloc::format!("{}", pids[i]).into_bytes(),
                file_type: file_type::DT_DIR,
            });
        }
    }

    Some(entries)
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
    get_file_ops: Some(procfs_get_file_ops),
    readdir: Some(procfs_readdir),
    open: Some(procfs_open),
    permission: None,  // Default: allow all
    getattr: Some(procfs_getattr),
    setattr: None,     // ProcFS is read-only
    iget: Some(procfs_iget),
    destroy_inode: None,
};
