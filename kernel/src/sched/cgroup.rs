//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! cgroup v2 — unified hierarchy core (simplified Linux cgroup v2)
//!
//! systemd hard-depends on a writable cgroup v2 hierarchy mounted at
//! /sys/fs/cgroup; without it systemd fails at boot. This module owns the
//! hierarchy itself (CgroupNode tree + task membership); the cgroupfs
//! presentation layer lives in `fs::cgroup`.
//!
//! ## Design (Linux cgroup v2, simplified)
//!
//! - Single unified hierarchy rooted at `CGROUP_ROOT` (mounted as "cgroup2").
//! - `CgroupNode`: name / parent / children / level / task list / controller
//!   state. Directory create/remove maps to mkdir/rmdir on cgroupfs.
//! - Task membership is a raw `cgroup_ptr` in `Task` (Linux
//!   task_struct::cgroups analogue). `NULL` means "implicit root, no
//!   limits" — kernel threads and early boot tasks never attach.
//! - Three controllers:
//!   - **cpu**: cpu.max quota/period bandwidth. scheduler_tick charges one
//!     tick of runtime into every ancestor; crossing quota throttles the
//!     cgroup until the period rolls over. The CFS/RT pick paths skip
//!     throttled tasks.
//!   - **pids**: pids.max task-count ceiling checked in fork (EAGAIN).
//!   - **memory**: memory.max byte ceiling charged at mmap time (simplified
//!     RSS approximation: charge mapping length, uncharge at munmap/exit).
//!
//! ## Locking
//!
//! Every cgroup Spinlock (children / tasks / controllers) is taken with
//! `lock_irqsave()` — the cpu accounting walks run from the timer tick
//! (IRQ context), and a process-context `lock()` holder interrupted on the
//! same CPU would deadlock. Lock order is always parent → child.
//!
//! Runtime accounting uses plain atomics (no lock) for the pick-path
//! `task_throttled()` check, which runs under the scheduler GRQ lock.
//!
//! ## Lifetime
//!
//! CgroupNodes are NEVER freed: rmdir requires an empty cgroup and then
//! only unlinks it from the parent's map (`mem::forget` keeps the
//! allocation alive), so raw `cgroup_ptr`s in tasks and leaked
//! cgroupfs inodes can never dangle.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::process::task::Task;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// Constants
// ============================================================================

/// Sentinel for "no limit" (Linux "max") in cpu.max / memory.max / pids.max.
pub const LIMIT_MAX: u64 = u64::MAX;

/// Default cpu.max period: 100 ms (Linux default, in ns here).
pub const DEFAULT_CPU_PERIOD_NS: u64 = 100_000_000;

/// One scheduler tick in ns (KERNEL_HZ ticks per second).
pub const CGROUP_TICK_NS: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);

/// Controller bit flags (cgroup.controllers / cgroup.subtree_control).
pub const CTRL_CPU: u32 = 1 << 0;
pub const CTRL_MEMORY: u32 = 1 << 1;
pub const CTRL_PIDS: u32 = 1 << 2;

/// All controllers this kernel implements.
pub const ALL_CONTROLLERS: u32 = CTRL_CPU | CTRL_MEMORY | CTRL_PIDS;

/// Controller names in /proc-styles display order.
pub const CONTROLLER_NAMES: [&str; 3] = ["cpu", "memory", "pids"];

// ============================================================================
// Controller state
// ============================================================================

/// Per-cgroup controller configuration.
///
/// `subtree_control` is the cgroup.subtree_control bitset (which controllers
/// are enabled FOR THE CHILDREN). The limit knobs are atomics so the timer
/// tick can read them without the enclosing Spinlock; the Spinlock still
/// serializes multi-field updates (e.g. cpu.max quota+period pairs).
pub struct CgroupControllers {
    /// Controllers enabled for children via cgroup.subtree_control.
    pub subtree_control: u32,

    /// cpu controller: bandwidth quota per period (ns; LIMIT_MAX = max).
    pub cpu_quota_ns: AtomicU64,
    /// cpu controller: period length (ns). Defaults to 100 ms.
    pub cpu_period_ns: AtomicU64,

    /// memory controller: memory.max in bytes (LIMIT_MAX = max).
    pub memory_max_bytes: AtomicU64,

    /// pids controller: pids.max task ceiling (LIMIT_MAX = max).
    pub pids_max: AtomicU64,
}

