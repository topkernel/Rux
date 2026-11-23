//! The scheduling ENTITY STORAGE (crate-flip S1b): the per-task
//! scheduling entities, the load weights, the class RUNQUEUES (the
//! GRQ's embedded storage) and the clock/slice math — everything Task
//! or GlobalRunQueue embed by value. Storage follows the owner; the
//! policy crate manipulates it through pub fields/methods and its
//! decision fns (the pick cores live policy-side as free fns over
//! this storage; the diag counters route through SchedHooks rows).

use super::sched_hooks::{CLASS_DL_BIT, CLASS_RT_BIT, CLASS_CFS_BIT};
use core::sync::atomic::{AtomicU64, AtomicBool, AtomicU32, AtomicI64, AtomicUsize, Ordering};
use alloc::collections::BTreeMap;
use super::edges::{VruntimeKey, RbNode, RbTimeline, node_key, node_of_task, task_of_node};
use crate::list::ListHead;
use crate::process::task::{Task, SchedPolicy};
use crate::config::{MAX_CPUS, MAX_TASKS, KERNEL_HZ, CFS_MIN_GRANULARITY_NS, CFS_LATENCY_NS};
use crate::trace;


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
    nr_running: AtomicU64,

    /// Total weight
    pub load_weight: AtomicU64,

    /// Next task ID (for generating unique keys)
    next_task_id: AtomicU64,
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
    avg_vruntime_sum: i128,
}

/// Get current time (nanoseconds)
///
/// Use RISC-V time register
pub fn sched_clock() -> u64 {
    // Arch wall-clock counter (rdtime on riscv64, TSC on x86_64).
    let time: u64 = crate::arch::cpu::read_time();
    // Assume clock frequency is 10MHz (100ns precision)
    // Actual value needs adjustment based on platform
    time * TIMEBASE_MULT
}

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

/// Scheduling granularity (nanoseconds) - from config
pub const SCHED_MIN_GRANULARITY_NS: u64 = CFS_MIN_GRANULARITY_NS;

/// Scheduling latency (nanoseconds) - from config
pub const SCHED_LATENCY_NS: u64 = CFS_LATENCY_NS;

/// Timebase multiplier: QEMU uses 10MHz timebase, convert to nanoseconds
const TIMEBASE_MULT: u64 = 100;

/// Clock frequency (HZ) - from config
const HZ: u64 = KERNEL_HZ as u64;

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

/// RT runqueue
/// Maximum RT priority (0-99, HIGHER value = higher priority, matching
/// Linux). The runqueue bitmap index is inverted (bit 0 = priority 99) so
/// the lowest set bit is the highest-priority runnable task.
pub const MAX_RT_PRIO: usize = 100;

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
            bitmap: [AtomicU64::new(0), AtomicU64::new(0)],
            queue,
        }
    }

    /// Const constructor for static initialization.
    /// ListHead::new() creates null-initialized self-pointing nodes.
    pub const fn new_const() -> Self {
        Self {
            rt_nr_running: AtomicU32::new(0),
            rr_nr_running: AtomicU32::new(0),
            highest_prio: AtomicU32::new(MAX_RT_PRIO as u32),
            bitmap: [AtomicU64::new(0), AtomicU64::new(0)],
            queue: [const { ListHead::new() }; MAX_RT_PRIO],
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

    /// timer-gap lane: raw bitmap word for wedge diagnostics (idx 0/1).
    #[inline]
    pub fn bitmap_word(&self, idx: usize) -> u64 {
        self.bitmap[idx.min(1)].load(Ordering::Acquire)
    }

    /// timer-gap lane: is any task linked at a USER priority strictly
    /// above `min_prio` whose affinity mask allows `mask`? Caller holds
    /// the GRQ. Used by change_task_policy's self-weakening poke: a
    /// running incumbent that just lowered itself must hand the cpu to
    /// a now-higher queued task (the reference's check_preempt_curr at
    /// __setscheduler). Walks bitmap words top-down; first allowed
    /// match decides.
    pub fn highest_linked_user_prio(&self, mask: u32, min_prio: u32) -> bool {
        if mask == 0 {
            return false;
        }
        for word_idx in 0..2usize {
            let word = self.bitmap[word_idx].load(Ordering::Acquire);
            let mut remaining = word;
            while remaining != 0 {
                let bit = remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                let user_prio = 99 - (word_idx * 64 + bit);
                if (user_prio as u32) <= min_prio {
                    // Bitmap is ordered by (inverted) priority: once we
                    // pass min_prio top-down, nothing above it remains.
                    return false;
                }
                let head = &self.queue[word_idx * 64 + bit] as *const _ as *mut ListHead;
                // SAFETY: head anchors linked tasks; bounded walk (8) —
                // a corrupted ring cannot loop forever.
                let mut pos = crate::sched::edges::list_next(head);
                let mut n = 0;
                while pos != head && n < 8 {
                    n += 1;
                    // SAFETY: pos is an embedded rt_run_list in a linked Task.
                    let task = crate::sched::edges::task_of_rt_list(pos);
                    if crate::sched::edges::task_shared(task).cpus_allowed() & mask != 0 {
                        return true;
                    }
                    pos = crate::sched::edges::list_next(pos);
                }
            }
        }
        false
    }

    /// timer-gap lane: walk the user-priority list (0-99), `f` per task.
    /// Caller holds the GRQ lock; read-only diagnostics. NOTE: queue[]
    /// and the bitmap use the INVERTED index (bit 0 = priority 99), same
    /// as enqueue/pick_next_cpu.
    pub fn walk_prio(&self, prio: usize, f: &mut dyn FnMut(*mut Task)) {
        if prio >= MAX_RT_PRIO {
            return;
        }
        let inv = MAX_RT_PRIO - 1 - prio;
        let word = self.bitmap[inv / 64].load(Ordering::Acquire);
        if word & (1u64 << (inv % 64)) == 0 {
            return;
        }
        let head = &self.queue[inv] as *const _ as *mut ListHead;
        // SAFETY: head anchors linked tasks; walking next pointers under
        // the GRQ lock is stable. Bounded at 8 entries to be safe against
        // a corrupted ring.
        let mut pos = crate::sched::edges::list_next(head);
        let mut n = 0;
        while pos != head && n < 8 {
            n += 1;
            // SAFETY: pos is an embedded rt_run_list in a linked Task.
            let task = crate::sched::edges::task_of_rt_list(pos);
            f(task);
            pos = crate::sched::edges::list_next(pos);
        }
    }

    /// Check if queue is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.rt_nr_running.load(Ordering::Acquire) == 0
    }

    /// Decrement with a CHECKED subtract (the fair-class twin, see
    /// fair.rs nr_running_dec): a raw fetch_sub on an unbalanced
    /// decrement wraps the u32 to ~4e9 — is_empty() then reads false
    /// forever and the idle fast path dies (the P08 comment's underflow
    /// hazard, now structural).
    fn rt_nr_dec(&self) {
        // grq-hold-slim Cut 2: on the 1 -> 0 transition clear the RT
        // class bit (GRQ-held; see CLASS_NONEMPTY in sched.rs).
        if let Ok(old) = self.rt_nr_running.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |v| v.checked_sub(1),
        ) {
            if old == 1 {
                crate::sched::sched::class_nonempty_clear(
                    crate::sched::sched_hooks::CLASS_RT_BIT,
                );
            }
        }
    }

    /// rr_nr_running twin of rt_nr_dec.
    fn rr_nr_dec(&self) {
        let _ = self.rr_nr_running.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |v| v.checked_sub(1),
        );
    }

    /// Find the highest priority with runnable tasks
    ///
    /// Returns the bitmap index (0-99) or None if empty; bit 0 is the
    /// inverted index for user priority 99, so a LOWER index is a HIGHER
    /// priority.
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
    /// Returns true if the task was actually inserted (R7-B5: false when
    /// the double-enqueue guard skipped it, so the caller keeps
    /// grq.nr_running balanced).
    pub fn enqueue(&mut self, task: *mut Task, head: bool) -> bool {
        if task.is_null() {
            return false;
        }

        // SAFETY: caller guarantees task is a valid pointer; null check above;
        // task is not yet in the RT queue and we are adding it under the caller's lock.
        {
            // INV-SCE-1: GRQ-held task.
            let t = crate::sched::edges::task_mut(task);
            let raw = t.rt_priority() as usize;
            if raw >= MAX_RT_PRIO {
                return false;
            }
            // Bitmap index is INVERTED: bit 0 = priority 99 (highest). The
            // old code stored the raw priority, making lower numbers win —
            // the opposite of Linux, so `chrt -f 99` was the WEAKEST RT
            // priority (review PROC-P06).
            let prio = MAX_RT_PRIO - 1 - raw;

            // Guard against double-enqueue: if the task is already on a
            // runqueue, skip the insertion to avoid list corruption.
            if t.rt_entity().is_on_rq() {
                return false;
            }

            // Add to priority list
            if head {
                // Add to front of list (for preempted tasks)
                crate::sched::edges::rt_list_add(t as *mut Task, &mut self.queue[prio] as *mut ListHead);
            } else {
                // Add to tail (for normal enqueue)
                crate::sched::edges::rt_list_add_tail(t as *mut Task, &mut self.queue[prio] as *mut ListHead);
            }

            // Set bit in bitmap
            let word_idx = prio / 64;
            let bit_idx = prio % 64;
            self.bitmap[word_idx].fetch_or(1u64 << bit_idx, Ordering::AcqRel);

            // Update counters
            // grq-hold-slim Cut 2: on the 0 -> nonempty transition set
            // the RT class bit (GRQ-held).
            let rt_old = self.rt_nr_running.fetch_add(1, Ordering::AcqRel);
            if rt_old == 0 {
                crate::sched::sched::class_nonempty_set(
                    crate::sched::sched_hooks::CLASS_RT_BIT,
                );
            }

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
        true
    }

    /// Dequeue a task
    pub fn dequeue(&mut self, task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }

        // SAFETY: task was previously enqueued and is still in the RT queue;
        // null check above; dequeue removes it from the list.
        {
            // INV-SCE-1: GRQ-held task.
            let t = crate::sched::edges::task_mut(task);
            // P08: dequeue of a task that is not linked (e.g. the currently
            // running task, already dequeued at pick time) must be a no-op —
            // it used to decrement rt_nr_running/rr_nr_running regardless,
            // underflowing the counters and permanently disabling the idle
            // fast path.
            if !t.rt_entity().is_on_rq() {
                return false;
            }
            let raw = t.rt_priority() as usize;
            if raw >= MAX_RT_PRIO {
                return false;
            }
            let prio = MAX_RT_PRIO - 1 - raw; // same inversion as enqueue

            // Remove from list
            crate::sched::edges::rt_list_del(t as *mut Task);

            // Clear bit in bitmap if list is now empty
            if self.queue[prio].is_empty() {
                let word_idx = prio / 64;
                let bit_idx = prio % 64;
                self.bitmap[word_idx].fetch_and(!(1u64 << bit_idx), Ordering::AcqRel);
            }

            // Update counters
            self.rt_nr_dec();

            if t.policy() == SchedPolicy::Rr {
                self.rr_nr_dec();
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
        true
    }

    /// R41: whether `task` is ACTUALLY linked on one of the priority
    /// lists — as opposed to what rt_entity's on_rq flag claims. A task's
    /// rt_run_list is init()-ed (self-pointing) at Task creation and
    /// del() re-self-links it on unlink, so a self-pointing or null node
    /// is provably NOT linked; a linked node's next can never point at
    /// itself (each node appears at most once per list). Used by
    /// __schedule's prev requeue to distinguish a legitimately-linked prev
    /// (RR tick rotation / racing wake) from a stale on_rq=true flag.
    /// Called with the GRQ lock held; refused-path only.
    pub fn is_linked(&self, task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }
        crate::sched::edges::rt_list_linked(task)
    }

    /// Unlink `task` from its priority list WITHOUT trusting the entity's
    /// on_rq flag. free_task_slot's freed-while-linked defense (oh-display
    /// b61-b66): a task whose page is about to return to the heap may
    /// carry a STALE on_rq=false while its rt_run_list node is still
    /// linked (one-sided unlink shapes) — plain dequeue() would refuse on
    /// the flag and leave the dangling node behind, poisoning every later
    /// walk of that list once the page is reused. is_linked() gates the
    /// call, so a task that is provably unlinked costs one pointer read.
    ///
    /// Counters/bitmap are reconciled exactly like dequeue(). Called with
    /// the GRQ lock held, from the task-free path only (the task memory is
    /// still valid — del() writes land in the list's real neighbors).
    pub fn force_unlink(&mut self, task: *mut Task) -> bool {
        if task.is_null() || !self.is_linked(task) {
            return false;
        }
        // SAFETY: is_linked() proved the node is linked into one of
        // self.queue's lists; the caller (free_task_slot) holds the GRQ
        // lock and the Task memory is not yet freed/poisoned.
        {
            // INV-SCE-1: GRQ-held task.
            let t = crate::sched::edges::task_mut(task);
            let raw = t.rt_priority() as usize;
            // Remove from list regardless of the (possibly stale) on_rq
            // flag — the linkage check above is the authority.
            crate::sched::edges::rt_list_del(t as *mut Task);

            if raw < MAX_RT_PRIO {
                // Same bitmap inversion as enqueue/dequeue.
                let prio = MAX_RT_PRIO - 1 - raw;
                if self.queue[prio].is_empty() {
                    let word_idx = prio / 64;
                    let bit_idx = prio % 64;
                    self.bitmap[word_idx].fetch_and(!(1u64 << bit_idx), Ordering::AcqRel);
                }
            } else {
                // Priority out of range: the node was linked through some
                // corruption; sweep every empty-list bit so the bitmap
                // cannot keep naming a prio whose list is now empty.
                for prio in 0..MAX_RT_PRIO {
                    if self.queue[prio].is_empty() {
                        let word_idx = prio / 64;
                        let bit_idx = prio % 64;
                        self.bitmap[word_idx].fetch_and(!(1u64 << bit_idx), Ordering::AcqRel);
                    }
                }
            }

            // Counters: rt_nr_running/rr_nr_running may already be
            // consistent (a normally-linked task) or drifted; clamp at 0
            // so a double-decrement cannot underflow.
            // Counters: may already be consistent (a normally-linked
            // task) or drifted; the checked subtract clamps at 0 so a
            // double-decrement cannot underflow-wrap (the raw load>0
            // guard raced a concurrent decrement).
            self.rt_nr_dec();
            if t.policy() == SchedPolicy::Rr {
                self.rr_nr_dec();
            }

            if self.is_empty() {
                self.highest_prio.store(MAX_RT_PRIO as u32, Ordering::Release);
            } else if let Some(new_highest) = self.find_highest_prio() {
                self.highest_prio.store(new_highest, Ordering::Release);
            }

            t.rt_entity().set_on_rq(false);
        }
        true
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
        // SAFETY: list_ptr is the head of a non-empty priority list; the embedded
        // ListHead is at offset rt_run_list within Task, so sub(offset) yields the Task.
        let task = crate::sched::edges::task_of_rt_list(list_ptr);

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
        // SAFETY: same as pick_next — list_ptr is an embedded ListHead in an enqueued Task.
        let task = crate::sched::edges::task_of_rt_list(list_ptr);

        Some(task)
    }
}

