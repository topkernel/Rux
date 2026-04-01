//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Completely Fair Scheduler (CFS) Implementation
//!
//! Core concepts of CFS:
//! 1. Use virtual runtime (vruntime) to measure CPU time obtained by processes
//! 2. vruntime = actual runtime * (NICE_0_LOAD / process weight)
//! 3. Higher priority processes have larger weights, vruntime grows slower
//! 4. When scheduling, select the process with smallest vruntime to run
//!
//! Key data structures:
//! - SchedEntity: Scheduling entity, containing vruntime, weight, etc.
//! - CfsRunQueue: CFS run queue, using BTreeMap sorted by vruntime
//! - LoadWeight: Process weight, related to nice value
//!
//! This file also implements the FairSchedClass which wraps CFS:
//! - SCHED_NORMAL: Normal tasks with nice-based weights
//! - SCHED_BATCH: Batch tasks (similar to normal but with different wakeup behavior)
//! - SCHED_IDLE: Idle policy tasks with very low weight (WEIGHT_IDLEPRIO = 3)
//!
//! SCHED_IDLE policy is NOT the same as idle_sched_class:
//! - idle_sched_class is for per-CPU idle tasks (pid=0) only
//! - SCHED_IDLE policy tasks use this class with minimal weight

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// Use config values for scheduler timing
use crate::config::{KERNEL_HZ, CFS_MIN_GRANULARITY_NS, CFS_LATENCY_NS};
use crate::process::task::{Task, SchedPolicy};
use super::class::{SchedClass, RunQueueRef};

/// Clock frequency (HZ) - from config
const HZ: u64 = KERNEL_HZ as u64;

/// Scheduling granularity (nanoseconds) - from config
pub const SCHED_MIN_GRANULARITY_NS: u64 = CFS_MIN_GRANULARITY_NS;

/// Scheduling latency (nanoseconds) - from config
pub const SCHED_LATENCY_NS: u64 = CFS_LATENCY_NS;

/// Weight when nice value is 0
pub const NICE_0_LOAD: u64 = 1024;

/// Weight for SCHED_IDLE tasks
/// This is a very low weight (3) to give idle tasks minimal CPU time
pub const WEIGHT_IDLEPRIO: u64 = 3;

/// Nice value to weight mapping table
///
/// Nice value range: -20 to +19, total 40 levels
/// Weights change by a factor of 1.25 (approximately 25% increase per 1024)
pub const PRIO_TO_WEIGHT: [u64; 40] = [
    /* -20 */ 88761, 71755, 56483, 46273, 36291,
    /* -15 */ 29154, 23254, 18705, 14949, 11916,
    /* -10 */ 9548,  7620,  6100,  4904,  3906,
    /*  -5 */ 3121,  2501,  1991,  1586,  1277,
    /*   0 */ 1024,   820,   655,   526,   423,
    /*   5 */ 335,    272,   215,   172,   137,
    /*  10 */ 110,     87,    70,    56,    45,
    /*  15 */ 36,     29,    23,    18,    15,
];

/// Nice value to weight multiplier mapping table (for fast calculation)
///
/// Used to calculate vruntime: delta_exec * weight / lw->weight
/// Stores NICE_0_LOAD * 2^32 / weight
pub const PRIO_TO_WMULT: [u64; 40] = [
    /* -20 */ 48388, 59856, 76040, 92818, 118348,
    /* -15 */ 147320, 184698, 229616, 288308, 360437,
    /* -10 */ 449829, 563644, 704093, 875809, 1099582,
    /*  -5 */ 1376151, 1717300, 2157191, 2708050, 3363326,
    /*   0 */ 4194304, 5237760, 6557202, 8165337, 10153587,
    /*   5 */ 12820794, 15790321, 19976592, 24970740, 31350126,
    /*  10 */ 39045157, 49367440, 61356676, 76695844, 95443717,
    /*  15 */ 119304647, 148154320, 186737708, 238609294, 286331153,
];

/// Load weight
#[derive(Debug, Clone, Copy)]
pub struct LoadWeight {
    /// Weight value
    pub weight: u64,
    /// Weight multiplier (for fast division)
    pub inv_weight: u64,
}