impl CgroupControllers {
    pub const fn new() -> Self {
        Self {
            subtree_control: 0,
            cpu_quota_ns: AtomicU64::new(LIMIT_MAX),
            cpu_period_ns: AtomicU64::new(DEFAULT_CPU_PERIOD_NS),
            memory_max_bytes: AtomicU64::new(LIMIT_MAX),
            pids_max: AtomicU64::new(LIMIT_MAX),
        }
    }
}

// ============================================================================
// Cgroup node
// ============================================================================

/// A node in the unified cgroup hierarchy.
///
/// Node lifetime: see module docs — nodes are never freed once created
/// (rmdir only unlinks), so `Arc<CgroupNode>` → raw pointer → `&'static`
/// reinterpretation is sound.
pub struct CgroupNode {
    /// Name of this cgroup (component name; "" for the root).
    pub name: String,
    /// Parent node (None for the root only).
    pub parent: Option<Arc<CgroupNode>>,
    /// Child cgroups, keyed by name (mkdir/rmdir on cgroupfs).
    pub children: Spinlock<BTreeMap<String, Arc<CgroupNode>>>,
    /// Depth from the root (root = 0).
    pub level: u32,
    /// PIDs of member tasks (leaf membership; ancestors see them via the
    /// subtree walk in `count_tasks_subtree`).
    pub tasks: Spinlock<Vec<u32>>,
    /// Controller configuration (limits + subtree_control).
    pub controllers: Spinlock<CgroupControllers>,

    // ---- cpu runtime accounting (atomics: touched from timer IRQ) ----
    /// Total cpu time consumed by this cgroup's tasks (ns), all periods.
    pub cpu_usage_total_ns: AtomicU64,
    /// cpu time consumed in the CURRENT period (ns).
    pub cpu_usage_period_ns: AtomicU64,
    /// sched_clock timestamp when the current period started (ns).
    pub cpu_period_start_ns: AtomicU64,
    /// Throttle flag: set when usage exceeds quota, cleared at rollover.
    pub cpu_throttled: AtomicBool,
    /// How many times this cgroup entered the throttled state.
    pub nr_throttled: AtomicU64,
    /// Throttle duration accumulator (ns) — approximated at unthrottle.
    pub throttled_ns: AtomicU64,

    // ---- memory accounting (atomics) ----
    /// memory.current: bytes charged to this cgroup (includes children —
    /// charges propagate up the chain).
    pub memory_usage_bytes: AtomicU64,

    /// Inode-number base for this node (cgroupfs). File inodes are
    /// `ino_base + file kind`, so the stride below must exceed the number
    /// of control files.
    pub ino_base: u64,
}

/// Inode-number stride between sibling cgroups (room for the control files).
pub const CGROUP_INO_STRIDE: u64 = 32;

/// Next inode base allocator (root gets 1).
static NEXT_INO_BASE: AtomicU64 = AtomicU64::new(1 + CGROUP_INO_STRIDE);

/// Number of cgroups that currently carry a cpu quota (limits the tick-time
/// period-rollover tree walk to "there is at least one limited cgroup").
static CPU_LIMITED_NODES: AtomicU64 = AtomicU64::new(0);

impl CgroupNode {
    /// Root node constructor.
    fn new_root() -> Self {
        Self {
            name: String::new(),
            parent: None,
            children: Spinlock::new(BTreeMap::new()),
            level: 0,
            tasks: Spinlock::new(Vec::new()),
            controllers: Spinlock::new(CgroupControllers::new()),
            cpu_usage_total_ns: AtomicU64::new(0),
            cpu_usage_period_ns: AtomicU64::new(0),
            cpu_period_start_ns: AtomicU64::new(0),
            cpu_throttled: AtomicBool::new(false),
            nr_throttled: AtomicU64::new(0),
            throttled_ns: AtomicU64::new(0),
            memory_usage_bytes: AtomicU64::new(0),
            ino_base: 1,
        }
    }

    /// Child node constructor.
    fn new_child(name: &str, parent: Arc<Self>, level: u32, ino_base: u64) -> Self {
        let now = crate::sched::fair::sched_clock();
        Self {
            name: name.to_string(),
            parent: Some(parent),
            children: Spinlock::new(BTreeMap::new()),
            level,
            tasks: Spinlock::new(Vec::new()),
            controllers: Spinlock::new(CgroupControllers::new()),
            cpu_usage_total_ns: AtomicU64::new(0),
            cpu_usage_period_ns: AtomicU64::new(0),
            cpu_period_start_ns: AtomicU64::new(now),
            cpu_throttled: AtomicBool::new(false),
            nr_throttled: AtomicU64::new(0),
            throttled_ns: AtomicU64::new(0),
            memory_usage_bytes: AtomicU64::new(0),
            ino_base,
        }
    }

