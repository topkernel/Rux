//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! I/O Completion — lightweight completion signal for async block I/O.
//!
//! Provides a wait/wakeup primitive for I/O completion notification.
//! extended with an I/O status code. Used to decouple I/O submission
//! from completion: the submitter creates an IoCompletion, passes it to
//! an async I/O function, then calls `wait()` later (or never, if polling).

use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use crate::process::wait::WaitQueueHead;

/// I/O completion signal.
///
/// # Usage
/// ```ignore
/// let comp = IoCompletion::new();
/// // submit async I/O with &comp ...
/// let status = comp.wait();  // blocks until I/O finishes
/// ```
///
/// Thread-safe: `complete()` is called from interrupt context,
/// `wait()` from any kernel task.
pub struct IoCompletion {
    /// True when the I/O has finished.
    done: AtomicBool,
    /// 0 = success, negative = errno (e.g. -EIO).
    status: AtomicI32,
    /// Tasks sleeping for completion.
    wait_queue: WaitQueueHead,
}

impl IoCompletion {
    /// Create a new, not-done completion.
    pub const fn new() -> Self {
        Self {
            done: AtomicBool::new(false),
            status: AtomicI32::new(0),
            wait_queue: WaitQueueHead::new(),
        }
    }

    /// Mark completion as done with the given status.
    ///
    /// Wakes all waiters. Safe to call from interrupt context
    /// (no allocation, no BKL).
    pub fn complete(&self, status: i32) {
        self.status.store(status, Ordering::Release);
        self.done.store(true, Ordering::Release);
        self.wait_queue.wake_up_all();
    }

    /// Block until completion is signaled. Returns the status code.
    ///
    /// Follows the standard BKL discipline: release BKL, schedule,
    /// re-acquire BKL.
    pub fn wait(&self) -> i32 {
        loop {
            if self.done.load(Ordering::Acquire) {
                return self.status.load(Ordering::Acquire);
            }

            let current = match crate::sched::current() {
                Some(task) => task,
                None => {
                    core::hint::spin_loop();
                    continue;
                }
            };

            let entry = crate::process::wait::WaitQueueEntry::new(current, false);
            self.wait_queue.add(entry);

            crate::sync::kernel_lock_release();
            crate::sched::schedule();
            crate::sync::kernel_lock_acquire();

            self.wait_queue.remove(current);
        }
    }

    /// Non-blocking check: returns Some(status) if done, None otherwise.
    pub fn try_wait(&self) -> Option<i32> {
        if self.done.load(Ordering::Acquire) {
            Some(self.status.load(Ordering::Acquire))
        } else {
            None
        }
    }

    /// Check if completion is done without returning status.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Reset to initial (not-done) state for reuse.
    pub fn reset(&self) {
        self.done.store(false, Ordering::Release);
        self.status.store(0, Ordering::Release);
    }
}

/// Wait for all completions in a slice. Returns 0 if all succeeded,
/// or the first error encountered.
pub fn wait_for_all(completions: &[&IoCompletion]) -> i32 {
    let mut first_error = 0;
    for comp in completions {
        let status = comp.wait();
        if status < 0 && first_error == 0 {
            first_error = status;
        }
    }
    first_error
}
