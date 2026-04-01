//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Real-Time Scheduling Implementation
//!
//! Implements SCHED_FIFO and SCHED_RR scheduling policies.
//!
//! Key features:
//! - Priority bitmap for O(1) highest priority task selection
//! - SCHED_FIFO: Run until blocked or preempted by higher priority
//! - SCHED_RR: FIFO with time slices (default 100ms)

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use crate::process::task::{Task, SchedPolicy};
use crate::list::ListHead;
use super::class::{SchedClass, RunQueueRef, ENQUEUE_HEAD};

/// Maximum RT priority (0-99, lower value = higher priority)
pub const MAX_RT_PRIO: usize = 100;

/// Default RR time slice in milliseconds
pub const RR_TIMESLICE_MS: u32 = 100;

/// RT runqueue
pub struct RtRunQueue {
    /// Number of RT tasks
    pub rt_nr_running: AtomicU32,

    /// Number of RR tasks
    pub rr_nr_running: AtomicU32,

    /// Highest priority in queue
    pub highest_prio: AtomicU32,

    /// Whether queue is overloaded (more tasks than CPUs)
    pub overloaded: AtomicBool,

    /// Priority bitmap: bit i is set if priority i has runnable tasks
    /// We need 100 bits, so use 2 x 64-bit words
    bitmap: [AtomicU64; 2],

    /// Per-priority task lists
    ///
    /// Each list head anchors tasks at that priority level
    queue: [ListHead; MAX_RT_PRIO],
}

impl RtRunQueue {
    /// Create a new RT runqueue
    pub fn new() -> Self {
        // Create array of ListHead - this is safe because ListHead::new()
        // creates a null-initialized struct
        let queue = [const { ListHead::new() }; MAX_RT_PRIO];

        Self {
            rt_nr_running: AtomicU32::new(0),
            rr_nr_running: AtomicU32::new(0),
            highest_prio: AtomicU32::new(MAX_RT_PRIO as u32),
            overloaded: AtomicBool::new(false),
            bitmap: [AtomicU64::new(0), AtomicU64::new(0)],
            queue,
        }
    }

    /// Initialize the runqueue (must call before use)
    pub fn init(&mut self) {
        for list in &mut self.queue {
            list.init();
        }
    }

    /// Get number of running tasks
    #[inline]
    pub fn nr_running(&self) -> u32 {
        self.rt_nr_running.load(Ordering::Acquire)
    }

