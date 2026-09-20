//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Timer Wheel
//!
//! Provides a simple timer mechanism for software timers.
//! Timers live in a FIXED-SLOT table (static .bss storage, no heap), and
//! the Hrtimer softirq handler scans occupied slots on each tick.
//!
//! Callbacks run in softirq context — must not sleep.
//!
//! Timer actions (signal delivery, timerfd notification, re-arming)
//! are registered via `add_timer_with_action()` and looked up by
//! timer ID during expiry.
//!
//! R37 (root fix for the TIMERS wedge family): the old pair of
//! `BTreeMap<u64, TimerEntry>` / `BTreeMap<u64, TimerAction>` tables
//! (TIMERS + ACTIONS locks) allocated BTreeMap NODES INSIDE the locks
//! on `insert()`. R34 had already made the softirq expiry path
//! zero-allocation (EXPIRY_BUDGET + in-place re-arm), but the add path
//! (`add_timer_wakeup` / `add_timer_with_action`) still allocated
//! under TIMERS→ACTIONS. On allocation failure `alloc_error_handler`
//! panics (no unwinding, panic = abort), parking the CPU in `wfi`
//! with the lock held while the other 3 CPUs spin on it forever — the
//! R36 run6 TIMERS deadlock reproduction. The slot table below makes
//! EVERY path under the lock allocation-free by construction: slots
//! are fixed .bss storage, occupancy is tracked in a fixed bitmap,
//! and entry+action are inlined in one slot (which also merges the
//! two tables into ONE lock, deleting the TIMERS→ACTIONS nesting
//! edge). Cost: 1024 × 48-byte slots + 128-byte bitmap ≈ 57 KB of
//! .bss — accepted.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::drivers::timer;

/// Maximum number of concurrent timers.
const MAX_TIMERS: usize = 1024;

/// Occupancy bitmap words (1 bit per slot).
const TIMER_WORDS: usize = MAX_TIMERS / 64;

// Compile-time sanity: the bitmap must cover the slot array exactly.
const _: () = assert!(MAX_TIMERS % 64 == 0);

/// Global timer ID counter.
/// Monotonic and NEVER reused: a freed slot always gets a fresh id on
/// reuse, so a stale id handed to del_timer/mod_timer/timer_pending can
/// only miss — it can never falsely match a slot that was recycled for
/// a different timer (id matching is how the slot table compensates for
/// losing the BTreeMap key).
static NEXT_TIMER_ID: AtomicU64 = AtomicU64::new(1);

/// A timer action: what to do when a timer expires.
/// Detached snapshot type — copied out of the slot table under the lock
/// and delivered after the lock is released (R12-3). Only the
/// delivery-relevant fields are carried: `interval_jiffies` is consumed
/// during the scan itself (in-place re-arm reads it from the slot), so
/// the snapshot has no use for it.
struct TimerAction {
    /// Target PID (0 = no signal delivery).
    pid: u32,
    /// Signal number to send (e.g., SIGALRM=14). 0 = no signal.
    signo: i32,
    /// Timerfd address (non-zero = timerfd mode: increment counter).
    /// When non-zero, signal delivery is skipped and the counter at
    /// this address is incremented instead.
    tfd_addr: u64,
    /// PID to wake up on expiry (non-zero = wake this process).
    wake_pid: u32,
}

/// One slot: timer entry (id + expires) with its action INLINED.
/// Merging the old TIMERS/ACTIONS pair into one slot removes the
/// TIMERS→ACTIONS lock nesting AND the transient state where an entry
/// was visible without its action.
#[derive(Clone, Copy)]
struct TimerSlot {
    /// Timer ID occupying this slot (monotonic — see NEXT_TIMER_ID).
    id: u64,
    /// Jiffies when this timer fires.
    expires: u64,
    // ---- action fields (old TimerAction) ----
    pid: u32,
    signo: i32,
    interval_jiffies: u64,
    tfd_addr: u64,
    wake_pid: u32,
}

impl TimerSlot {
    /// Copy the action half out (used for the lock-external delivery
    /// snapshot).
    fn action(&self) -> TimerAction {
        TimerAction {
            pid: self.pid,
            signo: self.signo,
            tfd_addr: self.tfd_addr,
            wake_pid: self.wake_pid,
        }
    }
}

