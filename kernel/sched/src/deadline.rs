//! The Deadline class (S1b sole copy): the DL policy over the
//! framework's DlRunQueue storage.

#![forbid(unsafe_code)]
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, AtomicI64, Ordering};
use crate::process::task::Task;
use crate::sched::class::{SchedClass, RunQueueRef};
use crate::sched::entities::{SchedEntity, SchedRtEntity, SchedDlEntity, LoadWeight, NICE_0_LOAD};
use crate::sched::entities::{DL_BW_UNIT, DL_DEFAULT_PERIOD_NS, DL_DEFAULT_RUNTIME_NS};



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
                    rux::api::sched::CLASS_DL_BIT,
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
        if crate::process::r#unsafe::borrow_task(task).dl_entity().on_rq.load(Ordering::Acquire) {
            return false;
        }

        // Caller guarantees `task` points to a valid, live Task that
        // remains allocated for the duration of this enqueue (the GRQ
        // hold pins it — INV-PROC-BORROW-1's GRQ arm). The Task is not
        // currently on any runqueue (precondition for enqueue).
        {
            let t = crate::process::r#unsafe::borrow_task(task);
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
                    rux::api::sched::CLASS_DL_BIT,
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
            let t = crate::process::r#unsafe::borrow_task(task);
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
            crate::process::r#unsafe::borrow_task(task).dl_entity().on_rq.store(false, Ordering::Release);

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
                let t = crate::process::r#unsafe::borrow_task(task);
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
                crate::process::r#unsafe::borrow_task(task).dl_entity().on_rq.store(false, Ordering::Release);

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

// Send/Sync impls live in unsafe.rs (this file carries zero unsafe).




// ============================================================================
// Deadline Scheduling Class
// ============================================================================

/// Deadline scheduling class
///
/// With the global RunQueue design, the actual DL enqueue/dequeue/pick
/// happens directly in sched.rs on GlobalRunQueue::dl_rq. The SchedClass
/// methods are simplified to no-ops or minimal stubs.
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

    /// Actual enqueue happens in sched.rs::enqueue_task_locked.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual dequeue happens in sched.rs::dequeue_task.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// Deadline tasks that yield update their deadline.
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Preemption checks happen in sched.rs::check_dl_preempt.
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

    /// DL load balancing is complex, skip for now
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// Deadline tasks are usually pinned
    fn select_task_rq(&self, _task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        cpu
    }

    /// Actual tick handling happens in sched.rs::scheduler_tick.
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// Minimal — DL runtime tracking happens in sched.rs::scheduler_tick.
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    /// Deadline tasks don't have fixed time slices
    fn get_rr_interval(&self, _rq: RunQueueRef, _task: *mut Task) -> u32 {
        0
    }

    /// Actual check happens in sched.rs via GlobalRunQueue::dl_rq.
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        false
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&crate::rt::RT_SCHED_CLASS)
    }
}

/// Global deadline scheduling class instance
pub static DL_SCHED_CLASS: DlSchedClass = DlSchedClass::new();

