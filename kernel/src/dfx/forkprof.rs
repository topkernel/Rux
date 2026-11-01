//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! fork/exec/exit phase profiler — attribution probe for the
//! process-lifecycle perf lane (`feature/perf-fork`).
//!
//! Armed by the kernel cmdline `forkprof=1` (own key, syscost-style;
//! no dfx/switches.rs bit is consumed). Costs nothing when off: every
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
pub const P_COUNT: usize = 30;

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
}
const EV0: TEv = TEv {
    tsc: AtomicU64::new(0),
    site: AtomicU8::new(255),
    pid: AtomicU32::new(0),
};
static RING: [TEv; 4096] = [EV0; 4096];
static RING_CUR: AtomicU64 = AtomicU64::new(0);

/// Trace armed separately: `forkprof=2` (phases + trace); `forkprof=1`
/// is phases only.
#[inline]
pub fn trace_on() -> bool {
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
        let d = if prev == 0 { 0 } else { t.saturating_sub(prev) / per_us };
        prev = t;
        let nm = if (site as usize) < names.len() { names[site as usize] } else { "?" };
        crate::println!("FTRACE {} pid={} dt={}us", nm, pid, d);
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
#[inline]
pub fn on() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => {
                    match crate::cmdline::get_param("forkprof").as_deref() {
                        Some("1") | Some("2") => 2,
                        _ => 1,
                    }
                }
            };
            ARMED.store(v, Ordering::Relaxed);
            v == 2
        }
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
    if !on() || REPORTED.swap(true, Ordering::Relaxed) {
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
        crate::println!(
            "FORKPROF {} n={} avg={}us max={}us",
            name,
            c,
            avg,
            mx
        );
    }
    crate::println!("FORKPROF END");
    report_trace();
}
