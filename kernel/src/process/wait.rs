//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Wait Queue mechanism
//!
//! Core concepts:
//! - Wait queues implement process blocking and waking
//! - When a process needs to wait for a condition, it joins the wait queue and calls schedule()
//! - When the condition is met, wake_up() wakes waiting processes

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use crate::sync::spinlock::Spinlock;

use super::Task;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum WakeUpHint {
    /// Normal wake up
    Normal = 0,
    /// Async wake up (don't actually wake process, just mark)
    Async = 1,
    /// Synchronous wake (reference WF_SYNC): the waker and wakee are a
    /// handoff pair — the waker is about to block. The scheduler stacks
    /// the wakee with the waker (never dispatches it onto another busy
    /// CPU to run in parallel). Used by binder's sync-txn/reply wakes
    /// (wake_up_interruptible_sync in binder.c, 66th session parity).
    Sync = 2,
}

#[repr(C)]
pub struct WaitQueueEntry {
    /// Associated task
    task: *mut Task,
    /// Exclusive flag (WQ_FLAG_EXCLUSIVE)
    exclusive: bool,
    /// Whether woken
    woken: AtomicBool,
    /// The task's pid at registration time (wake-drop fix, R127):
    /// entries are matched by (task pointer, pid) so a Task page that
    /// was freed and RECYCLED by a new task cannot be mistaken for the
    /// old owner. Pids are unique per boot (monotonic until wraparound
    /// far beyond any session's spawn volume), so the pair is a sound
    /// incarnation discriminator; free_task_slot's TASK_POISON write
    /// also fails the compare, catching dead-but-not-reused pages.
    pid: u32,
}

impl WaitQueueEntry {
    /// Create new wait queue entry
    ///
    /// # Arguments
    /// * `task` - Associated task
    /// * `exclusive` - Whether exclusive mode (mutual exclusion)
    pub fn new(task: *mut Task, exclusive: bool) -> Self {
        // SAFETY: every constructor passes the parking (current) task,
        // which is alive by construction; the pid read cannot fault
        // (kernel linear map) and cannot race the task's own free (a
        // task cannot free itself while executing its own park path).
        let pid = if task.is_null() {
            0
        } else {
            unsafe { (*task).pid() as u32 }
        };
        Self {
            task,
            exclusive,
            woken: AtomicBool::new(false),
            pid,
        }
    }

    /// The registration-time pid (wake-drop fix R127 — see the field).
    pub fn registered_pid(&self) -> u32 {
        self.pid
    }

    /// Check if woken
    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }

    /// Mark as woken
    pub fn set_woken(&self) {
        self.woken.store(true, Ordering::Release);
    }

    /// Clear the woken flag (lost-wake retry window).
    ///
    /// W1 (sticky-skip lost wake): `wake_up` marks the entry woken BEFORE
    /// calling `wake_up_process`, and refuses later wakes for an entry that
    /// is already flagged. When the underlying `wake_up_process` REFUSES the
    /// transition (target between its prepare_to_wait and its own
    /// `__schedule` link-drop — the R42 window — or a guard divergence),
    /// the waker's one-shot token is consumed while the task may still end
    /// up asleep: its `__schedule` reads the still-sleeping state, dequeues
    /// it, and no future wake retries this entry. Clearing the flag on a
    /// refused wake keeps the entry eligible so the NEXT event re-attempts
    /// the wake (idempotent: a genuinely running task is refused again
    /// harmlessly). The sc74 launcher IPC wedge — binder server threads
    /// parked >240 s owing replies with empty todos — has exactly this
    /// signature shape, so this closes the retry path; the stall-scan
    /// census in ipc/binder.rs still names any residual live occurrence.
    pub fn clear_woken(&self) {
        self.woken.store(false, Ordering::Release);
    }

    /// Get associated task
    pub fn task(&self) -> *mut Task {
        self.task
    }

    /// Check if exclusive mode
    pub fn is_exclusive(&self) -> bool {
        self.exclusive
    }
}

