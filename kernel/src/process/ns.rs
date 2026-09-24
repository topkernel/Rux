//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Namespaces (U1c) — systemd-style service isolation.
//!
//! Six namespace kinds following the Linux model:
//! - `UtsNamespace`   — hostname / domainname (fully isolated storage)
//! - `MountNamespace` — private mount table (copy-on-write Vec of entries;
//!   the dentry-level VFS remains global — recorded divergence)
//! - `PidNamespace`   — nested PID numbering; a task carries its ns-local
//!   pid (`Task::ns_local_pid`) plus the ns pointer; kill/wait translate
//! - `NetNamespace`   — stub (devices/addresses stay global)
//! - `IpcNamespace`   — stub (SysV IPC objects stay global)
//! - `UserNamespace`  — stub (uid/gid mapping is identity; only the initial
//!   namespace is truly supported)
//!
//! `None` in a Task ns slot means "initial namespace" — every accessor
//! falls back to the lazily created init instance, so boot-time tasks and
//! kernel threads need no explicit wiring.
//!
//! Syscalls: unshare(2), setns(2) (here); /proc/[pid]/ns/{kind} magic
//! symlinks + ns fds (open intercepted in sys_openat) live here too.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::sync::spinlock::Spinlock;

// ============================================================================
// CLONE_NEW* flag values (mirror process::fork; duplicated so this module
// has no dependency on the fork path)
// ============================================================================

pub const CLONE_NEWNS: u64 = 0x00020000;
pub const CLONE_NEWCGROUP: u64 = 0x02000000;
pub const CLONE_NEWUTS: u64 = 0x04000000;
pub const CLONE_NEWIPC: u64 = 0x08000000;
pub const CLONE_NEWUSER: u64 = 0x10000000;
pub const CLONE_NEWPID: u64 = 0x20000000;
pub const CLONE_NEWNET: u64 = 0x40000000;

/// All namespace-creating CLONE flags.
pub const CLONE_NEWMASK: u64 = CLONE_NEWNS
    | CLONE_NEWCGROUP
    | CLONE_NEWUTS
    | CLONE_NEWIPC
    | CLONE_NEWUSER
    | CLONE_NEWPID
    | CLONE_NEWNET;

// ============================================================================
// UTS namespace — hostname / domainname
// ============================================================================

/// Linux __NEW_UTS_LEN = 64 (65 with NUL; we store without NUL).
pub const UTS_LEN: usize = 64;

pub struct UtsNamespace {
    /// nsfs inode number (unique per namespace instance).
    pub inum: u64,
    /// NUL-terminated hostname buffer.
    pub hostname: Spinlock<[u8; UTS_LEN]>,
    /// NUL-terminated domainname buffer.
    pub domainname: Spinlock<[u8; UTS_LEN]>,
}

impl UtsNamespace {
    fn new(inum: u64) -> Self {
        Self {
            inum,
            hostname: Spinlock::new([0u8; UTS_LEN]),
            domainname: Spinlock::new([0u8; UTS_LEN]),
        }
    }

    /// Write `name` into a fixed UTS buffer (NUL-padded, length-checked).
    /// A full 64-byte name is stored without NUL (Linux __NEW_UTS_LEN).
    fn uts_set(buf: &mut [u8; UTS_LEN], name: &[u8]) -> bool {
        if name.len() > UTS_LEN {
            return false;
        }
        buf[..name.len()].copy_from_slice(name);
        if name.len() < UTS_LEN {
            buf[name.len()] = 0;
        }
        true
    }

    /// Read a fixed UTS buffer as a byte string up to the NUL.
    fn uts_get(buf: &[u8; UTS_LEN]) -> Vec<u8> {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(UTS_LEN);
        Vec::from(&buf[..len])
    }

    pub fn set_hostname(&self, name: &[u8]) -> bool {
        Self::uts_set(&mut self.hostname.lock(), name)
    }

    pub fn get_hostname(&self) -> Vec<u8> {
        Self::uts_get(&self.hostname.lock())
    }

    pub fn set_domainname(&self, name: &[u8]) -> bool {
        Self::uts_set(&mut self.domainname.lock(), name)
    }

    pub fn get_domainname(&self) -> Vec<u8> {
        Self::uts_get(&self.domainname.lock())
    }
}

// ============================================================================
// Mount namespace — private mount table
// ============================================================================

/// One row of the mount table (mirrors the legacy global registry tuple).
#[derive(Clone)]
pub struct MountEntry {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub flags: String,
}

