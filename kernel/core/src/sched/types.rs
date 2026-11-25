//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The sched subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct`, `enum` and type alias of the
//! subsystem's topic files lives here, together with the constants those
//! definitions name (array lengths, and the constants those derive from);
//! every `impl` block stays in the topic file beside the functions that
//! use it — an impl is code, a definition is data layout. The audited
//! `unsafe.rs` keeps the types its own contracts are written about. The
//! exceptions are definitions local to code: a type declared inside a
//! function or inside a private inline module.
//!
//! This file holds no `unsafe`. A field that was private to its old
//! topic file is `pub(super)` here because the code that touches it
//! stayed in that file: the sched directory is the smallest visibility
//! boundary Rust offers for that split.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64};

use crate::list::ListHead;
use crate::process::task::Task;
use crate::sync::spinlock::Spinlock;

// the trait the two class iterators name stays with the class code (class.rs
// in step 1, hotpath.rs after step 2)
use super::class::SchedClass;
use super::r#unsafe::{RbNode, RbTimeline, VruntimeKey};


// ---------------------------------------------------------------------------
// the scheduling entities, the load weight, the class run queues (CFS, RT, deadline)
// and the RT priority bound RtRunQueue's queue array names
// ---------------------------------------------------------------------------

/// CFS run queue
pub struct CfsRunQueue {
    /// Task timeline sorted by vruntime
    ///
    /// grq-10ns: entity-embedded intrusive red-black tree (the reference
    /// kernel's cfs_rq tasks_timeline / rb_root_cached). The nodes live
    /// inside each task's SchedEntity, so insert/erase under the GRQ are
    /// pointer rewires — the BTreeMap's allocator-backed split/merge is
    /// gone from the critical section.
    pub timeline: RbTimeline,

    /// Currently running scheduling entity
    pub curr: *mut crate::process::Task,

    /// Minimum vruntime in queue
    ///
    /// Used for new task vruntime initialization
    /// New task vruntime = min_vruntime, this prevents new tasks from getting too much CPU time
    pub min_vruntime: AtomicU64,

    /// Number of tasks in run queue
    pub(super) nr_running: AtomicU64,

    /// Total weight
    pub load_weight: AtomicU64,

    /// Next task ID (for generating unique keys)
    pub(super) next_task_id: AtomicU64,
    /// 64th (EEVDF avg_vruntime, 6.6 fair.c:706-739 parity): Sum of
    /// (vruntime_i - min_vruntime)*w_i over the QUEUED entities -- the
    /// RELATIVE form the reference derives explicitly ("since v_i is u64,
    /// and the multiplication could easily overflow, transform it into a
    /// relative form"). Maintained O(1) at timeline insert/remove; REBASED
    /// by avg_vruntime_update (sum -= delta*load) on every min_vruntime
    /// advance so every stored offset stays relative to the CURRENT
    /// min_vruntime. The 63rd's first attempt stored ABSOLUTE
    /// vruntime*weight sums and still added min_vruntime in the read --
    /// V ~ min_vr + true average, the placed values fed back
    /// multiplicatively (~2x per wake generation), wrapped u64 and froze
    /// garbage vruntimes within a minute (ohw36: init vr=0xfd79.. frozen
    /// unpicked from t=53s). The relative form is also leak-bounded: any
    /// unpaired insert/remove residue is (vr - min_vr)*w -- the size of
    /// the clamp window, never boot-magnitude. GRQ-HELD. i128: offsets
    /// are small but weight-scaled products of a deep queue still fit
    /// trivially.
    pub(super) avg_vruntime_sum: i128,
}

