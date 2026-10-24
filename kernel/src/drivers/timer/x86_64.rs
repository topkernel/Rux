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

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

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

/// Fractional shift used by the TSC -> nominal-domain mapping (pub for
/// hpet::calibrate_tsc — keep in sync with the promotion math).
///
/// sc110-114 ROOT CAUSE NOTE: this was 12 for the whole sc-series, where
/// mult = ceil(CLOCK_FREQ << 12 / tsc_hz) = ceil(9.5434) = 10 — a 10.5%
/// resolution grid — so the mapping ran 10/9.5434 = 4.796% FAST against
/// the HPET on every boot (sc114: "TSC 4292 MHz" -> 4,292.4 MHz measured,
/// ceil still 10). The "one-time ~25 s TSC-vs-HPET step" that parked the
/// timekeeper behind `tscclk` (and inspired the per-CPU vCPU-offset skew
/// theory) was exactly this: 4.796% x uptime crosses the resync's 25 s
/// hard-rebase threshold at uptime 522.17 s — deterministic, and the
/// sc113/sc114 rebases both fired at 522.17 s with lead 250.41M ticks to
/// within 0.6 ms, matching the rounding error to 4 significant digits.
/// (The per-CPU corrections measured ~-21.9 ms on the AP = the same
/// 4.796% x 0.456 s AP-bringup instant, not vCPU skew.) At shift 32 the
/// mult grid is ~100 ppm and round-to-nearest halves the residual, so
/// the 1 Hz bounded resync absorbs what is left.
pub const TSC_MULSHIFT: u32 = 32;

/// Map a raw TSC delta into the nominal 10 MHz domain at full precision:
/// MULTIPLY FIRST, SHIFT SECOND.
///
/// sc204 TICKPROBE (2026-10-24, launcher leg 12): the shift-first form
/// `(delta >> 32) * mult` introduced by 9e5f0147 (shift 12 -> 32) left
/// the kernel clock with a granularity of `mult` nominal ticks — at
/// 4.29 GHz that is ONE STEP EVERY 2^32 TSC ticks = 1.0008 SECONDS. Both
/// of the probe's nominal-2 ms sample windows measured dt = 10,008,094
/// ticks (one full step, identical twice); every sc200-sc203
/// WW IRQ-OFF-WINDOW read an age of exactly 1000 ms and every
/// WAKE-STRANDED strand quantized to the same grid, because read_time
/// itself only moved once per second. Every timed wait in the kernel
/// (nanosleep/expiry checks, scheduler deltas, binder slices, the OH
/// boot chain) coarsened to 1 s steps — the quiet-quiet soft-wedge /
/// crawl shape. At shift 12 the granularity was 2^12 TSC ticks (~1 us)
/// and nobody noticed the form; raising the shift made it 1 s.
/// Multiply-then-shift keeps the ~3 ppm rate accuracy of 9e5f0147 AND
/// restores ~0.23 ns granularity. The u128 product of delta (< uptime x
/// 4.3 GHz, far under 2^64) and mult (~10M) cannot overflow u128.
#[inline]
fn tsc_delta_nominal(delta: u64, mult: u64) -> u64 {
    (((delta as u128) * (mult as u128)) >> TSC_MULSHIFT) as u64
}

/// TSC timekeeping state (written once by `lapic_time_calibrated`):
/// nominal = ((tsc - base_tsc) >> SHIFT) * MULT, where
/// MULT = round(CLOCK_FREQ << SHIFT / tsc_freq).
static TSC_MULT: AtomicU64 = AtomicU64::new(0);
static TSC_BASE: AtomicU64 = AtomicU64::new(0);
/// Offset added to the TSC-derived value so the switch from the
/// jiffies-derived clock cannot run time backward.
static TSC_CONTINUITY_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Per-CPU additive correction to the TSC-derived nominal clock.
///
/// KVM can give every vCPU its own TSC offset (rates identical, offsets
/// not). Each AP measures its own (HPET - TSC-derived) delta once, right
/// before its first tick, and `read_time()` applies the current CPU's
/// correction — the same shape as Linux's per-CPU cyc2ns offsets.
/// 0 = not yet calibrated (index 0 stays 0: the BSP's correction is
/// folded into TSC_CONTINUITY_OFFSET at promote time).
///
/// HISTORY NOTE: sc110-114's "one-time ~25 s TSC-vs-HPET step" was NOT
/// vCPU skew — it was the shift-12 ceil rounding running the mapping
/// 4.796% fast (see the TSC_MULSHIFT note; the sc114 AP "skew" of
/// -21.9 ms was 4.796% x the 0.456 s AP-bringup instant). The per-CPU
/// discipline stays: it is correct in principle and costs one acquire
/// load on the read path.
///
/// Residual calibration noise between CPUs still makes tiny BACKWARD
/// steps possible on thread migration; sc113 measured one such step
/// under the OLD 4.8% mapping error (samgr printed `cost:4294946ms` —
/// a 21 ms backward step wrapping a u32 microsecond diff, which aborted
/// SA loads and killed the OsAccount host's clients). Two guards: the
/// calibration takes the MIN of three bracketed samples (noise floor),
/// and read_time ratchets through TSC_LAST (published time never goes
/// backward).
static TSC_CPU_OFF: [AtomicI64; crate::config::MAX_CPUS] =
    [const { AtomicI64::new(0) }; crate::config::MAX_CPUS];

