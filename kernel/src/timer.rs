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

    let mut timers = TIMERS.lock();
    if timers.len() >= MAX_TIMERS {
        return 0;
    }
    timers.insert(id, entry);

    let mut actions = ACTIONS.lock();
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

    let mut timers = TIMERS.lock();
    if timers.len() >= MAX_TIMERS {
        return 0;
    }
    timers.insert(id, entry);

    let mut actions = ACTIONS.lock();
    actions.insert(id, action);

    id
}

/// Delete a timer and its associated action.
///
/// # Returns
/// `true` if timer was found and removed.
pub fn del_timer(id: u64) -> bool {
    // Lock order: TIMERS then ACTIONS — matches add_timer / softirq handler
    let mut timers = TIMERS.lock();
    let removed = timers.remove(&id).is_some();
    let mut actions = ACTIONS.lock();
    actions.remove(&id);
    removed
}

/// Modify a timer's expiration time.
///
/// If the timer does not exist, does nothing and returns false.
pub fn mod_timer(id: u64, new_expires: u64) -> bool {
    let mut timers = TIMERS.lock();
    if let Some(entry) = timers.get_mut(&id) {
        entry.expires = new_expires;
        true
    } else {
        false
    }
}

/// Check if a timer is currently active.
pub fn timer_pending(id: u64) -> bool {
    let timers = TIMERS.lock();
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

    // Collect and process expired timers under locks (H48 fix)
    {
        let mut timers = TIMERS.lock();
        let mut actions = ACTIONS.lock();
        let mut expired = alloc::vec::Vec::new();
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

        // Process expired timers while still holding locks to prevent
        // concurrent timerfd_close from freeing the TimerFd (H48 fix).
        // del_timer acquires both TIMERS and ACTIONS locks, so it cannot
        // race with us here.
        for (id, action) in &expired {
            if action.wake_pid != 0 {
                let task = crate::process::pid_hash::pid_hash_lookup(action.wake_pid);
                if !task.is_null() {
                    crate::sched::wake_up_process(task);
                }
            } else if action.tfd_addr != 0 {
                unsafe {
                    let counter_ptr = action.tfd_addr as *const core::sync::atomic::AtomicU64;
                    (*counter_ptr).fetch_add(1, Ordering::Release);
                }
            } else if action.pid != 0 && action.signo != 0 {
                let _ = crate::signal::send_signal(action.pid, action.signo);
            }
        }

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

    }
}

// ==================== Initialization ====================

/// Initialize the timer subsystem.
pub fn init() {
    LAST_TICK.store(timer::get_jiffies(), Ordering::Relaxed);
    crate::pr_info!("timer: software timer subsystem initialized");
}