pub struct MountNamespace {
    /// nsfs inode number.
    pub inum: u64,
    /// Namespace-private root dentry. Captured at ns creation from the
    /// global VFS root (CLONE_NEWNS does not move "/" — it only private-
    /// copies the mount table). Reserved as the pivot_root bookkeeping
    /// anchor; path_lookup still walks the shared dentry tree.
    pub root: Spinlock<Option<Arc<crate::fs::dentry::Dentry>>>,
    /// Private copy of the mount table. Copy-on-write: modifications after
    /// CLONE_NEWNS only touch this Vec, never the parent's.
    pub mounts: Spinlock<Vec<MountEntry>>,
}

impl MountNamespace {
    fn new(inum: u64, mounts: Vec<MountEntry>) -> Self {
        Self {
            inum,
            root: Spinlock::new(crate::fs::vfs::get_vfs_root()),
            mounts: Spinlock::new(mounts),
        }
    }
}

// ============================================================================
// PID namespace — nested numbering
// ============================================================================

pub struct PidNamespace {
    /// nsfs inode number.
    pub inum: u64,
    /// Parent (enclosing) namespace; None for the init namespace.
    #[allow(dead_code)] // structural (level already derived at creation)
    pub parent: Option<Arc<PidNamespace>>,
    /// Depth below the init namespace (init == 0).
    pub level: u32,
    /// Last ns-local pid handed out.
    pub last_pid: AtomicU32,
}

impl PidNamespace {
    fn new(inum: u64, parent: Option<Arc<PidNamespace>>) -> Self {
        let level = parent.as_ref().map_or(0, |p| p.level + 1);
        Self {
            inum,
            parent,
            level,
            last_pid: AtomicU32::new(0),
        }
    }

    /// Allocate the next ns-local pid. Minimal: monotonic counter, wraps at
    /// pid_max. The first allocation in a fresh namespace yields 1 (the
    /// namespace init).
    pub fn alloc_local_pid(&self) -> u32 {
        loop {
            let cur = self.last_pid.load(Ordering::Relaxed);
            let next = if cur >= crate::process::pid::pid_max_live() - 1 {
                1
            } else {
                cur + 1
            };
            if self
                .last_pid
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return next;
            }
        }
    }
}

// ============================================================================
// Stub namespaces — net / ipc / user
// ============================================================================

/// Net namespace stub. Network devices and addresses are GLOBAL (shared by
/// every instance); creating one only yields a distinct /proc/[pid]/ns/net
/// identity for setns/open purposes.
pub struct NetNamespace {
    pub inum: u64,
}

/// IPC namespace stub. SysV IPC objects are global.
pub struct IpcNamespace {
    pub inum: u64,
}

/// User namespace stub. uid/gid mapping is identity (kernel uid == ns uid);
/// only the initial namespace is supported. A new instance exists purely so
/// /proc shows a distinct ns and CLONE_NEWUSER does not fail.
pub struct UserNamespace {
    pub inum: u64,
    #[allow(dead_code)] // structural (identity mapping stub)
    pub parent: Option<Arc<UserNamespace>>,
}

// ============================================================================
// Init namespaces (lazily created singletons)
// ============================================================================

/// nsfs-style inum allocator. High base avoids colliding with the procfs
/// pid-dir ino scheme (pid * 10000 + offset).
static NEXT_NS_INUM: AtomicU64 = AtomicU64::new(0xEA00_0001);

fn alloc_ns_inum() -> u64 {
    NEXT_NS_INUM.fetch_add(1, Ordering::Relaxed)
}

static INIT_UTS: Spinlock<Option<Arc<UtsNamespace>>> = Spinlock::new(None);
static INIT_MNT: Spinlock<Option<Arc<MountNamespace>>> = Spinlock::new(None);
static INIT_PID: Spinlock<Option<Arc<PidNamespace>>> = Spinlock::new(None);
static INIT_NET: Spinlock<Option<Arc<NetNamespace>>> = Spinlock::new(None);
static INIT_IPC: Spinlock<Option<Arc<IpcNamespace>>> = Spinlock::new(None);
static INIT_USER: Spinlock<Option<Arc<UserNamespace>>> = Spinlock::new(None);

/// The initial UTS namespace, seeded with hostname "rux".
pub fn init_uts_ns() -> Arc<UtsNamespace> {
    let mut guard = INIT_UTS.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(UtsNamespace::new(alloc_ns_inum()));
    ns.set_hostname(b"rux");
    *guard = Some(ns.clone());
    ns
}