/// One row of the strand-wall probe (CfsRunQueue::strand_wall_probe):
/// a timeline entry that sorts before a stranded victim — a pick-order
/// predecessor whose refusal every pick must digest before the victim
/// can be reached.
#[derive(Clone, Copy)]
pub struct StrandWallRow {
    pub vruntime: u64,
    pub pid: u32,
    /// SchedPolicy as u8 (0 Normal, 1 Fifo, 2 Rr, 3 Batch, 5 Idle, 6 Deadline)
    pub policy: u64,
    pub on_cpu: bool,
    pub claim: i32,
    pub aff: u32,
    /// 61st: the entry's pick-walk refusal history — a wall of entries
    /// with growing skip counts explains a victim deeper in the tree
    /// that is never even VISITED (the 32-slot stash never gets past a
    /// permanently-refused left edge; no overflow fires because the
    /// pick succeeds at position <= 33).
    pub skips: u32,
    pub why: u32,
    /// 63rd: which CPU slot names this entry at probe time (-1 = none)
    /// — a wall entry that is BOTH linked and on a slot is the R56d
    /// lying-slot / mid-switch-out shape, the refusal input the 62nd
    /// asked to name.
    pub slot: i32,
    /// 63rd: jiffies since this entry's last pick (u32::MAX = never
    /// picked since boot) — distinguishes the never-picked wheel class
    /// from the cycling churn.
    pub lp_jif: u32,
    /// 63rd: times this entry was linked (woken) since boot — the churn
    /// rate numerator.
    pub wakes: u32,
}

/// Maximum RT priority (0-99, HIGHER value = higher priority, matching
/// Linux). The runqueue bitmap index is inverted (bit 0 = priority 99) so
/// the lowest set bit is the highest-priority runnable task.
pub const MAX_RT_PRIO: usize = 100;

/// RT runqueue
pub struct RtRunQueue {
    /// Number of RT tasks
    pub rt_nr_running: AtomicU32,

    /// Number of RR tasks
    ///
    /// Maintained by enqueue/dequeue but not yet read anywhere (reserved for
    /// /proc/schedstat and RR throttling diagnostics) — kept accurate so a
    /// future reader does not inherit drift.
    pub rr_nr_running: AtomicU32,

    /// Highest priority in queue
    pub highest_prio: AtomicU32,

    /// Priority bitmap: bit i is set if priority i has runnable tasks
    /// We need 100 bits, so use 2 x 64-bit words
    pub bitmap: [AtomicU64; 2],

    /// Per-priority task lists
    ///
    /// Each list head anchors tasks at that priority level
    pub queue: [ListHead; MAX_RT_PRIO],
}

/// Load weight
#[derive(Debug, Clone, Copy)]
pub struct LoadWeight {
    /// Weight value
    pub weight: u64,
    /// Weight multiplier (for fast division)
    pub inv_weight: u64,
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

/// r24 (kill10 wedge hunt): discriminator census for the W2
/// WAKE-REFUSED-SLEEPING report — captured under the GRQ lock so the
/// line tells "linked but starved" from "orphaned live pick mark" from
/// "lying busy slot". Probe-lock discipline: this helper ONLY reads and
/// formats; the caller prints after the guard is dropped.
#[derive(Default)]
pub struct W2RefusalCensus {
    pub linked: bool,
    pub on_rq: bool,
    pub busy_slot: i32,
    pub idle_slot: i32,
    pub exec_ns: u64,
    pub jiffies: u64,
    pub quantum_ms: u32,
}

/// Deadline runqueue
pub struct DlRunQueue {
    /// Tasks sorted by deadline (earliest first)
    ///
    /// Key: (deadline, task_id)
    /// Value: Task pointer
    pub(super) tasks: BTreeMap<DlKey, *mut Task>,

    /// Number of DL tasks
    pub dl_nr_running: AtomicU32,

    /// Earliest deadline in queue
    pub earliest_dl: AtomicU64,