    /// Absolute path of this cgroup within the hierarchy
    /// ("" for root, "/init.scope" style for children).
    #[allow(dead_code)] // debug/diagnostic helper (pr_* on migration)
    pub fn path(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = self;
        loop {
            match cur.parent.as_ref() {
                Some(p) => {
                    if !cur.name.is_empty() {
                        parts.push(&cur.name);
                    }
                    // SAFETY: parent Arc outlives `cur` (children keep
                    // parents alive; nodes are never freed).
                    cur = unsafe { &*(Arc::as_ptr(p) as *const CgroupNode) };
                }
                None => break,
            }
        }
        if parts.is_empty() {
            return String::from("/");
        }
        parts.reverse();
        let mut s = String::new();
        for p in parts {
            s.push('/');
            s.push_str(p);
        }
        s
    }

    /// Run `f` on this node and every ancestor (self first, up to root).
    pub fn for_each_ancestor<F: FnMut(&CgroupNode)>(&self, mut f: F) {
        let mut cur = self;
        loop {
            f(cur);
            match cur.parent.as_ref() {
                Some(p) => {
                    // SAFETY: see path().
                    cur = unsafe { &*(Arc::as_ptr(p) as *const CgroupNode) };
                }
                None => return,
            }
        }
    }

    /// Whether any cgroup from this node up to the root is cpu-throttled.
    /// Lock-free (atomics only) — called from the scheduler pick path under
    /// the GRQ lock.
    pub fn chain_throttled(&self) -> bool {
        let mut hit = false;
        self.for_each_ancestor(|n| {
            if n.cpu_throttled.load(Ordering::Acquire) {
                hit = true;
            }
        });
        hit
    }

    /// Whether this cgroup currently has member tasks (cgroup.events
    /// "populated" — includes tasks living in descendant cgroups).
    pub fn populated(&self) -> bool {
        self.subtree_task_count() > 0
    }

    /// Count member tasks in this node's whole subtree (children included).
    pub fn subtree_task_count(&self) -> usize {
        let children: Vec<Arc<CgroupNode>> = {
            let guard = self.children.lock_irqsave();
            guard.values().cloned().collect()
        };
        let mut count = self.tasks.lock_irqsave().len();
        for c in children {
            // SAFETY: Arc-derived references; nodes are never freed.
            count += unsafe { &*Arc::as_ptr(&c) }.subtree_task_count();
        }
        count
    }
}

// SAFETY: all shared mutable state is behind irqsave Spinlocks or atomics.
unsafe impl Send for CgroupNode {}
// SAFETY: see above — no data race is possible across CPUs.
unsafe impl Sync for CgroupNode {}

// ============================================================================
// Global root
// ============================================================================

static CGROUP_ROOT_CELL: Spinlock<Option<Arc<CgroupNode>>> = Spinlock::new(None);

/// Initialize the unified hierarchy (idempotent). Called once at boot,
/// before the cgroup2 mount and before the first user task attaches.
pub fn init_cgroup() -> Result<(), i32> {
    let mut guard = CGROUP_ROOT_CELL.lock();
    if guard.is_none() {
        *guard = Some(Arc::new(CgroupNode::new_root()));
    }
    Ok(())
}

/// The unified-hierarchy root ("/sys/fs/cgroup" itself).
pub fn cgroup_root() -> Option<Arc<CgroupNode>> {
    CGROUP_ROOT_CELL.lock().clone()
}

/// Allocate the next cgroupfs inode base.
pub fn alloc_ino_base() -> u64 {
    NEXT_INO_BASE.fetch_add(CGROUP_INO_STRIDE, Ordering::Relaxed)
}

// ============================================================================
// Hierarchy operations (mkdir / rmdir / lookup)
// ============================================================================

/// Validate a cgroup name (mkdir component). Linux forbids '/', "." and ".."
/// and names that collide with control files are handled by the cgroupfs
/// lookup order (control files win).
fn valid_cgroup_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains('/')
}

