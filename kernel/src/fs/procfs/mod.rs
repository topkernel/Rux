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
pub mod swaps;
pub mod cpuinfo;
pub mod version;
pub mod uptime;
pub mod cmdline;
pub mod mounts;
pub mod loadavg;
pub mod self_proc;
pub mod pid;
pub mod diskstats;
pub mod interrupts;
pub mod kallsyms;
pub mod locks;
pub mod stat;
pub mod schedstat;
pub mod softirqs;
pub mod sysctl;
pub mod sysrq;
pub mod timer_list;
pub mod procfile;
pub mod vmstat;
pub mod net;
pub mod configgz;


/// ProcFS magic number
const PROCFS_MAGIC: u32 = 0x9fa0;

/// Ino scheme for the synthetic /proc/[pid] entries:
///   pid * 10000 + 100 + kind       — per-PID regular files / symlinks
///   pid * 10000 + FD_DIR_INO_OFF   — the "fd" subdirectory
///   pid * 10000 + FD_LINK_INO_OFF + fd — the "fd/<N>" symlinks
///   pid * 10000 + NS_DIR_INO_OFF   — the "ns" subdirectory (U1c)
///   pid * 10000 + NS_LINK_INO_OFF + kind_idx — the "ns/<kind>" links (U1c)
pub(crate) const FD_DIR_INO_OFF: u64 = 200;
pub(crate) const FD_LINK_INO_OFF: u64 = 300;
pub(crate) const NS_DIR_INO_OFF: u64 = 400;
pub(crate) const NS_LINK_INO_OFF: u64 = 410;
/// /proc/[pid]/attr subdirectory (SELinux label files).
pub(crate) const ATTR_DIR_INO_OFF: u64 = 500;
/// pid * 10000 + 510/511/512 = attr/{exec,current,fscreate}.
pub(crate) const ATTR_FILE_INO_OFF: u64 = 510;

/// Decode an attr file ino into (pid, which-attr) — 0 exec, 1 current,
/// 2 fscreate.
pub(crate) fn attr_file_kind_of(ino: u64) -> Option<(u64, u8)> {
    let pid_val = ino / 10000;
    let rem = ino % 10000;
    if pid::is_valid_pid(pid_val) && rem >= ATTR_FILE_INO_OFF && rem < ATTR_FILE_INO_OFF + 6 {
        Some((pid_val, (rem - ATTR_FILE_INO_OFF) as u8))
    } else {
        None
    }
}

/// /proc/[pid]/task subdirectory (thread IDs). The per-tid directories
/// alias the tid's own PID directory (same ino = tid), so
/// /proc/self/task/<tid>/attr/... resolves through the regular
/// /proc/[pid] machinery — libselinux addresses attr files through the
/// task path, not /proc/<pid>.
pub(crate) const TASK_DIR_INO_OFF: u64 = 620;

/// /proc/[pid]/fdinfo subdirectory: per-fd status files (fdinfo(5)).
/// Offsets stay clear of the fd-link range (300..1324) and of the
/// PidFileKind range (100..200).
pub(crate) const FDINFO_DIR_INO_OFF: u64 = 1400;
/// pid * 10000 + FDINFO_FILE_INO_OFF + fd — the "fdinfo/<N>" files.
pub(crate) const FDINFO_FILE_INO_OFF: u64 = 1500;

/// Is this ino the /proc/[pid]/fdinfo subdirectory?
pub(crate) fn is_fdinfo_dir(ino: u64) -> bool {
    let pid_val = ino / 10000;
    pid::is_valid_pid(pid_val) && ino % 10000 == FDINFO_DIR_INO_OFF
}

/// Decode a fdinfo file ino into (pid, fd).
pub(crate) fn fdinfo_file_of(ino: u64) -> Option<(u64, u32)> {
    let pid_val = ino / 10000;
    let rem = ino % 10000;
    let fd = rem.checked_sub(FDINFO_FILE_INO_OFF)?;
    if pid::is_valid_pid(pid_val) && (fd as usize) < crate::fs::file::MAX_FDS {
        Some((pid_val, fd as u32))
    } else {
        None
    }
}

/// Is this ino the /proc/[pid]/task subdirectory?
pub(crate) fn is_task_dir(ino: u64) -> bool {
    let pid_val = ino / 10000;
    pid::is_valid_pid(pid_val) && ino % 10000 == TASK_DIR_INO_OFF
}

/// Is this ino the /proc/[pid]/attr subdirectory?
pub(crate) fn is_attr_dir(ino: u64) -> bool {
    let pid_val = ino / 10000;
    pid::is_valid_pid(pid_val) && ino % 10000 == ATTR_DIR_INO_OFF
}

