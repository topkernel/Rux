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
use crate::drivers::timer::msecs_to_jiffies;
use crate::process::wait::WaitQueueHead;

/// Status returned by [`IoCompletion::wait`] when the bounded wait deadline
/// expired without the device signaling completion (-ETIMEDOUT).
///
/// Distinct from -EIO: a real device error means the chain COMPLETED (its
/// DMA writes are done); a timeout means the chain may STILL be in flight
/// on the device. Callers must therefore (a) retire the device-side pending
/// entry so its late completion cannot fire into memory they are about to
/// stop owning (bio::bread_wait does this via
/// virtio::blk_retire_pending_async), and (b) never free the request's DMA
/// target buffer — leak it instead.
pub const WAIT_TIMED_OUT: i32 = -110; // -ETIMEDOUT

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

    /// Block until completion is signaled. Returns the status code, or
    /// [`WAIT_TIMED_OUT`] if the bounded deadline expires first.
    ///
    /// Follows the standard BKL discipline: release BKL, schedule,
    /// re-acquire BKL.
    pub fn wait(&self) -> i32 {
        let mut deadline_timer = 0u64; // 0 = not armed (or arm failed)
        let mut deadline_armed = false;
        let mut poll_fallback = false; // timer budget exhausted: 1-jiffy re-arms
        let mut deadline = 0u64;
        let result;
        loop {
            if self.done.load(Ordering::Acquire) {
                result = self.status.load(Ordering::Acquire);
                break;
            }

            let current = match crate::sched::current() {
                Some(task) => task,
                None => {
                    core::hint::spin_loop();
                    continue;
                }
            };

            // WEDGE fix (LTP inode02/ftest family): an unbounded
            // kernel-side wait is unkillable — signals are delivered only
            // on the return to userspace, so a lost completion wedged the
            // whole sweep. Bound the wait (10s of jiffies); a lost
            // completion then reports ETIMEDOUT and the syscall unwinds.
            //
            // GSD fix (10s-deadline UAF): returning with the I/O still
            // queued left the virtio pending tables holding a raw pointer
            // to this (usually stack-allocated) completion; later
            // completion walkers called complete() through freed stack
            // memory (wild wake_up_all -> KERNPANIC under gnome-session).
            // Before unwinding (see the deadline check below): (1) kick +
            // drain one last time — recovers lost-kick stalls; (2) abandon
            // our pending-table entries so nothing dereferences this
            // memory after we return. Callers must likewise NOT free the
            // I/O's DMA buffers on ETIMEDOUT.
            if !deadline_armed && !poll_fallback {
                deadline = crate::drivers::timer::get_jiffies()
                    .saturating_add(msecs_to_jiffies(10_000));
                // Arm a wakeup AT THE DEADLINE itself. The deadline check
                // below only runs after schedule() returns, so without this
                // timer a completion that never arrives (and no other wake)
                // would sleep forever — unkillable, and with no timeout
                // print: exactly the silent-strand shape behind the freed
                // kernel-stack completion fires (the r2/r4 rounds).
                let pid = crate::sched::get_current_pid();
                deadline_timer = crate::timer::add_timer_wakeup(deadline, pid);
                if deadline_timer != 0 {
                    deadline_armed = true;
                } else {
                    // Timer table full: degrade to per-iteration 1-jiffy
                    // re-arms (wait_buffer_io_done discipline) so the
                    // deadline check still runs.
                    poll_fallback = true;
                }
            }

            // Use prepare_to_wait to atomically set INTERRUPTIBLE and
            // add to queue, preventing lost-wakeup race.
            self.wait_queue.prepare_to_wait(current, false, true);

            // Recheck after setting state (waker may have fired)
            if self.done.load(Ordering::Acquire) {
                self.wait_queue.finish_wait(current);
                // R36-B2 (R8-5 NEW-C2 discipline, missed here): a complete()
                // that raced between prepare_to_wait and this recheck woke
                // and enqueued us while we never slept — take ourselves
                // back off the GRQ (no-op via on_rq guards otherwise) or
                // nr_running stays inflated until our next context switch.
                crate::sched::dequeue_task(&*current);
                result = self.status.load(Ordering::Acquire);
                break;
            }

            // Deadline check AFTER prepare_to_wait: we are about to sleep,
            // so this is the last point the deadline can abort without a
            // wake. A concurrent complete() between the recheck above and
            // here is caught by the loop-top check on the next iteration
            // only if we get woken — the deadline timer covers that too.
            if (deadline_armed || poll_fallback)
                && crate::drivers::timer::get_jiffies() >= deadline
            {
                self.wait_queue.finish_wait(current);
                // Same R36-B2 compensation as the recheck exit: a racing
                // fire may have enqueued us between prepare and finish.
                crate::sched::dequeue_task(&*current);
                // Final chance (GSD fix): kick the queue and drain
                // completions once — recovers lost-kick stalls.
                crate::drivers::virtio::pci_blk_kick();
                crate::drivers::virtio::pci_process_async_completions();
                if self.done.load(Ordering::Acquire) {
                    crate::pr_err!("io_completion: recovered by final kick/drain");
                    result = self.status.load(Ordering::Acquire);
                    break;
                }
                crate::pr_err!(
                    "io_completion: lost completion — reporting ETIMEDOUT (wedge fix)"
                );
                // Abandon our pending-table entries so nothing
                // dereferences this memory after we return (GSD fix).
                crate::drivers::virtio::abandon_pending_completion(
                    self as *const _ as *mut _,
                );
                result = WAIT_TIMED_OUT;
                break;
            }

            if poll_fallback {
                // Re-arm a 1-jiffy wake so the deadline check above runs
                // every jiffy even without a completion wake.
                let pid = crate::sched::get_current_pid();
                let dl = crate::drivers::timer::get_jiffies().saturating_add(1);
                let id = crate::timer::add_timer_wakeup(dl, pid);
                // R54: schedule() now restores the caller's SIE state;
                // wait-path callers re-arm explicitly (semaphore.rs
                // discipline) so ticks/IPIs reach this CPU across the wait
                // loop.
                crate::arch::riscv64::cpu::restore_irq(true);
                crate::sched::schedule();
                if id != 0 {
                    crate::timer::del_timer(id);
                }
            } else {
                // R54: schedule() now restores the caller's SIE state;
                // wait-path callers re-arm explicitly (semaphore.rs
                // discipline) so ticks/IPIs reach this CPU across the wait
                // loop.
                crate::arch::riscv64::cpu::restore_irq(true);
                crate::sched::schedule();
            }

            self.wait_queue.finish_wait(current);
        }
        if deadline_armed && deadline_timer != 0 {
            crate::timer::del_timer(deadline_timer);
        }
        result
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
