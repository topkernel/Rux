//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! fork/exec/exit phase profiler — attribution probe for the
//! process-lifecycle perf lane (`feature/perf-fork`).
//!
//! Armed by the kernel cmdline `forkprof=1` (own key, syscost-style;
//! no debug/switches.rs bit is consumed). Costs nothing when off: every
//! hook is one relaxed load of the cached armed word, and the phase
//! adds are plain fetch_adds into static arrays.
//!
//! What it measures and why: lmbench lat_proc,fork sits ~20x over the
//! reference column while a small-mm fork+exit accounts for nearly all
//! of it (forkbench fb_mm). The phase timers decompose the two bodies:
//!
//! - do_clone (plain-fork arm, CLONE_VM==0): task-slot alloc,
//!   copy_thread, fdtable deep copy, the whole mm fork (and inside it:
//!   VMA snapshot, COW page-table walk, VMA replay), enqueue.
//! - do_exit (leader arm): mm drop (and inside MmStruct::drop: VMA
//!   release, synchronize_rcu grace wait, page-table free), orphan
//!   reparent, exit-notify defer.
//!
//! Dump: `report()` prints one line per phase (count, avg, max in us)
//! and is called from the shutdown cascade — every probe rig ends with
//! a reboot(2), so the numbers ride the serial log out.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// Phase slots (see `report()` for the names).
pub const P_FK_TOTAL: usize = 0;
pub const P_FK_ALLOC: usize = 1;
pub const P_FK_COPYTHREAD: usize = 2;
pub const P_FK_FDTABLE: usize = 3;
pub const P_FK_MMFORK: usize = 4;
pub const P_FK_ENQUEUE: usize = 5;
pub const P_MM_SNAPSHOT: usize = 6;
pub const P_MM_COW: usize = 7;
pub const P_MM_REPLAY: usize = 8;
pub const P_EX_TOTAL: usize = 9;
pub const P_EX_MMDROP: usize = 10;
pub const P_DROP_VMAFREE: usize = 11;
pub const P_DROP_RCU: usize = 12;
pub const P_DROP_PTFREE: usize = 13;
pub const P_EX_REPARENT: usize = 14;
pub const P_EX_NOTIFY: usize = 15;
pub const P_EXEC_TOTAL: usize = 16;
/// Scalar-only slots below: `bump()`ed counts (VMAs per fork, PTEs
/// copied, leaf tables allocated) — reported raw, never `add()`ed.
pub const S_FORKS_VMASES: usize = 17;
pub const S_COW_PTES: usize = 18;
pub const S_COW_TABLES: usize = 19;
pub const S_MM_DROPS: usize = 20;
/// wait4 whole body (includes the blocking sleep — the wall the
/// process-lifecycle loop pays waiting for the child side).
pub const P_WT_BODY: usize = 21;
/// The schedule() inside do_wait's sleep loop — returns only after the
/// child ran, exited and woke us: this is the fork->exit handoff.
pub const P_WT_SCHED: usize = 22;
/// release_task — the reaping parent frees the child's Task.
pub const P_WT_REAP: usize = 23;
/// Post-context-switch deferred exit-notify processing (SIGCHLD +
/// wait_chldexit group wake) — runs in the switcher's context.
pub const P_SCHED_NOTIFY: usize = 24;
/// Task::wake_up_impl body — the waker-side cost (select_cpu + locked
/// enqueue + resched/IPI dispatch).
pub const P_WAKE_BODY: usize = 25;
/// __schedule's pick section (queue pick + heal machinery).
pub const P_PICK: usize = 26;
/// context_switch proper (save/restore + switch asm tail).
pub const P_CTXSW: usize = 27;
/// wake stamp -> first context-switch-IN delta (the wake-to-run latency
/// the process-lifecycle loop pays).
pub const P_WAKE_RUN: usize = 28;
/// fork-enqueue stamp -> the child's first context-switch-IN (the new
/// task's queue latency; separate slot from real wakes).
pub const P_CHILD_RUN: usize = 29;
/// exit_files: dropping the fd-table reference — includes FdTable::drop
/// (the slot sweep + close decisions) when this was the last reference.
/// The perf-gap-hunt r11 decomposition slot: the exit body had ~11us
/// unattributed vs its sub-phases, and the Drop's full-table slot sweep
/// was the prime suspect.
pub const P_EX_FDDROP: usize = 30;
/// r11 round-2 decomposition: entry prologue (journal/lock force
/// releases, ptrace detach, coredump gate, binder reap, rusage fold)
/// up to exit_mm — the remaining unattributed exit-middle share.
pub const P_EX_PREMM: usize = 31;
/// exit_files end -> exit_notify start (futex/PI cleanup, itimers,
/// POSIX timers, SEM_UNDO, reparenting, thread-group split).
pub const P_EX_POSTF: usize = 32;
pub const P_COUNT: usize = 33;

