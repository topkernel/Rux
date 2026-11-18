//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Timer Wheel (facade)
//!
//! Public API and softirq flow are UNCHANGED from the flat-table era;
//! the storage is now the typed cascading wheel in `timer/wheel.rs`
//! (arch-s5 pilot, survey shortlist S-5 — see that module's design
//! note for the type-level enforcement story).
//!
//! R37 history (landed leg 49; design note in
//! docs/development/r37-timer-slotted-table-draft.rs): the old pair of
//! `BTreeMap<u64, TimerEntry>` / `BTreeMap<u64, TimerAction>` tables
//! (TIMERS + ACTIONS locks) allocated BTreeMap NODES INSIDE the locks
//! on `insert()`. R34 had already made the softirq expiry path
//! zero-allocation (EXPIRY_BUDGET + in-place re-arm), but the add path
//! still allocated under TIMERS→ACTIONS. On allocation failure
//! `alloc_error_handler` panics (no unwinding, panic = abort), parking
//! the CPU in `wfi` with the lock held while the other CPUs spin on it
//! forever — the R36 run6 TIMERS deadlock family. The slot table (and
//! now the wheel) keeps EVERY path under the lock allocation-free by
//! construction: slots are fixed .bss storage, occupancy is tracked in
//! a fixed bitmap, buckets are intrusive index lists in the same pool.
//!
//! Callbacks run in softirq context — must not sleep (the SoftirqCtx
//! capability token in `wheel.rs` is the type-level record).
//!
//! Timer actions (signal delivery, timerfd notification, re-arming)
//! are registered via `add_timer_with_action()` and looked up by
//! timer ID during expiry.

use core::sync::atomic::{AtomicU64, Ordering, AtomicUsize, AtomicU8, AtomicU32};
use crate::sync::spinlock::Spinlock;
use crate::drivers::timer;

pub mod nohz;
pub mod oneshot;
pub mod wheel;

pub use oneshot::{WakeArmed, WakeSpent};
pub use wheel::{
    Action, Deadline, Expiry, FiredToken, HresClock, JiffyClock, Now, OneshotAction,
    PeriodicAction, SoftirqCtx, Time,
};

/// Maximum number of concurrent timers (pool size, unchanged).
pub const MAX_TIMERS: usize = wheel::MAX_TIMERS;

/// Global timer ID counter.
static NEXT_TIMER_ID: AtomicU64 = AtomicU64::new(1);

/// Earliest outstanding high-resolution deadline (time CSR ticks);
/// u64::MAX when none. Drives timer re-arming: the per-hart timer is
/// programmed to min(next grid tick, this value).
static HRES_NEXT: AtomicU64 = AtomicU64::new(u64::MAX);

/// sc184 (lane 2, perf-audit F1): monotonic "a timer has been armed at
/// least once" latch. The timer-tick softirq raise for the Hrtimer
/// vector is gated on this (plus HRES_NEXT being due): before the first
/// timer exists the vector's handler has literally nothing to scan, but
/// the unconditional raise still cost an irq-exit dispatch pass at IF=0
/// on every tick of every CPU (pa3: 199 raises/s/CPU). Monotonic by
/// design — like the TCP table's high-water count, it never resets, so
/// after the first timer the raise always happens and the softirq
/// watchdog stamping inside the handler is unaffected.
static TIMER_EVER_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Lock-free pending-work test for the Hrtimer softirq raise gate (see
/// TIMER_EVER_ARMED). A race (timer armed right after this read false)
/// delays that timer's first dispatch opportunity by at most one tick:
/// the arming path itself re-arms the hardware deadline, whose expiry
/// re-raises the vector through the interrupt path.
pub fn timer_softirq_work_pending() -> bool {
    TIMER_EVER_ARMED.load(Ordering::Acquire)

        || HRES_NEXT.load(Ordering::Acquire) <= crate::drivers::timer::read_time()
        // perf events: an armed sampler needs this softirq every tick
        // to charge the IRQ-stashed samples (perf::softirq_tick). One
        // relaxed load when nothing is armed (the sc184 discipline).
        || crate::perf::any_armed()
}

fn mark_timer_armed() {
    TIMER_EVER_ARMED.store(true, Ordering::Release);
}

// ==================== Timed-sleep shadow (ticktrap, r41v17) ==============

/// Timed-sleep shadow kinds (`Task::timed_sleep_kind`).
pub const TSLEEP_NONE: u32 = 0;
/// nanosleep/clock_nanosleep class: deadline is absolute time-CSR ticks,
/// wheel registration is an hres one-shot.
pub const TSLEEP_HRES: u32 = 1;
/// futex/poll timeout class: deadline is absolute jiffies, wheel
/// registration is a jiffy one-shot.
pub const TSLEEP_JIFFY: u32 = 2;

/// RAII shadow for a scoped timed wait: stamps the current task's
/// timed-sleep fields at construction, clears them at scope exit (Drop
/// runs on every early return). The 1 Hz heartbeat reads the shadow to
/// adjudicate parked timed sleepers — deadline passed with no wheel
/// entry is a lost fire/wake; deadline far future is a long (or
/// garbage-duration) sleep; entry present and not due is healthy.
pub struct TimedSleepShadow;

impl TimedSleepShadow {
    pub fn new(kind: u32, deadline: u64, timer_id: u64, dur_ms: u32) -> Option<Self> {
        let t = crate::sched::current()?;
        // SAFETY: t is the currently running task (we are it).
        unsafe {
            (*t).timed_sleep_deadline.store(deadline, Ordering::Release);
            (*t).timed_sleep_kind.store(kind, Ordering::Release);
            (*t).timed_sleep_timer_id.store(timer_id, Ordering::Release);
            (*t).timed_sleep_armed_j
                .store(crate::drivers::timer::get_jiffies() as u32, Ordering::Release);
            (*t).timed_sleep_dur_ms.store(dur_ms, Ordering::Release);
        }
        Some(TimedSleepShadow)
    }

    /// Refresh the live registration id after a re-arm (nanosleep's
    /// belt-and-braces re-arm replaces the id mid-wait; the heartbeat
    /// must name the LIVE entry).
    pub fn refresh_id(&self, timer_id: u64) {
        if let Some(t) = crate::sched::current() {
            // SAFETY: t is the currently running task (we are it).
            unsafe {
                (*t).timed_sleep_timer_id.store(timer_id, Ordering::Release);
            }
        }
    }
}

impl Drop for TimedSleepShadow {
    fn drop(&mut self) {
        if let Some(t) = crate::sched::current() {
            // SAFETY: t is the currently running task (we are it). A
            // drop on a task switch away cannot happen: the guard lives
            // in the wait function's own frame, above schedule().
            unsafe {
                (*t).timed_sleep_kind.store(TSLEEP_NONE, Ordering::Release);
                (*t).timed_sleep_deadline.store(0, Ordering::Release);
                (*t).timed_sleep_timer_id.store(0, Ordering::Release);
            }
        }
    }
}

/// Read the earliest outstanding high-resolution deadline (u64::MAX = none).
pub fn hres_next_deadline() -> u64 {
    HRES_NEXT.load(Ordering::Acquire)
}

/// Derive an hres entry's first-check jiffy from its precise time-CSR
/// deadline via the timer driver's exact grid mapping. The wheel routes
/// hres entries by this value — callers' jiffy parameters stay
/// diagnostics-only (the pre-wheel comment promised exactly that, and
/// the wheel keeps the promise structural).
fn hres_horizon_for(precise: u64) -> u64 {
    timer::jiffy_floor_of_time(precise)
}

/// The typed cascading wheel: fixed slot pool + occupancy bitmap +
/// bucket lists + cursor, guarded by ONE spinlock (R37: all mutations
/// are plain stores — no code path under TIMER_TABLE can touch the
/// heap, so no alloc_error_handler panic can ever fire with the lock
/// held; R12-3: delivery happens only after the lock is released).
/// ~66 KB of .bss (1024 slots × 64 B + bitmap + bucket arrays).
static TIMER_TABLE: Spinlock<wheel::TimerWheel> =
    Spinlock::new(wheel::TimerWheel::new());

/// A pending timer, as /proc/timer_list reports it.
#[derive(Clone, Copy)]
pub struct TimerSnapshot {
    /// The timer id (the map key).
    pub id: u64,
    /// The absolute expiry in time-CSR ticks (100ns each); 0 for a
    /// jiffy-only timer.
    pub expires_time: u64,
    /// The expiry in jiffies.
    pub expires: u64,
    /// The action: target pid (0 = none).
    pub pid: u32,
    /// The action: signal number (0 = none).
    pub signo: i32,
    /// The action: repeat interval in jiffies (0 = one-shot).
    pub interval_jiffies: u64,
    /// The action: the pid to wake on expiry (0 = none).
    pub wake_pid: u32,
    /// The action: the timerfd address (0 = not a timerfd).
    pub tfd_addr: u64,
}

/// A snapshot of the pending timers, copied out under the timer table
/// lock so the caller never prints while it is held. R37: snapshot order
/// is slot order (was id-ascending under the BTreeMap); /proc/timer_list
/// has no ordering contract and its consumer is the wakewatch census,
/// which counts.
pub fn snapshot_timers() -> alloc::vec::Vec<TimerSnapshot> {
    let table = TIMER_TABLE.lock();
    let mut out = alloc::vec::Vec::with_capacity(table.len());
    table.push_snapshots(&mut out);
    out
}

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
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    let action = if wake_pid != 0 {
        OneshotAction::Wake { pid: wake_pid }
    } else {
        OneshotAction::None
    };

    // R37 discipline: fixed-slot occupy — a plain store, replacing the
    // old `timers.insert()` / `actions.insert()` BTreeMap node
    // allocations under the TIMERS→ACTIONS locks (the R36 OOM-deadlock
    // family).
    let mut table = TIMER_TABLE.lock_irqsave();
    if table.arm_oneshot_jiffy(expires, action, id) {
        id
    } else {
        0 // pool full (the old `timers.len() >= MAX_TIMERS` check)
    }
}

