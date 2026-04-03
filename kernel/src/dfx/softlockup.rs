//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Softlockup Detector
//!
//! Detects CPUs that are stuck in a non-sleeping loop for too long.
//! Each CPU updates a per-CPU timestamp on every `scheduler_tick()`.
//! A periodic check (driven by timer softirq) compares timestamps
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

/// Get nanosecond timestamp from RISC-V `rdtime`.
fn now_ns() -> u64 {
    let time: u64;
    unsafe {
        core::arch::asm!(
            "rdtime {}",
            out(reg) time,
            options(nomem, nostack)
        );
    }
    // Assume 10MHz timebase → 100ns per tick
    time * 100
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

/// Check all CPUs for softlockup.
///
/// Called periodically from timer softirq or a watchdog timer.
/// Reports any CPU that hasn't been touched for longer than the threshold.
pub fn check() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }

    let now = now_ns();
    let threshold_ns = SOFTLOCKUP_THRESHOLD_SECS * 1_000_000_000;

    for cpu in 0..MAX_CPUS {
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
                "BUG: soft lockup - CPU#{} stuck for {}s!\n",
                cpu, elapsed_secs
            );

            // Print the stuck CPU's current task (not the caller's).
            // `sched::current()` reads tp of the CALLING CPU, so we use
            // the per-CPU state array to get the correct task pointer.
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