/// Per-pid wake stamps (cycle domain) for the wake->run delta. Keyed by
/// pid & mask; a stamp is consumed (zeroed) by the first switch-in.
const WAKE_STAMP_PIDS: usize = 1024;
const ZERO_U64: AtomicU64 = AtomicU64::new(0);
static WAKE_STAMPS: [AtomicU64; WAKE_STAMP_PIDS] = [ZERO_U64; WAKE_STAMP_PIDS];
static CHILD_STAMPS: [AtomicU64; WAKE_STAMP_PIDS] = [ZERO_U64; WAKE_STAMP_PIDS];

/// Record the wake instant for `pid` (called on the wake's success
/// path, still inside the waker's context).
#[inline]
pub fn stamp_wake(pid: u32) {
    WAKE_STAMPS[(pid as usize) & (WAKE_STAMP_PIDS - 1)].store(now(), Ordering::Relaxed);
}

/// Record the enqueue instant of a freshly forked child (consumed at the
/// child's first switch-in — the new-task queue latency).
#[inline]
pub fn stamp_child(pid: u32) {
    CHILD_STAMPS[(pid as usize) & (WAKE_STAMP_PIDS - 1)].store(now(), Ordering::Relaxed);
}

/// Consume any pending stamp for `pid` and, if one was found, account
/// the stamp->switch-in delta (called at context-switch-in of `pid`).
#[inline]
pub fn consume_wake(pid: u32) {
    let idx = (pid as usize) & (WAKE_STAMP_PIDS - 1);
    let ct = CHILD_STAMPS[idx].load(Ordering::Relaxed);
    if ct != 0
        && CHILD_STAMPS[idx]
            .compare_exchange(ct, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        add(P_CHILD_RUN, ct);
        hist_account(0, now().saturating_sub(ct), pid);
        return;
    }
    let wt = WAKE_STAMPS[idx].load(Ordering::Relaxed);
    if wt == 0 {
        return;
    }
    if WAKE_STAMPS[idx]
        .compare_exchange(wt, 0, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        add(P_WAKE_RUN, wt);
        hist_account(1, now().saturating_sub(wt), pid);
    }
}

// ---- timeline trace (forkprof=2: phases PLUS the event ring) ----
//
// One ring of (site, pid, tsc) events covering the fork/exit/wait
// lifecycle; dumped raw at shutdown. 2048 dumped events is ~200
// lifecycle iterations — enough for median-pattern reading of where
// the wall goes, small enough not to drown the serial log.
pub const T_FORK_ENTRY: u8 = 0;
pub const T_FORK_DONE: u8 = 1;
pub const T_CTXSW_IN: u8 = 2;
pub const T_EXIT_ENTRY: u8 = 3;
pub const T_MMDROP_DONE: u8 = 4;
pub const T_NOTIFY_DONE: u8 = 5;
pub const T_SNDNOTIFY_DONE: u8 = 6;
pub const T_WAIT_ENTRY: u8 = 7;
pub const T_WAIT_FOUND: u8 = 8;
pub const T_REAP_DONE: u8 = 9;
pub const T_SCHED_SLEEP: u8 = 10;
pub const T_SCHED_WAKE: u8 = 11;
pub const T_NEWBORN_TAIL: u8 = 12;
pub const T_SIGCHECK: u8 = 13;
pub const T_SYSENTRY: u8 = 14;

struct TEv {
    tsc: AtomicU64,
    site: AtomicU8,
    pid: AtomicU32,
    /// Recording CPU (arch::cpu_id() in the recorder's context). For
    /// T_CTXSW_IN this is the CPU the task actually switched in on —
    /// the placement fact the wake/halt-alignment hunt needs (r12).
    cpu: AtomicU8,
}
const EV0: TEv = TEv {
    tsc: AtomicU64::new(0),
    site: AtomicU8::new(255),
    pid: AtomicU32::new(0),
    cpu: AtomicU8::new(255),
};
static RING: [TEv; 4096] = [EV0; 4096];
static RING_CUR: AtomicU64 = AtomicU64::new(0);