/// Kind of file inside a /proc/[pid] directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidFileKind {
    Status,
    Stat,
    Cmdline,
    Exe,
    Cwd,
    Maps,
    Smaps,
    Environ,
    OomScore,
    OomScoreAdj,
    /// /proc/[pid]/comm — the task's comm (basename of exe, 15 chars
    /// like TASK_COMM_LEN). systemd compares /proc/1/comm against
    /// "systemd" to detect PID 1.
    Comm,
    /// /proc/[pid]/cgroup — cgroups v2 membership ("0::/"). systemd's
    /// boot probe parses this to place itself in the unified hierarchy.
    Cgroup,
    /// /proc/[pid]/mounts — per-process mount list (same view as
    /// /proc/mounts for the single mount namespace). LTP tst_cgroup
    /// (cgroup_core01..03, clone303, madvise06, memcg_*) opens
    /// /proc/self/mounts to find the cgroup2 mount point.
    Mounts,
    /// /proc/[pid]/attr/exec — SELinux exec label (writable; OH init
    /// writes the service context here via setexeccon() before exec —
    /// ENOENT made init abort every display service child).
    AttrExec,
    /// /proc/[pid]/attr/current — SELinux current label (writable).
    AttrCurrent,
    /// /proc/[pid]/attr/fscreate — file-create label (writable).
    AttrFscreate,
    /// /proc/[pid]/attr/sockcreate — socket-create label (writable).
    /// libselinux's setsockcreatecon() writes here before OH init binds
    /// a service socket; ENOENT aborted every service child that owns a
    /// socket (hilogd, samgr-adjacent helpers) before exec.
    AttrSockcreate,
    /// /proc/[pid]/attr/prev — previous label (read-only, empty).
    AttrPrev,
    /// /proc/[pid]/attr/keycreate — keyring-create label (writable).
    AttrKeycreate,
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
    /// P1 /proc/sys: write handler (None = read-only file). Presence also
    /// upgrades the stat mode from 0444 to 0644 so the DAC write check
    /// passes for root.
    pub sysctl_write: Option<fn(&[u8]) -> i32>,
    /// fd of /proc/[pid]/fdinfo/<fd> nodes (content is generated per fd;
    /// pid + fd address the descriptor, see pid::generate_fdinfo).
    pub fdinfo_fd: Option<u32>,
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
            sysctl_write: None,
            fdinfo_fd: None,
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
            sysctl_write: None,
            fdinfo_fd: None,
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
            sysctl_write: None,
            fdinfo_fd: None,
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
            sysctl_write: None,
            fdinfo_fd: None,
        }
    }

    /// Create a sysctl file: dynamic read content + write handler
    /// (P1 /proc/sys).
    pub fn new_sysctl_file(
        name: Vec<u8>,
        generator: ContentGenerator,
        write: fn(&[u8]) -> i32,
        ino: u64,
    ) -> Self {
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
            sysctl_write: Some(write),
            fdinfo_fd: None,
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
            sysctl_write: None,
            fdinfo_fd: None,
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
        // /proc/[pid]/fdinfo/<fd>: per-descriptor status (fdinfo(5)).
        if let (Some(pid), Some(fd)) = (self.pid, self.fdinfo_fd) {
            let content = pid::generate_fdinfo(pid, fd);
            self.cached_size.store(content.len() as u64, Ordering::Relaxed);
            return content;
        }

        // Dispatch per-PID files
        if let (Some(pid), Some(kind)) = (self.pid, self.pid_file_kind) {
            let content = match kind {
                PidFileKind::Status => pid::generate_status(pid),
                PidFileKind::Stat => pid::generate_stat(pid),
                PidFileKind::Cmdline => pid::generate_cmdline(pid),
                PidFileKind::Maps => pid::generate_maps(pid),
                PidFileKind::Smaps => pid::generate_smaps(pid),
                PidFileKind::Environ => pid::generate_environ(pid),
                PidFileKind::OomScore => pid::generate_oom_score(pid),
                PidFileKind::OomScoreAdj => pid::generate_oom_score_adj(pid),
                PidFileKind::Comm => pid::generate_comm(pid),
                PidFileKind::Cgroup => pid::generate_cgroup(pid),
                PidFileKind::Mounts => mounts::generate(),
                PidFileKind::AttrExec => pid::attr_label_get(pid, true),
                PidFileKind::AttrCurrent => pid::attr_label_get(pid, false),
                PidFileKind::AttrFscreate => pid::attr_get(pid, pid::ATTR_FSCREATE),
                PidFileKind::AttrSockcreate => pid::attr_get(pid, pid::ATTR_SOCKCREATE),
                PidFileKind::AttrPrev => pid::attr_get(pid, pid::ATTR_PREV),
                PidFileKind::AttrKeycreate => pid::attr_get(pid, pid::ATTR_KEYCREATE),
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
        // Check if it's a PID directory request on the root node
        if self.name == b"/" && pid::is_pid_dir(name) {
            if let Some(pid) = pid::parse_pid(name) {
                if pid::is_valid_pid(pid) {
                    // Create a virtual PID directory node
                    let pid_dir = ProcFSNode::new_dir(
                        name.to_vec(),
                        pid,
                    );
                    return Some(Arc::new(pid_dir));
                }
            }
            return None;
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
        children
            .iter()
            .map(|c| (c.name.clone(), c.node_type, c.ino))
            .collect()
    }

    /// Increment reference count
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement reference count.
    /// Returns the **previous** value (pre-decrement). Callers checking for
    /// last reference should compare against 1, not 0.
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
        self.create_dynamic_file("swaps", swaps::generate);
        self.create_dynamic_file("cpuinfo", cpuinfo::generate);
        self.create_dynamic_file("version", version::generate);
        self.create_dynamic_file("uptime", uptime::generate);
        self.create_dynamic_file("cmdline", cmdline::generate);
        // IKCONFIG-style embedded config (LTP tst_kconfig reads this).
        self.create_dynamic_file("config.gz", configgz::generate);
        self.create_dynamic_file("loadavg", loadavg::generate);
        self.create_dynamic_file("mounts", mounts::generate);
        self.create_dynamic_file("filesystems", mounts::generate_filesystems);
        self.create_dynamic_file("mountinfo", mounts::generate_mountinfo);
        self.create_dynamic_file("interrupts", interrupts::generate);
        // Per-CPU time accounting + system counters (Linux layout).
        self.create_dynamic_file("stat", stat::generate);
        // The kernel symbol table (Linux layout).
        self.create_dynamic_file("kallsyms", kallsyms::generate);
        // The held file locks (Linux layout).
        self.create_dynamic_file("locks", locks::generate);
        // The per-device I/O statistics (Linux layout).
        self.create_dynamic_file("diskstats", diskstats::generate);
        // The pending kernel timers (Linux layout).
        self.create_dynamic_file("timer_list", timer_list::generate);
        // Memory statistics (Linux layout).
        self.create_dynamic_file("vmstat", vmstat::generate);
        self.create_dynamic_file("buddyinfo", vmstat::generate_buddyinfo);
        // Scheduler statistics (Linux layout).
        self.create_dynamic_file("schedstat", schedstat::generate);
        // Softirq statistics (Linux layout).
        self.create_dynamic_file("softirqs", softirqs::generate);
        self.create_dynamic_file("partitions", partitions_generate);

        // U2 kmod: registered loadable modules (kernel/src/module).
        self.create_dynamic_file("modules", crate::module::generate_proc_modules);

        // Kernel log buffer
        self.create_dynamic_file("kmsg", crate::printk::generate_kmsg);

        // SysRq trigger (Linux: /proc/sysrq-trigger). Writing a command
        // character runs that command; the handler lives in
        // fs/procfs/sysrq.rs.
        {
            let ino = self.alloc_ino();
            let file = Arc::new(ProcFSNode::new_sysctl_file(
                b"sysrq-trigger".to_vec(),
                || alloc::vec![b'\n'],
                sysrq::write_trigger,
                ino,
            ));
            self.root_node.add_child(file);
        }

        // /proc/self - symlink to current process directory
        self.create_dynamic_symlink("self", self_proc::get_self_link);
        // /proc/thread-self — symlink to the current THREAD's directory
        // (libselinux procattr path, OH Phase 2).
        self.create_dynamic_symlink("thread-self", self_proc::get_thread_self_link);

        // P1: /proc/sys minimal tree (fs/procfs/sysctl.rs for the storage).
        self.init_sys_tree();

        // /proc/sysvipc — live SysV IPC object listings (LTP line counts).
        self.init_sysvipc_tree();

        // P1: /proc/net — network subsystem introspection (ss/netstat
        // compatible formats).
        self.init_net_tree();

        // /proc/[pid] directories are handled dynamically
    }

    /// Build the /proc/net tree (P1): dev, tcp, tcp6, udp, udp6, arp,
    /// route, sockstat — Linux-aligned formats.
    fn init_net_tree(&self) {
        let net_dir = Arc::new(ProcFSNode::new_dir(b"net".to_vec(), self.alloc_ino()));
        self.root_node.add_child(net_dir.clone());

        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"dev".to_vec(),
            net::generate_dev,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"tcp".to_vec(),
            net::generate_tcp,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"tcp6".to_vec(),
            net::generate_tcp6,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"udp".to_vec(),
            net::generate_udp,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"udp6".to_vec(),
            net::generate_udp6,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"arp".to_vec(),
            net::generate_arp,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"route".to_vec(),
            net::generate_route,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"sockstat".to_vec(),
            net::generate_sockstat,
            self.alloc_ino(),
        )));
        net_dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"unix".to_vec(),
            net::generate_unix,
            self.alloc_ino(),
        )));
    }

    /// Build the /proc/sys/{kernel,vm,fs} tree with writable sysctl files.
    fn init_sys_tree(&self) {
        let sys = Arc::new(ProcFSNode::new_dir(b"sys".to_vec(), self.alloc_ino()));
        self.root_node.add_child(sys.clone());

        let kernel = Arc::new(ProcFSNode::new_dir(b"kernel".to_vec(), self.alloc_ino()));
        sys.add_child(kernel.clone());
        kernel.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"hostname".to_vec(),
            sysctl::generate_hostname,
            sysctl::write_hostname,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"threads-max".to_vec(),
            sysctl::generate_threads_max,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"pid_max".to_vec(),
            sysctl::generate_pid_max,
            sysctl::write_pid_max,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"ostype".to_vec(),
            sysctl::generate_ostype,
            self.alloc_ino(),
        )));
        // Kernel taint flags — always 0 here; read by LTP's per-test
        // taint check (tst_taint, timerfd_settime02 and friends).
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"tainted".to_vec(),
            sysctl::generate_tainted,
            self.alloc_ino(),
        )));
        // panic_on_oops — the oops policy knob (seeded from oops=panic).
        kernel.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"panic_on_oops".to_vec(),
            sysctl::generate_panic_on_oops,
            sysctl::write_panic_on_oops,
            self.alloc_ino(),
        )));
        // Highest supported capability number (read by libcap-ng init()).
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"cap_last_cap".to_vec(),
            sysctl::generate_cap_last_cap,
            self.alloc_ino(),
        )));
        // Core dump file name pattern (consumed by process::coredump).
        kernel.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"core_pattern".to_vec(),
            sysctl::generate_core_pattern,
            sysctl::write_core_pattern,
            self.alloc_ino(),
        )));
        // SysV IPC message-queue limits (read by LTP msgstress01 and
        // ipcs(1); msgmni = queue-count limit, msgmax = largest message,
        // msgmnb = default queue bytes).
        kernel.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"msgmni".to_vec(),
            sysctl::generate_msgmni,
            sysctl::write_msgmni,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"msgmax".to_vec(),
            sysctl::generate_msgmax,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"msgmnb".to_vec(),
            sysctl::generate_msgmnb,
            self.alloc_ino(),
        )));
        // SysV shm limits — read by LTP shmget03 (shmmni) and shmctl03
        // (cross-checked against shmctl IPC_INFO values).
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"shmmax".to_vec(),
            sysctl::generate_shmmax,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"shmmni".to_vec(),
            sysctl::generate_shmmni,
            self.alloc_ino(),
        )));
        kernel.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"shmall".to_vec(),
            sysctl::generate_shmall,
            self.alloc_ino(),
        )));

        let vm = Arc::new(ProcFSNode::new_dir(b"vm".to_vec(), self.alloc_ino()));
        sys.add_child(vm.clone());
        vm.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"overcommit_memory".to_vec(),
            sysctl::generate_overcommit_memory,
            sysctl::write_overcommit_memory,
            self.alloc_ino(),
        )));

        let fsdir = Arc::new(ProcFSNode::new_dir(b"fs".to_vec(), self.alloc_ino()));
        sys.add_child(fsdir.clone());
        fsdir.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"file-max".to_vec(),
            sysctl::generate_file_max,
            sysctl::write_file_max,
            self.alloc_ino(),
        )));
        fsdir.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"pipe-user-pages-soft".to_vec(),
            sysctl::generate_pipe_user_pages_soft,
            sysctl::write_pipe_user_pages_soft,
            self.alloc_ino(),
        )));
        fsdir.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"pipe-user-pages-hard".to_vec(),
            sysctl::generate_pipe_user_pages_hard,
            sysctl::write_pipe_user_pages_hard,
            self.alloc_ino(),
        )));
        // F_SETPIPE_SZ ceiling (read by LTP fcntl30).
        fsdir.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"pipe-max-size".to_vec(),
            sysctl::generate_pipe_max_size,
            sysctl::write_pipe_max_size,
            self.alloc_ino(),
        )));
        // Lease break timeout (read+written by LTP fcntl33).
        fsdir.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"lease-break-time".to_vec(),
            sysctl::generate_lease_break_time,
            sysctl::write_lease_break_time,
            self.alloc_ino(),
        )));
        // POSIX mq limits — LTP mq_open01 reads+writes queues_max.
        let mqdir = Arc::new(ProcFSNode::new_dir(b"mqueue".to_vec(), self.alloc_ino()));
        fsdir.add_child(mqdir.clone());
        mqdir.add_child(Arc::new(ProcFSNode::new_sysctl_file(
            b"queues_max".to_vec(),
            sysctl::generate_mq_queues_max,
            sysctl::write_mq_queues_max,
            self.alloc_ino(),
        )));
        mqdir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"msg_max".to_vec(),
            sysctl::generate_mq_msg_max,
            self.alloc_ino(),
        )));
        mqdir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"msgsize_max".to_vec(),
            sysctl::generate_mq_msgsize_max,
            self.alloc_ino(),
        )));
    }

    /// /proc/sysvipc/{msg,sem,shm} — live SysV IPC object listings.
    /// LTP counts lines (GET_USED_QUEUES/SEGMENTS/ARRAYS) and ipcs(1)
    /// parses the columns.
    fn init_sysvipc_tree(&self) {
        let dir = Arc::new(ProcFSNode::new_dir(b"sysvipc".to_vec(), self.alloc_ino()));
        self.root_node.add_child(dir.clone());
        dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"msg".to_vec(),
            crate::ipc::sysv_msg::proc_show,
            self.alloc_ino(),
        )));
        dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"sem".to_vec(),
            crate::ipc::sysv_sem::proc_show,
            self.alloc_ino(),
        )));
        dir.add_child(Arc::new(ProcFSNode::new_dynamic_file(
            b"shm".to_vec(),
            crate::ipc::sysv_shm::proc_show,
            self.alloc_ino(),
        )));
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
                if let Some(pid) = pid::parse_pid(component_bytes) {
                    if pid::is_valid_pid(pid) {
                        let pid_dir = ProcFSNode::new_dir(
                            component_bytes.to_vec(),
                            pid,
                        );
                        current = Arc::new(pid_dir);
                        continue;
                    }
                }
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
                "smaps" => Some(pid::generate_smaps(pid)),
                "exe" => Some(pid::generate_exe_link(pid)),
                "cwd" => Some(pid::generate_cwd_link(pid)),
                "environ" => Some(pid::generate_environ(pid)),
                "oom_score" => Some(pid::generate_oom_score(pid)),
                "oom_score_adj" => Some(pid::generate_oom_score_adj(pid)),
                "comm" => Some(pid::generate_comm(pid)),
                "cgroup" => Some(pid::generate_cgroup(pid)),
                "mounts" => Some(mounts::generate()),
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

    // Match /proc/[pid]/attr/<file> (4 components)
    if components.len() == 4 && components[0] == "proc" && components[2] == "attr" {
        let first = if components[1] == "self" {
            alloc::format!("{}", current_pid())
        } else {
            alloc::string::String::from(components[1])
        };
        if let Some(pid) = pid::parse_pid(first.as_bytes()) {
            return match components[3] {
                "exec" => Some(pid::attr_label_get(pid, true)),
                "fscreate" => Some(pid::attr_get(pid, pid::ATTR_FSCREATE)),
                "current" => Some(pid::attr_label_get(pid, false)),
                "sockcreate" => Some(pid::attr_get(pid, pid::ATTR_SOCKCREATE)),
                "prev" => Some(pid::attr_get(pid, pid::ATTR_PREV)),
                "keycreate" => Some(pid::attr_get(pid, pid::ATTR_KEYCREATE)),
                _ => None,
            };
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
                "smaps" => Some(pid::generate_smaps(pid)),
                "exe" => Some(pid::generate_exe_link(pid)),
                "cwd" => Some(pid::generate_cwd_link(pid)),
                "environ" => Some(pid::generate_environ(pid)),
                "oom_score" => Some(pid::generate_oom_score(pid)),
                "oom_score_adj" => Some(pid::generate_oom_score_adj(pid)),
                "comm" => Some(pid::generate_comm(pid)),
                "cgroup" => Some(pid::generate_cgroup(pid)),
                "mounts" => Some(mounts::generate()),
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
            b"smaps" => PidFileKind::Smaps,
            b"environ" => PidFileKind::Environ,
            b"oom_score" => PidFileKind::OomScore,
            b"oom_score_adj" => PidFileKind::OomScoreAdj,
            b"comm" => PidFileKind::Comm,
            b"cgroup" => PidFileKind::Cgroup,
            b"mounts" => PidFileKind::Mounts,
            _ => {
                // /proc/[pid]/fd — the fd subdirectory (review 5.7: used to
                // exist only behind a syscall-layer string special case, so
                // generic VFS paths — fstatat, name_to_handle — missed it).
                if name == b"fd" {
                    return Ok(pid_val * 10000 + FD_DIR_INO_OFF);
                }
                // /proc/[pid]/fdinfo — per-descriptor status files.
                if name == b"fdinfo" {
                    return Ok(pid_val * 10000 + FDINFO_DIR_INO_OFF);
                }
                // /proc/[pid]/ns — the namespace magic-link directory (U1c).
                if name == b"ns" {
                    return Ok(pid_val * 10000 + NS_DIR_INO_OFF);
                }
                // /proc/[pid]/attr — SELinux label directory (OH Phase 2).
                if name == b"attr" {
                    return Ok(pid_val * 10000 + ATTR_DIR_INO_OFF);
                }
                // /proc/[pid]/task — per-thread directory (attr files are
                // addressed through /proc/self/task/<tid>/attr/...).
                if name == b"task" {
                    return Ok(pid_val * 10000 + TASK_DIR_INO_OFF);
                }
                // /proc/[pid]/fd/<N> — per-descriptor symlinks.
                if name.len() <= 3
                    && !name.is_empty()
                    && name.iter().all(|b| b.is_ascii_digit())
                {
                    let fd_num: u64 = name
                        .iter()
                        .fold(0u64, |acc, b| acc * 10 + (b - b'0') as u64);
                    if (fd_num as usize) < crate::fs::file::MAX_FDS {
                        return Ok(pid_val * 10000 + FD_LINK_INO_OFF + fd_num);
                    }
                }
                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
            }
        };
        // Use pid * stride + kind as inode number. Stride must exceed max kind value.
        let file_ino = pid_val * 10000 + (kind as u64) + 100;
        return Ok(file_ino);
    }

    // /proc/[pid]/attr directory (private_data None, ino = pid*10000+500):
    // resolve exec/current/fscreate inside it.
    if dir.private_data.is_none() && is_attr_dir(dir.ino) {
        let pid_val = dir.ino / 10000;
        let off = match name {
            b"exec" => 0u64,
            b"current" => 1,
            b"fscreate" => 2,
            b"sockcreate" => 3,
            b"prev" => 4,
            b"keycreate" => 5,
            _ => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        };
        return Ok(pid_val * 10000 + ATTR_FILE_INO_OFF + off);
    }

    // /proc/[pid]/task directory: a numeric entry is a thread ID; alias
    // the tid's own PID directory (threads live in the same pid space).
    if dir.private_data.is_none() && is_task_dir(dir.ino) {
        if let Some(tid) = pid::parse_pid(name) {
            if pid::is_valid_pid(tid) {
                return Ok(tid);
            }
        }
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // /proc/[pid]/fdinfo directory: a numeric entry is an fd of the
    // target task (content generated per fd, see generate_fdinfo).
    if dir.private_data.is_none() && is_fdinfo_dir(dir.ino) {
        let pid_val = dir.ino / 10000;
        if !name.is_empty() && name.iter().all(|b| b.is_ascii_digit()) {
            let fd_num: u64 = name
                .iter()
                .fold(0u64, |acc, b| acc * 10 + (b - b'0') as u64);
            if (fd_num as usize) < crate::fs::file::MAX_FDS {
                return Ok(pid_val * 10000 + FDINFO_FILE_INO_OFF + fd_num);
            }
        }
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
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

    // Synthetic fd-link symlink (/proc/[pid]/fd/N): stat as a symlink.
    if inode.private_data.is_none() {
        let pid_val = inode.ino / 10000;
        let rem = inode.ino % 10000;
        if pid::is_valid_pid(pid_val) && rem >= FD_LINK_INO_OFF && rem < FD_LINK_INO_OFF + 1024 {
            let target = pid::generate_fd_link(pid_val, (rem - FD_LINK_INO_OFF) as u32);
            stat.st_ino = inode.ino;
            stat.st_mode = InodeMode::S_IFLNK | 0o777;
            stat.st_size = target.len() as i64;
            stat.st_nlink = 1;
            stat.st_uid = 0;
            stat.st_gid = 0;
            stat.st_rdev = 0;
            stat.st_blksize = 4096;
            stat.st_blocks = (stat.st_size + 511) / 512;
            stat.st_atime = 0;
            stat.st_mtime = 0;
            stat.st_ctime = 0;
            return 0;
        }
        // Synthetic ns magic link (/proc/[pid]/ns/<kind>, U1c).
        if pid::is_valid_pid(pid_val)
            && rem >= NS_LINK_INO_OFF
            && rem < NS_LINK_INO_OFF + 6
        {
            let kind = crate::process::ns::NsKind::from_index(rem - NS_LINK_INO_OFF);
            let size = match kind {
                Some(k) => crate::process::ns::ns_link_target(pid_val as u32, k).len() as i64,
                None => 0,
            };
            stat.st_ino = inode.ino;
            stat.st_mode = InodeMode::S_IFLNK | 0o777;
            stat.st_size = size;
            stat.st_nlink = 1;
            stat.st_uid = 0;
            stat.st_gid = 0;
            stat.st_rdev = 0;
            stat.st_blksize = 4096;
            stat.st_blocks = (stat.st_size + 511) / 512;
            stat.st_atime = 0;
            stat.st_mtime = 0;
            stat.st_ctime = 0;
            return 0;
        }
    }

    // /proc/[pid]/attr label files: owned by the target task (see the
    // matching iget branch).
    if let Some(node_ptr) = inode.private_data {
        let node = &*(node_ptr as *const ProcFSNode);
        if matches!(
            node.pid_file_kind,
            Some(
            PidFileKind::AttrExec
                | PidFileKind::AttrCurrent
                | PidFileKind::AttrFscreate
                | PidFileKind::AttrSockcreate
                | PidFileKind::AttrKeycreate,
        )
        ) {
            if let Some(pid) = node.pid {
                let target = if crate::process::current_pid() as u64 == pid {
                    crate::process::current_task()
                } else {
                    crate::process::find_task_by_pid(pid as u32)
                };
                if let Some(t) = target {
                    // SAFETY: task pointer is valid.
                    let uid = unsafe { (*t).cred().uid };
                    stat.st_uid = uid;
                }
            }
        }
    }

    // /proc/[pid]/task directory: plain synthetic directory.
    if inode.private_data.is_none() && is_task_dir(inode.ino) {
        stat.st_ino = inode.ino;
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

    // /proc/[pid]/attr directory: plain synthetic directory.
    if inode.private_data.is_none() && is_attr_dir(inode.ino) {
        stat.st_ino = inode.ino;
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
    } else if node.name.as_slice() == b"sysrq-trigger" {
        // /proc/sysrq-trigger is write-only (0200, root; host-verified).
        InodeMode::S_IFREG | 0o200
    } else if node.sysctl_write.is_some()
        || node.pid_file_kind == Some(PidFileKind::OomScoreAdj)
        || matches!(
            node.pid_file_kind,
            Some(
            PidFileKind::AttrExec
                | PidFileKind::AttrCurrent
                | PidFileKind::AttrFscreate
                | PidFileKind::AttrSockcreate
                | PidFileKind::AttrKeycreate,
        )
        )
    {
        // P1 /proc/sys writable sysctl, /proc/[pid]/oom_score_adj and the
        // /proc/[pid]/attr label files (Linux 0644: owner-writable).
        InodeMode::S_IFREG | 0o644
    } else {
        InodeMode::S_IFREG | 0o444  // read-only file
    };
    // Avoid calling node.size() here — it regenerates full file content
    // (via content_generator) just to get the length, which is expensive
    // and can cause slab allocator issues in stat context.
    // Use a cached/approximate size instead.
    stat.st_size = node.cached_size.load(Ordering::Relaxed) as i64;
    stat.st_nlink = if node.is_dir() { 2 } else { 1 };
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
    // Synthetic fd-link inodes carry no ProcFSNode; the (pid, fd) pair is
    // encoded in the ino (see procfs_iget). Review 5.7: /proc/self/fd/N
    // used to resolve only through the syscall-layer string shortcut.
    if inode.private_data.is_none() {
        let pid_val = inode.ino / 10000;
        let rem = inode.ino % 10000;
        if pid::is_valid_pid(pid_val) && rem >= FD_LINK_INO_OFF && rem < FD_LINK_INO_OFF + 1024 {
            let fd = (rem - FD_LINK_INO_OFF) as u32;
            let target = pid::generate_fd_link(pid_val, fd);
            if target.is_empty() {
                return errno::Errno::NoSuchFileOrDirectory.as_neg_i32() as isize;
            }
            let len = target.len().min(buf.len());
            buf[..len].copy_from_slice(&target[..len]);
            return len as isize;
        }
        // Synthetic ns magic link (/proc/[pid]/ns/<kind>, U1c): readlink
        // yields "<kind>:[<inum>]".
        if pid::is_valid_pid(pid_val) && rem >= NS_LINK_INO_OFF && rem < NS_LINK_INO_OFF + 6 {
            if let Some(kind) = crate::process::ns::NsKind::from_index(rem - NS_LINK_INO_OFF) {
                let target = crate::process::ns::ns_link_target(pid_val as u32, kind);
                let len = target.len().min(buf.len());
                buf[..len].copy_from_slice(&target[..len]);
                return len as isize;
            }
        }
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }

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
            b"smaps" => PidFileKind::Smaps,
            b"environ" => PidFileKind::Environ,
            b"oom_score" => PidFileKind::OomScore,
            b"oom_score_adj" => PidFileKind::OomScoreAdj,
            b"comm" => PidFileKind::Comm,
            b"cgroup" => PidFileKind::Cgroup,
            _ => {
                // /proc/[pid]/fd directory (review 5.7): a synthetic
                // directory whose readdir lists the target's open fds —
                // reuses the VFS-layer PROCFS_DIR_OPS (private_data = pid).
                if name == b"fd" && ino == pid_val * 10000 + FD_DIR_INO_OFF {
                    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o555));
                    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                    inode.ops = Some(&crate::fs::vfs::PROCFS_DIR_OPS);
                    inode.private_data = Some(pid_val as usize as *mut u8);
                    return Ok(Arc::new(inode));
                }
                // /proc/[pid]/attr directory: synthetic SELinux label
                // directory (exec/current/fscreate live at
                // pid*10000+ATTR_FILE_INO_OFF+N, dispatched through
                // PROCFS_INODE_OPS with a ProcFSNode like other pid files).
                if name == b"attr" && ino == pid_val * 10000 + ATTR_DIR_INO_OFF {
                    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o555));
                    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                    inode.ops = Some(&PROCFS_INODE_OPS);
                    // private_data None + is_attr_dir(ino) drives lookups.
                    return Ok(Arc::new(inode));
                }
                // /proc/[pid]/task directory: per-thread subdirectory.
                // MUST be served here: this first iget branch returns for
                // EVERY pid-dir child name, so a "task" case further down
                // the function is dead code — /proc/<pid>/task then never
                // materialized and every open answered ENOENT. That killed
                // hdf_devmgr (spawns an OS_IPC thread, opens
                // /proc/self/task/<tid>/comm, exits 1) and with it the
                // whole HDF host chain — composer included.
                if name == b"task" && ino == pid_val * 10000 + TASK_DIR_INO_OFF {
                    let mut inode =
                        Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o555));
                    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                    inode.ops = Some(&PROCFS_INODE_OPS);
                    // private_data None + is_task_dir(ino) drives lookups
                    // and readdir below.
                    return Ok(Arc::new(inode));
                }
                // /proc/[pid]/fdinfo directory: per-descriptor status
                // files; private_data None + is_fdinfo_dir(ino) drives
                // lookups/readdir below.
                if name == b"fdinfo" && ino == pid_val * 10000 + FDINFO_DIR_INO_OFF {
                    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o555));
                    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                    inode.ops = Some(&PROCFS_INODE_OPS);
                    return Ok(Arc::new(inode));
                }
                // /proc/[pid]/ns directory (U1c): synthetic directory whose
                // children are the six namespace magic links; lookups and
                // readdir run through PROCFS_NS_DIR_OPS.
                if name == b"ns" && ino == pid_val * 10000 + NS_DIR_INO_OFF {
                    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o555));
                    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                    inode.ops = Some(&PROCFS_NS_DIR_OPS);
                    inode.private_data = Some(pid_val as usize as *mut u8);
                    return Ok(Arc::new(inode));
                }
                // /proc/[pid]/fd/<N> symlink: readlink resolves through
                // pid::generate_fd_link (decoded from the ino).
                if ino >= pid_val * 10000 + FD_LINK_INO_OFF
                    && ino < pid_val * 10000 + FD_LINK_INO_OFF + 1024
                {
                    let mut inode =
                        Inode::new(ino, InodeMode::new(InodeMode::S_IFLNK | 0o777));
                    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                    inode.ops = Some(&PROCFS_INODE_OPS);
                    // private_data None + ino encodes (pid, fd); handled by
                    // procfs_readlink below.
                    return Ok(Arc::new(inode));
                }
                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
            }
        };

        let is_symlink = matches!(kind, PidFileKind::Exe | PidFileKind::Cwd);
        let mode = if is_symlink {
            InodeMode::new(InodeMode::S_IFLNK | 0o777)
        } else if kind == PidFileKind::OomScoreAdj {
            // Writable per-task knob (Linux 0644).
            InodeMode::new(InodeMode::S_IFREG | 0o644)
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
            sysctl_write: None,
            fdinfo_fd: None,
        });
        let raw_ptr = Arc::into_raw(proc_node) as *mut u8;

        let mut inode = Inode::new(ino, mode);
    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;  // icache isolation (VFS-H8)
        inode.ops = Some(&PROCFS_INODE_OPS);
        inode.private_data = Some(raw_ptr);
        return Ok(Arc::new(inode));
    }

    // Parent is the /proc/[pid]/task directory: a numeric name is a
    // thread ID — alias the tid's own PID directory (threads share the
    // pid space), so /proc/<tgid>/task/<tid>/{comm,status,attr/...}
    // resolve through the regular pid-dir machinery in the first branch
    // of this function. Without this branch the generic
    // `private_data.ok_or(ENOTDIR)` below ate every task-dir child.
    if parent.private_data.is_none() && is_task_dir(parent.ino) {
        if let Some(tid) = pid::parse_pid(name) {
            if pid::is_valid_pid(tid) {
                let mode = InodeMode::new(InodeMode::S_IFDIR | 0o555);
                let mut inode = Inode::new(tid, mode);
                inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
                inode.ops = Some(&PROCFS_INODE_OPS);
                return Ok(Arc::new(inode));
            }
        }
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }

    // Parent is the /proc/[pid]/attr directory: build the label file.
    if parent.private_data.is_none() && is_attr_dir(parent.ino) {
        let Some((_, which)) = attr_file_kind_of(ino) else {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        };
        let kind = match which {
            0 => PidFileKind::AttrExec,
            1 => PidFileKind::AttrCurrent,
            2 => PidFileKind::AttrFscreate,
            3 => PidFileKind::AttrSockcreate,
            4 => PidFileKind::AttrPrev,
            _ => PidFileKind::AttrKeycreate,
        };
        let proc_node = Arc::new(ProcFSNode {
            name: name.to_vec(),
            node_type: ProcFSType::RegularFile,
            content_generator: None,
            static_content: None,
            link_generator: None,
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(0),
            pid: Some(parent.ino / 10000),
            pid_file_kind: Some(kind),
            sysctl_write: None,
            fdinfo_fd: None,
        });
        let raw_ptr = Arc::into_raw(proc_node) as *mut u8;
        let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o644));
        inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
        inode.ops = Some(&PROCFS_INODE_OPS);
        inode.private_data = Some(raw_ptr);
        // Owned by the task itself (Linux procfs pid attr files): the
        // service child setuid()s to its final uid BEFORE writing its
        // exec label — a root-owned 0644 node answered EACCES and init
        // aborted the service (INIT_EEXEC_CONTENT).
        let target = if crate::process::current_pid() as u64 == parent.ino / 10000 {
            crate::process::current_task()
        } else {
            crate::process::find_task_by_pid((parent.ino / 10000) as u32)
        };
        if let Some(t) = target {
            // SAFETY: find_task_by_pid/current_task return a valid task.
            let uid = unsafe { (*t).cred().uid };
            inode.uid.store(uid, core::sync::atomic::Ordering::Release);
        }
        return Ok(Arc::new(inode));
    }

    // Parent is the /proc/[pid]/fdinfo directory: build the per-fd status
    // file. The fd is encoded in the ino (fdinfo_file_of); content is
    // generated on read through the ProcFSNode's fdinfo_fd.
    if parent.private_data.is_none() && is_fdinfo_dir(parent.ino) {
        let Some((_, fd)) = fdinfo_file_of(ino) else {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        };
        let proc_node = Arc::new(ProcFSNode {
            name: name.to_vec(),
            node_type: ProcFSType::RegularFile,
            content_generator: None,
            static_content: None,
            link_generator: None,
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
            cached_size: AtomicU64::new(0),
            pid: Some(parent.ino / 10000),
            pid_file_kind: None,
            sysctl_write: None,
            fdinfo_fd: Some(fd),
        });
        let raw_ptr = Arc::into_raw(proc_node) as *mut u8;
        let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o444));
        inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
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
    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;  // icache isolation (VFS-H8)
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
    } else if child.sysctl_write.is_some() || child.pid_file_kind == Some(PidFileKind::OomScoreAdj) {
        // P1 /proc/sys writable sysctl (and oom_score_adj) open for root.
        InodeMode::new(InodeMode::S_IFREG | 0o644)
    } else {
        InodeMode::new(InodeMode::S_IFREG | 0o444)
    };

    let mut inode = Inode::new(child.ino, mode);
    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;  // icache isolation (VFS-H8)
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