    /// Next task ID for unique keys
    pub(super) next_id: AtomicU64,
}

/// Key for deadline BTreeMap
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DlKey {
    pub(super) deadline: u64,
    pub(super) task_id: u64,
}

/// grq-10ns: poison-discipline trip counters for the intrusive timeline
/// (an insert descent or erase neighbor that failed the kernel-range
/// check — the op was refused to keep the kernel alive). Reported by
/// grq_diag_counts; a persistent nonzero rate names a scribble producer.
pub struct TlPoisonCounters {
    pub inserts: core::sync::atomic::AtomicU64,
    pub erases: core::sync::atomic::AtomicU64,
}

// ---------------------------------------------------------------------------
// the async-reap drainer's claim and its pending entry
// ---------------------------------------------------------------------------

/// ReapClaim — the linearly-owned storage-free obligation for one
/// unreachable task (Round-B pilot; the design page in run/UR-L1.md).
///
/// **MINT** — [`ReapClaim::mint`], exactly once per reap, from
/// `release_task` AFTER pid_hash_remove + tree detach (the
/// unreachability contract): from that moment nobody else can or will
/// free this task's storage — the claim IS that obligation as a type.
///
/// **BURN** — [`ReapClaim::into_free`], consumes `self`, runs the
/// storage-free window. Exactly-once is compiler-enforced: the claim
/// is not `Clone`/`Copy` and `into_free` takes it by value.
///
/// **READS** — the ladder's checks go through [`ReapClaim::task`]
/// (shared) and [`ReapClaim::as_ptr`] (the identity comparisons
/// against per-CPU slots): no raw task pointer escapes this module.
///
/// **DROP** — the over-cap leak path (explicit, counted in
/// [`CLAIM_DROPPED`]): a dropped claim leaks the Task page loudly,
/// exactly the pre-pilot over-cap semantics.
///
/// The page's own lifecycle (alloc/poison/dealloc, INV-TL-1/2) is
/// UNCHANGED — the claim types the free-obligation, not the page.
/// Identity preservation: `as_ptr()` returns the minted pointer,
/// always; the slot comparison and `into_free`'s body are
/// byte-identical to the raw-pointer form they replace (the
/// corruption-battery obligation).
pub struct ReapClaim(pub(super) core::ptr::NonNull<Task>);

pub(super) struct PendingReap {
    pub(super) claim: ReapClaim,
    /// The parent's pid at reap time (boot-118's WAIT4-PARK fix: a
    /// parent parked in wait4 on THIS child gets a deferred-notify
    /// wake when the drainer completes the child's storage — the
    /// reaper-side notify already ran in do_exit, but a racing park
    /// could have missed it; this is the belt-and-braces arm, 0 = no
    /// parent known). Captured at MINT (release_task still knows the
    /// tree), consumed at BURN.
    pub(super) parent_pid: u32,
    /// Consecutive ticks the entry has held the R85 conjunction.
    pub(super) r85_ticks: u32,
    /// Past the switch stage, waiting for the batch grace target.
    pub(super) grace: bool,
    /// P0 exit-reap (per-entry grace bound): the FIRST generation
    /// OPENED AFTER this entry's pid_hash removal (RCU_GEN at mint +
    /// 1). The entry frees only when the grace watermark reaches it —
    /// a report at generation G can only have been taken at a round
    /// opened after the removal, which is exactly the coverage the
    /// free needs (the synchronize_rcu_forced discipline). The old
    /// whole-batch target could not give this guarantee to entries
    /// that minted WHILE a round was already open: their removals
    /// postdate the target, and a CPU whose last report predates the
    /// removal could still satisfy "reported >= target" — a free
    /// under a live pid_hash reader.
    pub(super) needs: u32,
}

// ---------------------------------------------------------------------------
// the global run queue, its two guards, the per-CPU state, the pending
// diagnostic report and the compatibility RunQueue
// ---------------------------------------------------------------------------

/// Global run queue — one instance for the whole system.
///
/// Protected by a single `RawSpinlock`.  All enqueue / dequeue / pick_next
/// operations must hold this lock.  Timer-tick updates (`scheduler_tick`)
/// only touch the per-CPU `current` task and do NOT need the lock.
///
/// NOTE (2b092c49 withdrawal, 2026-10-29; mechanism corrected same day
/// by the refactor lane, 75dbf637): this lock was briefly a
/// `KeyedRawSpinlock<GrqKey>` (§3 typed lock keys pilot). That swap is
/// CONVICTED as the trigger of the d2a58ccb stochastic panic family by
/// a 13-boot paired adjudication (identical tree differing only by the
/// swap): WITH the swap, 6/6 x86 exitwalk boots panic in the CFS
/// timeline btree walk (navigate.rs:552 next_unchecked unwrap, stack:
/// pick_next_cpu → Iter::next) and the riscv64 orphan-storm churn
/// panics on a torn-context wild epc right after an R74-DOUBLE-SEED;
/// WITHOUT it (revert arm + all pre-swap kernels), 7/7 boots run the
/// full churn clean on both arches.
///
/// TRUE ROOT CAUSE (75dbf637 — the initial "latent GRQ race" theory
/// was WRONG): `RawSpinlock::lock()` returns `()` — the statement IS
/// the acquire — while `KeyedRawSpinlock::lock()` returns a guard, and
/// the swap kept the bare `self.lock.lock();` statement: the guard was
/// DISCARDED at the end of the statement and its Drop released the
/// lock immediately, so EVERY GRQ critical section ran unexcluded.
/// The R74-DOUBLE-SEED shape was the expected downstream of a lock
/// that never held. There is NO latent GRQ pick/ownership race; the
/// protocol is exonerated. The guard class is now `#[must_use]` (the
/// discard no longer compiles silently) and protocol-wrapping
/// subsystems use `lock_claim()`; a corrected re-adoption is staged
/// with its own fresh paired adjudication. This struct stays on
/// RawSpinlock until that lands.
pub struct GlobalRunQueue {
    /// Protects dl_rq, rt_rq, cfs_rq, nr_running
    pub(super) lock: crate::sync::spinlock::RawSpinlock,
    /// Deadline queue (EDF sorted by deadline)
    pub dl_rq: crate::sched::entities::DlRunQueue,
    /// Real-time queue (bitmap + per-priority lists)
    pub rt_rq: crate::sched::entities::RtRunQueue,
    /// CFS queue (BTreeMap sorted by vruntime)
    pub cfs_rq: crate::sched::entities::CfsRunQueue,
    /// Total runnable task count (read atomically for idle checks)
    pub nr_running: core::sync::atomic::AtomicUsize,
    /// Bitmap of idle CPUs: bit N = 1 means CPU N is idle
    pub idle_cpus: core::sync::atomic::AtomicU32,
}

/// Guard for `lock_irqsave()` — unlock + preempt enable + IRQ restore on drop.
pub struct GrqGuard<'a> {
    pub(super) grq: *mut GlobalRunQueue,
    pub(super) flags: bool,
    pub(super) _marker: core::marker::PhantomData<&'a GlobalRunQueue>,
}

