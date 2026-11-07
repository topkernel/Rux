//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Pressure Stall Information — /proc/pressure/{cpu,memory,io}
//! (the reference kernel's kernel/sched/psi.c semantics).
//!
//! File shape (identical to the reference):
//!
//! ```text
//! some avg10=0.00 avg60=0.00 avg300=0.00 total=0
//! ```
//!
//! `cpu` reports the `some` line only; `memory` and `io` report
//! `some` then `full`. `avgN` is the percentage of the last N
//! seconds during which some (`some`) / all non-idle (`full`)
//! tasks were stalled, 2-decimal precision, carried as a decaying
//! exponential average exactly like the reference (calc_load with
//! the psi constants, folds every 2 s). `total` is cumulative
//! stall time in microseconds.
//!
//! # Stall conditions (reference test_state, v6.1)
//!
//! The reference keeps per-CPU task counts (NR_RUNNING queued incl.
//! running, NR_IOWAIT io-sleeping, NR_MEMSTALL memstalled) and
//! charges elapsed time at every state transition:
//!
//! ```text
//! CPU_SOME: NR_RUNNING > oncpu          (runnable-but-not-running)
//! MEM_SOME: NR_MEMSTALL
//! MEM_FULL: NR_MEMSTALL && NR_RUNNING == NR_MEMSTALL_RUNNING
//! IO_SOME:  NR_IOWAIT
//! IO_FULL:  NR_IOWAIT && !NR_RUNNING
//! ```
//!
//! Rux has a global run queue: `grq.nr_running` counts queued
//! (runnable, not on-cpu) tasks, so NR_RUNNING == queued + oncpu and
//! the conditions become global system predicates:
//!
//! ```text
//! cpu.some  = queued > 0
//! mem.some  = memstall > 0
//! mem.full  = memstall > 0 && queued + oncpu <= memstall
//! io.some   = iowait > 0
//! io.full   = iowait > 0 && queued == 0 && oncpu == 0
//! ```
//!
//! (every memstalled task is itself queued — the alloc slowpath
//! waiter yields — or on-cpu — the reclaim holder, so
//! queued+oncpu == memstall means no other non-idle task exists.)
//!
//! # Cost shape: sampling, not per-transition work
//!
//! The reference charges time inside every psi_task_change —
//! an event-driven cost on every enqueue/dequeue/switch edge.
//! Rux's scheduler edges are perf-gated (GRQ hold, context-switch
//! cost), so this implementation inverts the design at zero
//! scheduler-path cost:
//!
//! - The sampler runs from the scheduler tick, at most once per
//!   jiffy (CAS-gated; any CPU may win). It reads the conditions
//!   (existing atomics: grq.nr_running, grq idle mask, plus the two
//!   psi stall counters) and charges the elapsed interval to each
//!   line whose condition held at the sample. NO new work runs on
//!   enqueue/dequeue/context-switch paths — cpu pressure costs the
//!   scheduler literally nothing extra.
//! - memory/io stalls are marked by the stalling paths themselves
//!   (page_alloc slowpath, IoCompletion::wait) with one relaxed
//!   atomic per enter/exit on a per-task bit; nested sections
//!   (reclaim's own allocations re-entering alloc_pages) count once.
//!
//! Precision: a condition is attributed to the whole sampling
//! interval during which it was observed (HZ=100 → 10 ms
//! granularity). Sustained pressure (the signal's purpose) is
//! exact; bursts shorter than a tick are under-sampled. avg10
//! resolution is therefore one tick per window (0.1 % of 10 s);
//! `total` counts in tick quanta. This divergence from the
//! reference's per-transition accounting is recorded in
//! docs/architecture/design-divergences.md.
//!
//! # Averages
//!
//! The reference folds a period sample into three exponential
//! averages every PSI_FREQ (2 s) with fixed-point constants
//! EXP_10s/60s/300s (include/linux/sched/loadavg.h calc_load,
//! FIXED_1 = 1 << 11). This module uses the same constants and the
//! same math; the fold runs from the tick sampler, so missed
//! periods (the reference's quiet-system deferred work) cannot
//! happen — the tick always fires.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::sync::spinlock::Spinlock;

