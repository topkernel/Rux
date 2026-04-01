//! Linux-compatible softirq framework
//!
//! Defers interrupt bottom-half processing out of hardirq context.
//! Softirq vectors are registered once at init, then raised from hardirq
//! handlers. Processing happens in `__do_softirq()` at `irq_exit()` time
//! or in the ksoftirqd kernel thread on overflow.
//!
//! Reference: Linux kernel/softirq.c

use core::sync::atomic::{AtomicU32, Ordering};
use crate::config::MAX_CPUS;

// ============================================================================
// Softirq vector numbers (match Linux)
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

/// Maximum softirq processing loops before waking ksoftirqd
const MAX_SOFTIRQ_RESTART: usize = 10;

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

/// Per-CPU flag: is __do_softirq currently executing on this CPU?
/// Prevents recursive softirq invocation.
static mut SOFTIRQ_IN_PROGRESS: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

// ============================================================================
// Registration
// ============================================================================

/// Register a softirq handler for vector `nr` (Linux: `open_softirq`).
///
/// Must be called during initialization, before interrupts are enabled.
/// Once registered, handlers cannot be changed.
pub fn open_softirq(nr: usize, handler: SoftirqHandler) {
    assert!(nr < NR_SOFTIRQS, "softirq: invalid vector number {}", nr);
    unsafe {
        SOFTIRQ_VEC[nr] = Some(handler);
    }
}

// ============================================================================
// Raising softirqs
// ============================================================================

/// Mark a softirq as pending on the current CPU (Linux: `__raise_softirq_irqoff`).
///
/// Caller must have IRQs disabled. Does NOT wake ksoftirqd.
#[inline]
pub fn raise_softirq_irqoff(nr: usize) {
    debug_assert!(nr < NR_SOFTIRQS);
    let cpu = crate::arch::cpu_id() as usize;
    unsafe {
        SOFTIRQ_PENDING[cpu].fetch_or(1u32 << nr, Ordering::Release);
    }
}

/// Raise a softirq and wake ksoftirqd if not in hardirq context (Linux: `raise_softirq`).
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

/// Process pending softirqs (Linux: `__do_softirq`).
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
    unsafe { SOFTIRQ_IN_PROGRESS[cpu].store(1, Ordering::Release); }

    // Enter softirq context in preempt_count
    crate::interrupt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET
    );

    let mut restart_count = 0;

    loop {
        // Atomically snapshot and clear pending bits
        let pending = unsafe {
            SOFTIRQ_PENDING[cpu].swap(0, Ordering::AcqRel)
        };

        if pending == 0 {
            break;
        }

        // Process each set bit from LSB (highest priority) to MSB
        let mut remaining = pending;
        while remaining != 0 {
            let bit = remaining.trailing_zeros() as usize;
            remaining &= !(1u32 << bit);

            // Dispatch to registered handler
            let handler = unsafe { SOFTIRQ_VEC[bit] };
            if let Some(h) = handler {
                h(bit);
            }
        }

        restart_count += 1;
        if restart_count >= MAX_SOFTIRQ_RESTART {
            break;
        }
    }

    // Leave softirq context
    crate::interrupt::preempt_count_sub(
        crate::interrupt::preempt::SOFTIRQ_OFFSET
    );

    unsafe { SOFTIRQ_IN_PROGRESS[cpu].store(0, Ordering::Release); }

    // Return whether there are still pending softirqs
    unsafe { SOFTIRQ_PENDING[cpu].load(Ordering::Acquire) != 0 }
}

/// Called from `irq_exit()` to process pending softirqs (Linux: `invoke_softirq`).
///
/// Tries to run `__do_softirq` inline. On overflow (too many pending),
/// wakes ksoftirqd to handle the rest.
#[inline]
pub fn invoke_softirq() {
    let cpu = crate::arch::cpu_id() as usize;
    let pending = unsafe {
        SOFTIRQ_PENDING[cpu].load(Ordering::Acquire)
    };

    if pending == 0 {
        return;
    }

    let overflow = __do_softirq();

    if overflow {
        wakeup_ksoftirqd();
    }
}

// ============================================================================
// Query functions
// ============================================================================

/// Check if any softirqs are pending on the current CPU.
/// Used by ksoftirqd to decide whether to sleep.
#[inline]
pub fn has_pending_softirqs() -> bool {
    let cpu = crate::arch::cpu_id() as usize;
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
    // Tasklet action handlers are registered by tasklet::init().
    crate::pr_info!("softirq: {} vectors registered", NR_SOFTIRQS);
}