/// Add a one-shot HIGH-RESOLUTION wake timer: fires when the time CSR
/// reaches `expires_time` (absolute ticks). `expires` is bookkeeping
/// only for hres entries — collection routing is DERIVED from
/// `expires_time` on the jiffy grid (`hres_horizon_for`), and the
/// expiry decision uses the precise compare exclusively (the old
/// jiffy-fallback early fire consumed the one-shot wake and hung
/// sleepers — the lost-timer-wakeup root cause; now the two clocks
/// have distinct types with no cross-compare). Used by
/// nanosleep/clock_nanosleep.
pub fn add_timer_wakeup_hres(expires: u64, expires_time: u64, wake_pid: u32) -> u64 {
    let _ = expires; // diagnostics-only for hres arms (kept for API symmetry)
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    let action = if wake_pid != 0 {
        OneshotAction::Wake { pid: wake_pid }
    } else {
        OneshotAction::None
    };
    let horizon = hres_horizon_for(expires_time);

    let mut table = TIMER_TABLE.lock_irqsave();
    if !table.arm_oneshot_hres(expires_time, horizon, action, id) {
        return 0; // pool full
    }
    // Keep the global earliest-hres view current so the caller (on its own
    // hart, before sleeping) can re-arm the hart timer to the earlier
    // deadline.
    if expires_time != 0 && expires_time < HRES_NEXT.load(Ordering::Acquire) {
        HRES_NEXT.store(expires_time, Ordering::Release);
    }

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
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    // R37: zero allocation under the lock (see add_timer_wakeup).
    // The interval split maps the legacy `interval_jiffies` parameter
    // onto the typed classes: 0 = one-shot (no interval to mis-pass),
    // n > 0 = periodic (NonZeroU64 by type — the busy-loop class is
    // unconstructible).
    let mut table = TIMER_TABLE.lock_irqsave();
    let armed = match non_zero_interval(interval_jiffies) {
        None => table.arm_oneshot_jiffy(
            expires,
            OneshotAction::from_legacy_oneshot(pid, signo, tfd_addr),
            id,
        ),
        Some(interval) => table.arm_periodic_jiffy(
            expires,
            PeriodicAction::from_legacy_periodic(pid, signo, tfd_addr),
            interval,
            id,
        ),
    };
    if armed {
        id
    } else {
        0 // pool full
    }
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
    let _ = expires; // diagnostics-only for hres arms (kept for API symmetry)
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    let horizon = hres_horizon_for(expires_time);
    let mut table = TIMER_TABLE.lock_irqsave();
    let armed = match non_zero_interval(interval_jiffies) {
        None => table.arm_oneshot_hres(
            expires_time,
            horizon,
            OneshotAction::from_legacy_oneshot(pid, signo, tfd_addr),
            id,
        ),
        Some(interval) => table.arm_periodic_hres(
            expires_time,
            horizon,
            PeriodicAction::from_legacy_periodic(pid, signo, tfd_addr),
            interval,
            id,
        ),
    };
    if !armed {
        return 0; // pool full
    }
    if expires_time != 0 && expires_time < HRES_NEXT.load(Ordering::Acquire) {
        HRES_NEXT.store(expires_time, Ordering::Release);
    }

    id
}

/// Interval jiffies → Option<NonZeroU64> (0 = one-shot = None). The
/// nonzero type makes "interval 0 periodic" (the busy-loop bug class)
/// unrepresentable on the wheel.
fn non_zero_interval(interval_jiffies: u64) -> Option<core::num::NonZeroU64> {
    if interval_jiffies == 0 {
        None
    } else {
        // SAFETY-free: checked construction.
        core::num::NonZeroU64::new(interval_jiffies)
    }
}

/// Delete a timer and its associated action.
///
/// # Returns
/// `true` if timer was found and removed.
pub fn del_timer(id: u64) -> bool {
    del_note(id);
    // R37: single lock — the old TIMERS→ACTIONS nesting (the lock-order
    // comment used to live here) is gone: entry and action are one slot.
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.remove(id) {
        wheel::RemoveOutcome::NotFound => false,
        wheel::RemoveOutcome::RemovedJiffy => true,
        wheel::RemoveOutcome::RemovedHres(_) => {
            // The deleted timer may have been the global hres minimum;
            // recompute now so hart re-arms stop over-firing sooner (the
            // cheap next recompute in the softirq would fix it too).
            let next = table.min_hres();
            drop(table);
            HRES_NEXT.store(next, Ordering::Release);
            true
        }
    }
}

/// Check if a timer is currently active.
// nanosleep-loss conviction: del_timer ring (id + caller line).
pub static DEL_IX: AtomicUsize = AtomicUsize::new(0);
pub static DEL_ID: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
pub static DEL_RA: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
pub static DEL_JIF: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];

#[track_caller]
fn del_note(id: u64) {
    let slot = DEL_IX.fetch_add(1, Ordering::Relaxed) % 64;
    DEL_ID[slot].store(id, Ordering::Relaxed);
    DEL_RA[slot].store(
        core::panic::Location::caller().line() as u64,
        Ordering::Relaxed,
    );
    DEL_JIF[slot].store(crate::drivers::timer::get_jiffies(), Ordering::Relaxed);
}

/// Was `id` deleted (and from which source line, how long ago)?
pub fn del_lookup(id: u64) -> Option<(u64, u64)> {
    for k in 0..64 {
        if DEL_ID[k].load(Ordering::Relaxed) == id {
            return Some((
                DEL_RA[k].load(Ordering::Relaxed),
                crate::drivers::timer::get_jiffies()
                    .saturating_sub(DEL_JIF[k].load(Ordering::Relaxed)),
            ));
        }
    }
    None
}

pub fn timer_pending(id: u64) -> bool {
    let table = TIMER_TABLE.lock_irqsave();
    table.contains(id)
}

/// Query a pending timer's absolute state.
///
/// Returns `Some((expires_jiffies, interval_jiffies))` while the timer is
/// pending; `interval_jiffies` is 0 for one-shot timers. Used by
/// getitimer/setitimer to report the remaining ITIMER_REAL time.
pub fn get_timer_state(id: u64) -> Option<(u64, u64)> {
    let table = TIMER_TABLE.lock_irqsave();
    table.state_of(id)
}

// ==================== Softirq Handler ====================

