//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! CLOCK_MONOTONIC jump tripwire (dfx=monowatch).
//!
//! The faultlat side-finding — an in-guest CLOCK_MONOTONIC that ran ~700 s
//! ahead of real elapsed during one probe run — made every duration,
//! timeout and animation-frame computation untrustworthy. Whether that
//! was a real clock jump or unaccounted blocking time could not be
//! adjudicated afterwards because nothing sampled the timekeeper. This
//! watch makes the timebase self-monitoring on every hunt boot:
//!
//! Every OBSERVE_JIFFIES (~1 s), the timer softirq samples
//! `read_time()` (the single source CLOCK_MONOTONIC, jiffies and all
//! hres deadlines derive from) and checks against the last accepted
//! sample:
//!
//! - backward:  new < last — the clock ran time BACKWARD;
//! - spike:     delta > MAX_STEP_S — the clock jumped forward far
//!              faster than any real interval between two timer ticks
//!              (a hypervisor pause/migration resync, a scribbled
//!              timebase offset, or a clocksource switch gone wrong).
//!
//! Reports name the ACTIVE clocksource (HPET / TSC / jiffies-PIT) so a
//! jump is attributed on the spot, and carry both sample values in the
//! nominal 10 MHz tick domain (×100 = ns). Rate-limited to 8 lines.
//!
//! Multi-CPU: samples from all harts interleave; the CAS accepts only
//! forward-in-bounds advances, so a racing sample from another CPU is
//! skipped, not misreported. On the alarm path the loser does not
//! update `last` (the next tick re-reports against the same baseline
//! until the clock passes it, capped by the rate limit).

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU64, Ordering};

/// Observation period in jiffies (100 = 1 s at HZ=100).
const OBSERVE_JIFFIES: u64 = 100;
/// Largest legitimate advance between two observations (host pause
/// with the VM stopped freezes the virtual clock, so real deltas never
/// exceed the observation interval by much; 5 s absorbs deep
/// interrupt-latency stalls under TCG while still tripping on the
/// ~700 s class of jump by three orders of magnitude).
const MAX_STEP_TICKS: u64 = 5 * crate::config::TIMER_CLOCK_FREQ_HZ;
/// Report budget per boot.
const MAX_REPORTS: u64 = 8;

/// Last accepted read_time() sample (nominal ticks); 0 = not seeded.
static LAST: AtomicU64 = AtomicU64::new(0);
/// Jiffies of the last observation (dedupe: one sample per period).
static LAST_J: AtomicU64 = AtomicU64::new(0);
/// Reports printed so far.
static REPORTS: AtomicU64 = AtomicU64::new(0);

/// Name of the active timebase (x86_64: HPET / TSC / PIT-jiffies;
/// riscv64: SBI time CSR).
fn clocksource_name() -> &'static str {
    #[cfg(feature = "x86_64")]
    {
        if crate::drivers::timer::hpet::ready() {
            "hpet"
        } else if crate::drivers::timer::tsc_active() {
            "tsc"
        } else {
            "pit-jiffies"
        }
    }
    #[cfg(feature = "riscv64")]
    {
        "sbi-time"
    }
}

/// One observation; called from the timer softirq (any CPU).
pub fn tick(current_jiffies: u64) {
    let last_j = LAST_J.load(Ordering::Acquire);
    // Dedupe inside the observation window (softirq runs per jiffy per
    // CPU; only the first CPU in each window samples).
    if current_jiffies.saturating_sub(last_j) < OBSERVE_JIFFIES && last_j != 0 {
        return;
    }
    let now = crate::drivers::timer::read_time();
    let last = LAST.load(Ordering::Acquire);

    if last == 0 {
        // Seed (the CAS below guards two CPUs racing the seed).
        if LAST
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            LAST_J.store(current_jiffies, Ordering::Release);
        }
        return;
    }

    if now < last {
        // Backward: report but do NOT pull `last` down — the clock may
        // recover; reporting against the old baseline keeps the record.
        report("backward", last, now, current_jiffies);
        return;
    }
    let delta = now - last;
    if delta > MAX_STEP_TICKS {
        report("spike", last, now, current_jiffies);
        // Accept the new value so one jump is one report, not MAX_REPORTS.
        LAST.store(now, Ordering::Release);
        LAST_J.store(current_jiffies, Ordering::Release);
        return;
    }
    // Forward-in-bounds: try to advance the baseline; a racing CPU that
    // already moved it further wins and we simply skip.
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        LAST_J.store(current_jiffies, Ordering::Release);
    }
}

fn report(kind: &str, last: u64, now: u64, jiffies: u64) {
    let n = REPORTS.fetch_add(1, Ordering::Relaxed);
    if n >= MAX_REPORTS {
        return;
    }
    let delta_ms = (now.saturating_sub(last)) / (crate::config::TIMER_CLOCK_FREQ_HZ / 1000);
    crate::pr_err!(
        "MONOWATCH-JUMP: {} clocksource={} last={:#x} now={:#x} delta={}ms jiffies={} ({}/{})",
        kind,
        clocksource_name(),
        last,
        now,
        delta_ms,
        jiffies,
        n + 1,
        MAX_REPORTS
    );
}