/// Trace armed separately: `forkprof=2` (phases + trace); `forkprof=1`
/// is phases only.
///
/// perf-gap-hunt (2026-11-05): the syscall entry path calls this on
/// EVERY syscall (trap.rs handle_syscall), and the outlined call+frame
/// was per-syscall tax even with the trace off. Same contract as the
/// syscost irq_note gate: the wrapper skips inline unless ARMED == 2
/// (the only state where `on()` can be true), so the undetermined
/// state (0) and the settled-off state (1) both return false without
/// a call, and the armed body runs only on forkprof boots.
#[inline(always)]
pub fn trace_on() -> bool {
    if ARMED.load(Ordering::Relaxed) != 2 {
        return false;
    }
    trace_on_armed()
}

/// Outlined body of [`trace_on`] (ARMED == 2 only).
#[cold]
#[inline(never)]
fn trace_on_armed() -> bool {
    on() && crate::cmdline::get_param("forkprof").as_deref() == Some("2")
}

/// Record one timeline event (dropped silently when the trace is off).
#[inline]
pub fn trace(site: u8, pid: u32) {
    if !trace_on() {
        return;
    }
    let i = (RING_CUR.fetch_add(1, Ordering::Relaxed) % 4096) as usize;
    RING[i].tsc.store(now(), Ordering::Relaxed);
    RING[i].site.store(site, Ordering::Relaxed);
    RING[i].pid.store(pid, Ordering::Relaxed);
    RING[i].cpu.store(crate::arch::cpu_id() as u8, Ordering::Relaxed);
}

/// Dump the ring (forkprof=2 only): one line per event, delta from the
/// previous event; last 2048 events.
pub fn report_trace() {
    if !trace_on() {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0;
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let names: [&str; 16] = [
        "fork_entry",
        "fork_done",
        "ctxsw_in",
        "exit_entry",
        "mmdrop_done",
        "notify_done",
        "sndnotify_done",
        "wait_entry",
        "wait_found",
        "reap_done",
        "sched_sleep",
        "sched_wake",
        "newborn_tail",
        "sigcheck",
        "sysentry",
        "sysnr",
    ];
    let cur = RING_CUR.load(Ordering::Relaxed);
    let start = cur.saturating_sub(2048);
    crate::println!("FTRACE BEGIN n={}", cur);
    let mut prev = 0u64;
    for k in start..cur {
        let i = (k % 4096) as usize;
        let t = RING[i].tsc.load(Ordering::Relaxed);
        if t == 0 {
            continue;
        }
        let site = RING[i].site.load(Ordering::Relaxed);
        let pid = RING[i].pid.load(Ordering::Relaxed);
        let cpu = RING[i].cpu.load(Ordering::Relaxed);
        let d = if prev == 0 { 0 } else { t.saturating_sub(prev) / per_us };
        prev = t;
        let nm = if (site as usize) < names.len() { names[site as usize] } else { "?" };
        if cpu != 255 {
            crate::println!("FTRACE {} pid={} cpu={} dt={}us", nm, pid, cpu, d);
        } else {
            crate::println!("FTRACE {} pid={} dt={}us", nm, pid, d);
        }
    }
    crate::println!("FTRACE END");
}
struct PhaseStat {
    count: AtomicU64,
    cyc: AtomicU64,
    max: AtomicU64,
}

const EMPTY: PhaseStat = PhaseStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

static STATS: [PhaseStat; P_COUNT] = [EMPTY; P_COUNT];

static ARMED: AtomicU8 = AtomicU8::new(0);
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Cached arm check (syscost::on pattern; never cache before cmdline).
///
/// ARMED values: 1 = off, 2 = fully on, 3 = tick-path only (see
/// tick_on). `on()` gates every fork/exit/wake/mm-path hook.
#[inline]
pub fn on() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 | 3 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => {
                    match crate::cmdline::get_param("forkprof").as_deref() {
                        Some("1") | Some("2") => 2,
                        Some("3") => 3,
                        _ => 1,
                    }
                }
            };
            ARMED.store(v, Ordering::Relaxed);
            v == 2
        }
    }
}

