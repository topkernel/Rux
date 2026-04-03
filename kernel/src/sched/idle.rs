//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Idle Scheduling Class Implementation
//!
//! The idle scheduling class manages per-CPU idle tasks. These are the lowest
//! priority tasks that run when no other tasks are runnable.
//!
//! IMPORTANT: This is for per-CPU idle tasks only, NOT for SCHED_IDLE tasks.
//! SCHED_IDLE tasks are handled by the fair class with very low weight (3).
//!
//! With the global RunQueue design, idle tasks are managed directly in
//! PerCpuState (sched.rs). The SchedClass methods are simplified because
//! the actual scheduling logic operates on PerCpuState directly.

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

    /// Idle tasks are never enqueued — they are always available via PerCpuState.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Sleeping in the idle task is illegal.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// Idle tasks don't yield
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Any waking task preempts idle immediately.
    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
        super::sched::resched_curr();
    }

    /// Actual pick happens in sched.rs::pick_next_task via PerCpuState.
    fn pick_next_task(&self, _rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        core::ptr::null_mut()
    }

    /// Idle tasks are never in the runqueue, nothing special to do.
    fn put_prev_task(&self, _rq: RunQueueRef, _prev: *mut Task, _next: *mut Task) {
    }

    /// Minimal — actual setup happens in sched.rs.
    fn set_next_task(&self, _rq: RunQueueRef, _next: *mut Task, _first: bool) {
    }

    /// No balancing for idle class
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// Idle task is per-CPU and cannot migrate
    fn select_task_rq(&self, _task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        cpu
    }

    /// Minimal — actual tick handling happens in sched.rs::scheduler_tick.
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// Minimal — idle tasks don't track runtime.
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    /// Idle tasks run indefinitely until something else becomes runnable
    fn get_rr_interval(&self, _rq: RunQueueRef, _task: *mut Task) -> u32 {
        0
    }

    /// The idle task is always available as a fallback
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        true
    }

    /// Idle is the lowest priority class — no next class
    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        None
    }
}

/// Global idle scheduling class instance
pub static IDLE_SCHED_CLASS: IdleSchedClass = IdleSchedClass::new();
