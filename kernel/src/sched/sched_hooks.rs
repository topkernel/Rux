//! The SCHED HOOK TABLE (crate-flip S1b): every lib-side consumer of
//! scheduling-policy internals resolves through this table (the
//! NetHooks pattern) — nullable fn rows, EMPTY defaults no-op until
//! the sched crate's register_all runs (the composition bin,
//! before the framework impl). The row list is the measured reverse
//! surface (run/s1b-hook-rows.json + the counter getters); the entity
//! TYPES live framework-side in sched/entities.rs.
//!
//! DEVIRT LANE (2026-11-23): the rows whose bodies are PLAIN
//! OBSERVABILITY STATE (per-CPU phase stamps, pick tracer tables, the
//! pick-generation ledger, the schedstat/wake-census counters, the
//! walk-census counters) left the table — their storage lives in
//! sched/entities.rs and framework callers reach it with direct calls
//! (see sched/sched.rs; the sched census code reads the same
//! statics through the crate::sched re-exports). Measured reason: the
//! debug-profile kernel does not inline the wrapper layer, and the
//! per-switch diagnostic traffic (pick_trace/pick_gen/ww_record/phase
//! stamps — 20+ rows per ping-pong round) paid wrapper + OnceSlot
//! read + indirect call for what is a single atomic op, which the
//! p2_ctxsw gate row measured at +24% vs same-window main. What
//! remains here is POLICY (pick/enqueue/wake-placement decisions) and
//! armed-gated diagnostics whose wrappers check their own arm flag
//! (framework-side) before dispatching.
//!
//! NO-SELF-RECURSION RULE (unchanged): a row's registered impl must
//! never call the wrapper of the SAME row (wrapper -> registered impl
//! -> wrapper...). The gated wrappers below consult only
//! framework-native flags, never hook rows, so they cannot recurse.

use crate::process::task::{Task, SchedPolicy};
use super::class::SchedClass;
use super::sched::{GlobalRunQueue, GrqPendingReport};
use super::entities::W2RefusalCensus;
use crate::config::MAX_CPUS;

// ---- The data contract (S1b): shared event codes / slot tags ----

// Constants the core and the policy both name live HERE (framework):
// the seam's vocabulary, like FileOps' flag bits.

pub const CLASS_DL_BIT: u32 = 1;
pub const CLASS_RT_BIT: u32 = 2;
pub const CLASS_CFS_BIT: u32 = 4;
pub const SLOT_W_SCHED_PICK: u32 = 1;
pub const SLOT_W_R56D: u32 = 2;
pub const SLOT_W_R56C: u32 = 3;
pub const SLOT_W_STEAL: u32 = 4;
pub const SLOT_W_ABORT_REPUB: u32 = 5;
pub const WW_EV_WAKE_OK: u32 = 1;
pub const WW_EV_LINK: u32 = 2;
pub const WW_EV_UNLINK_PICK: u32 = 3;
pub const WW_EV_UNLINK_SCHED: u32 = 4;
pub const WW_EV_UNLINK_DEQ: u32 = 5;
pub const WW_EV_UNLINK_POISON: u32 = 6;
pub const WW_EV_PICK_PUB: u32 = 7;
pub const WW_EV_WAKE_REFUSED: u32 = 8;
pub const WW_EV_AFFIRM: u32 = 9;

