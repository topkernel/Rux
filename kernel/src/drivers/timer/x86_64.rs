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

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

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

/// TSC timekeeping state lives in the vDSO DATA PAGE (mm/vdso.rs
/// x86_track) since the unified-track re-arm: ONE anchor
/// {cycle_last, base_mono_nom, mult} serves both the kernel's
/// read_time() and the user-side vDSO interpolation, bit-identically.
/// The only timer-local state left:
///
/// - TSC_ACTIVE: the page anchor is published (promote ran);
/// - TSC_FROM_HPET: the anchor was HPET-calibrated (gates the 1 Hz
///   resync);
/// - TSC_LAST: the kernel-side monotonic ratchet (see read_time).
static TSC_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// True when the active TSC timekeeper was calibrated against the HPET
/// (sc109: the OH launcher child burned its whole 3 s appspawn watchdog
/// in clock_gettime — no vDSO, so every call paid the syscall AND an
/// HPET MMIO read, which under KVM is a VM exit. KVM's TSC is invariant
/// and vCPU-synchronized, so after calibration RDTSC serves the nominal
/// clock with no exits; a 1 Hz resync against the HPET bounds drift).
static TSC_FROM_HPET: AtomicU64 = AtomicU64::new(0);

/// Monotonic ratchet: the largest value ever returned by read_time().
/// Plain unsynchronized max-store — a lost race only leaves the ratchet
/// one store behind, never lets it decrease.
///
/// On the unified track this NEVER engages on a healthy boot (the page
/// base only moves forward; the vCPU TSC never goes backward): it is
/// anomaly insurance for the >25 s rebase pull-back and any future
/// writer bug — the syscall clock freezes briefly instead of stepping
/// back. NOTE the vDSO track has no ratchet (userspace cannot see
/// TSC_LAST); that is exactly what makes the two tracks bit-identical,
/// and why the base must never move backward outside the rebase path.
static TSC_LAST: AtomicU64 = AtomicU64::new(0);

/// jiffies — global tick counter (advanced from the timer IRQ)
static JIFFIES: AtomicU64 = AtomicU64::new(0);
/// timebase value at the first LAPIC-tick jiffies update (grid origin).
static JIFFIES_BASE: AtomicU64 = AtomicU64::new(0);

/// Get current jiffies value (ticks since boot)
#[inline]
pub fn get_jiffies() -> u64 {
    JIFFIES.load(Ordering::Acquire)
}

/// Test-only deterministic jiffy advance (unit-test harness): the tests
/// run with the timer interrupt disabled and — since R87 keeps secondary
/// harts interrupt-less until boot_complete — nothing increments JIFFIES
/// during the suite. Tests that need "time has passed" semantics (e.g.
/// the tcp-karn RTT-sampling control) bump the counter explicitly
/// instead of relying on incidental ticks.
#[cfg(feature = "unit-test")]
pub fn test_advance_jiffies(n: u64) {
    JIFFIES.fetch_add(n, core::sync::atomic::Ordering::Release);
}