/// Timer softirq handler.
///
/// Called from `__do_softirq()` when Hrtimer softirq is raised.
/// Advances the wheel, collects due expiries (typed tokens), and
/// delivers outside the lock. Periodic timers are re-armed
/// automatically (same id, re-routed in place).
pub fn timer_softirq_handler(_nr: usize) {
    let current = timer::get_jiffies();
    let last = LAST_TICK.load(Ordering::Relaxed);

    // High-resolution timers are NOT jiffy-deduped: an extra hart IRQ fired
    // at a sub-jiffy deadline re-runs this scan even in the same jiffy.
    let now_time = crate::drivers::timer::read_time();
    let hres_due = HRES_NEXT.load(Ordering::Acquire) <= now_time;

    // Wakewatch forensics: stamp this CPU's Hrtimer-softirq entry
    // unconditionally (even a deduped no-op run proves the vector is
    // being serviced), then run the 1 Hz heartbeat + stall watchdog.
    {
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < crate::config::MAX_CPUS {
            WW_HRES_LAST[cpu].store(now_time, Ordering::Release);
            WW_HRES_N[cpu].fetch_add(1, Ordering::Relaxed);
        }
        // Stall watchdog: a CPU whose last Hrtimer run aged past 1 s
        // while the machine is up is timer-starved (IRQ-off windows or
        // softirq overflow with a starved ksoftirqd). One print per
        // episode start, rate-gated globally.
        //
        // UART perf gate (2026-10-20): the report now also carries the
        // stalled CPU's timer-IRQ arrival age and its current task
        // (pid/preempt_count/RIP). tick_age ≈ softirq age ⇒ that CPU's
        // timer IRQs stopped arriving too ⇒ a long IF=0 window (UART
        // THRE poll under the console lock was the convicted producer).
        // tick_age small but softirq age large ⇒ IRQs arrived and the
        // dispatch was skipped (in_softirq deferral to a starved
        // ksoftirqd, or stuck SOFTIRQ_IN_PROGRESS).
        for c in 0..crate::config::MAX_CPUS {
            let l = WW_HRES_LAST[c].load(Ordering::Acquire);
            if l != 0 && now_time.saturating_sub(l) > crate::config::TIMER_CLOCK_FREQ_HZ {
                if ww_latefire_budget_ok() {
                    let tick_age_ms = ww_tick_age_ms(c);
                    let (pid, pc, rip) = crate::sched::sched::ww_stalled_cpu_state(c);
                    // sc167 storm gate: a USER-space rip means the stalled
                    // CPU was running userspace with IRQs enabled — the
                    // tick simply stopped ARRIVING (KVM hrtimer delay under
                    // host load; 2477 of sc166's 2503 STALL lines were this
                    // family). It is not a Rux IF=0 window; printing it
                    // burns a direct-console line with IF=0 and feeds the
                    // storm. Only report stalls whose current context is
                    // kernel text.
                    let (klo, khi) = crate::sched::sched::kernel_text_window();
                    if rip >= klo && rip < khi {
                        crate::pr_err!(
                            "WW: TIMER-SOFTIRQ-STALL cpu={} age={}ms tick_age={}ms n={} pid={} pc={:#x} rip={:#x}",
                            c,
                            (now_time.saturating_sub(l)) * 1000
                                / crate::config::TIMER_CLOCK_FREQ_HZ,
                            tick_age_ms,
                            WW_HRES_N[c].load(Ordering::Relaxed),
                            pid,
                            pc,
                            rip
                        );
                    }
                }
            }
        }
        // 1 Hz heartbeat (100 jiffies at HZ=100).
        let hb_last = WW_LAST_HEARTBEAT.load(Ordering::Relaxed);
        if current.wrapping_sub(hb_last) >= 100 && WW_LAST_HEARTBEAT
            .compare_exchange(hb_last, current, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // Hunt instrument (serial-noise policy): the WW/WW2/WW3 census
            // lines print only on dfx=wwcensus boots; ordinary boots keep
            // a clean serial prompt.
            if crate::trace::EVENT_WWCENSUS.armed() {
                ww_heartbeat(now_time);
            }
            // leg-27: in-flight binder txn hang audit (dfx=txnhunt,
            // self-gated + rate-capped inside). Runs on the same ~1 Hz
            // heartbeat slot; zero cost when off.
            crate::ipc::binder::txn_hang_audit();
            // holder-chain lane (dfx=txnhunt, same switch): BWR-park
            // audit — every binder thread with a transaction stack (or
            // an anomalous ioctl park) gets one line naming its CURRENT
            // syscall + the decoded write sub-command of the BWR it is
            // parked in; the FFRT mutex holders parked inside a binder
            // ioctl (the futex-census blind spot) surface here.
            crate::ipc::binder::bwr_park_audit();
            // leg-30B: AMS46 node/proc census (dfx=ams46, self-gated +
            // rate-capped inside) — the Gate-1 delivery adjudication
            // backdrop for the accountmgr->AMS StartUser oneway.
            crate::ipc::binder::ams46_audit();
            // leg-29A: process-tree invariant validator (dfx=treehunt,
            // self-gated + rate-capped inside) — E8-REPAR unlinked-
            // snapshot root-cause hunt.
            crate::dfx::treehunt::tick();
            // leg-33B: slow-kernel-entry audit (dfx=slowtask, self-gated +
            // rate-capped inside) — leaf attribution for nested-call
            // cascades (fs/futex stalls behind reader exhaustion).
            crate::dfx::slowtask::tick();
            // 17th successor: futex-stuck census (dfx=fuxstuck, self-gated
            // + rate-capped inside) — per-condvar producer-starvation
            // adjudication for the OH post-creation stall web.
            crate::sync::futex::futex_stuck_census();
            // 53rd (52nd §6a): early-cadence S-state sleep snapshot
            // (dfx=sleepsnap inside): every ~5 s for the first minute, so
            // a 7.4 s park (hilogd main) is captured with its parked
            // kernel stack while the boot is still young; the khungtaskd
            // 60 s cadence continues afterward.
            crate::dfx::taskdump::sleep_snapshot_early();
            // 69th (105-family hunt): pid=1 RUNNING-spin watch (dfx=
            // pid1spin, self-gated + rate-capped inside) — init going
            // 100%-CPU silent is the sole remaining 105 cause; this
            // names the spin site via the kernel-entry frame.
            crate::dfx::taskdump::pid1spin_audit();
            // 99th (§87.8 wait4/SIGCHLD wakeup-loss hunt): wait4-park
            // census — self-gated 1 Hz + anomaly-gated inside (zero
            // serial on healthy boots); a parked waiter with a reapable
            // zombie child prints the full wake-chain verdict line.
            crate::process::exit::wait4_park_audit();
            // stress-port SP-5: direct-reclaim gate census — names the
            // holder's phase/wait-site when the gate has been held >=2 s
            // (self-gated: one atomic load when the gate is free; zero
            // serial on standard boots).
            crate::mm::page_alloc::reclaim_gate_census();
            // stress-port SP-7: per-page swap skip-reason census — one
            // line per second ONLY while anon reclaim is being exercised
            // (self-gated on census deltas; zero serial otherwise).
            crate::mm::vmscan::vscan_heartbeat();
            // stress-port r9 (kill10 face-1 handoff): overdue-wheel
            // anomaly — an entry still IN the wheel more than 2 s past
            // its deadline is a wheel-side loss (the delivery-loss face
            // leaves the wheel EMPTY while the waiter stays parked;
            // absence + a parked waiter is the other branch). Anomaly-
            // gated unconditional print (ticktrap precedent), globally
            // rate-capped at 1/s, max 4 entries per heartbeat.
            {
                let snaps = snapshot_timers();
                let mut budget = if ww_latefire_budget_ok() { 4u32 } else { 0u32 };
                for t in snaps.iter() {
                    let overdue_ticks = if t.expires_time != 0 {
                        now_time.saturating_sub(t.expires_time)
                    } else {
                        // jiffy-only entry: compare at the jiffy grid
                        current
                            .saturating_sub(t.expires)
                            .saturating_mul(crate::config::TIMER_CLOCK_FREQ_HZ
                                / crate::config::KERNEL_HZ as u64)
                    };
                    if overdue_ticks > 2 * crate::config::TIMER_CLOCK_FREQ_HZ && budget > 0 {
                        budget -= 1;
                        crate::pr_err!(
                            "HRES-OVERDUE id={:#x} expires_time={} age_ms={} pid={} signo={} wake_pid={} interval_jiffies={}",
                            t.id,
                            t.expires_time,
                            overdue_ticks / (crate::config::TIMER_CLOCK_FREQ_HZ / 1000),
                            t.pid,
                            t.signo,
                            t.wake_pid,
                            t.interval_jiffies
                        );
                    }
                }
            }
            // r41v17 ticktrap: the OTHER branch of the r9 dichotomy —
            // "absence + a parked waiter". Every task parked in a timed
            // sleep (shadow kind set, state sleeping, age >= 2 s) gets
            // one line naming its loss branch:
            //   deadline PASSSED + wheel entry absent -> lost fire/wake
            //     (the futex R32 class, or a wheel-side loss);
            //   deadline in the FUTURE (entry absent or present) -> an
            //     intended-or-garbage LONG sleep (r41v15 power_host
            //     adjudication);
            //   entry present + not due -> healthy.
            // No user memory is read (the bwr_park_audit lesson). Same
            // r9 precedent: anomaly-gated, unconditional, globally
            // rate-limited 1/s, pid-deduped 60 s, max 4 lines/beat.
            {
                struct TtRow {
                    pid: u32,
                    kind: u32,
                    armed_j: u32,
                    deadline: u64,
                    timer_id: u64,
                    dur_ms: u32,
                    /// GHOST branch: parked with a nanosleep-class entry
                    /// frame but NO shadow — the frame's syscall is not
                    /// where the task sleeps. Carries stack words from
                    /// the saved kernel sp for offline symbolization.
                    ghost: bool,
                    ghost_sys: u64,
                    stk: [u64; 4],
                }
                // Per-arch nanosleep-class syscall numbers (entry-frame
                // decode, same contract as the TXN-HANG sys= read).
                #[cfg(feature = "x86_64")]
                const NS_SYS: [u64; 2] = [35, 230]; // nanosleep, clock_nanosleep
                #[cfg(feature = "riscv64")]
                const NS_SYS: [u64; 2] = [35, 415]; // nanosleep, clock_nanosleep_time64
                #[cfg(feature = "aarch64")]
                const NS_SYS: [u64; 2] = [115, 115]; // clock_nanosleep
                let entry_sys = |p: *mut crate::process::Task| -> u64 {
                    if p.is_null() {
                        return 0;
                    }
                    // SAFETY: entry-frame read of a live task's mapped
                    // kernel stack (sleep_snapshot / TXN-HANG contract).
                    unsafe {
                        let tk = &*p;
                        let top = match tk.kernel_stack_top_checked() {
                            Some(t) => t,
                            None => return 0,
                        };
                        #[cfg(feature = "x86_64")]
                        {
                            let regs = (top.wrapping_sub(
                                crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                            )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                            return (*regs).orig_rax;
                        }
                        #[cfg(feature = "riscv64")]
                        {
                            let regs = (top.wrapping_sub(
                                crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                            )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                            return (*regs).a7;
                        }
                        #[cfg(feature = "aarch64")]
                        {
                            let regs = (top.wrapping_sub(
                                crate::arch::aarch64::pt_regs::PT_REGS_SIZE,
                            )) as *const crate::arch::aarch64::pt_regs::PtRegs;
                            return (*regs).syscallno;
                        }
                    }
                };
                let mut rows: [Option<TtRow>; 16] = [None, None, None, None, None, None, None,
                    None, None, None, None, None, None, None, None, None];
                let mut ghost_rows = 0usize;
                let nfound = crate::sched::for_each_task_try(|t| {
                    if t.is_null() {
                        return;
                    }
                    // SAFETY: diagnostic reads of a live task pinned by
                    // the pid-hash walk.
                    let (kind, sleeping) = unsafe {
                        (
                            (*t).timed_sleep_kind.load(Ordering::Acquire),
                            (*t).state().is_sleeping(),
                        )
                    };
                    if !sleeping {
                        return;
                    }
                    if kind == TSLEEP_NONE {
                        // GHOST hunt: parked under a nanosleep-class
                        // entry frame with no live nanosleep — the
                        // r41v15/v17/v18 power_host shape (sys=35,
                        // INTERRUPTIBLE for minutes, shadow never set).
                        // Cap ghosts at 4 rows.
                        if ghost_rows < 4 {
                            let sys = entry_sys(t);
                            if NS_SYS.contains(&sys) {
                                ghost_rows += 1;
                                // Saved-sp words from the CONTEXT-SWITCH
                                // saved sp (thread().sp — the live park
                                // site), NOT ti_kernel_sp (creation-time
                                // only). Same read contract as TXN-HANG-STK.
                                let mut stk = [0u64; 4];
                                unsafe {
                                    let tk = &*t;
                                    let sp = tk.thread().sp as usize;
                                    let top = tk.kernel_stack_top_checked().unwrap_or(0);
                                    let base = top.wrapping_sub(
                                        crate::config::KERNEL_STACK_SIZE,
                                    );
                                    if sp >= base && sp + 8 * 4 <= top && (sp & 7) == 0 {
                                        for (i, w) in stk.iter_mut().enumerate() {
                                            *w = core::ptr::read_volatile(
                                                (sp + i * 8) as *const u64,
                                            );
                                        }
                                    }
                                }
                                let pid = unsafe { (*t).pid() };
                                for r in rows.iter_mut() {
                                    if r.is_none() {
                                        *r = Some(TtRow {
                                            pid,
                                            kind: 0,
                                            armed_j: 0,
                                            deadline: 0,
                                            timer_id: 0,
                                            dur_ms: 0,
                                            ghost: true,
                                            ghost_sys: sys,
                                            stk,
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                        return;
                    }
                    // Per-kind age gate: a parked hres (nanosleep) wait
                    // >= 2 s is already unusual; FFRT's routine 60 s
                    // futex timeouts would flood the print, so the jiffy
                    // class reports only >= 30 s parks.
                    // SAFETY: same diagnostic-read contract.
                    let (pid, armed_j, deadline, tid, dur) = unsafe {
                        (
                            (*t).pid(),
                            (*t).timed_sleep_armed_j.load(Ordering::Acquire),
                            (*t).timed_sleep_deadline.load(Ordering::Acquire),
                            (*t).timed_sleep_timer_id.load(Ordering::Acquire),
                            (*t).timed_sleep_dur_ms.load(Ordering::Acquire),
                        )
                    };
                    let gate = if kind == TSLEEP_HRES { 200 } else { 3000 };
                    if current.wrapping_sub(armed_j as u64) < gate {
                        return;
                    }
                    for r in rows.iter_mut() {
                        if r.is_none() {
                            *r = Some(TtRow { pid, kind, armed_j, deadline, timer_id: tid, dur_ms: dur, ghost: false, ghost_sys: 0, stk: [0; 4] });
                            break;
                        }
                    }
                });
                if nfound > 0 && rows.iter().any(|r| r.is_some()) {
                    // Own 1/s budget — the WW family's shared gate must
                    // not starve this print (r41v18: benign wake-drops
                    // consumed every second, the ticktrap never fired).
                    static TT_LAST: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0);
                    let tl = TT_LAST.load(Ordering::Relaxed);
                    let tt_ok = current.wrapping_sub(tl) >= 100
                        && TT_LAST
                            .compare_exchange(tl, current, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok();
                    let snaps = snapshot_timers();
                    let mut printed = 0;
                    for r in rows.iter().flatten() {
                        if printed >= 4 || !tt_ok {
                            break;
                        }
                        printed += 1;
                        if r.ghost {
                            crate::pr_debug!(
                            "TIMEDSLEEP-GHOST pid={} sys={} no-shadow stk={:#x} {:#x} {:#x} {:#x}",
                                r.pid,
                                r.ghost_sys,
                                r.stk[0],
                                r.stk[1],
                                r.stk[2],
                                r.stk[3]
                            );
                            continue;
                        }
                        // Wheel state for this sleeper: the live
                        // registration (by id) or any wake entry
                        // targeting the pid.
                        let mut wheel = "none";
                        'w: for s in snaps.iter() {
                            if (r.timer_id != 0 && s.id == r.timer_id)
                                || (s.wake_pid == r.pid && s.wake_pid != 0)
                            {
                                wheel = "entry";
                                break 'w;
                            }
                        }
                        let (domain, passed) = if r.kind == TSLEEP_HRES {
                            ("hres", now_time >= r.deadline)
                        } else {
                            ("jiffy", current >= r.deadline)
                        };
                        let (rel_ms, rel_txt) = if r.kind == TSLEEP_HRES {
                            let d = now_time as i64 - r.deadline as i64;
                            (
                                d.abs() * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ as i64,
                                if d >= 0 { "past" } else { "future" },
                            )
                        } else {
                            let d = current as i64 - r.deadline as i64;
                            (d.abs() * 1000 / crate::config::KERNEL_HZ as i64, if d >= 0 { "past" } else { "future" })
                        };
                        crate::pr_debug!(
                            "TIMEDSLEEP-PARK pid={} kind={} age_s={} dur_ms={} deadline_{}ms_{} wheel={} timer_id={:#x}",
                            r.pid,
                            domain,
                            current.wrapping_sub(r.armed_j as u64) / 100,
                            r.dur_ms,
                            rel_ms,
                            rel_txt,
                            wheel,
                            r.timer_id
                        );
                    }
                }
            }
        }
    }

    if current == last && !hres_due {
        return;
    }
    // R20-7: record the processed jiffy so a second softirq within the same
    // jiffy returns early instead of re-scanning (and re-locking) the timer
    // maps. This store was missing, so the dedupe above never fired and the
    // full scan ran on every raise (TIMERS/ACTIONS lock churn).
    LAST_TICK.store(current, Ordering::Release);

    // VW forensic instrumentation: per-jiffy virtio completion-lag
    // watchdog (prints walker/ring state when PENDING_LAST lags the used
    // ring with no scheduled consumer — the ftest01 wedge family).
    crate::drivers::virtio::vw_lag_watchdog(current);

    // dfx=claiminject (GNOME-wedge adjudication): every 50 jiffies, leak
    // a foreign claim onto the linked pickable set. Unpatched kernels
    // must wedge (the g2 stranding reproduced deterministically); kernels
    // with the pick-side stale-claim heal must keep scheduling.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::ClaimInject)
        && current % 50 == 0
    {
        let n = crate::sched::sched::dfx_claim_inject();
        if n > 0 {
            crate::pr_debug!(
                            "CLAIM-INJECT poisoned={}", n);
        }
    }

    // dfx=oncpuinject (R85 tail-wedge adjudication): every 50 jiffies,
    // stamp the lost-release ownership shape {on_cpu=1, claim=7, LIVE}
    // onto the linked pickable set. Unpatched kernels strand the poisoned
    // tasks exactly like the pid-1 tail wedge (every pick skips them
    // why=1 forever); kernels with the R85 pick-side ownership
    // adjudication must sweep the poison (STALE-ONCPU-RESET) and keep
    // scheduling.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::OncpuInject)
        && current % 50 == 0
    {
        let n = crate::sched::sched::dfx_oncpu_inject();
        if n > 0 {
            crate::pr_debug!(
                            "ONCPU-INJECT poisoned={}", n);
        }
    }

    // Advance + collect under the lock; deliver AFTER releasing it
    // (R12-3 — see the delivery block below). The sweep body lives in
    // `run_wheel_sweep` (RT-2X split) so the hardirq fast path
    // `expire_hres_inline` runs the identical sequence at the top of
    // the timer IRQ; the diagnostics above this point stay
    // softirq-side only.
    static mut EXPIRED_BUF: [[FiredToken; wheel::EXPIRY_BUDGET];
        crate::config::MAX_CPUS] =
        [[FiredToken {
            id: 0,
            action: Action::None,
            late_ticks: 0,
            deadline: 0,
        }; wheel::EXPIRY_BUDGET]; crate::config::MAX_CPUS];
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return; // unreachable on this platform; keep the handler total
    }
    // SAFETY: per-CPU rows indexed by this CPU's id; the SOFTIRQ_IN_PROGRESS
    // guard prevents re-entry, so this CPU's row is exclusively ours.
    let buf_ptr: *mut [FiredToken; wheel::EXPIRY_BUDGET] =
        unsafe { (core::ptr::addr_of_mut!(EXPIRED_BUF) as *mut [FiredToken; wheel::EXPIRY_BUDGET]).add(cpu) };
    run_wheel_sweep(current, now_time, hres_due, buf_ptr);
    softirq_diagnostics(current);
}

/// RT-2X item 4 (pair-battery queue): hardirq-context fast expiry, the
/// reference's hrtimer_interrupt shape. Called at the TOP of the timer
/// IRQ (before the tick body) whenever a high-resolution deadline is
/// due: the precise-sleep wake then happens before the per-tick
/// accounting/scheduler_tick/scribble/softlockup body instead of after
/// it — measured 30-70 us of body between the LAPIC fire and the wake
/// on the debug kernel (run/RT-2X.md). Delivery context is the same
/// no-sleep contract as the softirq sweep (R12-3: tokens delivered
/// outside the TIMER_TABLE lock); all locks taken on this path are
/// irqsave spinlocks already shared with the softirq caller.
///
/// Uses its OWN token buffer: a timer IRQ can land while the softirq
/// deliver loop is mid-iteration over the per-CPU EXPIRED_BUF row (the
/// sweep's TIMER_TABLE critical section is irqsave and thus
/// un-interruptible, but the deliver phase is not) — sharing the row
/// would corrupt the in-flight delivery. This path cannot nest with
/// itself: the LAPIC one-shot never refires while its own vector is in
/// service (re-arm happens at tick_nohz_irq_exit, after this handler).
pub fn expire_hres_inline() -> bool {
    let now_time = crate::drivers::timer::read_time();
    if HRES_NEXT.load(Ordering::Acquire) > now_time {
        return false;
    }
    // Wakewatch stamping parity with the softirq path (the stall
    // watchdog reads these; the softirq dispatch later in the same IRQ
    // re-stamps — a few ns of atomics, worth keeping the forensics
    // unambiguous).
    {
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < crate::config::MAX_CPUS {
            WW_HRES_LAST[cpu].store(now_time, Ordering::Release);
            WW_HRES_N[cpu].fetch_add(1, Ordering::Relaxed);
        }
    }
    static mut HRES_FAST_BUF: [FiredToken; wheel::EXPIRY_BUDGET] =
        [FiredToken { id: 0, action: Action::None, late_ticks: 0, deadline: 0 }; wheel::EXPIRY_BUDGET];
    // SAFETY: single-CPU-use buffer owned by this call path (see the
    // nesting note above); the wheel writes at most EXPIRY_BUDGET tokens.
    let buf: *mut [FiredToken; wheel::EXPIRY_BUDGET] =
        core::ptr::addr_of_mut!(HRES_FAST_BUF) as *mut [FiredToken; wheel::EXPIRY_BUDGET];
    let current = timer::get_jiffies();
    run_wheel_sweep(current, now_time, true, buf);
    true
}

/// Advance + collect + deliver one wheel pass (the old tail of
/// `timer_softirq_handler`, split for the RT-2X hardirq fast path).
/// `buf` is the caller's dedicated expired-token storage: the softirq
/// path passes the per-CPU EXPIRED_BUF row (re-entry impossible via the
/// SOFTIRQ guard), the hardirq fast path passes its own single row (see
/// `expire_hres_inline` — the two never alias: a timer IRQ landing in
/// the softirq deliver loop overwrites only its own row, and the
/// interrupted softirq iterates tokens it already copied out... it does
/// NOT: it borrows the row. That is why the hardirq path carries a
/// SEPARATE buffer).
///
/// R34: the collection budget bounds the per-pass work; leftover due
/// entries stay in their wheel list and drain on the next pass.
fn run_wheel_sweep(
    current: u64,
    now_time: u64,
    hres_due: bool,
    buf_ptr: *mut [FiredToken; wheel::EXPIRY_BUDGET],
) {
    // SAFETY: caller-owned row (per-CPU softirq row or the hardirq fast
    // row); the wheel writes at most EXPIRY_BUDGET tokens.
    let expired_len = {
        let now = Now::new(current, now_time);
        let mut table = TIMER_TABLE.lock_irqsave();
        table.advance(now);
        let row: &mut [FiredToken; wheel::EXPIRY_BUDGET] = unsafe { &mut *buf_ptr };
        table.collect(now, row)
    }; // TIMER_TABLE released here

    // Refresh the earliest-hres view: fired/removed timers may have been
    // the minimum. Cheap scan under TIMERS only.
    if hres_due {
        let next = {
            let table = TIMER_TABLE.lock_irqsave();
            table.min_hres()
        };
        HRES_NEXT.store(next, Ordering::Release);
    }

    // R12-3: delivery OUTSIDE the timer locks. The old in-lock
    // wake_up_process created a TIMERS -> GRQ nesting that wedged all
    // CPUs on the TIMERS lock whenever the GRQ side stalled (observed
    // after pipelines). The tokens are a detached snapshot; the ids
    // were removed from the wheel under the lock above (or re-armed
    // in place with a fresh delivery queued for the NEXT expiry).
    // The SoftirqCtx token records the delivery-context contract.
    let ctx = SoftirqCtx::new();
    // SAFETY: slice of the caller's row, filled under the lock above.
    let expired: &[FiredToken] = &unsafe { &*buf_ptr }[..expired_len];
    deliver_fired(expired, ctx);
}

/// The softirq-side 1 Hz-class diagnostics that used to trail the sweep
/// inside `timer_softirq_handler` (RT-2X split: the hardirq fast path
/// runs the sweep only, never these). `current` is the jiffy at dispatch.
fn softirq_diagnostics(current: u64) {
    // DFX dfx=periodic: a task snapshot every PERIODIC_DUMP_SECS. Silent
    // hangs (form-B) trip no watchdog and swallow the UART magic (a wedged
    // shell stops draining the RX path) — a timed snapshot from the timer
    // softirq always lands. SBI-direct, no locks held here.
    // sc115: the 500-jiffy (5 s) interval x ~500 tasks floods the
    // busy-poll UART (~87 us/byte charged to whoever is current); 3000
    // jiffies (30 s) still catches a hung launcher child's parked
    // syscall — the children live ~10 s between LIFETIME_TIMEOUT and
    // the AMS restart, so every other dump sees one — without
    // distorting the boot it is diagnosing.
    if crate::trace::EVENT_TASKDUMP.armed()
        && current.saturating_sub(LAST_PERIODIC_DUMP.load(Ordering::Relaxed)) >= 3000
    {
        LAST_PERIODIC_DUMP.store(current, Ordering::Relaxed);
        // dfx-migrate: rux:taskdump kind=PERIODIC row + the serial dump
        // (echo consumer of the same event).
        crate::trace::trace_taskdump(1, current);
        crate::dfx::taskdump::dump_all_tasks("periodic");
    }

    // lmbench-fix lane: sweep the unlinked-RUNNING phantom (the
    // lat_sem/lat_connect/bw_tcp wedge family — a task frozen
    // RUNNING+unqueued+slotless is reached by no pick and no wake).
    // 1 Hz, unconditional: the scan is a pid-hash walk with an early
    // state filter; the GRQ is taken only when a candidate exists.
    if current.saturating_sub(LAST_PHANTOM_RELINK.load(Ordering::Relaxed)) >= 100 {
        LAST_PHANTOM_RELINK.store(current, Ordering::Relaxed);
        crate::sched::sched::relink_unlinked_running_phantoms();
    }

    // perf-audit line (cmdline `syscost=1`): whole-syscall cost table +
    // tick-mode accounting + kernel-rip histogram dump every 60 s from
    // this softirq. Own cmdline key — no dfx=switches.rs bit consumed.
    crate::dfx::syscost::tick(current);

    // vm-op stage profiler (cmdline `mmprof=1`, oh-launcher perf
    // handoff): mmap/munmap/mprotect stage table, same 60s cadence.
    crate::dfx::mmprof::tick(current);

    // perf events (PERF_TYPE_SOFTWARE sampling): charge the tick stash
    // captured by the hard IRQ into the armed events and emit sample
    // records. Bails on one relaxed load when nothing is armed.
    crate::perf::softirq_tick();

    // perf-audit line (cmdline `fsw=1`): ext4/bio cost-anatomy counters
    // (F4 diagnosis), same 60 s cadence.
    crate::dfx::fsw::tick(current);

    // dfx=monowatch: ~1 Hz timebase jump tripwire (backward motion or
    // multi-second spikes, clocksource named) — same printk discipline
    // as the periodic dump above.
    if crate::trace::EVENT_MONOWATCH.armed() {
        crate::dfx::monowatch::tick(current);
    }

    // dfx=wildwatch: canary re-verify on a 1 s cadence (DMA-class
    // writers bypass PTEs; the RO trap cannot see them).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WildWatch)
        && current.saturating_sub(LAST_WILDWATCH_VERIFY.load(Ordering::Relaxed)) >= 100
    {
        LAST_WILDWATCH_VERIFY.store(current, Ordering::Relaxed);
        crate::dfx::wildwatch::verify();
    }

    // dfx=metawatch: buddy-meta shadow audit on a 10 s cadence (the
    // buddy face of the wild writer — see dfx/metawatch.rs). Throttle
    // is internal to the module.
    if crate::trace::EVENT_METAWATCH.armed() {
        crate::dfx::metawatch::tick(current);
    }

    // dfx=schedtrace reaper forensics (125th waitid leg): 1 Hz zombie-
    // linger scan — names un-reaped children and what the non-reaping
    // parent is blocked on instead (dfx/reaper.rs).
    if crate::trace::EVENT_SCHEDTRACE.armed()
        && current.saturating_sub(LAST_REAPER_SCAN.load(Ordering::Relaxed)) >= 100
    {
        LAST_REAPER_SCAN.store(current, Ordering::Relaxed);
        crate::dfx::reaper::zombie_scan(current);
    }

    // dfx=ledger: run a pending on-demand accounting census latched by the
    // "LEDGER!" UART magic. The RX IRQ itself only sets the flag (printk
    // from IRQ context is the classic self-wedge); this softirq already
    // hosts the periodic dumps, so printk discipline is the same.
    crate::dfx::ledger::run_pending();
}

/// Deliver collected expiries. Takes the `SoftirqCtx` capability —
/// only `timer_softirq_handler` can construct one, so this cannot be
/// called from task context (Rux timer actions are softirq-context
/// deliveries; must not sleep).
fn deliver_fired(expired: &[FiredToken], _ctx: SoftirqCtx) {
    for token in expired {
        let action = token.action;
        // Wakewatch: fire-lateness classification (one-shot hres fires
        // only; periodic entries carry 0). Healthy fire lateness is
        // bounded by one tick (10 ms at HZ=100); anything past 20 ms
        // under load is part of the outlier family.
        let late_ticks = token.late_ticks;
        // timer-gap lane (cyclictest tail attribution, cmdline cycdbg=1):
        // ns-precision FIRE lateness for one-shot hres wakes — a new
        // boot max over 100 us prints the interrupted context (the
        // firing cpu's current task + preempt count), decomposing the
        // cyclictest Max tail into "fire late" vs "picked late" (the
        // wake→pick side prints from wakewatch, same gate). Budget
        // 8/boot; IRQ-context direct console (putc_brief).
        if late_ticks > 0
            && matches!(action, Action::Wake { .. })
            && crate::cmdline::get_param("cycdbg").as_deref() == Some("1")
        {
            const TNS_PER_TICK: u64 = 1_000_000_000 / crate::config::TIMER_CLOCK_FREQ_HZ as u64;
            let late_us = late_ticks * TNS_PER_TICK / 1000;
            let token_deadline = token.deadline;
            static FL_MAX: AtomicU64 = AtomicU64::new(0);
            static FL_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let mut cur = FL_MAX.load(Ordering::Relaxed);
            while late_us > cur {
                match FL_MAX.compare_exchange_weak(cur, late_us, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        if late_us > 100
                            && FL_N.fetch_add(1, Ordering::Relaxed) < 12
                        {
                            use crate::dfx::taskdump::putc_brief as putchar;
                            fn ph(mut v: u64) {
                                for _ in 0..16 {
                                    let b = (v >> 60) as u8;
                                    putchar(if b < 10 { b'0' + b } else { b'a' + b - 10 });
                                    v <<= 4;
                                }
                            }
                            let (cpid, cpol, cpc) =
                                crate::sched::current().map(|t| unsafe {
                                    ((*t).pid() as u64, (*t).policy() as u64, (*t).preempt_count() as u64)
                                }).unwrap_or((u64::MAX, 9, 0));
                            for &b in b"\nCYC-FIRE-LATE us=" { putchar(b); }
                            ph(late_us);
                            // rt-r4 discriminator: this CPU's LAST LAPIC
                            // arm deadline minus the wheel deadline —
                            // armdl≈0 says the one-shot was armed AT the
                            // deadline and KVM delivered it late (host
                            // term); armdl≈late_us says it was armed AT
                            // the late instant (kernel coarse-arm term).
                            #[cfg(feature = "x86_64")]
                            let arm_dl = crate::drivers::intc::apic::last_arm_deadline(
                                crate::arch::cpu_id() as usize,
                            );
                            #[cfg(not(feature = "x86_64"))]
                            let arm_dl: u64 = 0;
                            for &b in b" armdl=" { putchar(b); }
                            if arm_dl >= token_deadline {
                                ph((arm_dl - token_deadline) * TNS_PER_TICK / 1000);
                            } else {
                                // armed EARLIER than the deadline (a prior
                                // event's arm): print the wrapped distance
                                // as a negative marker (huge hex).
                                ph(u64::MAX);
                            }
                            // rt-r4 rate-bias discriminator: the HPET is
                            // the truth source the TSC track was calibrated
                            // against. hlate≈0 with us>0 says the fire was
                            // PUNCTUAL in true time and the TSC-track
                            // mapping runs fast (track bias); hlate≈us says
                            // true lateness (LAPIC arm conversion or
                            // delivery). One MMIO read, tail-print path
                            // only (budget 12/boot).
                            #[cfg(feature = "x86_64")]
                            {
                                let hlate = if crate::drivers::timer::hpet::ready() {
                                    crate::drivers::timer::hpet::read_time_nominal()
                                        .saturating_sub(token_deadline)
                                        .saturating_mul(TNS_PER_TICK)
                                        / 1000
                                } else {
                                    u64::MAX
                                };
                                for &b in b" hlate=" { putchar(b); }
                                ph(hlate);
                            }
                            for &b in b" pid=" { putchar(b); }
                            ph(match action { Action::Wake { pid } => pid as u64, _ => 0 });
                            for &b in b" int_ctx_pid=0x" { putchar(b); }
                            ph(cpid);
                            for &b in b" pol=" { putchar(b); }
                            ph(cpol);
                            for &b in b" pc=" { putchar(b); }
                            ph(cpc);
                            putchar(b'\n');
                        }
                        break;
                    }
                    Err(actual) => cur = actual,
                }
            }
        }
        if late_ticks > 0 {
            WW_FIRE_N.fetch_add(1, Ordering::Relaxed);
            // rt-r4: sum + us-max for the AVG side (see WW_FIRE_SUM_TICKS).
            // Plain add: wrapping needs ~584 years of summed lateness —
            // saturation is unreachable; fetch_add is one RMW.
            WW_FIRE_SUM_TICKS.fetch_add(late_ticks, Ordering::Relaxed);
            // Divide-first tick→µs (overflow discipline; 10 ticks/µs).
            let late_us_all = late_ticks
                / (crate::config::TIMER_CLOCK_FREQ_HZ as u64 / 1_000_000).max(1);
            let mut cur = WW_FIRE_MAX_US.load(Ordering::Relaxed);
            while late_us_all > cur {
                match WW_FIRE_MAX_US.compare_exchange_weak(
                    cur,
                    late_us_all,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => cur = actual,
                }
            }
            let late_ms = late_ticks * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ;
            if late_ms >= 1 {
                WW_FIRE_LATE_N.fetch_add(1, Ordering::Relaxed);
            }
            let mut cur = WW_FIRE_LATE_MAX_MS.load(Ordering::Relaxed);
            while late_ms > cur {
                match WW_FIRE_LATE_MAX_MS.compare_exchange_weak(
                    cur,
                    late_ms,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => cur = actual,
                }
            }
            if late_ms > 20 && ww_latefire_budget_ok() {
                crate::pr_err!(
                    "WW: TIMER-LATE-FIRE pid={} late={}ms (timer fired late; softirq starvation?)",
                    match action {
                        Action::Wake { pid } => pid,
                        _ => 0,
                    },
                    late_ms
                );
            }
        }
        match action {
            Action::Wake { pid } => {
                // R13-3: pinned (softirq wake racing a concurrent reap) — the
                // last unpinned cross-CPU wake path.
                let task = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
                if !task.is_null() {
                    let ok = crate::sched::wake_up_process(task);
                    // nanosleep-loss hunt: the wake's outcome + the
                    // target's post-wake linkage state.
                    if !task.is_null() {
                        // SAFETY: pinned task, diagnostic reads.
                        let (onrq, key, stb) = unsafe {
                            (
                                (*task).sched_entity().on_rq.load(Ordering::Relaxed),
                                (*task).sched_entity().timeline_key().is_some(),
                                (*task).state().bits(),
                            )
                        };
                        fw_record(pid, ok, onrq, key, stb);
                    } else {
                        fw_record(pid, ok, false, false, 0xFFFF);
                    }
                    // r41v17 ticktrap: a delivered timer wake that did not
                    // stick — the fire happened, the entry left the wheel,
                    // and the target did not end up woken. If the target
                    // was RUNNING at delivery (the sleeper's own recheck
                    // window) this drop is recoverable by the waiter's
                    // protocol; if it was SLEEPING the wake was refused
                    // and the sleeper may be stranded — the line names
                    // which, live.
                    if !ok {
                        // SAFETY: diagnostic reads of the pinned task.
                        let (st, on_rq, pk) = unsafe {
                            (
                                (*task).state().bits(),
                                (*task).sched_entity().on_rq.load(Ordering::Relaxed),
                                (*task).pick_state(),
                            )
                        };
                        // A RUNNING target at delivery is the benign
                        // recheck-window class (r41v17 boots show one
                        // phase per busy poller, ~1/s each — not worth a
                        // line, and NOT worth the shared 1/s budget
                        // either: r41v18 proved a steady stream of these
                        // starved the ticktrap print). Anything else — a
                        // SLEEPING target whose enqueue was refused, a
                        // dead/poisoned shape — is the anomaly class:
                        // budget only for those.
                        if st != 0 && ww_latefire_budget_ok() {
                            crate::pr_err!(
                                "TIMER-WAKE-DROPPED pid={} state={:#x} on_rq={} pk={} (timer fired, wake did not stick)",
                                pid,
                                st,
                                on_rq,
                                pk
                            );
                        }
                    }
                    crate::process::task::Task::task_put(task);
                } else if ww_latefire_budget_ok() {
                    WW_PIDMISS_N.fetch_add(1, Ordering::Relaxed);
                    crate::pr_err!(
                        "WW: WAKE-PID-MISS pid={} (timer fired but target unresolved — wake dropped)",
                        pid
                    );
                }
            }
            Action::TimerFd { tfd } => {
                // R13-3 (H48 re-closed after R12-3): re-validate under TIMERS
                // that the id is still absent (not deleted-and-re-added) before
                // touching the fd's counter — a timerfd_close + free between
                // snapshot and delivery made this an add on freed memory.
                // R31-2 (third and correct form): ALWAYS deliver — both R25-1
                // and R28-1 accidentally SUPPRESSED periodic timerfd delivery
                // (rearmed ids were excluded, but rearmed ids are exactly the
                // periodic ones). The H48 close-race (increment on a freed
                // counter) is accepted per the timerfd refcount audit.
                // The address names the whole TimerFd so the notify hook can
                // also wake readers blocked in timerfd_read (review批次1).
                crate::syscall::misc::timerfd_expire_notify(tfd);
            }
            Action::Signal { pid, signo } => {
                let _ = crate::signal::send_signal(pid, signo);
            }
            Action::None => {}
        }
    }
}

/// Jiffies of the last periodic DFX snapshot (dfx=periodic).
static LAST_PERIODIC_DUMP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static LAST_PHANTOM_RELINK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Jiffies of the last reaper zombie-linger scan (dfx=schedtrace).
static LAST_REAPER_SCAN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Jiffies of the last wildwatch canary verify (dfx=wildwatch).
static LAST_WILDWATCH_VERIFY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ==================== Wakewatch forensics (vsync-starvation hunt) ====
//
// cc13 evidence: a 16 ms usleep in the loaded OH system occasionally
// takes 0.5–0.7 s in-process, plus one ~41 s dead stop per boot, while
// the isolated probe stays at 16.2/16.8 ms. These counters bracket the
// outlier from the kernel side:
//
//   - WW_HRES_LAST/N: per-CPU stamp of the last Hrtimer-softirq entry
//     (time ticks) + run count. A CPU whose stamp ages > 1 s while
//     others tick names timer-softirq starvation on THAT CPU.
//   - fire lateness: for every fired hres entry, now - expires_time at
//     collection. TIMER-LATE-FIRE prints outliers (>20 ms) after the
//     TIMERS/ACTIONS locks drop; the heartbeat carries the max.
//   - WAKE-PID-MISS: an expiry whose wake_pid no longer resolves —
//     a silently dropped wake token.
//   - heartbeat: one printk line per second with the cumulative view
//     (fires, late max, per-CPU softirq age, wake→run stats from
//     sched::wakewatch).
static WW_HRES_LAST: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
static WW_HRES_N: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
static WW_FIRE_N: AtomicU64 = AtomicU64::new(0);
static WW_FIRE_LATE_MAX_MS: AtomicU64 = AtomicU64::new(0);
/// rt-r4 (R2 residual attribution): fire-lateness SUM in time ticks over
/// the same population as WW_FIRE_N (every hres fire with late_ticks>0 —
/// effectively ALL hres wake fires: any fire past its deadline by one
/// tick counts). avg = SUM/N is the AVG side of the CYC-FIRE-LATE tail
/// bracket (46 us cyclictest avg decomposition).
static WW_FIRE_SUM_TICKS: AtomicU64 = AtomicU64::new(0);
/// rt-r4: cumulative max fire lateness in us (the existing MAX_MS is
/// ms-granular — useless at the 10-1000 us class the residual lives in).
static WW_FIRE_MAX_US: AtomicU64 = AtomicU64::new(0);
static WW_FIRE_LATE_N: AtomicU64 = AtomicU64::new(0);
static WW_PIDMISS_N: AtomicU64 = AtomicU64::new(0);
static WW_LAST_HEARTBEAT: AtomicU64 = AtomicU64::new(0);
/// Wakewatch: nanosleep belt-and-braces re-arms (one-shot token
/// consumed before deadline — the early-fire family).
static WW_REARM_N: AtomicU64 = AtomicU64::new(0);

// nanosleep-loss hunt: deliver_fired per-wake outcome ring (module
// statics so the heartbeat census can read them).
pub static FW_IX: AtomicUsize = AtomicUsize::new(0);
pub static FW_PID: [AtomicU32; 64] = [const { AtomicU32::new(0) }; 64];
pub static FW_OK: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
pub static FW_ONRQ: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
pub static FW_KEY: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
pub static FW_ST: [AtomicU32; 64] = [const { AtomicU32::new(0) }; 64];

// nanosleep-loss hunt: per-pid LAST-fire record (module level).
pub static FWT_PID: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];
pub static FWT_OK: [AtomicU8; 32] = [const { AtomicU8::new(0) }; 32];
pub static FWT_ONRQ: [AtomicU8; 32] = [const { AtomicU8::new(0) }; 32];
pub static FWT_KEY: [AtomicU8; 32] = [const { AtomicU8::new(0) }; 32];
pub static FWT_ST: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];
pub static FWT_JIF: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

#[inline]
fn fw_record(pid: u32, ok: bool, on_rq: bool, key: bool, st: u32) {
    let slot = FW_IX.fetch_add(1, Ordering::Relaxed) % 64;
    FW_PID[slot].store(pid, Ordering::Relaxed);
    FW_OK[slot].store(ok as u8, Ordering::Relaxed);
    FW_ONRQ[slot].store(on_rq as u8, Ordering::Relaxed);
    FW_KEY[slot].store(key as u8, Ordering::Relaxed);
    FW_ST[slot].store(st, Ordering::Relaxed);
    // per-pid LAST-fire side table (survives the ring's roll)
    let jif = crate::drivers::timer::get_jiffies();
    for k in 0..32 {
        if FWT_PID[k].load(Ordering::Relaxed) == pid
            && FWT_PID[k]
                .compare_exchange(pid, pid, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            FWT_OK[k].store(ok as u8, Ordering::Relaxed);
            FWT_ONRQ[k].store(on_rq as u8, Ordering::Relaxed);
            FWT_KEY[k].store(key as u8, Ordering::Relaxed);
            FWT_ST[k].store(st, Ordering::Relaxed);
            FWT_JIF[k].store(jif, Ordering::Relaxed);
            return;
        }
    }
    for k in 0..32 {
        if FWT_PID[k]
            .compare_exchange(0, pid, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            FWT_OK[k].store(ok as u8, Ordering::Relaxed);
            FWT_ONRQ[k].store(on_rq as u8, Ordering::Relaxed);
            FWT_KEY[k].store(key as u8, Ordering::Relaxed);
            FWT_ST[k].store(st, Ordering::Relaxed);
            FWT_JIF[k].store(jif, Ordering::Relaxed);
            return;
        }
    }
}

// nanosleep-loss conviction: timed-sleep loop-iteration counter (side
// table keyed by task pointer; no Task layout change). One relaxed add
// per nanosleep_impl loop pass; read by the heartbeat census.
pub static NSL_TASK: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
pub static NSL_ITER: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

#[inline]
pub fn nsleep_iter_bump() {
    let Some(t) = crate::sched::current() else { return };
    let key = t as *mut crate::process::Task as usize;
    for k in 0..16 {
        if NSL_TASK[k].load(Ordering::Relaxed) == key {
            NSL_ITER[k].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    for k in 0..16 {
        if NSL_TASK[k]
            .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            NSL_ITER[k].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}

/// Reader: the task's timed-sleep loop-iteration count (0 = unseen).
pub fn nsleep_iter_of(task: *mut crate::process::Task) -> u64 {
    let key = task as usize;
    for k in 0..16 {
        if NSL_TASK[k].load(Ordering::Relaxed) == key {
            return NSL_ITER[k].load(Ordering::Relaxed);
        }
    }
    0
}

/// Wakewatch: count a nanosleep early-token-consumption re-arm.
pub fn ww_note_rearm() {
    WW_REARM_N.fetch_add(1, Ordering::Relaxed);
}

/// Wakewatch: per-CPU timer-IRQ arrival counts (storm detector).
pub static WW_TICK_IRQ: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Wakewatch: per-CPU stamp of the last timer-IRQ arrival (time ticks;
/// 0 = never). Lets the TIMER-SOFTIRQ-STALL report discriminate "IRQs
/// stopped arriving" (IF=0 window) from "arrived but dispatch skipped".
static WW_TICK_LAST: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Wakewatch: record a timer-IRQ arrival on `cpu` (x86_64 tick path).
pub fn ww_note_tick_arrival(cpu: usize) {
    if cpu < crate::config::MAX_CPUS {
        WW_TICK_LAST[cpu].store(read_time_stamp(), Ordering::Release);
    }
}

#[inline]
fn read_time_stamp() -> u64 {
    crate::drivers::timer::read_time()
}

/// Wakewatch: age in ms of cpu's last timer-IRQ arrival (0 = never
/// arrived — pre-tick boot, or the stamp predates the LAPIC switch).
pub fn ww_tick_age_ms(cpu: usize) -> u64 {
    if cpu >= crate::config::MAX_CPUS {
        return 0;
    }
    let last = WW_TICK_LAST[cpu].load(Ordering::Acquire);
    if last == 0 {
        return 0;
    }
    let age = crate::drivers::timer::read_time().saturating_sub(last);
    age * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ
}

/// Per-CPU softirq-run age in ms (heartbeat helper); 0 = never ran.
pub fn ww_hres_age_ms(cpu: usize) -> u64 {
    if cpu >= crate::config::MAX_CPUS {
        return 0;
    }
    let last = WW_HRES_LAST[cpu].load(Ordering::Acquire);
    if last == 0 {
        return 0;
    }
    let age = crate::drivers::timer::read_time().saturating_sub(last);
    age * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ
}

/// Print one WW heartbeat line (called from the softirq, printk-safe
/// context — the claim-inject probe above prints from here too).
/// v2: adds the softirq-storm counters (per-vector dispatch deltas,
/// softirq busy ticks), timer-IRQ rate, scheduler slots (per-CPU current
/// pid/state/preempt_count, idle + need_resched masks, queue depths)
/// and the wheel census (len, overdue backlog) — everything the wt1/wt2
/// collapse discrimination needs on one line per second.
fn ww_heartbeat(now_time: u64) {
    use crate::interrupt::softirq::{WW_SOFTIRQ_RUNS, WW_SOFTIRQ_TICKS, WW_VEC_RUN};
    static LAST_VEC: [AtomicU64; crate::interrupt::softirq::NR_SOFTIRQS] =
        [const { AtomicU64::new(0) }; crate::interrupt::softirq::NR_SOFTIRQS];
    static LAST_RUNS: AtomicU64 = AtomicU64::new(0);
    static LAST_TICKS: AtomicU64 = AtomicU64::new(0);
    static LAST_IRQ: [AtomicU64; crate::config::MAX_CPUS] =
        [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

    let fire_n = WW_FIRE_N.load(Ordering::Relaxed);
    let late_max = WW_FIRE_LATE_MAX_MS.load(Ordering::Relaxed);
    let pidmiss = WW_PIDMISS_N.load(Ordering::Relaxed);

    let runs_d = WW_SOFTIRQ_RUNS.load(Ordering::Relaxed)
        .saturating_sub(LAST_RUNS.load(Ordering::Relaxed));
    // Divide first: raw tick deltas can approach u64::MAX (e.g. HRES_NEXT
    // sentinel) and a *1000 would overflow in the debug build.
    let ticks_per_ms = crate::config::TIMER_CLOCK_FREQ_HZ / 1000;
    let busy_ms = WW_SOFTIRQ_TICKS
        .load(Ordering::Relaxed)
        .saturating_sub(LAST_TICKS.load(Ordering::Relaxed))
        / ticks_per_ms;
    let mut vecs = [0u64; crate::interrupt::softirq::NR_SOFTIRQS];
    for v in 0..crate::interrupt::softirq::NR_SOFTIRQS {
        vecs[v] = WW_VEC_RUN[v].load(Ordering::Relaxed) as u64
            - LAST_VEC[v].load(Ordering::Relaxed);
        LAST_VEC[v].store(WW_VEC_RUN[v].load(Ordering::Relaxed) as u64, Ordering::Relaxed);
    }
    LAST_RUNS.store(WW_SOFTIRQ_RUNS.load(Ordering::Relaxed), Ordering::Relaxed);
    LAST_TICKS.store(WW_SOFTIRQ_TICKS.load(Ordering::Relaxed), Ordering::Relaxed);

    let mut irq_d = [0u64; crate::config::MAX_CPUS];
    for c in 0..crate::config::MAX_CPUS {
        let cur = WW_TICK_IRQ[c].load(Ordering::Relaxed);
        irq_d[c] = cur.saturating_sub(LAST_IRQ[c].load(Ordering::Relaxed));
        LAST_IRQ[c].store(cur, Ordering::Relaxed);
    }

    // s15 (A2 idle-mark/WFI hunt): idle-loop phase stamps + tp-vs-slot
    // mismatch deltas. phase codes: 0=not-idle-loop 1=entering-schedule
    // 2=marked+recheck 3=pre-WFI 4=inside-WFI 5=post-WFI-accounting;
    // slotmm = per-second delta of tick-entry samples where the hardware
    // tp register disagreed with cpu_state.current (lying slot).
    static LAST_MM: [AtomicU64; crate::config::MAX_CPUS] =
        [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
    let mm = crate::sched::sched::ww_slot_mismatch_counters();
    let mut mm_d = [0u64; 2];
    for c in 0..2 {
        mm_d[c] = mm[c].saturating_sub(LAST_MM[c].load(Ordering::Relaxed));
        LAST_MM[c].store(mm[c], Ordering::Relaxed);
    }
    let phase0 = crate::sched::sched::WW_IDLE_PHASE[0].load(Ordering::Acquire);
    let phase1 = crate::sched::sched::WW_IDLE_PHASE[1].load(Ordering::Acquire);

    // Wheel census (short lock; heartbeat is 1 Hz and holds no timer
    // locks at this point). Wheel-side: len + overdue hres backlog.
    let (wheel_len, overdue) = {
        let table = TIMER_TABLE.lock_irqsave();
        (table.len(), table.overdue_hres(Time::from_time(now_time)))
    };

    let sched = crate::sched::sched::ww_sched_snapshot();

    // nanosleep-loss hunt: arm/fire accounting — NEXT_TIMER_ID-1 is the
    // total arms ever; fires + live entries + cancels must cover it. A
    // gap = entries vanishing unfired.
    {
        use crate::dfx::taskdump::putc_brief as putchar;
        fn pdec9(mut v: u64) {
            use crate::dfx::taskdump::putc_brief as p;
            let mut buf = [0u8; 20];
            let mut n = 0;
            if v == 0 { p(b'0'); return; }
            while v > 0 && n < buf.len() { buf[n] = b'0' + (v % 10) as u8; v /= 10; n += 1; }
            for k in (0..n).rev() { p(buf[k]); }
        }
        for &b in b"NSLPARK rescued=" { putchar(b); }
        pdec9(crate::sched::sched::NSLPARK_RESCUED.load(Ordering::Relaxed));
        putchar(b'\n');
        for &b in b"ARMS id=" { putchar(b); }
        pdec9(NEXT_TIMER_ID.load(Ordering::Relaxed).saturating_sub(1));
        for &b in b" fires=" { putchar(b); }
        pdec9(WW_FIRE_N.load(Ordering::Relaxed));
        for &b in b" live=" { putchar(b); }
        {
            let table = TIMER_TABLE.lock_irqsave();
            pdec9(table.len() as u64);
        }
        putchar(b'\n');
    }
    // nanosleep-loss lane (hunt): per-task pick-phase census — every
    // heartbeat, one line per task: pid, last pick-phase (1 walk-unlink,
    // 2 mark_picked, 3 slot-published, 4 repick-return, 5 wrapper-enter,
    // 6 affirm-ok, 7 affirm-fail, 8 early-abort, 9 asm-abort), phase age
    // (jiffies), ownership qword, pk, linked, state. A frozen task names
    // its terminal phase directly.
    {
        use crate::dfx::taskdump::putc_brief as putchar;
        fn pdec(mut v: u64) {
            use crate::dfx::taskdump::putc_brief as p;
            let mut buf = [0u8; 20];
            let mut n = 0;
            if v == 0 { p(b'0'); return; }
            while v > 0 && n < buf.len() { buf[n] = b'0' + (v % 10) as u8; v /= 10; n += 1; }
            for k in (0..n).rev() { p(buf[k]); }
        }
        static PTBUDGET: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        // nanosleep-loss: CPU-0 only — the direct-console census writes
        // garble when both CPUs print concurrently.
        if crate::arch::cpu_id() as usize == 0
            && PTBUDGET.fetch_add(1, Ordering::Relaxed) < 60 {
            putchar(b'\n');
            crate::process::pid_hash::pid_hash_for_each_task_try(|task| unsafe {
                let pid = (*task).pid();
                if pid == 0 || pid == crate::sched::sched::TASK_POISON { return; }
                let q = core::ptr::read_volatile(
                    core::ptr::addr_of!((*task).ti_on_cpu) as *const u64);
                let st = (*task).state().bits();
                let linked = match (*task).policy() {
                    crate::process::task::SchedPolicy::Fifo
                    | crate::process::task::SchedPolicy::Rr => false,
                    _ => (*task).sched_entity().timeline_key().is_some(),
                };
                let (ptc, ptj) =
                    match crate::sched::sched::pick_trace_peek(task) {
                        Some(x) => x,
                        None => (0, 0),
                    };
                for &b in b"PTC pid=" { putchar(b); }
                pdec(pid as u64);
                for &b in b" ph=" { putchar(b); }
                pdec(ptc as u64);
                for &b in b" ago=" { putchar(b); }
                pdec(ptj);
                for &b in b" q=0x" { putchar(b); }
                pdec(q);
                for &b in b" pk=" { putchar(b); }
                pdec((*task).pick_state() as u64);
                for &b in b" lnk=" { putchar(b); }
                pdec(linked as u64);
                let tid = (*task).timed_sleep_timer_id.load(Ordering::Relaxed);
                let tdl = (*task).timed_sleep_deadline.load(Ordering::Relaxed);
                let tpend = crate::timer::timer_pending(tid);
                for &b in b" tid=" { putchar(b); }
                pdec(tid);
                for &b in b" tp=" { putchar(b); }
                pdec(tpend as u64);
                for &b in b" tdl-now=" { putchar(b); }
                let nowt = crate::drivers::timer::read_time();
                if tdl > nowt {
                    pdec((tdl - nowt) * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ as u64);
                } else {
                    putchar(b'-');
                    pdec((nowt - tdl) * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ as u64);
                }
                for &b in b" st=0x" { putchar(b); }
                pdec(st as u64);
                for &b in b" itr=" { putchar(b); }
                pdec(crate::timer::nsleep_iter_of(task));
                // last fire-record for this pid (per-pid table)
                for k in 0..32 {
                    if FWT_PID[k].load(Ordering::Relaxed) == pid {
                        for &b in b" fwj=" { putchar(b); }
                        pdec(crate::drivers::timer::get_jiffies().saturating_sub(
                            FWT_JIF[k].load(Ordering::Relaxed)));
                        for &b in b" fw=(ok:" { putchar(b); }
                        pdec(FWT_OK[k].load(Ordering::Relaxed) as u64);
                        for &b in b",onrq:" { putchar(b); }
                        pdec(FWT_ONRQ[k].load(Ordering::Relaxed) as u64);
                        for &b in b",key:" { putchar(b); }
                        pdec(FWT_KEY[k].load(Ordering::Relaxed) as u64);
                        for &b in b",st:0x" { putchar(b); }
                        pdec(FWT_ST[k].load(Ordering::Relaxed) as u64);
                        putchar(b')');
                        break;
                    }
                }
                putchar(b'\n');
            });
        }
    }
    // s16 (WW5, A2 census): pick-walk and heal deltas plus cumulative
    // sub-gate strand counts — the counter-based adjudication of the
    // msg-region strand family (see fair.rs WW_* counters). OWN pr_info:
    // the WW..WW4 block above is already ~250 bytes and printk truncates
    // at RECORD_TEXT_SIZE (256) — appending here silently cut the tail.
    static LAST_WALKS: AtomicU64 = AtomicU64::new(0);
    static LAST_OH: AtomicU64 = AtomicU64::new(0);
    static LAST_CH: AtomicU64 = AtomicU64::new(0);
    let walks_total = crate::sched::fair::ww_pick_walks();
    let walks_d = walks_total.saturating_sub(LAST_WALKS.load(Ordering::Relaxed));
    LAST_WALKS.store(walks_total, Ordering::Relaxed);
    let (oh, ch) = crate::sched::fair::ww_heals();
    let oh_d = oh.saturating_sub(LAST_OH.load(Ordering::Relaxed));
    let ch_d = ch.saturating_sub(LAST_CH.load(Ordering::Relaxed));
    LAST_OH.store(oh, Ordering::Relaxed);
    LAST_CH.store(ch, Ordering::Relaxed);
    let (s250, s500, s750) = crate::sched::wakewatch::ww_subgate_counts();
    crate::pr_info!(
        "WW5: walks={} oncpu_heal={} claim_heal={} s250={} s500={} s750={}",
        walks_d, oh_d, ch_d, s250, s500, s750
    );
    // s16: WW4 (phase/slotmm) in its own pr_info too — as the TAIL of
    // the big block it was the part most often garbled by the per-CPU
    // wakewatch heartbeat racing the UART, and slotmm is the family-B
    // standing watch (it must stay parseable).
    crate::pr_info!(
        "WW4: phase=[{},{}] slotmm=[{},{}]",
        phase0,
        phase1,
        mm_d[0],
        mm_d[1]
    );

    // rt-r4 (R2 residual attribution): the AVG side of the CYC brackets —
    // cumulative hres fire-lateness mean/max plus the RT-class wake→pick
    // mean/max. With a cyclictest FIFO run, cyclictest_avg ≈ fire_avg +
    // rt_pick_avg + switch/user overhead; the residual attribution names
    // the top term from these two lines. Divide-first tick→µs (the WW
    // overflow discipline: sums can grow unbounded across a long boot).
    {
        let ticks_per_us = (crate::config::TIMER_CLOCK_FREQ_HZ / 1_000_000).max(1);
        let f_n = WW_FIRE_N.load(Ordering::Relaxed);
        let f_sum_us = WW_FIRE_SUM_TICKS.load(Ordering::Relaxed) / ticks_per_us;
        let f_avg = if f_n > 0 { f_sum_us / f_n } else { 0 };
        let (rt_n, rt_avg, rt_max) = crate::sched::wakewatch::ww_rt_pick_stats();
        crate::pr_info!(
            "WW6: fire_n={} fire_avg={}us fire_max={}us rt_pick_n={} rt_pick_avg={}us rt_pick_max={}us",
            f_n,
            f_avg,
            WW_FIRE_MAX_US.load(Ordering::Relaxed),
            rt_n,
            rt_avg,
            rt_max
        );
    }

    crate::pr_info!(
        "WW: fires={} late_max={}ms pidmiss={} rearm={} wheel={}/{} hres_next={}ms\nWW2: si_runs={} si_busy={}ms ticks=[{},{}] vec=[{},{},{},{},{},{},{},{},{},{}]\nWW3: grq={} cfs={} idle={:#x} nr={} cpu0=(pid={} st={:#x} pc={}) cpu1=(pid={} st={:#x} pc={})",
        fire_n,
        late_max,
        pidmiss,
        WW_REARM_N.load(Ordering::Relaxed),
        overdue,
        wheel_len,
        HRES_NEXT.load(Ordering::Acquire).saturating_sub(now_time) / ticks_per_ms,
        runs_d,
        busy_ms,
        irq_d[0],
        irq_d[1],
        vecs[0], vecs[1], vecs[2], vecs[3], vecs[4],
        vecs[5], vecs[6], vecs[7], vecs[8], vecs[9],
        sched[0], sched[1], sched[2], sched[3],
        sched[4], sched[5], sched[6],
        sched[7], sched[8], sched[9]
    );
    crate::sched::wakewatch::heartbeat();
    // sc158 (Session R): one-shot park dump for wedged foundation/sa_main
    // registrar threads - see wakewatch::stallwatch_tick.
    crate::sched::wakewatch::stallwatch_tick();
}

/// Rate-gated direct-console print for fire-lateness outliers (no GRQ
/// held at the call site, but printk could interleave with GRQ-held
/// direct writes — keep the same putchar_no_lock discipline).
fn ww_latefire_budget_ok() -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(Ordering::Acquire);
    // sc167 storm gate: 10 jiffies (100 ms) let sc166 print 2503 STALL
    // lines, each a ~110-char direct-console write charged with IF=0 —
    // the report fed the storm it measured. 100 jiffies = one per second.
    if now.wrapping_sub(last) < 100 {
        return false;
    }
    LAST.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}


// Initialization: the old dead `pub fn init()` (never called anywhere — it
// only seeded LAST_TICK and printed a banner) was removed in the review
// batch-8 dead-code cleanup; the softirq scan self-seeds LAST_TICK on its
// first run (line ~197).

// ============================================================================
// SP-5 round 5: frozen-tick trap (timers/interrupts guarded domain, x86_64)
// ============================================================================
//
// Round-4 clock discriminator proved the SP-5 cascade runs with the timer
// IRQs dead on BOTH CPUs while the timebase advances (tb 6.17M -> 7.05M,
// WW_TICK_IRQ frozen 46/50). This trap fires from contexts that provably
// still execute during the cascade (the OOM-FORENSIC dump on the
// allocation-failure path) and prints the four-way discrimination:
//   (a) LAPIC timer programmed off / one-shot exhausted (who re-arms?)
//   (b) IF=0 stuck on a CPU (peer IPI probe times out)
//   (c) IRR bit stuck (APIC accepted, delivery blocked)
//   (d) halt wakeup lost (peer idle + TMCCT exhausted)
//
// Everything is printed on ONE line per CPU so the known doubled-console
// interleave cannot split a report across lines.

#[cfg(target_arch = "x86_64")]
pub fn tick_trap_report(site: &str) {
    use crate::drivers::intc::apic;

    let me = crate::arch::cpu_id() as usize;
    let tb_now = crate::drivers::timer::read_time();

    // --- SELF (the CPU executing the dump) ---
    let if_self = crate::arch::cpu::get_interrupts_state();
    let snap_self = apic::lapic_timer_snapshot();
    let (idle_phase_self, pid_self) = {
        let cur = crate::sched::current();
        let p = cur.map(|t| t.pid()).unwrap_or(0);
        (crate::sched::sched::WW_IDLE_PHASE[me].load(Ordering::Relaxed), p)
    };
    match &snap_self {
        Some(s) => crate::println!(
            "TICKTRAP[{}] self cpu={} lvt={:#x} tmict={} tmcct={} svr={:#x} tpr={:#x} irr7={:#x} isr7={:#x} IF={} pid={} idlephase={} schedphase={} verdict={} arms={} armop={} arm_tb_delta={} arm_deadline={} arm_counts={} tickirq={} tickage_ms={} freq={} cli_open={}",
            site, me,
            s.lvt, s.tmict, s.tmcct, s.svr, s.tpr, s.irr7, s.isr7,
            if_self, pid_self, idle_phase_self,
            crate::sched::sched::SCHED_CS_PHASE[me].load(Ordering::Relaxed),
            apic::tick_trap_adjudicate(s),
            apic::TIMER_ARM_SEQ[me].load(Ordering::Relaxed),
            apic::TIMER_ARM_OP[me].load(Ordering::Relaxed),
            tb_now.saturating_sub(apic::TIMER_ARM_TB[me].load(Ordering::Relaxed)),
            apic::TIMER_ARM_DEADLINE[me].load(Ordering::Relaxed),
            apic::TIMER_ARM_COUNTS[me].load(Ordering::Relaxed),
            WW_TICK_IRQ[me].load(Ordering::Relaxed),
            ww_tick_age_ms(me),
            apic::LAPIC_TIMER_FREQ.load(Ordering::Relaxed),
            match crate::dfx::syscost::open_cli_segment(me) {
                Some((file, line, age_raw)) => alloc::format!(
                    "{}:{} age_ms~{}",
                    file,
                    line,
                    // raw TSC -> ms (indicative; KVM TSCs are synced)
                    age_raw / (apic::TSC_FREQ.load(Ordering::Relaxed) / 1000).max(1)
                ),
                None => alloc::string::String::from("-"),
            },
        ),
        None => crate::println!(
            "TICKTRAP[{}] self cpu={} apic-not-ready IF={} pid={} tickirq={}",
            site, me, if_self, pid_self, WW_TICK_IRQ[me].load(Ordering::Relaxed)
        ),
    }

    // --- PEERS via IRQ_WORK snapshot probe ---
    for cpu in 0..crate::config::MAX_CPUS {
        if cpu == me || !crate::arch::smp::cpu_started(cpu) {
            continue;
        }
        let lapic_id = apic::lapic_id(cpu);
        if lapic_id == u32::MAX {
            continue;
        }
        let req = crate::arch::ipi::SNAP_REQ[cpu].fetch_add(1, Ordering::AcqRel) + 1;
        apic::send_ipi(lapic_id, crate::arch::ipi::VECTOR_IRQ_WORK as u8);
        // Timebase-bounded poll (50 ms nominal): jiffies are dead by
        // hypothesis, and this CPU must not hang on an unresponsive peer.
        let deadline = tb_now + crate::config::TIMER_CLOCK_FREQ_HZ / 20;
        let mut served = false;
        while crate::drivers::timer::read_time() < deadline {
            if crate::arch::ipi::SNAP_DONE[cpu].load(Ordering::Acquire) >= req {
                served = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !served {
            // The peer did not service the IPI within 50 ms: it is either
            // stuck with IF=0, halted with no interrupt source, or its
            // LAPIC is not accepting. Its frozen tickirq counter plus the
            // timeout is itself the (b)/(d) discrimination; the peer's
            // OPEN cli segment (syscost model, cross-CPU read) names the
            // stuck region's opener.
            crate::println!(
                "TICKTRAP[{}] peer cpu={} TIMEOUT no-ipi-service tickirq={} tickage_ms={} armseq={} armop={} schedphase={} cli_open={}",
                site, cpu,
                WW_TICK_IRQ[cpu].load(Ordering::Relaxed),
                ww_tick_age_ms(cpu),
                apic::TIMER_ARM_SEQ[cpu].load(Ordering::Relaxed),
                apic::TIMER_ARM_OP[cpu].load(Ordering::Relaxed),
                crate::sched::sched::SCHED_CS_PHASE[cpu].load(Ordering::Relaxed),
                match crate::dfx::syscost::open_cli_segment(cpu) {
                    Some((file, line, age_raw)) => alloc::format!(
                        "{}:{} age_ms~{}",
                        file,
                        line,
                        age_raw / (apic::TSC_FREQ.load(Ordering::Relaxed) / 1000).max(1)
                    ),
                    None => alloc::string::String::from("-"),
                },
            );
            continue;
        }
        let s = apic::LapicTimerSnapshot {
            lvt: crate::arch::ipi::SNAP_LVT[cpu].load(Ordering::Relaxed),
            tmict: crate::arch::ipi::SNAP_TMICT[cpu].load(Ordering::Relaxed),
            tmcct: crate::arch::ipi::SNAP_TMCCT[cpu].load(Ordering::Relaxed),
            tdcr: 0,
            svr: crate::arch::ipi::SNAP_SVR[cpu].load(Ordering::Relaxed),
            tpr: crate::arch::ipi::SNAP_TPR[cpu].load(Ordering::Relaxed),
            irr7: crate::arch::ipi::SNAP_IRR7[cpu].load(Ordering::Relaxed),
            isr7: crate::arch::ipi::SNAP_ISR7[cpu].load(Ordering::Relaxed),
        };
        let peer_tb = crate::arch::ipi::SNAP_TB[cpu].load(Ordering::Relaxed);
        crate::println!(
            "TICKTRAP[{}] peer cpu={} lvt={:#x} tmict={} tmcct={} svr={:#x} tpr={:#x} irr7={:#x} isr7={:#x} verdict={} armseq={} armop={} arm_tb_delta={} arm_deadline={} arm_counts={} tickirq={} tickage_ms={} snap_age_ms={}",
            site, cpu,
            s.lvt, s.tmict, s.tmcct, s.svr, s.tpr, s.irr7, s.isr7,
            apic::tick_trap_adjudicate(&s),
            crate::arch::ipi::SNAP_ARMSEQ[cpu].load(Ordering::Relaxed),
            apic::TIMER_ARM_OP[cpu].load(Ordering::Relaxed),
            crate::drivers::timer::read_time().saturating_sub(apic::TIMER_ARM_TB[cpu].load(Ordering::Relaxed)),
            apic::TIMER_ARM_DEADLINE[cpu].load(Ordering::Relaxed),
            apic::TIMER_ARM_COUNTS[cpu].load(Ordering::Relaxed),
            WW_TICK_IRQ[cpu].load(Ordering::Relaxed),
            ww_tick_age_ms(cpu),
            crate::drivers::timer::read_time().saturating_sub(peer_tb) * 1000
                / crate::config::TIMER_CLOCK_FREQ_HZ,
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn tick_trap_report(_site: &str) {}