impl LoadWeight {
    /// Create new load weight
    pub fn new(weight: u64) -> Self {
        Self {
            weight,
            inv_weight: 0,
        }
    }

    /// Create load weight from nice value
    ///
    /// Nice value range: -20 to +19
    /// Default nice value is 0, corresponding weight 1024
    pub fn from_nice(nice: i32) -> Self {
        // Convert nice value to array index (0-39)
        let idx = (nice + 20) as usize;
        let idx = idx.min(39).max(0);

        Self {
            weight: PRIO_TO_WEIGHT[idx],
            inv_weight: PRIO_TO_WMULT[idx],
        }
    }

    /// Update inv_weight (for fast division)
    pub fn update_inv_weight(&mut self) {
        if self.inv_weight == 0 {
            if self.weight >= (1u64 << 32) {
                self.inv_weight = 1;
            } else {
                self.inv_weight = (1u64 << 32) / self.weight;
            }
        }
    }
}

impl Default for LoadWeight {
    fn default() -> Self {
        Self::from_nice(0) // Default nice value is 0
    }
}

/// Scheduling entity
#[derive(Debug)]
pub struct SchedEntity {
    /// Load weight
    pub load: LoadWeight,

    /// Virtual runtime
    ///
    /// Smaller vruntime means the process has obtained less CPU time
    /// Scheduler prioritizes processes with smaller vruntime
    pub vruntime: AtomicU64,

    /// Cumulative execution time (nanoseconds)
    pub sum_exec_runtime: AtomicU64,

    /// Last execution start time (nanoseconds)
    pub exec_start: AtomicU64,

    /// Last cumulative execution time (for calculating delta)
    pub prev_sum_exec_runtime: AtomicU64,

    /// Whether in run queue
    pub on_rq: AtomicBool,

    /// Time slice (nanoseconds)
    pub slice: AtomicU64,
}

impl SchedEntity {
    /// Create new scheduling entity
    pub fn new() -> Self {
        Self {
            load: LoadWeight::default(),
            vruntime: AtomicU64::new(0),
            sum_exec_runtime: AtomicU64::new(0),
            exec_start: AtomicU64::new(0),
            prev_sum_exec_runtime: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(0),
        }
    }

    /// Create scheduling entity from nice value
    pub fn from_nice(nice: i32) -> Self {
        Self {
            load: LoadWeight::from_nice(nice),
            vruntime: AtomicU64::new(0),
            sum_exec_runtime: AtomicU64::new(0),
            exec_start: AtomicU64::new(0),
            prev_sum_exec_runtime: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(0),
        }
    }

    /// Set nice value
    pub fn set_nice(&mut self, nice: i32) {
        self.load = LoadWeight::from_nice(nice);
    }

    /// Get virtual runtime
    #[inline]
    pub fn get_vruntime(&self) -> u64 {
        self.vruntime.load(Ordering::Acquire)
    }

    /// Set virtual runtime
    #[inline]
    pub fn set_vruntime(&self, vruntime: u64) {
        self.vruntime.store(vruntime, Ordering::Release);
    }

    /// Add virtual runtime
    #[inline]
    pub fn add_vruntime(&self, delta: u64) {
        self.vruntime.fetch_add(delta, Ordering::AcqRel);
    }

    /// Update execution time
    ///
    /// # Arguments
    /// - `now`: Current time (nanoseconds)
    ///
    /// # Returns
    /// Execution time delta for this run (nanoseconds)
    pub fn update_exec_runtime(&self, now: u64) -> u64 {
        let exec_start = self.exec_start.load(Ordering::Acquire);

        if exec_start == 0 {
            // First execution, record start time
            self.exec_start.store(now, Ordering::Release);
            return 0;
        }

        let delta = if now > exec_start {
            now - exec_start
        } else {
            0 // Prevent time wraparound
        };

        // Update cumulative execution time
        self.sum_exec_runtime.fetch_add(delta, Ordering::AcqRel);

        // Update start time
        self.exec_start.store(now, Ordering::Release);

        delta
    }