/// The initial mount namespace; its table seeds from the boot-time global
/// registry (rootfs row included).
pub fn init_mnt_ns() -> Arc<MountNamespace> {
    let mut guard = INIT_MNT.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(MountNamespace::new(alloc_ns_inum(), boot_mount_entries()));
    *guard = Some(ns.clone());
    ns
}

pub fn init_pid_ns() -> Arc<PidNamespace> {
    let mut guard = INIT_PID.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(PidNamespace::new(alloc_ns_inum(), None));
    *guard = Some(ns.clone());
    ns
}

pub fn init_net_ns() -> Arc<NetNamespace> {
    let mut guard = INIT_NET.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(NetNamespace {
        inum: alloc_ns_inum(),
    });
    *guard = Some(ns.clone());
    ns
}

pub fn init_ipc_ns() -> Arc<IpcNamespace> {
    let mut guard = INIT_IPC.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(IpcNamespace {
        inum: alloc_ns_inum(),
    });
    *guard = Some(ns.clone());
    ns
}

pub fn init_user_ns() -> Arc<UserNamespace> {
    let mut guard = INIT_USER.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(UserNamespace {
        inum: alloc_ns_inum(),
        parent: None,
    });
    *guard = Some(ns.clone());
    ns
}

/// True when `ns` IS the init pid namespace (identity translation zone).
pub fn is_init_pid_ns(ns: &Arc<PidNamespace>) -> bool {
    Arc::ptr_eq(ns, &init_pid_ns())
}

// ============================================================================
// Current-task accessors (None slot == initial namespace)
// ============================================================================

use crate::process::task::Task;

pub fn current_uts_ns() -> Arc<UtsNamespace> {
    crate::sched::current()
        .and_then(|t| t.ns_uts.clone())
        .unwrap_or_else(init_uts_ns)
}

pub fn current_mnt_ns() -> Arc<MountNamespace> {
    crate::sched::current()
        .and_then(|t| t.ns_mount.clone())
        .unwrap_or_else(init_mnt_ns)
}

/// Pid namespace of the current task (never None).
pub fn current_pid_ns() -> Arc<PidNamespace> {
    crate::sched::current()
        .and_then(|t| t.ns_pid.clone())
        .unwrap_or_else(init_pid_ns)
}

#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn current_net_ns() -> Arc<NetNamespace> {
    crate::sched::current()
        .and_then(|t| t.ns_net.clone())
        .unwrap_or_else(init_net_ns)
}

#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn current_ipc_ns() -> Arc<IpcNamespace> {
    crate::sched::current()
        .and_then(|t| t.ns_ipc.clone())
        .unwrap_or_else(init_ipc_ns)
}

#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn current_user_ns() -> Arc<UserNamespace> {
    crate::sched::current()
        .and_then(|t| t.ns_user.clone())
        .unwrap_or_else(init_user_ns)
}

// ============================================================================
// Mount-table bookkeeping (fs::mount routes here)
// ============================================================================

/// Boot-time seed for the initial mount namespace (the hardcoded rootfs row
/// plus anything registered before the first ns access).
fn boot_mount_entries() -> Vec<MountEntry> {
    // Read the legacy global registry (populated by do_mount at boot).
    crate::fs::mount::get_global_mounts()
        .into_iter()
        .map(|(d, m, f, fl)| MountEntry {
            device: d,
            mount_point: m,
            fs_type: f,
            flags: fl,
        })
        .collect()
}

/// Register a mount in the CURRENT mount namespace (fs::mount::register_mount
/// routes here). Re-mounting over an existing mountpoint replaces the row.
pub fn ns_register_mount(device: &str, mount_point: &str, fs_type: &str, flags: &str) {
    let ns = current_mnt_ns();
    let mut table = ns.mounts.lock();
    table.retain(|e| e.mount_point != mount_point);
    table.push(MountEntry {
        device: String::from(device),
        mount_point: String::from(mount_point),
        fs_type: String::from(fs_type),
        flags: String::from(flags),
    });
}

/// Drop a mount row from the current namespace (umount bookkeeping).
pub fn ns_unregister_mount(mount_point: &str) {
    let ns = current_mnt_ns();
    let mut table = ns.mounts.lock();
    table.retain(|e| e.mount_point != mount_point);
}

/// Snapshot of the current namespace's mount table.
pub fn ns_get_mounts() -> Vec<MountEntry> {
    current_mnt_ns().mounts.lock().clone()
}