/// Guard for plain `lock_plain()` (no IRQ save).
pub struct GrqPlainGuard<'a> {
    pub(super) grq: *mut GlobalRunQueue,
    pub(super) _marker: core::marker::PhantomData<&'a GlobalRunQueue>,
}

/// Minimal per-CPU state — no queues, just task pointers.
pub struct PerCpuState {
    /// Currently running task
    pub current: *mut Task,
    /// Per-CPU idle task (PID 0)
    pub idle: *mut Task,
    /// Per-CPU stop task (for hotplug)
    pub stop: *mut Task,
}

/// A diagnostic report CAPTURED under the GRQ and EMITTED after the lock
/// is released (grq-lock-1st).
///
/// printk is not lock-free: its ring-buffer write ends in
/// `KMSG_READ_WQ.wake_up_all()`, whose waitqueue lock is held across
/// `wake_up_process` -> `wake_up_enqueue_opt` -> `grq().lock_irqsave()`.
/// A report issued while the GRQ is held therefore re-acquires the GRQ
/// on the same cpu — the non-reentrant spinlock then spins forever with
/// IRQs off (the "IRQs-dead GRQ holder" wedge) — and against another cpu
/// it is the textbook ABAA (this cpu: GRQ -> WQ; the other: WQ -> GRQ).
/// The capture-then-emit shape keeps every field's value exact (they are
/// read under the lock) while moving the report itself out of the
/// critical section.
#[derive(Clone, Copy)]
pub struct GrqPendingReport {
    /// Which report this is (see the `GRQ_RPT_*` constants).
    pub kind: u8,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub d: u64,
    pub e: u64,
    pub comm: [u8; 16],
}

