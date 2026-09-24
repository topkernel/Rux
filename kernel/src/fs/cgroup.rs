//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! cgroupfs — the "cgroup2" filesystem (cgroup v2 unified hierarchy)
//!
//! Mounted at /sys/fs/cgroup. Every `CgroupNode` in the hierarchy (see
//! `sched::cgroup`) is presented as a directory carrying the standard
//! control files:
//!
//! ```text
//! /sys/fs/cgroup/
//! ├── cgroup.controllers     - available controllers ("cpu memory pids")
//! ├── cgroup.subtree_control - controllers enabled for children (rw)
//! ├── cgroup.procs           - member PID list (rw: echo pid migrates)
//! ├── cgroup.events          - "populated 0|1"
//! ├── cgroup.type            - "domain"
//! ├── cpu.max                - "max 100000" | "<quota_us> <period_us>" (rw)
//! ├── cpu.stat               - usage/throttle counters
//! ├── memory.max             - byte limit or "max" (rw)
//! ├── memory.current         - current charge (bytes)
//! ├── pids.max               - task limit or "max" (rw)
//! ├── pids.current           - current task count
//! ├── tasks                  - cgroup v1 compat alias of cgroup.procs (rw)
//! └── <child>/               - mkdir creates a child cgroup
//! ```
//!
//! Units: `cpu.max` uses Linux's MICROsecond file ABI (systemd writes
//! µs); internally the controller stores nanoseconds
//! (CgroupControllers::cpu_quota_ns / cpu_period_ns).
//!
//! systemd compatibility: the root directory must be writable, and
//! `cgroup.controllers` must advertise at least "cpu" and "memory" —
//! systemd's cgroups-v2 probe fails fatally otherwise.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::errno;
use crate::fs::inode::file_type;
use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps, VfsDirEntry};
use crate::fs::superblock::{FileSystemType, SuperBlock};
use crate::sched::cgroup::{self, CgroupNode, LIMIT_MAX};

// ============================================================================
// Control files
// ============================================================================

/// Which control file a cgroupfs inode represents. The discriminant is the
/// inode-number offset from the node's `ino_base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupFileKind {
    /// placeholder 0 — directories carry no kind
    Dir = 0,
    CgroupProcs = 1,
    CgroupControllers = 2,
    CgroupSubtreeControl = 3,
    CgroupEvents = 4,
    CgroupType = 5,
    CpuMax = 6,
    CpuStat = 7,
    MemoryMax = 8,
    MemoryCurrent = 9,
    PidsMax = 10,
    PidsCurrent = 11,
    /// cgroup v1 compat alias for cgroup.procs
    Tasks = 12,
}

/// (name, kind, writable) for every control file, readdir order.
const CONTROL_FILES: [(&str, CgroupFileKind, bool); 12] = [
    ("cgroup.procs", CgroupFileKind::CgroupProcs, true),
    ("cgroup.controllers", CgroupFileKind::CgroupControllers, false),
    ("cgroup.subtree_control", CgroupFileKind::CgroupSubtreeControl, true),
    ("cgroup.events", CgroupFileKind::CgroupEvents, false),
    ("cgroup.type", CgroupFileKind::CgroupType, false),
    ("cpu.max", CgroupFileKind::CpuMax, true),
    ("cpu.stat", CgroupFileKind::CpuStat, false),
    ("memory.max", CgroupFileKind::MemoryMax, true),
    ("memory.current", CgroupFileKind::MemoryCurrent, false),
    ("pids.max", CgroupFileKind::PidsMax, true),
    ("pids.current", CgroupFileKind::PidsCurrent, false),
    ("tasks", CgroupFileKind::Tasks, true),
];

/// Resolve a control-file name inside a cgroup directory.
fn control_file_by_name(name: &[u8]) -> Option<CgroupFileKind> {
    let s = core::str::from_utf8(name).ok()?;
    CONTROL_FILES
        .iter()
        .find(|(n, _, _)| *n == s)
        .map(|(_, k, _)| *k)
}

/// Per-inode private data: which cgroup + which control file.
struct CgroupInoData {
    node: Arc<CgroupNode>,
    kind: CgroupFileKind,
}

