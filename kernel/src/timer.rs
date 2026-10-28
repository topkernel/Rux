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
//! R37 (landed leg 49; design note in
//! docs/development/r37-timer-slotted-table-draft.rs): the old pair of
//! `BTreeMap<u64, TimerEntry>` / `BTreeMap<u64, TimerAction>` tables
//! (TIMERS + ACTIONS locks) allocated BTreeMap NODES INSIDE the locks
//! on `insert()`. R34 had already made the softirq expiry path
//! zero-allocation (EXPIRY_BUDGET + in-place re-arm), but the add path
//! still allocated under TIMERS→ACTIONS. On allocation failure
//! `alloc_error_handler` panics (no unwinding, panic = abort), parking
//! the CPU in `wfi` with the lock held while the other CPUs spin on it
//! forever — the R36 run6 TIMERS deadlock family. The slot table makes
//! EVERY path under the lock allocation-free by construction: slots are
//! fixed .bss storage, occupancy is tracked in a fixed bitmap, and
//! entry+action are inlined in one slot (which also merges the two
//! tables into ONE lock, deleting the TIMERS→ACTIONS nesting edge).
//! Cost: 1024 × 56-byte slots + 128-byte bitmap ≈ 57 KB of .bss —
//! accepted.
//!
//! Callbacks run in softirq context — must not sleep.
//!
//! Timer actions (signal delivery, timerfd notification, re-arming)
//! are registered via `add_timer_with_action()` and looked up by
//! timer ID during expiry.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::drivers::timer;

/// Maximum number of concurrent timers.
const MAX_TIMERS: usize = 1024;

/// Global timer ID counter.
static NEXT_TIMER_ID: AtomicU64 = AtomicU64::new(1);

/// Earliest outstanding high-resolution deadline (time CSR ticks);
/// u64::MAX when none. Drives timer re-arming: the per-hart timer is
/// programmed to min(next grid tick, this value).

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
        || HRES_NEXT.load(Ordering::Acquire) <= timer::read_time()
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

/// Recompute HRES_NEXT from the active timer set (min expires_time > 0).
/// Caller holds no locks requirement — takes TIMER_TABLE internally.
fn recompute_hres_next() {
    let table = TIMER_TABLE.lock_irqsave();
    let mut next = u64::MAX;
    for w in 0..TIMER_WORDS {
        let mut word = table.bitmap[w];
        while word != 0 {
            let bit = word.trailing_zeros() as usize;
            word &= !(1u64 << bit);
            if let Some(slot) = &table.slots[w * 64 + bit] {
                if slot.expires_time != 0 && slot.expires_time < next {
                    next = slot.expires_time;
                }
            }
        }
    }
    drop(table);
    HRES_NEXT.store(next, Ordering::Release);
}

/// A timer action: what to do when a timer expires.
///
/// Detached snapshot type — copied out of the slot table under the lock
/// and delivered after the lock is released (R12-3). Only the
/// delivery-relevant fields are carried: `interval_jiffies` is consumed
/// during the scan itself (in-place re-arm reads it from the slot), so
/// the snapshot has no use for it.
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

/// One slot: timer entry (id + expiry pair) with its action INLINED.
/// R37 (landed leg 49, see docs/development/r37-timer-slotted-table-draft.rs):
/// merging the old TIMERS/ACTIONS BTreeMap pair into one slot removes
/// the TIMERS→ACTIONS lock nesting AND the transient state where an
/// entry was visible without its action.
#[derive(Clone, Copy)]
struct TimerSlot {
    /// Timer ID occupying this slot (monotonic, never reused — a freed
    /// slot's id can never falsely match a recycled slot).
    id: u64,
    /// Jiffies when this timer fires (jiffy-only timers; bookkeeping for
    /// high-resolution entries — see `expires_time`).
    expires: u64,
    /// Optional HIGH-RESOLUTION absolute expiry (time CSR ticks, 100ns
    /// each; 0 = jiffies-only timer). When set, the timer fires EXACTLY
    /// when read_time() reaches it and never otherwise (see the expiry
    /// decision in timer_softirq_handler for the early-fire history).
    expires_time: u64,
    // ---- action fields (old TimerAction, inlined) ----
    pid: u32,
    signo: i32,
    interval_jiffies: u64,
    tfd_addr: u64,
    wake_pid: u32,
}

