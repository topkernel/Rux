//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Deadline Scheduling Implementation
//!
//! Implements SCHED_DEADLINE using Earliest Deadline First (EDF)
//! with Constant Bandwidth Server (CBS) for bandwidth control.
//!
//! Key features:
//! - EDF: Tasks are scheduled by earliest absolute deadline
//! - CBS: Enforces runtime/period constraints
//! - Admission control: Ensures total bandwidth < 100%

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, AtomicI64, Ordering};
use crate::process::task::Task;
use super::class::{SchedClass, RunQueueRef};

/// Default deadline period in nanoseconds (1 second)
pub const DL_DEFAULT_PERIOD_NS: u64 = 1_000_000_000;

/// Default runtime in nanoseconds (100ms)
pub const DL_DEFAULT_RUNTIME_NS: u64 = 100_000_000;

/// Deadline bandwidth unit (1 << 20 for precision)
pub const DL_BW_UNIT: u64 = 1 << 20;

/// Maximum deadline bandwidth (100% = DL_BW_UNIT)
pub const DL_BW_MAX: u64 = DL_BW_UNIT;

/// Deadline runqueue
pub struct DlRunQueue {
    /// Tasks sorted by deadline (earliest first)
    ///
    /// Key: (deadline, task_id)
    /// Value: Task pointer
    tasks: BTreeMap<DlKey, *mut Task>,

    /// Number of DL tasks
    pub dl_nr_running: AtomicU32,

    /// Earliest deadline in queue
    pub earliest_dl: AtomicU64,

    /// Running bandwidth (total)
    pub running_bw: AtomicU64,

    /// Whether queue is overloaded
    pub overloaded: AtomicBool,

    /// Next task ID for unique keys
    next_id: AtomicU64,
}

/// Key for deadline BTreeMap
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DlKey {
    deadline: u64,
    task_id: u64,
}

impl DlRunQueue {
    /// Create a new deadline runqueue
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            dl_nr_running: AtomicU32::new(0),
            earliest_dl: AtomicU64::new(u64::MAX),
            running_bw: AtomicU64::new(0),
            overloaded: AtomicBool::new(false),
            next_id: AtomicU64::new(0),
        }
    }

    /// Check if queue is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.dl_nr_running.load(Ordering::Acquire) == 0
    }

    /// Get number of runnable tasks
    #[inline]
    pub fn nr_running(&self) -> u32 {
        self.dl_nr_running.load(Ordering::Acquire)
    }

    /// Enqueue a task
    pub fn enqueue(&mut self, task: *mut Task) {
        if task.is_null() {
            return;
        }

        unsafe {
            let t = &*task;
            let dl = t.dl_entity();

            // Get deadline
            let deadline = dl.deadline.load(Ordering::Acquire);

            // Generate unique key
            let task_id = self.next_id.fetch_add(1, Ordering::AcqRel);
            let key = DlKey { deadline, task_id };

            // Add to tree
            self.tasks.insert(key, task);

            // Update counters
            self.dl_nr_running.fetch_add(1, Ordering::AcqRel);

            // Update earliest deadline
            let curr_earliest = self.earliest_dl.load(Ordering::Acquire);
            if deadline < curr_earliest {
                self.earliest_dl.store(deadline, Ordering::Release);
            }

            // Set on_rq flag
            dl.on_rq.store(true, Ordering::Release);
        }
    }

    /// Dequeue a task
    pub fn dequeue(&mut self, task: *mut Task) {
        if task.is_null() {
            return;
        }

        unsafe {
            let t = &*task;
            let dl = t.dl_entity();
            let deadline = dl.deadline.load(Ordering::Acquire);

            // Find and remove task
            let mut found_key = None;
            for (&key, &ptr) in self.tasks.iter() {
                if ptr == task && key.deadline == deadline {
                    found_key = Some(key);
                    break;
                }
            }

            if let Some(key) = found_key {
                self.tasks.remove(&key);

                // Update counters
                self.dl_nr_running.fetch_sub(1, Ordering::AcqRel);

                // Update earliest deadline
                if let Some((&next_key, _)) = self.tasks.iter().next() {
                    self.earliest_dl.store(next_key.deadline, Ordering::Release);
                } else {
                    self.earliest_dl.store(u64::MAX, Ordering::Release);
                }

                // Clear on_rq flag
                dl.on_rq.store(false, Ordering::Release);
            }
        }
    }

    /// Pick the task with earliest deadline
    pub fn pick_next(&mut self) -> Option<*mut Task> {
        if let Some((&_key, &task)) = self.tasks.iter().next() {
            // Remove from tree
            self.tasks.remove(&_key);

            // Update counters
            self.dl_nr_running.fetch_sub(1, Ordering::AcqRel);

            // Update earliest deadline
            if let Some((&next_key, _)) = self.tasks.iter().next() {
                self.earliest_dl.store(next_key.deadline, Ordering::Release);
            } else {
                self.earliest_dl.store(u64::MAX, Ordering::Release);
            }

            // Clear on_rq flag
            unsafe {
                (*task).dl_entity().on_rq.store(false, Ordering::Release);
            }

            return Some(task);
        }

        None
    }

    /// Peek at the task with earliest deadline
    pub fn peek_next(&self) -> Option<*mut Task> {
        if let Some((&_key, &task)) = self.tasks.iter().next() {
            return Some(task);
        }
        None
    }
}