    /// Check if queue is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.rt_nr_running.load(Ordering::Acquire) == 0
    }

    /// Find the highest priority with runnable tasks
    ///
    /// Returns priority (0-99) or None if empty
    /// Lower value = higher priority
    #[inline]
    fn find_highest_prio(&self) -> Option<u32> {
        let word0 = self.bitmap[0].load(Ordering::Acquire);
        if word0 != 0 {
            // Find first set bit (lowest priority number = highest priority)
            return Some(word0.trailing_zeros());
        }

        let word1 = self.bitmap[1].load(Ordering::Acquire);
        if word1 != 0 {
            // First set bit in word1 + 64
            return Some(word1.trailing_zeros() + 64);
        }

        None
    }

    /// Enqueue a task
    pub fn enqueue(&mut self, task: *mut Task, head: bool) {
        if task.is_null() {
            return;
        }

        unsafe {
            let t = &mut *task;
            let prio = t.rt_priority() as usize;

            if prio >= MAX_RT_PRIO {
                return;
            }

            // Add to priority list
            if head {
                // Add to front of list (for preempted tasks)
                t.rt_run_list.add(&mut self.queue[prio] as *mut ListHead);
            } else {
                // Add to tail (for normal enqueue)
                t.rt_run_list.add_tail(&mut self.queue[prio] as *mut ListHead);
            }

            // Set bit in bitmap
            let word_idx = prio / 64;
            let bit_idx = prio % 64;
            self.bitmap[word_idx].fetch_or(1u64 << bit_idx, Ordering::AcqRel);

            // Update counters
            self.rt_nr_running.fetch_add(1, Ordering::AcqRel);

            if t.policy() == SchedPolicy::Rr {
                self.rr_nr_running.fetch_add(1, Ordering::AcqRel);
            }

            // Update highest priority
            let curr_highest = self.highest_prio.load(Ordering::Acquire);
            if (prio as u32) < curr_highest {
                self.highest_prio.store(prio as u32, Ordering::Release);
            }

            // Set on_rq flag
            t.rt_entity().set_on_rq(true);
        }
    }

    /// Dequeue a task
    pub fn dequeue(&mut self, task: *mut Task) {
        if task.is_null() {
            return;
        }

        unsafe {
            let t = &mut *task;
            let prio = t.rt_priority() as usize;

            if prio >= MAX_RT_PRIO {
                return;
            }

            // Remove from list
            t.rt_run_list.del();

            // Clear bit in bitmap if list is now empty
            if self.queue[prio].is_empty() {
                let word_idx = prio / 64;
                let bit_idx = prio % 64;
                self.bitmap[word_idx].fetch_and(!(1u64 << bit_idx), Ordering::AcqRel);
            }

            // Update counters
            self.rt_nr_running.fetch_sub(1, Ordering::AcqRel);

            if t.policy() == SchedPolicy::Rr {
                self.rr_nr_running.fetch_sub(1, Ordering::AcqRel);
            }

            // Update highest priority if needed
            if self.is_empty() {
                self.highest_prio.store(MAX_RT_PRIO as u32, Ordering::Release);
            } else if let Some(new_highest) = self.find_highest_prio() {
                self.highest_prio.store(new_highest, Ordering::Release);
            }

            // Clear on_rq flag
            t.rt_entity().set_on_rq(false);
        }
    }

    /// Pick the next task to run
    pub fn pick_next(&mut self) -> Option<*mut Task> {
        let prio = self.find_highest_prio()?;
        let prio_usize = prio as usize;

        // Get first task from that priority's list
        // Use list_entry to get task from ListHead
        let list_ptr = self.queue[prio_usize].next;
        if list_ptr == &self.queue[prio_usize] as *const _ as *mut _ {
            return None;
        }

        // Calculate task pointer from list head pointer
        // Task contains rt_run_list at some offset
        let task = unsafe {
            let offset = core::mem::offset_of!(Task, rt_run_list);
            (list_ptr as *mut u8).sub(offset) as *mut Task
        };

        // Dequeue it
        self.dequeue(task);

        Some(task)
    }

    /// Peek at the next task (without removing)
    pub fn peek_next(&self) -> Option<*mut Task> {
        let prio = self.find_highest_prio()?;
        let prio_usize = prio as usize;

        let list_ptr = self.queue[prio_usize].next;
        if list_ptr == &self.queue[prio_usize] as *const _ as *mut _ {
            return None;
        }

        // Calculate task pointer from list head pointer
        let task = unsafe {
            let offset = core::mem::offset_of!(Task, rt_run_list);
            (list_ptr as *mut u8).sub(offset) as *mut Task
        };

        Some(task)
    }
}

unsafe impl Send for RtRunQueue {}
unsafe impl Sync for RtRunQueue {}

/// RT scheduling entity
///
/// Stored in the Task struct
#[derive(Debug)]
pub struct SchedRtEntity {
    /// Time slice (for SCHED_RR)
    pub time_slice: AtomicU32,

    /// Whether on runqueue
    pub on_rq: AtomicBool,
}

impl SchedRtEntity {
    /// Create a new RT entity
    pub fn new() -> Self {
        Self {
            time_slice: AtomicU32::new(RR_TIMESLICE_MS),
            on_rq: AtomicBool::new(false),
        }
    }

    /// Check if on runqueue
    #[inline]
    pub fn is_on_rq(&self) -> bool {
        self.on_rq.load(Ordering::Acquire)
    }

    /// Set runqueue status
    #[inline]
    pub fn set_on_rq(&self, on_rq: bool) {
        self.on_rq.store(on_rq, Ordering::Release);
    }

    /// Get time slice
    #[inline]
    pub fn get_time_slice(&self) -> u32 {
        self.time_slice.load(Ordering::Acquire)
    }

