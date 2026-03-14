//! Stop Task Scheduling Implementation
//!
//! Stop tasks are the highest priority tasks used for CPU hotplug
//! and active load balancing. They preempt everything and never migrate.

use crate::process::task::Task;
use super::class::{SchedClass, RunQueueRef};

/// Stop task scheduling class
pub struct StopSchedClass;

impl StopSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for StopSchedClass {
    fn name(&self) -> &'static str {
        "stop"
    }

    fn enqueue_task(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            // Set the stop task for this runqueue
            (*rq).stop = task;
        }
    }

    fn dequeue_task(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) -> bool {
        if rq.is_null() || task.is_null() {
            return false;
        }

        unsafe {
            if (*rq).stop == task {
                (*rq).stop = core::ptr::null_mut();
                return true;
            }
        }

        false
    }

    fn yield_task(&self, _rq: RunQueueRef) {
        // Stop tasks don't yield
    }

    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
        // Stop task preempts everything, no need to check
    }

    fn pick_next_task(&self, rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        if rq.is_null() {
            return core::ptr::null_mut();
        }

        unsafe {
            // Return the stop task if one exists
            let stop = (*rq).stop;
            if !stop.is_null() {
                return stop;
            }
        }

        core::ptr::null_mut()
    }

    fn put_prev_task(&self, _rq: RunQueueRef, _prev: *mut Task, _next: *mut Task) {
        // Stop tasks don't need special handling
    }

    fn set_next_task(&self, _rq: RunQueueRef, _next: *mut Task, _first: bool) {
        // Stop tasks don't need special setup
    }

    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        // Stop class doesn't balance
        false
    }

    fn select_task_rq(&self, _task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        // Stop task is per-CPU and cannot migrate
        cpu
    }

    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
        // Stop tasks don't have time slices
    }

    fn update_curr(&self, _rq: RunQueueRef) {
        // Stop tasks don't track runtime
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, _task: *mut Task) -> u32 {
        0
    }

    fn has_runnable(&self, rq: RunQueueRef) -> bool {
        if rq.is_null() {
            return false;
        }

        unsafe {
            !(*rq).stop.is_null()
        }
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&super::deadline::DL_SCHED_CLASS)
    }
}

/// Global stop scheduling class instance
pub static STOP_SCHED_CLASS: StopSchedClass = StopSchedClass::new();