/// Create a child cgroup (mkdir on cgroupfs).
pub fn cgroup_create(parent: &Arc<CgroupNode>, name: &str) -> Result<Arc<CgroupNode>, i32> {
    if !valid_cgroup_name(name) {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }

    let mut children = parent.children.lock_irqsave();
    if children.contains_key(name) {
        return Err(crate::errno::Errno::FileExists.as_neg_i32());
    }

    let node = Arc::new(CgroupNode::new_child(
        name,
        parent.clone(),
        parent.level + 1,
        alloc_ino_base(),
    ));
    children.insert(name.to_string(), node.clone());
    Ok(node)
}

/// Remove a child cgroup (rmdir on cgroupfs).
///
/// Linux semantics: the cgroup must have no children and no member tasks
/// (EBUSY otherwise). The node allocation is intentionally leaked (see
/// module lifetime docs) so outstanding raw pointers stay valid.
pub fn cgroup_rmdir(parent: &Arc<CgroupNode>, name: &str) -> Result<(), i32> {
    let mut children = parent.children.lock_irqsave();
    let node = match children.get(name) {
        Some(n) => n.clone(),
        None => {
            return Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }
    };

    // Lock order parent → child.
    if !node.children.lock_irqsave().is_empty() {
        return Err(crate::errno::Errno::DeviceOrResourceBusy.as_neg_i32());
    }
    if !node.tasks.lock_irqsave().is_empty() {
        return Err(crate::errno::Errno::DeviceOrResourceBusy.as_neg_i32());
    }

    // A limited cgroup disappearing must not leave the tick-time walk
    // counting a dead node.
    if node.controllers.lock_irqsave().cpu_quota_ns.load(Ordering::Acquire) != LIMIT_MAX {
        CPU_LIMITED_NODES.fetch_sub(1, Ordering::AcqRel);
    }

    let removed = children.remove(name);
    // Keep the allocation alive forever: task cgroup_ptrs and cached
    // cgroupfs inodes may still reference it.
    if let Some(arc) = removed {
        core::mem::forget(arc);
    }
    Ok(())
}

/// Look up a direct child cgroup by name.
pub fn cgroup_lookup_child(parent: &Arc<CgroupNode>, name: &str) -> Option<Arc<CgroupNode>> {
    parent.children.lock_irqsave().get(name).cloned()
}

// ============================================================================
// Task membership
// ============================================================================

/// Task's cgroup as a raw node pointer read back as a shared reference.
///
/// SAFETY contract (callers may rely on it): the pointer was obtained from
/// an `Arc<CgroupNode>` that is never freed (rmdir leaks the allocation),
/// and it is never written through. NULL → implicit root / no limits.
#[inline]
pub fn task_cgroup(task: *const Task) -> Option<&'static CgroupNode> {
    let ptr = unsafe { (*task).cgroup_ptr() };
    if ptr.is_null() {
        None
    } else {
        // SAFETY: see contract above.
        Some(unsafe { &*ptr })
    }
}

/// Attach a task to a cgroup (sets cgroup_ptr and records the pid).
///
/// `node` must be a reference into an `Arc<CgroupNode>` allocation — the
/// address stored into the task is exactly `Arc::as_ptr` of that Arc.
fn attach_task(node: &CgroupNode, task: *mut Task, pid: u32) {
    let mut tasks = node.tasks.lock_irqsave();
    if !tasks.contains(&pid) {
        tasks.push(pid);
    }
    drop(tasks);
    // SAFETY: exclusive access to the child's field; pointer stays valid
    // forever (nodes are never freed), so no refcount is needed.
    unsafe {
        (*task).set_cgroup_ptr(node as *const CgroupNode as *mut CgroupNode);
    }
}

/// Detach a task from its cgroup (drops pid membership only; memory
/// accounting is settled separately).
fn detach_task(node: &CgroupNode, pid: u32) {
    let mut tasks = node.tasks.lock_irqsave();
    if let Some(pos) = tasks.iter().position(|&p| p == pid) {
        tasks.swap_remove(pos);
    }
}

/// Attach the boot init task (PID 1) to the root cgroup so its membership
/// and memory usage are tracked from the start. Kernel threads stay
/// cgroup_ptr = NULL (implicit root, unlimited — Linux kthreads bypass
/// cgroup limits too).
pub fn attach_init_to_root(task: *mut Task) {
    if let Some(root) = cgroup_root() {
        let pid = unsafe { (*task).pid() };
        // SAFETY: root is a live Arc for the whole boot.
        attach_task(unsafe { &*(Arc::as_ptr(&root) as *const CgroupNode) }, task, pid);
    }
}

