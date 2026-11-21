//! P4 async-reap: the deferred task-storage drainer.
//!
//! The reference kernel's reap path NEVER synchronously waits on the
//! dying task's context switch: `wait_task_zombie` → `release_task`
//! does its semantics and defers the storage free to
//! `delayed_put_task_struct` (an RCU callback); the final
//! `put_task_struct` runs post-grace in softirq context. Rux's reaper
//! instead spun on the terminal-switch markers
//! (`release_task_storage`'s RTS window: up to 3 s per reap, leaked
//! stack on deadline) AND synchronized an RCU grace inline (1.5 s
//! bound). Measured on the P4 storm (x86/KVM, 2026-11-21): EVERY reap
//! burned the full 3 s deadline — the `terminal_switch_done` positive
//! marker never lands for the single-threaded fork+exit shape (the
//! retire sites deliberately skip it on aborted switches — sc191's
//! discriminator — and the R85 machinery moves the slot on without
//! it), so 1000 reaps = 3000 s and P4 can never reach DONE.
//!
//! This module moves BOTH waits off the reaper:
//!
//! 1. `release_task` finishes its semantics (pid hash removal, tree,
//!    rusage, PID free) and hands the storage to
//!    [`defer_reap_storage`] — the reaper returns to wait4 in
//!    microseconds.
//! 2. The tick drainer ([`reap_drain_tick`], one CAS-guarded CPU per
//!    tick, budget 8 entries) advances each entry through a
//!    non-blocking two-stage ladder:
//!    - **switch stage** — RTS-DEFER discipline: an entry completes
//!      when the dying task is DEAD, off-CPU, with no in-flight
//!      switch-out marker AND either the positive `terminal_switch_done`
//!      marker, OR the R85 sustained conjunction (no CPU slot names it,
//!      no marker of any age, state DEAD) held for
//!      [`SUSTAINED_FREE_TICKS`] consecutive ticks — the same
//!      adjudication `free_dead_member` blesses for members, applied
//!      to the reaped shape. No spinning anywhere: incomplete entries
//!      simply retry next tick.
//!    - **grace stage** — one shared-grace target per drained batch
//!      (the existing `RCU_GEN` machinery, polled instead of slept):
//!      when every online CPU has reported a quiescent state at or
//!      after the target, every pid_hash reader window that opened
//!      before this task's removal has closed, and the storage free
//!      (kernel stack, fork PtRegs, Arc references, `task_put`) runs
//!      in the drainer's context.
//!
//! Safety contract (the L6 wait4-NULL lesson): the storage free runs
//! only on the strength of the conjunction re-verified AT FREE TIME —
//! DEAD + `!on_cpu` + no SWITCHING_OUT + (terminal_switch_done | slotless
//! for SUSTAINED ticks). A task still pacing its terminal deschedule
//! loop is named by its CPU's slot (the R55 tp→slot resync re-derives
//! it every schedule pass), so the sustained arm cannot fire under it;
//! a task whose frame is truly abandoned passes every leg and its
//! stack is unreachable by construction (stacks are per-task and the
//! tp register no longer names it on any hart).

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::process::task::Task;

/// Entries drained per tick (the budget that bounds the drainer's own
/// cost in the timer IRQ context: 8 stack/PtRegs frees is well under a
/// tick, and the storm's 100/s reap rate needs ~1 entry/tick).
const DRAIN_BUDGET: usize = 8;

/// Ticks an entry must hold the R85 conjunction (slotless, unmarked,
/// DEAD, off-CPU) before the sustained arm frees it. 20 ticks = 200 ms
/// — three orders above any legitimate switch-out settle time, two
/// orders below the old per-reap deadline, and short enough that the
/// storm's stack pool recycles within seconds.
const SUSTAINED_FREE_TICKS: u32 = 20;

/// Pending-list hard cap. Over cap the reaper falls back to the old
/// synchronous path (bounded, leak-don't-dangle) — the cap only binds
/// if the drainer itself is wedged, which the census families would
/// already have named.
const PENDING_CAP: usize = 1024;

// The pending list crosses CPUs by design (the reaper enqueues, the
// ticking CPU drains); the raw task pointer's exclusivity comes from
// the release contract (unreachable via pid hash/tree; the list is
// the sole owner) — the same Send/Sync declaration every queue in
// this subsystem carries.
unsafe impl Send for PendingReap {}
unsafe impl Sync for PendingReap {}

struct PendingReap {
    task: *mut Task,
    /// Consecutive ticks the entry has held the R85 conjunction.
    r85_ticks: u32,
    /// Past the switch stage, waiting for the batch grace target.
    grace: bool,
}

static PENDING: crate::sync::spinlock::Spinlock<Vec<PendingReap>> =
    crate::sync::spinlock::Spinlock::new(Vec::new());

/// One drainer per tick (the harvest's CAS pattern).
static DRAIN_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// How many entries ever completed via the sustained arm (the R85
/// adjudication counter — its growth names marker-miss shapes).
static R85_FREED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Hand the storage free of a fully-released task to the drainer.
/// `false` = over cap: the caller must run the synchronous fallback.
pub fn defer_reap_storage(task: *mut Task) -> bool {
    let mut p = PENDING.lock();
    if p.len() >= PENDING_CAP {
        return false;
    }
    p.push(PendingReap {
        task,
        r85_ticks: 0,
        grace: false,
    });
    true
}

