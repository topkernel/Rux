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

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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
static mut SOFTIRQ_VEC: [Option<SoftirqHandler>; NR_SOFTIRQS] = [
    None, None, None, None, None, None, None, None, None, None,
];

// ============================================================================
// Per-CPU pending bitmask
// ============================================================================

/// Per-CPU softirq pending bitmask. Bit N is set when softirq N is pending.
/// Each CPU only operates on its own element — no cross-CPU contention.
static mut SOFTIRQ_PENDING: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

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
static mut SOFTIRQ_IN_PROGRESS: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

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
    unsafe {
        SOFTIRQ_VEC[nr] = Some(handler);
    }
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
    unsafe {
        SOFTIRQ_PENDING[cpu].fetch_or(1u32 << nr, Ordering::Release);
    }
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
    if unsafe { SOFTIRQ_IN_PROGRESS[cpu].load(Ordering::Acquire) } != 0 {
        return false;
    }
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    unsafe { SOFTIRQ_IN_PROGRESS[cpu].store(1, Ordering::Release); }

    // Enter softirq context in preempt_count
    crate::interrupt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET
    );
    // syscost probe: splits the open IF=0 segment only on the inline
    // irq-exit path (no-op when off).
    crate::dfx::syscost::zone_softirq_enter();

    let mut restart_count = 0;
    let budget_t0 = crate::drivers::timer::read_time();

    'outer: loop {
        // Atomically snapshot and clear pending bits
        let pending = unsafe {
            SOFTIRQ_PENDING[cpu].swap(0, Ordering::AcqRel)
        };

        if pending == 0 {
            break;
        }

        WW_SOFTIRQ_RUNS.fetch_add(1, Ordering::Relaxed);
        let t0 = crate::drivers::timer::read_time();

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
            if crate::drivers::timer::read_time().saturating_sub(budget_t0)
                >= MAX_SOFTIRQ_TIME_TICKS
            {
                // Leave the vectors we did not reach pending (Linux
                // leaves them in local_softirq_pending); the caller
                // then wakes ksoftirqd to run them.
                unsafe {
                    SOFTIRQ_PENDING[cpu].fetch_or(remaining, Ordering::Release);
                }
                break 'outer;
            }
            let bit = remaining.trailing_zeros() as usize;
            remaining &= !(1u32 << bit);

            // Dispatch to registered handler
            let handler = unsafe { SOFTIRQ_VEC[bit] };
            if let Some(h) = handler {
                WW_VEC_RUN[bit].fetch_add(1, Ordering::Relaxed);
                h(bit);
            }
        }

        WW_SOFTIRQ_TICKS.fetch_add(
            crate::drivers::timer::read_time().saturating_sub(t0),
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
            || crate::drivers::timer::read_time().saturating_sub(budget_t0)
                >= MAX_SOFTIRQ_TIME_TICKS
        {
            break 'outer;
        }
    }

    // Leave softirq context
    crate::interrupt::preempt_count_sub(
        crate::interrupt::preempt::SOFTIRQ_OFFSET
    );
    crate::dfx::syscost::zone_softirq_exit(); // syscost probe

    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    unsafe { SOFTIRQ_IN_PROGRESS[cpu].store(0, Ordering::Release); }

    // Return whether there are still pending softirqs
    // SAFETY: per-CPU arrays accessed only by current CPU; cpu_id is correct
    unsafe { SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) != 0 }
}

/// Called from `irq_exit()` to process pending softirqs.
///
/// If we're in hardirq context (on IRQ stack), runs inline.
/// Otherwise, switches to per-CPU IRQ stack for consistent stack usage.
#[inline]
pub fn invoke_softirq() {
    let cpu = crate::arch::cpu_id() as usize;
    let pending = unsafe {
        SOFTIRQ_PENDING[cpu].load(Ordering::Acquire)
    };

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
    unsafe { SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) }
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
    // SAFETY: __do_softirq is our own function that manages its own
    // preempt_count and recursion guard.
    unsafe { __do_softirq() }
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
    unsafe { SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) != 0 }
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

    open_softirq(
        SoftirqIndex::Timer as usize,
        crate::net::tcp_timer::timer_softirq_handler,
    );
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