/// Tick-path-only arming (`forkprof=3`): the freeze probes that live on
/// the timer tick (FTICKGAP, the GRQ sampler, tick RIP) run, but every
/// fork/exit/wake/mm-path hook stays off, so the measured path keeps
/// its unarmed timing. Purpose: full arming measurably suppresses the
/// fork slow mode (r13: armed gates 4/4 fast vs unarmed 3/5 slow), so
/// all prior slow-mode topology came from armed boots whose mode
/// distribution the arming itself shifted. Level 3 captures the freeze
/// anatomy of a genuinely-unarmed-slow boot.
#[inline]
pub fn tick_on() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        2 | 3 => true,
        1 => false,
        _ => {
            on();
            matches!(ARMED.load(Ordering::Relaxed), 2 | 3)
        }
    }
}

/// Cheap "possibly armed" check for hot paths (the tick): one relaxed
/// load once the word is resolved; the undetermined state falls back
/// to tick_on() which resolves it once (tick RIP belongs to the
/// tick-path probe set, so level 3 counts as armed here).
#[inline]
pub fn maybe_armed() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        0 => tick_on(),
        v => v >= 2,
    }
}

/// Cycle-counter read (raw TSC / cycle domain; the report converts).
#[inline]
pub fn now() -> u64 {
    crate::arch::cpu::read_counter()
}

/// Account `elapsed = now() - t0` cycles against phase `p`.
#[inline]
pub fn add(p: usize, t0: u64) {
    let d = now().saturating_sub(t0);
    let s = &STATS[p];
    s.count.fetch_add(1, Ordering::Relaxed);
    s.cyc.fetch_add(d, Ordering::Relaxed);
    s.max.fetch_max(d, Ordering::Relaxed);
}

/// Scalar counter (pages copied, tables freed, ...) — reported raw.
#[inline]
pub fn bump(p: usize, n: u64) {
    STATS[p].cyc.fetch_add(n, Ordering::Relaxed);
}

// ---- wake-latency histograms + stall snapshots (r12 wake/halt hunt) ----
//
// The avg/max phase table hides the slow-mode signature: rare multi-ms
// queue stalls (child_run max 64ms, wake_run max 10ms observed) that a
// mean dilutes and a max mis-scales. Log2 cycle histograms expose the
// tail mass; a stall snapshot (idle mask + per-cpu idle phase at rescue
// time) tells whether the stall was a lost-dispatch shape (task in the
// GRQ, both CPUs halted/idle-marked, IPI never sent) at the moment a
// tick finally rescued it.

const HIST_BUCKETS: usize = 24;
static HIST_CHILD: [AtomicU64; HIST_BUCKETS] = [ZERO_U64; HIST_BUCKETS];
static HIST_WAKE: [AtomicU64; HIST_BUCKETS] = [ZERO_U64; HIST_BUCKETS];

/// Stall snapshot ring: filled when a consumed stamp exceeded the
/// stall threshold; dumped in report().
struct StallSnap {
    delta_us: AtomicU64,
    /// 0 = child_run (fork-enqueue -> switch-in), 1 = wake_run.
    kind: AtomicU8,
    pid: AtomicU32,
    idle_mask: AtomicU32,
    nr_running: AtomicU64,
    /// Per-CPU WW_IDLE_PHASE at rescue (0..5; 4 = inside WFI).
    phase0: AtomicU8,
    phase1: AtomicU8,
    /// Per-CPU last-tick interrupted RIP at rescue: the spin site of
    /// whichever CPU was stuck (mid-benchmark reads only).
    rip0: AtomicU64,
    rip1: AtomicU64,
}
const SS0: StallSnap = StallSnap {
    delta_us: AtomicU64::new(0),
    kind: AtomicU8::new(255),
    pid: AtomicU32::new(0),
    idle_mask: AtomicU32::new(0),
    nr_running: AtomicU64::new(0),
    phase0: AtomicU8::new(255),
    phase1: AtomicU8::new(255),
    rip0: AtomicU64::new(0),
    rip1: AtomicU64::new(0),
};
static STALLS: [StallSnap; 12] = [SS0; 12];
static STALL_CUR: AtomicU64 = AtomicU64::new(0);
static STALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Snapshot the scheduler's idle state for the stall ring. Kept in
/// sched.rs (grq() is private there); returns (idle_mask, nr_running,
/// phase0, phase1) — cheap atomic reads, only called post-threshold.
#[inline]
fn sched_stall_snapshot() -> (u32, u64, u8, u8) {
    crate::sched::debug_idle_snapshot()
}

