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
    /// Jiffies when this timer fires (jiffy-only timers; bookkeeping for
    /// high-resolution entries — see `expires_time`).
    expires: u64,
    /// Optional HIGH-RESOLUTION absolute expiry (time CSR ticks, 100ns
    /// each; 0 = jiffies-only timer). When set, the timer fires EXACTLY
    /// when read_time() reaches it and never otherwise — nanosleep-class
    /// callers must never be woken early, and the jiffies counter can lag
    /// the time grid under load, so a jiffy fallback for these entries
    /// fired early and consumed the one-shot wake (lost-wakeup root
    /// cause; LTP clock_nanosleep02 also forbids early wakes).
    expires_time: u64,
}

/// Earliest outstanding high-resolution deadline (time CSR ticks);
/// u64::MAX when none. Drives timer re-arming: the per-hart timer is
/// programmed to min(next grid tick, this value).
static HRES_NEXT: AtomicU64 = AtomicU64::new(u64::MAX);

/// Read the earliest outstanding high-resolution deadline (u64::MAX = none).
pub fn hres_next_deadline() -> u64 {
    HRES_NEXT.load(Ordering::Acquire)
}

/// Recompute HRES_NEXT from the active timer set (min expires_time > 0).
/// Caller holds no locks requirement — takes TIMERS internally.
fn recompute_hres_next() {
    let timers = TIMERS.lock_irqsave();
    let mut next = u64::MAX;
    for entry in timers.values() {
        if entry.expires_time != 0 && entry.expires_time < next {
            next = entry.expires_time;
        }
    }
    drop(timers);
    HRES_NEXT.store(next, Ordering::Release);
}

/// A timer action: what to do when a timer expires.
#[derive(Clone, Copy)]
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

impl TimerAction {
    /// All-zero instance for array initialization (a zeroed TimerAction
    /// delivers nothing: pid/signo/wake_pid/tfd_addr are all 0).
    const EMPTY: TimerAction = TimerAction {
        pid: 0,
        signo: 0,
        interval_jiffies: 0,
        tfd_addr: 0,
        wake_pid: 0,
    };
}

/// Active timers: timer_id → TimerEntry.
static TIMERS: Spinlock<BTreeMap<u64, TimerEntry>> = Spinlock::new(BTreeMap::new());

/// Timer actions: timer_id → TimerAction.
static ACTIONS: Spinlock<BTreeMap<u64, TimerAction>> = Spinlock::new(BTreeMap::new());

/// Last-processed jiffies value.
static LAST_TICK: AtomicU64 = AtomicU64::new(0);

/// R34 (TIMERS-side wedge): maximum expiries processed per softirq pass.
/// The `expired`/`rearmed` Vecs are reserved to exactly this budget BEFORE
/// the TIMERS lock is taken, so the pushes inside the critical section can
/// never grow the buffer — every heap allocation that used to happen under
/// TIMERS+ACTIONS (Vec growth on `expired.push` inside `retain`) was an
/// OOM-panic point: `alloc_error_handler` panics (no unwinding, panic =
/// abort), leaving the panicking CPU parked in `wfi` with TIMERS held and
/// the other 3 CPUs spinning on the TIMERS lock forever (the observed
/// "holder never returns" signature). Leftover expired entries stay in the
/// map and are drained on the next jiffy (LAST_TICK dedupe only skips the
/// SAME jiffy).
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

    let entry = TimerEntry { expires, expires_time: 0 };
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

/// Add a one-shot HIGH-RESOLUTION wake timer: fires when the time CSR
/// reaches `expires_time` (absolute ticks). `expires` is bookkeeping only
/// for hres entries — the softirq expiry decision uses `expires_time`
/// exclusively (see timer_softirq_handler: a jiffy fallback firing an
/// hres entry EARLY consumed the one-shot wake and hung sleepers, the
/// lost-timer-wakeup root cause). Used by nanosleep/clock_nanosleep.
pub fn add_timer_wakeup_hres(expires: u64, expires_time: u64, wake_pid: u32) -> u64 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        return 0;
    }

    let entry = TimerEntry { expires, expires_time };
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
    // Keep the global earliest-hres view current so the caller (on its own
    // hart, before sleeping) can re-arm the hart timer to the earlier
    // deadline.
    if expires_time != 0 && expires_time < HRES_NEXT.load(Ordering::Acquire) {
        HRES_NEXT.store(expires_time, Ordering::Release);
    }

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

    let entry = TimerEntry { expires, expires_time: 0 };
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

