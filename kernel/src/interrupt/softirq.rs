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

use core::sync::atomic::{AtomicU32, Ordering};
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

/// Run `__do_softirq()` on the per-CPU interrupt stack.
///
/// Process softirqs on a separate stack. Under BKL,
/// the stack switch is a simple sp swap with no TLB/page table changes.
fn do_softirq_own_stack() -> bool {
    let stack_top = crate::arch::smp::get_per_cpu_intr_stack_top();
    // SAFETY: stack_top is the per-CPU IRQ stack pointer from get_per_cpu_intr_stack_top();
    // the asm block saves/restores sp around the call, and all callee-saved registers
    // are handled by the Rust ABI.
    unsafe {
        let mut result: usize;
        core::arch::asm!(
            // Save original sp
            "mv t0, sp",
            // Switch to per-CPU IRQ stack
            "mv sp, {stack}",
            // Call __do_softirq (returns bool in a0)
            "call {func}",
            // Save result, restore original sp
            "mv {ret}, a0",
            "mv sp, t0",
            stack = in(reg) stack_top,
            func = sym __do_softirq,
            ret = out(reg) result,
            out("t0") _,
            out("a0") _,
        );
        result != 0
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

    crate::pr_info!("softirq: {} vectors registered", NR_SOFTIRQS);
}