/// ProcFS file write operation: /proc/sys sysctl files (P1) and
/// /proc/[pid]/oom_score_adj. Everything else stays EINVAL — procfs has no
/// other writable nodes.
fn procfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: inode is written once at open time; read-only access here.
    let inode_opt = unsafe { (*file.inode.get()).clone() };
    let inode = match inode_opt {
        Some(i) => i,
        None => return -22, // EINVAL
    };
    // procfs file inodes carry a ProcFSNode pointer in private_data.
    let node_ptr = match inode.private_data {
        Some(p) => p,
        None => return -22,
    };
    let node = unsafe { &*(node_ptr as *const ProcFSNode) };

    // /proc/[pid]/oom_score_adj: store the adjustment on the task (Linux
    // semantics: decimal i32 in [-1000, 1000]; out-of-range/undecodable is
    // EINVAL). LTP's tst_enable_oom_protection writes -1000 in setup and
    // TWARNs on failure — that warning turned every mem test's exit code
    // into rc=4.
    if node.pid_file_kind == Some(PidFileKind::OomScoreAdj) {
        let pid = match node.pid {
            Some(p) => p,
            None => return -22, // EINVAL
        };
        let trimmed = core::str::from_utf8(buf).unwrap_or("").trim();
        // Whitespace-only buffer (the guest toybox `echo v > file` writes
        // the value and the trailing "\n" as SEPARATE write() calls):
        // accept as a no-op. Linux would EINVAL a lone "\n", but the split
        // write is this guest's stdio behavior, not a test's intent.
        if trimmed.is_empty() {
            return buf.len() as isize;
        }
        let adj = match trimmed.parse::<i32>() {
            Ok(v) => v,
            Err(_) => return -22, // EINVAL (Linux: kstrtoint failure)
        };
        if !(-1000..=1000).contains(&adj) {
            return -22; // EINVAL (Linux: OOM_SCORE_ADJ_MIN/MAX)
        }
        let task = if crate::process::current_pid() as u64 == pid {
            crate::process::current_task()
        } else {
            crate::process::find_task_by_pid(pid as u32)
        };
        return match task {
            Some(t) => {
                t.set_oom_score_adj(adj);
                buf.len() as isize
            }
            None => -3, // ESRCH: target task gone
        };
    }

    // /proc/[pid]/attr/{exec,current,fscreate}: store the SELinux label
    // (no enforcement — see pid::attr_label_set). libselinux writes the
    // context with a trailing NUL; accept the whole buffer.
    if matches!(
        node.pid_file_kind,
        Some(
            PidFileKind::AttrExec
                | PidFileKind::AttrCurrent
                | PidFileKind::AttrFscreate
                | PidFileKind::AttrSockcreate
                | PidFileKind::AttrKeycreate,
        )
    ) {
        let pid = node.pid.unwrap_or(0);
        let kind = match node.pid_file_kind {
            Some(PidFileKind::AttrExec) => pid::ATTR_EXEC,
            Some(PidFileKind::AttrCurrent) => pid::ATTR_CURRENT,
            Some(PidFileKind::AttrFscreate) => pid::ATTR_FSCREATE,
            Some(PidFileKind::AttrSockcreate) => pid::ATTR_SOCKCREATE,
            Some(PidFileKind::AttrKeycreate) => pid::ATTR_KEYCREATE,
            _ => pid::ATTR_EXEC,
        };
        pid::attr_set(pid, kind, buf);
        return buf.len() as isize;
    }

    match node.sysctl_write {
        Some(write_fn) => {
            let ret = write_fn(buf);
            if ret != 0 {
                ret as isize
            } else {
                buf.len() as isize
            }
        }
        None => -22, // EINVAL — read-only procfs node
    }
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

