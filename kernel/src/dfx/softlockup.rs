//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Softlockup Detector
//!
//! Detects CPUs that are stuck in a non-sleeping loop for too long.
//! Each CPU updates a per-CPU timestamp on every `scheduler_tick()`.
//! A periodic check (driven by timer interrupt) compares timestamps
//! against current time and reports any CPU that hasn't scheduled
//! for longer than the threshold.

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use crate::config::MAX_CPUS;
use crate::dfx::taint;
use crate::dfx::backtrace;
use crate::dfx::backtrace::ConsoleWriter;
use core::fmt::Write;

/// Softlockup threshold in seconds (default: 10s)
const SOFTLOCKUP_THRESHOLD_SECS: u64 = 10;

/// Per-CPU timestamp of last scheduler tick (nanoseconds)
static TOUCH_TS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Per-CPU flag indicating softlockup detected (to avoid repeated reports)
static REPORTED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Whether the softlockup detector is enabled
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Get nanosecond timestamp from the arch wall-clock counter.
fn now_ns() -> u64 {
    let time: u64 = crate::arch::cpu::read_time();
    // timebase ticks → ns: for the 10 MHz CLINT that is exactly 100 ns per
    // tick. The old `time * (FREQ / 1_000_000)` computed ×10 instead of
    // ×100 — every elapsed window read 10x too small, delaying a 10s
    // lockup report to ~100s real time (review批次8 换算修复).
    time.saturating_mul(1_000_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ as u64
}

/// Initialize the softlockup detector.
///
/// Called during `dfx::init()`. Sets all timestamps to current time.
pub fn init() {
    let now = now_ns();
    for i in 0..MAX_CPUS {
        TOUCH_TS[i].store(now, Ordering::Release);
        REPORTED[i].store(false, Ordering::Release);
    }
    ENABLED.store(true, Ordering::Release);
}

/// Touch the softlockup timestamp for the given CPU.
///
/// Called from `scheduler_tick()` to indicate this CPU is still scheduling.
pub fn touch(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    TOUCH_TS[cpu].store(now_ns(), Ordering::Release);
    REPORTED[cpu].store(false, Ordering::Release);
}

/// Last scheduler-tick timestamp (ns) recorded for the given CPU.
///
/// 0 when the CPU has never taken a tick (early boot) — callers must treat
/// that as "unknown", never as "stale". Used by the scheduler's orphan
/// harvester to distinguish a slot owned by a live, ticking CPU from one
/// left behind by a hart that stopped taking interrupts entirely.
pub fn last_touch_ns(cpu: usize) -> u64 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    TOUCH_TS[cpu].load(Ordering::Acquire)
}

/// R57 (vCPU-liveness under MTTCG starvation): last observed timer-IRQ
/// count per CPU, and the CLINT ns timestamp at which it last CHANGED.
/// A TCG-starved vCPU still services its timer ISR occasionally (3-10
/// IRQs/s under host contention vs 100 nominal — observed on CPU3 of the
/// n1-organic run: timer_irq 32604→33074 across a 1600s window while its
/// scheduler tick read "stale" the whole time). The scheduler's
/// slot-capture steal used tick-staleness alone as its "dead picker"
/// test and stole a mid-exit task from that alive-but-slow hart
/// (double-run, epc on the task's own kernel stack, illegal-instruction
/// panic at t=1796s). A hart whose timer count still moves is alive
/// no matter how stale its touch_ts reads.
static IRQ_COUNT_SEEN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static IRQ_COUNT_SEEN_AT_NS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Record each CPU's current timer-IRQ count; stamp the wall-clock ns of
/// any change. Called from check(), which runs on every timer interrupt —
/// healthy CPUs keep every CPU's "changed at" fresh, so a hart whose count
/// is frozen accumulates real staleness visible to any observer.
fn note_irq_counts(now: u64) {
    for cpu in 0..MAX_CPUS {
        let cnt = crate::fs::procfs::interrupts::timer_count(cpu);
        if cnt == 0 {
            // Hart has never taken a timer IRQ (early boot / not started):
            // leave seen-at at 0 — cpu_irq_frozen_ms reports 0 ("alive")
            // for that case so nothing is ever stolen from a never-started
            // hart either.
            continue;
        }
        let prev = IRQ_COUNT_SEEN[cpu].load(Ordering::Relaxed);
        if cnt != prev {
            IRQ_COUNT_SEEN[cpu].store(cnt, Ordering::Relaxed);
            IRQ_COUNT_SEEN_AT_NS[cpu].store(now, Ordering::Release);
        }
    }
}

