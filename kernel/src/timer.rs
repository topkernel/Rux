//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Timer Wheel
//!
//! Provides a simple timer mechanism for software timers.
//! Timers are stored in a BTreeMap keyed by timer ID, and the
//! Hrtimer softirq handler scans for expired timers on each tick.
//!
//! Callbacks run in softirq context — must not sleep.
//!
//! Timer actions (signal delivery, timerfd notification, re-arming)
//! are registered via `add_timer_with_action()` and looked up by
//! timer ID during expiry.

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::collections::BTreeMap;
use crate::sync::spinlock::Spinlock;
use crate::drivers::timer;

/// Maximum number of concurrent timers.
const MAX_TIMERS: usize = 1024;

/// Global timer ID counter.
static NEXT_TIMER_ID: AtomicU64 = AtomicU64::new(1);

/// A timer entry in the active set.
struct TimerEntry {
    /// Jiffies when this timer fires.
    expires: u64,
}

/// A timer action: what to do when a timer expires.
struct TimerAction {
    /// Target PID (0 = no signal delivery).
    pid: u32,
    /// Signal number to send (e.g., SIGALRM=14). 0 = no signal.
    signo: i32,
    /// Interval in jiffies for periodic timers (0 = one-shot).
    interval_jiffies: u64,
    /// Timerfd address (non-zero = timerfd mode: increment counter).
    /// When non-zero, signal delivery is skipped and the counter at
    /// this address is incremented instead.
    tfd_addr: u64,
    /// PID to wake up on expiry (non-zero = wake this process).
    wake_pid: u32,
}

/// Active timers: timer_id → TimerEntry.
static TIMERS: Spinlock<BTreeMap<u64, TimerEntry>> = Spinlock::new(BTreeMap::new());

/// Timer actions: timer_id → TimerAction.
static ACTIONS: Spinlock<BTreeMap<u64, TimerAction>> = Spinlock::new(BTreeMap::new());

/// Last-processed jiffies value.
static LAST_TICK: AtomicU64 = AtomicU64::new(0);

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

    let entry = TimerEntry { expires };
    let action = TimerAction {
        pid: 0,
        signo: 0,
        interval_jiffies: 0,
        tfd_addr: 0,
        wake_pid,
    };

    let mut timers = TIMERS.lock_irqsave();
    if timers.len() >= MAX_TIMERS {
        return 0;
    }
    timers.insert(id, entry);

    let mut actions = ACTIONS.lock_irqsave();
    actions.insert(id, action);

    id
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

    let entry = TimerEntry { expires };
    let action = TimerAction {
        pid,
        signo,
        interval_jiffies,
        tfd_addr,
        wake_pid: 0,
    };

    let mut timers = TIMERS.lock_irqsave();
    if timers.len() >= MAX_TIMERS {
        return 0;
    }
    timers.insert(id, entry);

    let mut actions = ACTIONS.lock_irqsave();
    actions.insert(id, action);

    id
}

/// Delete a timer and its associated action.
///
/// # Returns
/// `true` if timer was found and removed.
pub fn del_timer(id: u64) -> bool {
    // Lock order: TIMERS then ACTIONS — matches add_timer / softirq handler
    let mut timers = TIMERS.lock_irqsave();
    let removed = timers.remove(&id).is_some();
    let mut actions = ACTIONS.lock_irqsave();
    actions.remove(&id);
    removed
}

/// Modify a timer's expiration time.
///
/// If the timer does not exist, does nothing and returns false.
pub fn mod_timer(id: u64, new_expires: u64) -> bool {
    let mut timers = TIMERS.lock_irqsave();
    if let Some(entry) = timers.get_mut(&id) {
        entry.expires = new_expires;
        true
    } else {
        false
    }
}

/// Check if a timer is currently active.
pub fn timer_pending(id: u64) -> bool {
    let timers = TIMERS.lock_irqsave();
    timers.contains_key(&id)
}

// ==================== Softirq Handler ====================

/// Timer softirq handler.
///
/// Called from `__do_softirq()` when Hrtimer softirq is raised.
/// Scans all timers and fires those whose `expires <= current_jiffies`.
/// Periodic timers are re-armed automatically.
pub fn timer_softirq_handler(_nr: usize) {
    let current = timer::get_jiffies();
    let last = LAST_TICK.load(Ordering::Relaxed);

    if current == last {
        return;
    }
    // R20-7: record the processed jiffy so a second softirq within the same
    // jiffy returns early instead of re-scanning (and re-locking) the timer
    // maps. This store was missing, so the dedupe above never fired and the
    // full scan ran on every raise (TIMERS/ACTIONS lock churn).
    LAST_TICK.store(current, Ordering::Release);

    // Collect expired timers under locks; deliver AFTER releasing them
    // (R12-3 — see the moved delivery block below).
    let mut expired = alloc::vec::Vec::new();
    {
        let mut timers = TIMERS.lock_irqsave();
        let mut actions = ACTIONS.lock_irqsave();
        timers.retain(|&id, entry| {
            if entry.expires <= current {
                if let Some(action) = actions.get(&id) {
                    expired.push((id, TimerAction {
                        pid: action.pid,
                        signo: action.signo,
                        interval_jiffies: action.interval_jiffies,
                        tfd_addr: action.tfd_addr,
                        wake_pid: action.wake_pid,
                    }));
                }
                false
            } else {
                true
            }
        });

        // R12-3: the wake/signal/slot-free work moved OUT of the
        // TIMERS/ACTIONS critical section — holding both while calling
        // wake_up_process (TIMERS -> GRQ nesting) was the observed TIMERS
        // wedge ingredient (3 CPUs spinning on the TIMERS lock after a
        // pipeline). The `expired` list is a detached local snapshot, so
        // concurrency here is only against del_timer on the same ids;
        // the re-arm pass below still runs under the locks. The tfd
        // increment is a plain atomic (H48's close-race protection is the
        // refcount on the fd side; the H48 comment applied to freeing,
        // which does not happen in this loop).
        // (Delivery happens after the locks drop — see the moved block.)

        // Re-arm periodic timers (still under locks for consistency)
        for (id, action) in &expired {
            if action.interval_jiffies > 0 {
                actions.insert(*id, TimerAction {
                    pid: action.pid,
                    signo: action.signo,
                    interval_jiffies: action.interval_jiffies,
                    tfd_addr: action.tfd_addr,
                    wake_pid: action.wake_pid,
                });
                timers.insert(*id, TimerEntry {
                    expires: current + action.interval_jiffies,
                });
            }
        }
    } // TIMERS + ACTIONS released here

    // R12-3: delivery OUTSIDE the timer locks. The old in-lock
    // wake_up_process created a TIMERS -> GRQ nesting that wedged all
    // CPUs on the TIMERS lock whenever the GRQ side stalled (observed
    // after pipelines). `expired` is a detached snapshot; the ids were
    // removed from the maps under the locks above.
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
            let still_ours = {
                let timers = TIMERS.lock_irqsave();
                !timers.contains_key(id)
            };
            if still_ours {
                unsafe {
                    let counter_ptr = action.tfd_addr as *const core::sync::atomic::AtomicU64;
                    (*counter_ptr).fetch_add(1, Ordering::Release);
                }
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