/// True when `path` is a mountpoint of the current mount namespace.
pub fn ns_is_mountpoint(path: &str) -> bool {
    let norm = crate::fs::path::path_normalize(path);
    let ns = current_mnt_ns();
    let table = ns.mounts.lock();
    table.iter().any(|e| e.mount_point == norm)
}

// ============================================================================
// PID translation (vpids)
// ============================================================================

/// Pid namespace that NEW children of `task` land in (honors a previous
/// unshare(CLONE_NEWPID) / setns(pid_ns_fd)).
pub fn task_pid_ns_for_children(task: &Task) -> Arc<PidNamespace> {
    task.ns_pid_for_children
        .clone()
        .or_else(|| task.ns_pid.clone())
        .unwrap_or_else(init_pid_ns)
}

/// Translate a caller-namespace pid into the global pid of its thread-group
/// LEADER. Identity in the init namespace. None when no live leader matches
/// (the caller's namespace cannot see that pid).
pub fn resolve_vpid(local: u32) -> Option<u32> {
    let ns = current_pid_ns();
    if is_init_pid_ns(&ns) {
        return Some(local);
    }
    let mut found: Option<u32> = None;
    // SAFETY: callback receives hash-table task pointers; we only read
    // pid/ns fields that are immutable after fork-enqueue.
    unsafe {
        crate::process::pid_hash::pid_hash_for_each_task(|t| {
            if found.is_some() || t.is_null() {
                return;
            }
            let task = &*t;
            let same_ns = task
                .ns_pid
                .as_ref()
                .map_or(false, |a| Arc::ptr_eq(a, &ns));
            if same_ns && task.ns_local_pid == local && task.pid() == task.tgid() {
                found = Some(task.pid());
            }
        });
    }
    found
}

/// Global pid of a task as seen from the CURRENT task's pid namespace.
/// Same-namespace tasks report their ns-local pid; everything else reports
/// the global pid (single-level nesting boundary: ancestors above the
/// caller's ns are not tracked).
#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn task_vpid(task: &Task) -> u32 {
    let ns = current_pid_ns();
    let same_ns = task
        .ns_pid
        .as_ref()
        .map_or(is_init_pid_ns(&ns), |a| Arc::ptr_eq(a, &ns));
    if same_ns {
        task.ns_local_pid
    } else {
        task.pid()
    }
}

/// True when two tasks share a pid namespace (both None == init ns).
pub fn task_same_pid_ns(a: &Task, b: &Task) -> bool {
    match (a.ns_pid.as_ref(), b.ns_pid.as_ref()) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

/// wait4(2) reporting: a reaped child's pid as seen from the caller's pid
/// namespace. Falls back to the global pid when the task already left the
/// hash (late translation) or lives in another namespace.
pub fn wait_vpid(global_pid: u32) -> u32 {
    let ns = current_pid_ns();
    if is_init_pid_ns(&ns) {
        return global_pid;
    }
    match crate::process::find_task_by_pid(global_pid) {
        Some(t) => {
            let same = t
                .ns_pid
                .as_ref()
                .map_or(false, |a| Arc::ptr_eq(a, &ns));
            if same {
                t.ns_local_pid
            } else {
                global_pid
            }
        }
        None => global_pid,
    }
}

// ============================================================================
// fork / unshare / setns
// ============================================================================

/// Capability gate for namespace creation (Linux check_clonewhitelist/
/// create_new_namespaces): CAP_SYS_ADMIN unless CLONE_NEWUSER is present
/// (an unprivileged user may create a user ns first).
fn check_ns_capability(flags: u64) -> Result<(), i32> {
    if flags & CLONE_NEWUSER == 0 && !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return Err(-1); // EPERM
    }
    Ok(())
}