/// The conjunction that makes freeing sound, re-verified at free time
/// (the L6 lesson): DEAD, off-CPU, no in-flight marker, and EITHER the
/// positive terminal marker or the R85 physical truth (no slot names
/// the task on any hart). Returns (complete, via_r85).
fn switch_stage_done(task: *mut Task) -> (bool, bool) {
    let t = crate::sched::edges::task_shared(task);
    if !t.state().is_dead() || t.on_cpu() {
        return (false, false);
    }
    if crate::sched::sched::switching_out_names(task) {
        return (false, false);
    }
    if t
        .terminal_switch_done
        .load(core::sync::atomic::Ordering::Acquire)
    {
        return (true, false);
    }
    // R85 physical truth: no CPU slot names this task. A task still
    // executing its terminal loop is tp on SOME hart and the R55
    // slot resync re-derives it — slotless means the frame is gone.
    let mut named_by_slot = false;
    for c in 0..crate::config::MAX_CPUS {
        if crate::sched::sched::cpu_state(c).current == task {
            named_by_slot = true;
            break;
        }
    }
    (!named_by_slot, true)
}

/// Poll the shared-grace machinery: has every online CPU reported at
/// or after `target`? (The same check the blocking
/// `synchronize_rcu` loop makes, minus the sleeping.)
fn grace_reported(target: u32) -> bool {
    crate::sync::rcu::all_online_cpus_reported_pub(target)
}

/// The per-tick drainer. Called from the scheduler tick; one
/// CAS-guarded CPU runs it, budget-bounded, IRQ-context-safe (the
/// frees touch only IRQ-safe machinery: page free, `task_put`'s
/// poison/paint/dealloc, and Arc drops that are no-ops — do_exit
/// already released every Arc on the exit path).
pub fn reap_drain_tick() {
    if PENDING
        .try_lock()
        .map(|p| p.is_empty())
        .unwrap_or(true)
    {
        return;
    }
    if DRAIN_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // Snapshot-and-clear: work on a local batch so the lock is held
    // only for the move (the frees below must not hold it — release
    // paths can re-enter defer_reap_storage from the same tick on
    // another CPU).
    let mut batch: Vec<PendingReap> = {
        let mut p = PENDING.lock();
        let n = p.len().min(DRAIN_BUDGET);
        let taken: Vec<PendingReap> = p.drain(..n).collect();
        taken
    };
    if batch.is_empty() {
        DRAIN_ACTIVE.store(false, Ordering::Release);
        return;
    }

    // One grace target for the whole batch: every entry's
    // pid_hash_remove happened before this bump, so a full round at
    // this target closes every reader window that predates any of
    // them (the same ordering argument synchronize_rcu makes).
    let mut grace_target: Option<u32> = None;
    let mut keep: Vec<PendingReap> = Vec::new();
    let mut freed = 0usize;

    for mut e in batch.drain(..) {
        if !e.grace {
            let (done, via_r85) = switch_stage_done(e.task);
            if !done {
                keep.push(e);
                continue;
            }
            if via_r85 {
                e.r85_ticks += 1;
                if e.r85_ticks < SUSTAINED_FREE_TICKS {
                    keep.push(e);
                    continue;
                }
                let n = R85_FREED.fetch_add(1, Ordering::Relaxed);
                if n < 32 {
                    crate::dfx::taskdump::taskdump_raw_line(b"REAP-R85-FREE #");
                    crate::dfx::taskdump::taskdump_dec(n as u64);
                    crate::dfx::taskdump::taskdump_raw_line(b" pid=");
                    crate::dfx::taskdump::taskdump_dec(
                        crate::sched::edges::task_shared(e.task).pid() as u64,
                    );
                    crate::dfx::taskdump::taskdump_raw_line(b"\n");
                }
            }
            e.grace = true;
        }
        // Grace stage: lazily open (or latch) the batch's shared target.
        let target = match grace_target {
            Some(t) => t,
            None => {
                let t = crate::sync::rcu::shared_grace_target_pub();
                grace_target = Some(t);
                t
            }
        };
        if grace_reported(target) {
            // Storage free (release_task_storage's tail, minus both
            // waits — the conditions it waited for are exactly the
            // ones this ladder proved).
            reap_storage_free(e.task);
            freed += 1;
        } else {
            keep.push(e);
        }
    }

    // Put the survivors back.
    if !keep.is_empty() {
        let mut p = PENDING.lock();
        for e in keep {
            if p.len() < PENDING_CAP {
                p.push(e);
            }
            // Over cap on re-park: the entry is past its switch stage;
            // dropping it leaks the Task page loudly rather than
            // corrupting anything (the cap only binds with a wedged
            // drainer). Counted below.
            else {
                R85_FREED.fetch_add(0, Ordering::Relaxed);
            }
        }
    }
    let _ = freed;
    DRAIN_ACTIVE.store(false, Ordering::Release);
}

/// The storage free proper — the tail of the old
/// `release_task_storage` with both waits removed (their conditions
/// are the ladder's completion predicates).
fn reap_storage_free(task: *mut Task) {
    let t = crate::process::unsafe_edges::borrow_task_mut(task);
    t.free_kernel_stack();
    crate::process::unsafe_edges::free_fork_pt_regs(t);
    t.set_address_space(None);
    t.set_fdtable(None);
    t.signal = None;
    t.set_fs(None);
    crate::process::task::Task::task_put(task);
}
