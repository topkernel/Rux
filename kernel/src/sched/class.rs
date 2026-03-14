//! Scheduling Class Infrastructure
//!
//! Defines the SchedClass trait and scheduling flags

use crate::process::task::Task;

/// Enqueue flags
pub const ENQUEUE_WAKEUP: i32 = 0x0001;
pub const ENQUEUE_RESTORE: i32 = 0x0002;
pub const ENQUEUE_MOVE: i32 = 0x0004;
pub const ENQUEUE_NOCLOCK: i32 = 0x0008;
pub const ENQUEUE_MIGRATING: i32 = 0x0010;
pub const ENQUEUE_HEAD: i32 = 0x00010000;
pub const ENQUEUE_REPLENISH: i32 = 0x00020000;
pub const ENQUEUE_MIGRATED: i32 = 0x00040000;

/// Dequeue flags
pub const DEQUEUE_SLEEP: i32 = 0x0001;
pub const DEQUEUE_SAVE: i32 = 0x0002;
pub const DEQUEUE_MOVE: i32 = 0x0004;
pub const DEQUEUE_NOCLOCK: i32 = 0x0008;
pub const DEQUEUE_MIGRATING: i32 = 0x0010;

/// Wake flags
pub const WF_EXEC: i32 = 0x02;
pub const WF_FORK: i32 = 0x04;
pub const WF_TTWU: i32 = 0x08;
pub const WF_SYNC: i32 = 0x10;
pub const WF_MIGRATED: i32 = 0x20;
pub const WF_CURRENT_CPU: i32 = 0x40;

/// RunQueue type alias (forward declaration)
/// The actual struct is defined in sched.rs
pub type RunQueueRef = *mut crate::sched::sched::RunQueue;

/// Scheduling Class Trait
///
/// This trait defines the interface for all scheduling classes.
/// It is designed to be dyn-compatible (object-safe).
///
/// Scheduling classes are ordered by priority (highest to lowest):
/// 1. stop_sched_class - CPU hotplug/migration
/// 2. dl_sched_class - Deadline (EDF + CBS)
/// 3. rt_sched_class - Real-time (FIFO/RR)
/// 4. fair_sched_class - CFS
/// 5. idle_sched_class - Per-CPU idle task
pub trait SchedClass: Send + Sync {
    /// Get the name of this scheduling class
    fn name(&self) -> &'static str;

    /// Enqueue a task into the runqueue
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `task` - Task to enqueue
    /// * `flags` - Enqueue flags
    fn enqueue_task(&self, rq: RunQueueRef, task: *mut Task, flags: i32);

    /// Dequeue a task from the runqueue
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `task` - Task to dequeue
    /// * `flags` - Dequeue flags
    ///
    /// # Returns
    /// true if task was dequeued, false otherwise
    fn dequeue_task(&self, rq: RunQueueRef, task: *mut Task, flags: i32) -> bool;

    /// Yield the current task
    fn yield_task(&self, rq: RunQueueRef);

    /// Check if waking task should preempt current
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `task` - Task being woken
    /// * `flags` - Wake flags
    fn wakeup_preempt(&self, rq: RunQueueRef, task: *mut Task, flags: i32);

    /// Pick the next task to run
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `prev` - Previous task (may be null)
    ///
    /// # Returns
    /// Next task to run, or null if no task available
    fn pick_next_task(&self, rq: RunQueueRef, prev: *mut Task) -> *mut Task;

    /// Put the previous task back
    ///
    /// Called when a task is being replaced by another task.
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `prev` - Previous task
    /// * `next` - Next task
    fn put_prev_task(&self, rq: RunQueueRef, prev: *mut Task, next: *mut Task);

    /// Set the next task to run
    ///
    /// Called when a task is selected to run.
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `next` - Next task
    /// * `first` - Whether this is the first time setting this task
    fn set_next_task(&self, rq: RunQueueRef, next: *mut Task, first: bool);

    /// Balance the runqueue
    ///
    /// Called before pick_next_task to pull tasks from other CPUs.
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `prev` - Previous task
    ///
    /// # Returns
    /// true if tasks were moved, false otherwise
    fn balance(&self, rq: RunQueueRef, prev: *mut Task) -> bool;

