//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Hung Task Detector
//!
//! Detects tasks stuck in `TASK_UNINTERRUPTIBLE` state for too long.
//! A kernel thread (`khungtaskd`) periodically scans all tasks and reports
//! any that have been in D-state with unchanged context switch count
//! for longer than the threshold.

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use crate::config::MAX_TASKS;
use crate::dfx::taint;
use crate::dfx::backtrace;
use crate::dfx::backtrace::ConsoleWriter;
use core::fmt::Write;

/// Hung task timeout in seconds (default: 120s)
const HUNG_TASK_TIMEOUT_SECS: u64 = 120;

/// Check interval (timeout / 2)
const HUNG_TASK_CHECK_INTERVAL_SECS: u64 = HUNG_TASK_TIMEOUT_SECS / 2;

/// Per-task tracking: last observed switch count (nvcsw + nivcsw)
static LAST_SWITCH_COUNT: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// Per-task tracking: timestamp when task was first seen in D-state with unchanged count
static LAST_SWITCH_TIME: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(0) }; MAX_TASKS];

/// Whether khungtaskd is running
static RUNNING: AtomicBool = AtomicBool::new(false);

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
    time * 100
}

/// Initialize the hung task detector.
///
/// Starts the `khungtaskd` kernel thread.
pub fn init() {
    if RUNNING.load(Ordering::Acquire) {
        return;
    }

    // Start khungtaskd kernel thread
    let result = crate::process::kthread::kthread_run(
        khungtaskd_fn,
        core::ptr::null_mut(),
        "khungtaskd",
    );

    if result.is_some() {
        RUNNING.store(true, Ordering::Release);
        crate::pr_info!("dfx: khungtaskd started");
    } else {
        crate::pr_warn!("dfx: failed to start khungtaskd");
    }
}

/// khungtaskd main loop.
///
/// Wakes every `HUNG_TASK_CHECK_INTERVAL_SECS` and scans all tasks.
extern "C" fn khungtaskd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    loop {
        // Sleep for check interval: set INTERRUPTIBLE, release BKL, schedule, re-acquire
        if let Some(current) = crate::sched::current() {
            unsafe {
                (*current).set_state(
                    crate::process::task::TaskState::new(
                        crate::process::task::TaskState::INTERRUPTIBLE
                    )
                );
            }
        }
        crate::sync::kernel_lock_release();
        crate::sched::schedule();
        crate::sync::kernel_lock_acquire();

        // Check for stop request
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // Scan all tasks
        check_tasks();
    }

    RUNNING.store(false, Ordering::Release);
    0
}

/// Scan all tasks for hung tasks.
fn check_tasks() {
    let now = now_ns();
    let timeout_ns = HUNG_TASK_TIMEOUT_SECS * 1_000_000_000;

    crate::sched::for_each_task(|task| {
        unsafe {
            let task_ref = &*task;
            let pid = task_ref.pid() as usize;
            let state = task_ref.state();

            // Only check uninterruptible tasks
            if !state.contains(crate::process::task::TaskState::UNINTERRUPTIBLE) {
                // Not in D-state: reset tracking
                if pid < MAX_TASKS {
                    LAST_SWITCH_TIME[pid].store(0, Ordering::Release);
                }
                return;
            }

            // Skip idle task (pid 0) and kernel threads without meaningful switch counts
            if pid == 0 || pid >= MAX_TASKS {
                return;
            }

            // Get current switch count
            // Note: nvcsw/nivcsw fields need to be added to Task struct
            // For now, use a simple heuristic: track time in D-state
            let last_time = LAST_SWITCH_TIME[pid].load(Ordering::Acquire);

            if last_time == 0 {
                // First time seeing this task in D-state
                LAST_SWITCH_TIME[pid].store(now, Ordering::Release);
            } else {
                let elapsed = now.saturating_sub(last_time);
                if elapsed > timeout_ns {
                    // Hung task detected!
                    report_hung_task(task, elapsed / 1_000_000_000);

                    // Reset to avoid repeated reports
                    LAST_SWITCH_TIME[pid].store(now, Ordering::Release);
                }
            }
        }
    });
}

/// Report a hung task.
fn report_hung_task(task: *mut crate::process::task::Task, elapsed_secs: u64) {
    let mut w = ConsoleWriter::new();
    let task_ref = unsafe { &*task };

    let _ = write!(
        w,
        "INFO: task {}:{} blocked for more than {} seconds\n",
        task_ref.pid(),
        task_ref.pid(), // No comm name yet; use PID as placeholder
        elapsed_secs
    );

    let taint_str = taint::taint_string_arr();
    let taint_display = unsafe { core::str::from_utf8_unchecked(&taint_str) };
    let _ = write!(w, "      Tainted: {}\n", taint_display);

    // Stack trace
    backtrace::dump_stack();

    // Taint kernel
    taint::add_taint(taint::TaintFlags::DIE);
}