/// Account a stamp delta (cycles) into its histogram; capture a
/// snapshot when it exceeds the stall threshold (2 ms).
#[inline]
fn hist_account(kind: usize, delta_cyc: u64, pid: u32) {
    let b = (63 - delta_cyc.leading_zeros()).min((HIST_BUCKETS - 1) as u32) as usize;
    let h = if kind == 0 { &HIST_CHILD } else { &HIST_WAKE };
    h[b].fetch_add(1, Ordering::Relaxed);
    // Stall threshold: 2 ms in cycles (hz read per call is fine — this
    // is the rare path; the common path exits at the bucket add).
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0;
    let stall_cyc = if hz > 0 { hz / 500 } else { delta_cyc + 1 };
    if delta_cyc >= stall_cyc {
        let (idle_mask, nr_running, phase0, phase1) = sched_stall_snapshot();
        let i = (STALL_CUR.fetch_add(1, Ordering::Relaxed) % 12) as usize;
        let s = &STALLS[i];
        s.delta_us.store(delta_cyc / (hz / 1_000_000).max(1), Ordering::Relaxed);
        s.kind.store(kind as u8, Ordering::Relaxed);
        s.pid.store(pid, Ordering::Relaxed);
        s.idle_mask.store(idle_mask, Ordering::Relaxed);
        s.nr_running.store(nr_running, Ordering::Relaxed);
        s.phase0.store(phase0, Ordering::Relaxed);
        s.phase1.store(phase1, Ordering::Relaxed);
        s.rip0.store(TICK_RIP[0].load(Ordering::Relaxed), Ordering::Relaxed);
        s.rip1.store(TICK_RIP[1].load(Ordering::Relaxed), Ordering::Relaxed);
        STALL_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

// ---- tick-path GRQ sampler (r12): catch a stall IN PROGRESS ----
//
// The rescue-time snapshot (FSTALL) cannot see the during-stall state.
// This sampler fires every tick while armed, reads the GRQ under its
// lock (count + leftmost candidate's ownership qword + idle mask +
// phases), and keeps a 64-slot ring. Consecutive samples naming the
// same leftmost pid (>=2 ticks = >=8 ms stuck) are the during-stall
// truth the rescue snapshot can't provide.

const GS_BUDGET: usize = 64;
pub struct GrqSample {
    pub cfs_nr: AtomicU64,
    pub grq_nr: AtomicU64,
    pub lpid: AtomicU32,
    pub loncpu: AtomicU32,
    pub lclaim: AtomicU64, // i64 bit-cast
    pub lpk: AtomicU32,
    pub ltcpu: AtomicU32, // i32 bit-cast
    pub idle: AtomicU32,
    pub ph0: AtomicU32,
    pub ph1: AtomicU32,
}
const GS0: GrqSample = GrqSample {
    cfs_nr: AtomicU64::new(0),
    grq_nr: AtomicU64::new(0),
    lpid: AtomicU32::new(0),
    loncpu: AtomicU32::new(0),
    lclaim: AtomicU64::new(u64::MAX),
    lpk: AtomicU32::new(0),
    ltcpu: AtomicU32::new(u32::MAX),
    idle: AtomicU32::new(0),
    ph0: AtomicU32::new(255),
    ph1: AtomicU32::new(255),
};
static GSAMPLES: [GrqSample; GS_BUDGET] = [GS0; GS_BUDGET];
static GS_CUR: AtomicU64 = AtomicU64::new(0);

// ---- tick-gap detector (r12): quantify system-wide freezes ----
//
// The one-shot LAPIC re-arms every tick at <=4 ms, so a per-CPU tick
// gap beyond ~8 ms means THIS vCPU stopped taking timer interrupts:
// host-side descheduling or KVM-side blocking (guest spin cannot stop
// the timer — the IRQ would preempt the spinner). The gap event
// records the state at gap-close (the first tick after the silence).

const TG_BUDGET: usize = 32;
pub struct TickGap {
    /// Gap length in us.
    pub gap_us: AtomicU64,
    pub cpu: AtomicU32,
    pub idle: AtomicU32,
    pub ph0: AtomicU32,
    pub ph1: AtomicU32,
    pub cfs_nr: AtomicU64,
    pub lpid: AtomicU32,
    pub cur_pid: AtomicU32,
}
const TG0: TickGap = TickGap {
    gap_us: AtomicU64::new(0),
    cpu: AtomicU32::new(0),
    idle: AtomicU32::new(0),
    ph0: AtomicU32::new(255),
    ph1: AtomicU32::new(255),
    cfs_nr: AtomicU64::new(0),
    lpid: AtomicU32::new(0),
    cur_pid: AtomicU32::new(0),
};
static TGAPS: [TickGap; TG_BUDGET] = [TG0; TG_BUDGET];
static TG_CUR: AtomicU64 = AtomicU64::new(0);
static TG_TOTAL: AtomicU64 = AtomicU64::new(0);
static TG_SUM_US: AtomicU64 = AtomicU64::new(0);
static LAST_TICK_TSC: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// The interrupted-context RIP at each CPU's last tick (r12): the tick
/// preempts whatever the CPU was executing; a CPU stuck for many ticks
/// shows the SAME rip here every time — the spin site. Written from the
/// x86_64 timer tick when armed.
pub static TICK_RIP: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

#[inline]
pub fn note_tick_rip(cpu: usize, rip: u64) {
    if cpu < 4 {
        TICK_RIP[cpu].store(rip, Ordering::Relaxed);
    }
}

/// Called from scheduler_tick while forkprof is armed. Records one
/// sample; also detects streaks (same leftmost pid across >=2
/// consecutive samples) into a small streak table. Tick-path probe:
/// runs under full arming (2) and tick-only arming (3).
#[inline]
pub fn tick_grq_sample() {
    if !tick_on() {
        return;
    }
    tick_grq_sample_armed();
}

#[cold]
#[inline(never)]
fn tick_grq_sample_armed() {
    let cpu = crate::arch::cpu_id() as usize;
    // Tick-gap detection first: this tick vs this CPU's previous tick.
    let now = now();
    let last = LAST_TICK_TSC[cpu.min(3)].swap(now, Ordering::Relaxed);
    if last != 0 {
        #[cfg(feature = "x86_64")]
        let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
        #[cfg(not(feature = "x86_64"))]
        let hz = 0;
        let per_ms = hz / 1000;
        let gap_us = if per_ms > 0 { now.saturating_sub(last) / per_ms } else { 0 };
        // Normal tick: 4 ms (KERNEL_HZ=250). Report gaps beyond 8 ms.
        if gap_us > 8_000 {
            let (cfs_nr, _cl, _g, lpid, _o, _c, _p, _t, idle, ph0v, ph1v) =
                crate::sched::debug_grq_sample();
            let i = (TG_CUR.fetch_add(1, Ordering::Relaxed) % TG_BUDGET as u64) as usize;
            let g = &TGAPS[i];
            g.gap_us.store(gap_us, Ordering::Relaxed);
            g.cpu.store(cpu as u32, Ordering::Relaxed);
            g.idle.store(idle, Ordering::Relaxed);
            g.ph0.store(ph0v as u32, Ordering::Relaxed);
            g.ph1.store(ph1v as u32, Ordering::Relaxed);
            g.cfs_nr.store(cfs_nr, Ordering::Relaxed);
            g.lpid.store(lpid, Ordering::Relaxed);
            g.cur_pid.store(crate::sched::get_current_pid() as u32, Ordering::Relaxed);
            TG_TOTAL.fetch_add(1, Ordering::Relaxed);
            TG_SUM_US.fetch_add(gap_us, Ordering::Relaxed);
        }
    }
    let (cfs_nr, cfs_len, grq_nr, lpid, loncpu, lclaim, lpk, ltcpu, idle, ph0, ph1) =
        crate::sched::debug_grq_sample();
    // Count-vs-tree desync audit: counted nr_running != actual timeline
    // size is the freeze mechanism candidate — record it loud.
    if cfs_nr as usize != cfs_len {
        static DE_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        if DE_N.fetch_add(1, Ordering::Relaxed) < 16 {
            let mut buf: [u8; 96] = [0u8; 96];
            let h = b"DESYNC cfs-nr=";
            buf[..h.len()].copy_from_slice(h);
            let mut n = h.len();
            let v = cfs_nr;
            let mut d = [0u8; 24];
            let mut k = 0;
            let mut vv = v;
            if vv == 0 { d[k] = b'0'; k = 1; }
            while vv > 0 { d[k] = b'0' + (vv % 10) as u8; vv /= 10; k += 1; }
            d[..k].reverse();
            buf[n..n + k].copy_from_slice(&d[..k]);
            n += k;
            const M: &[u8] = b" len=";
            buf[n..n + M.len()].copy_from_slice(M);
            n += M.len();
            vv = cfs_len as u64;
            k = 0;
            if vv == 0 { d[k] = b'0'; k = 1; }
            while vv > 0 { d[k] = b'0' + (vv % 10) as u8; vv /= 10; k += 1; }
            d[..k].reverse();
            buf[n..n + k].copy_from_slice(&d[..k]);
            n += k;
            buf[n] = b'\n';
            crate::console::puts_bytes_no_lock(&buf[..=n]);
        }
    }
    let i = (GS_CUR.fetch_add(1, Ordering::Relaxed) % GS_BUDGET as u64) as usize;
    let s = &GSAMPLES[i];
    s.cfs_nr.store(cfs_nr, Ordering::Relaxed);
    s.grq_nr.store(grq_nr, Ordering::Relaxed);
    s.lpid.store(lpid, Ordering::Relaxed);
    s.loncpu.store(loncpu, Ordering::Relaxed);
    s.lclaim.store(lclaim as u64, Ordering::Relaxed);
    s.lpk.store(lpk, Ordering::Relaxed);
    s.ltcpu.store(ltcpu as u32, Ordering::Relaxed);
    s.idle.store(idle, Ordering::Relaxed);
    s.ph0.store(ph0 as u32, Ordering::Relaxed);
    s.ph1.store(ph1 as u32, Ordering::Relaxed);
}

#[inline]
pub fn peek(p: usize) -> (u64, u64, u64) {
    let s = &STATS[p];
    (
        s.count.load(Ordering::Relaxed),
        s.cyc.load(Ordering::Relaxed),
        s.max.load(Ordering::Relaxed),
    )
}

/// Print the phase table. Called once from the shutdown cascade.
pub fn report() {
    if !(on() || tick_on()) {
        return;
    }
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0; // cycle-domain report only on the other ports
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let names = [
        "fk_total",
        "fk_alloc",
        "fk_copythread",
        "fk_fdtable",
        "fk_mmfork",
        "fk_enqueue",
        "mm_snapshot",
        "mm_cow",
        "mm_replay",
        "ex_total",
        "ex_mmdrop",
        "drop_vmafree",
        "drop_rcu",
        "drop_ptfree",
        "ex_reparent",
        "ex_notify",
        "exec_total",
        "s_forks_vmas",
        "s_cow_ptes",
        "s_cow_tables",
        "s_mm_drops",
        "wt_body",
        "wt_sched",
        "wt_reap",
        "sched_notify",
        "wake_body",
        "pick",
        "ctxsw",
        "wake_run",
        "child_run",
        "ex_fddrop",
        "ex_premm",
        "ex_postf",
    ];
    crate::println!("FORKPROF BEGIN hz={}", hz);
    for (i, name) in names.iter().enumerate() {
        let (c, cyc, max) = peek(i);
        if c == 0 && cyc == 0 {
            continue;
        }
        if c == 0 {
            // scalar-only slot (bump()ed without add())
            crate::println!(
                "FORKPROF {} scalar={}",
                name,
                cyc
            );
            continue;
        }
        let avg = cyc / c.max(1) / per_us;
        let mx = max / per_us;
        // ns-resolution average (appended field): the integer-us avg
        // rounds whole sub-us phases (fk_fdtable, wake_body) to 0, and
        // the perf lane needs those numbers.
        let avgns = (cyc / c.max(1)) * 1000 / per_us.max(1);
        crate::println!(
            "FORKPROF {} n={} avg={}us max={}us avgns={}ns",
            name,
            c,
            avg,
            mx,
            avgns
        );
    }
    // Wake-latency histograms (log2 cycle buckets; edge printed in us).
    let names_h = ["child_run", "wake_run"];
    for (k, h) in [&HIST_CHILD, &HIST_WAKE].iter().enumerate() {
        let total: u64 = h.iter().map(|b| b.load(Ordering::Relaxed)).sum();
        if total == 0 {
            continue;
        }
        crate::println!("FHIST {} total={}", names_h[k], total);
        for (b, bucket) in h.iter().enumerate() {
            let c = bucket.load(Ordering::Relaxed);
            if c == 0 {
                continue;
            }
            let lo_us = if b == 0 { 0 } else { (1u64 << (b - 1)) / per_us.max(1) };
            let hi_us = (1u64 << b) / per_us.max(1);
            crate::println!("FHIST bucket{} lo={}us hi={}us n={}", b, lo_us, hi_us, c);
        }
    }
    // Stall snapshots.
    let sc = STALL_COUNT.load(Ordering::Relaxed);
    if sc > 0 {
        crate::println!("FSTALL total={}", sc);
        for i in 0..12 {
            let s = &STALLS[i];
            let d = s.delta_us.load(Ordering::Relaxed);
            if d == 0 {
                continue;
            }
            crate::println!(
                "FSTALL kind={} pid={} dt={}us idle={:#b} nrun={} ph0={} ph1={} rip0={:#x} rip1={:#x}",
                if s.kind.load(Ordering::Relaxed) == 0 { "child" } else { "wake" },
                s.pid.load(Ordering::Relaxed),
                d,
                s.idle_mask.load(Ordering::Relaxed),
                s.nr_running.load(Ordering::Relaxed),
                s.phase0.load(Ordering::Relaxed),
                s.phase1.load(Ordering::Relaxed),
                s.rip0.load(Ordering::Relaxed),
                s.rip1.load(Ordering::Relaxed),
            );
        }
    }
    // Tick-path GRQ samples: print non-idle-queue samples (a candidate
    // sitting in the tree across samples = the during-stall truth).
    let gs_total = GS_CUR.load(Ordering::Relaxed);
    if gs_total > 0 {
        crate::println!("FGSQ n={}", gs_total);
        let n = gs_total.min(GS_BUDGET as u64) as usize;
        for i in 0..n {
            let s = &GSAMPLES[i];
            let lpid = s.lpid.load(Ordering::Relaxed);
            let cfs = s.cfs_nr.load(Ordering::Relaxed);
            if lpid == 0 && cfs == 0 {
                continue;
            }
            crate::println!(
                "FGSQ cfs={} grq={} lpid={} oncpu={} claim={} pk={:#x} tcpu={} idle={:#b} ph={}/{} rip0={:#x} rip1={:#x}",
                cfs,
                s.grq_nr.load(Ordering::Relaxed),
                lpid,
                s.loncpu.load(Ordering::Relaxed),
                s.lclaim.load(Ordering::Relaxed) as i64,
                s.lpk.load(Ordering::Relaxed),
                s.ltcpu.load(Ordering::Relaxed) as i32,
                s.idle.load(Ordering::Relaxed),
                s.ph0.load(Ordering::Relaxed),
                s.ph1.load(Ordering::Relaxed),
                TICK_RIP[0].load(Ordering::Relaxed),
                TICK_RIP[1].load(Ordering::Relaxed),
            );
        }
    }
    // Tick-gap events: the freeze quantification.
    let tg_total = TG_TOTAL.load(Ordering::Relaxed);
    if tg_total > 0 {
        crate::println!(
            "FTICKGAP total={} sum_ms={}",
            tg_total,
            TG_SUM_US.load(Ordering::Relaxed) / 1000
        );
        let n = TG_CUR.load(Ordering::Relaxed).min(TG_BUDGET as u64) as usize;
        for i in 0..n {
            let g = &TGAPS[i];
            let d = g.gap_us.load(Ordering::Relaxed);
            if d == 0 {
                continue;
            }
            crate::println!(
                "FTICKGAP cpu={} gap={}us idle={:#b} ph={}/{} cfs={} lpid={} cur={}",
                g.cpu.load(Ordering::Relaxed),
                d,
                g.idle.load(Ordering::Relaxed),
                g.ph0.load(Ordering::Relaxed),
                g.ph1.load(Ordering::Relaxed),
                g.cfs_nr.load(Ordering::Relaxed),
                g.lpid.load(Ordering::Relaxed),
                g.cur_pid.load(Ordering::Relaxed),
            );
        }
    }
    crate::println!("FORKPROF END");
    report_trace();
}
