//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Completely Fair Scheduler (CFS) Implementation
//!
//! Core concepts of CFS:
//! 1. Use virtual runtime (vruntime) to measure CPU time obtained by processes
//! 2. vruntime = actual runtime * (NICE_0_LOAD / process weight)
//! 3. Higher priority processes have larger weights, vruntime grows slower
//! 4. When scheduling, select the process with smallest vruntime to run
//!
//! Key data structures:
//! - SchedEntity: Scheduling entity, containing vruntime, weight, etc.
//! - CfsRunQueue: CFS run queue, using BTreeMap sorted by vruntime
//! - LoadWeight: Process weight, related to nice value
//!
//! This file also implements the FairSchedClass which wraps CFS:
//! - SCHED_NORMAL: Normal tasks with nice-based weights
//! - SCHED_BATCH: Batch tasks (similar to normal but with different wakeup behavior)
//! - SCHED_IDLE: Idle policy tasks with very low weight (WEIGHT_IDLEPRIO = 3)
//!
//! SCHED_IDLE policy is NOT the same as idle_sched_class:
//! - idle_sched_class is for per-CPU idle tasks (pid=0) only
//! - SCHED_IDLE policy tasks use this class with minimal weight

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// Use config values for scheduler timing
use crate::config::{KERNEL_HZ, CFS_MIN_GRANULARITY_NS, CFS_LATENCY_NS};
use crate::process::task::{Task, SchedPolicy};
use super::class::{SchedClass, RunQueueRef};

// ============================================================================
// s14 pick-skip history (dfx=schedtrace) — the strand-victim discriminator.
//
// PICK-SKIP-ALL reports only the total-failure shape (a pick that found
// NOTHING pickable). The msg-region WAKE-STRANDED family shows the other
// shape: a victim refused by every pick AMID pickable candidates —
// invisible to the report, yet it is the ~1s-grid strand producer
// (fleet ltp-b r2: 100/100 strands in the msg* region with zero
// skip-alls). This side table accumulates, per task, the refusal
// reasons the pick walk issued while it sat queued; wakewatch's strand
// report prints and clears the history at pick time, so every strand
// line names WHY the victim was skipped for the whole window.
// ============================================================================

struct SkipHistSlot {
    task: AtomicU64,
    /// Refusal counts by reason (1=ownership/on_cpu/claim-this-cpu,
    /// 2=foreign claim, 3=affinity, 4=throttled, 5=healed reset).
    why: [AtomicU64; 5],
    /// Total refusals recorded.
    n: AtomicU64,
    /// jiffies at the FIRST recorded refusal of this episode.
    first_jif: AtomicU64,
}

const SKIP_HIST_SLOTS: usize = 256;
static SKIP_HIST: [SkipHistSlot; SKIP_HIST_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: SkipHistSlot = SkipHistSlot {
        task: AtomicU64::new(0),
        why: [const { AtomicU64::new(0) }; 5],
        n: AtomicU64::new(0),
        first_jif: AtomicU64::new(0),
    };
    [ZERO; SKIP_HIST_SLOTS]
};

// ============================================================================
// s16 (A2 census, WW5) — counter-based adjudication of the msg-region
// strand family. The r6/r7 canary rounds printed ZERO STALE-ONCPU-RESET
// lines through 753 WAKE-STRANDED events while STALE-CLAIM prints got
// through the same 2-jiffy rate gate — strong evidence the R85 on_cpu
// heal never fires during the strands, but prints are budget-gated and
// a print-null is not a count-null. These counters are exact and
// unconditional (same class as wakewatch's WAKE_N/PICK_N: one relaxed
// inc on the pick-walk entry and two on cold heal paths); the WW5
// heartbeat line reports them under dfx=wwcensus.
// ============================================================================
static WW_PICK_WALKS: AtomicU64 = AtomicU64::new(0);
static WW_ONCPU_HEALS: AtomicU64 = AtomicU64::new(0);
static WW_CLAIM_HEALS: AtomicU64 = AtomicU64::new(0);

/// Total pick-walk entries since boot (WW5 heartbeat; wake-side
/// snapshot for the strand line's visits= field).
pub fn ww_pick_walks() -> u64 {
    WW_PICK_WALKS.load(Ordering::Relaxed)
}

/// (on_cpu-mark heals, stale-claim heals) since boot — the two R85
/// heal legs. Reported per-second by WW5; ~0 with hundreds of strands
/// convicts "the heal never fires" where the rate-gated prints could
/// only suggest it.
pub fn ww_heals() -> (u64, u64) {
    (
        WW_ONCPU_HEALS.load(Ordering::Relaxed),
        WW_CLAIM_HEALS.load(Ordering::Relaxed),
    )
}

#[inline]
fn skip_hist_of(task: *mut Task) -> &'static SkipHistSlot {
    let a = task as usize;
    &SKIP_HIST[(a >> 6 ^ a >> 12) & (SKIP_HIST_SLOTS - 1)]
}

/// Record one pick-walk refusal of `task` (called under the GRQ; only
/// when dfx=schedtrace is armed).
pub fn note_pick_skip(task: *mut Task, reason: u8) {
    let s = skip_hist_of(task);
    if s.task.load(Ordering::Relaxed) != task as usize as u64 {
        // New episode for this slot: reset the counters.
        s.task.store(task as usize as u64, Ordering::Release);
        for w in &s.why {
            w.store(0, Ordering::Relaxed);
        }
        s.n.store(0, Ordering::Relaxed);
        s.first_jif.store(crate::drivers::timer::get_jiffies(), Ordering::Release);
    }
    if let Some(w) = s.why.get(reason as usize) {
        w.fetch_add(1, Ordering::Relaxed);
    }
    s.n.fetch_add(1, Ordering::Relaxed);
}

/// Print (direct console, GRQ-held discipline) and clear the recorded
/// skip history of `task`. Called from wakewatch's strand report.
pub fn print_skip_history(task: *mut Task) {
    use crate::console::putchar_no_lock as putchar;
    let s = skip_hist_of(task);
    if s.task.load(Ordering::Acquire) != task as usize as u64 {
        // s16: the 256-slot table is keyed by Task address and msgstress
        // churn steals slots mid-strand, so "no SKIPHIST line" was
        // ambiguous between "never refused" and "history clobbered by a
        // colliding task". Name the collision explicitly — the census
        // needs the distinction (a stolen slot cannot exonerate the
        // walk).
        for &c in b"\nSKIPHIST coll=1" {
            putchar(c);
        }
        putchar(b'\n');
        return;
    }
    let n = s.n.swap(0, Ordering::AcqRel);
    // s16: print the line even when n == 0. "SKIPHIST stays empty" was
    // previously an inference from no-line-at-all (ambiguous against
    // probe-off and slot-reuse); an explicit skips=0 on every strand
    // line makes the never-refused verdict census-readable.
    let now = crate::drivers::timer::get_jiffies();
    let first = s.first_jif.swap(0, Ordering::AcqRel);
    let mut whys = [0u64; 5];
    for (i, w) in s.why.iter().enumerate() {
        whys[i] = w.swap(0, Ordering::AcqRel);
    }
    s.task.store(0, Ordering::Release);
    fn pu(mut v: u64) {
        use crate::console::putchar_no_lock as putchar;
        let mut buf = [0u8; 20];
        let mut k = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && k < buf.len() {
            buf[k] = b'0' + (v % 10) as u8;
            v /= 10;
            k += 1;
        }
        while k > 0 {
            k -= 1;
            putchar(buf[k]);
        }
    }
    for &c in b"\nSKIPHIST skips=" {
        putchar(c);
    }
    pu(n);
    for (i, cnt) in whys.iter().enumerate() {
        if *cnt == 0 {
            continue;
        }
        for &c in b" why" {
            putchar(c);
        }
        pu((i + 1) as u64);
        putchar(b'=');
        pu(*cnt);
    }
    // s16: span only meaningful with at least one recorded refusal
    // (first_jif is never set on a refusal-free episode).
    if n > 0 {
        for &c in b" span_ms=" {
            putchar(c);
        }
        pu(now.wrapping_sub(first) * 1000 / crate::config::KERNEL_HZ as u64);
    }
    putchar(b'\n');
}

/// Clock frequency (HZ) - from config
const HZ: u64 = KERNEL_HZ as u64;

/// Scheduling granularity (nanoseconds) - from config
pub const SCHED_MIN_GRANULARITY_NS: u64 = CFS_MIN_GRANULARITY_NS;

/// Scheduling latency (nanoseconds) - from config
pub const SCHED_LATENCY_NS: u64 = CFS_LATENCY_NS;

/// Weight when nice value is 0
pub const NICE_0_LOAD: u64 = 1024;

/// Weight for SCHED_IDLE tasks
/// This is a very low weight (3) to give idle tasks minimal CPU time
pub const WEIGHT_IDLEPRIO: u64 = 3;