    /// Set time slice
    #[inline]
    pub fn set_time_slice(&self, slice: u32) {
        self.time_slice.store(slice, Ordering::Release);
    }

    /// Decrement time slice and return remaining
    #[inline]
    pub fn dec_time_slice(&self) -> u32 {
        let slice = self.time_slice.load(Ordering::Acquire);
        if slice > 0 {
            self.time_slice.store(slice - 1, Ordering::Release);
            slice - 1
        } else {
            0
        }
    }

    /// Reset time slice to default
    #[inline]
    pub fn reset_time_slice(&self) {
        self.time_slice.store(RR_TIMESLICE_MS, Ordering::Release);
    }
}

impl Default for SchedRtEntity {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// RT Scheduling Class
// ============================================================================

/// RT scheduling class
pub struct RtSchedClass;

impl RtSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for RtSchedClass {
    fn name(&self) -> &'static str {
        "rt"
    }

    fn enqueue_task(&self, rq: RunQueueRef, task: *mut Task, flags: i32) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;
            let head = (flags & ENQUEUE_HEAD) != 0;
            rq.rt.enqueue(task, head);
        }
    }

    fn dequeue_task(&self, rq: RunQueueRef, task: *mut Task, _flags: i32) -> bool {
        if rq.is_null() || task.is_null() {
            return false;
        }

        unsafe {
            let rq = &mut *rq;
            rq.rt.dequeue(task);
        }

        true
    }

    fn yield_task(&self, _rq: RunQueueRef) {
        // RT tasks don't yield to lower priority tasks
        // Just requeue at the end of the same priority list
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

            let curr_prio = (*curr).rt_priority();
            let task_prio = (*task).rt_priority();

            // Preempt if waking task has higher priority (lower value)
            if task_prio < curr_prio {
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

            if rq.rt.is_empty() {
                return core::ptr::null_mut();
            }

            rq.rt.pick_next().unwrap_or(core::ptr::null_mut())
        }
    }

    fn put_prev_task(&self, rq: RunQueueRef, prev: *mut Task, _next: *mut Task) {
        if rq.is_null() || prev.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;

            // Re-queue the previous task if it's still runnable
            if (*prev).state().bits() == 0 { // TASK_RUNNING
                rq.rt.enqueue(prev, false);
            }
        }
    }

    fn set_next_task(&self, _rq: RunQueueRef, next: *mut Task, _first: bool) {
        if next.is_null() {
            return;
        }

        unsafe {
            (*next).rt_entity().set_on_rq(false);
        }
    }

    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        // RT load balancing is done via push/pull operations
        false
    }

    fn select_task_rq(&self, task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        // For now, keep on current CPU
        // TODO: Implement proper RT load balancing
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
            let t = &*task;

            // Only SCHED_RR has time slices
            if t.policy() != SchedPolicy::Rr {
                return;
            }

            // Decrement time slice
            let remaining = t.rt_entity().dec_time_slice();

            if remaining == 0 && queued {
                // Time slice exhausted, reschedule
                t.rt_entity().reset_time_slice();

                // Re-queue at end of priority list
                let rq_ref = &mut *rq;
                rq_ref.rt.dequeue(task);
                rq_ref.rt.enqueue(task, false);

                // Request reschedule
                super::sched::resched_curr();
            }
        }
    }

    fn update_curr(&self, _rq: RunQueueRef) {
        // RT doesn't track vruntime
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, task: *mut Task) -> u32 {
        if task.is_null() {
            return 0;
        }

        unsafe {
            if (*task).policy() == SchedPolicy::Rr {
                RR_TIMESLICE_MS
            } else {
                0 // SCHED_FIFO has no time slice
            }
        }
    }

    fn has_runnable(&self, rq: RunQueueRef) -> bool {
        if rq.is_null() {
            return false;
        }

        unsafe {
            !(*rq).rt.is_empty()
        }
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&super::fair::FAIR_SCHED_CLASS)
    }
}

/// Global RT scheduling class instance
pub static RT_SCHED_CLASS: RtSchedClass = RtSchedClass::new();