/// Snapshot a node's Arc out of the hierarchy by matching an inode base.
///
/// The inode number of a cgroup directory is its `ino_base`; control files
/// are `ino_base + kind`. We keep the Arc alive in leaked CgroupInoData
/// structures (see `cgroupfs_iget`), and lookup/mkdir hand out fresh ones.
fn ino_of_dir(node: &CgroupNode) -> u64 {
    node.ino_base
}

fn ino_of_file(node: &CgroupNode, kind: CgroupFileKind) -> u64 {
    node.ino_base + kind as u64
}

// ============================================================================
// Content generation (reads)
// ============================================================================

fn generate_procs(node: &CgroupNode) -> Vec<u8> {
    let mut s = String::new();
    for &pid in node.tasks.lock_irqsave().iter() {
        s.push_str(&format!("{}\n", pid));
    }
    s.into_bytes()
}

fn generate_controllers() -> Vec<u8> {
    let mut s = String::new();
    for name in cgroup::CONTROLLER_NAMES {
        s.push_str(name);
        s.push(' ');
    }
    s.pop(); // trailing space → newline below
    s.push('\n');
    s.into_bytes()
}

fn generate_subtree_control(node: &CgroupNode) -> Vec<u8> {
    let bits = node.controllers.lock_irqsave().subtree_control;
    let mut s = cgroup::format_subtree_control(bits);
    s.push('\n');
    s.into_bytes()
}

fn generate_events(node: &CgroupNode) -> Vec<u8> {
    format!("populated {}\n", if node.populated() { 1 } else { 0 }).into_bytes()
}

fn generate_type() -> Vec<u8> {
    String::from("domain\n").into_bytes()
}

fn generate_cpu_max(node: &CgroupNode) -> Vec<u8> {
    let controllers = node.controllers.lock_irqsave();
    let quota = controllers.cpu_quota_ns.load(Ordering::Acquire);
    let period = controllers.cpu_period_ns.load(Ordering::Acquire);
    // File ABI is µs (Linux); internal storage is ns.
    let period_us = period / 1000;
    let s = if quota == LIMIT_MAX {
        format!("max {}\n", period_us)
    } else {
        format!("{} {}\n", quota / 1000, period_us)
    };
    s.into_bytes()
}

fn generate_cpu_stat(node: &CgroupNode) -> Vec<u8> {
    let usage_us = node.cpu_usage_total_ns.load(Ordering::Acquire) / 1000;
    let nr_throttled = node.nr_throttled.load(Ordering::Acquire);
    let throttled_us = node.throttled_ns.load(Ordering::Acquire) / 1000;
    format!(
        "usage_usec {}\nnr_periods 0\nnr_throttled {}\nthrottled_usec {}\n",
        usage_us, nr_throttled, throttled_us
    )
    .into_bytes()
}

fn generate_memory_max(node: &CgroupNode) -> Vec<u8> {
    let max = node
        .controllers
        .lock_irqsave()
        .memory_max_bytes
        .load(Ordering::Acquire);
    if max == LIMIT_MAX {
        String::from("max\n").into_bytes()
    } else {
        format!("{}\n", max).into_bytes()
    }
}

fn generate_memory_current(node: &CgroupNode) -> Vec<u8> {
    format!("{}\n", cgroup::node_memory_current(node)).into_bytes()
}

fn generate_pids_max(node: &CgroupNode) -> Vec<u8> {
    let max = node.controllers.lock_irqsave().pids_max.load(Ordering::Acquire);
    if max == LIMIT_MAX {
        String::from("max\n").into_bytes()
    } else {
        format!("{}\n", max).into_bytes()
    }
}

fn generate_pids_current(node: &CgroupNode) -> Vec<u8> {
    format!("{}\n", node.subtree_task_count()).into_bytes()
}