/// Monotonic ratchet: the largest value ever returned by read_time().
/// Plain unsynchronized max-store — a lost race only leaves the ratchet
/// one store behind, never lets it decrease.
static TSC_LAST: AtomicU64 = AtomicU64::new(0);

/// True once the TSC became the timekeeper.
static TSC_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// True when the active TSC timekeeper was calibrated against the HPET
/// (sc109: the OH launcher child burned its whole 3 s appspawn watchdog
/// in clock_gettime — no vDSO, so every call paid the syscall AND an
/// HPET MMIO read, which under KVM is a VM exit. KVM's TSC is invariant
/// and vCPU-synchronized, so after calibration RDTSC serves the nominal
/// clock with no exits; a 1 Hz resync against the HPET bounds drift).
static TSC_FROM_HPET: AtomicU64 = AtomicU64::new(0);

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

/// High-resolution nanoseconds since boot (see the riscv64 twin).
#[inline]
pub fn monotonic_ns_since_boot() -> u64 {
    ((read_time() as u128) * 1_000_000_000u128 / (CLOCK_FREQ as u128)) as u64
}

/// Read current time in the nominal CLOCK_FREQ clock domain.
///
/// TSC-derived first (fast: RDTSC, no VM exit) once calibrated — against
/// the LAPIC tick when there is no HPET, against the HPET itself when
/// there is (hpet::calibrate_tsc); HPET-derived (one MMIO read per call)
/// only in the pre-calibration window; jiffies-derived before either.
#[inline]
pub fn read_time() -> u64 {
    if TSC_ACTIVE.load(Ordering::Acquire) != 0 {
        let mult = TSC_MULT.load(Ordering::Acquire);
        let now = crate::arch::cpu::read_counter();
        let base = TSC_BASE.load(Ordering::Acquire);
        let delta = now.wrapping_sub(base);
        let mut v = tsc_delta_nominal(delta, mult)
            + TSC_CONTINUITY_OFFSET.load(Ordering::Acquire);
        // Per-CPU vCPU TSC-offset correction (TSC_CPU_OFF doc). Signed
        // add keeps cross-CPU monotonicity inside the i64 nominal range;
        // an uncalibrated CPU (0) reads the uncorrected value.
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < crate::config::MAX_CPUS {
            v = v.wrapping_add(TSC_CPU_OFF[cpu].load(Ordering::Acquire) as u64);
        }
        // Monotonic ratchet (sc113: per-CPU calibration noise stepped
        // the clock ~21 ms backward once, samgr's u32-us duration math
        // wrapped to 4294967ms and aborted SA loads). A value behind the
        // last published time is clamped up to it — time briefly freezes
        // (never more than the residual noise) instead of stepping back.
        let last = TSC_LAST.load(Ordering::Relaxed);
        if v > last {
            TSC_LAST.store(v, Ordering::Relaxed);
        } else if v < last {
            v = last;
        }
        v
    } else if crate::drivers::timer::hpet::ready() {
        crate::drivers::timer::hpet::read_time_nominal()
    } else {
        get_jiffies() * TIME_SLICE_TICKS
    }
}

/// Promote the calibrated TSC to timekeeper (hpet::calibrate_tsc).
///
/// `mult` maps (tsc >> TSC_MULSHIFT) deltas to the nominal domain,
/// `base_tsc` is the TSC stamp captured with `nominal_at_base`, and the
/// continuity offset makes tsc_nominal(base) == nominal_at_base so the
/// switch from the HPET clock cannot run time backward.
pub fn promote_tsc_timekeeper(mult: u64, base_tsc: u64, nominal_at_base: u64) {
    TSC_MULT.store(mult, Ordering::Release);
    TSC_BASE.store(base_tsc, Ordering::Release);
    TSC_CONTINUITY_OFFSET.store(nominal_at_base, Ordering::Release);
    TSC_ACTIVE.store(1, Ordering::Release);
    TSC_FROM_HPET.store(1, Ordering::Release);
}

