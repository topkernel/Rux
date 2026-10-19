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
/// The value BASE held at successful init. The HPET is a FIXED MMIO
/// block — this value can never legitimately change after init. b77
/// capture: a wild kernel write changed BASE 0xffff8900fed00000 →
/// 0xffff89004ed00000 mid-boot (one byte of the physical address); every
/// subsequent clock read dereferenced an unmapped physmap address and
/// the whole boot graph froze (all tasks crawling, serial silent).
static BASE_LOCKED: AtomicU64 = AtomicU64::new(0);
/// Redundant encoding of BASE_LOCKED (bitwise complement). The wt1
/// capture (vsync-starvation hunt, 2026-10-19) showed the wild-write
/// engine can take BASE AND the adjacent BASE_LOCKED in one smash:
/// every read_time() then returned boot-epoch garbage, nanosleep
/// deadlines landed minutes off (compose-content's 41.5 s dead stop and
/// 0.5-0.7 s usleep blowouts), and the CPU finally wedged inside
/// r32 with the smashed address (RIP=hpet::r32, RDI=0xffff89004ed000f4,
/// IF=0). Linux parity for that family: the fixmap clocksource address
/// is derived, never read back from scribbleable memory. Here the READ
/// PATH derives its address from this validated pair (and falls back to
/// the fixed q35 alias on any mismatch) — BASE itself is only the
/// ready flag.
static BASE_LOCKED_INV: AtomicU64 = AtomicU64::new(u64::MAX);
/// HPET ticks per nominal-domain tick (CLOCK_FREQ units). QEMU: 100 MHz
/// HPET / 10 MHz nominal = 10. 0 until `init` validated the period.
static DIV: AtomicU64 = AtomicU64::new(0);
/// Redundant encoding of DIV (bitwise complement). A zeroed DIV would
/// panic the divide in read_time_nominal; validation heals it back (or
/// falls back to the q35 constant 10).
static DIV_INV: AtomicU64 = AtomicU64::new(u64::MAX);
/// Value added to the HPET-derived reading so switching the timebase
/// onto it cannot move the clock backward (same discipline as the TSC
/// switch in x86_64.rs).
static BOOT_OFFSET: AtomicU64 = AtomicU64::new(0);
/// Redundant encoding of BOOT_OFFSET (bitwise complement).
static BOOT_OFFSET_INV: AtomicU64 = AtomicU64::new(u64::MAX);
/// Per-CPU last accepted reading (monotonic clamp — the clocksource
/// watchdog analogue). A garbage read that lands below this CPU's last
/// good time is re-read once and then FROZEN at the last good value
/// (a brief stall beats a minutes-long jump; the timer wheel's hres
/// deadlines are absolute, so a backward jump strands every sleeper
/// until the clock re-reaches the pre-jump value — the dead-stop
/// morphology). Per-CPU slots avoid a shared-line write on the hottest
/// path in the system.
static LAST_GOOD: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Backward steps up to this size are clamped away as jitter (50 ms in
/// the nominal 10 MHz tick domain).
const SMALL_BACKWARD_CLAMP_TICKS: u64 =
    50 * crate::config::TIMER_CLOCK_FREQ_HZ / 1000;

/// Direct-console heal report, rate-gated to one line per 25 jiffies
/// (callers may hold the TIMERS lock — R34 discipline: putchar_no_lock
/// only, never printk from the clock read path).
fn heal_report(prefix: &[u8], bad: u64, good: u64) {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < 25 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    for &b in prefix {
        crate::console::putchar_no_lock(b);
    }
    let mut v = bad;
    for _ in 0..16 {
        let nb = (v >> 60) as u8;
        crate::console::putchar_no_lock(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        v <<= 4;
    }
    crate::console::putchar_no_lock(b'-');
    crate::console::putchar_no_lock(b'>');
    let mut v = good;
    for _ in 0..16 {
        let nb = (v >> 60) as u8;
        crate::console::putchar_no_lock(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        v <<= 4;
    }
    crate::console::putchar_no_lock(b'\n');
}

/// Validate the locked base against its complement copy. Returns the
/// alias VA to read, healing the pair (or falling back to the fixed q35
/// alias) when a wild write has touched either half.
fn validated_read_base() -> usize {
    let locked = BASE_LOCKED.load(Ordering::Acquire);
    let inv = BASE_LOCKED_INV.load(Ordering::Acquire);
    if locked != 0 && locked == !inv {
        return locked as usize;
    }
    // Corrupt pair: a wild write hit BASE_LOCKED and/or its complement.
    // Fall back to the fixed q35 alias — the correct block on this
    // platform (the ACPI table reports the same 0xFED00000) — and heal
    // the pair so later reads take the fast path again. If the locked
    // value merely lost its complement (still plausible), keep it.
    let fallback = {
        use crate::arch::mm::memory_layout::device_alias;
        device_alias(HPET_FIXED_BASE as u64) as usize
    };
    let good = if locked != 0 && (locked as usize) & 0xFFF == 0 {
        // Address still looks structurally valid: re-affirm it.
        locked
    } else {
        fallback as u64
    };
    heal_report(b"hpet: clock base healed ", locked, good);
    BASE_LOCKED.store(good, Ordering::Release);
    BASE_LOCKED_INV.store(!good, Ordering::Release);
    BASE.store(good, Ordering::Release);
    good as usize
}

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
/// changes once every 2^32 / 1e8 s ~= 42.9 s.
///
/// The counter is 64-bit on QEMU (COUNT_SIZE_CAP) and at 100 MHz wraps
/// after ~5.8 kyr, so u64 arithmetic needs no wrap handling.
#[inline]
pub fn read_counter() -> u64 {
    // The read address is DERIVED from the validated locked pair, never
    // from BASE alone (wt1 capture: BASE and the old BASE-vs-LOCKED
    // comparison were both defeated by one smash of the adjacent
    // statics). BASE stays a pure ready flag here.
    let base = validated_read_base();
    // Re-arm the ready flag if a wild write zeroed it (ready()==false
    // would degrade read_time() to the coarse jiffies fallback).
    if BASE.load(Ordering::Acquire) as usize != base {
        BASE.store(base as u64, Ordering::Release);
    }
    loop {
        // SAFETY: validated_read_base returned a validated, mapped block.
        let hi1 = unsafe { r32(base, HPET_CNTR_HI) } as u64;
        let lo = unsafe { r32(base, HPET_CNTR_LO) } as u64;
        let hi2 = unsafe { r32(base, HPET_CNTR_HI) } as u64;
        if hi1 == hi2 {
            return (hi1 << 32) | lo;
        }
    }
}

/// Current time in the nominal CLOCK_FREQ domain (HPET-based), hardened
/// against the wild-write engine:
///  1. the counter address is derived from the validated locked pair
///     (fallback: fixed q35 alias);
///  2. DIV is validated (a zeroed DIV must panic-proof fall back);
///  3. BOOT_OFFSET is validated against its complement copy;
///  4. the result is clamped monotonic per CPU — a garbage read that
///     lands below this CPU's last accepted value is re-read once and
///     then FROZEN (never jump the clock backward: hres deadlines are
///     absolute, a backward jump strands every sleeper).
#[inline]
pub fn read_time_nominal() -> u64 {
    let div = {
        let d = DIV.load(Ordering::Acquire);
        let dinv = DIV_INV.load(Ordering::Acquire);
        if d != 0 && d == !dinv {
            d
        } else {
            // Scribbled (or zeroed): heal with the q35 constant — the
            // platform's only validated divisor (init rejects hardware
            // whose period is not an integer multiple of the nominal
            // domain, and this kernel targets QEMU q35/virt).
            const QEMU_DIV: u64 = 10; // 100 MHz HPET / 10 MHz nominal
            heal_report(b"hpet: clock div healed ", d, QEMU_DIV);
            DIV.store(QEMU_DIV, Ordering::Release);
            DIV_INV.store(!QEMU_DIV, Ordering::Release);
            QEMU_DIV
        }
    };
    let offset = {
        let o = BOOT_OFFSET.load(Ordering::Acquire);
        let oinv = BOOT_OFFSET_INV.load(Ordering::Acquire);
        if o == !oinv {
            o
        } else {
            // Complement lost: the plain value is the better guess (a
            // smashed value usually reads as a pointer, not the small
            // early-boot offset; the monotonic clamp below catches any
            // residual poison anyway). Heal the pair.
            heal_report(b"hpet: clock offset healed ", oinv, o);
            BOOT_OFFSET_INV.store(!o, Ordering::Release);
            o
        }
    };
    let raw = read_counter() / div + offset;

    // Per-CPU monotonic guard (clocksource watchdog analogue). The read
    // is already address/DIV/offset validated; what remains is read
    // jitter and transient residual poison:
    //  - a SMALL backward step (<= 50 ms) is clamped to the last good
    //    value (tearing/rounding artifacts must never move the clock
    //    back — hres deadlines are absolute);
    //  - a LARGE backward step is a CORRECTION (e.g. a forward-poisoned
    //    value was accepted a moment before validation healed the
    //    offset) and is HONORED — freezing at a poisoned-high value
    //    would stall the clock forever. Logged so the event is visible.
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return raw;
    }
    let last = LAST_GOOD[cpu].load(Ordering::Acquire);
    if raw >= last.saturating_sub(SMALL_BACKWARD_CLAMP_TICKS) {
        let accept = raw.max(last);
        LAST_GOOD[cpu].store(accept, Ordering::Release);
        return accept;
    }
    heal_report(b"hpet: clock corrected backward ", last, raw);
    LAST_GOOD[cpu].store(raw, Ordering::Release);
    raw
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
    // Runtime counter reads happen from timer-interrupt context under
    // USER CR3s — route them through the kernel-half device alias (Linux
    // fixmap discipline), never through a low identity VA. The alias
    // window [0xb0000000, 4GB) covers q35's fixed HPET base; an ACPI
    // table outside it gets one explicit alias page.
    let alias_va = {
        use crate::arch::mm::memory_layout::{
            device_alias, DEVICE_ALIAS_PHYS_BASE,
        };
        let in_window = (base as u64) >= DEVICE_ALIAS_PHYS_BASE as u64
            && (base as u64) < 0x1_0000_0000;
        if !in_window {
            let flags = crate::arch::mm::pagetable::PageTableEntry::P
                | crate::arch::mm::pagetable::PageTableEntry::RW
                | crate::arch::mm::pagetable::PageTableEntry::ACCESSED
                | crate::arch::mm::pagetable::PageTableEntry::DIRTY
                | crate::arch::mm::pagetable::PageTableEntry::IO;
            // SAFETY: mapping one uncached alias page for the HPET block
            // in the kernel half (reserved firmware region, never RAM).
            unsafe {
                let va = 0xffff_8a00_0000_0000u64 + (base as u64 & 0xF_FFFF_F000);
                crate::arch::mm::mmu_init::map_kernel_region(
                    va,
                    base as u64 & !0xFFFu64,
                    0x1000,
                    flags,
                );
                va
            }
        } else {
            device_alias(base as u64)
        }
    };
    let base = alias_va as usize;

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
    // The complement copies arm the read-path validation (wt1: the
    // wild-write engine took BASE and BASE_LOCKED together — adjacent
    // statics — and poisoned every clock read; the pair + fixed-alias
    // fallback make the read path derive a good address regardless).
    let continuity = crate::drivers::timer::x86_64::pre_hpet_time();
    DIV.store(div, Ordering::Release);
    DIV_INV.store(!div, Ordering::Release);
    BOOT_OFFSET.store(continuity, Ordering::Release);
    BOOT_OFFSET_INV.store(!continuity, Ordering::Release);
    BASE.store(base as u64, Ordering::Release);
    BASE_LOCKED.store(base as u64, Ordering::Release);
    BASE_LOCKED_INV.store(!(base as u64), Ordering::Release);
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