    /// Select the best CPU for a task
    ///
    /// # Arguments
    /// * `task` - Task to place
    /// * `cpu` - Current CPU
    /// * `flags` - Wake flags
    ///
    /// # Returns
    /// Best CPU ID for this task
    fn select_task_rq(&self, task: *mut Task, cpu: i32, flags: i32) -> i32;

    /// Scheduler tick callback
    ///
    /// Called on each timer tick for the current task.
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `task` - Current task
    /// * `queued` - Whether task is still queued
    fn task_tick(&self, rq: RunQueueRef, task: *mut Task, queued: bool);

    /// Update current task's runtime
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    fn update_curr(&self, rq: RunQueueRef);

    /// Get RR time slice interval
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    /// * `task` - Task to query
    ///
    /// # Returns
    /// Time slice in milliseconds
    fn get_rr_interval(&self, rq: RunQueueRef, task: *mut Task) -> u32;

    /// Check if this class has runnable tasks
    ///
    /// # Arguments
    /// * `rq` - RunQueue pointer
    ///
    /// # Returns
    /// true if there are runnable tasks
    fn has_runnable(&self, rq: RunQueueRef) -> bool;

    /// Get the next scheduling class in priority order
    ///
    /// Returns None if this is the lowest priority class
    fn next_class(&self) -> Option<&'static dyn SchedClass>;
}

/// Scheduling class IDs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SchedClassId {
    /// Stop class (highest priority)
    Stop = 0,
    /// Deadline class
    Deadline = 1,
    /// Real-time class
    Rt = 2,
    /// Fair class (CFS)
    Fair = 3,
    /// Idle class (lowest priority)
    Idle = 4,
}

impl SchedClassId {
    /// Get the scheduling class for this ID
    pub fn to_class(self) -> &'static dyn SchedClass {
        match self {
            SchedClassId::Stop => &super::stop_task::STOP_SCHED_CLASS,
            SchedClassId::Deadline => &super::deadline::DL_SCHED_CLASS,
            SchedClassId::Rt => &super::rt::RT_SCHED_CLASS,
            SchedClassId::Fair => &super::fair::FAIR_SCHED_CLASS,
            SchedClassId::Idle => &super::idle::IDLE_SCHED_CLASS,
        }
    }
}

/// Get the scheduling class for a task based on its policy
///
/// IMPORTANT: SCHED_IDLE policy tasks use fair_sched_class with very low weight (3).
/// Only per-CPU idle tasks (pid=0) use idle_sched_class.
pub fn task_sched_class(task: &Task) -> &'static dyn SchedClass {
    use crate::process::task::SchedPolicy;

    match task.policy() {
        SchedPolicy::Fifo | SchedPolicy::Rr => &super::rt::RT_SCHED_CLASS,
        SchedPolicy::Deadline => &super::deadline::DL_SCHED_CLASS,
        // SCHED_IDLE tasks are handled by fair class with weight=3 (WEIGHT_IDLEPRIO)
        SchedPolicy::Idle | SchedPolicy::Normal | SchedPolicy::Batch => &super::fair::FAIR_SCHED_CLASS,
    }
}

/// Iterate through scheduling classes in priority order (highest to lowest)
pub struct SchedClassIter {
    current: Option<&'static dyn SchedClass>,
}

impl SchedClassIter {
    /// Create a new iterator starting from the highest priority class
    pub fn new() -> Self {
        Self {
            current: Some(&super::stop_task::STOP_SCHED_CLASS),
        }
    }
}

impl Iterator for SchedClassIter {
    type Item = &'static dyn SchedClass;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current?;
        self.current = current.next_class();
        Some(current)
    }
}

/// Iterate through scheduling classes from a starting class
pub struct SchedClassFromIter {
    current: Option<&'static dyn SchedClass>,
}

impl SchedClassFromIter {
    /// Create a new iterator starting from the specified class
    pub fn from(start: &'static dyn SchedClass) -> Self {
        Self {
            current: Some(start),
        }
    }
}

impl Iterator for SchedClassFromIter {
    type Item = &'static dyn SchedClass;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current?;
        self.current = current.next_class();
        Some(current)
    }
}
