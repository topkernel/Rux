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

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::drivers::timer;

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
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WwCensus) {
                ww_heartbeat(now_time);
            }
            // leg-27: in-flight binder txn hang audit (dfx=txnhunt,
            // self-gated + rate-capped inside). Runs on the same ~1 Hz
            // heartbeat slot; zero cost when off.
            crate::ipc::binder::txn_hang_audit();
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
            // stress-port SP-5: direct-reclaim gate census — names the
            // holder's phase/wait-site when the gate has been held >=2 s
            // (self-gated: one atomic load when the gate is free; zero
            // serial on standard boots).
            crate::mm::page_alloc::reclaim_gate_census();
            // stress-port SP-7: per-page swap skip-reason census — one
            // line per second ONLY while anon reclaim is being exercised
            // (self-gated on census deltas; zero serial otherwise).
            crate::mm::vmscan::vscan_heartbeat();
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
            crate::pr_err!("CLAIM-INJECT poisoned={}", n);
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
            crate::pr_err!("ONCPU-INJECT poisoned={}", n);
        }
    }

    // Advance + collect under the lock; deliver AFTER releasing it
    // (R12-3 — see the delivery block below).
    // R34: the collection budget bounds the per-pass work; leftover
    // due entries stay in their wheel list and drain on the next pass
    // (same jiffy for hres re-runs — the precise compare re-runs —
    // next jiffy otherwise). WEDGE/perf fix: the expired list used to
    // be a heap `Vec::with_capacity(EXPIRY_BUDGET)` allocated on EVERY
    // tick — a 2560-byte alloc/dealloc pair per jiffy in IRQ/softirq
    // context. Under the LTP fs-storm wedges this churn dominated the
    // buddy allocator (photographed by gdb at exactly this line) and
    // was the sendmsg02 ALLOCTHROW (alloc-failure panic) site. The
    // per-CPU static backing storage below needs no lock: the
    // SOFTIRQ_IN_PROGRESS guard makes handler re-entry on the same CPU
    // impossible.
    static mut EXPIRED_BUF: [[FiredToken; wheel::EXPIRY_BUDGET];
        crate::config::MAX_CPUS] =
        [[FiredToken {
            id: 0,
            action: Action::None,
            late_ticks: 0,
        }; wheel::EXPIRY_BUDGET]; crate::config::MAX_CPUS];
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return; // unreachable on this platform; keep the handler total
    }
    // SAFETY: per-CPU rows indexed by this CPU's id; the SOFTIRQ_IN_PROGRESS
    // guard prevents re-entry, so this CPU's row is exclusively ours.
    let buf_ptr: *mut [FiredToken; wheel::EXPIRY_BUDGET] =
        unsafe { (core::ptr::addr_of_mut!(EXPIRED_BUF) as *mut [FiredToken; wheel::EXPIRY_BUDGET]).add(cpu) };
    let expired_len = {
        let now = Now::new(current, now_time);
        let mut table = TIMER_TABLE.lock_irqsave();
        table.advance(now);
        // SAFETY: per-CPU row, non-reentrant via SOFTIRQ guard; the
        // wheel writes at most EXPIRY_BUDGET tokens.
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
    // SAFETY: slice of this CPU's row, filled under the lock above.
    let expired: &[FiredToken] = &unsafe { &*buf_ptr }[..expired_len];
    deliver_fired(expired, ctx);

    // DFX dfx=periodic: a task snapshot every PERIODIC_DUMP_SECS. Silent
    // hangs (form-B) trip no watchdog and swallow the UART magic (a wedged
    // shell stops draining the RX path) — a timed snapshot from the timer
    // softirq always lands. SBI-direct, no locks held here.
    // sc115: the 500-jiffy (5 s) interval x ~500 tasks floods the
    // busy-poll UART (~87 us/byte charged to whoever is current); 3000
    // jiffies (30 s) still catches a hung launcher child's parked
    // syscall — the children live ~10 s between LIFECYCLE_TIMEOUT and
    // the AMS restart, so every other dump sees one — without
    // distorting the boot it is diagnosing.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::PeriodicDump)
        && current.saturating_sub(LAST_PERIODIC_DUMP.load(Ordering::Relaxed)) >= 3000
    {
        LAST_PERIODIC_DUMP.store(current, Ordering::Relaxed);
        crate::dfx::taskdump::dump_all_tasks("periodic");    }

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
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::MonoWatch) {
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
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::MetaWatch) {
        crate::dfx::metawatch::tick(current);
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
        if late_ticks > 0 {
            WW_FIRE_N.fetch_add(1, Ordering::Relaxed);
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
                    crate::sched::wake_up_process(task);
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
static WW_FIRE_LATE_N: AtomicU64 = AtomicU64::new(0);
static WW_PIDMISS_N: AtomicU64 = AtomicU64::new(0);
static WW_LAST_HEARTBEAT: AtomicU64 = AtomicU64::new(0);
/// Wakewatch: nanosleep belt-and-braces re-arms (one-shot token
/// consumed before deadline — the early-fire family).
static WW_REARM_N: AtomicU64 = AtomicU64::new(0);

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
