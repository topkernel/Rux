//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Timer driver
//!
//! Uses SBI calls or sstc extension to set timer

use riscv::register::time;
use crate::sbi;
use core::sync::atomic::{AtomicU64, Ordering};
use core::arch::asm;

/// Timer frequency (QEMU virt platform) - from config
pub const CLOCK_FREQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;

/// System clock frequency (HZ) - from config
///
/// Triggers HZ clock interrupts per second
pub const HZ: u64 = crate::config::KERNEL_HZ as u64;

/// Time slice length (10 milliseconds)
///
/// Each time slice is 10ms, used for preemptive scheduling
const TIME_SLICE_TICKS: u64 = CLOCK_FREQ / HZ;  // 10ms

/// jiffies - global clock counter
///
///
/// Used for:
/// - Time measurement
/// - Timeout management
/// - Scheduling statistics
/// - Performance analysis
///
/// Type: AtomicU64 (supports multi-core concurrent access)
static JIFFIES: AtomicU64 = AtomicU64::new(0);

/// timebase value at the first tick — the origin of the nominal tick grid.
static JIFFIES_BASE: AtomicU64 = AtomicU64::new(0);

/// jiffies related functions

/// Get current jiffies value
///
///
/// # Returns
/// - Current jiffies value (clock interrupt count since system boot)
#[inline]
pub fn get_jiffies() -> u64 {
    JIFFIES.load(Ordering::Acquire)
}

/// Advance jiffies from the TIMEBASE, not from IRQ counts.
///
/// Review批次7 (TIMING 高): every hart's timer IRQ incremented the single
/// global counter — with 4 CPUs, jiffies ran 4x fast (every jiffies-based
/// timeout fired at 1/4 of its nominal duration). Deriving jiffies from
/// `floor((time - base) / ticks_per_period)` instead makes the update
/// idempotent and multi-hart safe, and also keeps jiffies glued to the
/// NOMINAL tick grid (immune to accumulated re-arm latency).
#[inline]
fn increment_jiffies() {
    let now = read_time();
    // Establish the grid origin on the first call (boot CPU's first tick).
    if JIFFIES_BASE.load(Ordering::Acquire) == 0 {
        JIFFIES_BASE
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
            .ok();
    }
    let base = JIFFIES_BASE.load(Ordering::Acquire);
    let nominal = now.saturating_sub(base) / TIME_SLICE_TICKS;
    // Monotonic: only ever move forward (max of current and nominal).
    let mut cur = JIFFIES.load(Ordering::Acquire);
    while nominal > cur {
        match JIFFIES.compare_exchange_weak(
            cur,
            nominal,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

/// Convert jiffies to milliseconds
///
#[inline]
pub const fn jiffies_to_msecs(jiffies: u64) -> u64 {
    jiffies * 1000 / HZ
}

/// Convert milliseconds to jiffies
///
#[inline]
pub const fn msecs_to_jiffies(msecs: u64) -> u64 {
    msecs * HZ / 1000
}

/// Read current time (time CSR)
#[inline]
pub fn read_time() -> u64 {
    time::read() as u64
}

/// Set timer using stimecmp CSR (sstc extension)
///
/// This is more direct than SBI and avoids potential issues
#[inline]
fn set_timer_stimecmp(deadline: u64) {
    unsafe {
        asm!(
            "csrw stimecmp, {0}",
            in(reg) deadline,
            options(nomem, nostack)
        );
    }
}

/// Set timer using SBI call (fallback)
#[inline]
fn set_timer_sbi(deadline: u64) {
    sbi::set_timer(deadline);
}

/// Set timer - use SBI set_timer for reliable cross-hart behavior.
///
/// Direct stimecmp writes from S-mode can race with OpenSBI's M-mode
/// timer management on QEMU virt.  SBI set_timer goes through M-mode
/// which ensures the correct per-hart timer is armed.
pub fn set_timer(deadline: u64) {
    set_timer_sbi(deadline);
}

/// Set next timer interrupt, aligned to the NOMINAL tick grid.
///
/// Review批次8: arming at `now + period` accumulates every handler's
/// latency into the period (permanent drift — sleeps lengthen under
/// load). Rounding up to the next grid multiple keeps the interrupt
/// cadence glued to nominal 1/HZ boundaries.
pub fn set_next_trigger() {
    let current = read_time();
    let deadline = (current / TIME_SLICE_TICKS + 1) * TIME_SLICE_TICKS;
    set_timer(deadline);
}

/// Clock interrupt handler
///
///
/// # Functions
/// 1. Update jiffies counter
/// 2. Update system runtime statistics
/// 3. Trigger scheduler tick
/// 4. Process timer callbacks
///
/// # When called
/// Called by trap_handler on every clock interrupt
///
/// # Notes
/// - Called in interrupt context, cannot sleep
/// - Must complete quickly to avoid affecting system performance
pub fn timer_interrupt_handler() {
    // 1. Update jiffies counter
    increment_jiffies();

    // 2. Raise Timer softirq for TCP timer processing
    //    (retransmission, delayed ACK, etc. — deferred to bottom half)
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Timer as usize,
    );

    // 2.5 Raise Hrtimer softirq for software timer processing
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
    );

    // 3. TODO: Update process runtime statistics
    //    - Current process utime/stime
    //    - CPU statistics

    // 4. TODO: Process software timers
    //    - Check expired timers
    //    - Call timer callback functions

    // 5. TODO: Trigger scheduler tick
    //    - Update current process runtime
    //    - Check if scheduling is needed
    //    - Set need_resched flag

    // Note: Scheduling is handled by schedule() call in trap.rs
}