/// ProcFS setattr: ATTR_SIZE is a no-op (O_TRUNC on `echo x > file` must
/// not fail; procfs files have no persistent size). Other attributes
/// (chmod/chown/truncate-to-value) are EPERM — the tree is kernel-owned.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_setattr(_inode: &Inode, attr: u32, _arg1: u64, _arg2: u64) -> i32 {
    if attr == crate::fs::inode::setattr_attr::ATTR_SIZE {
        return 0;
    }
    errno::Errno::OperationNotPermitted.as_neg_i32()
}

/// ProcFS file operations table
pub static PROCFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(procfs_file_read),
    write: Some(procfs_file_write),
    lseek: Some(procfs_file_lseek),
    close: Some(procfs_file_close),
    poll: None,
    read_at: None,
};

/// ProcFS get_file_ops: return ops based on inode type
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn procfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        // /proc/kmsg is a BLOCKING STREAM, not a snapshot dynamic file:
        // hilogd.rd_kmsg read-EOF-looped at 60-92K/s on the snapshot
        // (perf-audit pa9/pa10). Identified by its generator fn pointer
        // — the node is created with create_dynamic_file("kmsg",
        // printk::generate_kmsg); procfs_open applies the matching
        // special case (single-reader gate, no snapshot Box).
        let node = inode
            .private_data
            .map(|p| &*(p as *const ProcFSNode));
        if let Some(node) = node {
            if node.content_generator == Some(crate::printk::generate_kmsg) {
                return Some(&crate::printk::PROC_KMSG_OPS);
            }
        }
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

    // /proc/sysrq-trigger: a write-only node (mode 0200) — an
    // open for read is refused, as on the live host.
    if node.name.as_slice() == b"sysrq-trigger" && !file.flags().is_writeonly() {
        return errno::Errno::PermissionDenied.as_neg_i32();
    }

    // /proc/kmsg stream: single-reader gate (second concurrent open
    // EBUSY, Linux fs/proc/kmsg.c), NO content snapshot — reads consume
    // the shared printk ring cursor (see PROC_KMSG_OPS / pa9-pa10).
    if node.content_generator == Some(crate::printk::generate_kmsg) {
        return crate::printk::proc_kmsg_open();
    }

    // ptrace_may_access gate for /proc/[pid]/environ (review 5.7: environ
    // 无 ptrace 权限): self or CAP_SYS_PTRACE. (The syscall-layer /proc
    // shortcut applies the same check before reaching here.)
    if let (Some(pid), Some(PidFileKind::Environ)) = (node.pid, node.pid_file_kind) {
        if !pid::environ_access_allowed(pid) {
            return errno::Errno::PermissionDenied.as_neg_i32();
        }
    }

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
    (b"mounts", file_type::DT_REG),
        (b"stat", file_type::DT_REG),
        (b"maps", file_type::DT_REG),
        (b"environ", file_type::DT_REG),
        (b"comm", file_type::DT_REG),
        (b"cgroup", file_type::DT_REG),
        (b"exe", file_type::DT_LNK),
        (b"cwd", file_type::DT_LNK),
        (b"fd", file_type::DT_DIR),
        (b"fdinfo", file_type::DT_DIR),
        (b"ns", file_type::DT_DIR),
        (b"attr", file_type::DT_DIR),
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

    // /proc/[pid]/task directory: every live thread of the group (Linux
    // lists all tids including the leader). Collect raw tids under the
    // pid-hash bucket locks, format names after the walk — no
    // allocation while holding the bucket spinlocks.
    if inode.private_data.is_none() && is_task_dir(inode.ino) {
        use crate::fs::inode::file_type;
        let pid_val = (inode.ino / 10000) as u32;
        let mut tids: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        crate::process::pid_hash::pid_hash_for_each_task(|t| unsafe {
            let t = &*t;
            if t.tgid() == pid_val {
                tids.push(t.pid() as u64);
            }
        });
        tids.sort_unstable();
        let entries = tids
            .into_iter()
            .map(|tid| crate::fs::inode::VfsDirEntry {
                ino: tid,
                name: alloc::format!("{}", tid).into_bytes(),
                file_type: file_type::DT_DIR,
            })
            .collect();
        return Some(entries);
    }

    // /proc/[pid]/fdinfo directory: one status file per open fd.
    if inode.private_data.is_none() && is_fdinfo_dir(inode.ino) {
        use crate::fs::inode::file_type;
        let pid_val = inode.ino / 10000;
        let entries = crate::fs::procfs::pid::list_fds(pid_val)
            .into_iter()
            .map(|(fd, _)| crate::fs::inode::VfsDirEntry {
                ino: pid_val * 10000 + FDINFO_FILE_INO_OFF + fd as u64,
                name: alloc::format!("{}", fd).into_bytes(),
                file_type: file_type::DT_REG,
            })
            .collect();
        return Some(entries);
    }

    // /proc/[pid]/attr directory: three label files.
    if inode.private_data.is_none() && is_attr_dir(inode.ino) {
        use crate::fs::inode::file_type;
        let pid_val = inode.ino / 10000;
        let mut entries = alloc::vec::Vec::new();
        for (i, name) in [
            b"exec" as &[u8],
            b"current",
            b"fscreate",
            b"sockcreate",
            b"prev",
            b"keycreate",
        ]
        .iter()
        .enumerate()
        {
            entries.push(crate::fs::inode::VfsDirEntry {
                ino: pid_val * 10000 + ATTR_FILE_INO_OFF + i as u64,
                name: name.to_vec(),
                file_type: file_type::DT_REG,
            });
        }
        return Some(entries);
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

    // For procfs root (ino == 1), also list all active PID directories.
    if inode.ino == 1 {
        use crate::process::pid_hash;
        let (pids, count, _truncated) = pid_hash::pid_hash_collect_all();
        for i in 0..count {
            // Linux-exact /proc root listing: pid 0 (the per-CPU idle
            // tasks / swapper) NEVER appears — Linux keeps idle tasks out
            // of the PID allocator, so no dirent is generated for them.
            // Rux pre-creates MAX_CPUS idle Tasks and hash-inserts them
            // all with pid 0 (sched::init), which used to surface one
            // "0" entry per pre-created idle task here — four on every
            // boot, regardless of the online CPU count. /proc/0 LOOKUP
            // already refused them (pid::is_valid_pid); readdir now
            // matches that.
            if pids[i] == 0 {
                continue;
            }
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
    setattr: Some(procfs_setattr), // P1: ATTR_SIZE no-op (O_TRUNC tolerance)
    iget: Some(procfs_iget),
    destroy_inode: None,
};

// ============================================================================
// /proc/[pid]/ns — namespace magic links (U1c)
// ============================================================================

/// Lookup inside the ns directory: "<kind>" → ns-link ino.
/// SAFETY: VFS callback contract; private_data stores the PID.
unsafe fn procfs_ns_dir_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let pid_val = match dir.private_data {
        Some(p) => p as usize as u64,
        None => return Err(errno::Errno::NotADirectory.as_neg_i32()),
    };
    match crate::process::ns::NsKind::from_name(name) {
        Some(kind) => Ok(pid_val * 10000 + NS_LINK_INO_OFF + kind.index()),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

/// Instantiate the ns-link inode: a symlink whose (pid, kind) pair is
/// encoded in the ino (readlink/getattr dispatch on that range).
/// SAFETY: VFS callback contract.
unsafe fn procfs_ns_dir_iget(
    _parent: &Inode,
    _name: &[u8],
    ino: Ino,
) -> Result<Arc<Inode>, i32> {
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFLNK | 0o777));
    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
    inode.ops = Some(&PROCFS_INODE_OPS);
    // private_data None + ino encodes (pid, kind); handled by
    // procfs_readlink / procfs_getattr.
    Ok(Arc::new(inode))
}

/// Readdir of the ns directory: ".", ".." and the six kind links.
/// SAFETY: VFS callback contract; private_data stores the PID.
unsafe fn procfs_ns_dir_readdir(
    inode: &Inode,
) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;
    let pid_val = inode.private_data? as usize as u64;
    let mut entries = alloc::vec::Vec::new();
    entries.push(crate::fs::inode::VfsDirEntry {
        ino: inode.ino,
        name: alloc::vec![b'.'],
        file_type: file_type::DT_DIR,
    });
    entries.push(crate::fs::inode::VfsDirEntry {
        ino: 1,
        name: alloc::vec![b'.', b'.'],
        file_type: file_type::DT_DIR,
    });
    for kind in crate::process::ns::NsKind::ALL {
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: pid_val * 10000 + NS_LINK_INO_OFF + kind.index(),
            name: kind.name().as_bytes().to_vec(),
            file_type: file_type::DT_LNK,
        });
    }
    Some(entries)
}

