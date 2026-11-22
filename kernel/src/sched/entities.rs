//! The scheduling ENTITY STORAGE (crate-flip S1b): the per-task
//! scheduling entities and the load-weight math live FRAMEWORK-side
//! because Task embeds them by value (storage follows the owner — the
//! reference kernel's sched_entities inside task_struct). The policy
//! crate manipulates them through their pub fields and methods; the
//! decision logic stays policy-side.
use core::sync::atomic::{AtomicU64, AtomicBool, AtomicU32, AtomicI64, Ordering};
use super::edges::{VruntimeKey, RbNode, RbTimeline};
use crate::process::task::Task;
use crate::config::MAX_CPUS;

/// Weight when nice value is 0
pub const NICE_0_LOAD: u64 = 1024;

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
    ///
    /// The inverse weight is computed here (2^32 / weight, the reference
    /// kernel's `__update_inv_weight`) so the vruntime update path never
    /// has to divide.
    pub fn new(weight: u64) -> Self {
        let mut lw = Self {
            weight,
            inv_weight: 0,
        };
        lw.update_inv_weight();
        lw
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

    /// Leg-55 GRQ slimming: the exact timeline key this entity is linked
    /// under, while linked. Set at enqueue_inner (the key's birthplace),
    /// cleared at every removal (dequeue/pick_next/pick_next_cpu).
    /// GRQ-HELD access only — the timeline is GRQ-protected, so the key
    /// shares its discipline (UnsafeCell, not atomic: no lockless reader).
    /// grq-10ns: with the intrusive timeline this key is the ORDER key
    /// only — the authoritative linkage is the embedded `run_node`'s
    /// `linked` flag; erase never needs the key (the node's position IS
    /// the linkage).
    pub(crate) timeline_key: core::cell::Cell<Option<VruntimeKey>>,
    /// grq-10ns: the entity-embedded red-black link (the reference
    /// kernel's sched_entity rb_node). While `linked`, parent/left/right
    /// name the timeline structure; insert/erase are pointer rewires with
    /// ZERO allocation. GRQ-HELD discipline (same as timeline_key).
    pub(crate) run_node: RbNode,
    /// 64th (EEVDF wake placement, 6.6 place_entity parity): the clamped
    /// VIRTUAL lag captured at the last sleep-dequeue (update_entity_lag,
    /// 6.6 fair.c:791: vlag = clamp(V - vruntime, +/- calc_delta_fair(
    /// max(2*slice, TICK_NSEC)))). Read once, at the next WAKE enqueue, to
    /// place the entity at V - vlag*(load+w)/load -- the property that
    /// keeps producer threads (FFRT wheel/workers) from ever sitting
    /// hundreds of ms behind a low-duty wake-churn pack (ohw34: a 400ms
    /// link->pick on a worker BEFORE the wedge; the old placement preserved
    /// unbounded vruntime debt). GRQ-HELD (Cell): no lockless reader.
    pub vlag: core::cell::Cell<i64>,
    /// 64th: false until this entity's first-ever link -- discriminates the
    /// 6.6 ENQUEUE_INITIAL placement (vruntime = avg_vruntime, lag zero)
    /// from a WAKE placement. GRQ-HELD. Fork children start false (fresh
    /// SchedEntity::new()), matching __sched_fork's vlag = 0.
    pub ever_enqueued: core::cell::Cell<bool>,
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
            timeline_key: core::cell::Cell::new(None),
            run_node: RbNode::unlinked(),
            vlag: core::cell::Cell::new(0),
            ever_enqueued: core::cell::Cell::new(false),
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
            timeline_key: core::cell::Cell::new(None),
            run_node: RbNode::unlinked(),
            vlag: core::cell::Cell::new(0),
            ever_enqueued: core::cell::Cell::new(false),
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
    /// vruntime += delta_exec * (NICE_0_LOAD / process weight)
    ///
    /// The reference kernel's form (kernel/sched/fair.c: __calc_delta) is
    /// `delta_exec * weight / lw.weight` computed as
    /// `(delta_exec * (weight * lw->inv_weight)) >> 32`, with
    /// weight = NICE_0_LOAD and lw->inv_weight = 2^32 / the entity's
    /// weight — i.e. the same `delta_exec * NICE_0_LOAD / weight`.
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

        // Use multiplication instead of division:
        //   delta = delta_exec * NICE_0_LOAD / weight
        //         = (delta_exec * (NICE_0_LOAD * inv_weight)) >> 32
        //           [inv_weight = 2^32 / weight]
        //
        // Use u128 to prevent overflow when delta_exec is large.
        // Matches kernel/sched/fair.c __calc_delta() semantics.
        let mut load = self.load;
        load.update_inv_weight();

        let fact = (NICE_0_LOAD as u128)
            .wrapping_mul(load.inv_weight as u128);
        ((delta_exec as u128 * fact) >> 32) as u64
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

    /// Leg-55: the timeline key this entity is linked under (None when
    /// unlinked). GRQ-HELD access only — see the field comment.
    #[inline]
    pub fn timeline_key(&self) -> Option<VruntimeKey> {
        // Cell<Option<Copy>>: the field comment's GRQ-held discipline is
        // unchanged; Cell's get/set are safe for a Copy option.
        self.timeline_key.get()
    }

    /// Leg-55: GRQ-held key update (enqueue stores it, removal clears it).
    #[inline]
    pub fn set_timeline_key(&self, key: Option<VruntimeKey>) {
        self.timeline_key.set(key);
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
            timeline_key: core::cell::Cell::new(None),
            run_node: RbNode::unlinked(),
            // 64th: fork parity with __sched_fork -- the child starts with
            // zero lag and takes the ENQUEUE_INITIAL placement at its
            // first link, not the parent's preserved wake lag.
            vlag: core::cell::Cell::new(0),
            ever_enqueued: core::cell::Cell::new(false),
        }
    }
}

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
            time_slice: AtomicU32::new(RR_TIMESLICE_TICKS),
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
        self.time_slice.store(RR_TIMESLICE_TICKS, Ordering::Release);
    }
}
impl Default for SchedRtEntity {
    fn default() -> Self {
        Self::new()
    }
}

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

    /// Kernel timer ID armed to replenish this entity at its deadline
    /// (0 = no timer pending). See sched.rs::arm_dl_replenish_timer —
    /// a throttled task sits off the runqueue and is re-enqueued (with a
    /// refilled budget) only by this timer's wake.
    pub replenish_timer: AtomicU64,

    /// Whether on runqueue
    pub on_rq: AtomicBool,

    /// Whether boosted (inheriting priority)
    pub dl_boosted: AtomicBool,

    /// Timestamp (ns) when this entity last started executing.
    /// Used by update_curr() to calculate actual runtime consumed.
    pub exec_start: AtomicU64,

    /// The RELATIVE deadline value from the user's sched_attr
    /// (sched_setattr), kept verbatim for sched_getattr readback. The
    /// scheduler's own absolute `deadline` above is internal state that
    /// moves with every replenishment and must not leak to user space.
    pub attr_deadline: AtomicU64,
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
            replenish_timer: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            dl_boosted: AtomicBool::new(false),
            exec_start: AtomicU64::new(0),
            attr_deadline: AtomicU64::new(0),
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

    /// Replenish runtime and advance deadline (CBS replenishment)
    ///
    /// Called ONLY when a full period has been served out — i.e. from
    /// `enqueue_task_locked` when the task's throttle window has expired
    /// (the replenish timer's wake, or a first-ever enqueue). CBS is defeated
    /// if this runs on every re-enqueue: the old code replenished on each
    /// __schedule requeue of prev, so a single DL task could burn 100% of
    /// every CPU (review batch 8, HIGH).
    pub fn replenish_runtime(&self) {
        let runtime = self.dl_runtime.load(Ordering::Acquire) as i64;
        self.runtime.store(runtime, Ordering::Release);
        // Advance deadline: now + period (CBS requirement)
        let now = crate::sched::fair::sched_clock();
        let period = self.dl_period.load(Ordering::Acquire);
        self.deadline.store(now + period, Ordering::Release);
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

pub const PRIO_TO_WMULT: [u64; 40] = [
    /* -20 */ 48388, 59856, 76040, 92818, 118348,
    /* -15 */ 147320, 184698, 229616, 287308, 360437,
    /* -10 */ 449829, 563644, 704093, 875809, 1099582,
    /*  -5 */ 1376151, 1717300, 2157191, 2708050, 3363326,
    /*   0 */ 4194304, 5237765, 6557202, 8165337, 10153587,
    /*   5 */ 12820798, 15790321, 19976592, 24970740, 31350126,
    /*  10 */ 39045157, 49367440, 61356676, 76695844, 95443717,
    /*  15 */ 119304647, 148102320, 186737708, 238609294, 286331153,
];

/// Default RR time slice in ticks. 100ms / (1000ms / KERNEL_HZ) = 10 ticks at HZ=100.
pub const RR_TIMESLICE_TICKS: u32 = (100 * crate::config::KERNEL_HZ) / 1000;
/// Default runtime in nanoseconds (100ms)
pub const DL_DEFAULT_RUNTIME_NS: u64 = 100_000_000;
/// Default deadline period in nanoseconds (1 second)
pub const DL_DEFAULT_PERIOD_NS: u64 = 1_000_000_000;
/// Deadline bandwidth unit (1 << 20 for precision)
pub const DL_BW_UNIT: u64 = 1 << 20;