pub const EV_ENQ_POISONED: u32 = 1; // "ENQ-POISONED-TASK"
pub const EV_ENQ_DEAD: u32 = 2; // "ENQ-DEAD-TASK"
pub const EV_DL_ENQ_DEFERRED: u32 = 3; // "DL-ENQ-DEFERRED"
pub const EV_R119_STALE_LIVE: u32 = 4; // "R119-STALE-LIVE-MARK-CLEARED"
pub const EV_R49_ONCPU_ORPHAN: u32 = 5; // "R49-ONCPU-ORPHAN healed"
pub const EV_R80_LIVE_PHANTOM: u32 = 6; // "R80-LIVE-PHANTOM-CLEARED"
pub const EV_R46_RUNNING_PHANTOM: u32 = 7; // "R46-RUNNING-PHANTOM healed"
pub const EV_W2_ENQ_HEAL_REFUSED: u32 = 8; // "W2-ENQUEUE-HEAL-REFUSED"
pub const EV_LP_WAKE_OUTCOME: u32 = 9; // "LP-WAKE-OUTCOME"
pub const EV_LP_WAKE_SKIP_EMPTY: u32 = 10; // "LP-WAKE-SKIP-EMPTY"
pub const EV_FREED_WHILE_LINKED: u32 = 11; // "FREED-WHILE-LINKED-UNLINKED"
pub const EV_NR_RUNNING_DESYNC: u32 = 12; // "NR-RUNNING-DESYNC"
pub const EV_R56_STEAL: u32 = 13; // "R56-STEAL"
pub const EV_R56_SLOT_CAPTURE: u32 = 14; // "R56-SLOT-CAPTURE-HEALED"
pub const EV_R56_QUEUED_MARK: u32 = 15; // "R56-QUEUED-MARK-CLEARED"
pub const EV_R80_LIVE_MARK: u32 = 16; // "R80-LIVE-MARK-CLEARED"
pub const EV_RQD_MIGRATE: u32 = 17; // "RQD-MIGRATE-DISPATCH"
pub const EV_RT_THROTTLED: u32 = 18; // "RT-CLASS-THROTTLED"

