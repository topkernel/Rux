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

use core::sync::atomic::{AtomicBool, Ordering};
use crate::dfx::taint;
use crate::dfx::backtrace;
use crate::dfx::backtrace::ConsoleWriter;
use core::fmt::Write;

/// Hung task timeout in seconds (default: 120s)
const HUNG_TASK_TIMEOUT_SECS: u64 = 120;

/// Check interval (timeout / 2)
const HUNG_TASK_CHECK_INTERVAL_SECS: u64 = HUNG_TASK_TIMEOUT_SECS / 2;

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
    // timebase ticks → ns (×100 for the 10 MHz CLINT; the old ×10
    // multiplier read every window 10x too small — review批次8).
    time.saturating_mul(1_000_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ as u64
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
/// Wakes every `HUNG_TASK_CHECK_INTERVAL_SECS` via a one-shot kernel timer
/// and scans all tasks (pid-hash traversal — never the pid-slot array).
extern "C" fn khungtaskd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::TaskState;

    // Check interval in jiffies (timeout/2, at least one tick).
    let interval_jiffies = (HUNG_TASK_CHECK_INTERVAL_SECS
        * crate::config::KERNEL_HZ as u64)
        .max(1);

    loop {
        // Check for stop request
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // --- Periodic sleep with a real waker (review批次8: the old loop
        // set INTERRUPTIBLE and scheduled with NO waker armed — khungtaskd
        // slept until some unrelated wake happened to land on it, so hung
        // tasks were detected minutes late or never). Mirror the
        // nanosleep_impl state-first discipline: arm a one-shot timer for
        // our pid, mark INTERRUPTIBLE, re-check, then sleep.
        let current = match crate::sched::current() {
            Some(t) => t as *mut crate::process::task::Task,
            None => break,
        };
        // SAFETY: current is this kthread's own task pointer.
        let my_pid = unsafe { (*current).pid() };
        let target = crate::drivers::timer::get_jiffies() + interval_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);

        if timer_id != 0 {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            // Re-check AFTER marking sleeping: the one-shot timer may have
            // fired between arming and set_state — either its wake captured
            // the INTERRUPTIBLE state, or the deadline is already visible.
            if crate::drivers::timer::get_jiffies() >= target {
                // SAFETY: current is the running task's pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while we
                    // are still executing — take ourselves back off.
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                // Enable interrupts so the tick can reach us, then sleep.
                crate::arch::riscv64::cpu::restore_irq(true);
                crate::sched::schedule();
                // Spurious wake before the deadline: drop the timer and
                // loop (re-armed next iteration).
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full — stay runnable, yield one scheduling round
            // and retry the arm.
            crate::sched::schedule();
        }

        // Scan all tasks
        check_tasks();
    }

    RUNNING.store(false, Ordering::Release);
    0
}

/// Scan all tasks for hung tasks (pid-hash traversal; bookkeeping lives on
/// each Task — the old pid-indexed side table skipped every task whose pid
/// was >= MAX_TASKS, which on this system means essentially all of them,
/// review批次8).
fn check_tasks() {
    let now = now_ns();
    let timeout_ns = HUNG_TASK_TIMEOUT_SECS * 1_000_000_000;

    crate::sched::for_each_task(|task| {
        // SAFETY: callback runs under the pid-hash bucket lock; reads and
        // atomic stores on the task's own hung_task_since field only.
        unsafe {
            let task_ref = &*task;
            let state = task_ref.state();
            let pid = task_ref.pid();

            // Only check uninterruptible tasks
            if !state.contains(crate::process::task::TaskState::UNINTERRUPTIBLE) {
                // Not in D-state: reset tracking
                task_ref.hung_task_since.store(0, Ordering::Release);
                return;
            }

            // Skip the idle task (pid 0).
            if pid == 0 {
                return;
            }

            let last_time = task_ref.hung_task_since.load(Ordering::Acquire);

            if last_time == 0 {
                // First time seeing this task in D-state
                task_ref.hung_task_since.store(now, Ordering::Release);
            } else {
                let elapsed = now.saturating_sub(last_time);
                if elapsed > timeout_ns {
                    // Hung task detected!
                    report_hung_task(task, elapsed / 1_000_000_000);

                    // Reset to avoid repeated reports
                    task_ref.hung_task_since.store(now, Ordering::Release);
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