/// Nice value to weight mapping table
///
/// Nice value range: -20 to +19, total 40 levels
/// Weights change by a factor of 1.25 (approximately 25% increase per 1024)
pub const PRIO_TO_WEIGHT: [u64; 40] = [
    /* -20 */ 88761, 71755, 56483, 46273, 36291,
    /* -15 */ 29154, 23254, 18705, 14949, 11916,
    /* -10 */ 9548,  7620,  6100,  4904,  3906,
    /*  -5 */ 3121,  2501,  1991,  1586,  1277,
    /*   0 */ 1024,   820,   655,   526,   423,
    /*   5 */ 335,    272,   215,   172,   137,
    /*  10 */ 110,     87,    70,    56,    45,
    /*  15 */ 36,     29,    23,    18,    15,
];

/// Nice value to weight multiplier mapping table (for fast calculation)
///
/// Used to calculate vruntime. The reference kernel's
/// `sched_prio_to_invweight[]` holds 2^32 / weight for each nice level
/// (same values, same order); the vruntime delta is then
/// `delta_exec * NICE_0_LOAD * inv_weight >> 32`, i.e.
/// `delta_exec * NICE_0_LOAD / weight`.
pub const PRIO_TO_WMULT: [u64; 40] = [
    /* -20 */ 48388, 59856, 76040, 92818, 118348,
    /* -15 */ 147320, 184698, 229616, 287308, 360437,
    /* -10 */ 449829, 563644, 704093, 875809, 1099582,
    /*  -5 */ 1376151, 1717300, 2157191, 2708050, 3363326,
    /*   0 */ 4194304, 5237765, 6557202, 8165337, 10153587,
    /*   5 */ 12820798, 15790321, 19976592, 24970740, 31350126,
    /*  10 */ 39045157, 49367440, 61356676, 76695844, 95443717,
    /*  15 */ 119304647, 148102320, 186737708, 238609294, 286331153,
];

/// Load weight
#[derive(Debug, Clone, Copy)]
pub struct LoadWeight {
    /// Weight value
    pub weight: u64,
    /// Weight multiplier (for fast division)
    pub inv_weight: u64,
}

impl LoadWeight {
    /// Create new load weight
    ///
    /// The inverse weight is computed here (2^32 / weight, the reference
    /// kernel's `__update_inv_weight`) so the vruntime update path never
    /// has to divide.
    pub fn new(weight: u64) -> Self {
        let mut lw = Self {
            weight,
            inv_weight: 0,
        };
        lw.update_inv_weight();
        lw
    }

    /// Create load weight from nice value
    ///
    /// Nice value range: -20 to +19
    /// Default nice value is 0, corresponding weight 1024
    pub fn from_nice(nice: i32) -> Self {
        // Convert nice value to array index (0-39)
        let idx = (nice + 20) as usize;
        let idx = idx.min(39).max(0);

        Self {
            weight: PRIO_TO_WEIGHT[idx],
            inv_weight: PRIO_TO_WMULT[idx],
        }
    }

    /// Update inv_weight (for fast division)
    pub fn update_inv_weight(&mut self) {
        if self.inv_weight == 0 {
            if self.weight >= (1u64 << 32) {
                self.inv_weight = 1;
            } else {
                self.inv_weight = (1u64 << 32) / self.weight;
            }
        }
    }
}

impl Default for LoadWeight {
    fn default() -> Self {
        Self::from_nice(0) // Default nice value is 0
    }
}

/// Scheduling entity
#[derive(Debug)]
pub struct SchedEntity {
    /// Load weight
    pub load: LoadWeight,

    /// Virtual runtime
    ///
    /// Smaller vruntime means the process has obtained less CPU time
    /// Scheduler prioritizes processes with smaller vruntime
    pub vruntime: AtomicU64,

    /// Cumulative execution time (nanoseconds)
    pub sum_exec_runtime: AtomicU64,

    /// Last execution start time (nanoseconds)
    pub exec_start: AtomicU64,

    /// Last cumulative execution time (for calculating delta)
    pub prev_sum_exec_runtime: AtomicU64,

    /// Whether in run queue
    pub on_rq: AtomicBool,

    /// Time slice (nanoseconds)
    pub slice: AtomicU64,
}

impl SchedEntity {
    /// Create new scheduling entity
    pub fn new() -> Self {
        Self {
            load: LoadWeight::default(),
            vruntime: AtomicU64::new(0),
            sum_exec_runtime: AtomicU64::new(0),
            exec_start: AtomicU64::new(0),
            prev_sum_exec_runtime: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(0),
        }
    }

    /// Create scheduling entity from nice value
    pub fn from_nice(nice: i32) -> Self {
        Self {
            load: LoadWeight::from_nice(nice),
            vruntime: AtomicU64::new(0),
            sum_exec_runtime: AtomicU64::new(0),
            exec_start: AtomicU64::new(0),
            prev_sum_exec_runtime: AtomicU64::new(0),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(0),
        }
    }

    /// Set nice value
    pub fn set_nice(&mut self, nice: i32) {
        self.load = LoadWeight::from_nice(nice);
    }

    /// Get virtual runtime
    #[inline]
    pub fn get_vruntime(&self) -> u64 {
        self.vruntime.load(Ordering::Acquire)
    }

    /// Set virtual runtime
    #[inline]
    pub fn set_vruntime(&self, vruntime: u64) {
        self.vruntime.store(vruntime, Ordering::Release);
    }

    /// Add virtual runtime
    #[inline]
    pub fn add_vruntime(&self, delta: u64) {
        self.vruntime.fetch_add(delta, Ordering::AcqRel);
    }

    /// Update execution time
    ///
    /// # Arguments
    /// - `now`: Current time (nanoseconds)
    ///
    /// # Returns
    /// Execution time delta for this run (nanoseconds)
    pub fn update_exec_runtime(&self, now: u64) -> u64 {
        let exec_start = self.exec_start.load(Ordering::Acquire);

        if exec_start == 0 {
            // First execution, record start time
            self.exec_start.store(now, Ordering::Release);
            return 0;
        }

        let delta = if now > exec_start {
            now - exec_start
        } else {
            0 // Prevent time wraparound
        };

        // Update cumulative execution time
        self.sum_exec_runtime.fetch_add(delta, Ordering::AcqRel);

        // Update start time
        self.exec_start.store(now, Ordering::Release);

        delta
    }

    /// Calculate virtual runtime delta
    ///
    /// vruntime += delta_exec * (NICE_0_LOAD / process weight)
    ///
    /// The reference kernel's form (kernel/sched/fair.c: __calc_delta) is
    /// `delta_exec * weight / lw.weight` computed as
    /// `(delta_exec * (weight * lw->inv_weight)) >> 32`, with
    /// weight = NICE_0_LOAD and lw->inv_weight = 2^32 / the entity's
    /// weight — i.e. the same `delta_exec * NICE_0_LOAD / weight`.
    ///
    /// # Arguments
    /// - `delta_exec`: Actual execution time (nanoseconds)
    ///
    /// # Returns
    /// Virtual runtime delta
    pub fn calc_delta_fair(&self, delta_exec: u64) -> u64 {
        // If weight equals NICE_0_LOAD, return directly
        if self.load.weight == NICE_0_LOAD {
            return delta_exec;
        }

        // Use multiplication instead of division:
        //   delta = delta_exec * NICE_0_LOAD / weight
        //         = (delta_exec * (NICE_0_LOAD * inv_weight)) >> 32
        //           [inv_weight = 2^32 / weight]
        //
        // Use u128 to prevent overflow when delta_exec is large.
        // Matches kernel/sched/fair.c __calc_delta() semantics.
        let mut load = self.load;
        load.update_inv_weight();

        let fact = (NICE_0_LOAD as u128)
            .wrapping_mul(load.inv_weight as u128);
        ((delta_exec as u128 * fact) >> 32) as u64
    }

    /// Update virtual runtime
    ///
    /// # Arguments
    /// - `delta_exec`: Actual execution time (nanoseconds)
    pub fn update_vruntime(&self, delta_exec: u64) -> u64 {
        let delta_vruntime = self.calc_delta_fair(delta_exec);
        self.add_vruntime(delta_vruntime);
        delta_vruntime
    }

    /// Check if in run queue
    #[inline]
    pub fn is_on_rq(&self) -> bool {
        self.on_rq.load(Ordering::Acquire)
    }

    /// Set run queue status
    #[inline]
    pub fn set_on_rq(&self, on_rq: bool) {
        self.on_rq.store(on_rq, Ordering::Release);
    }

    /// Get time slice
    #[inline]
    pub fn get_slice(&self) -> u64 {
        self.slice.load(Ordering::Acquire)
    }

    /// Set time slice
    #[inline]
    pub fn set_slice(&self, slice: u64) {
        self.slice.store(slice, Ordering::Release);
    }

    /// Get exec_start time
    #[inline]
    pub fn get_exec_start(&self) -> u64 {
        self.exec_start.load(Ordering::Acquire)
    }