/// Getattr of the ns directory itself.
/// SAFETY: VFS callback contract.
unsafe fn procfs_ns_dir_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    stat.st_ino = inode.ino;
    stat.st_mode = InodeMode::S_IFDIR | 0o555;
    stat.st_size = 0;
    stat.st_nlink = 2;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = 0;
    stat.st_atime = 0;
    stat.st_mtime = 0;
    stat.st_ctime = 0;
    0
}

/// Synthetic INodeOps for the /proc/[pid]/ns directory (U1c).
/// Lookup inside /proc/[pid]/fd: a numeric name resolves to the fd's
/// magic-link ino (pid * 10000 + FD_LINK_INO_OFF + fd). musl's
/// fchmodat(AT_SYMLINK_NOFOLLOW) fallback stats "/proc/self/fd/<n>" by
/// PATH — with lookup: None the walk failed ENOTDIR and every
/// socket-owning OH service child died at its socket chmod.
/// SAFETY: VFS callback contract; private_data stores the PID.
pub unsafe fn procfs_fd_dir_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let pid_val = match dir.private_data {
        Some(p) => p as usize as u64,
        None => return Err(errno::Errno::NotADirectory.as_neg_i32()),
    };
    if name.is_empty() || name.len() > 3 || !name.iter().all(|b| b.is_ascii_digit()) {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }
    let fd: u64 = name.iter().fold(0u64, |acc, b| acc * 10 + (b - b'0') as u64);
    // Closed (or out-of-range) fds are ENOENT, like Linux.
    let open = crate::process::find_task_by_pid(pid_val as u32)
        .or_else(|| {
            use crate::process::{current_pid, current_task};
            if current_pid() as u64 == pid_val { current_task() } else { None }
        })
        .and_then(|t| t.try_fdtable())
        .map(|ft| ft.get_file(fd as usize).is_some())
        .unwrap_or(false);
    if !open {
        return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    }
    Ok(pid_val * 10000 + FD_LINK_INO_OFF + fd)
}