/// The fixed-slot timer table: slot array + occupancy bitmap, guarded by
/// ONE spinlock. All mutations (occupy/free/re-arm) are plain stores —
/// no code path under TIMER_TABLE can touch the heap, so no
/// alloc_error_handler panic can ever fire with the lock held.
struct TimerTable {
    /// Occupancy bitmap: bit i set ⇔ slots[i] is Some. Lets both the
    /// softirq scan and the id lookup skip whole runs of empty slots
    /// with one word compare (mostly-empty table ⇒ ~16 loads per pass).
    bitmap: [u64; TIMER_WORDS],
    /// Fixed slot array. None = free.
    slots: [Option<TimerSlot>; MAX_TIMERS],
}

impl TimerTable {
    /// Find the first free slot (lowest index).
    /// Scans ≤ TIMER_WORDS bitmap words — no allocation, no failure
    /// mode other than "pool full".
    fn find_free_slot(&self) -> Option<usize> {
        for w in 0..TIMER_WORDS {
            let word = self.bitmap[w];
            if word != u64::MAX {
                let bit = (!word).trailing_zeros() as usize;
                return Some(w * 64 + bit);
            }
        }
        None
    }

    /// Mark a slot occupied. Caller guarantees `idx` is currently free.
    fn occupy(&mut self, idx: usize, slot: TimerSlot) {
        self.slots[idx] = Some(slot);
        self.bitmap[idx / 64] |= 1u64 << (idx % 64);
    }

    /// Free a slot (one-shot expiry or del_timer). Plain stores —
    /// cannot fail, cannot allocate.
    fn free(&mut self, idx: usize) {
        self.slots[idx] = None;
        self.bitmap[idx / 64] &= !(1u64 << (idx % 64));
    }

    /// Find the occupied slot holding `id`.
    /// Bitmap-accelerated walk: ids are unique and monotonic, so the
    /// first match is THE timer (a stale id matches nothing).
    fn find_by_id(&self, id: u64) -> Option<usize> {
        for w in 0..TIMER_WORDS {
            let mut word = self.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                let idx = w * 64 + bit;
                if let Some(slot) = &self.slots[idx] {
                    if slot.id == id {
                        return Some(idx);
                    }
                }
            }
        }
        None
    }
}

/// The timer table: ONE lock replaces the old TIMERS + ACTIONS pair,
/// deleting the TIMERS→ACTIONS nesting edge from the lock graph.
static TIMER_TABLE: Spinlock<TimerTable> = Spinlock::new(TimerTable {
    bitmap: [0; TIMER_WORDS],
    slots: [None; MAX_TIMERS],
});

/// Last-processed jiffies value.
static LAST_TICK: AtomicU64 = AtomicU64::new(0);

/// R34 (TIMERS-side wedge): maximum expiries processed per softirq pass.
/// The `expired` Vec is reserved to exactly this budget BEFORE the
/// TIMER_TABLE lock is taken, so the pushes inside the critical section
/// can never grow the buffer — every heap allocation that used to happen
/// under TIMERS+ACTIONS (Vec growth on `expired.push` inside the old
/// `retain`) was an OOM-panic point: `alloc_error_handler` panics (no
/// unwinding, panic = abort), leaving the panicking CPU parked in `wfi`
/// with TIMERS held and the other 3 CPUs spinning on the TIMERS lock
/// forever (the observed "holder never returns" signature). Leftover
/// expired entries stay in their slots and are drained on the next
/// jiffy (LAST_TICK dedupe only skips the SAME jiffy).
const EXPIRY_BUDGET: usize = 64;

// ==================== Public API ====================

/// Add a one-shot timer that wakes up a sleeping process on expiry.
///
/// Used by nanosleep: the caller sleeps in INTERRUPTIBLE state;
/// the timer softirq fires and calls wake_up_process to reschedule it.
///
/// # Arguments
/// - `expires`: jiffies value when the timer should fire
/// - `wake_pid`: PID of the process to wake
///
/// # Returns
/// Timer ID (u64), or 0 on failure.
pub fn add_timer_wakeup(expires: u64, wake_pid: u32) -> u64 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        return 0;
    }

    let slot = TimerSlot {
        id,
        expires,
        pid: 0,
        signo: 0,
        interval_jiffies: 0,
        tfd_addr: 0,
        wake_pid,
    };

    // R37: fixed-slot occupy — a plain store, replacing the old
    // `timers.insert()` / `actions.insert()` BTreeMap node allocations
    // that happened under the TIMERS→ACTIONS locks (the last under-lock
    // allocation in the subsystem; see the module doc).
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_free_slot() {
        Some(idx) => {
            table.occupy(idx, slot);
            id
        }
        None => 0, // pool full (old `timers.len() >= MAX_TIMERS` check)
    }
}

