//! Fair Scheduling (CFS) Class Implementation
//!
//! This wraps the existing CFS implementation to implement the SchedClass trait.
//!
//! IMPORTANT: This class handles:
//! - SCHED_NORMAL: Normal tasks with nice-based weights
//! - SCHED_BATCH: Batch tasks (similar to normal but with different wakeup behavior)
//! - SCHED_IDLE: Idle policy tasks with very low weight (WEIGHT_IDLEPRIO = 3)
//!
//! SCHED_IDLE policy is NOT the same as idle_sched_class:
//! - idle_sched_class is for per-CPU idle tasks (pid=0) only
//! - SCHED_IDLE policy tasks use this class with minimal weight

use crate::process::task::{Task, SchedPolicy};
use super::class::{SchedClass, RunQueueRef};

/// Fair scheduling class - wraps CFS
pub struct FairSchedClass;

impl FairSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for FairSchedClass {
    fn name(&self) -> &'static str {
        "fair"
    }

    fn enqueue_task(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            // For SCHED_IDLE tasks, set weight to WEIGHT_IDLEPRIO (3)
            // This gives them minimal CPU time compared to normal tasks
            if (*task).policy() == SchedPolicy::Idle {
                let se = (*task).sched_entity_mut();
                se.load.weight = super::cfs::WEIGHT_IDLEPRIO;
                se.load.inv_weight = 0; // Will be recalculated when needed
            }

            let rq = &mut *rq;
            rq.cfs_rq.enqueue(task);
        }
    }

    fn dequeue_task(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) -> bool {
        if rq.is_null() || task.is_null() {
            return false;
        }

        unsafe {
            let rq = &mut *rq;
            rq.cfs_rq.dequeue(task)
        }
    }

    fn yield_task(&self, rq: RunQueueRef) {
        if rq.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;
            let curr = rq.current;

            if !curr.is_null() {
                // Update vruntime to be slightly higher
                // This moves the task to the right in the RB-tree
                let se = (*curr).sched_entity();
                let vruntime = se.get_vruntime();
                se.set_vruntime(vruntime + super::cfs::SCHED_MIN_GRANULARITY_NS);
            }
        }
    }

    fn wakeup_preempt(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;
            let curr = rq.current;

            if curr.is_null() {
                return;
            }

            let curr_se = (*curr).sched_entity();
            let task_se = (*task).sched_entity();

            // Check if task should preempt current
            if rq.cfs_rq.check_preempt(curr_se, task_se) {
                super::sched::resched_curr();
            }
        }
    }

    fn pick_next_task(&self, rq: RunQueueRef, prev: *mut Task) -> *mut Task {
        if rq.is_null() {
            return core::ptr::null_mut();
        }

        unsafe {
            let rq = &mut *rq;

            // Update current task's runtime
            let now = super::cfs::sched_clock();
            rq.cfs_rq.update_curr(now);

            // If previous task is still running and was a fair task, re-enqueue it
            // This includes SCHED_NORMAL, SCHED_BATCH, and SCHED_IDLE tasks
            if !prev.is_null() && (*prev).state().bits() == 0 {
                let prev_policy = (*prev).policy();
                if prev_policy == SchedPolicy::Normal
                    || prev_policy == SchedPolicy::Batch
                    || prev_policy == SchedPolicy::Idle
                {
                    rq.cfs_rq.enqueue(prev);
                }
            }

            // Pick next task from CFS queue
            match rq.cfs_rq.pick_next() {
                Some(next) => {
                    // Set as current and calculate time slice
                    rq.cfs_rq.set_curr(next);

                    let se = (*next).sched_entity();
                    let slice_ns = rq.cfs_rq.sched_slice(se);
                    let slice_ms = super::cfs::sched_slice_to_ms(slice_ns);
                    (*next).set_time_slice(slice_ms.max(1) as u32);

                    next
                }
                None => core::ptr::null_mut(),
            }
        }
    }

    fn put_prev_task(&self, rq: RunQueueRef, prev: *mut Task, _next: *mut Task) {
        if rq.is_null() || prev.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;

            // Update runtime
            let now = super::cfs::sched_clock();
            rq.cfs_rq.update_curr(now);

            // Re-enqueue if still runnable
            if (*prev).state().bits() == 0 { // TASK_RUNNING
                rq.cfs_rq.enqueue(prev);
            }
        }
    }

    fn set_next_task(&self, rq: RunQueueRef, next: *mut Task, _first: bool) {
        if rq.is_null() || next.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;
            rq.cfs_rq.set_curr(next);

            // Set on_rq to false
            (*next).sched_entity().set_on_rq(false);
        }
    }

    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        // CFS load balancing is handled separately
        // This is called during pick_next_task
        false
    }

    fn select_task_rq(&self, task: *mut Task, cpu: i32, flags: i32) -> i32 {
        if task.is_null() {
            return cpu;
        }

        // For wakeups, try to find the best CPU
        // For now, keep on current CPU
        // TODO: Implement proper CPU selection for wake balancing
        let _ = flags; // suppress unused warning
        cpu
    }

    fn task_tick(&self, rq: RunQueueRef, task: *mut Task, queued: bool) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;

            // Update current task's runtime
            let now = super::cfs::sched_clock();
            rq.cfs_rq.update_curr(now);

            if !queued {
                return;
            }

            // Check if preemption is needed
            let curr_se = (*task).sched_entity();

            // Peek at next task
            if let Some(next) = rq.cfs_rq.peek_next() {
                if !next.is_null() && next != task {
                    let next_se = (*next).sched_entity();

                    if rq.cfs_rq.check_preempt(curr_se, next_se) {
                        super::sched::resched_curr();
                    }
                }
            }
        }
    }

    fn update_curr(&self, rq: RunQueueRef) {
        if rq.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;
            let now = super::cfs::sched_clock();
            rq.cfs_rq.update_curr(now);
        }
    }

    fn get_rr_interval(&self, rq: RunQueueRef, task: *mut Task) -> u32 {
        if rq.is_null() || task.is_null() {
            return 1;
        }

        unsafe {
            let rq = &*rq;
            let se = (*task).sched_entity();
            let slice_ns = rq.cfs_rq.sched_slice(se);
            super::cfs::sched_slice_to_ms(slice_ns).max(1)
        }
    }

    fn has_runnable(&self, rq: RunQueueRef) -> bool {
        if rq.is_null() {
            return false;
        }

        unsafe {
            !(*rq).cfs_rq.is_empty()
        }
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&super::idle::IDLE_SCHED_CLASS)
    }
}

/// Global fair scheduling class instance
pub static FAIR_SCHED_CLASS: FairSchedClass = FairSchedClass::new();