/// Dummy RunQueue for compatibility with SchedClass trait / procfs output.
pub struct RunQueue;

// ---------------------------------------------------------------------------
// the scheduling class id, the two class iterators and the run-queue pointer alias
// ---------------------------------------------------------------------------

/// RunQueue type alias (forward declaration)
/// The actual struct is defined in sched.rs
pub type RunQueueRef = *mut crate::sched::sched::RunQueue;

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

/// Iterate through scheduling classes in priority order (highest to lowest)
pub struct SchedClassIter {
    pub(super) current: Option<&'static dyn SchedClass>,
}

/// Iterate through scheduling classes from a starting class
pub struct SchedClassFromIter {
    pub(super) current: Option<&'static dyn SchedClass>,
}

// ---------------------------------------------------------------------------
// the cgroup v2 node and its controller configuration
// ---------------------------------------------------------------------------

/// Per-cgroup controller configuration.
///
/// `subtree_control` is the cgroup.subtree_control bitset (which controllers
/// are enabled FOR THE CHILDREN). The limit knobs are atomics so the timer
/// tick can read them without the enclosing Spinlock; the Spinlock still
/// serializes multi-field updates (e.g. cpu.max quota+period pairs).
pub struct CgroupControllers {
    /// Controllers enabled for children via cgroup.subtree_control.
    pub subtree_control: u32,

    /// cpu controller: bandwidth quota per period (ns; LIMIT_MAX = max).
    pub cpu_quota_ns: AtomicU64,
    /// cpu controller: period length (ns). Defaults to 100 ms.
    pub cpu_period_ns: AtomicU64,

    /// memory controller: memory.max in bytes (LIMIT_MAX = max).
    pub memory_max_bytes: AtomicU64,

    /// pids controller: pids.max task ceiling (LIMIT_MAX = max).
    pub pids_max: AtomicU64,
}

/// A node in the unified cgroup hierarchy.
///
/// Node lifetime: see module docs — nodes are never freed once created
/// (rmdir only unlinks), so `Arc<CgroupNode>` → raw pointer → `&'static`
/// reinterpretation is sound.
pub struct CgroupNode {
    /// Name of this cgroup (component name; "" for the root).
    pub name: String,
    /// Parent node (None for the root only).
    pub parent: Option<Arc<CgroupNode>>,
    /// Child cgroups, keyed by name (mkdir/rmdir on cgroupfs).
    pub children: Spinlock<BTreeMap<String, Arc<CgroupNode>>>,
    /// Depth from the root (root = 0).
    pub level: u32,
    /// PIDs of member tasks (leaf membership; ancestors see them via the
    /// subtree walk in `count_tasks_subtree`).
    pub tasks: Spinlock<Vec<u32>>,
    /// Controller configuration (limits + subtree_control).
    pub controllers: Spinlock<CgroupControllers>,

    // ---- cpu runtime accounting (atomics: touched from timer IRQ) ----
    /// Total cpu time consumed by this cgroup's tasks (ns), all periods.
    pub cpu_usage_total_ns: AtomicU64,
    /// cpu time consumed in the CURRENT period (ns).
    pub cpu_usage_period_ns: AtomicU64,
    /// sched_clock timestamp when the current period started (ns).
    pub cpu_period_start_ns: AtomicU64,
    /// Throttle flag: set when usage exceeds quota, cleared at rollover.
    pub cpu_throttled: AtomicBool,
    /// How many times this cgroup entered the throttled state.
    pub nr_throttled: AtomicU64,
    /// Throttle duration accumulator (ns) — approximated at unthrottle.
    pub throttled_ns: AtomicU64,

    // ---- memory accounting (atomics) ----
    /// memory.current: bytes charged to this cgroup (includes children —
    /// charges propagate up the chain).
    pub memory_usage_bytes: AtomicU64,

    /// Inode-number base for this node (cgroupfs). File inodes are
    /// `ino_base + file kind`, so the stride below must exceed the number
    /// of control files.
    pub ino_base: u64,
}