fn generate_content(node: &CgroupNode, kind: CgroupFileKind) -> Vec<u8> {
    match kind {
        CgroupFileKind::Dir => Vec::new(),
        CgroupFileKind::CgroupProcs | CgroupFileKind::Tasks => generate_procs(node),
        CgroupFileKind::CgroupControllers => generate_controllers(),
        CgroupFileKind::CgroupSubtreeControl => generate_subtree_control(node),
        CgroupFileKind::CgroupEvents => generate_events(node),
        CgroupFileKind::CgroupType => generate_type(),
        CgroupFileKind::CpuMax => generate_cpu_max(node),
        CgroupFileKind::CpuStat => generate_cpu_stat(node),
        CgroupFileKind::MemoryMax => generate_memory_max(node),
        CgroupFileKind::MemoryCurrent => generate_memory_current(node),
        CgroupFileKind::PidsMax => generate_pids_max(node),
        CgroupFileKind::PidsCurrent => generate_pids_current(node),
    }
}

// ============================================================================
// Content application (writes)
// ============================================================================

fn trim(input: &[u8]) -> &str {
    let s = core::str::from_utf8(input).unwrap_or("");
    s.trim()
}

fn apply_procs(node: &Arc<CgroupNode>, input: &str) -> i32 {
    match input.parse::<u32>() {
        Ok(pid) => match cgroup::cgroup_attach_pid(node, pid) {
            Ok(()) => 0,
            Err(e) => e,
        },
        Err(_) => errno::Errno::InvalidArgument.as_neg_i32(),
    }
}

fn apply_subtree_control(node: &CgroupNode, input: &str) -> i32 {
    match cgroup::parse_subtree_control_tokens(input) {
        Some((set, clear)) => {
            let mut controllers = node.controllers.lock_irqsave();
            controllers.subtree_control &= !clear;
            controllers.subtree_control |= set;
            // Only real controllers can ever be enabled.
            controllers.subtree_control &= cgroup::ALL_CONTROLLERS;
            0
        }
        None => errno::Errno::InvalidArgument.as_neg_i32(),
    }
}

/// cpu.max write: "max 100000" | "max" | "<quota_us> [<period_us>]".
/// Empty quota field means "max"; missing period keeps the current one.
fn apply_cpu_max(node: &Arc<CgroupNode>, input: &str) -> i32 {
    let mut parts = input.split_whitespace();
    let quota_tok = match parts.next() {
        Some(t) => t,
        None => return errno::Errno::InvalidArgument.as_neg_i32(),
    };

    let quota_us: u64 = if quota_tok == "max" {
        LIMIT_MAX
    } else {
        match quota_tok.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
        }
    };
    let period_us: Option<u64> = match parts.next() {
        Some(t) => match t.parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
        },
        None => None,
    };

    let quota_ns = if quota_us == LIMIT_MAX {
        LIMIT_MAX
    } else {
        quota_us.saturating_mul(1000)
    };
    if let Some(p) = period_us {
        // Linux clamps the period to [1ms, 1s].
        if !(1000..=1_000_000).contains(&p) {
            return errno::Errno::InvalidArgument.as_neg_i32();
        }
    }

    let was_limited;
    let now_limited;
    {
        let controllers = node.controllers.lock_irqsave();
        was_limited = controllers.cpu_quota_ns.load(Ordering::Acquire) != LIMIT_MAX;
        controllers.cpu_quota_ns.store(quota_ns, Ordering::Release);
        if let Some(p) = period_us {
            controllers
                .cpu_period_ns
                .store(p.saturating_mul(1000), Ordering::Release);
        }
        now_limited = quota_ns != LIMIT_MAX;
        // Removing a limit while throttled clears the flag immediately.
        if !now_limited {
            node.cpu_throttled.store(false, Ordering::Release);
        }
    }
    cgroup::note_cpu_limit_changed(node, was_limited, now_limited);
    0
}

fn apply_memory_max(node: &CgroupNode, input: &str) -> i32 {
    let max: u64 = if input == "max" {
        LIMIT_MAX
    } else {
        match input.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
        }
    };
    node.controllers
        .lock_irqsave()
        .memory_max_bytes
        .store(max, Ordering::Release);
    0
}

fn apply_pids_max(node: &CgroupNode, input: &str) -> i32 {
    let max: u64 = if input == "max" {
        LIMIT_MAX
    } else {
        match input.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
        }
    };
    node.controllers
        .lock_irqsave()
        .pids_max
        .store(max, Ordering::Release);
    0
}

