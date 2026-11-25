//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Softirq framework
//!
//! Defers interrupt bottom-half processing out of hardirq context.
//! Softirq vectors are registered once at init, then raised from hardirq
//! handlers. Processing happens in `__do_softirq()` at `irq_exit()` time
//! or in the ksoftirqd kernel thread on overflow.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use crate::config::MAX_CPUS;

// ============================================================================
// Softirq vector numbers
// ============================================================================

/// Softirq type indices
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftirqIndex {
    Hi         = 0,
    Timer      = 1,
    NetTx      = 2,
    NetRx      = 3,
    Block      = 4,
    IrqPoll    = 5,
    Tasklet    = 6,
    Sched      = 7,
    Hrtimer    = 8,
    Rcu        = 9,
}

/// Total number of softirq vectors
pub const NR_SOFTIRQS: usize = 10;

/// Per-CPU, per-vector softirq raise counters (Linux /proc/softirqs).
pub static SOFTIRQ_RAISED: [[AtomicU64; NR_SOFTIRQS]; crate::config::MAX_CPUS] =
    [const { [const { AtomicU64::new(0) }; NR_SOFTIRQS] }; crate::config::MAX_CPUS];

/// Read one per-CPU softirq counter (Linux /proc/softirqs).
pub fn raised_count(cpu: usize, vec: usize) -> u64 {
    if cpu < crate::config::MAX_CPUS && vec < NR_SOFTIRQS {
        SOFTIRQ_RAISED[cpu][vec].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Maximum softirq processing loops before waking ksoftirqd
const MAX_SOFTIRQ_RESTART: usize = 10;

/// Maximum wall time spent processing softirqs at one irq_exit before
/// deferring the rest to ksoftirqd — Linux MAX_SOFTIRQ_TIME parity
/// (kernel/softirq.c: "Do NOT run for too long or the watchdog fires").
/// WT4 evidence: an unbounded irq-exit softirq budget let a timer/IO
/// storm pin both CPUs in IRQ context (93% si_busy) with scheduling
/// starved to zero picks. 2 ms of the nominal 10 MHz timebase.
const MAX_SOFTIRQ_TIME_TICKS: u64 =
    2 * crate::config::TIMER_CLOCK_FREQ_HZ / 1000;

// ============================================================================
// Softirq action handler type
// ============================================================================

/// Softirq action handler function.
/// Receives the softirq vector number as argument.
pub type SoftirqHandler = fn(usize);

// ============================================================================
// Global softirq vector table
// ============================================================================

/// Global softirq vector table.
/// Write-once at init time (via `open_softirq`), read lock-free during dispatch.
static SOFTIRQ_VEC: [crate::safety::slot::OnceSlot<SoftirqHandler>; NR_SOFTIRQS] =
    [const { crate::safety::slot::OnceSlot::new() }; NR_SOFTIRQS];

// ============================================================================
// Per-CPU pending bitmask
// ============================================================================

/// Per-CPU softirq pending bitmask. Bit N is set when softirq N is pending.
/// Each CPU only operates on its own element — no cross-CPU contention.
static SOFTIRQ_PENDING: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

// ==================== Wakewatch forensics (softirq-storm hunt) ====
//
// wt1/wt2 (vsync-starvation boots): after ~60 s the machine collapses —
// picks go to zero while both vCPUs stay 100% busy with IF=0, sampled
// RIPs inside softirq-context code (buddy free, spinlock). __do_softirq
// has a RESTART budget but NO TIME budget (Linux caps irq-exit softirq
// work at 2 ms and defers the rest to ksoftirqd), so a packet/I-O storm
// can pin the CPUs in IRQ context forever — no schedule() ever runs
// again. These counters name the storm vector:
pub static WW_VEC_RUN: [AtomicU32; NR_SOFTIRQS] = [
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0),
];
/// Times __do_softirq spent in handler dispatch (ticks; 1 pass each).
pub static WW_SOFTIRQ_TICKS: AtomicU64 = AtomicU64::new(0);
/// __do_softirq invocations (all contexts).
pub static WW_SOFTIRQ_RUNS: AtomicU64 = AtomicU64::new(0);

/// Per-CPU flag: is __do_softirq currently executing on this CPU?
/// Prevents recursive softirq invocation.
static SOFTIRQ_IN_PROGRESS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

// ==================== Softirq window progress (phantom-bits) ====
//
// The monopolist face of the OH wedge (incumbents at preempt=0x100 /
// 0x101 / 0x102, CFS-undefeatable for tens of seconds) is a task stuck
// INSIDE an __do_softirq window. While the window runs, its booking is
// live and legal — the tick-side orphan scan and PREEMPT-HEAL can only
// observe it if a tick lands, and a wedged window (handler spinning on
// a same-CPU-held lock, often with IF=0) is untickable. These two
// per-CPU records let ANY CPU read what a monopolized CPU's softirq
// window is doing: which vector is dispatching, the restart count, the
// pending snapshot and the window age. Packed WIN: 1<<63 open |
/// handler bit << 48 | restart_count << 40 | pending (low 16).
pub static SOFTIRQ_WIN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// read_time() ticks when the open window began (0 = closed).
pub static SOFTIRQ_WIN_T0: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// r-strand instrument (lat_fs conviction round, 2026-11-19): stuck
/// softirq-window probe for the tick-side watchdog. While a window is
/// stuck (one handler never returned), this cpu's irq-exit cannot
/// dispatch anything — the per-cpu timer vector starves, the wheel
/// stops being processed, and every timed wait on this cpu strands.
/// The softirq-side watchdog cannot report the stuck window (it runs
/// BEHIND the same IN_PROGRESS guard), so the tick hardirq probes it
/// from outside. Returns Some((vector_bit, age_ticks)) when cpu's
/// window has been open longer than 1 s, else None.
pub fn stuck_window(cpu: usize) -> Option<(u64, u64)> {
    if cpu >= MAX_CPUS {
        return None;
    }
    // SAFETY: per-CPU arrays; bounded index above.
    let inprog =  SOFTIRQ_IN_PROGRESS[cpu].load(Ordering::Acquire) ;
    if inprog == 0 {
        return None;
    }
    let win = SOFTIRQ_WIN[cpu].load(Ordering::Acquire);
    if win & (1u64 << 63) == 0 {
        return None;
    }
    let t0 = SOFTIRQ_WIN_T0[cpu].load(Ordering::Acquire);
    if t0 == 0 {
        return None;
    }
    let age = crate::timer::read_time().saturating_sub(t0);
    if age <= crate::config::TIMER_CLOCK_FREQ_HZ {
        return None;
    }
    let vec = (win >> 48) & 0xFF;
    Some((vec, age))
}

#[inline]
fn win_note(cpu: usize, open: bool, bit: usize, restart: usize, pending: u32) {
    let v = if open {
        (1u64 << 63)
            | ((bit as u64 & 0xFF) << 48)
            | ((restart as u64 & 0xFF) << 40)
            | (pending as u64 & 0xFFFF)
    } else {
        0
    };
    // SAFETY: per-CPU arrays written only by current CPU; bounded index.
    
        SOFTIRQ_WIN[cpu].store(v, Ordering::Release);
    
}

// ==================== Carrier evidence (CARRIER-HEAL, 123rd) ====================
//
// HEAL-GATE (122nd) refused PREEMPT-HEAL for holders observed inside
// RawSpinlock::lock's CONTENDED loop (per-CPU {task, jiffies} stamps
// refreshed every 64K spins — sync::spinlock::ACQUIRE_EVIDENCE). Its
// residual class (HEAL-GATE.md §4.4): counts whose acquire evidence had
// gone STALE at adjudication — the holder left the contended loop and
// carries its levels through work somewhere else. One such family is
// structurally identifiable WITHOUT touching any fast path: holders
// inside an __do_softirq window. The window books SOFTIRQ_OFFSET on the
// entry task around handler dispatch, and the make-test VW-WEDGE
// conviction proved what a heal landing mid-window does: the executor
// becomes preemptible, the handler never finishes, its completions are
// lost and the machine wedges (29 completions, both CPUs idle forever).
// These slots are the carrier-side half: __do_softirq's OWN boundaries
// (window entry, every vector dispatch, window exit) publish the same
// 16-byte {task, jiffies} evidence shape as heal-gate — one store pair
// per boundary, nothing on any lock fast path.
//
// Concurrency argument (all Relaxed): the only writers are the CPU's
// own __do_softirq boundaries; the only reader is the tick IRQ on that
// SAME CPU. The tick interrupts the window, so writer and reader never
// run concurrently and per-word coherence is enough (same shape as
// ACQUIRE_EVIDENCE and TICK_ENTRY_PC). Task identity is read from the
// scheduler's published cpu_state(cpu).current — the exact word the
// tick-side heal adjudicates against, so writer and reader are
// symmetric by construction.

/// Per-CPU carrier-evidence slot for the softirq window (see the block
/// comment above). `task` doubles as the open flag: nonzero while a
/// window is open on this CPU (0 = closed).
pub struct CarrierEvidence {
    /// cpu_state(cpu).current when the window opened / last dispatched
    /// (0 = no open window). Written at the window's own boundaries,
    /// read by this CPU's tick.
    pub task: AtomicUsize,
    /// Jiffies at the last boundary stamp (entry, dispatch, or exit).
    /// Age is only meaningful together with `task`.
    pub jiffies: AtomicU64,
}

/// Freshness bound for carrier evidence, in jiffies. Vector dispatches
/// refresh the stamp (bounded by the 2 ms budget check before each
/// vector), so 10 jiffies (100 ms at 100Hz) keeps the heal-gate margin
/// doctrine — 20x UNDER the heal's own 200-tick (2s) bar — while a
/// leaked/stranded count is delayed by at most this window past the
/// last genuine softirq boundary. Windows whose current handler runs
/// longer than the bound go stale mid-handler; that residual gap is
/// covered by the exact window-open flag (`in_progress_cpu`) consulted
/// alongside this slot by the adjudication, and named in
/// run/CARRIER-HEAL.md.
pub const CARRIER_EVIDENCE_FRESH_JIF: u64 = 10;

/// The per-CPU carrier-evidence slots (see [`CarrierEvidence`]).
pub static CARRIER_EVIDENCE: [CarrierEvidence; MAX_CPUS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: CarrierEvidence = CarrierEvidence {
        task: AtomicUsize::new(0),
        jiffies: AtomicU64::new(0),
    };
    [ZERO; MAX_CPUS]
};

/// Publish carrier-side evidence from __do_softirq's boundaries (window
/// entry and every vector dispatch). Costs one per-CPU slot read (the
/// SAME cpu_state().current word the tick-side heal adjudicates against
/// — writer and reader are symmetric by construction, no tp-vs-slot
/// skew possible), one jiffies read and two relaxed local stores.
#[inline]
fn note_carrier_evidence() {
    let cpu = crate::arch::smp::cpu_id() as usize;
    if cpu >= CARRIER_EVIDENCE.len() {
        return;
    }
    let t = crate::sched::sched::cpu_state(cpu).current as usize;
    if t == 0 {
        return;
    }
    let ev = &CARRIER_EVIDENCE[cpu];
    ev.task.store(t, Ordering::Relaxed);
    ev.jiffies
        .store(crate::timer::get_jiffies(), Ordering::Relaxed);
}

/// Close the carrier-evidence window (called at __do_softirq exit,
/// AFTER the SOFTIRQ_OFFSET sub so the slot never says closed while
/// the count still carries the bit). The jiffies half records WHEN the
/// window closed — a heal firing later can print its age.
#[inline]
fn clear_carrier_evidence() {
    let cpu = crate::arch::smp::cpu_id() as usize;
    if cpu >= CARRIER_EVIDENCE.len() {
        return;
    }
    let ev = &CARRIER_EVIDENCE[cpu];
    ev.task.store(0, Ordering::Relaxed);
    ev.jiffies
        .store(crate::timer::get_jiffies(), Ordering::Relaxed);
}

/// Does `task` (a Task pointer) have LIVE carrier-side evidence on
/// `cpu` — i.e. was it observed inside an __do_softirq window on that
/// CPU within [`CARRIER_EVIDENCE_FRESH_JIF`] jiffies of `now`? Called
/// from the tick-side PREEMPT-HEAL adjudication: a `true` result means
/// the incumbent is executing softirq work (or did so within the last
/// 100 ms) and the heal must be refused. Callers should ALSO consult
/// [`in_progress_cpu`] for the exact any-length window-open truth —
/// this slot's freshness cannot vouch for the middle of a handler that
/// runs longer than the bound.
#[inline]
pub fn carrier_evidence_fresh(cpu: usize, task: usize, now: u64) -> bool {
    if cpu >= CARRIER_EVIDENCE.len() {
        return false;
    }
    let ev = &CARRIER_EVIDENCE[cpu];
    ev.task.load(Ordering::Relaxed) == task
        && now.wrapping_sub(ev.jiffies.load(Ordering::Relaxed))
            <= CARRIER_EVIDENCE_FRESH_JIF
}

/// Jiffies age of `cpu`'s last carrier-evidence boundary stamp (entry,
/// dispatch or exit — whichever ran last). For the refusal row's
/// audit trail only.
#[inline]
pub fn carrier_evidence_age(cpu: usize, now: u64) -> u64 {
    if cpu >= CARRIER_EVIDENCE.len() {
        return u64::MAX;
    }
    now.wrapping_sub(CARRIER_EVIDENCE[cpu].jiffies.load(Ordering::Relaxed))
}

// ============================================================================
// Registration
// ============================================================================

/// Register a softirq handler for vector `nr`.
///
/// Must be called during initialization, before interrupts are enabled.
/// Once registered, handlers cannot be changed.
pub fn open_softirq(nr: usize, handler: SoftirqHandler) {
    assert!(nr < NR_SOFTIRQS, "softirq: invalid vector number {}", nr);
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    
        SOFTIRQ_VEC[nr].publish(handler);
    
}

// ============================================================================
// Raising softirqs
// ============================================================================

/// Mark a softirq as pending on the current CPU.
///
/// Caller must have IRQs disabled. Does NOT wake ksoftirqd.
#[inline]
pub fn raise_softirq_irqoff(nr: usize) {
    debug_assert!(nr < NR_SOFTIRQS);
    let cpu = crate::arch::cpu_id() as usize;
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    
        SOFTIRQ_PENDING[cpu].fetch_or(1u32 << nr, Ordering::Release);
    
    // /proc/softirqs: count the raise on the CPU that raised it.
    if cpu < crate::config::MAX_CPUS && nr < NR_SOFTIRQS {
        SOFTIRQ_RAISED[cpu][nr].fetch_add(1, Ordering::Relaxed);
    }
}

/// Raise a softirq and wake ksoftirqd if not in hardirq context.
///
/// Safe to call from any context.
pub fn raise_softirq(nr: usize) {
    raise_softirq_irqoff(nr);

    // If we are NOT in hardirq context and NOT already processing softirqs,
    // wake ksoftirqd so it gets serviced promptly.
    // (When called from hardirq, invoke_softirq() in irq_exit handles it.)
    if !crate::interrupt::preempt::in_irq() {
        wakeup_ksoftirqd();
    }
}

// ============================================================================
// Softirq processing
// ============================================================================

/// Process pending softirqs.
///
/// Called from:
///   1. `invoke_softirq()` at `irq_exit()` time
///   2. `ksoftirqd` kernel thread
///
/// Returns `true` if there are still pending softirqs after exhausting
/// the restart budget (caller should wake ksoftirqd).
pub fn __do_softirq() -> bool {
    let cpu = crate::arch::cpu_id() as usize;

    // Prevent recursion
    if  SOFTIRQ_IN_PROGRESS[cpu].load(Ordering::Acquire)  != 0 {
        return false;
    }
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
     SOFTIRQ_IN_PROGRESS[cpu].store(1, Ordering::Release); 
    // phantom-bits: window-open progress record (cross-CPU readable).
    win_note(cpu, true, 0, 0, 0);
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    
        SOFTIRQ_WIN_T0[cpu].store(crate::timer::read_time(), Ordering::Release);
    

    // Session-6 discriminator (fleet s2): bracket the SOFTIRQ PROCESSING
    // window itself. The timer/soft/ext TRAP brackets are silent on v7
    // while the count-migration leak rate is unchanged — this window
    // books SOFTIRQ_OFFSET on the entry task around handler dispatch,
    // and a context switch between the add and the sub migrates the
    // offset to the switched-in task (the exit sub underflows there, the
    // entry task strands +SOFTIRQ_OFFSET). Same entry_tp/exit_tp shape
    // as IRQ-BRACKET-LEAK; first_pending names the vector class that ran.
    let tp_entry = crate::arch::cpu::get_thread_id() as usize;

    // Enter softirq context in preempt_count — booked on the ENTRY
    // task (119th HARDIRQ discipline, extended to the SOFTIRQ face):
    // the window can span a context switch (a handler wakes a waiter
    // and the coroutine resume lands the exit epilogue in another
    // task's frame), and the old unconditional sub-on-current stranded
    // +SOFTIRQ_OFFSET on the entry task forever (the Session-6
    // count-migration leak: a lat_fs worker observed carrying
    // preempt=0x...2 for 377 ticks, unpreemptible, un-sleepable — the
    // lock-convoy strand root). The exit releases on the BOOKED task.
    let sirq_entry_task = crate::arch::cpu::get_thread_id() as usize;
    crate::interrupt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET
    );
    // CARRIER-HEAL (123rd): window-open stamp, AFTER the booking so the
    // slot never vouches for a window whose count does not yet carry
    // the bit (conservative direction: a tick in the entry sliver
    // refuses a heal it could have taken one tick later).
    note_carrier_evidence();
    // syscost probe: splits the open IF=0 segment only on the inline
    // irq-exit path (no-op when off).
    crate::debug::syscost::zone_softirq_enter();

    let mut restart_count = 0;
    let budget_t0 = crate::timer::read_time();
    // First non-zero pending snapshot of this window (bracket print only).
    let mut first_pending: u32 = 0;

    'outer: loop {
        // Atomically snapshot and clear pending bits
        let pending = 
            SOFTIRQ_PENDING[cpu].swap(0, Ordering::AcqRel)
        ;

        if pending == 0 {
            break;
        }
        if first_pending == 0 {
            first_pending = pending;
        }

        WW_SOFTIRQ_RUNS.fetch_add(1, Ordering::Relaxed);
        let t0 = crate::timer::read_time();

        // Process each set bit from LSB (highest priority) to MSB
        //
        // Linux checks the 2 ms budget BEFORE every vector
        // (kernel/softirq.c: "Stop at the budget time — the softirq
        // might run for long") and clears only the bit it is about to
        // run, so the vectors it did not reach stay pending for the next
        // entry. Rux used to clear the whole snapshot up front and
        // check the budget only after the batch: a batch of slow
        // handlers could therefore overrun the budget by the time of
        // every vector left in the batch, and the interrupted context
        // (the idle loop or a user task) lost exactly that much CPU —
        // the outcome the budget exists to prevent (the WT4 collapse
        // shape). Linux's placement bounds the overrun to a single
        // handler.
        let mut remaining = pending;
        while remaining != 0 {
            if crate::timer::read_time().saturating_sub(budget_t0)
                >= MAX_SOFTIRQ_TIME_TICKS
            {
                // Leave the vectors we did not reach pending (Linux
                // leaves them in local_softirq_pending); the caller
                // then wakes ksoftirqd to run them.
                
                    SOFTIRQ_PENDING[cpu].fetch_or(remaining, Ordering::Release);
                
                break 'outer;
            }
            let bit = remaining.trailing_zeros() as usize;
            remaining &= !(1u32 << bit);

            // Dispatch to registered handler
            let handler = SOFTIRQ_VEC[bit].get();
            if let Some(h) = handler {
                WW_VEC_RUN[bit].fetch_add(1, Ordering::Relaxed);
                // phantom-bits: progress record — cross-CPU readers (the
                // wake-strand census) name the vector a monopolized CPU
                // is dispatching when the window wedges.
                win_note(cpu, true, bit, restart_count, pending);
                // CARRIER-HEAL (123rd): dispatch heartbeat — one store
                // pair per vector dispatch keeps the window's evidence
                // fresh while handlers keep running (a single handler
                // longer than CARRIER_EVIDENCE_FRESH_JIF is covered by
                // the in-progress flag the adjudication also consults).
                note_carrier_evidence();
                h(bit);
            }
        }

        WW_SOFTIRQ_TICKS.fetch_add(
            crate::timer::read_time().saturating_sub(t0),
            Ordering::Relaxed,
        );

        restart_count += 1;
        // Linux irq_exit() discipline: stop after the restart budget OR
        // the 2 ms time budget — leave the rest pending for ksoftirqd so
        // the interrupted context (idle loop / user task) always regains
        // the CPU and scheduling continues. Without the time budget a
        // self-re-raising vector (timer+net/blk storm) keeps this loop
        // inside the IRQ context forever (the WT4 collapse shape).
        if restart_count >= MAX_SOFTIRQ_RESTART
            || crate::timer::read_time().saturating_sub(budget_t0)
                >= MAX_SOFTIRQ_TIME_TICKS
        {
            break 'outer;
        }
    }

    // Leave softirq context — release on the BOOKED task (see the
    // entry-site comment): same-cpu fast path is the plain sub; a
    // window that spanned a switch releases through the remote helper
    // (refcount-checked; a freed task's count is meaningless).
    {
        let sirq_exit_task = crate::arch::cpu::get_thread_id() as usize;
        if sirq_exit_task == sirq_entry_task {
            crate::interrupt::preempt_count_sub(
                crate::interrupt::preempt::SOFTIRQ_OFFSET
            );
        } else {
            let released = crate::process::task::Task::release_hardirq_base_on(
                sirq_entry_task as *mut crate::process::task::Task,
                crate::interrupt::preempt::SOFTIRQ_OFFSET,
                core::panic::Location::caller(),
            );
            if !released {
                // Entry task exited inside the window: the base dies
                // with its page (the safe accessor skipped it). The
                // CURRENT task never carried this bit — do not sub it.
                static SEXIT_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if SEXIT_N.fetch_add(1, Ordering::Relaxed) < 12 {
                    crate::pr_err!(
                        "SOFTIRQ-EXIT-SPANNED released=0 entry_task={:#x}",
                        sirq_entry_task
                    );
                }
            }
        }
    }
    // CARRIER-HEAL (123rd): window-closed stamp, AFTER the booking sub
    // so the slot never says closed while the count still carries the
    // bit. (Between this store pair and the flag clear below, the slot
    // says closed a few instructions early — the epilogue there runs
    // no handler code, and the conservative failure direction is a
    // one-tick heal delay, not a mid-window heal.)
    clear_carrier_evidence();
    // phantom-bits: window closed.
    win_note(cpu, false, 0, 0, 0);
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    
        SOFTIRQ_WIN_T0[cpu].store(0, Ordering::Release);
    

    // Session-6 discriminator exit side (see entry comment): print when
    // the task switched inside the processing window. Direct console —
    // this can print from deep inside the irq-exit/ksoftirqd paths.
    let tp_exit = crate::arch::cpu::get_thread_id() as usize;
    if tp_entry != tp_exit {
        // Forensics edge (SOFTIRQ-BRACKET-LEAK): raw reads of tasks that
        // may already be gone — no typed handle can prove liveness here.
        let a = tp_entry as *mut crate::process::task::Task;
        let b = tp_exit as *mut crate::process::task::Task;
        let (pa, pb) = unsafe {
            ((*a).preempt_count(), (*b).preempt_count())
        };
        use core::fmt::Write;
        let mut w = crate::debug::backtrace::ConsoleWriter::new();
        let _ = write!(w,
            "SOFTIRQ-BRACKET-LEAK cpu={} entry_tp={:#x} (pid={} preempt={:#x}) exit_tp={:#x} (pid={} preempt={:#x}) first_pending={:#x}\n",
            cpu, tp_entry,
            unsafe { (*a).pid() }, pa,
            tp_exit, unsafe { (*b).pid() }, pb,
            first_pending);
    }
    crate::debug::syscost::zone_softirq_exit(); // syscost probe

    SOFTIRQ_IN_PROGRESS[cpu].store(0, Ordering::Release);

    // Return whether there are still pending softirqs
    SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) != 0
}