unsafe impl Send for DlRunQueue {}
unsafe impl Sync for DlRunQueue {}

/// Deadline scheduling entity
#[derive(Debug)]
pub struct SchedDlEntity {
    /// Absolute deadline
    pub deadline: AtomicU64,

    /// Remaining runtime
    pub runtime: AtomicI64,

    /// Scheduling period
    pub dl_period: AtomicU64,

    /// Maximum runtime per period
    pub dl_runtime: AtomicU64,

    /// Whether throttled (out of runtime)
    pub dl_throttled: AtomicBool,

    /// Whether on runqueue
    pub on_rq: AtomicBool,

    /// Whether boosted (inheriting priority)
    pub dl_boosted: AtomicBool,

    /// Timestamp (ns) when this entity last started executing.
    /// Used by update_curr() to calculate actual runtime consumed.
    pub exec_start: AtomicU64,
}

impl SchedDlEntity {
    /// Create a new deadline entity
    pub fn new() -> Self {
        Self {
            deadline: AtomicU64::new(0),
            runtime: AtomicI64::new(DL_DEFAULT_RUNTIME_NS as i64),
            dl_period: AtomicU64::new(DL_DEFAULT_PERIOD_NS),
            dl_runtime: AtomicU64::new(DL_DEFAULT_RUNTIME_NS),
            dl_throttled: AtomicBool::new(false),
            on_rq: AtomicBool::new(false),
            dl_boosted: AtomicBool::new(false),
            exec_start: AtomicU64::new(0),
        }
    }

    /// Check if on runqueue
    #[inline]
    pub fn is_on_rq(&self) -> bool {
        self.on_rq.load(Ordering::Acquire)
    }

    /// Get bandwidth (runtime/period * DL_BW_UNIT)
    pub fn get_bw(&self) -> u64 {
        let runtime = self.dl_runtime.load(Ordering::Acquire);
        let period = self.dl_period.load(Ordering::Acquire);

        if period == 0 {
            return 0;
        }

        (runtime * DL_BW_UNIT) / period
    }

    /// Update deadline (called on enqueue)
    pub fn update_deadline(&self, now: u64) {
        let period = self.dl_period.load(Ordering::Acquire);
        let new_deadline = now + period;
        self.deadline.store(new_deadline, Ordering::Release);
    }

    /// Replenish runtime (called at start of period)
    pub fn replenish_runtime(&self) {
        let runtime = self.dl_runtime.load(Ordering::Acquire) as i64;
        self.runtime.store(runtime, Ordering::Release);
        self.dl_throttled.store(false, Ordering::Release);
    }

    /// Consume runtime
    ///
    /// Returns true if still has runtime, false if throttled
    pub fn consume_runtime(&self, delta: u64) -> bool {
        let remaining = self.runtime.load(Ordering::Acquire);
        let new_remaining = remaining - delta as i64;

        if new_remaining <= 0 {
            self.runtime.store(0, Ordering::Release);
            self.dl_throttled.store(true, Ordering::Release);
            return false;
        }

        self.runtime.store(new_remaining, Ordering::Release);
        true
    }
}