/// Namespace half of copy_process: the child either shares the parent's
/// namespace objects (plain Arc clone — fork default) or gets fresh ones
/// for every CLONE_NEW* bit.
///
/// SAFETY: both pointers are valid tasks; `child` is not yet runnable.
pub unsafe fn copy_namespaces(parent: *mut Task, child: *mut Task, flags: u64) -> Result<(), i32> {
    let parent = &*parent;
    let child = &mut *child;

    check_ns_capability(flags & CLONE_NEWMASK)?;

    // --- UTS ---
    if flags & CLONE_NEWUTS != 0 {
        let ns = Arc::new(UtsNamespace::new(alloc_ns_inum()));
        // Linux copy_utsname: the child starts with a COPY of the parent's
        // names, then diverges.
        let (hn, dn) = match parent.ns_uts.as_ref() {
            Some(p) => (p.get_hostname(), p.get_domainname()),
            None => (
                init_uts_ns().get_hostname(),
                init_uts_ns().get_domainname(),
            ),
        };
        ns.set_hostname(&hn);
        ns.set_domainname(&dn);
        child.ns_uts = Some(ns);
    } else {
        child.ns_uts = parent.ns_uts.clone();
    }

    // --- mount ---
    if flags & CLONE_NEWNS != 0 {
        let table = match parent.ns_mount.as_ref() {
            Some(p) => p.mounts.lock().clone(),
            None => init_mnt_ns().mounts.lock().clone(),
        };
        child.ns_mount = Some(Arc::new(MountNamespace::new(alloc_ns_inum(), table)));
    } else {
        child.ns_mount = parent.ns_mount.clone();
    }

    // --- pid ---
    let parent_ns = parent.ns_pid.clone().unwrap_or_else(init_pid_ns);
    if flags & CLONE_NEWPID != 0 {
        let ns = Arc::new(PidNamespace::new(alloc_ns_inum(), Some(parent_ns)));
        child.ns_pid = Some(ns.clone());
        // The namespace init: first local pid is 1.
        child.ns_local_pid = ns.alloc_local_pid();
    } else {
        // Children land where the parent's ns_pid_for_children points
        // (unshare(CLONE_NEWPID) / setns(pid_ns_fd) semantics).
        let target = task_pid_ns_for_children(parent);
        if is_init_pid_ns(&target) {
            child.ns_pid = None; // slot None == initial namespace
            child.ns_local_pid = child.pid(); // identity in the init ns
        } else {
            child.ns_pid = Some(target.clone());
            child.ns_local_pid = target.alloc_local_pid();
        }
    }

    // --- stubs ---
    if flags & CLONE_NEWNET != 0 {
        child.ns_net = Some(Arc::new(NetNamespace {
            inum: alloc_ns_inum(),
        }));
    } else {
        child.ns_net = parent.ns_net.clone();
    }
    if flags & CLONE_NEWIPC != 0 {
        child.ns_ipc = Some(Arc::new(IpcNamespace {
            inum: alloc_ns_inum(),
        }));
    } else {
        child.ns_ipc = parent.ns_ipc.clone();
    }
    if flags & CLONE_NEWUSER != 0 {
        let p = parent.ns_user.clone();
        child.ns_user = Some(Arc::new(UserNamespace {
            inum: alloc_ns_inum(),
            parent: p,
        }));
    } else {
        child.ns_user = parent.ns_user.clone();
    }

    Ok(())
}

/// unshare(2) namespace half: give the CALLING task fresh namespaces for
/// every CLONE_NEW* bit. CLONE_NEWPID only switches the children target
/// (Linux: the caller keeps its own pids).
pub fn unshare_namespaces(flags: u64) -> Result<(), i32> {
    let nsflags = flags & CLONE_NEWMASK;
    if nsflags == 0 {
        return Ok(());
    }
    check_ns_capability(nsflags)?;

    let Some(task) = crate::sched::current() else {
        return Err(-1); // EPERM
    };

    // SAFETY: we are mutating the current task's own ns slots; no other
    // CPU holds a reference through them concurrently (fork takes the
    // parent's slots under the same task context).
    unsafe {
        if nsflags & CLONE_NEWUTS != 0 {
            let old = task_uts_of(task);
            let ns = Arc::new(UtsNamespace::new(alloc_ns_inum()));
            ns.set_hostname(&old.get_hostname());
            ns.set_domainname(&old.get_domainname());
            task.ns_uts = Some(ns);
        }
        if nsflags & CLONE_NEWNS != 0 {
            let table = task_mnt_table_of(task);
            task.ns_mount = Some(Arc::new(MountNamespace::new(alloc_ns_inum(), table)));
        }
        if nsflags & CLONE_NEWPID != 0 {
            // The caller itself stays where it is; only future children are
            // created inside the new namespace.
            let cur = task
                .ns_pid
                .clone()
                .unwrap_or_else(init_pid_ns);
            task.ns_pid_for_children = Some(Arc::new(PidNamespace::new(
                alloc_ns_inum(),
                Some(cur),
            )));
        }
        if nsflags & CLONE_NEWNET != 0 {
            task.ns_net = Some(Arc::new(NetNamespace {
                inum: alloc_ns_inum(),
            }));
        }
        if nsflags & CLONE_NEWIPC != 0 {
            task.ns_ipc = Some(Arc::new(IpcNamespace {
                inum: alloc_ns_inum(),
            }));
        }
        if nsflags & CLONE_NEWUSER != 0 {
            let p = task.ns_user.clone();
            task.ns_user = Some(Arc::new(UserNamespace {
                inum: alloc_ns_inum(),
                parent: p,
            }));
        }
    }
    Ok(())
}