impl TimerSlot {
    /// Copy the action half out (for the lock-external delivery
    /// snapshot).
    fn action(&self) -> TimerAction {
        TimerAction {
            pid: self.pid,
            signo: self.signo,
            interval_jiffies: self.interval_jiffies,
            tfd_addr: self.tfd_addr,
            wake_pid: self.wake_pid,
        }
    }
}

/// Occupancy bitmap words (1 bit per slot).
const TIMER_WORDS: usize = MAX_TIMERS / 64;

// Compile-time sanity: the bitmap must cover the slot array exactly.
const _: () = assert!(MAX_TIMERS % 64 == 0);

/// The fixed-slot timer table: slot array + occupancy bitmap, guarded by
/// ONE spinlock. R37: all mutations (occupy/free/re-arm) are plain
/// stores — no code path under TIMER_TABLE can touch the heap, so no
/// alloc_error_handler panic can ever fire with the lock held (the R36
/// OOM-deadlock family: BTreeMap node insert under TIMERS→ACTIONS, the
/// panicking CPU parked in wfi with the lock held, every other CPU
/// spinning on it forever).
struct TimerTable {
    /// Occupancy bitmap: bit i set ⇔ slots[i] is occupied. Lets the
    /// softirq scan and the id lookup skip whole runs of empty slots
    /// with one word compare (mostly-empty table ⇒ ~16 loads per pass).
    bitmap: [u64; TIMER_WORDS],
    /// Fixed slot array (None = free).
    slots: [Option<TimerSlot>; MAX_TIMERS],
}

impl TimerTable {
    /// Find the first free slot (lowest index). Scans ≤ TIMER_WORDS
    /// bitmap words — no allocation, no failure mode other than
    /// "pool full".
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

    /// Find the occupied slot holding `id`. Bitmap-accelerated walk:
    /// ids are unique and monotonic, so the first match is THE timer
    /// (a stale id matches nothing).
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

    /// Number of occupied slots (bitmap popcount — the old
    /// `timers.len()`).
    fn len(&self) -> usize {
        self.bitmap.iter().map(|w| w.count_ones() as usize).sum()
    }
}

/// The timer table: ONE lock replaces the old TIMERS + ACTIONS pair,
/// deleting the TIMERS→ACTIONS nesting edge from the lock graph.
/// ~57 KB of .bss (1024 slots × 56 B + 128 B bitmap) — accepted by the
/// R37 design note.
static TIMER_TABLE: Spinlock<TimerTable> = Spinlock::new(TimerTable {
    bitmap: [0; TIMER_WORDS],
    slots: [None; MAX_TIMERS],
});

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
    for w in 0..TIMER_WORDS {
        let mut word = table.bitmap[w];
        while word != 0 {
            let bit = word.trailing_zeros() as usize;
            word &= !(1u64 << bit);
            if let Some(slot) = &table.slots[w * 64 + bit] {
                out.push(TimerSnapshot {
                    id: slot.id,
                    expires_time: slot.expires_time,
                    expires: slot.expires,
                    pid: slot.pid,
                    signo: slot.signo,
                    interval_jiffies: slot.interval_jiffies,
                    wake_pid: slot.wake_pid,
                    tfd_addr: slot.tfd_addr,
                });
            }
        }
    }
    out
}

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
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    let slot = TimerSlot {
        id,
        expires,
        expires_time: 0,
        pid: 0,
        signo: 0,
        interval_jiffies: 0,
        tfd_addr: 0,
        wake_pid,
    };

    // R37: fixed-slot occupy — a plain store, replacing the old
    // `timers.insert()` / `actions.insert()` BTreeMap node allocations
    // under the TIMERS→ACTIONS locks (the R36 OOM-deadlock family).
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_free_slot() {
        Some(idx) => {
            table.occupy(idx, slot);
            id
        }
        None => 0, // pool full (the old `timers.len() >= MAX_TIMERS` check)
    }
}

