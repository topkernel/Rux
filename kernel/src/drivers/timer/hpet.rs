//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 HPET clocksource — exact wall-time tracking under QEMU TCG.
//!
//! X86-TIMEBASE: TCG's `rdtsc` is derived from the raw HOST TSC
//! (`cpu_get_ticks()` in QEMU's system/cpu-timers.c), while every
//! device timer — PIT, LAPIC timer, and the HPET main counter — runs on
//! QEMU_CLOCK_VIRTUAL, which tracks host CLOCK_MONOTONIC. On
//! WSL2/Hyper-V those two host clock domains drift against each other
//! by percent level under load (measured: guest mono 3-4% fast over
//! 60 s with a boot-time TSC-vs-PIT calibration), so a calibrated TSC
//! can never hold the kernel clock near wall time on this platform.
//!
//! The HPET main counter IS the virtual clock (`hpet_get_ticks()` in
//! hw/timer/hpet.c = ns_to_ticks(QEMU_CLOCK_VIRTUAL)), so deriving
//! `read_time()` from it makes CLOCK_MONOTONIC exact by construction:
//! sleeps expire on the same clock that advances the counter, and the
//! host observes guest seconds equal to wall seconds.
//!
//! The TSC stays in use for short busy delays (`apic::delay_ms`), where
//! percent-level rate drift is irrelevant.
//!
//! QEMU facts used here (v8.2, q35 default config):
//! - base 0xFED00000, 1 KB register block, inside the identity-mapped
//!   uncached PCI MMIO hole installed by `setup_device_mappings`;
//! - counter period 10 ns (HPET_PERIOD = 10,000,000 fs) → 100 MHz tick;
//! - 32-bit accesses only (memory region min_access_size 4), so the
//!   64-bit counter is read as two halves with a wraparound re-read;
//! - the counter is frozen at its latched value (0 after reset) until
//!   GEN_CFG.ENABLE_CNF (bit 0) is set; enabling it makes the counter
//!   start at the latched value and track virtual time;
//! - GEN_CFG.LEGACY_RT_CNF (bit 1) must stay CLEAR so the PIT (IRQ0
//!   fallback tick + calibration window) keeps working.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::config::TIMER_CLOCK_FREQ_HZ;

/// Fixed q35 HPET base (fallback when no ACPI HPET table is found).
const HPET_FIXED_BASE: usize = 0xFED0_0000;

// Register offsets within the HPET block.
const HPET_ID: usize = 0x000; // capabilities (low) / PERIOD fs (high)
const HPET_CFG: usize = 0x010; // general configuration
const HPET_CNTR_LO: usize = 0x0F0; // main counter, low 32 bits
const HPET_CNTR_HI: usize = 0x0F4; // main counter, high 32 bits

/// CAP bits: COUNT_SIZE_CAP (bit 15) — 64-bit main counter supported.
const ID_COUNT_SIZE_CAP: u32 = 1 << 15;
/// CFG bit 0: ENABLE_CNF — main counter runs.
const CFG_ENABLE: u32 = 1;

/// MMIO base (identity-mapped VA) once initialized; 0 = not present.
static BASE: AtomicU64 = AtomicU64::new(0);
/// HPET ticks per nominal-domain tick (CLOCK_FREQ units). QEMU: 100 MHz
/// HPET / 10 MHz nominal = 10. 0 until `init` validated the period.
static DIV: AtomicU64 = AtomicU64::new(0);
/// Value added to the HPET-derived reading so switching the timebase
/// onto it cannot move the clock backward (same discipline as the TSC
/// switch in x86_64.rs).
static BOOT_OFFSET: AtomicU64 = AtomicU64::new(0);

/// True once the HPET is the active timekeeper.
#[inline]
pub fn ready() -> bool {
    BASE.load(Ordering::Acquire) != 0
}

/// 32-bit MMIO read at `base + off`.
///
/// SAFETY (caller): `base` must be an initialized, mapped HPET block.
#[inline]
unsafe fn r32(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

/// 32-bit MMIO write at `base + off`.
///
/// SAFETY (caller): `base` must be an initialized, mapped HPET block.
#[inline]
unsafe fn w32(base: usize, off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) }
}

/// Read the 64-bit main counter (two 32-bit halves).  The HPET-spec
/// discipline compares the HIGH halves around the low-half read: at
/// 100 MHz the low 32 bits advance between any two MMIO reads (so
/// comparing low halves would loop forever), while the high half
/// changes once every 2^32 / 1e8 s ~= 429 s.
///
/// The counter is 64-bit on QEMU (COUNT_SIZE_CAP) and at 100 MHz wraps
/// after ~5.8 kyr, so u64 arithmetic needs no wrap handling.
#[inline]
pub fn read_counter() -> u64 {
    let base = BASE.load(Ordering::Acquire) as usize;
    debug_assert!(base != 0);
    loop {
        // SAFETY: BASE != 0 implies an initialized, mapped HPET.
        let hi1 = unsafe { r32(base, HPET_CNTR_HI) } as u64;
        let lo = unsafe { r32(base, HPET_CNTR_LO) } as u64;
        let hi2 = unsafe { r32(base, HPET_CNTR_HI) } as u64;
        if hi1 == hi2 {
            return (hi1 << 32) | lo;
        }
    }
}

/// Current time in the nominal CLOCK_FREQ domain (HPET-based).
#[inline]
pub fn read_time_nominal() -> u64 {
    let div = DIV.load(Ordering::Acquire);
    read_counter() / div + BOOT_OFFSET.load(Ordering::Acquire)
}