fn apply_content(node: &Arc<CgroupNode>, kind: CgroupFileKind, buf: &[u8]) -> i32 {
    let input = trim(buf);
    match kind {
        CgroupFileKind::CgroupProcs | CgroupFileKind::Tasks => {
            // SAFETY: node is a live Arc for the duration of the write.
            apply_procs(node, input)
        }
        CgroupFileKind::CgroupSubtreeControl => {
            // SAFETY: see above.
            apply_subtree_control(unsafe { &*(Arc::as_ptr(node) as *const CgroupNode) }, input)
        }
        CgroupFileKind::CpuMax => apply_cpu_max(node, input),
        CgroupFileKind::MemoryMax => {
            // SAFETY: see above.
            apply_memory_max(unsafe { &*(Arc::as_ptr(node) as *const CgroupNode) }, input)
        }
        CgroupFileKind::PidsMax => {
            // SAFETY: see above.
            apply_pids_max(unsafe { &*(Arc::as_ptr(node) as *const CgroupNode) }, input)
        }
        // Read-only control files.
        _ => errno::Errno::InvalidArgument.as_neg_i32(),
    }
}

// ============================================================================
// Filesystem type / mount
// ============================================================================

/// cgroup2 filesystem type (register_filesystem target for `mount -t cgroup2`).
pub static CGROUP2_FS_TYPE: FileSystemType = FileSystemType::new(
    "cgroup2",
    Some(cgroup2_mount),
    Some(cgroup2_kill_sb),
    0,
);

/// Global cgroup2 superblock pointer (created at init, never freed).
static GLOBAL_CGROUP2_SB: core::sync::atomic::AtomicPtr<SuperBlock> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

// SAFETY: FsContext is a valid mount context from the VFS; the superblock
// is a plain SuperBlock with no filesystem-private invariants.
unsafe extern "C" fn cgroup2_mount(
    _fs_context: &crate::fs::superblock::FsContext<'_>,
) -> Result<*mut SuperBlock, i32> {
    let sb = Box::new(SuperBlock::new(4096, CGROUP2_MAGIC));
    Ok(Box::into_raw(sb))
}

// SAFETY: sb came from cgroup2_mount (Box::into_raw of a SuperBlock).
unsafe extern "C" fn cgroup2_kill_sb(sb: *mut SuperBlock) {
    if !sb.is_null() {
        // The boot-created superblock is shared; only free private mounts.
        let current = GLOBAL_CGROUP2_SB.load(Ordering::Acquire);
        if current != sb {
            drop(Box::from_raw(sb));
        }
    }
}

/// cgroup2 magic (CGROUP2_SUPER_MAGIC).
const CGROUP2_MAGIC: u32 = 0x6367_7270;

/// Initialize the cgroup subsystem and register the cgroup2 fs type.
pub fn init_cgroupfs() -> Result<(), i32> {
    // Core hierarchy first (root node).
    cgroup::init_cgroup()?;

    // Superblock.
    if GLOBAL_CGROUP2_SB.load(Ordering::Acquire).is_null() {
        let sb = Box::new(SuperBlock::new(4096, CGROUP2_MAGIC));
        GLOBAL_CGROUP2_SB.store(Box::into_raw(sb), Ordering::Release);
    }

    // Filesystem type registration (for `mount -t cgroup2`).
    crate::fs::superblock::register_filesystem(&CGROUP2_FS_TYPE)?;

    Ok(())
}

/// Register the mount in the /proc/mounts table.
fn register_cgroup2_mount(target: &str) {
    crate::fs::mount::register_mount("cgroup2", target, "cgroup2", "rw");
}