    /// Calculate virtual runtime delta
    ///
    /// vruntime += delta_exec * (NICE_0_LOAD / weight)
    ///
    /// Use multiplier to avoid division:
    /// vruntime += delta_exec * (inv_weight >> 32)
    ///
    /// # Arguments
    /// - `delta_exec`: Actual execution time (nanoseconds)
    ///
    /// # Returns
    /// Virtual runtime delta
    pub fn calc_delta_fair(&self, delta_exec: u64) -> u64 {
        // If weight equals NICE_0_LOAD, return directly
        if self.load.weight == NICE_0_LOAD {
            return delta_exec;
        }

        // Use multiplication instead of division
        // delta = delta_exec * NICE_0_LOAD / weight
        //       = delta_exec * inv_weight >> 32
        let mut load = self.load;
        load.update_inv_weight();

        // Use 64-bit multiplication and shift
        let delta = (delta_exec * load.inv_weight) >> 32;

        delta
    }

    /// Update virtual runtime
    ///
    /// # Arguments
    /// - `delta_exec`: Actual execution time (nanoseconds)
    pub fn update_vruntime(&self, delta_exec: u64) {
        let delta_vruntime = self.calc_delta_fair(delta_exec);
        self.add_vruntime(delta_vruntime);
    }

    /// Check if in run queue
    #[inline]
    pub fn is_on_rq(&self) -> bool {
        self.on_rq.load(Ordering::Acquire)
    }

    /// Set run queue status
    #[inline]
    pub fn set_on_rq(&self, on_rq: bool) {
        self.on_rq.store(on_rq, Ordering::Release);
    }

    /// Get time slice
    #[inline]
    pub fn get_slice(&self) -> u64 {
        self.slice.load(Ordering::Acquire)
    }

    /// Set time slice
    #[inline]
    pub fn set_slice(&self, slice: u64) {
        self.slice.store(slice, Ordering::Release);
    }

    /// Get exec_start time
    #[inline]
    pub fn get_exec_start(&self) -> u64 {
        self.exec_start.load(Ordering::Acquire)
    }

    /// Set exec_start time
    #[inline]
    pub fn set_exec_start(&self, time: u64) {
        self.exec_start.store(time, Ordering::Release);
    }
}

impl Default for SchedEntity {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SchedEntity {
    fn clone(&self) -> Self {
        Self {
            load: self.load,
            vruntime: AtomicU64::new(self.vruntime.load(Ordering::Acquire)),
            sum_exec_runtime: AtomicU64::new(self.sum_exec_runtime.load(Ordering::Acquire)),
            exec_start: AtomicU64::new(0), // Reset execution start time
            prev_sum_exec_runtime: AtomicU64::new(self.prev_sum_exec_runtime.load(Ordering::Acquire)),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(self.slice.load(Ordering::Acquire)),
        }
    }
}

/// Key for BTreeMap
///
/// Since vruntime may be duplicate, we use (vruntime, task_ptr) as key
/// This ensures uniqueness
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VruntimeKey {
    vruntime: u64,
    task_id: u64, // Used to distinguish tasks with same vruntime
}

impl VruntimeKey {
    fn new(vruntime: u64, task_id: u64) -> Self {
        Self { vruntime, task_id }
    }
}

/// CFS run queue
pub struct CfsRunQueue {
    /// Task queue sorted by vruntime
    ///
    /// Key: (vruntime, task_id)
    /// Value: Task pointer
    tasks_timeline: BTreeMap<VruntimeKey, *mut crate::process::Task>,

    /// Currently running scheduling entity
    pub curr: *mut crate::process::Task,

    /// Minimum vruntime in queue
    ///
    /// Used for new task vruntime initialization
    /// New task vruntime = min_vruntime, this prevents new tasks from getting too much CPU time
    pub min_vruntime: AtomicU64,

    /// Number of tasks in run queue
    nr_running: AtomicU64,

    /// Total weight
    load_weight: AtomicU64,

    /// Next task ID (for generating unique keys)
    next_task_id: AtomicU64,
}

impl CfsRunQueue {
    /// Create new CFS run queue
    pub fn new() -> Self {
        Self {
            tasks_timeline: BTreeMap::new(),
            curr: core::ptr::null_mut(),
            min_vruntime: AtomicU64::new(0),
            nr_running: AtomicU64::new(0),
            load_weight: AtomicU64::new(0),
            next_task_id: AtomicU64::new(0),
        }
    }