/// add_timer_with_action with a HIGH-RESOLUTION absolute deadline (POSIX
/// timers armed by timer_settime — signal delivery at a precise time CSR
/// tick, jiffy `expires` as the collection fallback).
pub fn add_timer_with_action_hres(
    expires: u64,
    expires_time: u64,
    pid: u32,
    signo: i32,
    interval_jiffies: u64,
    tfd_addr: u64,
) -> u64 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        return 0;
    }

    let entry = TimerEntry { expires, expires_time };
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
    if expires_time != 0 && expires_time < HRES_NEXT.load(Ordering::Acquire) {
        HRES_NEXT.store(expires_time, Ordering::Release);
    }

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
    let removed_hres = timers.get(&id).map(|e| e.expires_time != 0).unwrap_or(false);
    let removed = timers.remove(&id).is_some();
    if removed_hres {
        // The deleted timer may have been the global hres minimum; the
        // cheap next recompute in the softirq will fix the value, but do it
        // now so hart re-arms stop over-firing sooner.
        let mut next = u64::MAX;
        for entry in timers.values() {
            if entry.expires_time != 0 && entry.expires_time < next {
                next = entry.expires_time;
            }
        }
        HRES_NEXT.store(next, Ordering::Release);
    }
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
        entry.expires_time = 0;
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