/// Discover, enable, and verify the HPET.  Idempotent; returns true when
/// the HPET became (or already was) the active timekeeper.
///
/// Runs with interrupts disabled on the BSP before any secondary CPU is
/// online (called from main.rs right after the device mappings exist),
/// so the BASE/DIV/BOOT_OFFSET publication order needs no locking.
pub fn init() -> bool {
    if ready() {
        return true;
    }
    // Debugging gate (dfx discipline): "nohpet" keeps the calibrated-TSC
    // timebase so clocksource behavior can be A/B compared.
    if crate::cmdline::get_param("nohpet").is_some() {
        return false;
    }

    // ACPI HPET table: header (36 bytes) + timer block ID (8) + generic
    // address structure (12, address u64 at +8) + counter/flags.  Fall
    // back to the fixed q35 base when the table is missing.
    let base = crate::drivers::intc::apic::find_hpet_base().unwrap_or(HPET_FIXED_BASE);
    if base == 0 || base & 0xFFF != 0 {
        return false;
    }
    // Bases inside the pre-mapped uncached device windows (q35 PCI MMIO
    // hole covers the fixed 0xFED00000) need no new mapping; anything
    // else gets one 4K uncached page, the same flags
    // setup_device_mappings uses.
    if !crate::arch::x86_64::mm::mmu_init::kernel_device_window_pte(base as u64).is_some() {
        let flags = crate::arch::mm::pagetable::PageTableEntry::P
            | crate::arch::mm::pagetable::PageTableEntry::RW
            | crate::arch::mm::pagetable::PageTableEntry::ACCESSED
            | crate::arch::mm::pagetable::PageTableEntry::DIRTY
            | crate::arch::mm::pagetable::PageTableEntry::IO;
        // SAFETY: identity-mapping one uncached device page for the
        // HPET register block (reserved firmware region, never RAM).
        unsafe {
            crate::arch::mm::mmu_init::map_kernel_region(
                base as u64,
                base as u64,
                0x1000,
                flags,
            );
        }
    }

    // Validate the block: vendor ID nonzero, 64-bit counter capability,
    // period within the HPET spec bound (<= 100 ns) and an integer
    // multiple of the nominal 10 MHz domain.
    // SAFETY: base validated above.
    let id = unsafe { r32(base, HPET_ID) };
    if id >> 16 == 0 || id & ID_COUNT_SIZE_CAP == 0 {
        return false;
    }
    let period_fs = unsafe { r32(base, HPET_ID + 4) } as u64;
    // 1 fs .. 100 ns; zero or huge periods mean no HPET / broken model.
    if period_fs == 0 || period_fs > 100_000_000 {
        return false;
    }
    let hpet_hz = 1_000_000_000_000_000 / period_fs;
    if hpet_hz < TIMER_CLOCK_FREQ_HZ || hpet_hz % TIMER_CLOCK_FREQ_HZ != 0 {
        // Nominal-domain conversion would not be exact (real HW with a
        // non-decade period): keep the calibrated-TSC timebase instead.
        return false;
    }
    let div = hpet_hz / TIMER_CLOCK_FREQ_HZ;

    // Enable the main counter (LEGACY stays clear: the PIT must keep
    // ticking for the IRQ0 fallback path and the TSC/LAPIC calibration).
    // SAFETY: base validated above.
    let cfg = unsafe { r32(base, HPET_CFG) };
    unsafe { w32(base, HPET_CFG, cfg | CFG_ENABLE) };

    // Verify it actually counts: at >= 10 MHz the counter must advance
    // during a short bounded spin.  Retry with a longer spin before
    // giving up (first MMIO reads after device bring-up can be slow).
    let mut ok = false;
    for spin in [20_000u64, 2_000_000] {
        // SAFETY: base validated above.
        let c0 = unsafe { read_counter_raw(base) };
        for _ in 0..spin {
            core::hint::spin_loop();
        }
        // SAFETY: base validated above.
        let c1 = unsafe { read_counter_raw(base) };
        if c1 > c0 {
            ok = true;
            break;
        }
    }
    if !ok {
        // Disable again — do not leave a half-initialized counter.
        // SAFETY: base validated above.
        unsafe { w32(base, HPET_CFG, cfg) };
        return false;
    }

    // Publish: offset first (the pre-HPET clock value at the switch
    // instant), then the base that flips read_time() onto the HPET.
    // Interrupts are off and APs are parked, so this is race-free.
    let continuity = crate::drivers::timer::x86_64::pre_hpet_time();
    DIV.store(div, Ordering::Release);
    BOOT_OFFSET.store(continuity, Ordering::Release);
    BASE.store(base as u64, Ordering::Release);
    true
}

/// One 64-bit counter read against an explicit base (init-time verify).
/// Same high-half discipline as [`read_counter`]; at most one re-read
/// when the 429 s high-half boundary lands mid-read.
///
/// SAFETY (caller): `base` must be a mapped HPET block.
unsafe fn read_counter_raw(base: usize) -> u64 {
    unsafe {
        let hi1 = r32(base, HPET_CNTR_HI) as u64;
        let lo = r32(base, HPET_CNTR_LO) as u64;
        let hi2 = r32(base, HPET_CNTR_HI) as u64;
        if hi1 == hi2 {
            (hi1 << 32) | lo
        } else {
            let hi = r32(base, HPET_CNTR_HI) as u64;
            let lo = r32(base, HPET_CNTR_LO) as u64;
            (hi << 32) | lo
        }
    }
}