    /// Get number of tasks in run queue
    #[inline]
    pub fn nr_running(&self) -> u64 {
        self.nr_running.load(Ordering::Acquire)
    }

    /// Check if run queue is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tasks_timeline.is_empty()
    }

    /// Get minimum vruntime
    #[inline]
    pub fn get_min_vruntime(&self) -> u64 {
        self.min_vruntime.load(Ordering::Acquire)
    }

    /// Update minimum vruntime
    fn update_min_vruntime(&mut self) {
        // Get minimum vruntime from queue
        if let Some((&key, _)) = self.tasks_timeline.iter().next() {
            let min_vruntime = self.min_vruntime.load(Ordering::Acquire);

            // min_vruntime only increases, ensure monotonic increase
            if key.vruntime > min_vruntime {
                self.min_vruntime.store(key.vruntime, Ordering::Release);
            }
        }
    }

    /// Enqueue task into run queue
    ///
    /// # Arguments
    /// - `task`: Task pointer to enqueue
    ///
    /// # Returns
    /// Returns true on success, false if task is already in queue
    pub fn enqueue(&mut self, task: *mut crate::process::Task) -> bool {
        self.enqueue_inner(task, false)
    }

    /// Enqueue a migrated task, preserving its vruntime.
    ///
    /// The task's vruntime is aligned to min_vruntime if it falls behind
    /// (Linux `place_entity` semantics), but is never reset forward.
    pub fn enqueue_migrate(&mut self, task: *mut crate::process::Task) -> bool {
        self.enqueue_inner(task, true)
    }

    fn enqueue_inner(&mut self, task: *mut crate::process::Task, migrate: bool) -> bool {
        if task.is_null() {
            return false;
        }

        unsafe {
            let task_ref = &mut *task;

            // Get scheduling entity
            let se = task_ref.sched_entity();

            // If task is already in run queue, don't enqueue again
            if se.is_on_rq() {
                return false;
            }

            // New task vruntime starts from min_vruntime
            // Migrated tasks keep their vruntime (aligned to min_vruntime if behind)
            let min_vruntime = self.get_min_vruntime();
            if !migrate {
                se.set_vruntime(min_vruntime);
            } else {
                // place_entity: if task's vruntime is far behind min_vruntime,
                // align it up to prevent it from monopolizing CPU time
                let vruntime = se.get_vruntime();
                if vruntime < min_vruntime {
                    se.set_vruntime(min_vruntime);
                }
            }

            // Generate unique key
            let task_id = self.next_task_id.fetch_add(1, Ordering::AcqRel);
            let key = VruntimeKey::new(se.get_vruntime(), task_id);

            // Add to BTreeMap
            self.tasks_timeline.insert(key, task);

            // Update status
            se.set_on_rq(true);

            // Update task count and total weight
            self.nr_running.fetch_add(1, Ordering::AcqRel);
            self.load_weight.fetch_add(se.load.weight, Ordering::AcqRel);

            // Update minimum vruntime
            self.update_min_vruntime();

            true
        }
    }

    /// Dequeue task from run queue
    ///
    /// # Arguments
    /// - `task`: Task pointer to dequeue
    ///
    /// # Returns
    /// Returns true on success
    pub fn dequeue(&mut self, task: *mut crate::process::Task) -> bool {
        if task.is_null() {
            return false;
        }

        unsafe {
            let task_ref = &mut *task;
            let se = task_ref.sched_entity();

            // Find and remove task
            let vruntime = se.get_vruntime();

            // Iterate to find matching task
            let mut found_key = None;
            for (&key, &ptr) in self.tasks_timeline.iter() {
                if ptr == task && key.vruntime == vruntime {
                    found_key = Some(key);
                    break;
                }
            }

            if let Some(key) = found_key {
                self.tasks_timeline.remove(&key);

                // Update status
                se.set_on_rq(false);

                // Update task count and total weight
                self.nr_running.fetch_sub(1, Ordering::AcqRel);
                self.load_weight.fetch_sub(se.load.weight, Ordering::AcqRel);

                // Update minimum vruntime
                self.update_min_vruntime();

                return true;
            }

            false
        }
    }