/// Add a timer with an associated action.
///
/// # Arguments
/// - `expires`: jiffies value when the timer should fire
/// - `pid`: target process ID (0 = no signal delivery)
/// - `signo`: signal to send on expiry (0 = no signal)
/// - `interval_jiffies`: re-arm interval (0 = one-shot)
/// - `tfd_addr`: timerfd address (0 = not a timerfd)
///
/// # Returns
/// Timer ID (u64), or 0 on failure.
pub fn add_timer_with_action(
    expires: u64,
    pid: u32,
    signo: i32,
    interval_jiffies: u64,
    tfd_addr: u64,
) -> u64 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        return 0;
    }

    let slot = TimerSlot {
        id,
        expires,
        pid,
        signo,
        interval_jiffies,
        tfd_addr,
        wake_pid: 0,
    };

    // R37: see add_timer_wakeup — zero allocation under the lock.
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_free_slot() {
        Some(idx) => {
            table.occupy(idx, slot);
            id
        }
        None => 0, // pool full (old `timers.len() >= MAX_TIMERS` check)
    }
}

/// Delete a timer and its associated action.
///
/// # Returns
/// `true` if timer was found and removed.
pub fn del_timer(id: u64) -> bool {
    // R37: single lock — the old TIMERS→ACTIONS nesting (lock order
    // comment used to live here) is gone: entry and action are one slot.
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_by_id(id) {
        Some(idx) => {
            table.free(idx);
            true
        }
        None => false,
    }
}

/// Modify a timer's expiration time.
///
/// If the timer does not exist, does nothing and returns false.
pub fn mod_timer(id: u64, new_expires: u64) -> bool {
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_by_id(id) {
        Some(idx) => {
            if let Some(slot) = table.slots[idx].as_mut() {
                slot.expires = new_expires;
            }
            true
        }
        None => false,
    }
}

/// Check if a timer is currently active.
pub fn timer_pending(id: u64) -> bool {
    let table = TIMER_TABLE.lock_irqsave();
    table.find_by_id(id).is_some()
}

// ==================== Softirq Handler ====================

