//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 Timer driver — 8254 PIT channel 0 + LAPIC timer / TSC
//!
//! Two phases:
//!
//! 1. **PIT (boot fallback)**: channel 0 free-runs at KERNEL_HZ (mode 3)
//!    and IRQ0 drives the tick; `read_time()` is jiffies-derived.
//! 2. **HPET (exact timebase, x86_64)**: discovered right after the
//!    device mappings exist; `read_time()` becomes the HPET main
//!    counter (QEMU virtual clock — see `hpet.rs` for why a calibrated
//!    TSC cannot hold wall time under TCG).  The TSC then serves only
//!    short busy delays.
//! 3. **LAPIC timer (after APIC calibration)**: every CPU runs a
//!    periodic local APIC timer (vector 0xE0, programmed by the intc
//!    driver); without an HPET the TSC becomes the timekeeper —
//!    `read_time()` maps `rdtsc` into the nominal 10 MHz domain through
//!    a calibrated multiplier/shift pair, kept continuous with the
//!    jiffies-derived pre-calibration clock via a one-time offset.
//!
//! With N CPUs ticking, jiffies must NOT be incremented once per IRQ
//! (that ran 4x fast on the riscv64 twin before review批次7): the tick
//! handler derives jiffies from the timebase (`floor(elapsed/period)`),
//! which is idempotent under concurrent tick handlers on any CPU.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::cpu::{inb, io_wait, outb};

/// PIT input gate frequency (fixed 8254 crystal, Hz)
pub const PIT_INPUT_FREQ: u64 = 1_193_182;

/// Nominal clock domain of `read_time()` — matches
/// `crate::config::TIMER_CLOCK_FREQ_HZ` so generic cycle arithmetic
/// keeps its units.
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

/// Fractional shift used by the TSC -> nominal-domain mapping.
const TSC_MULSHIFT: u32 = 12;

/// TSC timekeeping state (written once by `lapic_time_calibrated`):
/// nominal = ((tsc - base_tsc) >> SHIFT) * MULT, where
/// MULT = round(CLOCK_FREQ << SHIFT / tsc_freq).
static TSC_MULT: AtomicU64 = AtomicU64::new(0);
static TSC_BASE: AtomicU64 = AtomicU64::new(0);
/// Offset added to the TSC-derived value so the switch from the
/// jiffies-derived clock cannot run time backward.
static TSC_CONTINUITY_OFFSET: AtomicU64 = AtomicU64::new(0);

/// True once the TSC became the timekeeper.
static TSC_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// jiffies — global tick counter (advanced from the timer IRQ)
static JIFFIES: AtomicU64 = AtomicU64::new(0);
/// timebase value at the first LAPIC-tick jiffies update (grid origin).
static JIFFIES_BASE: AtomicU64 = AtomicU64::new(0);

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
/// HPET-derived (exact) once the HPET clocksource initialized;
/// TSC-derived (with a continuity offset) if only the APIC calibration
/// ran; jiffies-derived before either.
#[inline]
pub fn read_time() -> u64 {
    if crate::drivers::timer::hpet::ready() {
        return crate::drivers::timer::hpet::read_time_nominal();
    }
    if TSC_ACTIVE.load(Ordering::Acquire) != 0 {
        let mult = TSC_MULT.load(Ordering::Acquire);
        let now = crate::arch::cpu::read_counter();
        let base = TSC_BASE.load(Ordering::Acquire);
        let delta = now.wrapping_sub(base);
        (delta >> TSC_MULSHIFT).wrapping_mul(mult)
            + TSC_CONTINUITY_OFFSET.load(Ordering::Acquire)
    } else {
        get_jiffies() * TIME_SLICE_TICKS
    }
}

/// Called by the intc/APIC driver after calibration: promote the TSC to
/// timekeeper.  `tsc_freq == 0` (failed calibration) keeps the PIT phase.
/// No-op when the HPET clocksource is active — the TSC then stays a
/// delay-only counter (see `hpet.rs`).
pub fn lapic_time_calibrated(_lapic_freq: u64, tsc_freq: u64) {
    if tsc_freq == 0 {
        return;
    }
    if crate::drivers::timer::hpet::ready() {
        return;
    }
    // nominal = (tsc >> S) * (CLOCK_FREQ << S / tsc_freq) — pick the
    // multiplier so the mapping stays exact within one shift quantum.
    let mult = (CLOCK_FREQ << TSC_MULSHIFT).div_ceil(tsc_freq);
    let now = crate::arch::cpu::read_counter();
    let continuity = get_jiffies() * TIME_SLICE_TICKS;
    TSC_MULT.store(mult, Ordering::Release);
    TSC_BASE.store(now, Ordering::Release);
    TSC_CONTINUITY_OFFSET.store(continuity, Ordering::Release);
    TSC_ACTIVE.store(1, Ordering::Release);
}

/// Program PIT channel 0 for KERNEL_HZ square-wave ticks.
///
/// Called from the x86_64 timer-enable path when the LAPIC tick could
/// not arm (IRQ0 fallback); idempotent.
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

/// Set next timer interrupt — no-op (PIT and LAPIC timer are periodic).
pub fn set_next_trigger() {}

/// Re-arm for a high-resolution deadline — no-op (tick is periodic).
pub fn rearm_for_hres() {}

/// True when a precise (non-jiffies) timebase is active — the tick
/// handler derives jiffies from it instead of counting IRQs.
#[inline]
fn timebase_precise() -> bool {
    crate::drivers::timer::hpet::ready() || TSC_ACTIVE.load(Ordering::Acquire) != 0
}

/// Timebase value just before the HPET takes over (continuity offset
/// for the switch; the HPET counter starts near 0 at its enable).
pub fn pre_hpet_time() -> u64 {
    read_time()
}

/// Advance jiffies from the TIMEBASE, not from IRQ counts (riscv64
/// review批次7 discipline): with N CPUs each running a periodic LAPIC
/// timer, per-IRQ increments ran jiffies N times fast.  Deriving
/// `floor((now - base) / period)` makes the update idempotent and
/// multi-CPU safe.
#[inline]
fn increment_jiffies_from_timebase() {
    let now = read_time();
    // Establish the grid origin on the first call.
    if JIFFIES_BASE.load(Ordering::Acquire) == 0 {
        JIFFIES_BASE
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
            .ok();
    }
    let base = JIFFIES_BASE.load(Ordering::Acquire);
    let nominal = now.saturating_sub(base) / TIME_SLICE_TICKS;
    // Monotonic: only ever move forward.
    let mut cur = JIFFIES.load(Ordering::Acquire);
    while nominal > cur {
        match JIFFIES.compare_exchange_weak(cur, nominal, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

/// Clock interrupt handler (called from the x86_64 trap entry)
pub fn timer_interrupt_handler() {
    if timebase_precise() {
        // LAPIC-timer phase: idempotent, multi-CPU-safe jiffies.
        increment_jiffies_from_timebase();
    } else {
        // PIT phase: the free-running channel 0 makes every IRQ exactly
        // one tick (single-CPU boot fallback).
        JIFFIES.fetch_add(1, Ordering::AcqRel);
    }

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