/// Query a pending timer's absolute state.
///
/// Returns `Some((expires_jiffies, interval_jiffies))` while the timer is
/// pending; `interval_jiffies` is 0 for one-shot timers. Used by
/// getitimer/setitimer to report the remaining ITIMER_REAL time.
pub fn get_timer_state(id: u64) -> Option<(u64, u64)> {
    // Lock order: TIMERS then ACTIONS — matches add/del paths.
    let timers = TIMERS.lock_irqsave();
    let expires = timers.get(&id)?.expires;
    let actions = ACTIONS.lock_irqsave();
    let interval_jiffies = actions.get(&id).map(|a| a.interval_jiffies).unwrap_or(0);
    Some((expires, interval_jiffies))
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

    // High-resolution timers are NOT jiffy-deduped: an extra hart IRQ fired
    // at a sub-jiffy deadline re-runs this scan even in the same jiffy.
    let now_time = timer::read_time();
    let hres_due = HRES_NEXT.load(Ordering::Acquire) <= now_time;

    if current == last && !hres_due {
        return;
    }
    // R20-7: record the processed jiffy so a second softirq within the same
    // jiffy returns early instead of re-scanning (and re-locking) the timer
    // maps. This store was missing, so the dedupe above never fired and the
    // full scan ran on every raise (TIMERS/ACTIONS lock churn).
    LAST_TICK.store(current, Ordering::Release);

    // Collect expired timers under locks; deliver AFTER releasing them
    // (R12-3 — see the moved delivery block below).
    // R34: capacity reserved OUTSIDE the lock; the budget counter below
    // guarantees no in-lock growth, and periodic timers are re-armed IN
    // PLACE inside the retain closure (node kept, only `expires` mutated),
    // so the TIMERS+ACTIONS critical section performs NO heap allocation
    // at all — every allocation that used to happen under the locks
    // (`expired`/`rearmed` Vec growth, remove+insert churn of re-armed
    // nodes) was an OOM-panic point: `alloc_error_handler` panics
    // (panic=abort, no unwinding), parking the CPU in `wfi` with TIMERS
    // held while the other 3 CPUs spin on the TIMERS lock forever — the
    // observed "holder never returns" signature. Leftover expired entries
    // (budget exhausted) stay in the map and drain on the next jiffy
    // (LAST_TICK dedupe only skips the SAME jiffy).
    // WEDGE/perf fix: the expired list used to be a heap
    // `Vec::with_capacity(EXPIRY_BUDGET)` allocated on EVERY tick — a
    // 2560-byte alloc/dealloc pair per jiffy in IRQ/softirq context.
    // Under the LTP fs-storm wedges this churn dominated the buddy
    // allocator (photographed by gdb at exactly this line) and was the
    // sendmsg02 ALLOCTHROW (alloc-failure panic) site. Use per-CPU
    // static backing storage instead: the SOFTIRQ_IN_PROGRESS guard
    // makes handler re-entry on the same CPU impossible, so a per-CPU
    // buffer needs no lock. (TimerAction is Copy — moved out of the map
    // below.)
    /// Per-CPU expired-timer scratch (EXPIRY_BUDGET entries).
    static mut EXPIRED_BUF: [[(u64, TimerAction); EXPIRY_BUDGET];
        crate::config::MAX_CPUS] =
        [[(0, TimerAction::EMPTY); EXPIRY_BUDGET]; crate::config::MAX_CPUS];
    /// Per-CPU fill level of EXPIRED_BUF.
    static mut EXPIRED_LEN: [usize; crate::config::MAX_CPUS] = [0; crate::config::MAX_CPUS];
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return; // unreachable on this platform; keep the handler total
    }
    // SAFETY: per-CPU rows indexed by this CPU's id; the SOFTIRQ_IN_PROGRESS
    // guard prevents re-entry, so this CPU's row is exclusively ours.
    let buf_ptr: *mut [(u64, TimerAction); EXPIRY_BUDGET] =
        unsafe { (core::ptr::addr_of_mut!(EXPIRED_BUF) as *mut [(u64, TimerAction); EXPIRY_BUDGET]).add(cpu) };
    let len_ptr: *mut usize =
        unsafe { (core::ptr::addr_of_mut!(EXPIRED_LEN) as *mut usize).add(cpu) };
    unsafe { *len_ptr = 0 };
    {
        let mut timers = TIMERS.lock_irqsave();
        let mut actions = ACTIONS.lock_irqsave();
        let mut budget = EXPIRY_BUDGET;
        timers.retain(|&id, entry| {
            // Expiry. High-resolution entries (expires_time != 0) fire ONLY
            // on the precise time-CSR compare — the old `||
            // entry.expires <= current` jiffy fallback ALSO fired them, and
            // that is the lost-timer-wakeup root cause (timer-fix hunt,
            // unix_wedge TIMER-STALL / GNOME final blocker):
            //
            //   `expires` is computed as get_jiffies() + ceil(...) + 1 at
            //   ARM time, but jiffies advance only inside
            //   increment_jiffies() on some hart's timer IRQ — under load
            //   (long SIE=0 windows) the counter LAGS the time grid by more
            //   than the +1 grid-tick margin, so `expires` lands BEFORE the
            //   real time that hres_deadline represents. When ticks resume,
            //   jiffies jump forward (max-with-nominal) and the scan fires
            //   the entry EARLY — observed 100us..2.4ms before
            //   expires_time (TIMER-EARLY-FIRE ... via=2 tripwire), always
            //   immediately followed by a permanently sleeping task.
            //
            // Why that is fatal: the one-shot wake is the ONLY waker for
            // nanosleep-class sleepers. An early fire consumes the token;
            // the sleeper's re-check still sees time < hres_deadline, so it
            // goes back to INTERRUPTIBLE sleep — with no timer left. The
            // task hangs in state=S forever (timer_probe reproduces at
            // ~60%/60s; unix_wedge PROC-STALL epw/srv state=S).
            //
            // The jiffy fallback is also redundant for hres entries:
            // collection is guaranteed by ANY later scan, because
            // `expires_time <= now_time` eventually holds on the jiffy
            // grid alone (now_time only grows). Jiffy-only entries
            // (expires_time == 0: poll_sleep_slice, unix_wait_round,
            // itimers ...) keep firing on `expires <= current` — their
            // callers re-check against the same jiffies clock, so an
            // early-by-grid-jump fire is unobservable to them.
            let due = if entry.expires_time != 0 {
                entry.expires_time <= now_time
            } else {
                entry.expires <= current
            };
            if due {
                if budget == 0 {
                    // Budget exhausted: keep the entry — it is re-scanned on
                    // the next jiffy. Never let the critical section allocate.
                    return true;
                }
                budget -= 1;
                if let Some(action) = actions.get(&id) {
                    if action.interval_jiffies > 0 {
                        // Periodic: re-arm IN PLACE (same id, node kept) —
                        // semantically identical to the old remove+re-insert
                        // of the same key, but without the under-lock
                        // dealloc+alloc pair. Bonus: del_timer can no longer
                        // miss the briefly-removed id (orphan-timer race).
                        // The action stays in ACTIONS untouched.
                        entry.expires = current + action.interval_jiffies;
                        // R37: a PERIODIC timer must also DELIVER on every
                        // expiry — the old path only re-armed, so repeating
                        // ITIMER_REAL (setitimer with it_interval != 0) and
                        // periodic posix timers armed through the wheel
                        // never sent a single signal (LTP setitimer01/
                        // timer_settime hangs). Queue a COPY of the action
                        // for the outside-locks delivery pass below; the
                        // map keeps its own entry for the next expiry.
                        unsafe {
                            if *len_ptr < EXPIRY_BUDGET {
                                (*buf_ptr)[*len_ptr] = (id, *action); *len_ptr += 1;
                            }
                        }
                        // Periodic re-arm is jiffy-based — drop any stale
                        // high-resolution deadline so it cannot re-fire the
                        // very next scan.
                        entry.expires_time = 0;
                        return true;
                    }
                }
                // One-shot: TAKE the action out (remove = dealloc only,
                // cannot allocate — the no-alloc-under-lock discipline is
                // preserved). The old code copied it with `actions.get`
                // and left the entry in ACTIONS forever, so every one-shot
                // timer (nanosleep / glib timeout / timerfd tick in GNOME)
                // leaked one TimerAction: the ACTIONS BTreeMap grew
                // ~4.8 nodes/s until the 128MB kernel heap died at ~65min
                // (the GNOME-oom leak; memwatch live-site evidence:
                // add_timer_wakeup leaf/internal nodes live=3000+, frees=4).
                if let Some(action) = actions.remove(&id) {
                    unsafe {
                        // SAFETY: budget > 0 guarantees *len < EXPIRY_BUDGET;
                        // per-CPU buffer, non-reentrant via SOFTIRQ guard.
                        let n = *len_ptr;
                        (*buf_ptr)[n] = (id, action);
                        *len_ptr = n + 1;
                    }
                }
                false // one-shot: remove (dealloc cannot fail)
            } else {
                true
            }
        });

        // R12-3: the wake/signal/slot-free work moved OUT of the
        // TIMERS/ACTIONS critical section — holding both while calling
        // wake_up_process (TIMERS -> GRQ nesting) was the observed TIMERS
        // wedge ingredient (3 CPUs spinning on the TIMERS lock after a
        // pipeline). The `expired` list is a detached local snapshot, so
        // concurrency here is only against del_timer on the same ids.
        // The tfd increment is a plain atomic (H48's close-race protection
        // is the refcount on the fd side; the H48 comment applied to
        // freeing, which does not happen in this loop).
        // (Delivery happens after the locks drop — see the moved block.)
    } // TIMERS + ACTIONS released here

    // Refresh the earliest-hres view: fired/removed timers may have been
    // the minimum. Cheap scan under TIMERS only.
    if hres_due {
        recompute_hres_next();
    }

    // R12-3: delivery OUTSIDE the timer locks. The old in-lock
    // wake_up_process created a TIMERS -> GRQ nesting that wedged all
    // CPUs on the TIMERS lock whenever the GRQ side stalled (observed
    // after pipelines). `expired` is a detached snapshot; the ids were
    // removed from the maps under the locks above.
    // SAFETY: slice of this CPU's row, filled under the locks above.
    let expired_len = unsafe { *len_ptr };
    let expired: &[(u64, TimerAction)] = &unsafe { &*buf_ptr }[..expired_len];
    for (id, action) in expired {
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
            // The address now names the whole TimerFd so the notify hook can
            // also wake readers blocked in timerfd_read (review批次1).
            crate::syscall::misc::timerfd_expire_notify(action.tfd_addr);
        } else if action.pid != 0 && action.signo != 0 {
            let _ = crate::signal::send_signal(action.pid, action.signo);
        }
    }

    // DFX dfx=periodic: a task snapshot every PERIODIC_DUMP_SECS. Silent
    // hangs (form-B) trip no watchdog and swallow the UART magic (a wedged
    // shell stops draining the RX path) — a timed snapshot from the timer
    // softirq always lands. SBI-direct, no locks held here.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::PeriodicDump)
        && current.saturating_sub(LAST_PERIODIC_DUMP.load(Ordering::Relaxed)) >= 500
    {
        LAST_PERIODIC_DUMP.store(current, Ordering::Relaxed);
        crate::dfx::taskdump::dump_all_tasks("periodic");    }
}

/// Jiffies of the last periodic DFX snapshot (dfx=periodic).
static LAST_PERIODIC_DUMP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// Initialization: the old dead `pub fn init()` (never called anywhere — it
// only seeded LAST_TICK and printed a banner) was removed in the review
// batch-8 dead-code cleanup; the softirq scan self-seeds LAST_TICK on its
// first run (line ~197).