/// fork hook: pids controller check + child inherits the parent's cgroup.
///
/// Linux checks the limit in every ancestor where the pids controller is
/// relevant and fails the fork with EAGAIN. Returns Ok(()) when the child
/// was attached.
pub fn cgroup_on_fork(parent: *mut Task, child: *mut Task) -> Result<(), i32> {
    let node = match task_cgroup(parent) {
        Some(n) => n,
        None => {
            // Parent is an unattached (kernel) task: the child inherits the
            // implicit-root status — nothing to check or record.
            return Ok(());
        }
    };

    // pids.max along the whole ancestor chain (self first).
    let mut cur = node;
    loop {
        {
            let controllers = cur.controllers.lock_irqsave();
            let max = controllers.pids_max.load(Ordering::Acquire);
            if max != LIMIT_MAX {
                drop(controllers);
                if cur.subtree_task_count() as u64 >= max {
                    return Err(crate::errno::Errno::TryAgain.as_neg_i32());
                }
            }
        }
        match cur.parent.as_ref() {
            Some(p) => {
                // SAFETY: nodes are never freed.
                cur = unsafe { &*(Arc::as_ptr(p) as *const CgroupNode) };
            }
            None => break,
        }
    }

    let child_pid = unsafe { (*child).pid() };
    attach_task(node, child, child_pid);
    Ok(())
}

/// Move a task (by pid) into `node` — the `echo <pid> > cgroup.procs`
/// operation.
///
/// Memory accounting follows the task: its mm's charged footprint is
/// subtracted from the old chain and added to the new chain so
/// memory.current stays consistent across migration.
pub fn cgroup_attach_pid(node: &Arc<CgroupNode>, pid: u32) -> Result<(), i32> {
    let task = crate::process::find_task_by_pid(pid)
        .ok_or(crate::errno::Errno::NoSuchProcess.as_neg_i32())? as *mut Task;

    // Ledger BEFORE moving (migration transfers the task's charge; the
    // new chain is charged without a limit check — Linux memcg migration
    // moves charges and cannot fail the cgroup.procs write here).
    let charged = unsafe { (*task).cgroup_mem_charged() };

    let old = task_cgroup(task);
    if let Some(old_node) = old {
        detach_task(old_node, pid);
        if charged > 0 {
            uncharge_bytes(old_node, charged);
        }
    }
    // SAFETY: node is a live Arc for the duration of the write.
    attach_task(unsafe { &*(Arc::as_ptr(node) as *const CgroupNode) }, task, pid);
    if charged > 0 {
        // SAFETY: see above.
        charge_bytes(unsafe { &*(Arc::as_ptr(node) as *const CgroupNode) }, charged);
    }
    Ok(())
}

/// exit hook: drop membership and settle memory accounting.
///
/// Called from do_exit while the task still lives; the uncharge is the
/// task's whole remaining ledger (Task::cgroup_mem_charged), so it is
/// exactly what was charged via mmap(2) — CLONE_VM threads that never
/// called mmap themselves carry a zero ledger and uncharge nothing.
pub fn cgroup_exit(task: *mut Task) {
    let node = match task_cgroup(task) {
        Some(n) => n,
        None => return,
    };
    let pid = unsafe { (*task).pid() };
    detach_task(node, pid);

    let charged = unsafe { (*task).cgroup_mem_charged() };
    if charged > 0 {
        uncharge_bytes(node, charged);
        // SAFETY: task is the exiting task; ledger reset is defensive.
        unsafe { (*task).sub_cgroup_mem_charged(charged) };
    }
}

// ============================================================================
// cpu controller
// ============================================================================

/// Charge one tick of cpu time into `node`'s ancestor chain and evaluate
/// the throttle condition. Returns true when the chain is (now) throttled.
fn cpu_charge_chain(node: &CgroupNode, delta_ns: u64) -> bool {
    let mut throttled = false;
    node.for_each_ancestor(|n| {
        n.cpu_usage_total_ns.fetch_add(delta_ns, Ordering::AcqRel);
        let controllers = n.controllers.lock_irqsave();
        let quota = controllers.cpu_quota_ns.load(Ordering::Acquire);
        if quota == LIMIT_MAX {
            return;
        }
        drop(controllers);
        let used = n.cpu_usage_period_ns.fetch_add(delta_ns, Ordering::AcqRel) + delta_ns;
        if used > quota && !n.cpu_throttled.swap(true, Ordering::AcqRel) {
            n.nr_throttled.fetch_add(1, Ordering::AcqRel);
        }
        if n.cpu_throttled.load(Ordering::Acquire) {
            throttled = true;
        }
    });
    throttled
}