#[repr(C)]
pub struct WaitQueueHead {
    /// Wait queue list
    /// Uses Vec to store waiting processes
    list: Spinlock<Vec<WaitQueueEntry>>,
    /// Entries currently on the list (leg-36 wake fast path).
    ///
    /// Maintained ONLY inside the list lock's critical sections
    /// (insertions increment, retains decrement by what they removed),
    /// so `0` is a lock-free proof that the list is empty — the same
    /// contract as Linux's `wq_has_sleeper()` reading `head->next`
    /// before taking the wait-queue lock. `wake_up` loads it with
    /// Acquire FIRST and returns without touching the irqsave lock
    /// when it reads 0: on a box where the consumer polls instead of
    /// sleeping (hilogd's drain loops, pbench socket pairs), that
    /// empty-wake lock was paid on every single event.
    ///
    /// Lost-wake safety (same argument as Linux's): a waker that
    /// reads 0 runs strictly before the sleeper's in-lock increment
    /// becomes visible; the sleeper's condition re-check AFTER
    /// prepare_to_wait (every wait site in this kernel re-checks —
    /// see wait_event and unix_wait_round) then observes the waker's
    /// condition mutation, which precedes its wake call. Either the
    /// waker sees the waiter, or the waiter sees the event.
    waiters: AtomicUsize,
}

// Safety: WaitQueueHead uses internal Mutex for synchronization.
// The *mut Task pointer is only dereferenced under scheduler lock protection.
unsafe impl Send for WaitQueueHead {}
unsafe impl Sync for WaitQueueHead {}

/// Total stale wait-queue entries evicted by the R127 wake-time screen
/// (wake-drop hunt diagnostic: a non-zero value on a boot means tasks
/// died with entries still parked — the exit-without-unwind leak class).
pub static WQ_STALE_EVICTED: AtomicU64 = AtomicU64::new(0);

/// Clamped decrement of a task's live-registration count (R127 helper).
/// The eviction path can name a Task page that was freed and RECYCLED —
/// its new incarnation owns the word now, and its count must never
/// underflow on the predecessor's behalf (a wrapped u32::MAX would pin
/// the exit-leak detector on that task forever). Clamps at zero.
///
/// # Safety
/// `t` points into the kernel linear map (same read-contract as the
/// wake-time staleness screen).
unsafe fn wq_regs_dec(t: *mut Task, n: u32) {
    use core::sync::atomic::Ordering as AO;
    let mut cur = (*t).wq_regs.load(AO::Relaxed);
    loop {
        let next = cur.saturating_sub(n);
        match (*t).wq_regs.compare_exchange_weak(
            cur,
            next,
            AO::Relaxed,
            AO::Relaxed,
        ) {
            Ok(_) => return,
            Err(v) => cur = v,
        }
    }
}

impl WaitQueueHead {
    /// Create new wait queue head
    pub const fn new() -> Self {
        Self {
            list: Spinlock::new(Vec::new()),
            waiters: AtomicUsize::new(0),
        }
    }

    /// Initialize wait queue head (runtime initialization)
    pub fn init(&self) {
        // Vec is automatically initialized
    }