    /// Set exec_start time
    #[inline]
    pub fn set_exec_start(&self, time: u64) {
        self.exec_start.store(time, Ordering::Release);
    }
}

impl Default for SchedEntity {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SchedEntity {
    fn clone(&self) -> Self {
        Self {
            load: self.load,
            vruntime: AtomicU64::new(self.vruntime.load(Ordering::Acquire)),
            sum_exec_runtime: AtomicU64::new(self.sum_exec_runtime.load(Ordering::Acquire)),
            exec_start: AtomicU64::new(0), // Reset execution start time
            prev_sum_exec_runtime: AtomicU64::new(self.prev_sum_exec_runtime.load(Ordering::Acquire)),
            on_rq: AtomicBool::new(false),
            slice: AtomicU64::new(self.slice.load(Ordering::Acquire)),
        }
    }
}

/// Key for BTreeMap
///
/// Since vruntime may be duplicate, we use (vruntime, task_ptr) as key
/// This ensures uniqueness
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VruntimeKey {
    vruntime: u64,
    task_id: u64, // Used to distinguish tasks with same vruntime
}

impl VruntimeKey {
    fn new(vruntime: u64, task_id: u64) -> Self {
        Self { vruntime, task_id }
    }
}

// R76 (fast-pace starvation hunt): per-pid pick-skip counters, direct-mapped
// by pid into 64 slots. Only read/written from inside pick_next_cpu's report
// block, which always runs under the GRQ lock — atomics are for the static's
// Sync bound only. When one pid's skip count crosses 1000 the report emits a
// SKIP-DEEP line with the task's state word, queue flag, cpu, and both stack
// pointers so a post-mortem can tell a linked-but-starved task from a
// phantom or a sleeper (the full3 wedge shape: PICK-SKIP-ALL naming pids
// 576-599 for minutes with no other explanation in the log).
pub(crate) struct SkipTrackEntry {
    pub(crate) pid: core::sync::atomic::AtomicU32,
    count: core::sync::atomic::AtomicU32,
    dumped_at: core::sync::atomic::AtomicU32,
}
static SKIP_TRACK: [SkipTrackEntry; 64] = [
    const {
        SkipTrackEntry {
            pid: core::sync::atomic::AtomicU32::new(0),
            count: core::sync::atomic::AtomicU32::new(0),
            dumped_at: core::sync::atomic::AtomicU32::new(0),
        }
    };
    64
];

/// CFS run queue
pub struct CfsRunQueue {
    /// Task queue sorted by vruntime
    ///
    /// Key: (vruntime, task_id)
    /// Value: Task pointer
    tasks_timeline: BTreeMap<VruntimeKey, *mut crate::process::Task>,

    /// Currently running scheduling entity
    pub curr: *mut crate::process::Task,

    /// Minimum vruntime in queue
    ///
    /// Used for new task vruntime initialization
    /// New task vruntime = min_vruntime, this prevents new tasks from getting too much CPU time
    pub min_vruntime: AtomicU64,

    /// Number of tasks in run queue
    nr_running: AtomicU64,

    /// Total weight
    pub(crate) load_weight: AtomicU64,

    /// Next task ID (for generating unique keys)
    next_task_id: AtomicU64,
}

impl CfsRunQueue {
    /// Create new CFS run queue
    pub fn new() -> Self {
        Self {
            tasks_timeline: BTreeMap::new(),
            curr: core::ptr::null_mut(),
            min_vruntime: AtomicU64::new(0),
            nr_running: AtomicU64::new(0),
            load_weight: AtomicU64::new(0),
            next_task_id: AtomicU64::new(0),
        }
    }

    /// Get number of tasks in run queue
    #[inline]
    pub fn nr_running(&self) -> u64 {
        self.nr_running.load(Ordering::Acquire)
    }

    /// Decrement the runnable count with a CHECKED subtract. A raw
    /// `fetch_sub` on an unbalanced decrement WRAPS to u64::MAX, and the
    /// next sched_slice() then panics on
    /// `SCHED_MIN_GRANULARITY_NS * nr_running` (multiply overflow — the
    /// x86 power-off teardown capture: both CPUs panicked in
    /// scheduler_tick -> sched_slice right after init's reboot()). The
    /// grq-level counter already uses this exact discipline
    /// (sched.rs); a refused decrement leaves the value at 0 and
    /// grq_diag_counts() names the drift.
    #[track_caller]
    fn nr_running_dec(&self) {
        if self.nr_running
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
            .is_err()
        {
            // REGRESSION GUARD: the counter was already 0 when a
            // removal path decremented again. The one historical producer
            // was the 2b092c49 discarded-guard GRQ unlock (every critical
            // section unexcluded — see the GlobalRunQueue doc note); with
            // mutual exclusion intact this must never fire. Rate-limited
            // direct console (may run under the GRQ); names the offending
            // caller site via #[track_caller].
            static LAST: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::drivers::timer::get_jiffies();
            let last = LAST.load(Ordering::Acquire);
            if now.wrapping_sub(last) >= 10
                && LAST.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire).is_ok()
            {
                let loc = core::panic::Location::caller();
                let pid = crate::sched::get_current_pid();
                use crate::console::putchar_no_lock as putchar;
                for &b in b"\nNRUN-DOUBLE-DEC pid=0x" { putchar(b); }
                let mut v = pid as u64;
                for _ in 0..8 {
                    putchar(b'0' + ((v >> 28) & 0xF) as u8);
                    v <<= 4;
                }
                for &b in b" at " { putchar(b); }
                let f = loc.file();
                let base = f.rfind('/').map(|i| i + 1).unwrap_or(0);
                for &b in f.as_bytes()[base..].iter() {
                    putchar(b);
                }
                putchar(b':');
                let mut ln = loc.line() as u64;
                let mut buf = [0u8; 10];
                let mut n = 0;
                if ln == 0 { putchar(b'0'); } else {
                    while ln > 0 && n < 10 { buf[n] = b'0' + (ln % 10) as u8; ln /= 10; n += 1; }
                    while n > 0 { n -= 1; putchar(buf[n]); }
                }
                putchar(b'\n');
            }
        }
    }