/// Period rollover + unthrottle pass over the whole tree.
///
/// Runs only when at least one cgroup carries a cpu quota (the common
/// unlimited case costs one atomic load per tick). Any CPU's tick can
/// unthrottle a cgroup; the sleeping-on-runqueue tasks of an unthrottled
/// cgroup become pickable again immediately (the pick path only skips
/// throttled chains), so no explicit wakeup list is needed.
fn cpu_period_rollover(now: u64) -> bool {
    let mut unthrottled_any = false;

    fn walk(node: &CgroupNode, now: u64, unthrottled_any: &mut bool) {
        {
            let controllers = node.controllers.lock_irqsave();
            let quota = controllers.cpu_quota_ns.load(Ordering::Acquire);
            let period = controllers.cpu_period_ns.load(Ordering::Acquire).max(1);
            if quota != LIMIT_MAX {
                let start = node.cpu_period_start_ns.load(Ordering::Acquire);
                if now.saturating_sub(start) >= period {
                    // New period: reset usage, move the window.
                    node.cpu_period_start_ns.store(now, Ordering::Release);
                    node.cpu_usage_period_ns.store(0, Ordering::Release);
                    if node.cpu_throttled.swap(false, Ordering::AcqRel) {
                        node.throttled_ns
                            .fetch_add(now.saturating_sub(start), Ordering::AcqRel);
                        *unthrottled_any = true;
                    }
                }
            }
        }
        let children: Vec<Arc<CgroupNode>> = {
            let guard = node.children.lock_irqsave();
            guard.values().cloned().collect()
        };
        for c in children {
            // SAFETY: nodes are never freed.
            walk(unsafe { &*(Arc::as_ptr(&c) as *const CgroupNode) }, now, unthrottled_any);
        }
    }

    if let Some(root) = cgroup_root() {
        // SAFETY: nodes are never freed.
        walk(unsafe { &*(Arc::as_ptr(&root) as *const CgroupNode) }, now, &mut unthrottled_any);
    }
    unthrottled_any
}

/// scheduler_tick hook: charge the current task's cgroup chain, roll
/// periods over, and report whether the current task is now throttled
/// (the caller then sets need_resched so the CPU switches away).
///
/// Kernel threads / unattached tasks (cgroup_ptr NULL) are skipped — they
/// are implicitly root and the root carries no quota in practice.
pub fn cgroup_cpu_tick(current: *mut Task) -> bool {
    if CPU_LIMITED_NODES.load(Ordering::Acquire) == 0 {
        return false;
    }

    let now = crate::sched::fair::sched_clock();
    let unthrottled = cpu_period_rollover(now);

    let throttled = match task_cgroup(current) {
        Some(node) => cpu_charge_chain(node, CGROUP_TICK_NS),
        None => false,
    };

    // Throttled → the CPU must switch away. Unthrottled → some runqueue
    // may now hold pickable tasks that were being skipped; ask for a
    // resched so they get reconsidered.
    throttled || unthrottled
}

/// Pick-path check: is this task's cgroup chain currently cpu-throttled?
/// Lock-free; called from fair/rt pick_next_cpu under the GRQ lock.
pub fn task_cgroup_throttled(task: *const Task) -> bool {
    match task_cgroup(task) {
        Some(node) => node.chain_throttled(),
        None => false,
    }
}

/// Account for a cpu.max write (fs::cgroup): track limited-node count and
/// refresh the period window.
pub fn note_cpu_limit_changed(node: &Arc<CgroupNode>, was_limited: bool, now_limited: bool) {
    if was_limited && !now_limited {
        CPU_LIMITED_NODES.fetch_sub(1, Ordering::AcqRel);
    } else if !was_limited && now_limited {
        CPU_LIMITED_NODES.fetch_add(1, Ordering::AcqRel);
        let now = crate::sched::fair::sched_clock();
        // SAFETY: nodes are never freed.
        let n = unsafe { &*(Arc::as_ptr(node) as *const CgroupNode) };
        n.cpu_period_start_ns.store(now, Ordering::Release);
        n.cpu_usage_period_ns.store(0, Ordering::Release);
    }
}