/// 1 Hz drift discipline for the HPET-calibrated TSC (timer tick calls
/// this; see TSC_FROM_HPET). Corrections are PER-CPU (TSC_CPU_OFF):
/// vCPU TSC offsets differ, so a single global step would yo-yo the
/// clock as the tick migrates CPUs. A TSC that fell behind (or a KVM
/// TSC resync) is stepped forward, clamped to 10 ms per resync so a
/// poisoned HPET read cannot jump the clock; a TSC ahead by >250 ms is
/// REPORTED (post shift-32 this should be silent forever — the marker
/// proves the mapping error stays under a quarter second), and a lead
/// >25 s is pulled back as a last resort (with the shift-12 rounding
/// error fixed that means a real vCPU TSC smash, not calibration).
pub fn resync_tsc_against_hpet() {
    if TSC_FROM_HPET.load(Ordering::Acquire) == 0 {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    let mult = TSC_MULT.load(Ordering::Acquire);
    let base = TSC_BASE.load(Ordering::Acquire);
    let off = TSC_CONTINUITY_OFFSET.load(Ordering::Acquire);
    if mult == 0 {
        return;
    }
    let raw = crate::arch::cpu::read_counter();
    let tsc_nom_uncorr = tsc_delta_nominal(raw.wrapping_sub(base), mult).wrapping_add(off);
    let hpet_nom = crate::drivers::timer::hpet::read_time_nominal();
    let tsc_nom = tsc_nom_uncorr.wrapping_add(TSC_CPU_OFF[cpu].load(Ordering::Acquire) as u64);
    if hpet_nom > tsc_nom {
        let behind = hpet_nom - tsc_nom;
        // 10 ms nominal-domain clamp (CLOCK_FREQ=10 MHz → 100_000 ticks).
        let step = behind.min(100_000);
        if behind > 100_000 {
            resync_report(b"tsc-clock: fell behind ", behind);
        }
        TSC_CPU_OFF[cpu].fetch_add(step as i64, Ordering::AcqRel);
    } else if tsc_nom > hpet_nom && tsc_nom - hpet_nom > 2_500_000 {
        // 250 ms..25 s lead: REPORT ONLY. Proof-of-fix marker for the
        // shift-32 correction — at ~3 ppm residual mapping error this
        // needs ~23 h of uptime to fire, so any print here means a NEW
        // divergence (or an HPET read stall), worth reading.
        resync_report(b"tsc-clock: lead ", tsc_nom - hpet_nom);
    } else if tsc_nom - hpet_nom > 250_000_000 {
        // >25 s lead on THIS cpu: either a vCPU TSC-offset jump (skew)
        // or a smashed mapping. Pull this CPU's offset back into sync
        // and print the raw pair for offline skew-vs-smash triage.
        let lead = tsc_nom - hpet_nom;
        resync_report(b"tsc-clock: rebase, lead ", lead);
        crate::pr_warn!(
            "tsc-clock: rebase cpu={} raw_tsc={:#x} tsc_nom={:#x} hpet_nom={:#x} base={:#x} mult={:#x} cpu_off={:#x}",
            cpu,
            raw,
            tsc_nom,
            hpet_nom,
            base,
            mult,
            TSC_CPU_OFF[cpu].load(Ordering::Acquire)
        );
        TSC_CPU_OFF[cpu].fetch_sub(lead as i64, Ordering::AcqRel);
    }
}

/// One-shot per-CPU calibration of TSC_CPU_OFF (TSC_CPU_OFF doc): each
/// AP measures its own (HPET - TSC-derived) delta right after its LAPIC
/// timer is programmed and before its first tick, closing the vCPU
/// TSC-offset skew window (sc110's one-time ~25 s step). Runs on the AP
/// itself with IRQs off; safe to call multiple times.
pub fn calibrate_cpu_tsc_offset(cpu: usize) {
    if TSC_FROM_HPET.load(Ordering::Acquire) == 0 {
        return; // TSC not the HPET-calibrated timekeeper
    }
    if cpu >= crate::config::MAX_CPUS || cpu == 0 {
        return; // index 0 is folded into TSC_CONTINUITY_OFFSET at promote
    }
    let mult = TSC_MULT.load(Ordering::Acquire);
    let base = TSC_BASE.load(Ordering::Acquire);
    let off = TSC_CONTINUITY_OFFSET.load(Ordering::Acquire);
    if mult == 0 {
        return;
    }
    let raw = crate::arch::cpu::read_counter();
    let mut delta = i64::MAX;
    // Three bracketed samples, take the MINIMUM (hpet - tsc_nom): the
    // HPET MMIO read can stall (sc113 measured ~21 ms of read skew right
    // after AP startup), and a too-LARGE correction is what steps the
    // clock backward on migration; a slightly-small one is absorbed by
    // the 1 Hz forward-only resync and the read_time ratchet.
    for _ in 0..3 {
        let hpet_nom = crate::drivers::timer::hpet::read_time_nominal();
        let t_after = crate::arch::cpu::read_counter();
        // Same base/mult math for each bracket's TSC stamps.
        let nom = |raw_t: u64| -> i64 {
            tsc_delta_nominal(raw_t.wrapping_sub(base), mult)
                .wrapping_add(off) as i64
        };
        // Lower bound of the true offset: the HPET read happened at or
        // before the t_after stamp, so hpet_nom - nom(t_after) can only
        // underestimate the correction.
        let d = hpet_nom as i64 - nom(t_after);
        if d < delta {
            delta = d;
        }
    }
    let tsc_nom_now = tsc_delta_nominal(raw.wrapping_sub(base), mult)
        .wrapping_add(off) as i64;
    // Negative-sum guard: a vCPU that reads BEHIND by more than the
    // current uptime would wrap the u64 clock forward by ~2^64. Clamp
    // the correction so corrected >= 0 (the CPU then reads a hair fast
    // until uptime outgrows the skew — never wraps).
    if tsc_nom_now + delta < 0 {
        delta = -tsc_nom_now;
    }
    // Residual is the vCPU TSC-offset skew — apply it whatever the size
    // (25 s is the observed magnitude), but say so beyond 1 ms so the
    // boot log documents each vCPU's skew once.
    if delta.abs() > 10_000 {
        crate::pr_info!(
            "tsc-clock: cpu {} vCPU TSC offset skew {:+} us vs HPET (corrected)",
            cpu,
            delta / 10
        );
    }
    TSC_CPU_OFF[cpu].store(delta, Ordering::Release);
}

/// Rate-limited one-line report for resync events (max 8 per boot).
fn resync_report(msg: &[u8], v: u64) {
    static N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8 {
        return;
    }
    let mut line = alloc::string::String::new();
    use core::fmt::Write;
    let _ = write!(line, "{}{v}", core::str::from_utf8(msg).unwrap_or(""));
    crate::pr_warn!("{}", line);
}

/// True when the calibrated TSC is the active timekeeper (no HPET).
/// Exposed for diagnostics (dfx monowatch names the clocksource).
pub fn tsc_active() -> bool {
    TSC_ACTIVE.load(Ordering::Acquire) != 0
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
    // nominal = (tsc >> S) * round(CLOCK_FREQ << S / tsc_freq) — nearest
    // rounding, NOT ceil: at a coarse shift a ceiled mult runs the whole
    // mapping fast by up to one mult quantum (sc110-114: 4.796% fast at
    // shift 12; see the TSC_MULSHIFT note).
    let mult = ((CLOCK_FREQ << TSC_MULSHIFT) + tsc_freq / 2) / tsc_freq;
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
    // Wakewatch: per-CPU timer-IRQ arrival count (storm detector — the
    // heartbeat prints the per-second delta) + last-arrival stamp, so a
    // TIMER-SOFTIRQ-STALL report can tell "this CPU's timer IRQs stopped
    // arriving too" (long IRQ-off window: putc/uart_write/console lock)
    // from "ticks kept arriving but softirq dispatch was skipped"
    // (in_softirq deferral / stuck SOFTIRQ_IN_PROGRESS).
    {
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < crate::config::MAX_CPUS {
            crate::timer::WW_TICK_IRQ[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            crate::timer::ww_note_tick_arrival(cpu);
        }
    }
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

    // Timer softirq for TCP timers (retransmission, delayed ACK).
    // sc184 (lane 2, perf-audit F1): the raise was unconditional —
    // 199 raises/s per CPU for a usually-empty TCP table (pa3), each
    // costing an irq-exit softirq dispatch pass at IF=0. The handler
    // (tcp_timer_tick) already early-returns when the table's high-water
    // count is 0, so gating the raise on the same count is
    // behavior-identical minus the wasted dispatch. count() is a
    // high-water mark: once any TCP socket has existed the raise
    // resumes unconditionally (and the handler then has real work).
    if crate::net::tcp::get_tcp_socket_table().count() > 0 {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Timer as usize,
        );
    }

    // Hrtimer softirq for software timers. Same F1 gate: skip the raise
    // while no timer has ever been armed AND no high-resolution deadline
    // is due (timer::timer_softirq_work_pending is lock-free: a monotonic
    // ever-armed atomic plus the HRES_NEXT deadline word). Before the
    // first timer this skips the vector entirely; afterwards it never
    // gates (the handler sweeps real timers).
    if crate::timer::timer_softirq_work_pending() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
        );
    }
}