    /// Same no-wrap discipline for the aggregated weight: a wrapped
    /// load_weight reads as a huge total and collapses every task's
    /// slice to the minimum granularity. Regression-guard twin of
    /// nr_running_dec above (the 2b092c49 unexcluded sections were the
    /// one historical producer).
    #[track_caller]
    fn load_weight_sub(&self, weight: u64) {
        if self.load_weight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(weight))
            .is_err()
        {
            static LASTW: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::drivers::timer::get_jiffies();
            let last = LASTW.load(Ordering::Acquire);
            if now.wrapping_sub(last) >= 10
                && LASTW.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire).is_ok()
            {
                let loc = core::panic::Location::caller();
                use crate::console::putchar_no_lock as putchar;
                for &b in b"\nLWEIGHT-UNDERFLOW at " { putchar(b); }
                let f = loc.file();
                let base = f.rfind('/').map(|i| i + 1).unwrap_or(0);
                for &b in f.as_bytes()[base..].iter() {
                    putchar(b);
                }
                putchar(b'\n');
            }
        }
    }

    /// Check if run queue is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tasks_timeline.is_empty()
    }

    /// Get minimum vruntime
    #[inline]
    pub fn get_min_vruntime(&self) -> u64 {
        self.min_vruntime.load(Ordering::Acquire)
    }

    /// Wakewatch: the leftmost queued entity's vruntime (None when the
    /// timeline is empty). Diagnostic read; callers hold the GRQ lock.
    pub fn leftmost_vruntime(&self) -> Option<u64> {
        self.tasks_timeline.iter().next().map(|(&k, _)| k.vruntime)
    }

    /// s16 (A2 census): how many timeline entries sort at or before
    /// `vr` — the tie-mass standing ahead of a strand victim in
    /// service order. The r6/r7 strand lines all read vr=0ms
    /// lead-lag=0ms (victim at min_vruntime AND at the leftmost key),
    /// which is only possible as a TIE: a strictly-leftmost
    /// never-refused candidate is taken by the very next walk. tie=
    /// quantifies the mass the victim waits behind (the fair-queue
    /// hypothesis predicts tie ~ nr_running; a small tie with a big
    /// visits= would instead convict the walk itself). O(n) scan, but
    /// called only from the rate-gated strand print path (GRQ held).
    pub fn ww_tie_mass(&self, vr: u64) -> u64 {
        let mut n = 0u64;
        for (&k, _) in self.tasks_timeline.iter() {
            if k.vruntime <= vr {
                n += 1;
            } else {
                break;
            }
        }
        n
    }

    /// Update minimum vruntime
    ///
    /// Linux update_min_vruntime semantics (review batch 8): the value is the
    /// MINIMUM of the leftmost queued entity's vruntime and the running
    /// entity's vruntime (monotonically non-decreasing). Excluding `curr`
    /// let min_vruntime run arbitrarily far ahead of the running task — a
    /// sleeper woken later than one latency window was clamped up to
    /// min_vruntime, erasing its entire sleep compensation.
    fn update_min_vruntime(&mut self) {
        // SAFETY: self.curr is either null or set by set_curr() to a valid
        // Task pointer; only its sched_entity vruntime is read.
        let curr_v = if self.curr.is_null() {
            None
        } else {
            Some(unsafe { (*self.curr).sched_entity().get_vruntime() })
        };

        let leftmost = self.tasks_timeline.iter().next().map(|(&k, _)| k.vruntime);

        // vruntime = curr (if running) clamped by leftmost (if any queued).
        let vruntime = match (curr_v, leftmost) {
            (Some(c), Some(l)) => Some(c.min(l)),
            (only_one, _) => only_one.or(leftmost),
        };

        if let Some(v) = vruntime {
            let min_vruntime = self.min_vruntime.load(Ordering::Acquire);
            // min_vruntime only increases, ensure monotonic
            if v > min_vruntime {
                self.min_vruntime.store(v, Ordering::Release);
            }
        }
    }

    /// Enqueue task into run queue
    ///
    /// # Arguments
    /// - `task`: Task pointer to enqueue
    ///
    /// # Returns
    /// Returns true on success, false if task is already in queue
    pub fn enqueue(&mut self, task: *mut crate::process::Task) -> bool {
        self.enqueue_inner(task, false)
    }

    /// Enqueue a migrated task, preserving its vruntime.
    ///
    /// The task's vruntime is aligned to min_vruntime if it falls behind
    /// preserving vruntime, but is never reset forward.
    pub fn enqueue_migrate(&mut self, task: *mut crate::process::Task) -> bool {
        self.enqueue_inner(task, true)
    }

    fn enqueue_inner(&mut self, task: *mut crate::process::Task, migrate: bool) -> bool {
        if task.is_null() {
            return false;
        }

        // SAFETY: caller guarantees task is a valid pointer to a Task; null check above;
        // we only mutate the task's sched_entity fields which are owned by the scheduler.
        unsafe {
            let task_ref = &mut *task;

            // Get scheduling entity
            let se = task_ref.sched_entity();

            // If task is already in run queue, don't enqueue again
            if se.is_on_rq() {
                return false;
            }

            // Sleep/wake compensation with a bounded window (review batch 8,
            // Linux place_entity): a waking sleeper keeps its vruntime credit
            // so it preempts a CPU hog that ran while it slept, but the
            // credit is clamped to one scheduling-latency window behind
            // min_vruntime — an arbitrarily long sleeper must not monopolize
            // the CPU for an equivalently long burst after waking. The old
            // unconditional clamp-up to min_vruntime erased ALL compensation.
            let min_vruntime = self.get_min_vruntime();
            let floor = min_vruntime.saturating_sub(SCHED_LATENCY_NS);
            let vruntime = se.get_vruntime();
            if vruntime < floor {
                se.set_vruntime(floor);
            }

            // Generate unique key
            let task_id = self.next_task_id.fetch_add(1, Ordering::AcqRel);
            let key = VruntimeKey::new(se.get_vruntime(), task_id);

            // Add to BTreeMap
            self.tasks_timeline.insert(key, task);

            // Update status
            se.set_on_rq(true);

            // Update task count and total weight
            self.nr_running.fetch_add(1, Ordering::AcqRel);
            self.load_weight.fetch_add(se.load.weight, Ordering::AcqRel);

            // Update minimum vruntime
            self.update_min_vruntime();

            true
        }
    }

    /// Dequeue task from run queue
    ///
    /// # Arguments
    /// - `task`: Task pointer to dequeue
    ///
    /// # Returns
    /// Returns true on success
    pub fn dequeue(&mut self, task: *mut crate::process::Task) -> bool {
        if task.is_null() {
            return false;
        }

        // SAFETY: task is a valid pointer from the CFS queue (was enqueued earlier);
        // null check above; we only access sched_entity fields.
        unsafe {
            let task_ref = &mut *task;
            let se = task_ref.sched_entity();

            // Find and remove task by pointer only.
            // We do NOT match vruntime because update_curr() may have
            // changed the sched_entity's vruntime after the task was
            // enqueued, making the BTreeMap key's vruntime stale.
            let mut found_key = None;
            for (&key, &ptr) in self.tasks_timeline.iter() {
                if ptr == task {
                    found_key = Some(key);
                    break;
                }
            }

            if let Some(key) = found_key {
                self.tasks_timeline.remove(&key);

                // Update status
                se.set_on_rq(false);

                // Update task count and total weight
                self.nr_running_dec();
                self.load_weight_sub(se.load.weight);

                // Update minimum vruntime
                self.update_min_vruntime();

                return true;
            }

            false
        }
    }

    /// R41: whether `task` is ACTUALLY linked on this queue (scan by
    /// pointer, mirroring dequeue's lookup) — as opposed to what the
    /// entity's on_rq flag claims. Used by __schedule's prev requeue to
    /// distinguish a legitimately-linked prev (racing wake during a
    /// prepare-to-wait recheck window) from a stale on_rq=true flag.
    ///
    /// INVARIANT: the timeline BTreeMap (including this read-only scan)
    /// is only touched with the GRQ lock held. The DFX taskdump calls it
    /// via grq_diag_cfs_linked's try-lock (busy lock → linkage reported
    /// unknown); a lockless scan would race structural insert/remove on
    /// other CPUs and corrupt the stdlib iterator.
    pub fn is_linked(&self, task: *mut crate::process::Task) -> bool {
        if task.is_null() {
            return false;
        }
        self.tasks_timeline.iter().any(|(_, &ptr)| ptr == task)
    }

    /// Shared heal-report rate gate: at most one heal line per 2 jiffies
    /// system-wide (the picking CPUs can pass a healing candidate hot while
    /// a queue unwedges). R34 discipline: callers hold the GRQ lock.
    /// s16 (WW5): the counters below are exact and unconditional — only
    /// the PRINT rides this budget.
    fn heal_report_budget_ok() -> bool {
        static LAST_REPORT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let now = crate::drivers::timer::get_jiffies();
        let last = LAST_REPORT.load(core::sync::atomic::Ordering::Acquire);
        if now.wrapping_sub(last) < 2 {
            return false;
        }
        LAST_REPORT
            .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
            .is_ok()
    }

    /// One heal line prefix: TAG by_cpu=N pid=P from_claim=C[ pk=K] (no
    /// trailing newline — callers append their context tail first).
    fn stale_heal_line(tag: &[u8], cpu_id: usize, pid: u32, was: i32, pk: u32) {
        use crate::console::putchar_no_lock as putchar;
        for &b in tag {
            putchar(b);
        }
        const MB: &[u8] = b" by_cpu=";
        for &b in MB {
            putchar(b);
        }
        Self::put_dec(cpu_id as u64);
        const M2: &[u8] = b" pid=";
        for &b in M2 {
            putchar(b);
        }
        Self::put_dec(pid as u64);
        const M3: &[u8] = b" from_claim=";
        for &b in M3 {
            putchar(b);
        }
        if (was as i64) < 0 {
            putchar(b'-');
        }
        Self::put_dec((was as i64).unsigned_abs());
        if pk != u32::MAX {
            const M4: &[u8] = b" pk=";
            for &b in M4 {
                putchar(b);
            }
            Self::put_dec(pk as u64);
        }
    }

    /// GNOME-wedge report for the stale-claim heal inside pick_next_cpu
    /// (R34: GRQ-held callers, direct console writes only).
    fn stale_claim_healed(cpu_id: usize, pid: u32, was: i32) {
        WW_CLAIM_HEALS.fetch_add(1, Ordering::Relaxed);
        if !Self::heal_report_budget_ok() {
            return;
        }
        Self::stale_heal_line(b"STALE-CLAIM-RESET", cpu_id, pid, was, u32::MAX);
        crate::console::putchar_no_lock(b'\n');
    }

    /// R85 (tail-phase PICK-SKIP wedge): always-on, rate-limited report
    /// for the pick-side stale on_cpu heal — the released FULL qword is
    /// the store __switch_to would have made; the print names the victim
    /// (pid, state word, ti_cpu), the pre-heal claim, the advisory
    /// pick-state and the owner masks, so a capture can tell a lost
    /// release (slot=0 swout=0) from a lying-slot double-run fight
    /// (marks flipping between two cpus' claims). Same R34 discipline
    /// and 2s system-wide budget as the claim twin.
    fn stale_oncpu_healed(cpu_id: usize, task: *mut crate::process::Task, claim: i32, pk: u32) {
        WW_ONCPU_HEALS.fetch_add(1, Ordering::Relaxed);
        if !Self::heal_report_budget_ok() {
            return;
        }
        // SAFETY: diagnostic reads of stable Task fields; the task is
        // linked on this queue (pinned by the GRQ lock the caller holds).
        unsafe {
            let pid = (*task).pid();
            let st = (*task).state().bits();
            let tcpu = (*task).ti_cpu() as i64;
            let slot = crate::sched::sched::owner_slot_mask(task);
            let swout = crate::sched::sched::switching_out_mask(task);
            Self::stale_heal_line(b"STALE-ONCPU-RESET", cpu_id, pid, claim, pk);
            // Context tail: st= state word, tcpu= ti_cpu, slot=/swout=
            // owner masks at heal time (both must be 0 — that is the
            // adjudication's precondition; a nonzero mask here names a
            // racing owner).
            use crate::console::putchar_no_lock as putchar;
            const M: &[u8] = b" st=0x";
            for &b in M {
                putchar(b);
            }
            let mut v = st as u64;
            for sh in (0..8u32).rev() {
                let nb = ((v >> (sh * 4)) & 0xF) as u8;
                putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            }
            const M2: &[u8] = b" tcpu=";
            for &b in M2 {
                putchar(b);
            }
            if tcpu < 0 {
                putchar(b'-');
            }
            Self::put_dec(tcpu.unsigned_abs());
            const M3: &[u8] = b" slot=";
            for &b in M3 {
                putchar(b);
            }
            Self::put_dec(slot as u64);
            const M4: &[u8] = b" swout=";
            for &b in M4 {
                putchar(b);
            }
            Self::put_dec(swout as u64);
            putchar(b'\n');
        }
    }

    /// Decimal printer for the heal reports (R34: direct console writes).
    fn put_dec(v: u64) {
        use crate::console::putchar_no_lock as putchar;
        if v == 0 {
            putchar(b'0');
            return;
        }
        let mut buf = [0u8; 20];
        let mut n = 0;
        let mut v = v;
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
    }

    /// dfx=claiminject adjudication scan (GNOME-wedge hunt): poison every
    /// linked task that currently shows the fully-pickable ownership state
    /// {on_cpu=false, claim=-1} with a claim from CPU 7 — a CPU that does
    /// not exist on any board we boot, so no picker can ever own it. On an
    /// unpatched kernel every poisoned task strands exactly like the g2
    /// freeze (linked, counted, skipped forever); with the pick-side
    /// stale-claim heal the poison is swept at the next pick. Caller holds
    /// the GRQ lock.
    pub fn claim_inject_scan(&mut self) -> usize {
        let mut n = 0;
        for (_, &task) in self.tasks_timeline.iter() {
            // SAFETY: diagnostic write of one atomic field on a task pinned
            // in the timeline by the caller's GRQ lock.
            unsafe {
                if !(*task).on_cpu()
                    && (*task).running_on_cpu.load(Ordering::Acquire) == -1
                {
                    (*task).running_on_cpu.store(7, Ordering::Release);
                    n += 1;
                }
            }
        }
        n
    }

    /// dfx=oncpuinject adjudication scan (R85 tail-wedge hunt): stamp the
    /// FULL lost-release shape — {on_cpu=true, claim=7, pk=LIVE} via
    /// publish_pick_ownership — onto every linked task currently showing
    /// the pickable {on_cpu=false, claim=-1}. CPU 7 exists on no board we
    /// boot, so no slot can ever back the claim: the state is exactly the
    /// terminal microstate of the pid-1 tail wedge (linked, marked, owned
    /// by neither a slot nor an in-flight switch). An unpatched kernel
    /// strands every poisoned task forever (picks skip it why=1); with the
    /// R85 pick-side ownership adjudication the poison is swept within
    /// ~50 jiffies (STALE-ONCPU-RESET). Caller holds the GRQ lock.
    pub fn oncpu_inject_scan(&mut self) -> usize {
        let mut n = 0;
        for (_, &task) in self.tasks_timeline.iter() {
            // SAFETY: diagnostic publish on a task pinned in the timeline
            // by the caller's GRQ lock. Only tasks with a fully released
            // qword are stamped, so no legitimate in-flight ownership is
            // overwritten (running/picked tasks are not linked).
            unsafe {
                if !(*task).on_cpu()
                    && (*task).running_on_cpu.load(Ordering::Acquire) == -1
                {
                    (*task).publish_pick_ownership(7, true);
                    n += 1;
                }
            }
        }
        n
    }

    /// Pick next task to run
    ///
    /// Select task with smallest vruntime
    ///
    /// # Returns
    /// Next task pointer to run, or None if queue is empty
    pub fn pick_next(&mut self) -> Option<*mut crate::process::Task> {
        // Get task with smallest vruntime
        if let Some((&key, &task)) = self.tasks_timeline.iter().next() {
            // b50 #GP guard — see pick_next_cpu. A non-canonical slot
            // pointer must never reach the deref below.
            if !crate::sched::sched::kernel_task_ptr_range_ok(task as *const u8) {
                self.nr_running_dec();
                crate::sched::sched::report_poisoned_rq_entry(usize::MAX, task as usize);
                return None;
            }
            // Remove from queue using key (don't call dequeue, as vruntime may have changed)
            self.tasks_timeline.remove(&key);

            // SAFETY: task was stored in tasks_timeline from a valid &mut Task pointer;
            // it was just removed from the queue so no aliasing references exist.
            unsafe {
                // Update status
                let task_ref = &mut *task;
                let se = task_ref.sched_entity();
                se.set_on_rq(false);

                // Update task count and total weight
                self.nr_running_dec();
                self.load_weight_sub(se.load.weight);

                // Update minimum vruntime
                self.update_min_vruntime();
            }

            return Some(task);
        }

        None
    }

    /// Peek next task to run (without removing)
    ///
    /// Used to check next task without changing queue state
    pub fn peek_next(&self) -> Option<*mut crate::process::Task> {
        if let Some((&_key, &task)) = self.tasks_timeline.iter().next() {
            return Some(task);
        }
        None
    }

    /// Pick the leftmost (min-vruntime) task that is allowed to run on `cpu_id`.
    /// Scans from left to right, skipping tasks that cannot run on this CPU.
    ///
    /// Temporarily removes non-matching entries to inspect subsequent ones,
    /// then re-inserts them. CFS task counts are typically reasonable so this
    /// works fine without allocating.
    pub fn pick_next_cpu(&mut self, cpu_id: usize, prev: *mut crate::process::Task) -> Option<*mut crate::process::Task> {
        // s16 (WW5): one exact count per walk entry — the visits= field
        // on the strand line divides this by the strand window.
        WW_PICK_WALKS.fetch_add(1, Ordering::Relaxed);
        // R7-G1 (GDB capture): this was Vec::with_capacity(256) — a 6KB
        // HEAP allocation on every pick, made while the caller holds the
        // GRQ irqsave lock (alloc failure there panics the scheduler;
        // contending the global allocator with IRQs off wedged all CPUs).
        // A fixed stack array of 32 entries needs no allocator at all;
        // overflow falls back to re-insert-and-give-up exactly like the
        // old capacity-overflow path. 32 stack slots of 24 bytes = 768B.
        // R76 (fast-pace starvation hunt): the third slot records the
        // DECISION-TIME skip reason — the old report re-read on_cpu/claim
        // at print time, microseconds AFTER the decision, and the
        // switch-out release store (__switch_to, no GRQ) legally rewrites
        // the qword in that window, so a genuine mid-switch-out skip
        // printed as {oncpu=0, claim=-1} and was indistinguishable from
        // "pickable but skipped". Reason codes:
        // 1 = on_cpu set (mid switch-out / fast-path repick window),
        // 2 = foreign live claim (claim=X, picker sees X as alive),
        // 3 = affinity mask, 4 = cgroup throttled, 5 = stale claim healed.
        let mut skipped: [(VruntimeKey, *mut crate::process::Task, u32); 32] =
            [(VruntimeKey::new(0, 0), core::ptr::null_mut(), 0); 32];
        let mut skip_count = 0usize;
        let mut result = None;

        loop {
            let key = match self.tasks_timeline.iter().next() {
                Some((&k, _)) => k,
                None => break,
            };
            let task = match self.tasks_timeline.get(&key) {
                Some(&t) => t,
                None => break,
            };

            // b50 #GP guard: a scribbled timeline slot yields a
            // non-canonical *mut Task; the ownership probe below would
            // #GP the kernel on its first qword load (boot b50, ~20 min
            // in). Drop the poisoned entry — the task it named is
            // unrecoverable either way; keeping the kernel (and this
            // CPU's pick) alive matters more. load_weight is left
            // stale: reading it would deref the same garbage.
            if !crate::sched::sched::kernel_task_ptr_range_ok(task as *const u8) {
                self.nr_running_dec();
                crate::sched::sched::report_poisoned_rq_entry(cpu_id, task as usize);
                continue;
            }

            // SAFETY: task is from the BTreeMap, a valid pointer stored during enqueue.
            // R8-1b: skip tasks still marked on-CPU (context not saved —
            // picking one would resume a STALE thread.sp) UNLESS it is the
            // switching CPU's own prev, which re-picks itself only via the
            // next == prev early return (no context restore happens).
            // R65 (dual-bit ownership): ti_on_cpu and running_on_cpu are
            // set/cleared together by the switch protocol (mark at pick,
            // publish at switch-out), so a pickable task must show
            // on_cpu == false AND claim == -1 (or our own claim). Testing
            // BOTH closes the single-word scribble: a stray 8-byte store
            // of a kernel-static pointer over the ownership word resets
            // it to {false, -1} (x86-scribble3: the word held
            // 0xffffffff_80661100, a .rodata ops pointer), which let a
            // second CPU resume a task that was live on the first CPU's
            // stack — the shared-stack scribble engine. With the claim in
            // the predicate the corrupted task simply stays unpickable
            // (the detector names it) instead of double-running.
            //
            // GNOME-wedge (stale-claim heal): a LINKED task showing
            // {on_cpu == false, claim == X != -1} is unreachable by the
            // switch protocol itself. The only legitimate linked states
            // are {false, -1} (sleeping/woken, preempted-and-requeued
            // after the switch-out release) and {true, X} (mid
            // switch-out / fast-path re-pick, skipped on the on_cpu leg).
            // The claim is published together with on_cpu at pick time
            // under the GRQ (publish_pick_ownership), and released only
            // by __switch_to's single full-qword {false, -1} store — no
            // code path clears one bit without the other on a linked
            // task. A linked task also cannot be
            // executing anywhere (running tasks are dequeued by their
            // pick), so resetting the claim cannot re-open the R65
            // double-run window: that guard protects RUNNING tasks, which
            // are unlinked and never reach this scan. Every producer of
            // the impossible state (a heal path that cleared on_cpu
            // alone, a qword-zeroing scribble, a wedged picker that never
            // reached its switch-out release) left the task stranded
            // permanently: linked, counted in nr_running, skipped by
            // every CPU forever — the g2 GNOME freeze (10 tasks stuck,
            // all CPUs idle-looping on the R20-2 recheck for 35 min,
            // screen dead, khungtaskd itself stranded). Reset the stale
            // claim, pick the task, and report loudly (rate-limited).
            // cgroup v2 (U1b): skip tasks whose cgroup chain is
            // cpu-throttled — they stay on the timeline and become
            // pickable again when the period rolls over (the same
            // affinity-style skip machinery above).
            // R76: skip_reason is written at DECISION time (see the array
            // comment above) — 0 = none, 1 = on_cpu, 2 = foreign live
            // claim, 3 = affinity, 4 = cgroup throttled, 5 = stale claim
            // healed (pickable on the NEXT pass).
            let mut skip_reason = 0u32;
            let allowed = unsafe {
                // R65-pick (probe-slow): the dual-bit ownership test reads
                // the whole qword in one load (Task::ownership_pickable) —
                // same e87a8b40 semantics, half the loads per candidate.
                // The GNOME-wedge stale-claim heal rides the REFUSAL path
                // only: ownership_pickable is the fast path, and a refusal
                // is split by re-reading the two bits — on_cpu=true is the
                // legitimate NEW2 skip, while {on_cpu=false, foreign claim}
                // on a LINKED candidate is the protocol-impossible state
                // the heal resets (see the comment above).
                let ownership_ok = if task == prev {
                    true
                } else if (*task).ownership_pickable(cpu_id) {
                    true
                } else {
                    let claim = (*task).running_on_cpu.load(Ordering::Acquire);
                    // R85 (tail-phase PICK-SKIP wedge): adjudicate every
                    // refusal against SCHEDULER TRUTH instead of the
                    // advisory bits. pick_owner_live (sched.rs) scans the
                    // per-CPU current slots and the SWITCHING_OUT marks
                    // under this same GRQ hold: a genuinely owned task is
                    // named by its CPU's slot (running / picked — the mark
                    // and slot publishes share one GRQ hold) or by a
                    // SWITCHING_OUT entry (mid switch-out, release store
                    // still pending). A marked task named by NEITHER
                    // executes nowhere and no path will ever release its
                    // marks: every pick would skip it with why=1/why=2
                    // forever (the pid-1 tail wedge: linked,
                    // {oncpu=1, claim=1, pk=1}, box idle).
                    //
                    // Organic captures (fork-storm soak, smp2, no
                    // injectors): pid 1 re-entered these shapes every few
                    // seconds for minutes — {oncpu=1, claim=-1, pk=1,
                    // st=RUNNING, slot=0, swout=0, tcpu=crossed} (a
                    // byte-granular on_cpu flip or a lost release; ti_cpu
                    // naming the OTHER cpu) and {oncpu=0, claim=foreign}
                    // (why=2). On dd5537f1 each occurrence was a
                    // potential permanent strand; with this adjudication
                    // each is swept in <=50 jiffies and the soak completes
                    // all phases.
                    let owner_live =
                        crate::sched::sched::pick_owner_live(task);
                    if !(*task).on_cpu()
                        && claim != -1
                        && claim as usize != cpu_id
                    {
                        // R74 (double-run gate, superseded): the old gate
                        // trusted pick_state==LIVE plus hart liveness
                        // (irq-frozen < 5s) — but LIVE legitimately
                        // persists after a switch-out and says nothing
                        // about whether the CLAIMER still owns the task.
                        // The r74a capture (reset while CPU0 was inside
                        // its pick → double-run) is now excluded
                        // STRUCTURALLY: a pick's mark→slot publish window
                        // is one continuous GRQ hold, so this scan can
                        // never interleave it; the r74a shape reads
                        // slot(claimer)==task → owner_live → refused. A
                        // {on_cpu=false, foreign claim} candidate owned by
                        // no slot and no in-flight switch is a byte-clear
                        // residue or scribble garbage — reset it.
                        if !owner_live {
                            (*task).running_on_cpu.store(-1, Ordering::Release);
                            Self::stale_claim_healed(cpu_id, (*task).pid(), claim);
                            skip_reason = 5;
                        } else {
                            skip_reason = 2;
                        }
                        !owner_live
                    } else {
                        // why=1 leg: on_cpu set (or the claim names THIS
                        // picker's cpu). This is the legitimate mid
                        // switch-out / fast-repick transient — UNLESS no
                        // owner backs it (slotless, no SWITCHING_OUT): the
                        // release store was lost and the marks are stale
                        // forever. Time-confirm through the orphan watch
                        // (50 jiffies) so a picker that briefly appears
                        // slotless under an out-of-tree divergence window
                        // is re-recorded, then release the FULL qword —
                        // the exact store __switch_to would have made.
                        if !owner_live
                            && crate::sched::sched::stale_mark_confirm(
                                task,
                                (*task).pid(),
                                50,
                            )
                        {
                            let was_pk = (*task).pick_state();
                            (*task).release_pick_ownership();
                            Self::stale_oncpu_healed(cpu_id, task, claim, was_pk);
                            skip_reason = 5;
                        } else {
                            skip_reason = 1;
                        }
                        false
                    }
                };
                let affinity_ok = (*task).cpu_allowed(cpu_id);
                if !affinity_ok {
                    skip_reason = 3;
                }
                let throttled = crate::sched::cgroup::task_cgroup_throttled(task);
                if throttled {
                    skip_reason = 4;
                }
                affinity_ok && ownership_ok && !throttled
            };
            if allowed {
                // Found a match — remove and return it
                self.tasks_timeline.remove(&key);

                // SAFETY: task was just removed from the queue; no aliasing references exist.
                unsafe {
                    let task_ref = &mut *task;
                    let se = task_ref.sched_entity();
                    se.set_on_rq(false);

                    self.nr_running_dec();
                    self.load_weight_sub(se.load.weight);
                    // min_vruntime is updated AFTER the skipped entries are
                    // re-inserted (see the comment at the re-insert loop).
                }

                result = Some(task);
                break;
            }

            // Not allowed — remove temporarily and stash
            self.tasks_timeline.remove(&key);
            // s14 (msg-region strand family): PICK-SKIP-ALL names only
            // the total-failure shape (nothing pickable). A victim
            // refused AMID other candidates is invisible — yet that is
            // the msg* WAKE-STRANDED shape (100/100 strands with zero
            // skip-alls in ltp-b r2's msg region). Stamp every refusal
            // into a per-task history so the strand report (wakewatch
            // check_picked) can name WHY the victim sat unpicked for
            // the whole strand window. Armed only under dfx=schedtrace.
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SchedTrace) {
                note_pick_skip(task, skip_reason as u8);
            }
            if skip_count < skipped.len() {
                skipped[skip_count] = (key, task, skip_reason);
                skip_count += 1;
            } else {
                // Overflow — re-insert and give up (very unlikely)
                self.tasks_timeline.insert(key, task);
                break;
            }
        }

        // Re-insert all skipped entries
        for i in (0..skip_count).rev() {
            let (key, task, _) = skipped[i];
            self.tasks_timeline.insert(key, task);
        }

        // Advance min_vruntime over the COMPLETE timeline. The pick loop
        // temporarily lifts the entries this CPU cannot run (affinity,
        // cgroup throttle, ownership) out of the tree; updating
        // min_vruntime while they were out made the leftmost look further
        // right than it is. min_vruntime only ever grows, so the drift was
        // permanent — and min_vruntime is the reference point new tasks
        // and waking sleepers are placed against (enqueue_inner's
        // sleep-compensation floor), so a drifted value silently erases
        // their sleep credit. The reference kernel computes min_vruntime
        // from the leftmost entry of the full tree
        // (kernel/sched/fair.c: update_min_vruntime), which is the state
        // this ordering restores: the picked task is out, every other
        // entry is back in.
        if result.is_some() {
            self.update_min_vruntime();
        }

        // GNOME-wedge canary: the timeline held candidates but NONE was
        // pickable by this CPU — the exact shape that stranded 10 linked
        // tasks in the g2 freeze (Xorg/dbus/gmain linked, every CPU
        // idle-looping on the R20-2 recheck, nothing picked for 35 min).
        // The 5s DFX dump shows the state but never the REASON, so name it
        // here at skip time: per task its on_cpu/claim/affinity/throttle
        // legs. Rate-limited to one report per 100 jiffies system-wide so
        // the hot idle-loop callers cannot flood the console. SBI direct
        // write discipline (R34): we hold the GRQ lock here.
        if result.is_none() && skip_count > 0 {
            use crate::console::putchar_no_lock as putchar;
            fn pd(mut v: u64) {
                use crate::console::putchar_no_lock as putchar;
                let mut buf = [0u8; 20];
                let mut n = 0;
                if v == 0 {
                    putchar(b'0');
                    return;
                }
                while v > 0 {
                    buf[n] = b'0' + (v % 10) as u8;
                    v /= 10;
                    n += 1;
                }
                while n > 0 {
                    n -= 1;
                    putchar(buf[n]);
                }
            }
            fn ps(s: &[u8]) {
                for &b in s {
                    crate::console::putchar_no_lock(b);
                }
            }
            static LAST_REPORT: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now = crate::drivers::timer::get_jiffies();
            let last = LAST_REPORT.load(core::sync::atomic::Ordering::Acquire);
            // serial-clean: this canary fires on benign mid-switch-out
            // transients too — print only behind dfx=schedtrace.
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SchedTrace)
                && now.wrapping_sub(last) >= 100
                && LAST_REPORT
                    .compare_exchange(
                        last,
                        now,
                        core::sync::atomic::Ordering::AcqRel,
                        core::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                ps(b"\nPICK-SKIP-ALL cpu=");
                pd(cpu_id as u64);
                ps(b" skipped=");
                pd(skip_count as u64);
                ps(b"\n");
                for i in 0..skip_count {
                    let (_, task, reason) = skipped[i];
                    // SAFETY: diagnostic read of stable Task fields of a task
                    // that is linked on this queue (pinned by the GRQ lock we
                    // hold).
                    unsafe {
                        let claim = (*task).running_on_cpu.load(Ordering::Relaxed);
                        ps(b"  pid=");
                        pd((*task).pid() as u64);
                        // R76: WHY the decision refused — captured at scan
                        // time, immune to the switch-out release racing the
                        // print. The oncpu/claim/pk fields below are the
                        // print-time snapshot (kept for continuity with the
                        // older captures).
                        ps(b" why=");
                        pd(reason as u64);
                        ps(b" oncpu=");
                        putchar(b'0' + (*task).on_cpu() as u8);
                        ps(b" claim=");
                        if (claim as i64) < 0 {
                            putchar(b'-');
                        }
                        pd((claim as i64).unsigned_abs());
                        // R57: pick-state word (0=PICKED 1=LIVE 2=REVOKED) —
                        // with oncpu/claim it names the exact protocol leg
                        // that refused the candidate.
                        ps(b" pk=");
                        pd((*task).pick_state() as u64);
                        // R85 (tail-phase wedge hunt): the ownership
                        // ADJUDICATION fields — st= the state word (the
                        // harvester only heals RUNNING orphans; a wedged
                        // waiter shows SLEEPING here), slot= the CPUs whose
                        // current slot names the task, swout= the CPUs
                        // whose in-flight switch-out names it. why=1 with
                        // slot=0 swout=0 persisting across reports is the
                        // un-owned stale mark the R85 pick-side heal
                        // releases (STALE-ONCPU-RESET).
                        ps(b" st=0x");
                        let st = (*task).state().bits();
                        let mut v = st as u64;
                        for sh in (0..8u32).rev() {
                            let nb = ((v >> (sh * 4)) & 0xF) as u8;
                            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                        }
                        ps(b" slot=");
                        pd(crate::sched::sched::owner_slot_mask(task) as u64);
                        ps(b" swout=");
                        pd(crate::sched::sched::switching_out_mask(task) as u64);
                        ps(b" aff=0x");
                        let mut v = (*task).cpus_allowed() as u64;
                        for sh in (0..8u32).rev() {
                            let nb = ((v >> (sh * 4)) & 0xF) as u8;
                            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                        }
                        if crate::sched::cgroup::task_cgroup_throttled(task) {
                            ps(b" THROTTLED");
                        }
                        putchar(b'\n');
                        // R76 (fast-pace starvation hunt): per-pid skip
                        // counting. The full3 wedge showed freshly spawned
                        // tasks named in PICK-SKIP-ALL reports for minutes;
                        // a task whose skip count crosses 1000 is either a
                        // genuine starvation victim or a decision-time
                        // transient that repeats forever — both deserve one
                        // full-context dump (state word, queue flags, stack
                        // pointers) so the post-mortem knows whether the
                        // task was linked/sleeping/phantom at skip time.
                        // Table is direct-mapped by pid (64 slots), only
                        // ever touched under the GRQ lock this pick holds.
                        {
                            use core::sync::atomic::Ordering as Rel;
                            let pid = (*task).pid();
                            let slot = (pid as usize) & (SKIP_TRACK.len() - 1);
                            let entry = &SKIP_TRACK[slot];
                            if entry.pid.load(Rel::Relaxed) != pid {
                                entry.pid.store(pid, Rel::Relaxed);
                                entry.count.store(0, Rel::Relaxed);
                                entry.dumped_at.store(0, Rel::Relaxed);
                            }
                            let count = {
                                let c = entry.count.load(Rel::Relaxed);
                                if c < u32::MAX { c + 1 } else { c }
                            };
                            entry.count.store(count, Rel::Relaxed);
                            let dumped_at = entry.dumped_at.load(Rel::Relaxed);
                            if count >= 1000 && count / 1000 > dumped_at {
                                entry.dumped_at.store(count / 1000, Rel::Relaxed);
                                ps(b"  SKIP-DEEP pid=");
                                pd(pid as u64);
                                ps(b" n=");
                                pd(count as u64);
                                ps(b" state=");
                                let st = (*task).state().bits();
                                let mut v = st as u64;
                                for sh in (0..8u32).rev() {
                                    let nb = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                }
                                ps(b" onrq=");
                                putchar(b'0' + (*task).sched_entity().is_on_rq() as u8);
                                ps(b" ti_cpu=");
                                if ((*task).ti_cpu() as i64) < 0 {
                                    putchar(b'-');
                                }
                                pd(((*task).ti_cpu() as i64).unsigned_abs() as u64);
                                ps(b" ksp=0x");
                                let mut v = (*task).ti_kernel_sp() as u64;
                                for sh in (0..16u32).rev() {
                                    let nb = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                }
                                ps(b" thsp=0x");
                                let mut v = (*task).thread().sp;
                                for sh in (0..16u32).rev() {
                                    let nb = ((v >> (sh * 4)) & 0xF) as u8;
                                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                }
                                ps(b" vrun=");
                                pd((*task).sched_entity().get_vruntime());
                                putchar(b'\n');
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// Update current task runtime
    ///
    /// # Arguments
    /// - `now`: Current time (nanoseconds)
    pub fn update_curr(&mut self, now: u64) {
        if self.curr.is_null() {
            return;
        }

        // SAFETY: self.curr is set by set_curr() to a valid Task pointer; null check above.
        unsafe {
            let task = &mut *self.curr;
            let se = task.sched_entity();

            // Update execution time
            let delta_exec = se.update_exec_runtime(now);

            if delta_exec > 0 {
                // Update virtual runtime
                se.update_vruntime(delta_exec);

                // Update minimum vruntime
                self.update_min_vruntime();
            }
        }
    }

    /// Calculate time slice
    ///
    /// Time slice = scheduling latency * process weight / total weight
    ///
    /// # Arguments
    /// - `se`: Scheduling entity
    ///
    /// # Returns
    /// Time slice (nanoseconds)
    pub fn sched_slice(&self, se: &SchedEntity) -> u64 {
        let nr_running = self.nr_running.load(Ordering::Acquire);

        if nr_running == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // Calculate scheduling period
        // If process count is high, use min_granularity * nr_running
        // Otherwise use fixed scheduling latency
        // Garbage tripwire (regression guard): with the checked-sub
        // discipline the count can no longer wrap; anything beyond any
        // possible task bound is a corrupted read of the counter itself
        // (the 2b092c49 unexcluded-section family could produce it) —
        // name it once per boot.
        if nr_running > 1_000_000 {
            static GARBAGE_SEEN: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            if !GARBAGE_SEEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
                crate::pr_warn!(
                    "sched_slice: nr_running={} is impossible (corrupted read)",
                    nr_running
                );
            }
        }
        let sched_period = if nr_running > SCHED_LATENCY_NS / SCHED_MIN_GRANULARITY_NS {
            // Saturating: a garbage/huge count must not panic the tick —
            // belt-and-braces for nr_running_dec (see its doc).
            SCHED_MIN_GRANULARITY_NS.saturating_mul(nr_running)
        } else {
            SCHED_LATENCY_NS
        };

        // Calculate time slice
        // slice = period * weight / total_weight
        let load_weight = self.load_weight.load(Ordering::Acquire);

        if load_weight == 0 {
            return SCHED_MIN_GRANULARITY_NS;
        }

        // Multiply first, then divide to avoid integer truncation to 0.
        // This matches Linux's approach (period * weight / total_weight).
        let slice = (sched_period as u128 * se.load.weight as u128 / load_weight as u128) as u64;

        // Ensure not less than minimum granularity
        slice.max(SCHED_MIN_GRANULARITY_NS)
    }

    /// Check if current task needs to be preempted
    ///
    /// # Arguments
    /// - `curr`: Current task
    /// - `se`: Newly woken task
    ///
    /// # Returns
    /// Returns true if preemption is needed
    pub fn check_preempt(&self, curr: &SchedEntity, se: &SchedEntity) -> bool {
        // If new task's vruntime is smaller than current task, should preempt
        let curr_vruntime = curr.get_vruntime();
        let se_vruntime = se.get_vruntime();

        // Use "wakeup granularity" as threshold
        // Only preempt if difference exceeds this value
        let wakeup_granularity = SCHED_MIN_GRANULARITY_NS;

        // Prevent vruntime wraparound
        if se_vruntime < curr_vruntime {
            let delta = curr_vruntime - se_vruntime;
            delta > wakeup_granularity
        } else {
            false
        }
    }

    /// Set currently running task
    pub fn set_curr(&mut self, task: *mut crate::process::Task) {
        self.curr = task;
    }

    /// Get currently running task
    #[inline]
    pub fn get_curr(&self) -> *mut crate::process::Task {
        self.curr
    }

    /// Clear run queue
    pub fn clear(&mut self) {
        // Mark all tasks as not in queue
        for (_, &task) in self.tasks_timeline.iter() {
            if !task.is_null() {
                // SAFETY: task is from the queue, a valid pointer; clear() drains the entire queue.
                unsafe {
                    let task_ref = &mut *task;
                    task_ref.sched_entity().set_on_rq(false);
                }
            }
        }

        self.tasks_timeline.clear();
        self.curr = core::ptr::null_mut();
        self.nr_running.store(0, Ordering::Release);
        self.load_weight.store(0, Ordering::Release);
    }
}

impl Default for CfsRunQueue {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for CfsRunQueue {}
unsafe impl Sync for CfsRunQueue {}

/// Calculate time slice (milliseconds)
///
/// Convert nanosecond time slice to milliseconds for timer interrupt
pub fn sched_slice_to_ms(slice_ns: u64) -> u32 {
    (slice_ns / 1_000_000) as u32
}

/// Convert milliseconds to nanoseconds
pub fn ms_to_ns(ms: u32) -> u64 {
    (ms as u64) * 1_000_000
}

/// Timebase multiplier: QEMU uses 10MHz timebase, convert to nanoseconds
const TIMEBASE_MULT: u64 = 100;

/// Get current time (nanoseconds)
///
/// Use RISC-V time register
pub fn sched_clock() -> u64 {
    // Arch wall-clock counter (rdtime on riscv64, TSC on x86_64).
    let time: u64 = crate::arch::cpu::read_time();
    // Assume clock frequency is 10MHz (100ns precision)
    // Actual value needs adjustment based on platform
    time * TIMEBASE_MULT
}

// ============================================================================
// Fair Scheduling Class Implementation
// ============================================================================

/// Fair scheduling class - wraps CFS
///
/// With the global RunQueue design, the actual CFS enqueue/dequeue/pick
/// happens directly in sched.rs on GlobalRunQueue::cfs_rq. The SchedClass
/// methods are simplified to no-ops or minimal stubs.
pub struct FairSchedClass;

impl FairSchedClass {
    pub const fn new() -> Self {
        Self
    }
}

impl SchedClass for FairSchedClass {
    fn name(&self) -> &'static str {
        "fair"
    }

    /// Actual enqueue happens in sched.rs::enqueue_task_locked.
    fn enqueue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) {
    }

    /// Actual dequeue happens in sched.rs::dequeue_task.
    fn dequeue_task(&self, _rq: RunQueueRef, _task: *mut Task, _flags: i32) -> bool {
        false
    }

    /// Actual yield handling happens in sched.rs.
    fn yield_task(&self, _rq: RunQueueRef) {
    }

    /// Preemption checks happen in sched.rs.
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

    /// CFS load balancing is handled separately
    fn balance(&self, _rq: RunQueueRef, _prev: *mut Task) -> bool {
        false
    }

    /// CPU selection — simplified for global RQ.
    /// The global queue is inherently balanced; just keep on the current CPU.
    fn select_task_rq(&self, task: *mut Task, cpu: i32, _flags: i32) -> i32 {
        if task.is_null() {
            return cpu;
        }

        // With global RQ, no per-CPU load balancing needed.
        // Just return the preferred CPU from the task's affinity.
        // SAFETY: task is a valid pointer from the caller; null check above.
        unsafe {
            let task_ref = &*task;
            let cpus_allowed = task_ref.cpus_allowed();
            let wake_cpu = task_ref.wake_cpu();

            // Prefer wake_cpu if allowed
            if wake_cpu >= 0 {
                let wake = wake_cpu as usize;
                if wake < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << wake)) != 0 {
                    return wake_cpu;
                }
            }

            // Check if current cpu is allowed
            let cpu_usize = cpu as usize;
            if cpu_usize < crate::config::MAX_CPUS && (cpus_allowed & (1u32 << cpu_usize)) != 0 {
                return cpu;
            }

            // Fallback: first allowed CPU
            for c in 0..crate::config::MAX_CPUS {
                if (cpus_allowed & (1u32 << c)) != 0 {
                    return c as i32;
                }
            }

            cpu
        }
    }

    /// Actual tick handling happens in sched.rs::scheduler_tick.
    fn task_tick(&self, _rq: RunQueueRef, _task: *mut Task, _queued: bool) {
    }

    /// Actual runtime tracking happens in sched.rs.
    fn update_curr(&self, _rq: RunQueueRef) {
    }

    fn get_rr_interval(&self, _rq: RunQueueRef, task: *mut Task) -> u32 {
        if task.is_null() {
            return 1;
        }

        // Return a reasonable default; actual slice is computed in sched.rs.
        1
    }

    /// Actual check happens in sched.rs via GlobalRunQueue::cfs_rq.
    fn has_runnable(&self, _rq: RunQueueRef) -> bool {
        false
    }

    fn next_class(&self) -> Option<&'static dyn SchedClass> {
        Some(&super::idle::IDLE_SCHED_CLASS)
    }
}

/// Global fair scheduling class instance
pub static FAIR_SCHED_CLASS: FairSchedClass = FairSchedClass::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_weight() {
        // nice = 0 weight should be 1024
        let lw = LoadWeight::from_nice(0);
        assert_eq!(lw.weight, 1024);

        // nice = -20 weight should be largest
        let lw_high = LoadWeight::from_nice(-20);
        assert!(lw_high.weight > lw.weight);

        // nice = 19 weight should be smallest
        let lw_low = LoadWeight::from_nice(19);
        assert!(lw_low.weight < lw.weight);
    }

    #[test]
    fn test_vruntime_calculation() {
        let se = SchedEntity::new();

        // When nice = 0, vruntime should equal actual runtime
        let delta = 1_000_000; // 1ms
        let vruntime = se.calc_delta_fair(delta);
        assert_eq!(vruntime, delta);
    }

    #[test]
    fn test_cfs_rq_enqueue_dequeue() {
        let mut rq = CfsRunQueue::new();

        // Create test task structure
        // Note: Actual testing requires valid Task pointers
        assert!(rq.is_empty());
        assert_eq!(rq.nr_running(), 0);
    }
}