/// The hook set: policy fns the core and the wider kernel call.
#[derive(Clone, Copy)]
pub struct SchedHooks {
    pub r56_print_pid: fn(prefix: &[u8], pid: u32, suffix: &[u8]),
    pub sched_clock: fn() -> u64,
    pub change_task_policy: fn(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32),
    pub ww_record: fn(task: *mut Task, code: u32),
    pub orphan_watch_confirm: fn(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool,
    pub yield_cpu: fn(),
    pub enqueue_task_locked: fn(grq: &mut GlobalRunQueue, task: &mut Task) -> bool,
    pub ww_watch_add: fn(task: *mut Task),
    pub abort_next_release_orphans: fn(cpu: usize, next: *mut Task),
    pub r56_report_switch_abort: fn(cpu: usize, pid: u32),
    pub sched_renice_locked: fn(task: &mut Task, niceval: i32),
    pub psledger_consume: fn(task: *mut Task) -> bool,
    pub steer_task_cpu: fn(task: *mut Task, target: i32),
    pub sched_can_stop_tick: fn() -> bool,
    pub dl_throttle: fn(task: &mut Task),
    pub ensure_linked_locked: fn(grq: &mut GlobalRunQueue, task: &mut Task) -> bool,
    pub check_cfs_preempt: fn(task: *mut Task, cpus_allowed: u32),
    pub check_rt_preempt: fn(task: *mut Task, cpus_allowed: u32, prefer: i32),
    pub check_dl_preempt: fn(task: *mut Task, cpus_allowed: u32, prefer: i32),
    pub orphan_watch_clear: fn(task: *mut Task),
    pub pick_giveups: fn() -> (u64, u64),
    pub ww_slot_mismatch_counters: fn() -> [u64; MAX_CPUS],
    pub ww_sched_snapshot: fn() -> [u64; 10],
    pub ww_stalled_cpu_state: fn(cpu: usize) -> (u64, u64, u64),
    pub debug_claim_inject: fn() -> usize,
    pub debug_oncpu_inject: fn() -> usize,
    pub ww_heals: fn() -> (u64, u64),
    pub ww_subgate_counts: fn() -> (u64, u64, u64),
    pub ww_rt_pick_stats: fn() -> (u64, u64, u64),
    pub heartbeat: fn(),
    pub stallwatch_tick: fn(),
    pub preempt_check_wake: fn(task: *mut Task),
    pub preempt_check_wake_sync: fn(task: *mut Task),
    pub w2_refusal_census: fn(task: *mut Task) -> W2RefusalCensus,
    pub task_sched_class: fn(task: &Task) -> &'static dyn SchedClass,
    pub ww_note_slot_mismatch: fn(cpu: usize),
    pub monopolist_census: fn(now_j: u64),
    pub runq_strand_census: fn(now_j: u64),
    pub ffrt_wait_census: fn(),
    pub rt_wedge_dump_tick: fn(cpu: usize),
    pub rt_bandwidth_tick: fn(cpu: usize, current: *mut Task),
    pub emit_grq_reports: fn(reports: &[GrqPendingReport]),
    pub pick_next_task: fn(grq: &mut GlobalRunQueue, cpu_id: usize, prev: *mut Task) -> *mut Task,
    pub requeue_prev_locked: fn(grq: &mut GlobalRunQueue, prev: &mut Task),
    pub g_diag_idle_bit: fn(cpu: usize) -> bool,
    pub cpu_tick_stale_ms: fn(cpu: usize) -> u64,
    pub lp_idle_lie_report: fn(cpu: usize),
    pub sched_slice_to_ms: fn(slice_ns: u64) -> u32,
    pub sched_ev_drain: fn(budget: usize) -> usize,
    pub note_task_freed: fn(task: *mut crate::process::Task),
    pub report_poisoned_rq_entry: fn(cpu: usize, raw: usize),
    pub psledger_task: fn(cpu: usize) -> usize,
    pub psledger_cpu: fn(cpu: usize) -> u32,
    pub psledger_jif: fn(cpu: usize) -> u64,
    pub psledger_pk: fn(cpu: usize) -> u32,
    pub ww_watch: fn(cpu: usize) -> usize,
    pub ww_watch_slots: fn() -> alloc::vec::Vec<usize>,
    pub load_balance: fn(),
    pub psledger_note_publish: fn(task: *mut Task, cpu: usize, pk: u32),
    pub psledger_peek: fn(task: *mut Task) -> Option<(u32, u32, u64)>,
    pub sched_hist_ring: fn(task: &Task, kind: u64, why: u64, aux: u64),
    pub sched_hist_refuse: fn(task: *mut Task, skip_reason: u32, cpu: usize),
    pub sched_hist_link: fn(task: *mut Task),
    pub sched_hist_pick: fn(task: *mut Task, cpu: usize),
    pub ww_cfs_geometry: fn(task: *mut Task) -> (u64, u64, u64),
    pub ww_cfs_tie_mass: fn(task: *mut Task) -> u64,
    pub ww_cfs_tie_mass_vr: fn(vr: u64) -> u64,
    pub ww_tail_fill: fn(ix: usize) -> u64,
    pub ww_append_dump: fn(out: &mut dyn core::fmt::Write, task: *mut Task, now: u64),
    pub fair_sched_class: fn() -> &'static dyn SchedClass,
    pub rt_sched_class: fn() -> &'static dyn SchedClass,
    pub dl_sched_class: fn() -> &'static dyn SchedClass,
    pub idle_sched_class: fn() -> &'static dyn SchedClass,
    pub stop_sched_class: fn() -> &'static dyn SchedClass,

    pub sched_ev: fn(tag: u32, pid: u32, a: u64, b: u64),
    pub vr_canary: fn(rq: &mut crate::sched::entities::CfsRunQueue, task: *mut Task, site: &[u8; 3], vr: u64),
    pub stale_oncpu_healed: fn(tag: &[u8], cpu_id: usize, task: *mut Task, claim: i32, pk: u32),


}

impl SchedHooks {
    pub const EMPTY: SchedHooks = SchedHooks {
        r56_print_pid: |prefix, pid, suffix| Default::default(),
        sched_clock: || crate::sched::entities::sched_clock(),
        change_task_policy: |task, new_policy, rt_prio| Default::default(),
        ww_record: |task, code| Default::default(),
        orphan_watch_confirm: |task, pid, now, threshold| Default::default(),
        yield_cpu: || Default::default(),
        enqueue_task_locked: |grq, task| Default::default(),
        ww_watch_add: |task| Default::default(),
        abort_next_release_orphans: |cpu, next| Default::default(),
        r56_report_switch_abort: |cpu, pid| Default::default(),
        sched_renice_locked: |task, niceval| Default::default(),
        psledger_consume: |task| Default::default(),
        steer_task_cpu: |task, target| Default::default(),
        sched_can_stop_tick: || Default::default(),
        dl_throttle: |task| Default::default(),
        ensure_linked_locked: |grq, task| Default::default(),
        check_cfs_preempt: |task, cpus_allowed| Default::default(),
        check_rt_preempt: |task, cpus_allowed, prefer| Default::default(),
        check_dl_preempt: |task, cpus_allowed, prefer| Default::default(),
        orphan_watch_clear: |task| Default::default(),
        pick_giveups: || Default::default(),
        ww_slot_mismatch_counters: || Default::default(),
        ww_sched_snapshot: || Default::default(),
        ww_stalled_cpu_state: |cpu| Default::default(),
        debug_claim_inject: || Default::default(),
        debug_oncpu_inject: || Default::default(),
        ww_heals: || Default::default(),
        ww_subgate_counts: || Default::default(),
        ww_rt_pick_stats: || Default::default(),
        heartbeat: || Default::default(),
        stallwatch_tick: || Default::default(),
        preempt_check_wake: |task| Default::default(),
        preempt_check_wake_sync: |task| Default::default(),
        w2_refusal_census: |task| Default::default(),
        task_sched_class: |task| panic!("SchedHooks: task_sched_class used before registration"),
        ww_note_slot_mismatch: |cpu| Default::default(),
        monopolist_census: |now_j| Default::default(),
        runq_strand_census: |now_j| Default::default(),
        ffrt_wait_census: || Default::default(),
        rt_wedge_dump_tick: |cpu| Default::default(),
        rt_bandwidth_tick: |cpu, current| Default::default(),
        emit_grq_reports: |reports| Default::default(),
        pick_next_task: |grq, cpu_id, prev| Default::default(),
        requeue_prev_locked: |grq, prev| Default::default(),
        g_diag_idle_bit: |cpu| Default::default(),
        cpu_tick_stale_ms: |cpu| Default::default(),
        lp_idle_lie_report: |cpu| Default::default(),
        sched_slice_to_ms: |slice_ns| crate::sched::entities::sched_slice_to_ms(slice_ns),
        sched_ev_drain: |budget| Default::default(),
        note_task_freed: |task| Default::default(),
        report_poisoned_rq_entry: |cpu, raw| Default::default(),
        psledger_task: |_| Default::default(),
        psledger_cpu: |_| Default::default(),
        psledger_jif: |_| Default::default(),
        psledger_pk: |_| Default::default(),
        ww_watch: |_| Default::default(),
        ww_watch_slots: || alloc::vec::Vec::new(),
        load_balance: || Default::default(),
        psledger_note_publish: |task, cpu, pk| Default::default(),
        psledger_peek: |task| Default::default(),
        sched_hist_ring: |task, kind, why, aux| Default::default(),
        sched_hist_refuse: |task, skip_reason, cpu| Default::default(),
        sched_hist_link: |task| Default::default(),
        sched_hist_pick: |task, cpu| Default::default(),
        ww_cfs_geometry: |task| Default::default(),
        ww_cfs_tie_mass: |task| Default::default(),
        ww_cfs_tie_mass_vr: |vr| Default::default(),
        ww_tail_fill: |ix| Default::default(),
        ww_append_dump: |out, task, now| Default::default(),
        fair_sched_class: || panic!("SchedHooks: fair_sched_class before registration"),
        rt_sched_class: || panic!("SchedHooks: rt_sched_class before registration"),
        dl_sched_class: || panic!("SchedHooks: dl_sched_class before registration"),
        idle_sched_class: || panic!("SchedHooks: idle_sched_class before registration"),
        stop_sched_class: || panic!("SchedHooks: stop_sched_class before registration"),
        sched_ev: |_,_,_,_| {},
        vr_canary: |_,_,_,_| {},
        stale_oncpu_healed: |_,_,_,_,_| {},
    };
}

pub static SCHED_HOOKS: crate::safety::slot::OnceSlot<SchedHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (the policy crate at boot).
pub fn sched_register_hooks(hooks: SchedHooks) {
    let _ = SCHED_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static SchedHooks {
    // Borrow, do not copy: SchedHooks is 111 fn pointers (~888 B); the
    // by-value copy put that whole table on the stack on EVERY wrapper
    // call (5-15 per context switch), which the p2_ctxsw gate row
    // measured directly (2.19 vs 1.24 us med). Read the one field
    // through the shared reference instead.
    match SCHED_HOOKS.get() {
        Some(h) => h,
        None => &SchedHooks::EMPTY,
    }
}

// The lib-side call surface.

#[inline]
pub fn r56_print_pid(prefix: &[u8], pid: u32, suffix: &[u8]) { (hooks().r56_print_pid)(prefix, pid, suffix) }

#[inline]
pub fn sched_clock() -> u64 { (hooks().sched_clock)() }

#[inline]
pub fn change_task_policy(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32) { (hooks().change_task_policy)(task, new_policy, rt_prio) }

/// Gated wrapper (devirt lane): the policy body's first act is the
/// schedtrace arm check — a disarmed boot (every standard boot) records
/// nothing, so the wrapper checks the same FRAMEWORK-side flag and
/// skips the seam dispatch entirely. Armed boots record exactly as
/// before. This site fires 2-4 times per context switch (PICK_PUB,
/// AFFIRM, WAKE_OK, UNLINK_*), which is why it is gated rather than
/// merely borrowed.
#[inline]
pub fn ww_record(task: *mut Task, code: u32) {
    if !crate::trace::EVENT_SCHEDTRACE.armed() {
        return;
    }
    (hooks().ww_record)(task, code)
}

#[inline]
pub fn orphan_watch_confirm(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool { (hooks().orphan_watch_confirm)(task, pid, now, threshold) }

#[inline]
pub fn yield_cpu() { (hooks().yield_cpu)() }

#[inline]
pub fn enqueue_task_locked(grq: &mut GlobalRunQueue, task: &mut Task) -> bool { (hooks().enqueue_task_locked)(grq, task) }

#[inline]
pub fn ww_watch_add(task: *mut Task) { (hooks().ww_watch_add)(task) }

#[inline]
pub fn abort_next_release_orphans(cpu: usize, next: *mut Task) { (hooks().abort_next_release_orphans)(cpu, next) }

#[inline]
pub fn r56_report_switch_abort(cpu: usize, pid: u32) { (hooks().r56_report_switch_abort)(cpu, pid) }

#[inline]
pub fn sched_renice_locked(task: &mut Task, niceval: i32) { (hooks().sched_renice_locked)(task, niceval) }

/// Gated wrapper (devirt lane): the policy body returns false
/// immediately when leakhunt is disarmed; the wrapper checks the same
/// FRAMEWORK-side flag first. Fires once per switch-in (the affirm
/// path's ledger consume).
#[inline]
pub fn psledger_consume(task: *mut Task) -> bool {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return false;
    }
    (hooks().psledger_consume)(task)
}

#[inline]
pub fn steer_task_cpu(task: *mut Task, target: i32) { (hooks().steer_task_cpu)(task, target) }

#[inline]
pub fn sched_can_stop_tick() -> bool { (hooks().sched_can_stop_tick)() }

#[inline]
pub fn dl_throttle(task: &mut Task) { (hooks().dl_throttle)(task) }

#[inline]
pub fn ensure_linked_locked(grq: &mut GlobalRunQueue, task: &mut Task) -> bool { (hooks().ensure_linked_locked)(grq, task) }

#[inline]
pub fn check_cfs_preempt(task: *mut Task, cpus_allowed: u32) { (hooks().check_cfs_preempt)(task, cpus_allowed) }

#[inline]
pub fn check_rt_preempt(task: *mut Task, cpus_allowed: u32, prefer: i32) { (hooks().check_rt_preempt)(task, cpus_allowed, prefer) }

#[inline]
pub fn check_dl_preempt(task: *mut Task, cpus_allowed: u32, prefer: i32) { (hooks().check_dl_preempt)(task, cpus_allowed, prefer) }

#[inline]
pub fn orphan_watch_clear(task: *mut Task) { (hooks().orphan_watch_clear)(task) }

#[inline]
pub fn pick_giveups() -> (u64, u64) { (hooks().pick_giveups)() }

#[inline]
pub fn ww_slot_mismatch_counters() -> [u64; MAX_CPUS] { (hooks().ww_slot_mismatch_counters)() }

#[inline]
pub fn ww_sched_snapshot() -> [u64; 10] { (hooks().ww_sched_snapshot)() }

#[inline]
pub fn ww_stalled_cpu_state(cpu: usize) -> (u64, u64, u64) { (hooks().ww_stalled_cpu_state)(cpu) }

#[inline]
pub fn debug_claim_inject() -> usize { (hooks().debug_claim_inject)() }

#[inline]
pub fn debug_oncpu_inject() -> usize { (hooks().debug_oncpu_inject)() }

#[inline]
pub fn ww_heals() -> (u64, u64) { (hooks().ww_heals)() }

#[inline]
pub fn ww_subgate_counts() -> (u64, u64, u64) { (hooks().ww_subgate_counts)() }

#[inline]
pub fn ww_rt_pick_stats() -> (u64, u64, u64) { (hooks().ww_rt_pick_stats)() }

#[inline]
pub fn heartbeat() { (hooks().heartbeat)() }

#[inline]
pub fn stallwatch_tick() { (hooks().stallwatch_tick)() }

#[inline]
pub fn preempt_check_wake(task: *mut Task) { (hooks().preempt_check_wake)(task) }

#[inline]
pub fn preempt_check_wake_sync(task: *mut Task) { (hooks().preempt_check_wake_sync)(task) }

#[inline]
pub fn w2_refusal_census(task: *mut Task) -> W2RefusalCensus { (hooks().w2_refusal_census)(task) }

#[inline]
pub fn task_sched_class(task: &Task) -> &'static dyn SchedClass { (hooks().task_sched_class)(task) }

#[inline]
pub fn ww_note_slot_mismatch(cpu: usize) { (hooks().ww_note_slot_mismatch)(cpu) }

#[inline]
pub fn monopolist_census(now_j: u64) { (hooks().monopolist_census)(now_j) }

#[inline]
pub fn runq_strand_census(now_j: u64) { (hooks().runq_strand_census)(now_j) }

#[inline]
pub fn ffrt_wait_census() { (hooks().ffrt_wait_census)() }

#[inline]
pub fn rt_wedge_dump_tick(cpu: usize) { (hooks().rt_wedge_dump_tick)(cpu) }

#[inline]
pub fn rt_bandwidth_tick(cpu: usize, current: *mut Task) { (hooks().rt_bandwidth_tick)(cpu, current) }

#[inline]
pub fn emit_grq_reports(reports: &[GrqPendingReport]) { (hooks().emit_grq_reports)(reports) }

#[inline]
pub fn pick_next_task(grq: &mut GlobalRunQueue, cpu_id: usize, prev: *mut Task) -> *mut Task { (hooks().pick_next_task)(grq, cpu_id, prev) }

#[inline]
pub fn requeue_prev_locked(grq: &mut GlobalRunQueue, prev: &mut Task) { (hooks().requeue_prev_locked)(grq, prev) }

#[inline]
pub fn g_diag_idle_bit(cpu: usize) -> bool { (hooks().g_diag_idle_bit)(cpu) }

#[inline]
pub fn cpu_tick_stale_ms(cpu: usize) -> u64 { (hooks().cpu_tick_stale_ms)(cpu) }

#[inline]
pub fn lp_idle_lie_report(cpu: usize) { (hooks().lp_idle_lie_report)(cpu) }

#[inline]
pub fn sched_slice_to_ms(slice_ns: u64) -> u32 { (hooks().sched_slice_to_ms)(slice_ns) }

#[inline]
pub fn sched_ev_drain(budget: usize) -> usize { (hooks().sched_ev_drain)(budget) }

#[inline]
pub fn note_task_freed(task: *mut crate::process::Task) { (hooks().note_task_freed)(task) }

#[inline]
pub fn report_poisoned_rq_entry(cpu: usize, raw: usize) { (hooks().report_poisoned_rq_entry)(cpu, raw) }

#[inline]
pub fn psledger_task(cpu: usize) -> usize { (hooks().psledger_task)(cpu) }

#[inline]
pub fn psledger_cpu(cpu: usize) -> u32 { (hooks().psledger_cpu)(cpu) }

#[inline]
pub fn psledger_jif(cpu: usize) -> u64 { (hooks().psledger_jif)(cpu) }

#[inline]
pub fn psledger_pk(cpu: usize) -> u32 { (hooks().psledger_pk)(cpu) }

#[inline]
pub fn ww_watch(cpu: usize) -> usize { (hooks().ww_watch)(cpu) }
#[inline]
pub fn ww_watch_slots() -> alloc::vec::Vec<usize> { (hooks().ww_watch_slots)() }

#[inline]
pub fn load_balance() { (hooks().load_balance)() }

/// Gated wrapper (devirt lane): same leakhunt arm check the policy
/// body performs first; disarmed boots skip the seam.
#[inline]
pub fn psledger_note_publish(task: *mut Task, cpu: usize, pk: u32) {
    if !crate::trace::EVENT_LEAKHUNT.armed() {
        return;
    }
    (hooks().psledger_note_publish)(task, cpu, pk)
}

#[inline]
pub fn psledger_peek(task: *mut Task) -> Option<(u32, u32, u64)> { (hooks().psledger_peek)(task) }

#[inline]
pub fn sched_hist_ring(task: &Task, kind: u64, why: u64, aux: u64) { (hooks().sched_hist_ring)(task, kind, why, aux) }

#[inline]
pub fn sched_hist_refuse(task: *mut Task, skip_reason: u32, cpu: usize) { (hooks().sched_hist_refuse)(task, skip_reason, cpu) }

#[inline]
pub fn sched_hist_link(task: *mut Task) { (hooks().sched_hist_link)(task) }

#[inline]
pub fn sched_hist_pick(task: *mut Task, cpu: usize) { (hooks().sched_hist_pick)(task, cpu) }

#[inline]
pub fn ww_cfs_geometry(task: *mut Task) -> (u64, u64, u64) { (hooks().ww_cfs_geometry)(task) }

#[inline]
pub fn ww_cfs_tie_mass(task: *mut Task) -> u64 { (hooks().ww_cfs_tie_mass)(task) }

#[inline]
pub fn ww_cfs_tie_mass_vr(vr: u64) -> u64 { (hooks().ww_cfs_tie_mass_vr)(vr) }

#[inline]
pub fn ww_tail_fill(ix: usize) -> u64 { (hooks().ww_tail_fill)(ix) }

#[inline]
pub fn ww_append_dump(out: &mut dyn core::fmt::Write, task: *mut Task, now: u64) { (hooks().ww_append_dump)(out, task, now) }

#[inline]
pub fn fair_sched_class() -> &'static dyn SchedClass { (hooks().fair_sched_class)() }
#[inline]
pub fn rt_sched_class() -> &'static dyn SchedClass { (hooks().rt_sched_class)() }
#[inline]
pub fn dl_sched_class() -> &'static dyn SchedClass { (hooks().dl_sched_class)() }
#[inline]
pub fn idle_sched_class() -> &'static dyn SchedClass { (hooks().idle_sched_class)() }
#[inline]
pub fn stop_sched_class() -> &'static dyn SchedClass { (hooks().stop_sched_class)() }

#[inline]
pub fn sched_ev(tag: u32, pid: u32, a: u64, b: u64) { (hooks().sched_ev)(tag, pid, a, b) }
#[inline]
pub fn stale_oncpu_healed(tag: &[u8], cpu_id: usize, task: *mut Task, claim: i32, pk: u32) { (hooks().stale_oncpu_healed)(tag, cpu_id, task, claim, pk) }

/// Gated wrapper (devirt lane): vr_canary's policy body first checks
/// the sched-hist arm flag — when the history is disarmed (every
/// standard boot) the row does nothing, so the wrapper checks the same
/// FRAMEWORK-side condition and skips the seam entirely. Armed boots
/// dispatch exactly as before.
#[inline]
pub fn vr_canary(rq: &mut crate::sched::entities::CfsRunQueue, task: *mut Task, site: &[u8; 3], vr: u64) {
    if !crate::sched::sched::sched_hist_armed() {
        return;
    }
    (hooks().vr_canary)(rq, task, site, vr)
}
