//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Per-CPU ksoftirqd kernel thread
//!
//! When `__do_softirq()` exceeds its iteration budget, it wakes ksoftirqd
//! to drain remaining softirqs at normal scheduling priority.

use core::sync::atomic::{AtomicBool, Ordering};
use crate::config::MAX_CPUS;

// ============================================================================
// Per-CPU ksoftirqd state
// ============================================================================

/// Per-CPU ksoftirqd task pointers. Set once during init; read-only thereafter.
static mut KSOFTIRQD_TASK: [*mut crate::process::task::Task; MAX_CPUS] =
    [core::ptr::null_mut(); MAX_CPUS];

/// Per-CPU wake flag. Set by `wakeup_ksoftirqd()`, checked by ksoftirqd loop.
static KSOFTIRQD_WAKE: [AtomicBool; MAX_CPUS] = [
    AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false), AtomicBool::new(false),
];

// ============================================================================
// ksoftirqd thread function
// ============================================================================

/// Main loop for ksoftirqd (one per CPU).
///
/// Runs `__do_softirq()` in a loop until no more pending softirqs,
/// then sleeps. Woken by `wakeup_ksoftirqd()` or when a softirq is
/// raised from process context.
extern "C" fn ksoftirqd_fn(arg: *mut core::ffi::c_void) -> i32 {
    let cpu = arg as usize;

    crate::pr_info!("ksoftirqd/{} started", cpu);

    loop {
        // Check if we should stop
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // Clear wake flag before draining
        KSOFTIRQD_WAKE[cpu].store(false, Ordering::Release);

        // Drain softirqs
        while crate::interrupt::softirq::has_pending_softirqs() {
            crate::interrupt::softirq::__do_softirq();
        }

        // Nothing more to do — sleep.
        // Release BKL, set INTERRUPTIBLE, schedule, re-acquire BKL on wake.
        if let Some(current) = crate::sched::current() {
            unsafe {
                (*current).set_state(
                    crate::process::task::TaskState::new(
                        crate::process::task::TaskState::INTERRUPTIBLE
                    )
                );
            }
        }

        crate::sched::schedule();

        // Woken up — loop back to check pending softirqs
    }

    0
}

// ============================================================================
// Wakeup (called from softirq.rs)
// ============================================================================

/// Wake the ksoftirqd thread for the current CPU.
///
/// Called by `raise_softirq()` (from process context) or `invoke_softirq()`
/// (on overflow). Idempotent — multiple calls before ksoftirqd runs
/// are collapsed by the AtomicBool flag.
pub fn wakeup_ksoftirqd() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Avoid redundant wakeups
    if KSOFTIRQD_WAKE[cpu].swap(true, Ordering::AcqRel) {
        return; // already flagged
    }

    unsafe {
        let task_ptr = KSOFTIRQD_TASK[cpu];
        if !task_ptr.is_null() {
            crate::process::task::Task::wake_up(task_ptr);
        }
    }
}

// ============================================================================
// Initialization
// ============================================================================

/// Create ksoftirqd kernel threads for all online CPUs.
///
/// Must be called after `sched::init()` since it uses `kthread_run()`.
/// Should be called before interrupts are enabled.
pub fn init() {
    for cpu in 0..MAX_CPUS {
        let name = match cpu {
            0 => "ksoftirqd/0",
            1 => "ksoftirqd/1",
            2 => "ksoftirqd/2",
            3 => "ksoftirqd/3",
            _ => "ksoftirqd/?",
        };

        let task = crate::process::kthread::kthread_run(
            ksoftirqd_fn,
            cpu as *mut core::ffi::c_void,
            name,
        );

        if let Some(t) = task {
            let t_ptr = t as *mut _;
            // kthread_run enqueued on boot CPU's RQ; migrate to target CPU.
            // Safe: timer interrupts not yet enabled, no concurrency.
            crate::sched::dequeue_task(t);
            crate::process::kthread::kthread_bind(t, cpu);
            crate::sched::enqueue_task(t);
            unsafe {
                KSOFTIRQD_TASK[cpu] = t_ptr;
            }
        } else {
            crate::pr_err!("ksoftirqd: failed to create thread for cpu {}", cpu);
        }
    }
}
