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
#![forbid(unsafe_code)]

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

/// ReapClaim — the linearly-owned storage-free obligation for one
/// unreachable task (Round-B pilot; the design page in run/UR-L1.md).
///
/// **MINT** — [`ReapClaim::mint`], exactly once per reap, from
/// `release_task` AFTER pid_hash_remove + tree detach (the
/// unreachability contract): from that moment nobody else can or will
/// free this task's storage — the claim IS that obligation as a type.
///
/// **BURN** — [`ReapClaim::into_free`], consumes `self`, runs the
/// storage-free window. Exactly-once is compiler-enforced: the claim
/// is not `Clone`/`Copy` and `into_free` takes it by value.
///
/// **READS** — the ladder's checks go through [`ReapClaim::task`]
/// (shared) and [`ReapClaim::as_ptr`] (the identity comparisons
/// against per-CPU slots): no raw task pointer escapes this module.
///
/// **DROP** — the over-cap leak path (explicit, counted in
/// [`CLAIM_DROPPED`]): a dropped claim leaks the Task page loudly,
/// exactly the pre-pilot over-cap semantics.
///
/// The page's own lifecycle (alloc/poison/dealloc, INV-TL-1/2) is
/// UNCHANGED — the claim types the free-obligation, not the page.
/// Identity preservation: `as_ptr()` returns the minted pointer,
/// always; the slot comparison and `into_free`'s body are
/// byte-identical to the raw-pointer form they replace (the
/// corruption-battery obligation).
pub struct ReapClaim(core::ptr::NonNull<Task>);

/// Claims ever dropped unburned (the over-cap leak counter — nonzero
/// growth names a wedged drainer, same as the pre-pilot semantics).
static CLAIM_DROPPED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

// The Send commitment moved to the subsystem's one audited unsafe
// file (crate::sched::edges, unsafe.rs) — mainline consolidation: the
// claim crosses CPUs by design (reaper mints, the ticking CPU burns);
// the exclusivity is the linear type itself plus the unreachability
// contract minted on.

impl ReapClaim {
    /// Mint the storage-free obligation for `task`.
    ///
    /// # Contract (the mint side of the pilot)
    /// `task` is non-null and UNREACHABLE: pid_hash_remove and the
    /// tree detach already ran (the caller is `release_task`'s tail);
    /// no slot, queue, or tp register will name it again except
    /// transiently through the ladder's re-verified checks. `None`
    /// means null input (caller bug — the release path never passes
    /// one).
    pub(crate) fn mint(task: *mut Task) -> Option<Self> {
        // SAFETY: NonNull::new is the null check; the unreachability
        // contract is the caller's (documented above, one call site).
        core::ptr::NonNull::new(task).map(ReapClaim)
    }

    /// The identity (bit-identical to the minted pointer, always).
    #[inline]
    pub fn as_ptr(&self) -> *mut Task {
        self.0.as_ptr()
    }

    /// Shared read view for the ladder's checks (the INV-SCE-2
    /// contract: unreachable-but-mapped; the claim keeps the page
    /// alive by postponing the free).
    #[inline]
    fn task(&self) -> &Task {
        // The claim owns the free; until into_free runs the page is
        // mapped and immutable to everyone else (DEAD, unlinked) —
        // the same argument every task_shared caller makes, now with
        // the obligation typed. Crosses the audited INV-SCE-2 edge.
        crate::sched::edges::task_shared(self.0.as_ptr())
    }

    /// The channel boundary: hand the obligation back to a
    /// lock-typed raw channel (the dead_threads lists). The re-park
    /// exits of the sweep/fallback paths use this — the parked channel
    /// is lock-disciplined raw (typing IT needs the Task field change
    /// in the process lane's active zone; see the coverage map).
    #[inline]
    pub(crate) fn into_parked(self) -> *mut Task {
        let p = self.0.as_ptr();
        core::mem::forget(self);
        p
    }

    /// Burn: consume the obligation and run the storage free.
    /// Byte-identical to the raw form's `reap_storage_free` (the
    /// identity-preservation obligation).
    fn into_free(self) -> (u32, u32) {
        let task = self.0.as_ptr();
        core::mem::forget(self);
        reap_storage_free(task)
    }
}