    /// Add to wait queue
    ///
    /// # Arguments
    /// * `entry` - Wait queue entry
    pub fn add(&self, entry: WaitQueueEntry) {
        // Use lock_irqsave: wake_up is called from IRQ handlers
        let mut list = self.list.lock_irqsave();
        // R127 wake-drop hunt: per-task live-registration count (see
        // Task::wq_regs) — the exit-leak detector's datum.
        let t = entry.task();
        if !t.is_null() {
            // SAFETY: the entry's task is parking on this queue; the
            // count is a diagnostic mirror of the list we hold.
            unsafe { (*t).wq_regs.fetch_add(1, Ordering::Relaxed) };
        }
        // Non-exclusive entries added to head, exclusive entries added to tail
        if entry.is_exclusive() {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
        // In-section (see `waiters`): published before the lock release.
        self.waiters.fetch_add(1, Ordering::Release);
    }

    /// Remove from wait queue
    ///
    /// # Arguments
    /// * `task` - Task to remove
    ///
    /// Returns how many entries were dropped (wake-drop hunt: a remove
    /// that drops MORE entries than the caller registered — or drops
    /// entries on a queue the caller never parked on — is the
    /// pointer-recycling theft signature; Task pages are recycled after
    /// free, so a stale entry keyed by the old pointer matches the new
    /// incarnation's remove).
    ///
    /// R127: entries are matched by (task pointer, registration pid) —
    /// a recycled Task page (same address, new pid) no longer matches,
    /// so a live task's unwait cannot strip a dead predecessor's stale
    /// entries. Those are evicted lazily by [`Self::wake_up`]'s
    /// staleness screen instead.
    pub fn remove_count(&self, task: *mut Task) -> usize {
        // SAFETY: every caller passes its own (live, running) task;
        // reading its pid is a const offset read in the linear map.
        let pid = if task.is_null() {
            0
        } else {
            unsafe { (*task).pid() as u32 }
        };
        let mut list = self.list.lock_irqsave();
        let before = list.len();
        // R127 wake-drop hunt: mirror the per-task count for every entry
        // this remove actually drops (same (ptr, pid) match as the retain).
        let mut regs_dropped: u32 = 0;
        list.retain(|entry| {
            let hit = entry.task() == task && entry.registered_pid() == pid;
            if hit && !entry.task().is_null() {
                regs_dropped = regs_dropped.saturating_add(1);
            }
            !hit
        });
        let dropped = before - list.len();
        if regs_dropped > 0 {
            // SAFETY: task is the caller's own live task (see above).
            unsafe { wq_regs_dec(task, regs_dropped) };
        }
        if dropped > 0 {
            self.waiters.fetch_sub(dropped, Ordering::Release);
        }
        dropped
    }

    /// Remove from wait queue (drop count not needed).
    pub fn remove(&self, task: *mut Task) {
        let _ = self.remove_count(task);
    }

    /// Wake up processes in wait queue
    ///
    /// # Arguments
    /// * `mode` - Wake mode
    /// * `nr` - Number of processes to wake (0 means wake all)
    ///
    /// # Returns
    /// Actual number of processes woken
    ///
    /// LOCK ORDER (INV-LOCK-6, virtio-blk ABBA fix): this function takes the
    /// wait-queue lock and holds it across sched::wake_up_process for every
    /// woken waiter. Callers must therefore NOT hold any virtio driver lock
    /// (VIRTIO_PCI_BLK_LOCK, MMIO virtqueue lock, VIRTIO_*_PENDING) when
    /// calling wake_up/wake_up_all/add/prepare_to_wait/finish_wait: nesting
    /// a wait-queue lock inside a virtio lock is the BLK↔waitqueue ABBA that
    /// froze GNOME final6 with timer IRQs stopped. Drivers collect completed
    /// work under their locks and wake OUTSIDE them (timer.rs R12-3 pattern).
    pub fn wake_up(&self, mode: WakeUpHint, nr: usize) -> usize {
        // Lock-free empty fast path (leg-36): one Acquire load instead
        // of an irqsave lock/unlock pair for every wake of a queue
        // nobody sleeps on — the common case for polling consumers.
        // `waiters` is only moved inside the list lock's critical
        // sections, so 0 means the list is (or was, microseconds of
        // coherence ago, in which case the sleeper's condition
        // re-check owns the event) empty. See the field's doc block.
        if self.waiters.load(Ordering::Acquire) == 0 {
            return 0;
        }
        // Use lock_irqsave: this is called from interrupt handlers.
        let mut list = self.list.lock_irqsave();
        let mut awakened = 0;
        // Reference __wake_up_common budget discipline: ONLY exclusive
        // entries consume the nr_exclusive budget ("wake one" = wake ONE
        // EXCLUSIVE waiter); non-exclusive entries (poll/epoll
        // registrations) are woken on every pass WITHOUT consuming it —
        // so a data-arrival wake reaches every poller plus exactly one
        // exclusive reader. nr == 0 (wake_up_all) wakes EVERY entry,
        // exclusive or not (the old form stopped at the first exclusive
        // entry, which only worked because every registration was
        // non-exclusive; the AF_UNIX reader/writer queue split registers
        // exclusive waiters and needs the full reference semantics).
        let mut budget = if nr == 0 { usize::MAX } else { nr };

        // R12-1 (deferred-wake UAF — the definitive fix): wake WHILE STILL
        // HOLDING the queue lock. A not-yet-woken entry on the queue proves
        // its task has not passed finish_wait (which needs this same lock),
        // therefore has not returned to userspace, therefore has not
        // exited or been reaped — the entry's task pointer cannot go stale
        // under us. The old collect-then-drop-then-wake window let a
        // signal-woken sleeper run, exit and be FREED before our wake —
        // the wild-pointer enqueue and the S1 jump-to-user corruption.
        // Lock order queue->GRQ is safe: no GRQ-held path takes a
        // waitqueue lock (scheduler_tick polls the console queue BEFORE
        // acquiring the GRQ). The earlier PID-revalidation and reverted
        // RCU-wrap were interim measures.
        //
        // R127 AMENDMENT: the proof above covers tasks that unwind their
        // park through finish_wait/remove — but the OH wake-drop hunt
        // captured a poller thread that exited with its poll entries
        // still queued (no unwait ran; the entries then vanished only
        // via a recycled-page remove). Stale entries therefore EXIST;
        // the R127 screen below evicts them at wake time and the
        // (pointer, pid) key makes them unstealable meanwhile.
        //
        // R34: wake directly inside the iteration — the earlier
        // collect-into-a-Vec (heap allocation under this irqsave lock)
        // was pure overhead: waking here is equivalent (a woken task's
        // finish_wait on another CPU simply blocks on this lock, it
        // cannot mutate the list mid-iteration) and removes an in-lock
        // heap allocation from every event source's hot path.
        // Stale-entry eviction set (wake-drop fix R127): entries whose
        // task died without unwinding its park (observed on the OH input
        // hunt: a poller thread exited with its evdev poll entries still
        // queued — no finish_wait/remove ran, R12-1's liveness proof has
        // exactly this hole) keep naming a freed Task page. Once that
        // page is recycled the entry either strips ANOTHER task's
        // registrations (remove-by-pointer theft) or wakes the WRONG
        // (new) task. The registration-time pid (see WaitQueueEntry)
        // discriminates incarnations; mismatched entries are evicted
        // here, under the same lock, self-healing the queue at the next
        // wake. Fixed array, no heap: queues hold single-digit entries.
        let mut stale: [*mut Task; 16] = [core::ptr::null_mut(); 16];
        let mut stale_n: usize = 0;

        for entry in list.iter() {
            if budget == 0 {
                break;
            }

            // R127 staleness screen: one volatile pid read per entry
            // (the same offset wake_up_process reads anyway). A freed
            // Task carries TASK_POISON in pid; a recycled one carries
            // the NEW task's pid; both fail the compare.
            let entry_task = entry.task();
            let entry_stale = entry_task.is_null() || {
                // SAFETY: read at a compile-time Task offset inside the
                // kernel linear map (same contract as the poison probe
                // in Task::is_plausible_task_ptr — cannot fault).
                let cur_pid = unsafe { core::ptr::read_volatile(
                    (entry_task as *const u8).add(crate::process::task::task_offsets::TASK_PID)
                        as *const u32,
                ) };
                cur_pid != entry.registered_pid()
            };
            if entry_stale {
                if stale_n < stale.len() && !stale[..stale_n].contains(&entry_task) {
                    stale[stale_n] = entry_task;
                    stale_n += 1;
                }
                continue;
            }

            if !entry.is_woken() {
                entry.set_woken();

                let task = entry.task();
                if !task.is_null() {
                    // W1: a refused wake must not consume the entry's
                    // one-shot token — clear the flag so a later wake can
                    // retry (the task may still be heading into
                    // __schedule with a sleeping state).
                    // 66th: mode == Sync routes through the WF_SYNC wake
                    // (same transition, stacking dispatch — see
                    // Task::wake_up_impl).
                    let woke = if matches!(mode, WakeUpHint::Sync) {
                        crate::sched::wake_up_process_sync(task)
                    } else {
                        crate::sched::wake_up_process(task)
                    };
                    if !woke {
                        entry.clear_woken();
                        // Lost-park ring (dfx=lostpark): the refused-wake
                        // token return — pid, state bits, preempt_count.
                        if crate::dfx::lostpark::armed() {
                            let st = unsafe { (*task).state() };
                            let pc = unsafe { (*task).preempt_count() };
                            let pid = unsafe { (*task).pid() };
                            crate::dfx::lostpark::record(
                                4,
                                pid as u64,
                                st.bits() as u64,
                                pc as u32 as u64,
                                crate::drivers::timer::get_jiffies(),
                            );
                        }
                        // Do not count a refused wake as awakened: no task
                        // transition happened. Continue scanning so other
                        // entries on this queue still get their chance.
                        continue;
                    }
                }

                awakened += 1;

                if entry.is_exclusive() {
                    // Only an EXCLUSIVE entry consumes the budget (the
                    // reference's `if (ret && (flags & WQ_FLAG_EXCLUSIVE)
                    // && !--nr_exclusive) break;`).
                    budget -= 1;
                }
            } else {
                // sc100 (sticky-token lost wake): a woken-flagged entry
                // whose task is SLEEPING is a consumed one-shot token
                // whose wake never reached the task — the binder-stall
                // census captured the shape live (q=1 woken=1 while the
                // task sat INTERRUPTIBLE ∧ unlinked ∧ owned by no cpu
                // slot for minutes; every REKICK skipped the entry and
                // no other wake path exists). The token was spent, so
                // this queue will never deliver the task another wake
                // unless it is re-issued HERE. Re-issue it for sleeping
                // targets only: a genuinely running task's refusal is
                // benign (it either holds a live slot or the R46 heal
                // path owns it), and a freed target is caught by
                // wake_up_process's own poison/floor guards.
                let task = entry.task();
                if !task.is_null() {
                    // SAFETY: read-only state snapshot; wake_up_process's
                    // WP2/R15-6 guards handle stale task pointers.
                    let st = unsafe { (*task).state() };
                    if st.is_sleeping() || st.contains(crate::process::task::TaskState::STOPPED) {
                        if crate::sched::wake_up_process(task) {
                            awakened += 1;
                        }
                    }
                }
            }
        }

        // R127 eviction pass: drop the stale entries collected above
        // (still under the lock — no interleaving remove/add can race
        // it) and keep the `waiters` counter exact so the leg-36
        // fast path re-engages once the queue drains.
        if stale_n > 0 {
            let before = list.len();
            // Per-task count mirror: group the evicted entries' tasks so
            // each freed page's wq_regs decrements exactly once per
            // evicted entry naming it (the page may be recycled already —
            // the count then lands on the new incarnation, which the
            // reset-at-reuse list re-zeroes anyway; a DEAD-but-unfreed
            // page keeps an accurate count).
            let mut per_task: [(*mut Task, u32); 16] = [(core::ptr::null_mut(), 0); 16];
            let mut per_task_n = 0usize;
            list.retain(|e| {
                if stale[..stale_n].contains(&e.task()) {
                    let t = e.task();
                    if !t.is_null() {
                        if let Some(slot) =
                            per_task[..per_task_n].iter_mut().find(|(x, _)| *x == t)
                        {
                            slot.1 += 1;
                        } else if per_task_n < per_task.len() {
                            per_task[per_task_n] = (t, 1);
                            per_task_n += 1;
                        }
                    }
                    false
                } else {
                    true
                }
            });
            let evicted = before - list.len();
            for (t, n) in per_task[..per_task_n].iter() {
                // SAFETY: diagnostic count bookkeeping on a page that was
                // on this queue an instant ago (kernel linear map; the
                // same read-contract as the staleness screen above).
                unsafe { wq_regs_dec(*t, *n) };
            }
            if evicted > 0 {
                self.waiters.fetch_sub(evicted, Ordering::Release);
                WQ_STALE_EVICTED.fetch_add(evicted as u64, Ordering::Relaxed);
            }
        }
        drop(list);

        awakened
    }

    /// Wake all waiting processes (non-exclusive)
    pub fn wake_up_all(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 0)
    }