/// Called from `irq_exit()` to process pending softirqs.
///
/// If we're in hardirq context (on IRQ stack), runs inline.
/// Otherwise, switches to per-CPU IRQ stack for consistent stack usage.
#[inline]
pub fn invoke_softirq() {
    let cpu = crate::arch::cpu_id() as usize;
    let pending = 
        SOFTIRQ_PENDING[cpu].load(Ordering::Acquire)
    ;

    if pending == 0 {
        return;
    }

    let overflow = if crate::interrupt::preempt::in_irq() {
        // Already on IRQ stack (called from irq_exit) — run inline
        __do_softirq()
    } else {
        // Not on IRQ stack (called from ksoftirqd or other context)
        // Switch to per-CPU IRQ stack for consistent stack usage
        do_softirq_own_stack()
    };

    if overflow {
        wakeup_ksoftirqd();
    }
}

/// Raw per-CPU pending mask (VW forensic instrumentation).
pub fn softirq_pending_raw(cpu: usize) -> u32 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    // SAFETY: bounded read of a per-CPU static.
     SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) 
}

/// Is a softirq processing window open on `cpu`? (phantom-bits orphan
/// scan: a task observed with the SOFTIRQ bit set while NO window is
/// open on its CPU carries a stranded booking — the 0x100 monopolist
/// face. A SeqLock::write section is irq-off, so a tick can never land
/// inside one; __do_softirq sets the flag before booking.)
pub fn in_progress_cpu(cpu: usize) -> bool {
    if cpu >= MAX_CPUS {
        return false;
    }
    // SAFETY: per-CPU static read, bounded index.
     SOFTIRQ_IN_PROGRESS[cpu].load(Ordering::Acquire) != 0 
}