/// SAFETY: task is the current task pointer.
unsafe fn task_uts_of(task: &mut Task) -> Arc<UtsNamespace> {
    task.ns_uts.clone().unwrap_or_else(init_uts_ns)
}

/// SAFETY: task is the current task pointer.
unsafe fn task_mnt_table_of(task: &mut Task) -> Vec<MountEntry> {
    match task.ns_mount.as_ref() {
        Some(a) => a.mounts.lock().clone(),
        None => init_mnt_ns().mounts.lock().clone(),
    }
}

// ============================================================================
// Namespace kinds + /proc/[pid]/ns symlinks + ns fds (setns targets)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsKind {
    Uts,
    Mnt,
    Pid,
    Net,
    Ipc,
    User,
}

impl NsKind {
    /// /proc/[pid]/ns entry name.
    pub fn name(&self) -> &'static str {
        match self {
            NsKind::Uts => "uts",
            NsKind::Mnt => "mnt",
            NsKind::Pid => "pid",
            NsKind::Net => "net",
            NsKind::Ipc => "ipc",
            NsKind::User => "user",
        }
    }

    pub fn from_name(name: &[u8]) -> Option<NsKind> {
        match name {
            b"uts" => Some(NsKind::Uts),
            b"mnt" => Some(NsKind::Mnt),
            b"pid" => Some(NsKind::Pid),
            b"net" => Some(NsKind::Net),
            b"ipc" => Some(NsKind::Ipc),
            b"user" => Some(NsKind::User),
            _ => None,
        }
    }

    /// Corresponding CLONE_NEW* bit (setns nstype check).
    pub fn clone_flag(&self) -> u64 {
        match self {
            NsKind::Uts => CLONE_NEWUTS,
            NsKind::Mnt => CLONE_NEWNS,
            NsKind::Pid => CLONE_NEWPID,
            NsKind::Net => CLONE_NEWNET,
            NsKind::Ipc => CLONE_NEWIPC,
            NsKind::User => CLONE_NEWUSER,
        }
    }

    /// All kinds (readdir order for /proc/[pid]/ns).
    pub const ALL: [NsKind; 6] = [
        NsKind::Mnt,
        NsKind::Uts,
        NsKind::Pid,
        NsKind::Net,
        NsKind::Ipc,
        NsKind::User,
    ];

    /// Stable index (procfs ino encoding for /proc/[pid]/ns/<kind>).
    pub fn index(&self) -> u64 {
        match self {
            NsKind::Mnt => 0,
            NsKind::Uts => 1,
            NsKind::Pid => 2,
            NsKind::Net => 3,
            NsKind::Ipc => 4,
            NsKind::User => 5,
        }
    }

    /// Reverse of `index`.
    pub fn from_index(i: u64) -> Option<NsKind> {
        match i {
            0 => Some(NsKind::Mnt),
            1 => Some(NsKind::Uts),
            2 => Some(NsKind::Pid),
            3 => Some(NsKind::Net),
            4 => Some(NsKind::Ipc),
            5 => Some(NsKind::User),
            _ => None,
        }
    }
}

/// Namespace handle kept alive for open ns fds.
#[derive(Clone)]
pub enum NsRef {
    Uts(Arc<UtsNamespace>),
    Mnt(Arc<MountNamespace>),
    Pid(Arc<PidNamespace>),
    Net(Arc<NetNamespace>),
    Ipc(Arc<IpcNamespace>),
    User(Arc<UserNamespace>),
}

impl NsRef {
    pub fn kind(&self) -> NsKind {
        match self {
            NsRef::Uts(_) => NsKind::Uts,
            NsRef::Mnt(_) => NsKind::Mnt,
            NsRef::Pid(_) => NsKind::Pid,
            NsRef::Net(_) => NsKind::Net,
            NsRef::Ipc(_) => NsKind::Ipc,
            NsRef::User(_) => NsKind::User,
        }
    }

