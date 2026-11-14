//! dfx=spinwatch — rapid zero-event epoll-return census (hilog-flood lane).
//!
//! The OH boot's log flood is the visible face of service loops that
//! should PARK but cycle: FFRT sleepers (os_ds_service's EpollManager
//! logs "in WaitTimeout, epoll_wait timeout" at 4.5k/s), mmi's dispatch
//! loop, systemui's loop handler. When a task's epoll_wait returns ZERO
//! events far faster than its asked timeout could expire, some fd in
//! its set is mis-signalled, or its timeout arithmetic is off. This
//! watch names the culprit: sys_epoll_wait's zero-event return path
//! (timeout expiry AND the 10 ms re-check slice) calls `record()`;
//! over the threshold (>80 zero-event returns per 200 ms) the caller
//! dumps the task's whole epoll interest set (fd, wanted mask, CURRENT
//! poll verdict, ops pointer for nm symbolization) once per 5 s.
//!
//! The walk itself lives in syscall/misc.rs (EpollFile/EpollEntry are
//! private to that module); this file owns arming, per-task counting
//! and the report rate limit.
//!
//! Arming: cmdline `spinwatch=1` (own key, unixrd pattern; the bespoke
//! dfx= switch table stays frozen). Zero cost when off (one relaxed
//! load per zero-event return).

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};

static ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn on() -> bool {
    match ARMED8.load(Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("spinwatch").as_deref() == Some("1"),
            };
            ARMED8.store(if v { 2 } else { 1 }, Relaxed);
            crate::pr_warn!(
                "spinwatch: {} (cmdline spinwatch param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("spinwatch")
            );
            v
        }
    }
}

/// Per-task watch state, direct-mapped by pid (512 slots — attribution
/// fidelity at census granularity, the hilogcensus trade-off).
struct Watch {
    pid: AtomicU32,
    /// zero-event returns in the current window.
    n: AtomicU32,
    /// jiffies when the window opened.
    win: AtomicU64,
    /// jiffies when this task last REPORTED (rate limit).
    last_rep: AtomicU64,
}

impl Watch {
    const fn new() -> Self {
        Watch {
            pid: AtomicU32::new(u32::MAX),
            n: AtomicU32::new(0),
            win: AtomicU64::new(0),
            last_rep: AtomicU64::new(0),
        }
    }
}

const SLOTS: usize = 512;
static WATCH: [Watch; SLOTS] = [const { Watch::new() }; SLOTS];
static ANY_REPORTED: AtomicBool = AtomicBool::new(false);

/// Threshold: zero-event returns per 200 ms window before the interest
/// set is dumped. A healthy waiter with a 10 ms slice re-check does at
/// most ~20; the FFRT spin face does 100+.
const N_THRESH: u32 = 80;
const WIN_JIFFIES: u64 = 20; // 200 ms at 100 Hz
const REP_GAP_JIFFIES: u64 = 500; // 5 s per task

/// Hook: one zero-event epoll_wait return (timeout expiry or empty
/// re-check slice). Returns true when the caller should dump its
/// interest set NOW (threshold crossed, per-task rate limit honored).
#[inline]
pub fn record() -> bool {
    if !on() {
        return false;
    }
    let pid = crate::process::current_pid();
    let w = &WATCH[(pid as usize) & (SLOTS - 1)];
    if w.pid.load(Relaxed) != pid {
        w.pid.store(pid, Relaxed);
        w.n.store(0, Relaxed);
        w.win.store(0, Relaxed);
        w.last_rep.store(0, Relaxed);
    }
    let now = crate::drivers::timer::get_jiffies();
    let win = w.win.load(Relaxed);
    if win == 0 || now.saturating_sub(win) >= WIN_JIFFIES {
        w.win.store(now, Relaxed);
        w.n.store(1, Relaxed);
        return false;
    }
    let n = w.n.fetch_add(1, Relaxed) + 1;
    if n >= N_THRESH {
        let last = w.last_rep.load(Relaxed);
        if now.saturating_sub(last) >= REP_GAP_JIFFIES {
            w.last_rep.store(now, Relaxed);
            // Reset the window so the count needs to re-climb before the
            // next report (steady spinners re-report every 5 s).
            w.n.store(0, Relaxed);
            w.win.store(now, Relaxed);
            if !ANY_REPORTED.swap(true, Relaxed) {
                crate::pr_warn!(
                    "SPINWATCH first report (threshold {} per {}ms)",
                    N_THRESH,
                    WIN_JIFFIES * 10
                );
            }
            return true;
        }
    }
    false
}