    /// 118th kick forensics: the queue's true state (waiters counter,
    /// per-entry task pid/state/woken/on_cpu) for the binder kick dump.
    /// Diagnostic only; takes the irqsave list lock briefly.
    pub fn debug_entries(&self) -> (usize, Vec<(u32, u32, bool, bool)>) {
        let list = self.list.lock_irqsave();
        let mut out = Vec::new();
        for e in list.iter() {
            let t = e.task();
            if t.is_null() {
                out.push((0, 0xFFFF_FFFF, e.is_woken(), false));
                continue;
            }
            // SAFETY: read-only diagnostic snapshot of a queued entry's
            // task; the entry keeps the task alive on this queue.
            let (pid, st, oncpu) = unsafe { ((*t).pid(), (*t).state().bits(), (*t).on_cpu()) };
            out.push((pid as u32, st, e.is_woken(), oncpu));
        }
        (self.waiters.load(Ordering::Acquire), out)
    }

    /// Wake one process (exclusive)
    pub fn wake_up_one(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 1)
    }

    /// Wake-drop probe snapshot (input:evdev_wake hunt): entries as
    /// (pid, state bits, woken flag, task ptr) so the push_event probe
    /// can key pre/post wake verdicts per entry AND read scheduler
    /// linkage for the exact task. Diagnostic only; irqsave list lock.
    pub fn debug_entries_ptr(&self) -> (usize, Vec<(u32, u32, bool, *mut Task)>) {
        let list = self.list.lock_irqsave();
        let mut out = Vec::new();
        for e in list.iter() {
            let t = e.task();
            if t.is_null() {
                out.push((0, 0xFFFF_FFFF, e.is_woken(), t));
                continue;
            }
            // SAFETY: read-only diagnostic snapshot; the entry keeps the
            // task alive on this queue while the lock is held.
            let (pid, st) = unsafe { ((*t).pid(), (*t).state().bits()) };
            out.push((pid as u32, st, e.is_woken(), t));
        }
        (self.waiters.load(Ordering::Acquire), out)
    }

    /// Lock-free proof of waiters (the `waiters` fast-path counter):
    /// true when at least one entry is queued. Callers that only need
    /// "is anyone parked here?" (e.g. the idle loop's UART drain gate,
    /// which skips a port-read VM exit when no reader sleeps) use this
    /// instead of taking the irqsave list lock.
    pub fn has_sleepers(&self) -> bool {
        self.waiters.load(Ordering::Acquire) != 0
    }

    /// Diagnostic: entries currently queued, and how many carry the
    /// woken flag (VW forensic instrumentation — a woken-flagged entry
    /// whose task sleeps is the sticky-skip lost-wake signature).
    pub fn vw_queue_census(&self) -> (usize, usize) {
        let list = self.list.lock_irqsave();
        let total = list.len();
        let woken = list.iter().filter(|e| e.is_woken()).count();
        drop(list);
        (total, woken)
    }

    /// Try-lock variant for IRQ-context diagnostics: None when the list
    /// lock is held (never block — the holder may be the interrupted
    /// context on this very CPU).
    pub fn vw_queue_census_try(&self) -> Option<(usize, usize)> {
        let list = self.list.try_lock_irqsave()?;
        let total = list.len();
        let woken = list.iter().filter(|e| e.is_woken()).count();
        drop(list);
        Some((total, woken))
    }

    /// Prepare to wait: atomically add entry to queue AND set task state.
    ///
    /// This prevents the classic lost-wakeup race where a waker finds the
    /// entry on the queue but the task is still RUNNING (is_sleeping()=false),
    /// marks the entry as woken, then the task sets INTERRUPTIBLE and vanishes
    /// from the runqueue — never to be woken again.
    ///
    /// By holding the waitqueue lock across both operations, the waker (which
    /// also holds this lock) will always see a consistent state.
    pub fn prepare_to_wait(&self, task: *mut Task, exclusive: bool, interruptible: bool) {
        let entry = WaitQueueEntry::new(task, exclusive);
        let mut list = self.list.lock_irqsave();

        // Set task state BEFORE adding to queue (both under same lock).
        // Waker holds this same lock, so it will see either:
        //   - task still RUNNING, entry not on queue → no action (correct)
        //   - task INTERRUPTIBLE, entry on queue → wake_up succeeds (correct)
        if interruptible {
            unsafe {
                (*task).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::INTERRUPTIBLE));
            }
        } else {
            unsafe {
                (*task).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::UNINTERRUPTIBLE));
            }
        }

        // Add entry to queue
        if exclusive {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
        // In-section (see `waiters`): the increment must be published
        // before the lock release so a waker's fast-path load pairs
        // with it (the lost-wake argument in the field's doc block).
        self.waiters.fetch_add(1, Ordering::Release);
        // Lock released here — now waker can see consistent state
    }

    /// Finish wait: remove entry from queue and restore task state to RUNNING.
    ///
    /// Called after schedule() returns (task was woken) or when condition is
    /// already met after prepare_to_wait.
    pub fn finish_wait(&self, task: *mut Task) {
        let mut list = self.list.lock_irqsave();
        let before = list.len();
        list.retain(|entry| entry.task() != task);
        let dropped = before - list.len();
        if dropped > 0 {
            self.waiters.fetch_sub(dropped, Ordering::Release);
        }

        // If task is still sleeping (condition met before schedule, or
        // called from finish path), restore to RUNNING.
        unsafe {
            let state = (*task).state();
            if state.is_sleeping() {
                (*task).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING));
            }
        }
    }
}