    pub fn inum(&self) -> u64 {
        match self {
            NsRef::Uts(a) => a.inum,
            NsRef::Mnt(a) => a.inum,
            NsRef::Pid(a) => a.inum,
            NsRef::Net(a) => a.inum,
            NsRef::Ipc(a) => a.inum,
            NsRef::User(a) => a.inum,
        }
    }
}

/// Open-ns-fd registry: nsfs inum → handle. Entries keep the namespace
/// alive while any fd references it (bounded by distinct-ns count).
static NS_FD_TABLE: Spinlock<Vec<(u64, NsRef)>> = Spinlock::new(Vec::new());

fn ns_fd_register(handle: NsRef) {
    let inum = handle.inum();
    let mut table = NS_FD_TABLE.lock();
    if !table.iter().any(|(i, _)| *i == inum) {
        table.push((inum, handle));
    }
}

fn ns_fd_lookup(inum: u64) -> Option<NsRef> {
    let table = NS_FD_TABLE.lock();
    table
        .iter()
        .find(|(i, _)| *i == inum)
        .map(|(_, h)| h.clone())
}

/// The namespace handle of `pid`'s `kind` slot (falls back to the init
/// namespace for tasks whose slot is None).
pub fn task_ns_handle(task: &Task, kind: NsKind) -> NsRef {
    match kind {
        NsKind::Uts => NsRef::Uts(task.ns_uts.clone().unwrap_or_else(init_uts_ns)),
        NsKind::Mnt => NsRef::Mnt(task.ns_mount.clone().unwrap_or_else(init_mnt_ns)),
        NsKind::Pid => NsRef::Pid(task.ns_pid.clone().unwrap_or_else(init_pid_ns)),
        NsKind::Net => NsRef::Net(task.ns_net.clone().unwrap_or_else(init_net_ns)),
        NsKind::Ipc => NsRef::Ipc(task.ns_ipc.clone().unwrap_or_else(init_ipc_ns)),
        NsKind::User => NsRef::User(task.ns_user.clone().unwrap_or_else(init_user_ns)),
    }
}

/// readlink(2) target for /proc/[pid]/ns/<kind>: "<kind>:[<inum>]".
pub fn ns_link_target(pid: u32, kind: NsKind) -> Vec<u8> {
    let handle = match crate::process::find_task_by_pid(pid) {
        Some(t) => task_ns_handle(t, kind),
        None => return alloc::format!("{}:[{}]", kind.name(), fallback_inum(kind)).into_bytes(),
    };
    alloc::format!("{}:[{}]", kind.name(), handle.inum()).into_bytes()
}

/// Init-namespace inum for a dead task's ns link.
fn fallback_inum(kind: NsKind) -> u64 {
    task_ns_handle_init(kind).inum()
}

fn task_ns_handle_init(kind: NsKind) -> NsRef {
    match kind {
        NsKind::Uts => NsRef::Uts(init_uts_ns()),
        NsKind::Mnt => NsRef::Mnt(init_mnt_ns()),
        NsKind::Pid => NsRef::Pid(init_pid_ns()),
        NsKind::Net => NsRef::Net(init_net_ns()),
        NsKind::Ipc => NsRef::Ipc(init_ipc_ns()),
        NsKind::User => NsRef::User(init_user_ns()),
    }
}

/// nsfs file read payload (Linux /proc/<pid>/ns/<kind> read): "<kind>:[<inum>]\n".
fn ns_file_content(handle: &NsRef) -> Vec<u8> {
    let mut out = alloc::format!("{}:[{}]", handle.kind().name(), handle.inum()).into_bytes();
    out.push(b'\n');
    out
}

/// Open a namespace fd for (pid, kind) — the /proc/[pid]/ns/<kind> magic
/// link target. Returns an fd whose inode ino is the ns inum and whose
/// read() returns "<kind>:[<inum>]\n".
pub fn open_ns_file(pid: u32, kind: NsKind, flags: u32) -> Result<usize, i32> {
    let handle = crate::process::find_task_by_pid(pid)
        .map(|t| task_ns_handle(t, kind))
        .ok_or(crate::errno::Errno::NoSuchProcess.as_neg_i32())?;

    let content = alloc::boxed::Box::new(NsFileContent {
        data: ns_file_content(&handle),
    });

    // SAFETY: static FileOps; ownership of the boxed content moves to the
    // File via set_private_data and is reclaimed in ns_file_close.
    unsafe {
        let file = Arc::new(crate::fs::File::new(crate::fs::FileFlags::new(flags)));
        file.set_ops(&NS_FILE_OPS);
        file.set_private_data(alloc::boxed::Box::into_raw(content) as *mut u8);

        let mut inode = crate::fs::inode::Inode::new(
            handle.inum(),
            crate::fs::inode::InodeMode::new(
                crate::fs::inode::InodeMode::S_IFREG | 0o444,
            ),
        );
        inode.fs_id = crate::fs::inode::FS_ID_PROCFS;
        file.set_inode(Arc::new(inode));

        // Keep the namespace alive for the fd's lifetime.
        ns_fd_register(handle);

        crate::fs::file::get_file_fd_install(file)
            .ok_or(crate::errno::Errno::TooManyOpenFiles.as_neg_i32())
    }
}