/// Floor-map an absolute timebase value onto the jiffy grid — the
/// read side of increment_jiffies_from_timebase's
/// `floor((now - base) / period)` derivation (0 before the grid origin
/// is established). The timer wheel uses this to derive an hres
/// entry's first-check jiffy from its precise time-CSR deadline, so
/// routing never trusts caller-supplied jiffy bookkeeping.
#[inline]
pub fn jiffy_floor_of_time(t: u64) -> u64 {
    let base = JIFFIES_BASE.load(Ordering::Acquire);
    if base == 0 {
        return 0;
    }
    t.saturating_sub(base) / TIME_SLICE_TICKS
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
/// TSC-derived first (fast: RDTSC, no VM exit) once the page anchor is
/// published — the UNIFIED TRACK (mm/vdso.rs x86_track): the same page
/// words, the same seq protocol, the same integer arithmetic the vDSO
/// functions run in userspace, so a syscall read and a vDSO read at the
/// same instant are bit-identical (LTP clock_gettime04's strict
/// adjacent-pair requirement). HPET-derived (one MMIO read per call)
/// only in the pre-promotion window; jiffies-derived before either.
#[inline]
pub fn read_time() -> u64 {
    if TSC_ACTIVE.load(Ordering::Acquire) != 0 {
        if let Some(mut v) =
            crate::mm::vdso::x86_track::tsc_track_now_nominal()
        {
            // Monotonic ratchet (TSC_LAST doc): never engages on a
            // healthy unified track; freezes the clock through the
            // rebase pull-back instead of stepping back.
            let last = TSC_LAST.load(Ordering::Relaxed);
            if v > last {
                TSC_LAST.store(v, Ordering::Relaxed);
            } else if v < last {
                v = last;
            }
            return v;
        }
        // Page unreadable (writer wedged — should be impossible: the
        // writer runs IRQ-off for a handful of stores): fall through to
        // the HPET/jiffies branches; the ratchet keeps the visible
        // clock monotonic across the excursion.
    }
    if crate::drivers::timer::hpet::ready() {
        crate::drivers::timer::hpet::read_time_nominal()
    } else {
        get_jiffies() * TIME_SLICE_TICKS
    }
}

/// Promote the calibrated TSC to timekeeper (hpet::calibrate_tsc /
/// lapic_time_calibrated).
///
/// `mult` maps TSC deltas to the nominal domain (mult-then-shift),
/// `base_tsc` is the TSC stamp captured with `nominal_at_base`, and the
/// continuity base makes track(base_tsc) == nominal_at_base so the
/// switch from the previous clock cannot run time backward. The anchor
/// is published to the vDSO data page (the single timekeeping state);
/// from this instant read_time() and the vDSO fast path compute the
/// same value from the same words.
pub fn promote_tsc_timekeeper(mult: u64, base_tsc: u64, nominal_at_base: u64) {
    crate::mm::vdso::x86_track::publish_tsc_anchor(mult, base_tsc, nominal_at_base);
    TSC_ACTIVE.store(1, Ordering::Release);
    TSC_FROM_HPET.store(1, Ordering::Release);
}

/// 1 Hz drift discipline for the HPET-calibrated unified track (timer
/// tick calls this; see TSC_FROM_HPET). The correction is a SINGLE
/// FORWARD STEP of the page anchor's base (tsc_anchor_step_forward):
/// forward moves are safe for the strict adjacent-pair tests (every
/// reader — kernel or vDSO — jumps forward together, since both read
/// the same word), while any per-CPU correction would re-introduce the
/// cross-CPU spread the unified track exists to eliminate. A track that
/// fell behind (mapping drift, KVM TSC resync) is stepped forward,
/// clamped to 10 ms per resync so a poisoned HPET read cannot jump the
/// clock; a lead >250 ms is REPORTED (post shift-32 this should be
/// silent for ~a day — any print is a new divergence worth reading),
/// and a lead >25 s is pulled back as a last resort (a real vCPU TSC
/// smash; the one backward base move — see tsc_anchor_step_back).
pub fn resync_tsc_against_hpet() {
    if TSC_FROM_HPET.load(Ordering::Acquire) == 0 {
        return;
    }
    let Some((cl, base, mult)) = crate::mm::vdso::x86_track::tsc_anchor_peek() else {
        return;
    };
    let raw = crate::mm::vdso::x86_track::read_tsc_ordered();
    let tsc_nom = base
        .wrapping_add(crate::mm::vdso::x86_track::tsc_delta_nominal(
            raw.wrapping_sub(cl),
            mult,
        ));
    let hpet_nom = crate::drivers::timer::hpet::read_time_nominal();
    if hpet_nom > tsc_nom {
        let behind = hpet_nom - tsc_nom;
        // 10 ms nominal-domain clamp (CLOCK_FREQ=10 MHz → 100_000 ticks).
        let step = behind.min(100_000);
        if behind > 100_000 {
            resync_report(b"tsc-clock: fell behind ", behind);
        }
        crate::mm::vdso::x86_track::tsc_anchor_step_forward(step);
    } else if tsc_nom > hpet_nom && tsc_nom - hpet_nom > 2_500_000 {
        // 250 ms..25 s lead: REPORT ONLY. Proof-of-fix marker for the
        // shift-32 correction — at ~3 ppm residual mapping error this
        // needs ~23 h of uptime to fire, so any print here means a NEW
        // divergence (or an HPET read stall), worth reading.
        resync_report(b"tsc-clock: lead ", tsc_nom - hpet_nom);
    } else if tsc_nom - hpet_nom > 250_000_000 {
        // >25 s lead: either a vCPU TSC-offset jump (skew) or a smashed
        // mapping. Pull the anchor base back into sync and print the
        // raw pair for offline skew-vs-smash triage. The TSC_LAST
        // ratchet freezes the syscall track through the pull; the vDSO
        // track steps back (a broken-by-definition state — but better
        // than a clock 25 s off, and loud in the log).
        let lead = tsc_nom - hpet_nom;
        resync_report(b"tsc-clock: rebase, lead ", lead);
        crate::pr_warn!(
            "tsc-clock: rebase raw_tsc={:#x} tsc_nom={:#x} hpet_nom={:#x} cl={:#x} base={:#x} mult={:#x}",
            raw,
            tsc_nom,
            hpet_nom,
            cl,
            base,
            mult
        );
        crate::mm::vdso::x86_track::tsc_anchor_step_back(lead);
    }
}

/// One-shot per-CPU divergence CHECK (AP bringup, IRQs off, before the
/// AP's first tick). With the unified track there is no per-CPU state
/// to calibrate — the page anchor serves every CPU identically, which
/// is only correct when the vCPU TSC offsets are synchronized (KVM
/// invariant TSC; the sc110-114 "skews" were the shift-12 mult bug).
/// Each AP therefore MEASURES its own (HPET − track) divergence and
/// reports anything beyond 1 ms: the tripwire for a host that really
/// arms per-vCPU TSC offsets (hotplug, post-migration), where the
/// global 1 Hz resync would oscillate by the skew. The residual µs
/// noise (HPET MMIO exit latency) is absorbed by the resync clamp.
pub fn calibrate_cpu_tsc_offset(cpu: usize) {
    if TSC_FROM_HPET.load(Ordering::Acquire) == 0 {
        return; // track not HPET-calibrated — nothing to check against
    }
    if cpu == 0 || cpu >= crate::config::MAX_CPUS {
        return;
    }
    let Some((cl, base, mult)) = crate::mm::vdso::x86_track::tsc_anchor_peek() else {
        return;
    };
    // Three bracketed samples, take the extreme |hpet - track|: the
    // HPET MMIO read can stall (sc113 measured ~21 ms of read skew
    // right after AP startup), and a stall INFLATES the apparent
    // divergence — report the minimum-magnitude sample to keep the
    // tripwire honest.
    let mut best = i64::MAX;
    for _ in 0..3 {
        let hpet_nom = crate::drivers::timer::hpet::read_time_nominal();
        let t_after = crate::mm::vdso::x86_track::read_tsc_ordered();
        let track_after = base.wrapping_add(
            crate::mm::vdso::x86_track::tsc_delta_nominal(
                t_after.wrapping_sub(cl),
                mult,
            ),
        );
        let d = hpet_nom as i64 - track_after as i64;
        if d.abs() < best.abs() {
            best = d;
        }
    }
    if best != i64::MAX && best.abs() > 10_000 {
        crate::pr_info!(
            "tsc-clock: cpu {} diverges {:+} us from the HPET track (no per-CPU correction; invariant-TSC assumption check)",
            cpu,
            best / 10
        );
    }
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
/// delay-only counter (see `hpet.rs`). Publishes the page anchor (the
/// unified track) but does NOT arm the 1 Hz HPET resync (no HPET to
/// check against — TSC_FROM_HPET stays 0).
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
    crate::mm::vdso::x86_track::publish_tsc_anchor(mult, now, continuity);
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

/// Arm the LAPIC timer in one-shot mode at the absolute nominal-domain
/// `deadline` (perf lane pd — see apic::timer_arm_oneshot). No-op on the
/// PIT fallback path (channel 0 free-runs at a fixed HZ cadence).
pub fn set_timer(deadline: u64) {
    crate::drivers::intc::apic::timer_arm_oneshot(deadline);
}

/// Set the next timer interrupt: the NEXT NOMINAL GRID TICK, but never
/// later than the earliest outstanding HIGH-RESOLUTION deadline (the
/// riscv64 twin's discipline — precise sleeps must be able to fire
/// BETWEEN grid ticks). Arming is skipped while no precise timebase is
/// active: read_time() is then jiffies-derived and an one-shot armed
/// from the coarse clock would mistime; the periodic LAPIC/PIT tick
/// keeps the grid in that window (grid-only hres granularity, the
/// pre-fix behavior, only on calibration-failed boots).
///
/// NOTE (NOHZ): the adaptive-tick machinery lives in
/// crate::timer::nohz (a function-for-function mirror of the
/// reference's kernel/time/tick-sched.c) and re-arms through
/// tick_nohz_irq_exit at the IRQ exits. This function keeps ONLY the
/// pre-nohz grid-glue discipline for the callers outside the tick
/// re-evaluation path (the PIT-fallback tail and the early
/// hres-sleep re-arm; the latter now routes through tick_nohz as well).
pub fn set_next_trigger() {
    if !timebase_precise() {
        return;
    }
    // Grid-glued (review批次8 discipline): round UP to the next grid
    // multiple so handler latency never accumulates into the cadence.
    let current = read_time();
    let mut deadline = (current / TIME_SLICE_TICKS + 1) * TIME_SLICE_TICKS;
    let hres = crate::timer::hres_next_deadline();
    if hres < deadline {
        // Never arm in the past (a WRMSR count of 0 disarms; clamp to
        // just-after-now so an already-expired hres deadline fires at
        // once and the softirq sweep delivers it).
        deadline = hres.max(current + 1);
    }
    set_timer(deadline);
}

/// Re-arm THIS CPU's timer to min(next grid tick, earliest hres
/// deadline). Called by a task that just registered a high-resolution
/// sleep so the already-armed (later) deadline does not delay its
/// precise wake (perf lane pd: this used to be a no-op on x86_64 —
/// every hres sleep waited for the next 10 ms grid tick).
///
/// NOHZ: the sleeper's CPU re-evaluates through the tick_nohz
/// machinery — tick_nohz_next_event arms at the new wheel deadline
/// (the reference's timer-enqueue path re-programs the local
/// clockevent the same way); the retain case keeps the grid glue.
pub fn rearm_for_hres() {
    // Non-full-dynticks CPUs keep the plain grid-glue arm (the
    // reference's timer-enqueue re-program applies to nohz cpus only).
    let cpu = crate::arch::cpu_id() as usize;
    if !crate::timer::nohz::tick_nohz_full_cpu(cpu) {
        set_next_trigger();
        return;
    }
    crate::timer::nohz::tick_nohz_rearm_local();
}

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
/// multi-CPU safe.  Returns how many jiffies THIS call advanced (RT-2X:
/// 0 marks an hres-only fire — a deadline between grid ticks — which
/// the trap handler turns into the light no-tick-duty exit).
#[inline]
fn increment_jiffies_from_timebase() -> u64 {
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
    nominal.saturating_sub(cur)
}

/// Clock interrupt handler (called from the x86_64 trap entry).
/// Returns the number of jiffies this fire advanced (RT-2X): 0 marks an
/// hres-only fire (a precise deadline between grid ticks) — the trap
/// handler takes the light exit with no scheduler-tick duty for those.
pub fn timer_interrupt_handler() -> u64 {
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
    let advanced = if timebase_precise() {
        // LAPIC-timer phase: idempotent, multi-CPU-safe jiffies.
        increment_jiffies_from_timebase()
    } else {
        // PIT phase: the free-running channel 0 makes every IRQ exactly
        // one tick (single-CPU boot fallback).
        JIFFIES.fetch_add(1, Ordering::AcqRel);
        1
    };

    // RT-2X item 4 (pair-battery queue): expire due high-resolution
    // timers HERE, at the top of the IRQ, before the tick body — the
    // reference's hrtimer_interrupt shape. Precise sleeps then wake
    // ahead of the per-tick accounting / scheduler_tick / scribble /
    // softlockup work (measured 30-70 us of body on the debug kernel)
    // instead of behind it; the irq-exit softirq dispatch below finds
    // the wheel already drained (dedupe fast-out). The raise stays:
    // timers armed DURING the delivery (re-arm actions) still get their
    // dispatch in the same IRQ.
    crate::timer::expire_hres_inline();

    // Tick-cached wall second for advisory atime stamps (oc-rc read
    // path) — idempotent relaxed store, once per tick per CPU.
    crate::drivers::rtc::tick_refresh_cached_secs();

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
    advanced
}