// ============================================================================
// memory controller
// ============================================================================

/// Charge `bytes` into `node`'s ancestor chain (memory.current semantics:
/// ancestors see descendant usage). Returns false if a limit would be
/// exceeded (the charge is rolled back).
fn charge_bytes(node: &CgroupNode, bytes: u64) -> bool {
    // Charge every level first...
    node.for_each_ancestor(|n| {
        n.memory_usage_bytes.fetch_add(bytes, Ordering::AcqRel);
    });
    // ...then verify every level, rolling back on failure.
    let mut over = false;
    node.for_each_ancestor(|n| {
        let controllers = n.controllers.lock_irqsave();
        let max = controllers.memory_max_bytes.load(Ordering::Acquire);
        drop(controllers);
        if max != LIMIT_MAX && n.memory_usage_bytes.load(Ordering::Acquire) > max {
            over = true;
        }
    });
    if over {
        uncharge_bytes(node, bytes);
        return false;
    }
    true
}

/// Uncharge `bytes` from `node`'s ancestor chain (saturating at zero —
/// defense in depth against any accounting imbalance).
fn uncharge_bytes(node: &CgroupNode, bytes: u64) {
    node.for_each_ancestor(|n| {
        let mut current = n.memory_usage_bytes.load(Ordering::Acquire);
        loop {
            let next = current.saturating_sub(bytes);
            match n.memory_usage_bytes.compare_exchange(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    });
}

/// mmap hook: charge the current task's cgroup chain for a new mapping
/// AND record the charge in the task's ledger (Task::cgroup_mem_charged)
/// so every future uncharge is exactly balanced.
/// Returns false when memory.max would be exceeded (caller fails with
/// ENOMEM). Unattached tasks (kernel) are unlimited.
pub fn cgroup_memory_charge(bytes: usize) -> bool {
    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return true,
    };
    let bytes = bytes as u64;
    match task_cgroup(current) {
        Some(node) => {
            if charge_bytes(node, bytes) {
                // SAFETY: current is this CPU's running task.
                unsafe { (*current).add_cgroup_mem_charged(bytes) };
                true
            } else {
                false
            }
        }
        None => true,
    }
}

/// mmap-failure / munmap hook: give `bytes` back to the current task's
/// cgroup chain and its ledger.
pub fn cgroup_memory_uncharge(bytes: usize) {
    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return,
    };
    match task_cgroup(current) {
        Some(node) => {
            uncharge_bytes(node, bytes as u64);
            // SAFETY: current is this CPU's running task.
            unsafe { (*current).sub_cgroup_mem_charged(bytes as u64) };
        }
        None => {}
    }
}

/// Snapshot helpers for the cgroupfs presentation layer.
pub fn node_memory_current(node: &CgroupNode) -> u64 {
    node.memory_usage_bytes.load(Ordering::Acquire)
}

/// Parse helpers shared by the cgroupfs write path.
///
/// Parse a whitespace-separated controller list like "+cpu +memory -pids"
/// into a (set_bits, clear_bits) pair. Unknown names → None (EINVAL).
pub fn parse_subtree_control_tokens(input: &str) -> Option<(u32, u32)> {
    let mut set = 0u32;
    let mut clear = 0u32;
    for token in input.split_whitespace() {
        let (sign, name) = match token.as_bytes().first() {
            Some(b'+') => (true, &token[1..]),
            Some(b'-') => (false, &token[1..]),
            _ => return None,
        };
        let bit = match name {
            "cpu" => CTRL_CPU,
            "memory" => CTRL_MEMORY,
            "pids" => CTRL_PIDS,
            _ => return None,
        };
        if sign {
            set |= bit;
        } else {
            clear |= bit;
        }
    }
    Some((set, clear))
}

/// Format a subtree_control bitset as a space-separated name list.
pub fn format_subtree_control(bits: u32) -> String {
    let mut s = String::new();
    for (bit, name) in [
        (CTRL_CPU, "cpu"),
        (CTRL_MEMORY, "memory"),
        (CTRL_PIDS, "pids"),
    ] {
        if bits & bit != 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(name);
        }
    }
    s
}