struct PendingReap {
    claim: ReapClaim,
    /// The parent's pid at reap time (boot-118's WAIT4-PARK fix: a
    /// parent parked in wait4 on THIS child gets a deferred-notify
    /// wake when the drainer completes the child's storage — the
    /// reaper-side notify already ran in do_exit, but a racing park
    /// could have missed it; this is the belt-and-braces arm, 0 = no
    /// parent known). Captured at MINT (release_task still knows the
    /// tree), consumed at BURN.
    parent_pid: u32,
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

// ---- Grace-batch state (the boot-42/63 redesign, 2026-11-21) ----
//
// The original grace stage had two protocol bugs that made the round
// NEVER complete under load: (1) it never held the RCU waiter count,
// and rcu_note_context_switch SUPPRESSES its QS store when
// RCU_WAITERS == 0 — a poller that nobody counts starves its own
// round; (2) it never released GP_IN_FLIGHT on completion, so every
// target after the first only ever latched the finished generation.
// Fix: a PERSISTENT grace batch (entries that passed the switch stage
// wait here across ticks), the waiter count held while the batch is
// non-empty, done+release on completion, and an expedited nudge
// (resched IPIs to the lagging CPUs — the reference's
// synchronize_rcu_expedited shape) when a round stalls.

/// The in-flight grace batch (switch-stage graduates awaiting grace).
static GRACE_BATCH: crate::sync::spinlock::Spinlock<Vec<PendingReap>> =
    crate::sync::spinlock::Spinlock::new(Vec::new());
/// The batch's target generation (0 = none open).
static GRACE_TARGET: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Consecutive drainer runs the current target has been unmet.
static GRACE_STALL_RUNS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Waiter-count held (idempotent hold/release via compare).
static GRACE_WAITER_HELD: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Nudge threshold: runs of an unmet target before the IPIs fire
/// (3 ticks = 30 ms — far above a normal round's completion).
const GRACE_NUDGE_RUNS: u32 = 3;

/// How many entries ever completed via the sustained arm (the R85
/// adjudication counter — its growth names marker-miss shapes).
static R85_FREED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// wedge-hunt diagnostic: drainer samples blocked ONLY by the pacing
/// gate (DEAD+off-CPU+unmarked but terminal_pacing still set).
pub static PACED_BLOCKED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Hand the storage free of a fully-released task to the drainer.
/// `false` = over cap: the caller must run the synchronous fallback.
pub fn defer_reap_storage(task: *mut Task, parent_pid: u32) -> bool {
    // The mint: the obligation leaves the reaper's hands here. On
    // over-cap return the OBLIGATION stays with the caller (the raw
    // fallback frees it) — the claim is not minted.
    let claim = match ReapClaim::mint(task) {
        Some(c) => c,
        None => return false,
    };
    let mut p = PENDING.lock();
    if p.len() >= PENDING_CAP {
        // Un-mint: hand the obligation back to the caller by
        // returning false; the claim must not drop (that would leak
        // what the fallback is about to free). into_free is the only
        // exit — so burn it here into the raw form the fallback
        // expects. (The mint/burn pair at the cap boundary is the
        // one place the raw form resurfaces, by design.)
        core::mem::forget(claim);
        return false;
    }
    p.push(PendingReap {
        claim,
        parent_pid,
        r85_ticks: 0,
        grace: false,
    });
    true
}

/// The conjunction that makes freeing sound, re-verified at free time
/// (the L6 lesson): DEAD, off-CPU, no in-flight marker, and EITHER the
/// positive terminal marker or the R85 physical truth (no slot names
/// the task on any hart). Returns (complete, via_r85).
fn switch_stage_done(claim: &ReapClaim) -> (bool, bool) {
    let task = claim.as_ptr();
    let t = claim.task();
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
    // boot-63 residue: the PACING gate — a task still executing its
    // terminal deschedule loop publishes positive knowledge
    // (terminal_pacing, set at loop entry, cleared only at the real
    // retire sites). Its abort-retry windows can hold the whole
    // slotless+unmarked+!on_cpu conjunction while the page must not be
    // freed; the 20-tick timing bet is no longer the discriminator.
    if t
        .terminal_pacing
        .load(core::sync::atomic::Ordering::Acquire)
    {
        // wedge-hunt diagnostic: count the samples whose ONLY failing
        // leg is the pacing gate. Nonzero growth while tsd stays 0
        // names "entered the terminal loop, never switched out".
        PACED_BLOCKED.fetch_add(1, Ordering::Relaxed);
        return (false, false);
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

/// The per-tick drainer. Called from the scheduler tick; one
/// CAS-guarded CPU runs it, budget-bounded, IRQ-context-safe (the
/// frees touch only IRQ-safe machinery: page free, `task_put`'s
/// poison/paint/dealloc, and Arc drops that are no-ops — do_exit
/// already released every Arc on the exit path).
pub fn reap_drain_tick() {
    // Fast path: nothing pending AND no open grace batch.
    let pend_empty = PENDING
        .try_lock()
        .map(|p| p.is_empty())
        .unwrap_or(true);
    let batch_empty = GRACE_BATCH
        .try_lock()
        .map(|b| b.is_empty())
        .unwrap_or(true);
    if pend_empty && batch_empty {
        return;
    }
    if DRAIN_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // Adaptive budget (the backpressure that self-heals cap pressure):
    // the nominal 8 grows with pending depth so a full list drains at
    // 4x-8x rate instead of spilling to the fallback.
    let depth = PENDING.try_lock().map(|p| p.len()).unwrap_or(0);
    let budget = if depth > (PENDING_CAP * 3) / 4 {
        DRAIN_BUDGET * 8
    } else if depth > PENDING_CAP / 2 {
        DRAIN_BUDGET * 4
    } else {
        DRAIN_BUDGET
    };
    // wedge-hunt diagnostic: name the pacing-gate blocker growth and
    // the pending depth on a budgeted cadence.
    static DIAG_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let dn = DIAG_N.fetch_add(1, Ordering::Relaxed);
    if dn < 8 || dn % 512 == 0 {
        let paced = PACED_BLOCKED.load(Ordering::Relaxed);
        let depth = depth;
        crate::dfx::taskdump::taskdump_raw_line(b"REAP-DIAG run=");
        crate::dfx::taskdump::taskdump_dec(dn as u64);
        crate::dfx::taskdump::taskdump_raw_line(b" paced=");
        crate::dfx::taskdump::taskdump_dec(paced as u64);
        crate::dfx::taskdump::taskdump_raw_line(b" depth=");
        crate::dfx::taskdump::taskdump_dec(depth as u64);
        crate::dfx::taskdump::taskdump_raw_line(b" r85=");
        crate::dfx::taskdump::taskdump_dec(R85_FREED.load(Ordering::Relaxed) as u64);
        crate::dfx::taskdump::taskdump_raw_line(b"\n");
    }
    // Snapshot-and-clear of the switch-stage candidates.
    let mut batch: Vec<PendingReap> = {
        let mut p = PENDING.lock();
        let n = p.len().min(budget);
        let taken: Vec<PendingReap> = p.drain(..n).collect();
        taken
    };

    let mut keep: Vec<PendingReap> = Vec::new();
    for mut e in batch.drain(..) {
        if e.grace {
            // Re-queued grace entry (survivor of a prior run).
            keep.push(e);
            continue;
        }
        let (done, via_r85) = switch_stage_done(&e.claim);
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
                crate::dfx::taskdump::taskdump_dec(e.claim.task().pid() as u64);
                crate::dfx::taskdump::taskdump_raw_line(b"\n");
            }
        }
        e.grace = true;
        keep.push(e);
    }

    // Put the not-yet-grace survivors back.
    if !keep.is_empty() {
        let mut p = PENDING.lock();
        for mut e in keep.drain(..) {
            if !e.grace && p.len() < PENDING_CAP {
                p.push(e);
            } else {
                // Grace-marked entries join the persistent batch below;
                // the only drop here is a non-grace over-cap re-park
                // (the loud leak, counted).
                if !e.grace {
                    CLAIM_DROPPED.fetch_add(1, Ordering::Relaxed);
                } else {
                    let mut gb = GRACE_BATCH.lock();
                    gb.push(e);
                }
            }
        }
    }

    // ---- The grace stage (persistent batch, held-waiter protocol) ----
    // Merge this run's graduates into the batch; open or reuse the
    // target; hold the waiter count while the batch is non-empty
    // (rcu_note_context_switch suppresses QS stores otherwise — the
    // boot-42 stall root cause); on completion, free the whole batch
    // and release GP_IN_FLIGHT (the boot-63 latch root cause).
    let cur_target = GRACE_TARGET.load(Ordering::Acquire);
    let target = if cur_target == 0 {
        let t = crate::sync::rcu::shared_grace_target_pub();
        GRACE_TARGET.store(t, Ordering::Release);
        if GRACE_WAITER_HELD
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            crate::sync::rcu::grace_waiter_hold();
        }
        GRACE_STALL_RUNS.store(0, Ordering::Release);
        t
    } else {
        cur_target
    };