/// Mount cgroup2 at `target` (boot path: "/sys/fs/cgroup").
pub fn mount_cgroupfs(target: &str) -> Result<(), i32> {
    // The hierarchy must exist.
    cgroup_root_required()?;

    // Create the on-disk (/rootfs) mount-point directories when the rootfs
    // is available so the path is visible there too (best effort).
    if let Some(rootfs_sb) = crate::fs::rootfs::get_rootfs_sb() {
        // SAFETY: pointer from get_rootfs_sb is the global RootFS instance.
        unsafe {
            let _ = (*rootfs_sb).create_dir("/sys", 0o755);
            let _ = (*rootfs_sb).create_dir("/sys/fs", 0o755);
            let _ = (*rootfs_sb).create_dir(target, 0o755);
        }
    }

    // Dentry tree mount.
    crate::fs::vfs::vfs_mount(
        target,
        create_root_inode(),
        crate::fs::mount::MntFlags::new(0),
    );
    register_cgroup2_mount(target);
    Ok(())
}

fn cgroup_root_required() -> Result<(), i32> {
    if cgroup::cgroup_root().is_some() {
        Ok(())
    } else {
        Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32())
    }
}

/// Build the VFS inode for the cgroupfs root directory.
pub fn create_root_inode() -> Arc<Inode> {
    let root = match cgroup::cgroup_root() {
        Some(r) => r,
        None => {
            // Defensive: init_cgroupfs() must have run. init_cgroup() is
            // idempotent, so heal instead of failing the mount.
            let _ = cgroup::init_cgroup();
            match cgroup::cgroup_root() {
                Some(r) => r,
                None => unreachable!("cgroup root cannot fail to initialize"),
            }
        }
    };
    make_dir_inode(root)
}

// ============================================================================
// Inode construction
// ============================================================================

/// Build a directory inode for a cgroup node.
fn make_dir_inode(node: Arc<CgroupNode>) -> Arc<Inode> {
    // SAFETY: the Arc keeps the node alive for the inode's lifetime (and
    // nodes are never freed anyway — see sched::cgroup lifetime docs).
    let node_ref = unsafe { &*(Arc::as_ptr(&node) as *const CgroupNode) };
    let mut inode = Inode::new(
        ino_of_dir(node_ref),
        InodeMode::new(InodeMode::S_IFDIR | 0o755),
    );
    inode.fs_id = crate::fs::inode::FS_ID_CGROUPFS;
    inode.ops = Some(&CGROUP_INODE_OPS);
    inode.private_data = Some(leak_ino_data(node, CgroupFileKind::Dir));
    Arc::new(inode)
}

/// Build a control-file inode.
fn make_file_inode(node: Arc<CgroupNode>, kind: CgroupFileKind) -> Arc<Inode> {
    // SAFETY: see make_dir_inode.
    let node_ref = unsafe { &*(Arc::as_ptr(&node) as *const CgroupNode) };
    let writable = CONTROL_FILES
        .iter()
        .find(|(_, k, _)| *k == kind)
        .map(|(_, _, w)| *w)
        .unwrap_or(false);
    let mode = if writable { 0o644 } else { 0o444 };
    let mut inode = Inode::new(
        ino_of_file(node_ref, kind),
        InodeMode::new(InodeMode::S_IFREG | mode),
    );
    inode.fs_id = crate::fs::inode::FS_ID_CGROUPFS;
    inode.ops = Some(&CGROUP_INODE_OPS);
    inode.private_data = Some(leak_ino_data(node, kind));
    Arc::new(inode)
}

/// Leak a CgroupInoData box and return the raw pointer for inode.private_data.
///
/// Intentional leak (bounded by the icache size): cgroup nodes are never
/// freed either, and inode lifetimes are managed by the dentry/icache
/// without a destroy hook we can rely on here.
fn leak_ino_data(node: Arc<CgroupNode>, kind: CgroupFileKind) -> *mut u8 {
    Box::into_raw(Box::new(CgroupInoData { node, kind })) as *mut u8
}

/// Read the private data back.
// SAFETY: pointer came from leak_ino_data and is never freed.
unsafe fn ino_data(inode: &Inode) -> Option<&'static CgroupInoData> {
    inode
        .private_data
        .map(|p| &*(p as *const CgroupInoData))
}

// ============================================================================
// Inode operations
// ============================================================================