/// Fixed-point shift for the averages (the reference's FSHIFT).
const FSHIFT: u64 = 11;
/// Fixed-point 1.0 (the reference's FIXED_1).
const FIXED_1: u64 = 1 << FSHIFT;

/// EMA decay factors for 2-second folds (the reference's psi.c
/// constants: 1/exp(2s/window) in FIXED_1 fixed point).
const EXP_10S: u64 = 1677;
const EXP_60S: u64 = 1981;
const EXP_300S: u64 = 2034;
const EXP: [u64; 3] = [EXP_10S, EXP_60S, EXP_300S];

/// Fold period in jiffies (the reference's PSI_FREQ = 2*HZ+1 — the
/// +1 de-aliases the work from the tick; Rux folds from the tick
/// itself so a whole period is used).
const PSI_FREQ_J: u64 = 2 * crate::config::KERNEL_HZ as u64;

/// Line indices: [0]=cpu.some, [1]=mem.some, [2]=mem.full,
/// [3]=io.some, [4]=io.full.
const NLINE: usize = 5;
const L_CPU_SOME: usize = 0;
const L_MEM_SOME: usize = 1;
const L_MEM_FULL: usize = 2;
const L_IO_SOME: usize = 3;
const L_IO_FULL: usize = 4;

/// Per-task psi stall bits (Task::psi_stall).
pub const STALL_MEMSTALL: u8 = 1 << 0;
pub const STALL_IOWAIT: u8 = 1 << 1;

/// Tasks currently marked in a memory stall section (alloc
/// slowpath: gate wait + direct reclaim).
static MEMSTALL_N: AtomicUsize = AtomicUsize::new(0);

/// Tasks currently blocked in IoCompletion::wait (block io).
static IOWAIT_N: AtomicUsize = AtomicUsize::new(0);

/// One reported line: decaying averages (fixed-point percent) plus
/// the cumulative stall time.
struct Line {
    /// Stall time accumulated in the current (partially elapsed)
    /// fold period, nanoseconds.
    period_stall_ns: u64,
    /// Cumulative stall time, nanoseconds (displayed as µs).
    total_ns: u64,
    /// avg10/avg60/avg300 in FIXED_1 units of percent
    /// (100 % == 100 * FIXED_1).
    avg: [u64; 3],
}

impl Line {
    const fn new() -> Self {
        Line { period_stall_ns: 0, total_ns: 0, avg: [0; 3] }
    }
}

/// Sampler state — everything behind one short spinlock.
struct PsiState {
    /// sched_clock of the last sample point (interval attribution).
    last_sample_ns: u64,
    /// sched_clock of the last EMA fold.
    last_fold_ns: u64,
    /// jiffies of the last EMA fold.
    last_fold_j: u64,
    lines: [Line; NLINE],
}

impl PsiState {
    const fn new() -> Self {
        PsiState {
            last_sample_ns: 0,
            last_fold_ns: 0,
            last_fold_j: 0,
            lines: [const { Line::new() }; NLINE],
        }
    }
}

static PSI: Spinlock<PsiState> = Spinlock::new(PsiState::new());

/// Once-per-jiffy CAS gate so exactly one CPU folds each tick.
static LAST_SAMPLE_J: AtomicU64 = AtomicU64::new(0);

/// Monotonic nanoseconds (sched_clock; the timer's ratchet keeps it
/// cross-CPU monotonic — deltas are clamped defensively anyway).
#[inline]
fn now_ns() -> u64 {
    crate::sched::fair::sched_clock()
}

/// Number of busy (non-idle) CPUs, from the GRQ idle bitmap.
fn oncpu_count() -> u64 {
    let started = crate::arch::smp::num_started_cpus();
    let idle = crate::sched::sched::GlobalRunQueue::grq_idle_mask() as u64;
    let idle_bits = (idle & ((1u64 << started) - 1)).count_ones() as u64;
    (started as u64).saturating_sub(idle_bits)
}

