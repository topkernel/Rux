//! Idle Scheduling Class Implementation
//!
//! The idle scheduling class manages per-CPU idle tasks. These are the lowest
//! priority tasks that run when no other tasks are runnable.
//!
//! IMPORTANT: This is for per-CPU idle tasks only, NOT for SCHED_IDLE tasks.
//! SCHED_IDLE tasks are handled by the fair class with very low weight (3).
//!
//! Key characteristics:
//! - Idle tasks are never enqueued (no enqueue_task)
//! - Idle tasks are never dequeued (dequeue prints warning)
//! - pick_task simply returns the per-CPU idle task
//! - Any waking task preempts idle immediately

use core::sync::atomic::Ordering;
use crate::process::task::Task;
use super::class::{SchedClass, RunQueueRef};

/// Idle scheduling class
pub struct IdleSchedClass;

impl IdleSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for IdleSchedClass {
    fn name(&self) -> &'static str {
        "idle"
    }

    /// Idle tasks are never enqueued - they are always available
    ///
    /// The idle task is always present at rq->idle and is picked
    /// when no other tasks are runnable.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
        // Idle tasks are never enqueued
        // This should never be called
    }

    /// It is not legal to sleep in the idle task
    ///
    /// Sleeping in the idle task is a bug.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        // This should never happen - sleeping in idle task is illegal
        // "bad: scheduling from the idle thread!"
        false
    }

    /// Idle tasks don't yield
    fn yield_task(&self, _rq: RunQueueRef) {
        // Idle tasks don't yield - they're already lowest priority
    }

    /// Idle tasks are unconditionally preempted
    ///
    /// Any waking task should preempt the idle task.
    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
        // Any waking task preempts idle - request reschedule
        super::sched::resched_curr();
    }

    /// Pick the idle task
    ///
    /// This is called when no higher-priority class has a runnable task.
    fn pick_next_task(&self, rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        if rq.is_null() {
            return core::ptr::null_mut();
        }

        unsafe {
            let rq_ref = &mut *rq;

            // Return the per-CPU idle task
            let idle = rq_ref.idle;

            if idle.is_null() {
                return core::ptr::null_mut();
            }

            // Set next task (update exec_start, etc.)
            self.set_next_task(rq, idle, true);

            idle
        }
    }

    /// Put previous task back
    ///
    /// Since idle tasks are never in the runqueue, nothing special to do.
    fn put_prev_task(&self, rq: RunQueueRef, prev: *mut Task, _next: *mut Task) {
        if rq.is_null() || prev.is_null() {
            return;
        }

        unsafe {
            // Update exec_start for the idle task
            let now = super::cfs::sched_clock();
            (*prev).sched_entity().set_exec_start(now);
        }
    }

    /// Set next task to run
    ///
    /// Updates exec_start for the idle task.
    fn set_next_task(&self, rq: RunQueueRef, next: *mut Task, _first: bool) {
        if rq.is_null() || next.is_null() {
            return;
        }

        unsafe {
            // Update exec_start for the idle task
            let now = super::cfs::sched_clock();
            (*next).sched_entity().set_exec_start(now);
        }
    }

    /// Balance is not applicable to idle class
    ///
    /// Balance should never be called for the idle class.
    /// It's only called as a fallback when no higher class has tasks.
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        // No balancing for idle class
        false
    }

    /// Idle task is per-CPU and cannot migrate
    fn select_task_rq(&self, _task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        // Idle task stays on its CPU
        cpu
    }

    /// Scheduler tick for idle task
    ///
    /// Updates the idle task's execution time.
    fn task_tick(&self, rq: RunQueueRef, task: *mut Task, _queued: bool) {
        if rq.is_null() || task.is_null() {
            return;
        }

        // Update current idle task's runtime
        self.update_curr(rq);
    }

    /// Update current task's runtime
    ///
    /// Updates the idle task's exec_start.
    fn update_curr(&self, rq: RunQueueRef) {
        if rq.is_null() {
            return;
        }

        unsafe {
            let rq_ref = &*rq;
            let curr = rq_ref.current;

            if curr.is_null() {
                return;
            }

            // Update exec_start
            let now = super::cfs::sched_clock();
            (*curr).sched_entity().set_exec_start(now);
        }
    }

    /// Get RR time slice (idle tasks don't have time slices)
    fn get_rr_interval(&self, _rq: RunQueueRef, _task: *mut Task) -> u32 {
        // Idle tasks run indefinitely until something else becomes runnable
        0
    }

    /// Idle class always has the idle task available
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        // The idle task is always available as a fallback
        true
    }

    /// Idle is the lowest priority class - no next class
    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        None
    }
}

/// Global idle scheduling class instance
pub static IDLE_SCHED_CLASS: IdleSchedClass = IdleSchedClass::new();