/// Timer softirq handler.
///
/// Called from `__do_softirq()` when Hrtimer softirq is raised.
/// Scans all occupied slots and fires those whose `expires <=
/// current_jiffies`. Periodic timers are re-armed automatically.
pub fn timer_softirq_handler(_nr: usize) {
    let current = timer::get_jiffies();
    let last = LAST_TICK.load(Ordering::Relaxed);

    if current == last {
        return;
    }
    // R20-7: record the processed jiffy so a second softirq within the same
    // jiffy returns early instead of re-scanning (and re-locking) the timer
    // table. This store was missing, so the dedupe above never fired and the
    // full scan ran on every raise (timer-table lock churn).
    LAST_TICK.store(current, Ordering::Release);

    // Collect expired timers under the lock; deliver AFTER releasing it
    // (R12-3 — see the moved delivery block below).
    // R34: capacity reserved OUTSIDE the lock; the budget counter below
    // guarantees no in-lock growth, and periodic timers are re-armed IN
    // PLACE (same id, same slot, only `expires` mutated), so the
    // TIMER_TABLE critical section performs NO heap allocation at all —
    // every allocation that used to happen under the locks
    // (`expired`/`rearmed` Vec growth, remove+insert churn of re-armed
    // nodes) was an OOM-panic point: `alloc_error_handler` panics
    // (panic=abort, no unwinding), parking the CPU in `wfi` with the
    // lock held while the other 3 CPUs spin on it forever — the
    // observed "holder never returns" signature. Leftover expired
    // entries (budget exhausted) stay in their slots and drain on the
    // next jiffy (LAST_TICK dedupe only skips the SAME jiffy).
    // R37: with the fixed-slot table this zero-allocation property is
    // now structural — slot free/re-arm are plain stores, and the add
    // path allocates nothing under the lock either (see module doc).
    let mut expired: alloc::vec::Vec<(u64, TimerAction)> = alloc::vec::Vec::with_capacity(EXPIRY_BUDGET);
    {
        let mut table = TIMER_TABLE.lock_irqsave();
        let mut budget = EXPIRY_BUDGET;
        // Bitmap walk: empty bitmap words cost one compare each; each set
        // bit is one occupied slot. Bits already visited are cleared from
        // the local `word` copy, so freeing a one-shot slot (which clears
        // the bit in table.bitmap) cannot cause a double visit.
        for w in 0..TIMER_WORDS {
            let mut word = table.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                let idx = w * 64 + bit;
                // Copy the slot out (TimerSlot is Copy) so no borrow is
                // held across the in-place re-arm / free below. A clear
                // slot under a set bit is impossible (occupy/free keep
                // them in sync); the guard is a no-panic fallback.
                let slot = match table.slots[idx] {
                    Some(s) => s,
                    None => continue,
                };
                if slot.expires > current {
                    continue;
                }
                if budget == 0 {
                    // Budget exhausted: keep the entry — it is re-scanned
                    // on the next jiffy. Never let the critical section
                    // allocate.
                    continue;
                }
                budget -= 1;
                expired.push((slot.id, slot.action()));
                if slot.interval_jiffies > 0 {
                    // Periodic: re-arm IN PLACE (same id, slot kept) —
                    // semantically identical to the old remove+re-insert
                    // of the same key, but without the under-lock
                    // dealloc+alloc pair. Bonus: del_timer can no longer
                    // miss the briefly-removed id (orphan-timer race).
                    if let Some(s) = table.slots[idx].as_mut() {
                        s.expires = current + slot.interval_jiffies;
                    }
                } else {
                    // One-shot: free the slot (a plain store — cannot
                    // fail, cannot allocate).
                    table.free(idx);
                }
            }
        }

        // R12-3: the wake/signal/slot-free work moved OUT of the
        // TIMER_TABLE critical section — holding the lock while calling
        // wake_up_process (TIMERS -> GRQ nesting) was the observed TIMERS
        // wedge ingredient (3 CPUs spinning on the TIMERS lock after a
        // pipeline). The `expired` list is a detached local snapshot, so
        // concurrency here is only against del_timer on the same ids.
        // The tfd increment is a plain atomic (H48's close-race protection
        // is the refcount on the fd side; the H48 comment applied to
        // freeing, which does not happen in this loop).
        // (Delivery happens after the lock drops — see the moved block.)
    } // TIMER_TABLE released here

    // R12-3: delivery OUTSIDE the timer locks. The old in-lock
    // wake_up_process created a TIMERS -> GRQ nesting that wedged all
    // CPUs on the TIMERS lock whenever the GRQ side stalled (observed
    // after pipelines). `expired` is a detached snapshot; the ids were
    // removed from the table under the lock above.
    for (id, action) in &expired {
        if action.wake_pid != 0 {
            // R13-3: pinned (softirq wake racing a concurrent reap) — the
            // last unpinned cross-CPU wake path.
            let task = crate::process::pid_hash::pid_hash_lookup_pinned(action.wake_pid);
            if !task.is_null() {
                crate::sched::wake_up_process(task);
                crate::process::task::Task::task_put(task);
            }
        } else if action.tfd_addr != 0 {
            // R13-3 (H48 re-closed after R12-3): re-validate under TIMERS
            // that the id is still absent (not deleted-and-re-added) before
            // touching the fd's counter — a timerfd_close + free between
            // snapshot and delivery made this an add on freed memory.
            // R31-2 (third and correct form): ALWAYS deliver — both R25-1
            // and R28-1 accidentally SUPPRESSED periodic timerfd delivery
            // (rearmed ids were excluded, but rearmed ids are exactly the
            // periodic ones). The H48 close-race (increment on a freed
            // counter) is accepted per the timerfd refcount audit.
            // (R34: the `rearmed` id list itself became unnecessary when
            // re-arming moved in-place into the retain closure.)
            unsafe {
                let counter_ptr = action.tfd_addr as *const core::sync::atomic::AtomicU64;
                (*counter_ptr).fetch_add(1, Ordering::Release);
            }
        } else if action.pid != 0 && action.signo != 0 {
            let _ = crate::signal::send_signal(action.pid, action.signo);
        }
    }
}

// ==================== Initialization ====================

/// Initialize the timer subsystem.
pub fn init() {
    LAST_TICK.store(timer::get_jiffies(), Ordering::Relaxed);
    crate::pr_info!("timer: software timer subsystem initialized");
}