/// Milliseconds since the named CPU's timer-IRQ count last changed.
/// 0 means "alive or unknown" (the count is currently moving, or the hart
/// has never ticked) — the SAFE answer for any "may I steal from it"
/// test: a wrong 0 merely refuses a heal, a wrong u64::MAX destroys a
/// live task.
pub fn cpu_irq_frozen_ms(cpu: usize) -> u64 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    let seen_at = IRQ_COUNT_SEEN_AT_NS[cpu].load(Ordering::Acquire);
    if seen_at == 0 {
        return 0;
    }
    let now = now_ns();
    if now <= seen_at {
        return 0;
    }
    (now - seen_at) / 1_000_000
}

/// Check all CPUs for softlockup.
///
/// Called periodically from timer interrupt handler.
/// Reports any CPU that hasn't been touched for longer than the threshold.
pub fn check() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }

    let now = now_ns();
    let threshold_ns = SOFTLOCKUP_THRESHOLD_SECS * 1_000_000_000;

    // R57: refresh the timer-count liveness table before any consumer
    // (the scheduler's slot-capture steal) reads it.
    note_irq_counts(now);

    for cpu in 0..MAX_CPUS {
        // Skip CPUs that haven't received any timer interrupt yet.
        // During boot, secondary CPUs may not have started their timers,
        // so TOUCH_TS remains at the init() value — not a real lockup.
        let timer_cnt = crate::fs::procfs::interrupts::timer_count(cpu);
        if timer_cnt == 0 {
            continue;
        }

        // Skip CPUs that are idle with no runnable tasks on the system.
        // After init exits, all CPUs enter idle loop with no work to
        // do — timer ticks stop and timestamps go stale, but this is
        // not a lockup.
        let nr_running = crate::sched::sched::GlobalRunQueue::grq_nr_running();
        if nr_running == 0 {
            continue;
        }

        // Skip CPUs running the idle task (PID 0).  The idle task
        // intentionally never schedules — it only runs when there is
        // nothing else to do.  A stale touch_ts on an idle CPU is
        // expected behaviour, not a lockup.
        {
            let current = unsafe { crate::sched::sched::cpu_state(cpu).current };
            if current.is_null() || unsafe { (*current).pid() } == 0 {
                continue;
            }
        }

        let touch_ts = TOUCH_TS[cpu].load(Ordering::Acquire);

        // Skip CPUs that haven't been initialized (touch_ts == 0)
        if touch_ts == 0 {
            continue;
        }

        let elapsed = now.saturating_sub(touch_ts);

        if elapsed > threshold_ns {
            // Only report once per occurrence
            if REPORTED[cpu].compare_exchange(
                false, true,
                Ordering::AcqRel, Ordering::Acquire
            ).is_err() {
                continue;
            }

            let elapsed_secs = elapsed / 1_000_000_000;
            let mut w = ConsoleWriter::new();

            let _ = write!(
                w,
                "BUG: soft lockup - CPU#{} stuck for {}s! timer_irq={}\n",
                cpu, elapsed_secs, timer_cnt
            );

            // Print the stuck CPU's current task
            let stuck_task = unsafe {
                crate::sched::sched::cpu_state(cpu).current
            };
            if !stuck_task.is_null() {
                let pid = unsafe { (*stuck_task).pid() };
                let _ = write!(w, "  CPU: {} PID: {}\n", cpu, pid);
            }

            // Stack trace
            backtrace::dump_stack();

            // Taint kernel
            taint::add_taint(taint::TaintFlags::SOFTLOCKUP);
        }
    }
}