struct NsFileContent {
    data: Vec<u8>,
}

fn ns_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data is the NsFileContent installed by open_ns_file.
    unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(ptr) = *data_opt {
            let content = &*(ptr as *const NsFileContent);
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
            -9 // EBADF
        }
    }
}

fn ns_file_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see ns_file_read.
    unsafe {
        let data_opt = &mut *file.private_data.get();
        if let Some(ptr) = data_opt.take() {
            let _ = alloc::boxed::Box::from_raw(ptr as *mut NsFileContent);
        }
    }
    0
}

static NS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(ns_file_read),
    write: None,
    lseek: None,
    close: Some(ns_file_close),
    poll: None,
};

/// Intercept /proc/[pid]/ns/<kind> opens from sys_openat. Returns
/// Some(fd-or-negative-errno) when the path matched.
pub fn try_open_ns_path(path: &str, flags: u32) -> Option<i64> {
    let parts: Vec<&str> = path
        .trim_end_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 4 || parts[0] != "proc" || parts[2] != "ns" {
        return None;
    }
    let pid: u32 = if parts[1] == "self" {
        crate::process::current_pid()
    } else {
        match parts[1].parse::<u32>() {
            Ok(p) => p,
            Err(_) => return None,
        }
    };
    let kind = NsKind::from_name(parts[3].as_bytes())?;
    Some(match open_ns_file(pid, kind, flags) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    })
}

// ============================================================================
// setns(2)
// ============================================================================

/// setns(fd, nstype): reassociate the calling thread with the namespace
/// referenced by an ns fd (opened from /proc/[pid]/ns/<kind>).
///
/// PID namespaces follow Linux: the change only affects future children
/// (ns_pid_for_children). Joining a user namespace is accepted but the
/// mapping stays identity (stub).
pub fn setns(fd: i32, nstype: u64) -> Result<(), i32> {
    if fd < 0 {
        return Err(crate::errno::Errno::BadFileNumber.as_neg_i32());
    }
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return Err(crate::errno::Errno::OperationNotPermitted.as_neg_i32());
    }

    // SAFETY: fd validated non-negative; get_file_fd returns an Arc clone.
    let file = unsafe { crate::fs::get_file_fd(fd as usize) }
        .ok_or(crate::errno::Errno::BadFileNumber.as_neg_i32())?;

    // SAFETY: inode is written once at open time; read-only access here.
    let ino = unsafe {
        let inode_opt = &*file.inode.get();
        inode_opt.as_ref().map(|i| i.ino)
    }
    .ok_or(crate::errno::Errno::BadFileNumber.as_neg_i32())?;

    let handle = ns_fd_lookup(ino)
        .ok_or(crate::errno::Errno::InvalidArgument.as_neg_i32())?;

    // nstype check: when nonzero it must name the fd's kind.
    if nstype != 0 && nstype != handle.kind().clone_flag() {
        return Err(crate::errno::Errno::BadFileNumber.as_neg_i32());
    }

    let Some(task) = crate::sched::current() else {
        return Err(crate::errno::Errno::OperationNotPermitted.as_neg_i32());
    };

    // current() yields &mut Task: ns slot writes need no unsafe.
    match handle {
        NsRef::Uts(ns) => task.ns_uts = Some(ns),
        NsRef::Mnt(ns) => task.ns_mount = Some(ns),
        NsRef::Net(ns) => task.ns_net = Some(ns),
        NsRef::Ipc(ns) => task.ns_ipc = Some(ns),
        NsRef::User(ns) => task.ns_user = Some(ns),
        NsRef::Pid(ns) => {
            // Cannot renumber ourselves: only future children enter.
            task.ns_pid_for_children = Some(ns);
        }
    }
    Ok(())
}
