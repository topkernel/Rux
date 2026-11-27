//! cgroup v2 policy operations (shrink stage, user method 2026-11-26).
//!
//! Moved out of the core's `sched/hotpath.rs` cgroup section as the
//! measured-cold set: zero samples in the x86/KVM pbench-sampling
//! baseline (run/baseline/report.txt), and no core-side caller by name.
//! Callers live in service crates (fs cgroupfs presentation layer,
//! syscall mmap/munmap hooks) that reach them with direct sibling-crate
//! calls — the free direction (crate->crate direct; the interface-table
//! cost only exists for core->crate). The tick-facing charge path
//! (cgroup_cpu_tick / cpu_charge_chain / cpu_period_rollover) and the
//! task-membership core hooks (task_cgroup, attach_task, detach_task,
//! cgroup_on_fork, cgroup_exit, attach_init_to_root) stay in the core:
//! the scheduler tick calls them by name, and the process crate (which
//! cannot depend on this crate without a cycle) calls cgroup_on_fork /
//! cgroup_exit.
//!
//! Types (CgroupNode, CgroupControllers) and their inherent impls stay
//! framework-side in sched/types.rs + hotpath.rs (same-crate rule for
//! inherent impls); every field is pub, so the walks below read them
//! directly, exactly as before the move.
//!
//! One file per feature point (user order 2026-11-26): this file is the
//! hierarchy/memory/policy operations consumed by cgroupfs and the mmap
//! charge hooks.
#![forbid(unsafe_code)]

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::process::types::Task;
use crate::sched::hotpath::{
    attach_task, cgroup_root, current, detach_task, sched_clock, task_cgroup, uncharge_bytes,
    CGROUP_ROOT_CELL, LIMIT_MAX,
};
use crate::sched::r#unsafe::{task_borrow, task_shared};
use crate::sched::types::CgroupNode;

// ============================================================================
// Inode-number allocation
// ============================================================================

/// Next inode base allocator (root gets 1).
static NEXT_INO_BASE: AtomicU64 = AtomicU64::new(1 + CGROUP_INO_STRIDE);

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
// Task migration (the cgroup.procs write path)
// ============================================================================

/// Move a task (by pid) into `node` — the `echo <pid> > cgroup.procs`
/// operation.
///
/// Memory accounting follows the task: its mm's charged footprint is
/// subtracted from the old chain and added to the new chain so
/// memory.current stays consistent across migration.
pub fn cgroup_attach_pid(node: &Arc<CgroupNode>, pid: u32) -> Result<(), i32> {
    let task = crate::process::hotpath::find_task_by_pid(pid)
        .ok_or(crate::errno::Errno::NoSuchProcess.as_neg_i32())? as *mut crate::process::types::Task;

    // Ledger BEFORE moving (migration transfers the task's charge; the
    // new chain is charged without a limit check — Linux memcg migration
    // moves charges and cannot fail the cgroup.procs write here).
    // SAFETY (one adopt edge): live task handle for this context.
    let charged = crate::sched::r#unsafe::task_borrow(task)
        .map(|t| t.cgroup_mem_charged())
        .unwrap_or(0);

    let old = task_cgroup(task);
    if let Some(old_node) = old {
        crate::sched::hotpath::detach_task(old_node, pid);
        if charged > 0 {
            uncharge_bytes(old_node, charged);
        }
    }
    // SAFETY: node is a live Arc for the duration of the write.
    attach_task(node, task, pid);
    if charged > 0 {
        // SAFETY: see above.
        charge_bytes(node, charged);
    }
    Ok(())
}

// ============================================================================
// cpu controller policy
// ============================================================================

