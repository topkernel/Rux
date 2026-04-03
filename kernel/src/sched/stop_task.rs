//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Stop Task Scheduling Implementation
//!
//! Stop tasks are the highest priority tasks used for CPU hotplug
//! and active load balancing. They preempt everything and never migrate.
//!
//! With the global RunQueue design, stop tasks are managed directly in
//! PerCpuState (sched.rs). The SchedClass methods are no-ops because
//! the actual scheduling logic operates on PerCpuState directly.

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

    /// Stop tasks are managed in PerCpuState — no-op here.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Stop tasks are managed in PerCpuState — no-op here.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// Stop tasks don't yield
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Stop task preempts everything, no need to check
    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual pick happens in sched.rs::pick_next_task via PerCpuState.
    fn pick_next_task(&self, _rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        core::ptr::null_mut()
    }

    /// Stop tasks don't need special handling
    fn put_prev_task(&self, _rq: RunQueueRef, _prev: *mut Task, _next: *mut Task) {
    }

    /// Stop tasks don't need special setup
    fn set_next_task(&self, _rq: RunQueueRef, _next: *mut Task, _first: bool) {
    }

    /// Stop class doesn't balance
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// Stop task is per-CPU and cannot migrate
    fn select_task_rq(&self, _task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        cpu
    }

    /// Stop tasks don't have time slices
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// Stop tasks don't track runtime
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, _task: *mut Task) -> u32 {
        0
    }

    /// Actual runnable check happens in sched.rs via PerCpuState.
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        false
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&super::deadline::DL_SCHED_CLASS)
    }
}

/// Global stop scheduling class instance
pub static STOP_SCHED_CLASS: StopSchedClass = StopSchedClass::new();