/// cgroupfs lookup: child cgroup directory or a control file.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let data = ino_data(dir).ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    // Control files take precedence over same-named cgroup children
    // (mirrors kernfs: control files cannot be shadowed).
    if let Some(kind) = control_file_by_name(name) {
        return Ok(ino_of_file(&data.node, kind));
    }

    if let Some(child) = cgroup::cgroup_lookup_child(&data.node, core::str::from_utf8(name).unwrap_or("")) {
        // SAFETY: child is a live Arc.
        return Ok(ino_of_dir(unsafe { &*(Arc::as_ptr(&child) as *const CgroupNode) }));
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
}

/// cgroupfs mkdir: create a child cgroup.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_mkdir(dir: &Inode, name: &[u8], _mode: InodeMode) -> Result<Arc<Inode>, i32> {
    let data = ino_data(dir).ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let name_str = core::str::from_utf8(name)
        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    // Control-file names are reserved.
    if control_file_by_name(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
    }

    let node = cgroup::cgroup_create(&data.node, name_str)?;
    Ok(make_dir_inode(node))
}

/// cgroupfs rmdir: remove an (empty) child cgroup.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_rmdir(dir: &Inode, name: &[u8]) -> i32 {
    let data = match ino_data(dir) {
        Some(d) => d,
        None => return errno::Errno::NotADirectory.as_neg_i32(),
    };
    let name_str = match core::str::from_utf8(name) {
        Ok(s) => s,
        Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
    };

    match cgroup::cgroup_rmdir(&data.node, name_str) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// cgroupfs readdir: control files + child cgroup directories.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_readdir(inode: &Inode) -> Option<Vec<VfsDirEntry>> {
    let data = ino_data(inode)?;
    let node = &data.node;

    let mut entries = Vec::new();

    entries.push(VfsDirEntry {
        ino: inode.ino,
        name: alloc::vec![b'.'],
        file_type: file_type::DT_DIR,
    });
    entries.push(VfsDirEntry {
        ino: 1,
        name: alloc::vec![b'.', b'.'],
        file_type: file_type::DT_DIR,
    });

    for (name, kind, _) in CONTROL_FILES.iter() {
        entries.push(VfsDirEntry {
            ino: ino_of_file(node, *kind),
            name: name.as_bytes().to_vec(),
            file_type: file_type::DT_REG,
        });
    }

    let children: Vec<Arc<CgroupNode>> = {
        let guard = node.children.lock_irqsave();
        guard.values().cloned().collect()
    };
    for child in children {
        // SAFETY: live Arc; nodes are never freed.
        let child_ref = unsafe { &*(Arc::as_ptr(&child) as *const CgroupNode) };
        entries.push(VfsDirEntry {
            ino: ino_of_dir(child_ref),
            name: child_ref.name.as_bytes().to_vec(),
            file_type: file_type::DT_DIR,
        });
    }

    Some(entries)
}

/// cgroupfs iget: instantiate the VFS inode for (parent, name, ino).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, i32> {
    let data = ino_data(parent).ok_or(errno::Errno::NotADirectory.as_neg_i32())?;

    // Control file?
    if let Some(kind) = control_file_by_name(name) {
        if ino == ino_of_file(&data.node, kind) {
            return Ok(make_file_inode(data.node.clone(), kind));
        }
    }

    // Child cgroup directory (verify against the live hierarchy).
    let name_str = core::str::from_utf8(name).unwrap_or("");
    if let Some(child) = cgroup::cgroup_lookup_child(&data.node, name_str) {
        // SAFETY: live Arc.
        let child_ref = unsafe { &*(Arc::as_ptr(&child) as *const CgroupNode) };
        if ino_of_dir(child_ref) == ino {
            return Ok(make_dir_inode(child));
        }
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
}

/// cgroupfs getattr.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let data = match ino_data(inode) {
        Some(d) => d,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };

    stat.st_ino = inode.ino;
    stat.st_mode = inode.mode.bits();
    stat.st_size = if inode.mode.is_directory() {
        4096
    } else {
        generate_content(&data.node, data.kind).len() as i64
    };
    stat.st_nlink = if inode.mode.is_directory() { 2 } else { 1 };
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = (stat.st_size + 511) / 512;
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;
    0
}