    if crate::sync::rcu::all_online_cpus_reported_pub(target) {
        // Round complete: free the whole batch, close the round.
        let drained: Vec<PendingReap> = {
            let mut gb = GRACE_BATCH.lock();
            core::mem::replace(&mut *gb, Vec::new())
        };
        for e in drained {
            let (pp, cp) = e.claim.into_free();
            notify_parent_if_any(pp, cp);
        }
        crate::sync::rcu::shared_grace_done_pub();
        GRACE_TARGET.store(0, Ordering::Release);
        GRACE_STALL_RUNS.store(0, Ordering::Release);
        if GRACE_WAITER_HELD
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            crate::sync::rcu::grace_waiter_release();
        }
    } else {
        // The drainer's own tick context is outside every RCU
        // read-side section: report THIS cpu's quiescent state at the
        // current generation (boot-118: the surviving wedge's lagging
        // cpu was one that reported via switches alone, and long
        // user-mode stretches between switches stalled the round while
        // generations climbed around it).
        crate::sync::rcu::report_self_quiescent();
        // Stall watch: expedite by poking the lagging CPUs (the
        // reference's synchronize_rcu_expedited shape — a resched IPI
        // on a kernel-preempt host forces the IRQ-exit switch that
        // stores the QS report the round is waiting on).
        let runs = GRACE_STALL_RUNS.fetch_add(1, Ordering::Relaxed) + 1;
        if runs >= GRACE_NUDGE_RUNS {
            let mut laggards = crate::sync::rcu::grace_lagging_cpus(target);
            let me = crate::arch::cpu_id() as usize;
            // Self-only stall (the drainer's own CPU between context
            // switches — the p4 storm's shape: the tick that opened
            // the target is always one QS behind its own bump until
            // its task next switches) is NORMAL and resolves at that
            // switch: no IPI, no print. Only a STALLED PEER gets the
            // expedited nudge.
            laggards.retain(|&c| c != me);
            if !laggards.is_empty() {
                let mut names = alloc::string::String::new();
                use core::fmt::Write as _;
                for cpu in laggards {
                    crate::arch::ipi::send_reschedule_ipi(cpu);
                    let _ = write!(names, "{} ", cpu);
                }
                static NUDGE_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = NUDGE_N.fetch_add(1, Ordering::Relaxed);
                if n < 32 {
                    crate::dfx::taskdump::taskdump_raw_line(b"REAP-GP-NUDGE #");
                    crate::dfx::taskdump::taskdump_dec(n as u64);
                    crate::dfx::taskdump::taskdump_raw_line(b" gen=");
                    crate::dfx::taskdump::taskdump_dec(target as u64);
                    crate::dfx::taskdump::taskdump_raw_line(b" laggards=");
                    for b in names.as_bytes() {
                        crate::dfx::taskdump::taskdump_raw_line(&[*b]);
                    }
                    crate::dfx::taskdump::taskdump_raw_line(b"\n");
                }
            }
            // Re-arm: one nudge burst per GRACE_NUDGE_RUNS window.
            GRACE_STALL_RUNS.store(0, Ordering::Release);
        }
    }

    DRAIN_ACTIVE.store(false, Ordering::Release);
}