/// Add a one-shot HIGH-RESOLUTION wake timer: fires when the time CSR
/// reaches `expires_time` (absolute ticks). `expires` is bookkeeping only
/// for hres entries — the softirq expiry decision uses `expires_time`
/// exclusively (see timer_softirq_handler: a jiffy fallback firing an
/// hres entry EARLY consumed the one-shot wake and hung sleepers, the
/// lost-timer-wakeup root cause). Used by nanosleep/clock_nanosleep.
pub fn add_timer_wakeup_hres(expires: u64, expires_time: u64, wake_pid: u32) -> u64 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    let slot = TimerSlot {
        id,
        expires,
        expires_time,
        pid: 0,
        signo: 0,
        interval_jiffies: 0,
        tfd_addr: 0,
        wake_pid,
    };

    let mut table = TIMER_TABLE.lock_irqsave();
    let idx = match table.find_free_slot() {
        Some(idx) => idx,
        None => return 0, // pool full
    };
    table.occupy(idx, slot);
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

    let slot = TimerSlot {
        id,
        expires,
        expires_time: 0,
        pid,
        signo,
        interval_jiffies,
        tfd_addr,
        wake_pid: 0,
    };

    // R37: zero allocation under the lock (see add_timer_wakeup).
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_free_slot() {
        Some(idx) => {
            table.occupy(idx, slot);
            id
        }
        None => 0, // pool full
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
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed);
    mark_timer_armed();
    if id == 0 {
        return 0;
    }

    let slot = TimerSlot {
        id,
        expires,
        expires_time,
        pid,
        signo,
        interval_jiffies,
        tfd_addr,
        wake_pid: 0,
    };

    let mut table = TIMER_TABLE.lock_irqsave();
    let idx = match table.find_free_slot() {
        Some(idx) => idx,
        None => return 0, // pool full
    };
    table.occupy(idx, slot);
    if expires_time != 0 && expires_time < HRES_NEXT.load(Ordering::Acquire) {
        HRES_NEXT.store(expires_time, Ordering::Release);
    }

    id
}