    /// Pick next task to run
    ///
    /// Select task with smallest vruntime
    ///
    /// # Returns
    /// Next task pointer to run, or None if queue is empty
    pub fn pick_next(&mut self) -> Option<*mut crate::process::Task> {
        // Get task with smallest vruntime
        if let Some((&key, &task)) = self.tasks_timeline.iter().next() {
            // Remove from queue using key (don't call dequeue, as vruntime may have changed)
            self.tasks_timeline.remove(&key);

            unsafe {
                // Update status
                let task_ref = &mut *task;
                let se = task_ref.sched_entity();
                se.set_on_rq(false);

                // Update task count and total weight
                self.nr_running.fetch_sub(1, Ordering::AcqRel);
                self.load_weight.fetch_sub(se.load.weight, Ordering::AcqRel);

                // Update minimum vruntime
                self.update_min_vruntime();
            }

            return Some(task);
        }

        None
    }

    /// Peek next task to run (without removing)
    ///
    /// Used to check next task without changing queue state
    pub fn peek_next(&self) -> Option<*mut crate::process::Task> {
        if let Some((&_key, &task)) = self.tasks_timeline.iter().next() {
            return Some(task);
        }
        None
    }

    /// Update current task runtime
    ///
    /// # Arguments
    /// - `now`: Current time (nanoseconds)
    pub fn update_curr(&mut self, now: u64) {
        if self.curr.is_null() {
            return;
        }

        unsafe {
            let task = &mut *self.curr;
            let se = task.sched_entity();

            // Update execution time
            let delta_exec = se.update_exec_runtime(now);

            if delta_exec > 0 {
                // Update virtual runtime
                se.update_vruntime(delta_exec);

                // Update minimum vruntime
                self.update_min_vruntime();
            }
        }
    }

    /// Calculate time slice
    ///
    /// Time slice = scheduling latency * process weight / total weight
    ///
    /// # Arguments
    /// - `se`: Scheduling entity
    ///
    /// # Returns
    /// Time slice (nanoseconds)
    pub fn sched_slice(&self, se: &SchedEntity) -> u64 {
        let nr_running = self.nr_running.load(Ordering::Acquire);

        if nr_running == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // Calculate scheduling period
        // If process count is high, use min_granularity * nr_running
        // Otherwise use fixed scheduling latency
        let sched_period = if nr_running > SCHED_LATENCY_NS / SCHED_MIN_GRANULARITY_NS {
            SCHED_MIN_GRANULARITY_NS * nr_running
        } else {
            SCHED_LATENCY_NS
        };

        // Calculate time slice
        // slice = period * weight / total_weight
        let load_weight = self.load_weight.load(Ordering::Acquire);

        if load_weight == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // Use multiplication to avoid division precision issues
        let slice = (sched_period * se.load.weight) / load_weight;

        // Ensure not less than minimum granularity
        slice.max(SCHED_MIN_GRANULARITY_NS)
    }

    /// Check if current task needs to be preempted
    ///
    /// # Arguments
    /// - `curr`: Current task
    /// - `se`: Newly woken task
    ///
    /// # Returns
    /// Returns true if preemption is needed
    pub fn check_preempt(&self, curr: &SchedEntity, se: &SchedEntity) -> bool {
        // If new task's vruntime is smaller than current task, should preempt
        let curr_vruntime = curr.get_vruntime();
        let se_vruntime = se.get_vruntime();

        // Use "wakeup granularity" as threshold
        // Only preempt if difference exceeds this value
        let wakeup_granularity = SCHED_MIN_GRANULARITY_NS;

        // Prevent vruntime wraparound
        if se_vruntime < curr_vruntime {
            let delta = curr_vruntime - se_vruntime;
            delta > wakeup_granularity
        } else {
            false
        }
    }

    /// Set currently running task
    pub fn set_curr(&mut self, task: *mut crate::process::Task) {
        self.curr = task;
    }

    /// Get currently running task
    #[inline]
    pub fn get_curr(&self) -> *mut crate::process::Task {
        self.curr
    }

    /// Clear run queue
    pub fn clear(&mut self) {
        // Mark all tasks as not in queue
        for (_, &task) in self.tasks_timeline.iter() {
            if !task.is_null() {
                unsafe {
                    let task_ref = &mut *task;
                    task_ref.sched_entity().set_on_rq(false);
                }
            }
        }

        self.tasks_timeline.clear();
        self.curr = core::ptr::null_mut();
        self.nr_running.store(0, Ordering::Release);
        self.load_weight.store(0, Ordering::Release);
    }
}