/// Evaluate the five stall conditions (see the module comment for
/// the derivation from the reference's test_state).
fn conditions() -> [bool; NLINE] {
    let queued =
        crate::sched::sched::GlobalRunQueue::grq_nr_running() as u64;
    let oncpu = oncpu_count();
    let memstall = MEMSTALL_N.load(Ordering::Relaxed) as u64;
    let iowait = IOWAIT_N.load(Ordering::Relaxed) as u64;
    [
        queued > 0,
        memstall > 0,
        memstall > 0 && queued + oncpu <= memstall,
        iowait > 0,
        iowait > 0 && queued == 0 && oncpu == 0,
    ]
}

/// The reference's calc_load (include/linux/sched/loadavg.h):
/// `newload = (load*exp + active*(FIXED_1-exp)) / FIXED_1` with the
/// same rounding term.
fn calc_load(load: u64, exp: u64, active: u64) -> u64 {
    let mut newload = load * exp + active * (FIXED_1 - exp);
    if active >= load {
        newload += FIXED_1 - 1;
    }
    newload / FIXED_1
}

/// One sample point: charge the elapsed interval to every line
/// whose condition holds, then fold the EMA averages when a 2 s
/// period has fully elapsed. Called from the tick (once per jiffy)
/// and from reads (an extra sample makes `total` fresh at read
/// time, like the reference's collect_percpu_times on psi_show).
fn sample_point() {
    let mut st = PSI.lock_irqsave();
    let now = now_ns();
    let elapsed = now.saturating_sub(st.last_sample_ns);
    st.last_sample_ns = now;

    let cond = conditions();
    for i in 0..NLINE {
        if cond[i] && elapsed > 0 {
            st.lines[i].total_ns += elapsed;
            st.lines[i].period_stall_ns += elapsed;
        }
    }

    // 2 s fold (PSI_FREQ): sample the period as a percentage and
    // feed the three decay windows.
    let j = crate::drivers::timer::get_jiffies();
    if j.wrapping_sub(st.last_fold_j) >= PSI_FREQ_J {
        let period_ns = now.saturating_sub(st.last_fold_ns).max(1);
        for i in 0..NLINE {
            let stall = st.lines[i].period_stall_ns;
            // pct = stall*100/period in FIXED_1 units (the reference
            // does div_u64(time*100, period) * FIXED_1).
            let pct = stall.saturating_mul(100) / period_ns * FIXED_1;
            for w in 0..3 {
                st.lines[i].avg[w] = calc_load(st.lines[i].avg[w], EXP[w], pct);
            }
            st.lines[i].period_stall_ns = 0;
        }
        st.last_fold_j = j;
        st.last_fold_ns = now;
    }
}

/// Tick hook: called from scheduler_tick on every CPU. At most one
/// CPU folds per jiffy (CAS gate); the fold itself is a few atomic
/// loads plus arithmetic under a short irqsave spinlock — ~100 ns
/// per 10 ms, off every scheduling fast path.
pub fn psi_tick() {
    let j = crate::drivers::timer::get_jiffies();
    let last = LAST_SAMPLE_J.load(Ordering::Relaxed);
    if j != last
        && LAST_SAMPLE_J
            .compare_exchange(last, j, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        sample_point();
    }
}

// ---------------------------------------------------------------------------
// Stall marking (memory / io phases)
// ---------------------------------------------------------------------------

/// RAII stall-section marker. Nested sections (reclaim's own
/// allocations re-entering the alloc slowpath, waits inside waits)
/// count once: the per-task bit is only counted globally by the
/// outermost section.
pub struct StallGuard {
    task: Option<&'static crate::process::task::Task>,
    bit: u8,
    counted: bool,
}