/// Instantiate the fd magic-link inode (symlink; readlink handled by
/// procfs_readlink's FD_LINK branch).
/// SAFETY: VFS callback contract.
pub unsafe fn procfs_fd_dir_iget(
    _parent: &Inode,
    _name: &[u8],
    ino: Ino,
) -> Result<Arc<Inode>, i32> {
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFLNK | 0o777));
    inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
    inode.ops = Some(&PROCFS_INODE_OPS);
    Ok(Arc::new(inode))
}

static PROCFS_NS_DIR_OPS: INodeOps = INodeOps {
    lookup: Some(procfs_ns_dir_lookup),
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None, // handled on the ns-link inodes (PROCFS_INODE_OPS)
    get_file_ops: None,
    readdir: Some(procfs_ns_dir_readdir),
    open: None,
    permission: None,
    getattr: Some(procfs_ns_dir_getattr),
    setattr: None,
    iget: Some(procfs_ns_dir_iget),
    destroy_inode: None,
};




/// /proc/partitions — major minor #blocks name rows. Lists the root disk
/// plus any bound loop devices (LTP tst_acquire_device's fallback probe
/// and its "No free devices found" report read this).
fn partitions_generate() -> alloc::vec::Vec<u8> {
    let mut out = alloc::string::String::from("major minor  #blocks  name\n\n");
    if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
        // SAFETY: get_pci_gen_disk returns a valid GenDisk pointer.
        let cap = unsafe { (*disk).capacity.load(core::sync::atomic::Ordering::Acquire) };
        out.push_str(&alloc::format!(
            " 254     0 {:>10} vda\n",
            cap / 2 // sectors -> KiB blocks
        ));
    }
    out.push_str(&alloc::string::String::from_utf8_lossy(
        &crate::drivers::loop_dev::partitions_lines(),
    ));
    out.into_bytes()
}