/// Pick-path check: is this task's cgroup chain currently cpu-throttled?
/// Lock-free; called from fair/rt pick_next_cpu under the GRQ lock.
/// Lives HERE (not the core): its only callers are this crate's pick
/// policy files — the call stays direct (crate-local) after the move;
/// task_cgroup itself stays framework-side (the core's exit/attach
/// hooks call it by name).
pub fn task_cgroup_throttled(task: *const crate::process::types::Task) -> bool {
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
        let now = crate::sched::hotpath::sched_clock();
        // SAFETY: nodes are never freed.
        let n = node;
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

/// mmap hook: charge the current task's cgroup chain for a new mapping
/// AND record the charge in the task's ledger (Task::cgroup_mem_charged)
/// so every future uncharge is exactly balanced.
/// Returns false when memory.max would be exceeded (caller fails with
/// ENOMEM). Unattached tasks (kernel) are unlimited.
pub fn cgroup_memory_charge(bytes: usize) -> bool {
    let current = match crate::sched::hotpath::current() {
        Some(c) => c as *mut crate::process::types::Task,
        None => return true,
    };
    let bytes = bytes as u64;
    match task_cgroup(current) {
        Some(node) => {
            if charge_bytes(node, bytes) {
                // SAFETY: current is this CPU's running task.
                task_shared(current).add_cgroup_mem_charged(bytes);
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
    let current = match crate::sched::hotpath::current() {
        Some(c) => c as *mut crate::process::types::Task,
        None => return,
    };
    match task_cgroup(current) {
        Some(node) => {
            uncharge_bytes(node, bytes as u64);
            // SAFETY: current is this CPU's running task.
            task_shared(current).sub_cgroup_mem_charged(bytes as u64);
        }
        None => {}
    }
}

/// sc126 (Session P): one-line attribution when cgroup memory.max refuses
/// an mmap charge — names the limiting level's usage vs max so the
/// virtual-vs-RSS charge semantics question is decidable from the log.
pub fn debug_report_memory_refusal(comm: &str, len: usize) {
    let current = match crate::sched::hotpath::current() {
        Some(c) => c as *mut crate::process::types::Task,
        None => return,
    };
    let node = match task_cgroup(current) {
        Some(n) => n,
        None => return,
    };
    let mut levels = alloc::string::String::new();
    use core::fmt::Write;
    let mut first = true;
    node.for_each_ancestor(|n| {
        let controllers = n.controllers.lock_irqsave();
        let max = controllers.memory_max_bytes.load(Ordering::Acquire);
        drop(controllers);
        let usage = n.memory_usage_bytes.load(Ordering::Acquire);
        let _ = write!(
            levels,
            "{}usage={:#x}max={:#x}",
            if first { "" } else { " | " },
            usage,
            max
        );
        first = false;
    });
    crate::pr_warn!(
        "mmap-enomem: CGROUP comm='{}' len={:#x} chain: {}",
        comm,
        len,
        levels
    );
}

// ============================================================================
// cgroupfs presentation helpers
// ============================================================================

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


pub fn init_cgroup() -> Result<(), i32> {
    let mut guard = CGROUP_ROOT_CELL.lock();
    if guard.is_none() {
        *guard = Some(Arc::new(CgroupNode::new_root()));
    }
    Ok(())
}

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
            walk(&c, now, unthrottled_any);
        }
    }

    if let Some(root) = cgroup_root() {
        // SAFETY: nodes are never freed.
        walk(&root, now, &mut unthrottled_any);
    }
    unthrottled_any
}

pub const CGROUP_TICK_NS: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);

pub const CTRL_CPU: u32 = 1 << 0;

pub const CTRL_MEMORY: u32 = 1 << 1;

pub const CTRL_PIDS: u32 = 1 << 2;

pub const ALL_CONTROLLERS: u32 = CTRL_CPU | CTRL_MEMORY | CTRL_PIDS;

pub const CONTROLLER_NAMES: [&str; 3] = ["cpu", "memory", "pids"];

pub const CGROUP_INO_STRIDE: u64 = 32;

pub static CPU_LIMITED_NODES: AtomicU64 = AtomicU64::new(0);

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
                cur = p;
            }
            None => break,
        }
    }

    // SAFETY (one adopt edge): the fork hook's just-built child (live,
    // pre-enqueue — exclusive by construction).
    let child_pid = crate::sched::r#unsafe::task_borrow(child)
        .map(|t| t.pid())
        .ok_or(crate::errno::Errno::TryAgain.as_neg_i32())?;
    attach_task(node, child, child_pid);
    Ok(())
}

pub fn cgroup_exit(task: *mut Task) {
    let node = match task_cgroup(task) {
        Some(n) => n,
        None => return,
    };
    let pid = crate::sched::r#unsafe::task_shared(task).pid();
    detach_task(node, pid);

    // SAFETY (one adopt edge): live task handle for this context.
    let charged = crate::sched::r#unsafe::task_borrow(task)
        .map(|t| t.cgroup_mem_charged())
        .unwrap_or(0);
    if charged > 0 {
        uncharge_bytes(node, charged);
        // SAFETY: task is the exiting task; ledger reset is defensive.
        crate::sched::r#unsafe::task_shared(task).sub_cgroup_mem_charged(charged);
    }
}

pub fn cgroup_cpu_tick(current: *mut Task) -> bool {
    if CPU_LIMITED_NODES.load(Ordering::Acquire) == 0 {
        return false;
    }

    let now = crate::sched::hotpath::sched_clock();
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