#[macro_export]
macro_rules! wait_event {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        loop {
            // Check condition
            if $condition {
                break;
            }

            // Condition not met, prepare to wait.
            let current = match crate::sched::current() {
                Some(task) => task,
                None => break,
            };

            // Atomically add to wait queue AND set UNINTERRUPTIBLE state
            // (both under the waitqueue lock) to prevent lost-wakeup race.
            wq_head.prepare_to_wait(current, false, false);

            // Re-check condition after prepare_to_wait (state is now UNINTERRUPTIBLE)
            if $condition {
                wq_head.finish_wait(current);
                // R8-5 (NEW-C2): a concurrent wake between prepare_to_wait
                // and here may have enqueued us while we never slept — undo
                // (per-class on_rq guards make it a no-op otherwise).
                crate::sched::dequeue_task(&*current);
                break;
            }

            // Yield CPU — task removed from runqueue by __schedule()
            //
            // Enable interrupts before schedule(). We're in syscall context
            // (SIE=0). This ensures lock_irqsave in __schedule saves SIE=1,
            // so when the task is later switched back in, restore_irq restores
            // SIE=1 (via __schedule's unconditional restore_irq(true)).
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();

            // After wakeup, state is RUNNING (set by enqueue_task_locked)
            // Remove from wait queue and restore state
            wq_head.finish_wait(current);

            // Re-check condition
        }
    }};
}

