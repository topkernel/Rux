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
    pub fn update_vruntime(&self, delta_exec: u64) -> u64 {
        let delta_vruntime = self.calc_delta_fair(delta_exec);
        self.add_vruntime(delta_vruntime);
        delta_vruntime
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
    /// preserving vruntime, but is never reset forward.
    pub fn enqueue_migrate(&mut self, task: *mut crate::process::Task) -> bool {
        self.enqueue_inner(task, true)
    }

    fn enqueue_inner(&mut self, task: *mut crate::process::Task, migrate: bool) -> bool {
        if task.is_null() {
            return false;
        }

        // SAFETY: caller guarantees task is a valid pointer to a Task; null check above;
        // we only mutate the task's sched_entity fields which are owned by the scheduler.
        unsafe {
            let task_ref = &mut *task;

            // Get scheduling entity
            let se = task_ref.sched_entity();

            // If task is already in run queue, don't enqueue again
            if se.is_on_rq() {
                return false;
            }

            // Align vruntime to min_vruntime if it falls behind.
            // For yielding tasks (migrate=false), keep their vruntime so they
            // don't regain priority over tasks that haven't run yet.
            // For migrated tasks, same treatment — preserve vruntime, only
            // bump up to min_vruntime if behind.
            let min_vruntime = self.get_min_vruntime();
            let vruntime = se.get_vruntime();
            if vruntime < min_vruntime {
                se.set_vruntime(min_vruntime);
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

        // SAFETY: task is a valid pointer from the CFS queue (was enqueued earlier);
        // null check above; we only access sched_entity fields.
        unsafe {
            let task_ref = &mut *task;
            let se = task_ref.sched_entity();

            // Find and remove task by pointer only.
            // We do NOT match vruntime because update_curr() may have
            // changed the sched_entity's vruntime after the task was
            // enqueued, making the BTreeMap key's vruntime stale.
            let mut found_key = None;
            for (&key, &ptr) in self.tasks_timeline.iter() {
                if ptr == task {
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

            // SAFETY: task was stored in tasks_timeline from a valid &mut Task pointer;
            // it was just removed from the queue so no aliasing references exist.
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

    /// Pick the leftmost (min-vruntime) task that is allowed to run on `cpu_id`.
    /// Scans from left to right, skipping tasks that cannot run on this CPU.
    ///
    /// Temporarily removes non-matching entries to inspect subsequent ones,
    /// then re-inserts them. CFS task counts are typically reasonable so this
    /// works fine without allocating.
    pub fn pick_next_cpu(&mut self, cpu_id: usize) -> Option<*mut crate::process::Task> {
        // Heap buffer for skipped entries — tasks that fail CPU affinity
        // check are stashed and re-inserted after the scan completes.
        // Uses Vec instead of a large stack array to avoid kernel stack overflow
        // (pick_next_cpu can be deep in the scheduler call chain).
        let mut skipped: alloc::vec::Vec<(VruntimeKey, *mut crate::process::Task)> =
            alloc::vec::Vec::with_capacity(256);
        let mut skip_count = 0usize;
        let mut result = None;

        loop {
            let key = match self.tasks_timeline.iter().next() {
                Some((&k, _)) => k,
                None => break,
            };
            let task = match self.tasks_timeline.get(&key) {
                Some(&t) => t,
                None => break,
            };

            // SAFETY: task is from the BTreeMap, a valid pointer stored during enqueue.
            let allowed = unsafe { (*task).cpu_allowed(cpu_id) };
            if allowed {
                // Found a match — remove and return it
                self.tasks_timeline.remove(&key);

                // SAFETY: task was just removed from the queue; no aliasing references exist.
                unsafe {
                    let task_ref = &mut *task;
                    let se = task_ref.sched_entity();
                    se.set_on_rq(false);

                    self.nr_running.fetch_sub(1, Ordering::AcqRel);
                    self.load_weight.fetch_sub(se.load.weight, Ordering::AcqRel);
                    self.update_min_vruntime();
                }

                result = Some(task);
                break;
            }

            // Not allowed — remove temporarily and stash
            self.tasks_timeline.remove(&key);
            if skip_count < skipped.capacity() {
                skipped.push((key, task));
                skip_count += 1;
            } else {
                // Overflow — re-insert and give up (very unlikely)
                self.tasks_timeline.insert(key, task);
                break;
            }
        }

        // Re-insert all skipped entries
        for i in (0..skip_count).rev() {
            let (key, task) = skipped[i];
            self.tasks_timeline.insert(key, task);
        }

        result
    }

    /// Update current task runtime
    ///
    /// # Arguments
    /// - `now`: Current time (nanoseconds)
    pub fn update_curr(&mut self, now: u64) {
        if self.curr.is_null() {
            return;
        }

        // SAFETY: self.curr is set by set_curr() to a valid Task pointer; null check above.
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

        // Rearrange to (period / total_weight) * weight to avoid overflow.
        // This loses some precision from integer truncation but produces the
        // correct order of magnitude and never exceeds sched_period.
        let slice = sched_period / load_weight * se.load.weight;

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
                // SAFETY: task is from the queue, a valid pointer; clear() drains the entire queue.
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

/// Timebase multiplier: QEMU uses 10MHz timebase, convert to nanoseconds
const TIMEBASE_MULT: u64 = 100;

/// Get current time (nanoseconds)
///
/// Use RISC-V time register
pub fn sched_clock() -> u64 {
    // SAFETY: rdtime reads the RISC-V time CSR, a read-only hardware register.
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
    time * TIMEBASE_MULT
}

// ============================================================================
// Fair Scheduling Class Implementation
// ============================================================================

/// Fair scheduling class - wraps CFS
///
/// With the global RunQueue design, the actual CFS enqueue/dequeue/pick
/// happens directly in sched.rs on GlobalRunQueue::cfs_rq. The SchedClass
/// methods are simplified to no-ops or minimal stubs.
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

    /// Actual enqueue happens in sched.rs::enqueue_task_locked.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual dequeue happens in sched.rs::dequeue_task.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// Actual yield handling happens in sched.rs.
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Preemption checks happen in sched.rs.
    fn wakeup_preempt(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual pick happens in sched.rs::pick_next_task.
    fn pick_next_task(&self, _rq: RunQueueRef, _prev: *mut Task) -> *mut Task {
        core::ptr::null_mut()
    }

    /// Actual re-enqueue happens in sched.rs::__schedule.
    fn put_prev_task(&self, _rq: RunQueueRef, _prev: *mut Task, _next: *mut Task) {
    }

    /// Minimal — actual setup happens in sched.rs.
    fn set_next_task(&self, _rq: RunQueueRef, _next: *mut Task, _first: bool) {
    }

    /// CFS load balancing is handled separately
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// CPU selection — simplified for global RQ.
    /// The global queue is inherently balanced; just keep on the current CPU.
    fn select_task_rq(&self, task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        if task.is_null() {
            return cpu;
        }

        // With global RQ, no per-CPU load balancing needed.
        // Just return the preferred CPU from the task's affinity.
        // SAFETY: task is a valid pointer from the caller; null check above.
        unsafe {
            let task_ref = &*task;
            let cpus_allowed = task_ref.cpus_allowed();
            let wake_cpu = task_ref.wake_cpu();

            // Prefer wake_cpu if allowed
            if wake_cpu >= 0 {
                let wake = wake_cpu as usize;
                if wake < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << wake)) != 0 {
                    return wake_cpu;
                }
            }

            // Check if current cpu is allowed
            let cpu_usize = cpu as usize;
            if cpu_usize < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << cpu_usize)) != 0 {
                return cpu;
            }

            // Fallback: first allowed CPU
            for c in 0..crate::config::MAX_CPUS {
                if (cpus_allowed & (1u32 << c)) != 0 {
                    return c as i32;
                }
            }

            cpu
        }
    }

    /// Actual tick handling happens in sched.rs::scheduler_tick.
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// Actual runtime tracking happens in sched.rs.
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, task: *mut Task) -> u32 {
        if task.is_null() {
            return 1;
        }

        // Return a reasonable default; actual slice is computed in sched.rs.
        1
    }

    /// Actual check happens in sched.rs via GlobalRunQueue::cfs_rq.
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        false
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
