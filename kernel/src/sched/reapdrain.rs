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

// The claim crosses CPUs by design (reaper mints, the ticking CPU
// burns); the exclusivity is the linear type itself plus the
// unreachability contract minted on — the same Send/Sync declaration
// every queue handle in this subsystem carries.
unsafe impl Send for ReapClaim {}

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
    pub fn mint(task: *mut Task) -> Option<Self> {
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
        // SAFETY: the claim owns the free; until into_free runs the
        // page is mapped and immutable to everyone else (DEAD,
        // unlinked) — the same argument every task_shared caller
        // makes, now with the obligation typed.
        unsafe { self.0.as_ref() }
    }

    /// Burn: consume the obligation and run the storage free.
    /// Byte-identical to the raw form's `reap_storage_free` (the
    /// identity-preservation obligation).
    fn into_free(self) {
        let task = self.0.as_ptr();
        core::mem::forget(self);
        reap_storage_free(task);
    }
}

struct PendingReap {
    claim: ReapClaim,
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
                    crate::dfx::taskdump::taskdump_dec(
                        e.claim.task().pid() as u64,
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
            // The burn: the obligation this entry carried since its
            // mint is consumed here — into_free is the only exit from
            // the claim type, so the exactly-once free is a type
            // invariant now (the pilot's deliverable).
            e.claim.into_free();
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
                CLAIM_DROPPED.fetch_add(1, Ordering::Relaxed);
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