impl StallGuard {
    #[inline]
    fn enter(bit: u8, counter: &AtomicUsize) -> Self {
        let task = match crate::sched::current() {
            Some(t) => t,
            None => return StallGuard { task: None, bit, counted: false },
        };
        // The current task outlives the section: a stall section
        // always unwinds inside its own syscall frame.
        let prev = task.psi_stall.fetch_or(bit, Ordering::Relaxed);
        let counted = prev & bit == 0;
        if counted {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        StallGuard { task: Some(task), bit, counted }
    }

    /// Mark the current task in a memory stall section (the alloc
    /// slowpath: gate wait + direct reclaim — the reference's
    /// psi_memstall_enter around __alloc_pages_slowpath).
    #[inline]
    pub fn memstall() -> Self {
        StallGuard::enter(STALL_MEMSTALL, &MEMSTALL_N)
    }

    /// Mark the current task blocked on block I/O
    /// (IoCompletion::wait — the reference's in_iowait around
    /// io_schedule).
    #[inline]
    pub fn iowait() -> Self {
        StallGuard::enter(STALL_IOWAIT, &IOWAIT_N)
    }
}

impl Drop for StallGuard {
    #[inline]
    fn drop(&mut self) {
        if let Some(task) = self.task {
            let prev = task.psi_stall.fetch_and(!self.bit, Ordering::Relaxed);
            let was_set = prev & self.bit != 0;
            if self.counted && was_set {
                match self.bit {
                    STALL_MEMSTALL => {
                        MEMSTALL_N.fetch_sub(1, Ordering::Relaxed);
                    }
                    _ => {
                        IOWAIT_N.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reporting (procfs read side)
// ---------------------------------------------------------------------------

/// Resource selector for the per-resource files.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Memory,
    Io,
}

/// One display line (`some avg10=... total=...`) for a line index.
fn render_line(out: &mut alloc::string::String, label: &str, line: &Line) {
    let (i10, f10) = load_int_frac(line.avg[0]);
    let (i60, f60) = load_int_frac(line.avg[1]);
    let (i300, f300) = load_int_frac(line.avg[2]);
    use core::fmt::Write;
    let _ = write!(
        out,
        "{} avg10={}.{:02} avg60={}.{:02} avg300={}.{:02} total={}\n",
        label,
        i10,
        f10,
        i60,
        f60,
        i300,
        f300,
        line.total_ns / 1000
    );
}

/// The reference's LOAD_INT/LOAD_FRAC: integer and 2-decimal part
/// of a FIXED_1 fixed-point percentage.
fn load_int_frac(v: u64) -> (u64, u64) {
    ((v >> FSHIFT) as u64, ((v & (FIXED_1 - 1)) * 100) >> FSHIFT)
}

/// Generate the file content for one resource. Takes one extra
/// sample point first (fresh `total`, like the reference's
/// collect-on-show) and then renders under the same lock.
pub fn generate(res: Resource) -> alloc::vec::Vec<u8> {
    sample_point();
    let st = PSI.lock_irqsave();
    let mut out = alloc::string::String::new();
    match res {
        Resource::Cpu => {
            // "CPU FULL is undefined at the system level" — the
            // reference prints the some line only.
            render_line(&mut out, "some", &st.lines[L_CPU_SOME]);
        }
        Resource::Memory => {
            render_line(&mut out, "some", &st.lines[L_MEM_SOME]);
            render_line(&mut out, "full", &st.lines[L_MEM_FULL]);
        }
        Resource::Io => {
            render_line(&mut out, "some", &st.lines[L_IO_SOME]);
            render_line(&mut out, "full", &st.lines[L_IO_FULL]);
        }
    }
    drop(st);
    out.into_bytes()
}

/// Expose a line snapshot for the in-kernel test (format + EMA
/// sanity; runs without touching live accounting).
#[cfg(feature = "unit-test")]
pub fn snapshot_for_test() -> [(u64, u64, u64, u64); NLINE] {
    let st = PSI.lock_irqsave();
    let mut r = [(0u64, 0u64, 0u64, 0u64); NLINE];
    for i in 0..NLINE {
        r[i] = (
            st.lines[i].avg[0],
            st.lines[i].avg[1],
            st.lines[i].avg[2],
            st.lines[i].total_ns,
        );
    }
    r
}

/// One EMA fold step for the in-kernel test: sample a period of
/// `stall_ns` out of `period_ns` into window `w` (0/1/2 =
/// 10/60/300 s) of `load`.
#[cfg(feature = "unit-test")]
pub fn ema_step_for_test(load: u64, w: usize, stall_ns: u64, period_ns: u64) -> u64 {
    let pct = stall_ns.saturating_mul(100) / period_ns.max(1) * FIXED_1;
    calc_load(load, EXP[w], pct)
}

/// The reference's LOAD_INT/LOAD_FRAC split, exposed for the
/// in-kernel test.
#[cfg(feature = "unit-test")]
pub fn load_int_frac_for_test(v: u64) -> (u64, u64) {
    load_int_frac(v)
}
