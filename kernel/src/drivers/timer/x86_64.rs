//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 Timer driver — 8254 PIT channel 0
//!
//! Bring-up clock source: PIT channel 0 is programmed for KERNEL_HZ
//! square-wave ticks (mode 3, divisor PIT_INPUT_FREQ / HZ) and
//! free-runs; `set_next_trigger()` is a re-arm no-op (the contract the
//! shared timer module expects of a free-running tick source).
//!
//! `read_time()` synthesizes a counter in the nominal
//! `TIMER_CLOCK_FREQ_HZ` (10 MHz) clock domain from jiffies, matching
//! the units every generic cycles-to-nanoseconds conversion assumes.
//! TSC/LAPIC-timer based timekeeping is a later upgrade.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::cpu::{inb, io_wait, outb};

/// PIT input gate frequency (fixed 8254 crystal, Hz)
pub const PIT_INPUT_FREQ: u64 = 1_193_182;

/// Nominal clock domain of `read_time()` — matches
/// `crate::config::TIMER_CLOCK_FREQ_HZ` so generic cycle arithmetic
//! keeps its units.
pub const CLOCK_FREQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;

/// System clock frequency (HZ) — ticks per second
pub const HZ: u64 = crate::config::KERNEL_HZ as u64;

/// Nominal clock ticks per jiffy (10 MHz / HZ)
const TIME_SLICE_TICKS: u64 = CLOCK_FREQ / HZ;

/// PIT I/O ports (channel 0 data, mode/command register)
const PIT_CH0_DATA: u16 = 0x40;
const PIT_MODE_CMD: u16 = 0x43;

/// Mode/command byte: channel 0, lobyte/hibyte access, mode 3 (square wave),
/// binary count.
const PIT_CH0_MODE3_BIN: u8 = 0x36;

/// jiffies — global tick counter (advanced from the timer IRQ)
static JIFFIES: AtomicU64 = AtomicU64::new(0);

/// Get current jiffies value (ticks since boot)
#[inline]
pub fn get_jiffies() -> u64 {
    JIFFIES.load(Ordering::Acquire)
}

/// Coarse time in nanoseconds since boot, snapped to the nominal tick
/// grid (same contract as the riscv64 twin).
#[inline]
pub fn coarse_ns_since_boot() -> u64 {
    read_time() / TIME_SLICE_TICKS * (1_000_000_000 / HZ)
}

/// Convert jiffies to milliseconds
#[inline]
pub const fn jiffies_to_msecs(jiffies: u64) -> u64 {
    jiffies * 1000 / HZ
}

/// Convert milliseconds to jiffies
#[inline]
pub const fn msecs_to_jiffies(msecs: u64) -> u64 {
    msecs * HZ / 1000
}

/// Read current time in the nominal CLOCK_FREQ clock domain.
///
/// Tick-granular (jiffies-derived) until TSC calibration lands; the unit
/// contract (TIMER_CLOCK_FREQ_HZ ticks per second) is what generic code
/// relies on.
#[inline]
pub fn read_time() -> u64 {
    get_jiffies() * TIME_SLICE_TICKS
}

/// Program PIT channel 0 for KERNEL_HZ square-wave ticks.
///
/// Called once from the x86_64 early boot path (main.rs).
pub fn init() {
    let divisor = (PIT_INPUT_FREQ / HZ) as u16;
    // SAFETY: ports 0x40/0x43 are the fixed 8254 channel-0 data and
    // mode/command registers; this is the standard programming sequence.
    unsafe {
        outb(PIT_MODE_CMD, PIT_CH0_MODE3_BIN);
        io_wait();
        outb(PIT_CH0_DATA, (divisor & 0xff) as u8);
        io_wait();
        outb(PIT_CH0_DATA, (divisor >> 8) as u8);
    }
}

/// Arm the tick deadline. The PIT free-runs at a fixed HZ cadence, so
/// this is a no-op (kept for interface parity with the riscv64 twin).
pub fn set_timer(_deadline: u64) {}

/// Set next timer interrupt — no-op on a free-running PIT.
pub fn set_next_trigger() {}

/// Re-arm for a high-resolution deadline — no-op (tick is periodic).
pub fn rearm_for_hres() {}

/// Clock interrupt handler (called from the x86_64 trap entry)
pub fn timer_interrupt_handler() {
    // Monotonic jiffies advance: the PIT is periodic, so every IRQ is
    // exactly one tick.
    JIFFIES.fetch_add(1, Ordering::AcqRel);

    // vDSO time page refresh (shared with the riscv64 twin)
    crate::mm::vdso::vdso_data_tick();

    // Timer softirq for TCP timers (retransmission, delayed ACK)
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Timer as usize,
    );

    // Hrtimer softirq for software timers
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
    );
}