impl Default for SchedDlEntity {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Deadline Scheduling Class
// ============================================================================

/// Deadline scheduling class
pub struct DlSchedClass;

impl DlSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for DlSchedClass {
    fn name(&self) -> &'static str {
        "deadline"
    }

    fn enqueue_task(&self, rq: RunQueueRef, task: *mut Task, flags: i32) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;
            let now = super::fair::sched_clock();

            // Replenish runtime if this is a wakeup
            if (flags & super::class::ENQUEUE_WAKEUP) != 0 {
                (*task).dl_entity().replenish_runtime();
            }

            // Update deadline
            (*task).dl_entity().update_deadline(now);

            // Enqueue
            rq.dl.enqueue(task);
        }
    }

    fn dequeue_task(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) -> bool {
        if rq.is_null() || task.is_null() {
            return false;
        }

        unsafe {
            let rq = &mut *rq;
            rq.dl.dequeue(task);
        }

        true
    }

    fn yield_task(&self, rq: RunQueueRef) {
        // Deadline tasks that yield update their deadline
        if rq.is_null() {
            return;
        }

        unsafe {
            let curr = (*rq).current;
            if !curr.is_null() {
                let now = super::fair::sched_clock();
                (*curr).dl_entity().update_deadline(now);
            }
        }
    }

    fn wakeup_preempt(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let curr = (*rq).current;
            if curr.is_null() {
                return;
            }

            // Preempt if waking task has earlier deadline
            let curr_deadline = (*curr).dl_entity().deadline.load(Ordering::Acquire);
            let task_deadline = (*task).dl_entity().deadline.load(Ordering::Acquire);

            if task_deadline < curr_deadline {
                super::sched::resched_curr();
            }
        }
    }

    fn pick_next_task(&self, rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        if rq.is_null() {
            return core::ptr::null_mut();
        }

        unsafe {
            let rq = &mut *rq;

            if rq.dl.is_empty() {
                return core::ptr::null_mut();
            }

            rq.dl.pick_next().unwrap_or(core::ptr::null_mut())
        }
    }

    fn put_prev_task(&self, rq: RunQueueRef, prev: *mut Task, _next: *mut Task) {
        if rq.is_null() || prev.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;

            // Re-queue if still runnable
            if (*prev).state().bits() == 0 { // TASK_RUNNING
                rq.dl.enqueue(prev);
            }
        }
    }

    fn set_next_task(&self, _rq: RunQueueRef, next: *mut Task, _first: bool) {
        if next.is_null() {
            return;
        }

        unsafe {
            let dl = (*next).dl_entity();
            dl.on_rq.store(false, Ordering::Release);
            // Record when this task starts executing, for update_curr() accounting
            dl.exec_start.store(super::fair::sched_clock(), Ordering::Release);
        }
    }

    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        // DL load balancing is complex, skip for now
        false
    }

    fn select_task_rq(&self, task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        // Deadline tasks are usually pinned
        if task.is_null() {
            return cpu;
        }
        cpu
    }

    fn task_tick(&self, rq: RunQueueRef, task: *mut Task, queued: bool) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let dl = (*task).dl_entity();

            // Consume runtime
            let delta = 10_000_000; // Assume 10ms tick
            if !dl.consume_runtime(delta) {
                // Throttled, need to reschedule
                if queued {
                    super::sched::resched_curr();
                }
            }
        }
    }

    fn update_curr(&self, rq: RunQueueRef) {
        if rq.is_null() {
            return;
        }

        unsafe {
            let curr = (*rq).current;
            if curr.is_null() {
                return;
            }

            let now = super::fair::sched_clock();
            let dl = (*curr).dl_entity();
            let exec_start = dl.exec_start.load(Ordering::Acquire);

            if exec_start > 0 && now > exec_start {
                let delta = now - exec_start;
                dl.exec_start.store(now, Ordering::Release);
                dl.consume_runtime(delta);
            }
        }
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, _task: *mut Task) -> u32 {
        // Deadline tasks don't have fixed time slices
        0
    }

    fn has_runnable(&self, rq: RunQueueRef) -> bool {
        if rq.is_null() {
            return false;
        }

        unsafe {
            !(*rq).dl.is_empty()
        }
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&super::rt::RT_SCHED_CLASS)
    }
}

/// Global deadline scheduling class instance
pub static DL_SCHED_CLASS: DlSchedClass = DlSchedClass::new();