/// cgroupfs setattr: ATTR_SIZE is a no-op (O_TRUNC on `echo x > file` must
/// not fail); everything else is EPERM — the tree is kernel-owned (same
/// policy as procfs).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_setattr(_inode: &Inode, attr: u32, _arg1: u64, _arg2: u64) -> i32 {
    if attr == crate::fs::inode::setattr_attr::ATTR_SIZE {
        return 0;
    }
    errno::Errno::OperationNotPermitted.as_neg_i32()
}

/// cgroupfs open: snapshot control-file content for reads.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_open(inode: &Inode, file: &crate::fs::File) -> i32 {
    if !inode.mode.is_regular_file() {
        return 0;
    }
    let data = match ino_data(inode) {
        Some(d) => d,
        None => return 0,
    };
    let content = generate_content(&data.node, data.kind);
    let snapshot = Box::new(CgroupFileContent {
        data: content,
    });
    file.set_private_data(Box::into_raw(snapshot) as *mut u8);
    0
}

/// cgroupfs get_file_ops.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn cgroupfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&CGROUP_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// cgroupfs inode operations table.
pub static CGROUP_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(cgroupfs_lookup),
    create: None,      // no file creation on cgroupfs
    link: None,
    unlink: None,
    symlink: None,
    mkdir: Some(cgroupfs_mkdir),
    rmdir: Some(cgroupfs_rmdir),
    mknod: None,
    rename: None,      // cgroups cannot be renamed
    readlink: None,
    get_file_ops: Some(cgroupfs_get_file_ops),
    readdir: Some(cgroupfs_readdir),
    open: Some(cgroupfs_open),
    permission: None,  // default mode-bit DAC
    getattr: Some(cgroupfs_getattr),
    setattr: Some(cgroupfs_setattr),
    iget: Some(cgroupfs_iget),
    destroy_inode: None,
};

// ============================================================================
// File operations (read/write on control files)
// ============================================================================

/// Read-side snapshot stored in file.private_data at open time.
struct CgroupFileContent {
    data: Vec<u8>,
}

fn cgroupfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data is either None or a valid CgroupFileContent
    // placed by cgroupfs_open; it is only freed by cgroupfs_file_close.
    unsafe {
        let data_ptr = match *file.private_data.get() {
            Some(p) => p,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };
        let content = &*(data_ptr as *const CgroupFileContent);
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
    }
}

fn cgroupfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: inode was written once at open time; read-only access here.
    let inode_opt = unsafe { (*file.inode.get()).clone() };
    let inode = match inode_opt {
        Some(i) => i,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    // SAFETY: cgroupfs inodes carry a CgroupInoData in private_data.
    let data = match unsafe { ino_data(&inode) } {
        Some(d) => d,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let ret = apply_content(&data.node, data.kind, buf);
    if ret != 0 {
        ret as isize
    } else {
        buf.len() as isize
    }
}

fn cgroupfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    // SAFETY: see cgroupfs_file_read.
    unsafe {
        let data_ptr = match *file.private_data.get() {
            Some(p) => p,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };
        let content = &*(data_ptr as *const CgroupFileContent);
        let file_size = content.data.len() as isize;
        let new_offset = match whence {
            0 => offset,
            1 => file.get_pos() as isize + offset,
            2 => file_size + offset,
            _ => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
        };
        if new_offset < 0 || new_offset > file_size {
            return errno::Errno::InvalidArgument.as_neg_i32() as isize;
        }
        file.set_pos(new_offset as u64);
        new_offset
    }
}

fn cgroupfs_file_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see cgroupfs_file_read; null-out first to prevent
    // use-after-free by concurrent readers.
    unsafe {
        if let Some(data_ptr) = *file.private_data.get() {
            *file.private_data.get() = None;
            drop(Box::from_raw(data_ptr as *mut CgroupFileContent));
        }
    }
    0
}

/// cgroupfs file operations table.
pub static CGROUP_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(cgroupfs_file_read),
    write: Some(cgroupfs_file_write),
    lseek: Some(cgroupfs_file_lseek),
    close: Some(cgroupfs_file_close),
    poll: None,
};
