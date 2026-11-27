//! deferred-reap drain family (tick drainer via the api/sched.rs row).
//!
//! Shrink stage (user method 2026-11-26; whole-subsystem move, user order
//! 2026-11-27): moved from the core's sched/hotpath.rs — zero samples in the
//! p23-hotmap union (docs/development/core-hotmap.md) and no staying core
//! caller. crate-level forbid(unsafe_code) applies.

#![forbid(unsafe_code)]
use crate::config::MAX_CPUS;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use crate::arch;
use crate::config::{CFS_LATENCY_NS, CFS_MIN_GRANULARITY_NS, KERNEL_HZ, MAX_TASKS};
use crate::errno;
use crate::list::ListHead;
use crate::println;
use crate::process::{types::Pid, types::SchedPolicy, types::Task, types::TaskState};
use crate::sync::types::{RawSpinlock, Spinlock};
use crate::trace;

#[allow(unused_imports)]
use crate::sched::hotpath::{PENDING, cpu_state, current, defer_exit_notify, switching_out_names};
#[allow(unused_imports)]
use crate::sched::types::{PendingReap, ReapClaim};

pub fn reap_drain_tick() {
    // Fast path: nothing pending AND no open grace batch.
    let pend_empty = PENDING
        .try_lock()
        .map(|p| p.is_empty())
        .unwrap_or(true);
    let batch_empty = GRACE_BATCH
        .try_lock()
        .map(|b| b.is_empty())
        .unwrap_or(true);
    if pend_empty && batch_empty {
        return;
    }
    if DRAIN_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // Adaptive budget (the backpressure that self-heals cap pressure):
    // the nominal 8 grows with pending depth so a full list drains at
    // 4x-8x rate instead of spilling to the fallback.
    let depth = PENDING.try_lock().map(|p| p.len()).unwrap_or(0);
    let budget = if depth > (PENDING_CAP * 3) / 4 {
        DRAIN_BUDGET * 8
    } else if depth > PENDING_CAP / 2 {
        DRAIN_BUDGET * 4
    } else {
        DRAIN_BUDGET
    };
    // Snapshot-and-clear of the switch-stage candidates.
    let mut batch: Vec<PendingReap> = {
        let mut p = PENDING.lock();
        let n = p.len().min(budget);
        let taken: Vec<PendingReap> = p.drain(..n).collect();
        taken
    };

    let mut keep: Vec<PendingReap> = Vec::new();
    for mut e in batch.drain(..) {
        if e.grace {
            // Re-queued grace entry (survivor of a prior run).
            keep.push(e);
            continue;
        }
        let (done, via_r85) = switch_stage_done(&e.claim);
        if !done {
            keep.push(e);
            continue;
        }
        if via_r85 {
            e.r85_ticks += 1;
            if e.r85_ticks < SUSTAINED_FREE_TICKS {
                keep.push(e);
                continue;
            }
            let n = R85_FREED.fetch_add(1, Ordering::Relaxed);
            if n < 32 {
                crate::debug::hotpath::taskdump_raw_line(b"REAP-R85-FREE #");
                crate::debug::hotpath::taskdump_dec(n as u64);
                crate::debug::hotpath::taskdump_raw_line(b" pid=");
                crate::debug::hotpath::taskdump_dec(e.claim.task().pid() as u64);
                crate::debug::hotpath::taskdump_raw_line(b"\n");
            }
        }
        e.grace = true;
        keep.push(e);
    }

    // Put the not-yet-grace survivors back.
    if !keep.is_empty() {
        let mut p = PENDING.lock();
        for mut e in keep.drain(..) {
            if !e.grace && p.len() < PENDING_CAP {
                p.push(e);
            } else {
                // Grace-marked entries join the persistent batch below;
                // the only drop here is a non-grace over-cap re-park
                // (the loud leak, counted).
                if !e.grace {
                    CLAIM_DROPPED.fetch_add(1, Ordering::Relaxed);
                } else {
                    let mut gb = GRACE_BATCH.lock();
                    gb.push(e);
                }
            }
        }
    }

    // ---- The grace stage (persistent batch, held-waiter protocol,
    // per-entry grace bound) ----
    // Merge this run's graduates into the batch; hold the waiter count
    // while the batch is non-empty (rcu_note_context_switch suppresses
    // QS stores otherwise — the boot-42 stall root cause); keep ONE
    // round handle open so generations advance (reports only reach a
    // generation someone opened); free the entries whose per-entry
    // `needs` the grace WATERMARK covers (P0: the old whole-batch
    // "all reported >= target" free could free an entry whose
    // pid_hash removal POSTDATED the target — see PendingReap.needs);
    // on an empty batch, close the round and release GP_IN_FLIGHT
    // (the boot-63 latch root cause).
    let cur_target = GRACE_TARGET.load(Ordering::Acquire);
    let target = if cur_target == 0 {
        let t = crate::sync::hotpath::shared_grace_target_pub();
        GRACE_TARGET.store(t, Ordering::Release);
        if GRACE_WAITER_HELD
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            crate::sync::hotpath::grace_waiter_hold();
        }
        GRACE_STALL_RUNS.store(0, Ordering::Release);
        t
    } else {
        cur_target
    };

    // The per-entry completion: the watermark is the largest
    // generation every online CPU has reported at/after — entries
    // whose needs it covers are safe to free NOW; the rest wait for
    // the next round (the round handle advances generations until it
    // passes the oldest outstanding needs).
    let watermark = crate::sync::hotpath::rcu_grace_watermark();
    {
        let mut gb = GRACE_BATCH.lock();
        let mut i = 0;
        while i < gb.len() {
            if gb[i].needs <= watermark {
                let e = gb.swap_remove(i);
                let (pp, cp) = e.claim.into_free();
                notify_parent_if_any(pp, cp);
            } else {
                i += 1;
            }
        }
    }
    let batch_empty = GRACE_BATCH.try_lock().map(|b| b.is_empty()).unwrap_or(false);

    if batch_empty || crate::sync::hotpath::all_online_cpus_reported_pub(target) {
        // Close the round handle so the next tick opens a FRESH
        // generation for any newer needs — never reuse a target that
        // predates outstanding removals. Releasing the shared latch on
        // an incomplete round is sound: GP_IN_FLIGHT only arbitrates
        // who OPENS the next round; a waiter that latched this target
        // polls all-reported(target) on its own, and reports at later
        // generations still satisfy it.
        crate::sync::hotpath::shared_grace_done_pub();
        GRACE_TARGET.store(0, Ordering::Release);
        GRACE_STALL_RUNS.store(0, Ordering::Release);
        if batch_empty
            && GRACE_WAITER_HELD
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            crate::sync::hotpath::grace_waiter_release();
        }
    } else {
        // The drainer's own tick context is outside every RCU
        // read-side section: report THIS cpu's quiescent state at the
        // current generation (boot-118: the surviving wedge's lagging
        // cpu was one that reported via switches alone, and long
        // user-mode stretches between switches stalled the round while
        // generations climbed around it).
        crate::sync::hotpath::report_self_quiescent();
        // Stall watch: expedite by poking the lagging CPUs (the
        // reference's synchronize_rcu_expedited shape — a resched IPI
        // on a kernel-preempt host forces the IRQ-exit switch that
        // stores the QS report the round is waiting on).
        let runs = GRACE_STALL_RUNS.fetch_add(1, Ordering::Relaxed) + 1;
        if runs >= GRACE_NUDGE_RUNS {
            let mut laggards = crate::sync::hotpath::grace_lagging_cpus(target);
            let me = crate::arch::cpu_id() as usize;
            // Self-only stall (the drainer's own CPU between context
            // switches — the p4 storm's shape: the tick that opened
            // the target is always one QS behind its own bump until
            // its task next switches) is NORMAL and resolves at that
            // switch: no IPI, no print. Only a STALLED PEER gets the
            // expedited nudge.
            laggards.retain(|&c| c != me);
            if !laggards.is_empty() {
                let mut names = alloc::string::String::new();
                use core::fmt::Write as _;
                for cpu in laggards {
                    crate::arch::ipi::send_reschedule_ipi(cpu);
                    let _ = write!(names, "{} ", cpu);
                }
                static NUDGE_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = NUDGE_N.fetch_add(1, Ordering::Relaxed);
                if n < 32 {
                    crate::debug::hotpath::taskdump_raw_line(b"REAP-GP-NUDGE #");
                    crate::debug::hotpath::taskdump_dec(n as u64);
                    crate::debug::hotpath::taskdump_raw_line(b" gen=");
                    crate::debug::hotpath::taskdump_dec(target as u64);
                    crate::debug::hotpath::taskdump_raw_line(b" laggards=");
                    for b in names.as_bytes() {
                        crate::debug::hotpath::taskdump_raw_line(&[*b]);
                    }
                    crate::debug::hotpath::taskdump_raw_line(b"\n");
                }
            }
            // Re-arm: one nudge burst per GRACE_NUDGE_RUNS window.
            GRACE_STALL_RUNS.store(0, Ordering::Release);
        }
    }

    DRAIN_ACTIVE.store(false, Ordering::Release);
}

