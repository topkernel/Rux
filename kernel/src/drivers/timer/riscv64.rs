//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Timer driver
//!
//! Uses SBI calls to set timer

use riscv::register::time;
use crate::sbi;
use core::sync::atomic::{AtomicU64, Ordering};

/// Timer frequency (QEMU virt platform)
pub const CLOCK_FREQ: u64 = 10_000_000;  // 10 MHz

/// System clock frequency (HZ)
///
/// Triggers 100 clock interrupts per second (every 10ms)
pub const HZ: u64 = 100;

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

/// Increment jiffies counter
///
/// Called on every clock interrupt
#[inline]
fn increment_jiffies() {
    JIFFIES.fetch_add(1, Ordering::Release);
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

/// Set timer (using SBI call)
pub fn set_timer(deadline: u64) {
    sbi::set_timer(deadline);
}

/// Set next timer interrupt (time slice length)
///
pub fn set_next_trigger() {
    let current = read_time();
    let deadline = current + TIME_SLICE_TICKS;  // Trigger after 10ms
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

    // 2. TCP timer tick (retransmission, delayed ACK, etc.)
    crate::net::tcp_timer::tcp_timer_tick();

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