/// Delete a timer and its associated action.
///
/// # Returns
/// `true` if timer was found and removed.
pub fn del_timer(id: u64) -> bool {
    // R37: single lock — the old TIMERS→ACTIONS nesting (the lock-order
    // comment used to live here) is gone: entry and action are one slot.
    let mut table = TIMER_TABLE.lock_irqsave();
    match table.find_by_id(id) {
        Some(idx) => {
            let removed_hres =
                table.slots[idx].map(|s| s.expires_time != 0).unwrap_or(false);
            table.free(idx);
            if removed_hres {
                // The deleted timer may have been the global hres minimum;
                // the cheap next recompute in the softirq will fix the
                // value, but do it now so hart re-arms stop over-firing
                // sooner.
                let mut next = u64::MAX;
                for w in 0..TIMER_WORDS {
                    let mut word = table.bitmap[w];
                    while word != 0 {
                        let bit = word.trailing_zeros() as usize;
                        word &= !(1u64 << bit);
                        if let Some(slot) = &table.slots[w * 64 + bit] {
                            if slot.expires_time != 0 && slot.expires_time < next {
                                next = slot.expires_time;
                            }
                        }
                    }
                }
                HRES_NEXT.store(next, Ordering::Release);
            }
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
                slot.expires_time = 0;
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

/// Query a pending timer's absolute state.
///
/// Returns `Some((expires_jiffies, interval_jiffies))` while the timer is
/// pending; `interval_jiffies` is 0 for one-shot timers. Used by
/// getitimer/setitimer to report the remaining ITIMER_REAL time.
pub fn get_timer_state(id: u64) -> Option<(u64, u64)> {
    let table = TIMER_TABLE.lock_irqsave();
    let slot = table.slots[table.find_by_id(id)?].as_ref()?;
    Some((slot.expires, slot.interval_jiffies))
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
            // leg-33B: slow-kernel-entry audit (dfx=slowtask, self-gated
            // + rate-capped inside) — leaf attribution for nested-call
            // cascades (fs/futex stalls behind reader exhaustion).
            crate::dfx::slowtask::tick();
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
    /// Per-CPU expired-timer scratch (EXPIRY_BUDGET entries). Third
    /// element: fire lateness in time ticks (now_time - expires_time at
    /// collection; wakewatch forensics).
    static mut EXPIRED_BUF: [[(u64, TimerAction, u64); EXPIRY_BUDGET];
        crate::config::MAX_CPUS] =
        [[(0, TimerAction::EMPTY, 0); EXPIRY_BUDGET]; crate::config::MAX_CPUS];
    /// Per-CPU fill level of EXPIRED_BUF.
    static mut EXPIRED_LEN: [usize; crate::config::MAX_CPUS] = [0; crate::config::MAX_CPUS];
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return; // unreachable on this platform; keep the handler total
    }
    // SAFETY: per-CPU rows indexed by this CPU's id; the SOFTIRQ_IN_PROGRESS
    // guard prevents re-entry, so this CPU's row is exclusively ours.
    let buf_ptr: *mut [(u64, TimerAction, u64); EXPIRY_BUDGET] =
        unsafe { (core::ptr::addr_of_mut!(EXPIRED_BUF) as *mut [(u64, TimerAction, u64); EXPIRY_BUDGET]).add(cpu) };
    let len_ptr: *mut usize =
        unsafe { (core::ptr::addr_of_mut!(EXPIRED_LEN) as *mut usize).add(cpu) };
    unsafe { *len_ptr = 0 };
    {
        let mut table = TIMER_TABLE.lock_irqsave();
        let mut budget = EXPIRY_BUDGET;
        // R37: bitmap walk — empty words cost one compare each; bits
        // already visited are cleared from the local `word` copy, so
        // freeing a one-shot slot (which clears the bit in table.bitmap)
        // cannot cause a double visit.
        for w in 0..TIMER_WORDS {
            let mut word = table.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                let idx = w * 64 + bit;
                // Copy the slot out (TimerSlot is Copy) so no borrow is
                // held across the in-place re-arm / free below.
                let slot = match table.slots[idx] {
                    Some(s) => s,
                    None => continue, // impossible (occupy/free keep in sync); no-panic fallback
                };
                // Expiry. High-resolution entries (expires_time != 0) fire
                // ONLY on the precise time-CSR compare — the old `||
                // slot.expires <= current` jiffy fallback ALSO fired them,
                // and that is the lost-timer-wakeup root cause (timer-fix
                // hunt, unix_wedge TIMER-STALL / GNOME final blocker):
                //
                //   `expires` is computed as get_jiffies() + ceil(...) + 1
                //   at ARM time, but jiffies advance only inside
                //   increment_jiffies() on some hart's timer IRQ — under
                //   load (long SIE=0 windows) the counter LAGS the time
                //   grid by more than the +1 grid-tick margin, so `expires`
                //   lands BEFORE the real time that hres_deadline
                //   represents. When ticks resume, jiffies jump forward
                //   (max-with-nominal) and the scan fires the entry EARLY —
                //   observed 100us..2.4ms before expires_time (TIMER-EARLY-
                //   FIRE ... via=2 tripwire), always immediately followed
                //   by a permanently sleeping task.
                //
                // Why that is fatal: the one-shot wake is the ONLY waker
                // for nanosleep-class sleepers. An early fire consumes the
                // token; the sleeper's re-check still sees time <
                // hres_deadline, so it goes back to INTERRUPTIBLE sleep —
                // with no timer left. The task hangs in state=S forever
                // (timer_probe reproduces at ~60%/60s; unix_wedge
                // PROC-STALL epw/srv state=S).
                //
                // The jiffy fallback is also redundant for hres entries:
                // collection is guaranteed by ANY later scan, because
                // `expires_time <= now_time` eventually holds on the jiffy
                // grid alone (now_time only grows). Jiffy-only entries
                // (expires_time == 0: poll_sleep_slice, unix_wait_round,
                // itimers ...) keep firing on `expires <= current` — their
                // callers re-check against the same jiffies clock, so an
                // early-by-grid-jump fire is unobservable to them.
                let due = if slot.expires_time != 0 {
                    slot.expires_time <= now_time
                } else {
                    slot.expires <= current
                };
                if !due {
                    continue;
                }
                if budget == 0 {
                    // Budget exhausted: keep the entry — it is re-scanned
                    // on the next jiffy. Never let the critical section
                    // allocate.
                    continue;
                }
                budget -= 1;
                if slot.interval_jiffies > 0 {
                    // Periodic: re-arm IN PLACE (same id, same slot, only
                    // the expiry mutated) — semantically identical to the
                    // old remove+re-insert of the same key, but without
                    // the under-lock dealloc+alloc pair. Bonus: del_timer
                    // can no longer miss the briefly-removed id
                    // (orphan-timer race).
                    // R37: a PERIODIC timer must also DELIVER on every
                    // expiry — the old path only re-armed, so repeating
                    // ITIMER_REAL (setitimer with it_interval != 0) and
                    // periodic posix timers armed through the wheel never
                    // sent a single signal (LTP setitimer01/timer_settime
                    // hangs). Queue a COPY of the action for the
                    // outside-locks delivery pass below; the slot keeps
                    // its own entry for the next expiry.
                    if let Some(s) = table.slots[idx].as_mut() {
                        s.expires = current + slot.interval_jiffies;
                        // Periodic re-arm is jiffy-based — drop any stale
                        // high-resolution deadline so it cannot re-fire the
                        // very next scan.
                        s.expires_time = 0;
                    }
                    unsafe {
                        if *len_ptr < EXPIRY_BUDGET {
                            (*buf_ptr)[*len_ptr] = (slot.id, slot.action(), 0);
                            *len_ptr += 1;
                        }
                    }
                } else {
                    // One-shot: TAKE the action and free the slot — both
                    // plain stores, no allocation, no failure mode (the
                    // no-alloc-under-lock discipline is now structural).
                    // The old code left the action in ACTIONS forever, so
                    // every one-shot timer (nanosleep / glib timeout /
                    // timerfd tick in GNOME) leaked one TimerAction (the
                    // GNOME-oom leak; see the R34 history above).
                    table.free(idx);
                    unsafe {
                        // SAFETY: budget > 0 guarantees *len < EXPIRY_BUDGET;
                        // per-CPU buffer, non-reentrant via SOFTIRQ guard.
                        let n = *len_ptr;
                        // Wakewatch: capture the fire lateness of this
                        // one-shot. HRES entries only — a jiffy entry's
                        // expires_time is 0 and (now - 0) would report the
                        // whole uptime as "lateness" (probe bug found on
                        // wt5: late_max tracked boot age, not delay).
                        let late = if slot.expires_time != 0 {
                            now_time.saturating_sub(slot.expires_time)
                        } else {
                            0
                        };
                        (*buf_ptr)[n] = (slot.id, slot.action(), late);
                        *len_ptr = n + 1;
                    }
                }
            }
        }

        // R12-3: the wake/signal/slot-free work moved OUT of the
        // TIMER_TABLE critical section — holding the lock while calling
        // wake_up_process (timer-table -> GRQ nesting) was the observed
        // TIMERS wedge ingredient (3 CPUs spinning on the timer lock
        // after a pipeline). The `expired` list is a detached local
        // snapshot, so concurrency here is only against del_timer on the
        // same ids. The tfd increment is a plain atomic (H48's
        // close-race protection is the refcount on the fd side; the H48
        // comment applied to freeing, which does not happen in this
        // loop).
        // (Delivery happens after the lock drops — see the moved block.)
    } // TIMER_TABLE released here

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
    let expired: &[(u64, TimerAction, u64)] = &unsafe { &*buf_ptr }[..expired_len];
    for (id, action, late_ticks) in expired {
        // Wakewatch: fire-lateness classification (one-shot hres fires
        // only; periodic entries carry 0). Healthy fire lateness is
        // bounded by one tick (10 ms at HZ=100); anything past 20 ms
        // under load is part of the outlier family.
        let _ = id;
        if *late_ticks > 0 {
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
                    action.wake_pid,
                    late_ms
                );
            }
        }
        if action.wake_pid != 0 {
            // R13-3: pinned (softirq wake racing a concurrent reap) — the
            // last unpinned cross-CPU wake path.
            let task = crate::process::pid_hash::pid_hash_lookup_pinned(action.wake_pid);
            if !task.is_null() {
                crate::sched::wake_up_process(task);
                crate::process::task::Task::task_put(task);
            } else if ww_latefire_budget_ok() {
                WW_PIDMISS_N.fetch_add(1, Ordering::Relaxed);
                crate::pr_err!(
                    "WW: WAKE-PID-MISS pid={} (timer fired but target unresolved — wake dropped)",
                    action.wake_pid
                );
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
    // locks at this point). R37: bitmap walk over the slot table.
    let (wheel_len, overdue) = {
        let table = TIMER_TABLE.lock_irqsave();
        let len = table.len();
        let mut od = 0u64;
        for w in 0..TIMER_WORDS {
            let mut word = table.bitmap[w];
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= !(1u64 << bit);
                if let Some(slot) = &table.slots[w * 64 + bit] {
                    if slot.expires_time != 0 && slot.expires_time <= now_time {
                        od += 1;
                    }
                }
            }
        }
        (len, od)
    };

    let sched = crate::sched::sched::ww_sched_snapshot();

    crate::pr_info!(
        "WW: fires={} late_max={}ms pidmiss={} rearm={} wheel={}/{} hres_next={}ms\nWW2: si_runs={} si_busy={}ms ticks=[{},{}] vec=[{},{},{},{},{},{},{},{},{},{}]\nWW3: grq={} cfs={} idle={:#x} nr={} cpu0=(pid={} st={:#x} pc={}) cpu1=(pid={} st={:#x} pc={})\nWW4: phase=[{},{}] slotmm=[{},{}]",
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
        sched[7], sched[8], sched[9],
        phase0,
        phase1,
        mm_d[0],
        mm_d[1]
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