/// The burn's wake arm (boot-118): a parent parked in wait4 on this
/// child must observe the completion even when the drainer (not the
/// parent's own reaper) finished it. The deferred-notify machinery is
/// the proven channel (processed on the next schedule tail / repick on
/// the parent's CPU; single-slot overwrite fires predecessors inline).
fn notify_parent_if_any(parent_pid: u32, child_pid: u32) {
    if parent_pid != 0 && parent_pid != child_pid {
        crate::sched::sched::defer_exit_notify(parent_pid, child_pid);
    }
}

/// The storage free proper — the tail of the old
/// `release_task_storage` with both waits removed (their conditions
/// are the ladder's completion predicates).
fn reap_storage_free(task: *mut Task) -> (u32, u32) {
    let t = crate::process::edges::borrow_task_mut(task);
    // The wake arm's inputs, read BEFORE the page may leave our hands
    // (task_put's last-pin free): the parent (tree edge) and the
    // child's own pid (the notify's subject).
    let parent_pid = t.parent_ptr().map(|p: *const Task| crate::process::edges::borrow_task(p as *mut Task).pid()).unwrap_or(0);
    let child_pid = t.pid();
    t.free_kernel_stack();
    crate::process::edges::free_fork_pt_regs(t);
    t.set_address_space(None);
    t.set_fdtable(None);
    t.signal = None;
    t.set_fs(None);
    crate::process::task::Task::task_put(task);
    (parent_pid, child_pid)
}