pub fn defer_reap_storage(task: *mut Task, parent_pid: u32) -> bool {
    // The mint: the obligation leaves the reaper's hands here. On
    // over-cap return the OBLIGATION stays with the caller (the raw
    // fallback frees it) — the claim is not minted.
    let claim = match ReapClaim::mint(task) {
        Some(c) => c,
        None => return false,
    };
    let mut p = PENDING.lock();
    if p.len() >= PENDING_CAP {
        // Un-mint: hand the obligation back to the caller by
        // returning false; the claim must not drop (that would leak
        // what the fallback is about to free). into_free is the only
        // exit — so burn it here into the raw form the fallback
        // expects. (The mint/burn pair at the cap boundary is the
        // one place the raw form resurfaces, by design.)
        core::mem::forget(claim);
        return false;
    }
    p.push(PendingReap {
        claim,
        parent_pid,
        r85_ticks: 0,
        grace: false,
        needs: crate::sync::hotpath::rcu_gen_now().wrapping_add(1),
    });
    true
}

fn switch_stage_done(claim: &ReapClaim) -> (bool, bool) {
    let task = claim.as_ptr();
    let t = claim.task();
    if !t.state().is_dead() || t.on_cpu() {
        return (false, false);
    }
    if crate::sched::hotpath::switching_out_names(task) {
        return (false, false);
    }
    if t
        .terminal_switch_done
        .load(core::sync::atomic::Ordering::Acquire)
    {
        return (true, false);
    }
    // boot-63 residue: the PACING gate — a task still executing its
    // terminal deschedule loop publishes positive knowledge
    // (terminal_pacing, set at loop entry, cleared only at the real
    // retire sites). Its abort-retry windows can hold the whole
    // slotless+unmarked+!on_cpu conjunction while the page must not be
    // freed; the 20-tick timing bet is no longer the discriminator.
    if t
        .terminal_pacing
        .load(core::sync::atomic::Ordering::Acquire)
    {
        return (false, false);
    }
    // R85 physical truth: no CPU slot names this task. A task still
    // executing its terminal loop is tp on SOME hart and the R55
    // slot resync re-derives it — slotless means the frame is gone.
    let mut named_by_slot = false;
    for c in 0..crate::config::MAX_CPUS {
        if crate::sched::hotpath::cpu_state(c).current == task {
            named_by_slot = true;
            break;
        }
    }
    (!named_by_slot, true)
}

fn notify_parent_if_any(parent_pid: u32, child_pid: u32) {
    if parent_pid != 0 && parent_pid != child_pid {
        crate::sched::hotpath::defer_exit_notify(parent_pid, child_pid);
    }
}

const DRAIN_BUDGET: usize = 8;

const SUSTAINED_FREE_TICKS: u32 = 20;

const PENDING_CAP: usize = 1024;

static CLAIM_DROPPED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

static DRAIN_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

static GRACE_BATCH: crate::sync::types::Spinlock<Vec<PendingReap>> =
    crate::sync::types::Spinlock::new(Vec::new());

static GRACE_TARGET: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

static GRACE_STALL_RUNS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

static GRACE_WAITER_HELD: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

const GRACE_NUDGE_RUNS: u32 = 3;

static R85_FREED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