// ---- The committed entity set (a7f385732) ----

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
        let now = sched_clock();
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

impl core::fmt::Display for W2RefusalCensus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "linked={} on_rq={} busy_slot={} idle_slot={} exec_ns={} jif={} q_ms={}",
            self.linked,
            self.on_rq,
            self.busy_slot,
            self.idle_slot,
            self.exec_ns,
            self.jiffies,
            self.quantum_ms
        )
    }
}

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

    /// Decrement with a CHECKED subtract (the fair-class twin, see
    /// fair.rs nr_running_dec): a raw fetch_sub wraps on an unbalanced
    /// decrement and is_empty() then reads false forever.
    fn dl_nr_dec(&self) {
        // grq-hold-slim Cut 2: on the 1 -> 0 transition clear the DL
        // class bit (GRQ-held; see CLASS_NONEMPTY in sched.rs).
        if let Ok(old) = self.dl_nr_running.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |v| v.checked_sub(1),
        ) {
            if old == 1 {
                crate::sched::sched::class_nonempty_clear(
                    crate::sched::sched_hooks::CLASS_DL_BIT,
                );
            }
        }
    }

    /// Enqueue a task
    /// Returns true if the task was actually inserted (R7-B5).
    pub fn enqueue(&mut self, task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }

        // R7-B4: double-enqueue guard. CFS and RT check their entity's
        // on_rq before inserting; DL did not — two concurrent wake_ups of
        // the same sleeping SCHED_DEADLINE task (timer timeout on one CPU +
        // futex_wake on another) both passed the is_sleeping check and
        // inserted the Task twice: two CPUs picked it and ran one kernel
        // stack on two harts.
        if crate::process::unsafe_edges::borrow_task(task).dl_entity().on_rq.load(Ordering::Acquire) {
            return false;
        }

        // Caller guarantees `task` points to a valid, live Task that
        // remains allocated for the duration of this enqueue (the GRQ
        // hold pins it — INV-PROC-BORROW-1's GRQ arm). The Task is not
        // currently on any runqueue (precondition for enqueue).
        {
            let t = crate::process::unsafe_edges::borrow_task(task);
            let dl = t.dl_entity();

            // Get deadline
            let deadline = dl.deadline.load(Ordering::Acquire);

            // Generate unique key
            let task_id = self.next_id.fetch_add(1, Ordering::AcqRel);
            let key = DlKey { deadline, task_id };

            // Add to tree
            self.tasks.insert(key, task);

            // Update counters
            // grq-hold-slim Cut 2: on the 0 -> nonempty transition set
            // the DL class bit (GRQ-held).
            let dl_old = self.dl_nr_running.fetch_add(1, Ordering::AcqRel);
            if dl_old == 0 {
                crate::sched::sched::class_nonempty_set(
                    crate::sched::sched_hooks::CLASS_DL_BIT,
                );
            }

            // Update earliest deadline
            let curr_earliest = self.earliest_dl.load(Ordering::Acquire);
            if deadline < curr_earliest {
                self.earliest_dl.store(deadline, Ordering::Release);
            }

            // Set on_rq flag
            dl.on_rq.store(true, Ordering::Release);
        }
        true
    }

    /// Dequeue a task
    ///
    /// Returns true only if the task was actually linked on this queue
    /// (R32-DL2, mirroring rt.rs P08): a dequeue of a task that is not
    /// linked (e.g. the currently running task, already dequeued at pick
    /// time) must report false so callers keep grq.nr_running balanced.
    pub fn dequeue(&mut self, task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }

        // Caller guarantees `task` points to a valid, live Task
        // (INV-PROC-BORROW-1's GRQ arm).
        {
            let t = crate::process::unsafe_edges::borrow_task(task);
            let dl = t.dl_entity();

            // P08-mirror: a task that is not linked must be a no-op. The
            // old unconditional `true` lied to dequeue_task() /
            // change_task_policy() — every no-op exit of an RT/DL waiter's
            // cleanup path decremented grq.nr_running, drifting the count.
            if !dl.on_rq.load(Ordering::Acquire) {
                return false;
            }

            // Find and remove task by pointer only. Do NOT match on deadline
            // because the deadline may have been updated between enqueue and
            // dequeue, causing the search to miss the entry.
            let mut found_key = None;
            for (&key, &ptr) in self.tasks.iter() {
                if ptr == task {
                    found_key = Some(key);
                    break;
                }
            }

            if let Some(key) = found_key {
                self.tasks.remove(&key);

                // Update counters
                self.dl_nr_dec();

                // Update earliest deadline
                if let Some((&next_key, _)) = self.tasks.iter().next() {
                    self.earliest_dl.store(next_key.deadline, Ordering::Release);
                } else {
                    self.earliest_dl.store(u64::MAX, Ordering::Release);
                }

                // Clear on_rq flag
                dl.on_rq.store(false, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// R41: whether `task` is ACTUALLY linked on this queue (scan by
    /// pointer, mirroring dequeue's lookup) — as opposed to what the
    /// entity's on_rq flag claims. Used by __schedule's prev requeue to
    /// distinguish a legitimately-linked prev from a stale on_rq=true
    /// flag. Called with the GRQ lock held; refused-path only.
    pub fn is_linked(&self, task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }
        self.tasks.iter().any(|(_, &ptr)| ptr == task)
    }

    /// Pick the task with earliest deadline
    pub fn pick_next(&mut self) -> Option<*mut Task> {
        if let Some((&_key, &task)) = self.tasks.iter().next() {
            // Remove from tree
            self.tasks.remove(&_key);

            // Update counters
            self.dl_nr_dec();

            // Update earliest deadline
            if let Some((&next_key, _)) = self.tasks.iter().next() {
                self.earliest_dl.store(next_key.deadline, Ordering::Release);
            } else {
                self.earliest_dl.store(u64::MAX, Ordering::Release);
            }

            // Clear on_rq flag: `task` was just removed from self.tasks
            // (a valid pointer to a live Task previously enqueued here).
            crate::process::unsafe_edges::borrow_task(task).dl_entity().on_rq.store(false, Ordering::Release);

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

    /// Pick the earliest-deadline task that is allowed to run on `cpu_id`.
    /// Scans from the leftmost (earliest deadline) entry, skipping tasks
    /// whose `cpus_allowed` mask does not include `cpu_id`.
    ///
    /// Iterates by temporarily removing non-matching entries to inspect
    /// subsequent ones, then putting them back. DL task counts are typically
    /// very small so this is fine.
    pub fn pick_next_cpu(&mut self, cpu_id: usize, prev: *mut Task) -> Option<*mut Task> {
        // Stash entries that don't match, then re-insert them after.
        let mut skipped: [(DlKey, *mut Task); 64] = [(DlKey { deadline: 0, task_id: 0 }, core::ptr::null_mut()); 64];
        let mut skip_count = 0usize;
        let mut result = None;

        loop {
            let key = match self.tasks.iter().next() {
                Some((&k, _)) => k,
                None => break,
            };
            let task = match self.tasks.get(&key) {
                Some(&t) => t,
                None => break,
            };

            // b50 #GP guard — see the fair-class twin. A scribbled slot
            // pointer must never reach the deref below.
            if !crate::sched::sched::kernel_task_ptr_range_ok(task as *const u8) {
                self.tasks.remove(&key);
                self.dl_nr_dec();
                crate::sched::sched::report_poisoned_rq_entry(cpu_id, task as usize);
                continue;
            }

            // SAFETY: `task` was obtained from self.tasks which only stores
            // valid, live Task pointers inserted by enqueue().
            // R8-1b: skip tasks whose context is not yet saved, except
            // the switching CPU's own prev (fast path).
            // R65: dual-bit ownership (on_cpu AND running_on_cpu claim
            // must both be free) — see the fair-class twin.
            let allowed = {
                // R65-pick (probe-slow): single-qword ownership read —
                // see the fair-class twin.
                let t = crate::process::unsafe_edges::borrow_task(task);
                let ownership_ok =
                    task == prev || t.ownership_pickable(cpu_id);
                t.cpu_allowed(cpu_id) && ownership_ok
            };
            if allowed {
                // Found a match — remove and return it
                self.tasks.remove(&key);
                self.dl_nr_dec();

                if let Some((&next_key, _)) = self.tasks.iter().next() {
                    self.earliest_dl.store(next_key.deadline, Ordering::Release);
                } else {
                    self.earliest_dl.store(u64::MAX, Ordering::Release);
                }

                // `task` was just removed from self.tasks; a valid
                // pointer to a live Task previously enqueued.
                crate::process::unsafe_edges::borrow_task(task).dl_entity().on_rq.store(false, Ordering::Release);

                result = Some(task);
                break;
            }

            // Not allowed — remove temporarily and stash
            self.tasks.remove(&key);
            if skip_count < skipped.len() {
                skipped[skip_count] = (key, task);
                skip_count += 1;
            } else {
                // More than 64 skipped DL tasks (very unlikely) — just re-insert
                self.tasks.insert(key, task);
                break;
            }
        }

        // Re-insert all skipped entries
        for i in (0..skip_count).rev() {
            let (key, task) = skipped[i];
            self.tasks.insert(key, task);
        }

        result
    }
}

impl CfsRunQueue {
    /// Create new CFS run queue
    pub fn new() -> Self {
        Self {
            timeline: RbTimeline::new(),
            curr: core::ptr::null_mut(),
            min_vruntime: AtomicU64::new(0),
            nr_running: AtomicU64::new(0),
            load_weight: AtomicU64::new(0),
            next_task_id: AtomicU64::new(0),
            avg_vruntime_sum: 0,
        }
    }

    /// The leftmost (min-vruntime) entry — O(1) via the cached leftmost
    /// node (the reference's rb_leftmost; no invalidation-walk corner:
    /// the cache is maintained exactly at insert/erase). GRQ-HELD.
    #[inline]
    fn leftmost_entry(&mut self) -> Option<(VruntimeKey, *mut crate::process::Task)> {
        let nil = self.timeline.nil();
        let node = self.timeline.leftmost;
        if node == nil || node.is_null() || !RbTimeline::node_ok(node) {
            return None;
        }
        // SAFETY: node range-checked above; linked-node key invariant.
        let key = node_key(node);
        Some((key, task_of_node(node)))
    }

    /// 64th (EEVDF): the weighted average vruntime of the competition --
    /// 6.6 avg_vruntime(): V = min_vruntime + (Sum (vr_i-min_vr)*w_i +
    /// curr term) / (Sum w_i + curr w). This -- NOT min_vruntime -- is
    /// the placement base for wakes and first links: min_vruntime is
    /// pinned by whatever leftmost wakee the lag clamp placed just below
    /// it, while the average advances with the pack and the running
    /// entity, so a bounded-lag placement against V cannot strand a
    /// producer hundreds of ms behind a low-duty pack the way the old
    /// keep-your-own-vr wake path did (ohw34's 400ms pre-wedge
    /// link->pick). GRQ-HELD.
    ///
    /// GRQ divergence (single global curr slot, R20-6): the reference is
    /// per-runqueue and counts exactly its own curr; we count THE curr
    /// slot when it names a live CFS task. Other concurrently-running CFS
    /// tasks are invisible to V (bounded skew -- they are within the
    /// clamp window of the pack), and a curr that was re-linked by a
    /// preemption is double-counted for one pick window (the reference
    /// has the same transient between put_prev_entity and
    /// set_next_entity).
    pub fn avg_vruntime(&self) -> u64 {
        let base = self.get_min_vruntime() as i128;
        let mut sum = self.avg_vruntime_sum;
        let mut load = self.load_weight.load(Ordering::Acquire) as i128;
        if !self.curr.is_null() {
            // SAFETY: curr is null or a valid Task set by set_curr;
            // GRQ-held read of stable fields. Per-CPU idle tasks (pid 0)
            // are not CFS competition (6.6's idle class): skip.
            let c = crate::sched::edges::task_shared(self.curr);
            if c.pid() != 0 {
                let se = c.sched_entity();
                let w = se.load.weight as i128;
                sum += (se.get_vruntime() as i128 - base) * w;
                load += w;
            }
        }
        if load <= 0 {
            return self.get_min_vruntime();
        }
        let v = base + sum / load;
        v.clamp(0, u64::MAX as i128) as u64
    }

    /// 64th (update_entity_lag parity): capture the clamped virtual lag
    /// of the RUNNING task that is now going to sleep. The reference runs
    /// update_entity_lag inside dequeue_entity for curr (fair.c:5534,
    /// ahead of __dequeue_entity -- the entity is still counted in the
    /// average); our picked tasks are already unlinked, so the sleep
    /// transition in __schedule calls this instead (linked sleepers get
    /// theirs inside dequeue()). GRQ-HELD.
    pub fn capture_sleep_vlag(&mut self, task: *mut crate::process::Task) {
        if task.is_null() {
            return;
        }
        // SAFETY: the caller passes the switching-out prev; stable-field
        // reads under the GRQ.
        let se = crate::sched::edges::task_shared(task).sched_entity();
        if se.is_on_rq() {
            return; // linked sleeper: dequeue() owns the capture
        }
        let base = self.get_min_vruntime() as i128;
        let w = se.load.weight.max(1) as i128;
        // V with THIS task counted as competition (it is still the
        // curr-class runner; the reference's avg_vruntime includes curr),
        // plus the global curr slot when it names someone else.
        let mut sum = self.avg_vruntime_sum + (se.get_vruntime() as i128 - base) * w;
        let mut load = self.load_weight.load(Ordering::Acquire) as i128 + w;
        if !self.curr.is_null() && self.curr != task {
            // SAFETY: as avg_vruntime.
            let c = crate::sched::edges::task_shared(self.curr);
            if c.pid() != 0 {
                let cse = c.sched_entity();
                let cw = cse.load.weight as i128;
                sum += (cse.get_vruntime() as i128 - base) * cw;
                load += cw;
            }
        }
        let v = if load > 0 { base + sum / load } else { base };
        // 6.6 entity_lag: limit = calc_delta_fair(max(2*slice, TICK_NSEC))
        // -- the wall-ns bound converted into this entity's virtual units.
        let tick_ns = 1_000_000_000u64 / HZ;
        let limit_base = (2 * SCHED_MIN_GRANULARITY_NS).max(tick_ns);
        let limit = (limit_base as i128 * NICE_0_LOAD as i128) / w;
        se.vlag.set(((v - se.get_vruntime() as i128).clamp(-limit, limit)) as i64);
    }

    /// grq-10ns: link `task` into the intrusive timeline under `key`.
    /// Allocation-free (node embedded in the entity). Bracketed by the
    /// cctx FAIR_TL probe (zero cost when cctx is off). Returns false
    /// when the poison discipline refused the link.
    #[inline]
    pub fn timeline_insert(&mut self, key: VruntimeKey, task: *mut crate::process::Task) -> bool {
        let t0 = crate::debug::cctx::now();
        // nanosleep-loss conviction: inserting a node that is STILL
        // PHYSICALLY LINKED (a prior removal failed silently — pick_next
        // ignores timeline_remove's failure) corrupts the tree (the
        // insert overwrites the in-tree node's parent/child links,
        // orphaning whatever referenced it). Name it live.
        if !task.is_null() {
            let node = node_of_task(task);
            if RbNode::linked_at(node) {
                static DI_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if DI_N.fetch_add(1, Ordering::Relaxed) < 12 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(mut v: u64) {
                        for _ in 0..8 {
                            let n = (v >> 28) as u8;
                            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                            v <<= 4;
                        }
                    }
                    for &b in b"\nTL-INSERT-OF-LINKED pid=0x" { putchar(b); }
                    ph(crate::sched::edges::task_shared(task).pid() as u64);
                    putchar(b'\n');
                }
            }
        }
        // Store the ORDER key first (the tree's comparisons read it —
        // insert stores, the descent reads candidate keys).
        {
            // INV-SCE-1/6: GRQ-held task; timeline ops are safe-signed
            // edges with the poison discipline inside.
            let se = crate::sched::edges::task_shared(task);
            se.sched_entity().set_timeline_key(Some(key));
            if !self.timeline.insert(key, node_of_task(task)) {
                // Poison discipline (see RbTimeline::insert): the descent
                // hit an out-of-range hop. Refuse the link, clear the
                // key, keep the kernel alive; the counters stay paired
                // with the timeline (the entity was NOT linked).
                se.sched_entity().set_timeline_key(None);
                crate::sched::sched::tl_poison_insert_inc();
                crate::sched::sched::report_poisoned_rq_entry(
                    usize::MAX,
                    task as usize,
                );
                crate::debug::cctx::add(&crate::debug::cctx::FAIR_TL_NS, t0);
                crate::debug::cctx::bump(&crate::debug::cctx::FAIR_TL_N);
                return false;
            }
        }
        // 64th (EEVDF): avg_vruntime bookkeeping -- the RELATIVE offset of
        // the inserted key (6.6 avg_vruntime_add / entity_key). The paired
        // removal subtracts the same offset formula; rebases keep every
        // stored offset relative to the current min_vruntime, so
        // insert/remove pair exactly regardless of min_vr movement in
        // between.
        // SAFETY: linked node — valid Task.
        let w = crate::sched::edges::task_shared(task).sched_entity().load.weight as i128;
        self.avg_vruntime_sum +=
            (key.vruntime as i128 - self.get_min_vruntime() as i128) * w;
        crate::debug::cctx::add(&crate::debug::cctx::FAIR_TL_NS, t0);
        crate::debug::cctx::bump(&crate::debug::cctx::FAIR_TL_N);
        true
    }

    /// grq-10ns: unlink `task`'s node from the timeline (keyless — the
    /// node's position IS the linkage). `key` feeds the avg_vruntime
    /// bookkeeping arithmetic (the stored ORDER key, which the caller
    /// still holds). Returns false when the poison discipline refused
    /// the unlink (node stays linked; counters stay paired).
    #[inline]
    pub fn timeline_remove(&mut self, key: &VruntimeKey, task: *mut crate::process::Task) -> bool {
        let t0 = crate::debug::cctx::now();
        // SAFETY: callers pass the task whose node is linked (or, on the
        // poison path, a null task — handled below).
        let ok = if task.is_null() {
            false
        } else {
            self.timeline.erase(node_of_task(task))
        };
        if !ok {
            // Refused (poison discipline or not-linked): report; leave
            // the counters paired with the timeline (the entry is still
            // linked).
            // nanosleep-loss conviction: name the refusal live (the
            // silent producer of still-linked nodes — a later insert of
            // such a node corrupts the tree).
            static TRE_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if !task.is_null()
                && RbNode::linked_at(node_of_task(task))
                && TRE_N.fetch_add(1, Ordering::Relaxed) < 12
            {
                use crate::debug::taskdump::putc_brief as putchar;
                fn ph(mut v: u64) {
                    for _ in 0..8 {
                        let n = (v >> 28) as u8;
                        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                        v <<= 4;
                    }
                }
                for &b in b"\nTL-ERASE-REFUSED-ON-LINKED pid=0x" { putchar(b); }
                ph(crate::sched::edges::task_shared(task).pid() as u64);
                putchar(b'\n');
            }
            crate::sched::sched::tl_poison_erase_inc();
            crate::debug::cctx::add(&crate::debug::cctx::FAIR_TL_NS, t0);
            crate::debug::cctx::bump(&crate::debug::cctx::FAIR_TL_N);
            return false;
        }
        // SAFETY: unlinked now; valid Task.
        crate::sched::edges::task_shared(task).sched_entity().set_timeline_key(None);
        if !task.is_null() {
            let w = crate::sched::edges::task_shared(task).sched_entity().load.weight as i128;
            self.avg_vruntime_sum -=
                (key.vruntime as i128 - self.get_min_vruntime() as i128) * w;
        }
        crate::debug::cctx::add(&crate::debug::cctx::FAIR_TL_NS, t0);
        crate::debug::cctx::bump(&crate::debug::cctx::FAIR_TL_N);
        true
    }

    /// Get number of tasks in run queue
    #[inline]
    pub fn nr_running(&self) -> u64 {
        self.nr_running.load(Ordering::Acquire)
    }

    /// Decrement the runnable count with a CHECKED subtract. A raw
    /// `fetch_sub` on an unbalanced decrement WRAPS to u64::MAX, and the
    /// next sched_slice() then panics on
    /// `SCHED_MIN_GRANULARITY_NS * nr_running` (multiply overflow — the
    /// x86 power-off teardown capture: both CPUs panicked in
    /// scheduler_tick -> sched_slice right after init's reboot()). The
    /// grq-level counter already uses this exact discipline
    /// (sched.rs); a refused decrement leaves the value at 0 and
    /// grq_diag_counts() names the drift.
    #[track_caller]
    pub fn nr_running_dec(&self) {
        // grq-10ns: single locked-RMW form (fetch_sub = one xadd) instead
        // of the fetch_update CAS loop. Every writer of this counter
        // holds the GRQ lock, so a wrap (old == 0) can be restored with a
        // plain store — no other decrement can interleave between the
        // sub and the restore. Same checked-sub semantics, one bus op.
        let old = self.nr_running.fetch_sub(1, Ordering::SeqCst);
        if old == 1 {
            // grq-hold-slim Cut 2: CFS emptied — clear the class
            // nonempty bit (GRQ-held; see CLASS_NONEMPTY in sched.rs).
            crate::sched::sched::class_nonempty_clear(
                crate::sched::sched_hooks::CLASS_CFS_BIT,
            );
        }
        if old == 0 {
            self.nr_running.store(0, Ordering::SeqCst);
            // Cut 2 consistency: the counter reads 0 again after the
            // restore — keep the class bit identical to what is_empty()
            // would now report.
            crate::sched::sched::class_nonempty_clear(
                crate::sched::sched_hooks::CLASS_CFS_BIT,
            );
            // REGRESSION GUARD: the counter was already 0 when a
            // removal path decremented again. The one historical producer
            // was the 2b092c49 discarded-guard GRQ unlock (every critical
            // section unexcluded — see the GlobalRunQueue doc note); with
            // mutual exclusion intact this must never fire. Rate-limited
            // direct console (may run under the GRQ); names the offending
            // caller site via #[track_caller].
            static LAST: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::timer::get_jiffies();
            let last = LAST.load(Ordering::Acquire);
            if now.wrapping_sub(last) >= 10
                && LAST.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire).is_ok()
            {
                let loc = core::panic::Location::caller();
                let pid = crate::sched::get_current_pid();
                use crate::debug::taskdump::putc_brief as putchar;
                for &b in b"\nNRUN-DOUBLE-DEC pid=0x" { putchar(b); }
                let mut v = pid as u64;
                for _ in 0..8 {
                    putchar(b'0' + ((v >> 28) & 0xF) as u8);
                    v <<= 4;
                }
                for &b in b" at " { putchar(b); }
                let f = loc.file();
                let base = f.rfind('/').map(|i| i + 1).unwrap_or(0);
                for &b in f.as_bytes()[base..].iter() {
                    putchar(b);
                }
                putchar(b':');
                let mut ln = loc.line() as u64;
                let mut buf = [0u8; 10];
                let mut n = 0;
                if ln == 0 { putchar(b'0'); } else {
                    while ln > 0 && n < 10 { buf[n] = b'0' + (ln % 10) as u8; ln /= 10; n += 1; }
                    while n > 0 { n -= 1; putchar(buf[n]); }
                }
                putchar(b'\n');
            }
        }
    }

    /// Same no-wrap discipline for the aggregated weight: a wrapped
    /// load_weight reads as a huge total and collapses every task's
    /// slice to the minimum granularity. Regression-guard twin of
    /// nr_running_dec above (the 2b092c49 unexcluded sections were the
    /// one historical producer).
    #[track_caller]
    pub fn load_weight_sub(&self, weight: u64) {
        // grq-10ns: single locked-RMW form (see nr_running_dec) — one
        // xadd; a wrap (old < weight) is restored to `old` because every
        // writer holds the GRQ.
        let old = self.load_weight.fetch_sub(weight, Ordering::SeqCst);
        if old < weight {
            self.load_weight.store(old, Ordering::SeqCst);
            static LASTW: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::timer::get_jiffies();
            let last = LASTW.load(Ordering::Acquire);
            if now.wrapping_sub(last) >= 10
                && LASTW.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire).is_ok()
            {
                let loc = core::panic::Location::caller();
                use crate::debug::taskdump::putc_brief as putchar;
                for &b in b"\nLWEIGHT-UNDERFLOW at " { putchar(b); }
                let f = loc.file();
                let base = f.rfind('/').map(|i| i + 1).unwrap_or(0);
                for &b in f.as_bytes()[base..].iter() {
                    putchar(b);
                }
                putchar(b'\n');
            }
        }
    }

    /// Check if run queue is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.timeline.is_empty()
    }

    /// Get minimum vruntime
    #[inline]
    pub fn get_min_vruntime(&self) -> u64 {
        self.min_vruntime.load(Ordering::Acquire)
    }

    /// Wakewatch: the leftmost queued entity's vruntime (None when the
    /// timeline is empty). Diagnostic read; callers hold the GRQ lock.
    /// grq-10ns: O(1) via the cached leftmost node.
    pub fn leftmost_vruntime(&self) -> Option<u64> {
        let nil = self.timeline.nil();
        let node = self.timeline.leftmost;
        if node == nil || node.is_null() || !RbTimeline::node_ok(node) {
            return None;
        }
        // SAFETY: linked, in-range node.
        Some(node_key(node).vruntime)
    }

    /// s16 (A2 census): how many timeline entries sort at or before
    /// `vr` — the tie-mass standing ahead of a strand victim in
    /// service order. The r6/r7 strand lines all read vr=0ms
    /// lead-lag=0ms (victim at min_vruntime AND at the leftmost key),
    /// which is only possible as a TIE: a strictly-leftmost
    /// never-refused candidate is taken by the very next walk. tie=
    /// quantifies the mass the victim waits behind (the fair-queue
    /// hypothesis predicts tie ~ nr_running; a small tie with a big
    /// visits= would instead convict the walk itself). O(n) in-order
    /// walk (grq-10ns: successor chain), called only from the
    /// rate-gated strand print path (GRQ held).
    pub fn ww_tie_mass(&self, vr: u64) -> u64 {
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        let mut n = 0u64;
        while node != nil && node != core::ptr::null_mut() && RbTimeline::node_ok(node) {
            // SAFETY: linked, in-range node.
            let k = node_key(node);
            if k.vruntime <= vr {
                n += 1;
            } else {
                break;
            }
            // SAFETY: in-range node.
            node = self.timeline.next(node);
        }
        n
    }

    /// Update minimum vruntime
    ///
    /// Linux update_min_vruntime semantics (review batch 8): the value is the
    /// MINIMUM of the leftmost queued entity's vruntime and the running
    /// entity's vruntime (monotonically non-decreasing). Excluding `curr`
    /// let min_vruntime run arbitrarily far ahead of the running task — a
    /// sleeper woken later than one latency window was clamped up to
    /// min_vruntime, erasing its entire sleep compensation.
    pub fn update_min_vruntime(&mut self) {
        // SAFETY: self.curr is either null or set by set_curr() to a valid
        // Task pointer; only its sched_entity vruntime is read.
        let curr_v = if self.curr.is_null() {
            None
        } else {
            Some(crate::sched::edges::task_shared(self.curr).sched_entity().get_vruntime())
        };

        // Leg-55: O(1) via the leftmost cache (update_min_vruntime runs on
        // every enqueue/dequeue/pick — the walk was a per-op GRQ cost).
        let leftmost = self.leftmost_entry().map(|(k, _)| k.vruntime);

        // vruntime = curr (if running) clamped by leftmost (if any queued).
        let vruntime = match (curr_v, leftmost) {
            (Some(c), Some(l)) => Some(c.min(l)),
            (only_one, _) => only_one.or(leftmost),
        };

        if let Some(v) = vruntime {
            let min_vruntime = self.min_vruntime.load(Ordering::Acquire);
            // min_vruntime only increases, ensure monotonic
            if v > min_vruntime {
                // 64th (6.6 __update_min_vruntime -> avg_vruntime_update):
                // rebase the stored relative offsets so every term stays
                // relative to the NEW min_vruntime: sum -= delta*avg_load.
                // load_weight is the queued tree's weight (the exact set
                // whose offsets live in the sum); GRQ-held here.
                let delta = (v - min_vruntime) as i128;
                self.avg_vruntime_sum -=
                    delta * self.load_weight.load(Ordering::Acquire) as i128;
                self.min_vruntime.store(v, Ordering::Release);
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
        {
            // INV-SCE-1: GRQ-held task (the caller holds the runqueue).
            let task_ref = crate::sched::edges::task_mut(task);

            // Get scheduling entity
            let se = task_ref.sched_entity();

            // If task is already in run queue, don't enqueue again
            if se.is_on_rq() {
                return false;
            }

            // 64th (EEVDF wake placement, 6.6 place_entity parity): the
            // old keep-your-own-vr wake path (with only a
            // min_vruntime-SCHED_LATENCY floor) let a task that had run
            // hard land arbitrarily far AHEAD of the pack at its next wake
            // — measured 400ms link→pick on an FFRT worker DURING the
            // busy boot phase (ohw34 t=22s, wedge still 5s away) — because
            // nothing bounded the vruntime DEBT a sleeper could carry.
            // The reference (6.6) clamps the lag at sleep-dequeue to
            // ±max(2*slice, TICK) virtual and places the wake at
            // avg_vruntime − lag·(load+w)/load, so no wake can land more
            // than ~one virtual tick from the competition's average.
            // Three arms, matching the reference's enqueue flags (this
            // tree's 6.6 has no START_DEBIT: INITIAL places at V with
            // zero lag — PLACE_DEADLINE_INITIAL only halves the EEVDF
            // deadline, which our min-vruntime pick does not use):
            //   INITIAL (first-ever link, fork): lag zero, place at V.
            //   WAKE (was sleeping; vlag captured at the sleep-dequeue):
            //     place at V − vlag', vlag' load-compensated (PLACE_LAG).
            //   Requeue of a preempted prev (state RUNNING at entry —
            //     6.6's put_prev_entity re-enqueues without placement):
            //     keep the current vruntime.
            if migrate {
                // Migrations keep the vruntime as-is (the GRQ has no
                // cross-queue migration, so this arm is the
                // set_cpus_allowed re-link only).
            } else if !se.ever_enqueued.get() {
                se.ever_enqueued.set(true);
                se.vlag.set(0);
                se.set_vruntime(self.avg_vruntime());
            } else if task_ref.state().bits()
                != crate::process::task::TaskState::RUNNING as u32
            {
                let v = self.avg_vruntime() as i128;
                let w = se.load.weight as i128;
                let mut load = self.load_weight.load(Ordering::Acquire) as i128;
                if !self.curr.is_null() {
                    // SAFETY: curr is null or valid (set_curr); GRQ-held.
                    let c = crate::sched::edges::task_shared(self.curr);
                    if c.pid() != 0 {
                        load += c.sched_entity().load.weight as i128;
                    }
                }
                // PLACE_LAG: lag *= (load + w) / load, preserving the lag
                // against the average's dilution by the joining entity.
                let mut lag = se.vlag.get() as i128;
                if load > 0 {
                    lag = lag * (load + w) / load;
                } else {
                    lag = 0;
                }
                let placed = (v - lag).clamp(0, u64::MAX as i128) as u64;
                se.set_vruntime(placed);
            }
            // else: preempted prev (state RUNNING) — keep vruntime.

            // 64th: vruntime sanity canary (see vr_canary).
            crate::sched::sched_hooks::vr_canary(self, task, b"ENQ", se.get_vruntime());

            // Generate unique key
            let task_id = self.next_task_id.fetch_add(1, Ordering::AcqRel);
            let key = VruntimeKey::new(se.get_vruntime(), task_id);

            // Link into the intrusive timeline (the entity's run_node —
            // zero allocation; the ORDER key is stored on the entity
            // inside timeline_insert). A refused link (poison
            // discipline) leaves the entity unlinked with the counters
            // still paired — report and refuse the enqueue.
            if !self.timeline_insert(key, task) {
                return false;
            }

            // Update status
            se.set_on_rq(true);

            // Update task count and total weight
            // grq-hold-slim Cut 2: on the 0 -> nonempty transition set
            // the class bit (GRQ-held; see CLASS_NONEMPTY in sched.rs).
            let nr_old = self.nr_running.fetch_add(1, Ordering::AcqRel);
            if nr_old == 0 {
                crate::sched::sched::class_nonempty_set(
                    crate::sched::sched_hooks::CLASS_CFS_BIT,
                );
            }
            self.load_weight.fetch_add(se.load.weight, Ordering::AcqRel);

            // Update minimum vruntime
            self.update_min_vruntime();

            crate::sched::sched_hooks::ww_record(task, crate::sched::sched_hooks::WW_EV_LINK);

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
        {
            // INV-SCE-1: GRQ-held task (the caller holds the runqueue).
            let task_ref = crate::sched::edges::task_mut(task);
            let se = task_ref.sched_entity();

            // grq-10ns: linkage is PHYSICAL — the entity's embedded node
            // is in the tree (node.linked), and erase needs no key at
            // all (the node's position IS the linkage). The Leg-55
            // stored-key membership check and its O(n) heal scan are
            // gone: a scribbled key can no longer desynchronize "key
            // says linked" from "map holds the task" because both were
            // views of the same BTreeMap entry; the intrusive node has
            // exactly one linkage truth.
            if se.timeline_key().is_some() {
                let key = se.timeline_key().unwrap_or(VruntimeKey::new(0, 0));
                // 64th: vruntime sanity canary (see vr_canary).
                crate::sched::sched_hooks::vr_canary(self, task, b"DEQ", key.vruntime);
                // 64th (update_entity_lag parity, 6.6 fair.c:5534):
                // capture the clamped lag BEFORE the removal —
                // avg_vruntime still includes this entity, exactly like
                // the reference computes it inside dequeue_entity ahead of
                // __dequeue_entity. The next WAKE enqueue reads it once
                // (place at V − lag), bounding how far any sleeper — or
                // any heavy producer between naps — can land from the
                // pack to ±max(2*granularity, 1 tick) virtual time.
                {
                    let v = self.avg_vruntime() as i128;
                    let tick_ns = 1_000_000_000u64 / HZ;
                    let base = (2 * SCHED_MIN_GRANULARITY_NS).max(tick_ns);
                    let w = se.load.weight.max(1) as i128;
                    let limit = (base as i128 * NICE_0_LOAD as i128) / w;
                    let lag = (v - se.get_vruntime() as i128).clamp(-limit, limit);
                    se.vlag.set(lag as i64);
                }
                if !self.timeline_remove(&key, task) {
                    // Poison discipline refused the unlink: the node
                    // stays linked; leave the counters paired with the
                    // timeline and report the refusal upstream.
                    return false;
                }

                // Update status
                se.set_on_rq(false);

                // Update task count and total weight
                self.nr_running_dec();
                self.load_weight_sub(se.load.weight);

                // Update minimum vruntime
                self.update_min_vruntime();

                return true;
            }

            false
        }
    }

    /// R41: whether `task` is ACTUALLY linked on this queue — as
    /// opposed to what the entity's on_rq flag claims. Used by
    /// __schedule's prev requeue to distinguish a legitimately-linked
    /// prev (racing wake during a prepare-to-wait recheck window) from a
    /// stale on_rq=true flag.
    ///
    /// INVARIANT: the timeline (including this read) is only touched
    /// with the GRQ lock held (the DEBUG taskdump's try-lock calls obey
    /// the same rule). grq-10ns: linkage is the entity node's physical
    /// `linked` state — exactly "timeline_insert succeeded and no
    /// removal has run since" — visible as the stored ORDER key being
    /// Some (set/cleared in lockstep with the node link). Same
    /// authority as the old key+map-value identity check, in one load.
    pub fn is_linked(&self, task: *mut crate::process::Task) -> bool {
        if task.is_null() {
            return false;
        }
        // SAFETY: task is a valid Task pointer (null-checked above) and
        // the timeline discipline covers its sched_entity access.
        let se = crate::sched::edges::task_shared(task).sched_entity();
        se.timeline_key().is_some()
    }

    /// r-strand face (a) heal (lat_fs conviction, 2026-11-19): counter/
    /// timeline desync — the GRQ counter reads 0 while the timeline
    /// still holds entries, so every idle CPU WFI-halts on
    /// nr_running==0 and never attempts the pick (the R20-2 recheck is
    /// counter-gated): the stranded entry starves with all CPUs idle.
    /// Recount the timeline and resync the counter + class bit when
    /// they disagree. GRQ-HELD by the caller; 1 Hz cadence (the phantom
    /// sweep) keeps the walk cost out of the hot path. Returns
    /// (was, now) for the heal report, or None when paired.
    pub fn resync_nr_running_from_timeline(&mut self) -> Option<(u64, u64)> {
        let truth = self.timeline_head_diag(&mut []) as u64;
        let was = self.nr_running.load(Ordering::Acquire);
        if truth == was {
            return None;
        }
        self.nr_running.store(truth, Ordering::Release);
        if truth > 0 {
            crate::sched::sched::class_nonempty_set(
                crate::sched::sched_hooks::CLASS_CFS_BIT,
            );
        } else {
            crate::sched::sched::class_nonempty_clear(
                crate::sched::sched_hooks::CLASS_CFS_BIT,
            );
        }
        Some((was, truth))
    }

    /// nanosleep-loss lane (hunt instrument): bounded in-order walk of
    /// the timeline filling `rows` with (pid, vruntime, on_cpu, claim)
    /// in PICK ORDER (leftmost first) — the traversal the pick uses, so
    /// a pid absent from these rows while its stored key says Some is a
    /// node the pick can never reach. Returns the TOTAL number of nodes
    /// reachable by the walk (capped at 999). GRQ-HELD by the caller.
    pub fn timeline_head_diag(
        &mut self,
        rows: &mut [(u32, u64, bool, i32)],
    ) -> usize {
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        let mut total = 0usize;
        let mut filled = 0usize;
        while node != nil && !node.is_null() && RbTimeline::node_ok(node) && total < 999 {
            let t = task_of_node(node);
            if !t.is_null() && filled < rows.len() {
                // INV-SCE-2 diag: the packed {on_cpu, claim} qword read.
                let q = crate::sched::edges::task_ownership_qword(t);
                rows[filled] = (
                    crate::sched::edges::task_shared(t).pid(),
                    crate::sched::edges::task_shared(t).sched_entity().get_vruntime(),
                    (q & 0xFF) != 0,
                    (q >> 32) as i32,
                );
                filled += 1;
            }
            // SAFETY: linked, in-range node.
            node = self.timeline.next(node);
            total += 1;
        }
        total
    }

    /// 60th (RUNQ-STRAND census): bounded in-order walk of the timeline
    /// naming the WALL below a stranded victim — every entry whose key
    /// sorts before the victim's stored key is consulted by EVERY pick
    /// before the victim is reached, so a wall of entries that refuse
    /// their picks (affinity pinning, ownership marks, throttling)
    /// delays the victim; past 32 stash slots the pick gives up entirely
    /// and the victim (and everything above it) is never visited.
    ///
    /// Returns (entries_below_victim, total_entries, rows_filled).
    /// `rows` is filled with the EARLIEST wall entries in pick order
    /// (their vruntime, pid, policy, on_cpu, claim, affinity mask) —
    /// the census prints them so the refusal shape is named in the
    /// same line as the victim. GRQ-HELD. Read-only: the tree is not
    /// modified; only a stable snapshot of POD fields is copied.
    /// 61st: `state_hist` (3 cells) collects the wall's task-state
    /// histogram [RUNNING, sleeping(INTERRUPTIBLE|UNINTERRUPTIBLE|
    /// STOPPED-ish low bits), other] when non-NULL.
    pub fn strand_wall_probe(
        &mut self,
        victim: *mut crate::process::Task,
        rows: &mut [StrandWallRow],
        state_hist: &mut [usize; 3],
    ) -> (usize, usize, usize) {
        let victim_key = if victim.is_null() {
            None
        } else {
            // SAFETY: GRQ-held timeline discipline covers the SE access.
            crate::sched::edges::task_shared(victim).sched_entity().timeline_key()
        };
        let mut below = 0usize;
        let mut total = 0usize;
        let mut filled = 0usize;
        let mut wall_running = 0usize;
        let mut wall_sleeping = 0usize;
        let mut wall_other = 0usize;
        // grq-10ns: in-order successor walk (was the BTreeMap iter).
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        while node != nil
            && !node.is_null()
            && RbTimeline::node_ok(node)
        {
            let k = node_key(node);
            let t = task_of_node(node);
            total += 1;
            let is_below = match victim_key {
                Some(vk) => k < vk,
                None => true, // victim not in tree: everything is "before" it
            };
            if is_below {
                below += 1;
                // 61st (ohw12 ghost class): the wall's TASK-STATE
                // histogram — a wall of PARKED (INTERRUPTIBLE) entries
                // that never run names the dequeue-integrity bug (a
                // sleeper left linked = a permanent left-edge wall every
                // pick must digest before reaching real work); a wall of
                // RUNNING entries points at the pick-refusal class
                // instead.
                let st = crate::sched::edges::task_shared(t).state().bits();
                if st & 0x3 != 0 {
                    wall_sleeping += 1;
                } else if st == 0 {
                    wall_running += 1;
                } else {
                    wall_other += 1;
                }
                if filled < rows.len() {
                    // SAFETY: the map holds valid Task pointers; the GRQ
                    // we hold pins them against dequeue for the read.
                    let tr = crate::sched::edges::task_shared(t);
                    let (policy, on_cpu, claim, aff) = (
                        tr.policy() as u8 as u64,
                        tr.on_cpu(),
                        tr.running_on_cpu.load(Ordering::Acquire),
                        tr.cpus_allowed(),
                    );
                    // 61st: refusal history of the wall entry itself.
                    // 63rd: read from the PERSISTENT per-task counters
                    // (armed on fuxstuck boots too) — the SKIP_HIST
                    // side-table slots were stolen by churn and read
                    // (0,0) on every fuxstuck-only boot. why = the
                    // reason code with the highest count (1 on_cpu-mark,
                    // 2 foreign-claim, 3 affinity, 4 throttle, 5 heal).
                    let (visits, why) = {
                        let tr = crate::sched::edges::task_shared(t);
                        let v = tr.pick_visit_n.load(Ordering::Relaxed) as u32;
                        let mut best = 0u64;
                        let mut bi = 0u32;
                        for (i, c) in tr.pick_refuse_why.iter().enumerate() {
                            let c = c.load(Ordering::Relaxed);
                            if c > best {
                                best = c;
                                bi = i as u32 + 1;
                            }
                        }
                        (v, bi)
                    };
                    // 63rd: slot ownership + last-pick/wake counts at
                    // probe time (the lying-slot refusal input).
                    let (slot, lp_jif, wakes) = {
                        let tr = crate::sched::edges::task_shared(t);
                        let mut sl: i32 = -1;
                        for c in 0..crate::config::MAX_CPUS {
                            if crate::sched::sched::cpu_state(c).current == t {
                                sl = c as i32;
                                break;
                            }
                        }
                        let last = tr.pick_last_jif.load(Ordering::Relaxed);
                        let now = crate::timer::get_jiffies();
                        let lp = if last == 0 {
                            u32::MAX
                        } else {
                            (now.saturating_sub(last)).min(u32::MAX as u64) as u32
                        };
                        (
                            sl,
                            lp,
                            tr.wake_n.load(Ordering::Relaxed) as u32,
                        )
                    };
                    rows[filled] = StrandWallRow {
                        vruntime: k.vruntime,
                        pid: crate::sched::edges::task_shared(t).pid(),
                        policy,
                        on_cpu,
                        claim,
                        aff,
                        skips: visits,
                        why,
                        slot,
                        lp_jif,
                        wakes,
                    };
                    filled += 1;
                }
            }
            // Advance the successor chain (grq-10ns walk).
            node = self.timeline.next(node);
        }
        state_hist[0] = wall_running;
        state_hist[1] = wall_sleeping;
        state_hist[2] = wall_other;
        (below, total, filled)
    }

    /// Shared heal-report rate gate: at most one heal line per 2 jiffies
    /// system-wide (the picking CPUs can pass a healing candidate hot while
    /// a queue unwedges). R34 discipline: callers hold the GRQ lock.
    /// s16 (WW5): the counters below are exact and unconditional — only
    /// the PRINT rides this budget.
    pub fn heal_report_budget_ok() -> bool {
        static LAST_REPORT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let now = crate::timer::get_jiffies();
        let last = LAST_REPORT.load(core::sync::atomic::Ordering::Acquire);
        if now.wrapping_sub(last) < 2 {
            return false;
        }
        LAST_REPORT
            .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
            .is_ok()
    }

    /// One heal line prefix: TAG by_cpu=N pid=P from_claim=C[ pk=K] (no
    /// trailing newline — callers append their context tail first).
    pub fn stale_heal_line(tag: &[u8], cpu_id: usize, pid: u32, was: i32, pk: u32) {
        use crate::debug::taskdump::putc_brief as putchar;
        for &b in tag {
            putchar(b);
        }
        const MB: &[u8] = b" by_cpu=";
        for &b in MB {
            putchar(b);
        }
        Self::put_dec(cpu_id as u64);
        const M2: &[u8] = b" pid=";
        for &b in M2 {
            putchar(b);
        }
        Self::put_dec(pid as u64);
        const M3: &[u8] = b" from_claim=";
        for &b in M3 {
            putchar(b);
        }
        if (was as i64) < 0 {
            putchar(b'-');
        }
        Self::put_dec((was as i64).unsigned_abs());
        if pk != u32::MAX {
            const M4: &[u8] = b" pk=";
            for &b in M4 {
                putchar(b);
            }
            Self::put_dec(pk as u64);
        }
    }

    /// GNOME-wedge report for the stale-claim heal inside pick_next_cpu
    /// (R34: GRQ-held callers, direct console writes only).
    /// 63rd: `task` added for the persistent ring (the print can be
    /// budget-swallowed; the ring cannot).
    fn stale_claim_healed(cpu_id: usize, pid: u32, was: i32, task: *mut crate::process::Task) {
        crate::sched::sched::ww_claim_heal_inc();
        if crate::sched::sched::sched_hist_armed() && !task.is_null() {
            crate::sched::sched::sched_hist_ring(
                crate::sched::edges::task_shared(task),
                4,
                2,
                (cpu_id as u64 & 0xF) | (((was as u64) & 0xF) << 4),
            );
        }
        if !Self::heal_report_budget_ok() {
            return;
        }
        Self::stale_heal_line(b"STALE-CLAIM-RESET", cpu_id, pid, was, u32::MAX);
        // wake-fastrepro correlation: the victim's runtime so far (0 =
        // never scheduled: recycle-residue class) and the jiffy stamp —
        // pairs with ENQ-CLAIMED-SLOTBK jif= lines (same pid+claim).
        if !task.is_null() {
            use crate::debug::taskdump::putc_brief as putchar;
            fn pdec5(mut v: u64) {
                use crate::debug::taskdump::putc_brief as p;
                let mut buf = [0u8; 20];
                let mut n = 0;
                if v == 0 {
                    p(b'0');
                    return;
                }
                while v > 0 && n < buf.len() {
                    buf[n] = b'0' + (v % 10) as u8;
                    v /= 10;
                    n += 1;
                }
                for i in (0..n).rev() {
                    p(buf[i]);
                }
            }
            const M: &[u8] = b" ticks=";
            for &b in M {
                putchar(b);
            }
            pdec5(crate::sched::edges::task_shared(task).utime_ticks.load(Ordering::Relaxed)
                + crate::sched::edges::task_shared(task).stime_ticks.load(Ordering::Relaxed));
            const M2: &[u8] = b" jif=";
            for &b in M2 {
                putchar(b);
            }
            pdec5(crate::timer::get_jiffies());
            // pick→switch ledger: an intact publication record for this
            // victim = the claim came from a pick that was never
            // affirmed (switched to) NOR aborted — the switchless pick,
            // named with its publisher and age.
            if let Some((pcpu, ppk, pjif)) =
                crate::sched::sched::psledger_peek(task)
            {
                const A: &[u8] = b" PSLEDGER=SWITCHLESS pub_cpu=";
                for &b in A {
                    putchar(b);
                }
                pdec5(pcpu as u64);
                const B: &[u8] = b" pub_pk=";
                for &b in B {
                    putchar(b);
                }
                pdec5(ppk as u64);
                const C: &[u8] = b" age_jif=";
                for &b in C {
                    putchar(b);
                }
                pdec5(crate::timer::get_jiffies().wrapping_sub(pjif));
            }
        }
        crate::debug::taskdump::putc_brief(b'\n');
    }

    /// R85 (tail-phase PICK-SKIP wedge): always-on, rate-limited report
    /// for the stale on_cpu heal (pick-side leg passes the classic tag,
    /// the tick-side CLAIM-SWEEP passes its own) — the released FULL qword is
    /// the store __switch_to would have made; the print names the victim
    /// (pid, state word, ti_cpu), the pre-heal claim, the advisory
    /// pick-state and the owner masks, so a capture can tell a lost
    /// release (slot=0 swout=0) from a lying-slot double-run fight
    /// (marks flipping between two cpus' claims). Same R34 discipline
    /// and 2s system-wide budget as the claim twin.
    fn stale_oncpu_healed(tag: &[u8], cpu_id: usize, task: *mut crate::process::Task, claim: i32, pk: u32) {
        crate::sched::sched::ww_oncpu_heal_inc();
        // 63rd: the heal event goes on the task's ring BEFORE the print
        // budget can swallow the line (a budgeted-out heal must still
        // appear in the victim's history).
        if crate::sched::sched::sched_hist_armed() && !task.is_null() {
            crate::sched::sched::sched_hist_ring(
                crate::sched::edges::task_shared(task),
                5,
                1,
                (cpu_id as u64 & 0xF) | (((claim as u64) & 0xF) << 4) | (((pk as u64) & 0xF) << 8),
            );
        }
        if !Self::heal_report_budget_ok() {
            return;
        }
        // SAFETY: diagnostic reads of stable Task fields; the task is
        // linked on this queue (pinned by the GRQ lock the caller holds).
        {
            let tr = crate::sched::edges::task_shared(task);
            let pid = tr.pid();
            let st = tr.state().bits();
            let tcpu = tr.ti_cpu() as i64;
            let slot = crate::sched::sched::owner_slot_mask(task);
            let swout = crate::sched::sched::switching_out_mask(task);
            Self::stale_heal_line(tag, cpu_id, pid, claim, pk);
            // Context tail: st= state word, tcpu= ti_cpu, slot=/swout=
            // owner masks at heal time (both must be 0 — that is the
            // adjudication's precondition; a nonzero mask here names a
            // racing owner).
            use crate::debug::taskdump::putc_brief as putchar;
            const M: &[u8] = b" st=0x";
            for &b in M {
                putchar(b);
            }
            let mut v = st as u64;
            for sh in (0..8u32).rev() {
                let nb = ((v >> (sh * 4)) & 0xF) as u8;
                putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            }
            const M2: &[u8] = b" tcpu=";
            for &b in M2 {
                putchar(b);
            }
            if tcpu < 0 {
                putchar(b'-');
            }
            Self::put_dec(tcpu.unsigned_abs());
            const M3: &[u8] = b" slot=";
            for &b in M3 {
                putchar(b);
            }
            Self::put_dec(slot as u64);
            const M4: &[u8] = b" swout=";
            for &b in M4 {
                putchar(b);
            }
            Self::put_dec(swout as u64);
            putchar(b'\n');
        }
    }

    /// Decimal printer for the heal reports (R34: direct console writes).
    pub fn put_dec(v: u64) {
        use crate::debug::taskdump::putc_brief as putchar;
        if v == 0 {
            putchar(b'0');
            return;
        }
        let mut buf = [0u8; 20];
        let mut n = 0;
        let mut v = v;
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
    }

    /// rux:claiminject (retired debug=claiminject) adjudication scan (GNOME-wedge hunt): poison every
    /// linked task that currently shows the fully-pickable ownership state
    /// {on_cpu=false, claim=-1} with a claim from CPU 7 — a CPU that does
    /// not exist on any board we boot, so no picker can ever own it. On an
    /// unpatched kernel every poisoned task strands exactly like the g2
    /// freeze (linked, counted, skipped forever); with the pick-side
    /// stale-claim heal the poison is swept at the next pick. Caller holds
    /// the GRQ lock.
    pub fn claim_inject_scan(&mut self) -> usize {
        let mut n = 0;
        // grq-10ns: in-order successor walk (was the BTreeMap iter).
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        while node != nil && !node.is_null() && RbTimeline::node_ok(node) {
            let task = task_of_node(node);
            // SAFETY: diagnostic write of one atomic field on a task pinned
            // in the timeline by the caller's GRQ lock.
            {
                let tr = crate::sched::edges::task_shared(task);
                if !tr.on_cpu()
                    && tr.running_on_cpu.load(Ordering::Acquire) == -1
                {
                    tr.running_on_cpu.store(7, Ordering::Release);
                    n += 1;
                }
            }
            node = self.timeline.next(node);
        }
        n
    }

    /// rux:oncpuinject (retired debug=oncpuinject) adjudication scan (R85 tail-wedge hunt): stamp the
    /// FULL lost-release shape — {on_cpu=true, claim=7, pk=LIVE} via
    /// publish_pick_ownership — onto every linked task currently showing
    /// the pickable {on_cpu=false, claim=-1}. CPU 7 exists on no board we
    /// boot, so no slot can ever back the claim: the state is exactly the
    /// terminal microstate of the pid-1 tail wedge (linked, marked, owned
    /// by neither a slot nor an in-flight switch). An unpatched kernel
    /// strands every poisoned task forever (picks skip it why=1); with the
    /// R85 pick-side ownership adjudication the poison is swept within
    /// ~50 jiffies (STALE-ONCPU-RESET). Caller holds the GRQ lock.
    pub fn oncpu_inject_scan(&mut self) -> usize {
        let mut n = 0;
        // grq-10ns: in-order successor walk (was the BTreeMap iter).
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        while node != nil && !node.is_null() && RbTimeline::node_ok(node) {
            let task = task_of_node(node);
            // SAFETY: diagnostic publish on a task pinned in the timeline
            // by the caller's GRQ lock. Only tasks with a fully released
            // qword are stamped, so no legitimate in-flight ownership is
            // overwritten (running/picked tasks are not linked).
            {
                let tr = crate::sched::edges::task_shared(task);
                if !tr.on_cpu()
                    && tr.running_on_cpu.load(Ordering::Acquire) == -1
                {
                    tr.publish_pick_ownership(7, true);
                    n += 1;
                }
            }
            node = self.timeline.next(node);
        }
        n
    }

    /// OH-launcher wall (2026-10-31, lb20/lb21): the pick-side stale-claim
    /// heal only fires when a pick WALK examines the victim — and the walk
    /// scans leftmost-first, stopping at the first allowed candidate. A
    /// stale-claimed victim sitting mid-tree (its vruntime is frozen at the
    /// strike; the pack has to advance past it for it to drift leftmost)
    /// is skipped by every walk without ever being EXAMINED, so
    /// STALE-CLAIM-RESET never fires for it. lb20's softbus_server (pid
    /// 724) went silent at registration (228.8s) with exactly one reset at
    /// 262s and the next at 841s — a 579s hole during which its worker
    /// threads were never swept at all, while device_manager's code-3
    /// handler burned its 5.24s softbus timeout 102 times and
    /// bootevent.account.ready never landed. accesstoken (17 resets) and
    /// device_manager's own worker (16) show the same repeat-victim
    /// pattern: the phantom-mark producer strikes, the victim strands for
    /// as long as it takes to drift leftmost.
    ///
    /// This sweep makes the heal POSITION-INDEPENDENT: run from the timer
    /// tick (once per 100 jiffies, one CPU), it walks the WHOLE timeline
    /// under the GRQ and applies the exact pick-side adjudication to every
    /// linked task — {on_cpu=false, claim!=-1} owned by no CPU slot and no
    /// in-flight switch-out is protocol-impossible (an in-flight pick
    /// unlinks and publishes under one GRQ hold, so it cannot interleave
    /// this scan; a mid-switch-out task carries on_cpu=TRUE until the
    /// full-qword release store, which cannot leave a half-written claim),
    /// so the claim is scribble residue and resetting it makes the task
    /// pickable where it stands. CFS ordering then schedules it within the
    /// normal service latency instead of after a leftmost drift measured
    /// in hundreds of seconds. Caller holds the GRQ lock.
    ///
    /// Returns the census tuple (linked, stale_reset, owned_claim,
    /// oncpu_marked, oncpu_pids, owned_pids) — printed by the tick hook
    /// (rux:wwcensus (retired debug=wwcensus)). owned_claim counts the lying-slot shape (a live
    /// slot names the task): NOT healable here (that adjudication needs
    /// the frozen-exec-clock conviction against the SLOT's liveness, not
    /// just the task's), but the census names its pids so the next
    /// capture can size it.
    pub fn claim_sweep(
        &self,
        cpu_id: usize,
    ) -> (usize, usize, usize, usize, [u32; 4], [u32; 4]) {
        let mut linked = 0usize;
        let mut stale_reset = 0usize;
        let mut owned_claim = 0usize;
        let mut oncpu_marked = 0usize;
        let mut oncpu_pids = [0u32; 4];
        let mut owned_pids = [0u32; 4];
        // grq-10ns: in-order successor walk (was the BTreeMap iter).
        // node_ok IS the b50 #GP guard: a scribbled timeline link must
        // not reach the derefs below. The successor is computed BEFORE
        // the body so the body's `continue` statements (inherited from
        // the iterator form) still advance the walk.
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        loop {
            if !(node != nil && !node.is_null() && RbTimeline::node_ok(node)) {
                break;
            }
            let task = task_of_node(node);
            // SAFETY: node is linked and in-range; next() reads only
            // link fields.
            node = self.timeline.next(node);
            // SAFETY: task is from the timeline, a valid pointer stored
            // during enqueue; the caller's GRQ hold pins the timeline and
            // excludes every concurrent pick publish / switch-out release.
            {
                let tr = crate::sched::edges::task_shared(task);
                if tr.on_cpu() {
                    // Legitimate mid-switch-out / fast-repick transient,
                    // or an orphaned on_cpu mark. The idle-loop harvester
                    // owns this shape but only runs when a CPU goes IDLE
                    // — under the OH load collapse the CPUs never idle,
                    // and the pick-side stale-oncpu heal is as
                    // order-dependent as the claim leg was. Sweep it with
                    // the R80 discipline: no live owner (slot /
                    // SWITCHING_OUT) under this GRQ hold, PLUS the
                    // 50-jiffy exec-frozen orphan-watch confirmation, then
                    // release the FULL qword — the exact store
                    // __switch_to would have made. A legitimately
                    // mid-switch-out task (microseconds) never matures
                    // the watch; a LIVE-published fast repick keeps its
                    // picker's slot naming it (owner_live) and is never
                    // touched.
                    let owner_live = crate::sched::sched::pick_owner_live(task);
                    if !owner_live
                        && crate::sched::sched::stale_mark_confirm(task, tr.pid(), 50)
                    {
                        // TORN re-read guard (the why=2 pick-walk twin's
                        // discipline, smp8 orphan-race): decode the pair
                        // from ONE atomic qword load, re-read it after the
                        // owner scan + orphan-watch confirmation, and heal
                        // ONLY if it still shows the exact same pair — the
                        // GRQ-free asm switch-out release store can land
                        // under this scan, and releasing on a moved qword
                        // would wipe evidence a live owner just published.
                        let whole = crate::sched::edges::task_ownership_qword(task);
                        let w_oncpu = (whole & 0xFF) != 0;
                        let claim = (whole >> 32) as i32;
                        if !w_oncpu {
                            // The mark cleared under us (a real switch-out
                            // release landed): already healed, nothing to do.
                            oncpu_marked += 1;
                            if let Some(slot) = oncpu_pids.iter_mut().find(|p| **p == 0) {
                                *slot = tr.pid();
                            }
                            continue;
                        }
                        let was_pk = tr.pick_state();
                        tr.release_pick_ownership();
                        stale_reset += 1;
                        crate::sched::sched_hooks::stale_oncpu_healed(
                            b"SWEEP-ONCPU-RESET",
                            cpu_id,
                            task,
                            claim,
                            was_pk,
                        );
                        continue;
                    }
                    oncpu_marked += 1;
                    if let Some(slot) = oncpu_pids.iter_mut().find(|p| **p == 0) {
                        *slot = tr.pid();
                    }
                    continue;
                }
                let claim = tr.running_on_cpu.load(Ordering::Acquire);
                if claim == -1 {
                    linked += 1; // fully released: pickable class
                    continue;
                }
                linked += 1;
                // {on_cpu=false, claim!=-1}: pickable class UNLESS a live
                // owner backs the claim. Same adjudication as the pick
                // walk's R85/R74 legs: slots and SWITCHING_OUT are
                // scheduler-truth, and under this GRQ hold a genuinely
                // owned task is always named by one of them.
                if crate::sched::sched::pick_owner_live(task) {
                    owned_claim += 1;
                    if let Some(slot) = owned_pids.iter_mut().find(|p| **p == 0) {
                        *slot = tr.pid();
                    }
                    continue;
                }
                tr.running_on_cpu.store(-1, Ordering::Release);
                stale_reset += 1;
                crate::sched::sched::ww_claim_heal_inc();
                if Self::heal_report_budget_ok() {
                    Self::stale_heal_line(b"SWEEP-CLAIM-RESET", cpu_id, tr.pid(), claim, u32::MAX);
                    crate::debug::taskdump::putc_brief(b'\n');
                }
            }
        }
        (linked, stale_reset, owned_claim, oncpu_marked, oncpu_pids, owned_pids)
    }

    /// Pick next task to run
    ///
    /// Select task with smallest vruntime
    ///
    /// # Returns
    /// Next task pointer to run, or None if queue is empty
    pub fn pick_next(&mut self) -> Option<*mut crate::process::Task> {
        // Get task with smallest vruntime (O(1) via the cached leftmost
        // node — the reference's rb_leftmost)
        if let Some((key, task)) = self.leftmost_entry() {
            // 64th: vruntime sanity canary (see vr_canary).
            crate::sched::sched_hooks::vr_canary(self, task, b"PK1", key.vruntime);
            // b50 #GP guard family — a non-canonical pointer must never
            // reach the derefs below. grq-10ns: with the intrusive
            // timeline the entry cannot be unlinked (the link fields live
            // inside the garbage) — refuse the pick, keep the kernel
            // alive, and let the census name the wedge; the counters stay
            // paired with the still-linked entry.
            if !crate::sched::sched::kernel_task_ptr_range_ok(task as *const u8) {
                crate::sched::sched::tl_poison_erase_inc();
                crate::sched::sched::report_poisoned_rq_entry(usize::MAX, task as usize);
                return None;
            }
            // Remove from queue (keyless unlink — don't call dequeue, as
            // vruntime may have changed)
            self.timeline_remove(&key, task);

            // SAFETY: task was in the timeline from a valid &mut Task
            // pointer; just removed, no aliasing references exist.
            {
                // Update status (INV-SCE-1: GRQ-held task).
                let task_ref = crate::sched::edges::task_mut(task);
                let se = task_ref.sched_entity();
                se.set_on_rq(false);

                // Update task count and total weight
                self.nr_running_dec();
                self.load_weight_sub(se.load.weight);

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
        let nil = self.timeline.nil();
        let node = self.timeline.leftmost;
        if node != nil && !node.is_null() && RbTimeline::node_ok(node) {
            return Some(task_of_node(node));
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

        // SAFETY: self.curr is set by set_curr() to a valid Task pointer; null check above.
        {
            // INV-SCE-1: the GRQ-held curr slot owner.
            let task = crate::sched::edges::task_mut(self.curr);
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
        // Garbage tripwire (regression guard): with the checked-sub
        // discipline the count can no longer wrap; anything beyond any
        // possible task bound is a corrupted read of the counter itself
        // (the 2b092c49 unexcluded-section family could produce it) —
        // name it once per boot.
        if nr_running > 1_000_000 {
            static GARBAGE_SEEN: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            if !GARBAGE_SEEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
                crate::pr_warn!(
                    "sched_slice: nr_running={} is impossible (corrupted read)",
                    nr_running
                );
            }
        }
        let sched_period = if nr_running > SCHED_LATENCY_NS / SCHED_MIN_GRANULARITY_NS {
            // Saturating: a garbage/huge count must not panic the tick —
            // belt-and-braces for nr_running_dec (see its doc).
            SCHED_MIN_GRANULARITY_NS.saturating_mul(nr_running)
        } else {
            SCHED_LATENCY_NS
        };

        // Calculate time slice
        // slice = period * weight / total_weight
        let load_weight = self.load_weight.load(Ordering::Acquire);

        if load_weight == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // Multiply first, then divide to avoid integer truncation to 0.
        // This matches Linux's approach (period * weight / total_weight).
        // grq-hold-slim Cut 1: 64-bit form (the reference's own
        // arithmetic class — fair.c computes the slice with 64-bit
        // multiply/divide). The u128 lowering pulled a software 128-bit
        // divide into the GRQ-held pick on every switch; every real
        // input is far below overflow (sched_period <= ~3 s even at the
        // garbage-capped count, weight <= ~87x NICE_0_LOAD, product
        // <= ~2.7e17 << u64::MAX), and counts beyond the 1e6 tripwire
        // above are already reported, so saturating_mul returns the
        // exact u128 value wherever the value is meaningful.
        let slice = sched_period.saturating_mul(se.load.weight) / load_weight;

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
        // Drain the intrusive timeline: walk the successor chain
        // (computing each node's successor BEFORE detaching it), then
        // clear the task-side state (on_rq, ORDER key) and detach the
        // node in place.
        let nil = self.timeline.nil();
        let mut node = self.timeline.leftmost;
        while node != nil && !node.is_null() && RbTimeline::node_ok(node) {
            // SAFETY: linked, in-range node.
            let next = self.timeline.next(node);
            let task = task_of_node(node);
            if !task.is_null() {
                // SAFETY: task is from the queue, a valid pointer; clear()
                // drains the entire queue.
                {
                    let task_ref = crate::sched::edges::task_mut(task);
                    task_ref.sched_entity().set_on_rq(false);
                    task_ref.sched_entity().set_timeline_key(None);
                }
            }
            // INV-SCE-6: detach resets this node's link fields only (the
            // successor was captured above; the tree below/around is
            // untouched).
            RbNode::detach_at(node, nil);
            node = next;
        }
        self.timeline.root = nil;
        self.timeline.leftmost = nil;
        self.curr = core::ptr::null_mut();
        self.nr_running.store(0, Ordering::Release);
        self.load_weight.store(0, Ordering::Release);
        // grq-hold-slim Cut 2: the drain emptied CFS — clear the class
        // bit (GRQ-held).
        crate::sched::sched::class_nonempty_clear(crate::sched::sched_hooks::CLASS_CFS_BIT);
    }
}

/// grq-10ns: poison-discipline trip counters for the intrusive timeline
/// (an insert descent or erase neighbor that failed the kernel-range
/// check — the op was refused to keep the kernel alive). Reported by
/// grq_diag_counts; a persistent nonzero rate names a scribble producer.
pub struct TlPoisonCounters {
    pub inserts: core::sync::atomic::AtomicU64,
    pub erases: core::sync::atomic::AtomicU64,
}

pub static TL_POISON: TlPoisonCounters = TlPoisonCounters {
    inserts: core::sync::atomic::AtomicU64::new(0),
    erases: core::sync::atomic::AtomicU64::new(0),
};

// ==================== Shared sched observability state ====================
// (sched-seam devirt lane, 2026-11-23): the per-switch diagnostic counters
// below are PLAIN STATE, not policy — no decision reads them; the census
// walks and /proc readers do. They originally moved into the rux-sched
// service crate with the S1b flip, which put every per-switch access
// through a SchedHooks fn-pointer row (wrapper -> OnceSlot read ->
// indirect call) — p2_ctxsw measured the layer at ~0.4 us per switch
// round (2.12-2.19 vs 1.708 same-window main, outside the +-20% sched
// band). The storage and its trivial accessors live HERE (framework)
// now; the rux-sched census code reads them through the normal
// crate::sched re-exports (the same visibility PICK_GEN always had).
// Same single instance, same stores at the same call sites — only the
// dispatch layer is gone.

/// r11: per-CPU pick-phase tracer table (pick_trace's key side).
pub static PT_TASK: [AtomicUsize; 32] = [const { AtomicUsize::new(0) }; 32];
pub static PT_CODE: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];
pub static PT_JIF: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

/// grq-hold-slim Cut 2: single-class-chain short-circuit mask — each
/// class's 0 <-> nonempty transition (GRQ-held) sets/clears its bit;
/// pick_next_task's class walk loads it once instead of per-class
/// is_empty() loads.
pub static CLASS_NONEMPTY: AtomicU32 = AtomicU32::new(0);

/// Wakewatch (s15 A2 idle-mark/WFI hunt): per-CPU idle-loop phase stamp,
/// written by the idle loop at each phase transition, read by the 1 Hz
/// heartbeat. Codes: 0 = not in the idle loop, 1 = entering schedule(),
/// 2 = idle post-schedule (idle-marked, R20-2 recheck window), 3 =
/// pre-WFI (uart poll), 4 = inside WFI, 5 = post-WFI accounting.
pub static WW_IDLE_PHASE: [AtomicU32; MAX_CPUS] =
    [const { AtomicU32::new(0) }; MAX_CPUS];

/// SP-5 r5 frozen-tick trap: per-CPU schedule() milestone inside the
/// IF=0 window that opens at the GRQ acquire. 0 = not inside
/// schedule()'s GRQ window; 1 = about to acquire (or spinning); 2 =
/// acquired, inside the CS; 3 = lock released IRQ-retained,
/// context-switch tail; 4 = past restore (immediately cleared).
pub static SCHED_CS_PHASE: [AtomicU32; MAX_CPUS] =
    [const { AtomicU32::new(0) }; MAX_CPUS];

/// runq-strand lane: per-CPU schedule()-entry count (one relaxed
/// fetch_add per schedule); the monopolist census reads its per-burst
/// delta (frozen = a CPU that stopped scheduling).
pub static SCHED_CALLS: [AtomicU64; MAX_CPUS] =
    [const { AtomicU64::new(0) }; MAX_CPUS];

/// 105th wake-dispatch census: the dispatch outcome at each wake (1 =
/// idle CPU claimed + IPI, 2 = no idle; sync wake won the vruntime
/// compare -> poke, 3 = sync compare lost -> park handoff, 4 = async
/// preempt check poked, 5 = async recency/compare gate -> no poke).
pub static WAKE_DISP: AtomicU32 = AtomicU32::new(0);

/// Same census: the idle-CPU mask latched at wake ENTRY (before the
/// claim), so "idle existed but claim lost" vs "no idle at all" are
/// told apart.
pub static WAKE_IDLE_MASK: AtomicU32 = AtomicU32::new(0);

/// Per-task schedstats runtime gate (/proc/sys/kernel/sched_schedstats,
/// default OFF like the reference). One relaxed load per schedule edge
/// when off.
pub static SCHEDSTATS: AtomicBool = AtomicBool::new(false);

/// s14/60th walk-census counters: CFS pick-walk length and the two
/// ownership-heal families. Reported by the census getters.
pub static WW_PICK_WALKS: AtomicU64 = AtomicU64::new(0);
pub static WW_ONCPU_HEALS: AtomicU64 = AtomicU64::new(0);
pub static WW_CLAIM_HEALS: AtomicU64 = AtomicU64::new(0);