/// Wait for a condition to become true, interruptible by signals.
///
/// Returns 0 on success (condition met), or -ERESTARTSYS if interrupted by a signal.
#[macro_export]
macro_rules! wait_event_interruptible {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        let _ret = loop {
            // Check condition first (fast path, no locking needed)
            if $condition {
                break 0i32;
            }

            // Check for pending signals
            if crate::signal::signal_pending() {
                break -512i32; // -ERESTARTSYS
            }

            let current = match crate::sched::current() {
                Some(task) => task,
                None => break 0i32,
            };

            // Atomically add to wait queue AND set INTERRUPTIBLE (under lock).
            // This prevents the lost-wakeup race where the waker finds the
            // entry on the queue but the task is still RUNNING, marks the
            // entry woken, then the task sets INTERRUPTIBLE and vanishes.
            wq_head.prepare_to_wait(current, false, true);

            // Re-check condition after prepare_to_wait (state is now INTERRUPTIBLE)
            if $condition {
                // Condition met — restore RUNNING and remove from queue
                wq_head.finish_wait(current);
                // R8-5 (NEW-C2): undo a concurrent wake enqueue.
                crate::sched::dequeue_task(&*current);
                break 0i32;
            }

            // Re-check signals after prepare_to_wait
            if crate::signal::signal_pending() {
                wq_head.finish_wait(current);
                // R9-17: R8-5 discipline for the signal path too — undo a
                // concurrent wake enqueue before returning.
                crate::sched::dequeue_task(&*current);
                break -512i32; // -ERESTARTSYS
            }

            // Enable interrupts before schedule(). We're in syscall context
            // (SIE=0). This ensures lock_irqsave in __schedule saves SIE=1,
            // so when the task is later switched back in, restore_irq restores
            // SIE=1 (via __schedule's unconditional restore_irq(true)).
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();

            // After wakeup, state is RUNNING (set by enqueue_task_locked).
            // Remove from wait queue.
            wq_head.finish_wait(current);
        };
        _ret
    }};
}