/// Run `__do_softirq()` on the per-CPU interrupt stack.
///
/// Process softirqs on a separate stack.
///
/// NOTE: The original inline-asm stack-switch implementation had a subtle
/// `ra` clobber bug (`call` inside asm clobbers ra but the compiler didn't
/// know).  Even with `out("ra")` added, different compiler optimization
/// levels could still miscompile it.  For robustness, call __do_softirq
/// directly (without stack switch).  The stack-switch optimisation can be
/// re-introduced later using a proper separate assembly file.
fn do_softirq_own_stack() -> bool {
    __do_softirq()
}

// ============================================================================
// Query functions
// ============================================================================

/// Check if any softirqs are pending on the current CPU.
/// Used by ksoftirqd to decide whether to sleep.
#[inline]
pub fn has_pending_softirqs() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
    // SAFETY: SOFTIRQ_PENDING is a per-CPU array indexed by cpu_id; current CPU
    // only accesses its own element, so no cross-CPU data race is possible.
     SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) != 0 
}

// ============================================================================
// ksoftirqd interface (implemented in ksoftirqd.rs)
// ============================================================================

/// Wake the per-CPU ksoftirqd kernel thread.
/// Forwarded to ksoftirqd module to avoid circular dependency.
fn wakeup_ksoftirqd() {
    crate::interrupt::ksoftirqd::wakeup_ksoftirqd();
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the softirq subsystem.
/// Called once during boot, before interrupts are enabled.
pub fn init() {
    // SOFTIRQ_PENDING and SOFTIRQ_IN_PROGRESS are already zero-initialized.

    // Register softirq handlers for each vector.
    // Tasklet vectors (Hi, Tasklet) are registered by tasklet::init().
    // Note: tasklet::init() runs after softirq::init() in interrupt::init(),
    // so it will overwrite the None entries for Hi and Tasklet — that's fine.

    open_softirq(SoftirqIndex::Timer as usize, crate::net::net_hooks::tcp_timer_softirq);
    open_softirq(
        SoftirqIndex::NetRx as usize,
        crate::drivers::net::virtio_net::net_rx_softirq_handler,
    );
    open_softirq(
        SoftirqIndex::Block as usize,
        crate::drivers::virtio::block_bh_handler,
    );
    open_softirq(
        SoftirqIndex::Hrtimer as usize,
        crate::timer::timer_softirq_handler,
    );

}