impl Default for CfsRunQueue {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for CfsRunQueue {}
unsafe impl Sync for CfsRunQueue {}

/// Calculate time slice (milliseconds)
///
/// Convert nanosecond time slice to milliseconds for timer interrupt
pub fn sched_slice_to_ms(slice_ns: u64) -> u32 {
    (slice_ns / 1_000_000) as u32
}

/// Convert milliseconds to nanoseconds
pub fn ms_to_ns(ms: u32) -> u64 {
    (ms as u64) * 1_000_000
}

/// Get current time (nanoseconds)
///
/// Use RISC-V time register
pub fn sched_clock() -> u64 {
    // Read time register
    let time: u64;
    unsafe {
        core::arch::asm!(
            "rdtime {time}",
            time = out(reg) time,
            options(nomem, nostack)
        );
    }
    // Assume clock frequency is 10MHz (100ns precision)
    // Actual value needs adjustment based on platform
    time * 100
}

// ============================================================================
// Fair Scheduling Class Implementation
// ============================================================================

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
                se.load.weight = WEIGHT_IDLEPRIO;
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
                se.set_vruntime(vruntime + SCHED_MIN_GRANULARITY_NS);
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
            let now = sched_clock();
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
                    let slice_ms = sched_slice_to_ms(slice_ns);
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
            let now = sched_clock();
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

    fn select_task_rq(&self, task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        if task.is_null() {
            return cpu;
        }

        unsafe {
            let task_ref = &*task;
            let cpus_allowed = task_ref.cpus_allowed();

            // Wake-affine: if wake_cpu is idle and allowed, prefer it for cache warmth
            let wake_cpu = task_ref.wake_cpu();
            if wake_cpu >= 0 {
                let wake = wake_cpu as usize;
                if wake < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << wake)) != 0 {
                    if let Some(rq) = super::cpu_rq(wake) {
                        let rq = rq.lock();
                        // Only prefer wake_cpu if it's idle (only idle task running)
                        let load = super::sched::rq_load(&*rq);
                        if load == 0 {
                            return wake_cpu;
                        }
                    }
                }
            }

            // Fallback: find least-loaded CPU in cpus_allowed
            let mut best_cpu = cpu;
            let mut best_load = usize::MAX;

            for c in 0..crate::config::MAX_CPUS {
                if (cpus_allowed & (1u32 << c)) == 0 {
                    continue;
                }
                if let Some(rq) = super::cpu_rq(c) {
                    let load = super::sched::rq_load(&*rq.lock());
                    if load < best_load {
                        best_load = load;
                        best_cpu = c as i32;
                    }
                }
            }

            best_cpu
        }
    }

    fn task_tick(&self, rq: RunQueueRef, task: *mut Task, queued: bool) {
        if rq.is_null() || task.is_null() {
            return;
        }

        unsafe {
            let rq = &mut *rq;

            // Update current task's runtime
            let now = sched_clock();
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
            let now = sched_clock();
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
            sched_slice_to_ms(slice_ns).max(1)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_weight() {
        // nice = 0 weight should be 1024
        let lw = LoadWeight::from_nice(0);
        assert_eq!(lw.weight, 1024);

        // nice = -20 weight should be largest
        let lw_high = LoadWeight::from_nice(-20);
        assert!(lw_high.weight > lw.weight);

        // nice = 19 weight should be smallest
        let lw_low = LoadWeight::from_nice(19);
        assert!(lw_low.weight < lw.weight);
    }

    #[test]
    fn test_vruntime_calculation() {
        let se = SchedEntity::new();

        // When nice = 0, vruntime should equal actual runtime
        let delta = 1_000_000; // 1ms
        let vruntime = se.calc_delta_fair(delta);
        assert_eq!(vruntime, delta);
    }

    #[test]
    fn test_cfs_rq_enqueue_dequeue() {
        let mut rq = CfsRunQueue::new();

        // Create test task structure
        // Note: Actual testing requires valid Task pointers
        assert!(rq.is_empty());
        assert_eq!(rq.nr_running(), 0);
    }
}
