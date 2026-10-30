//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 Timer driver — architectural generic timer (virtual counter).
//!
//! Uses CNTVCT_EL0 (monotonic counter), CNTFRQ_EL0 (frequency) and the
//! EL1 VIRTUAL timer (CNTV_CVAL_EL0 / CNTV_CTL_EL0) for the periodic tick.
//! The virtual timer is chosen over the physical timer because it is
//! always accessible at EL1 — no CNTHCTL_EL2 gating to arrange while
//! bringing up the EL2→EL1 boot path. GICv3 PPI for the virtual timer is
//! INTID 27 (PPI 11); wired in stage 3 with the GIC bring-up.
//!
//! The jiffies machinery mirrors the riscv64 driver: jiffies derive from
//! the LIVE counter against the NOMINAL tick grid (multi-cpu safe,
//! immune to accumulated re-arm latency — see the riscv64 twin for the
//! review history).

use core::sync::atomic::{AtomicU64, Ordering};

/// System clock frequency (HZ) - from config
pub const HZ: u64 = crate::config::KERNEL_HZ as u64;

/// Cached CNTFRQ_EL0 (read once at first use; 64 MHz class on QEMU virt)
static CLOCK_FREQ_CACHED: AtomicU64 = AtomicU64::new(0);

/// Nominal ticks per period — computed from the LIVE frequency
#[inline]
fn ticks_per_period() -> u64 {
    clock_freq() / HZ
}

/// Timer counter frequency (runtime from CNTFRQ_EL0)
#[inline]
pub fn clock_freq() -> u64 {
    let f = CLOCK_FREQ_CACHED.load(Ordering::Acquire);
    if f != 0 {
        return f;
    }
    let live = crate::arch::cpu::get_counter_freq();
    CLOCK_FREQ_CACHED
        .compare_exchange(0, live, Ordering::AcqRel, Ordering::Acquire)
        .ok();
    live
}

/// Compile-time nominal frequency (config parity; runtime CNTFRQ is
/// authoritative — QEMU virt aarch64 does NOT run at the 10 MHz the
/// riscv64 CLINT does)
pub const CLOCK_FREQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;

/// jiffies - global clock counter
static JIFFIES: AtomicU64 = AtomicU64::new(0);

/// timebase value at the first tick — origin of the nominal tick grid
static JIFFIES_BASE: AtomicU64 = AtomicU64::new(0);

/// Get current jiffies value
#[inline]
pub fn get_jiffies() -> u64 {
    JIFFIES.load(Ordering::Acquire)
}

/// Coarse time in nanoseconds since boot, snapped to the NOMINAL tick
/// grid and computed from the LIVE timebase (riscv64 twin has the LTP
/// utime01 history; same contract here)
#[inline]
pub fn coarse_ns_since_boot() -> u64 {
    let period_ns = 1_000_000_000 / HZ;
    read_time() / ticks_per_period() * period_ns
}

/// Advance jiffies from the TIMEBASE (idempotent, multi-cpu safe)
#[inline]
fn increment_jiffies() {
    let now = read_time();
    if JIFFIES_BASE.load(Ordering::Acquire) == 0 {
        JIFFIES_BASE
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
            .ok();
    }
    let base = JIFFIES_BASE.load(Ordering::Acquire);
    let nominal = now.saturating_sub(base) / ticks_per_period();
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
#[inline]
pub const fn jiffies_to_msecs(jiffies: u64) -> u64 {
    jiffies * 1000 / HZ
}

/// Convert milliseconds to jiffies
#[inline]
pub const fn msecs_to_jiffies(msecs: u64) -> u64 {
    msecs * HZ / 1000
}

/// Read current time (CNTVCT_EL0 virtual counter)
#[inline]
pub fn read_time() -> u64 {
    crate::arch::cpu::read_counter()
}

/// High-resolution nanoseconds since boot (u128 intermediate — see the
/// riscv64 twin's overflow note)
#[inline]
pub fn monotonic_ns_since_boot() -> u64 {
    let freq = clock_freq() as u128;
    ((read_time() as u128) * 1_000_000_000u128 / freq) as u64
}

/// Arm the EL1 virtual timer to an absolute CNTVCT deadline
#[inline]
pub fn set_timer(deadline: u64) {
    // SAFETY: writing the virtual-timer compare/enable registers arms the
    // per-cpu timer; the only side effect is the future PPI interrupt.
    unsafe {
        core::arch::asm!(
            "msr cntv_cval_el0, {d}",
            "msr cntv_ctl_el0, {e}",
            d = in(reg) deadline,
            e = in(reg) 1u64, // enable, unmasked
            options(nomem, nostack)
        );
    }
}

/// Set next timer interrupt, aligned to the NOMINAL tick grid but never
/// later than the earliest high-resolution deadline (riscv64 twin logic)
pub fn set_next_trigger() {
    let current = read_time();
    let tpp = ticks_per_period();
    let mut deadline = (current / tpp + 1) * tpp;
    let hres = crate::timer::hres_next_deadline();
    if hres < deadline {
        deadline = hres.max(current + 1);
    }
    set_timer(deadline);
}

/// Re-arm this cpu's timer to min(next grid tick, earliest hres deadline)
pub fn rearm_for_hres() {
    set_next_trigger();
}

/// Clock interrupt handler (called from the EL1 IRQ trap path)
pub fn timer_interrupt_handler() {
    // 1. Update jiffies counter
    increment_jiffies();

    // 1.5 vDSO time-data page refresh
    crate::mm::vdso::vdso_data_tick();

    // 2. Raise Timer softirq for TCP timer processing (gated on table
    //    occupancy — see the riscv64 twin / sc184)
    if crate::net::tcp::get_tcp_socket_table().count() > 0 {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Timer as usize,
        );
    }

    // 2.5 Raise Hrtimer softirq when work is pending
    if crate::timer::timer_softirq_work_pending() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
        );
    }
}
