//! The SCHED HOOK TABLE (crate-flip S1b): every lib-side consumer of
//! scheduling-policy internals resolves through this table (the
//! NetHooks pattern) — nullable fn rows, EMPTY defaults no-op until
//! the rux-sched-policy crate's register_all runs (the composition
//! bin, before the framework impl). The row list is the measured
//! reverse surface (run/s1b-hook-rows.json + the counter getters);
//! the entity TYPES live framework-side in sched/entities.rs.

use crate::process::task::{Task, SchedPolicy};
use super::class::SchedClass;
use super::sched::{GlobalRunQueue, GrqPendingReport};
use super::entities::W2RefusalCensus;
use crate::config::MAX_CPUS;

/// The hook set: policy fns the core and the wider kernel call.
#[derive(Clone, Copy)]
pub struct SchedHooks {
    pub enabled: fn() -> bool,
    pub r56_print_pid: fn(prefix: &[u8], pid: u32, suffix: &[u8]),
    pub sched_clock: fn() -> u64,
    pub pick_trace: fn(task: *mut Task, code: u32),
    pub pick_gen_supersede: fn(cpu: usize, task: *mut Task),
    pub change_task_policy: fn(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32),
    pub ww_record: fn(task: *mut Task, code: u32),
    pub orphan_watch_confirm: fn(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool,
    pub pick_trace_peek: fn(task: *mut Task) -> Option<(u32, u64)>,
    pub yield_cpu: fn(),
    pub enqueue_task_locked: fn(grq: &mut GlobalRunQueue, task: &mut Task) -> bool,
    pub ww_watch_add: fn(task: *mut Task),
    pub abort_next_release_orphans: fn(cpu: usize, next: *mut Task),
    pub r56_report_switch_abort: fn(cpu: usize, pid: u32),
    pub sched_renice_locked: fn(task: &mut Task, niceval: i32),
    pub pick_gen_retire: fn(cpu: usize),
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
    pub dfx_claim_inject: fn() -> usize,
    pub dfx_oncpu_inject: fn() -> usize,
    pub ww_pick_walks: fn() -> u64,
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
    pub set_enabled: fn(on: bool),
    pub monopolist_census: fn(now_j: u64),
    pub runq_strand_census: fn(now_j: u64),
    pub ffrt_wait_census: fn(),
    pub rt_wedge_dump_tick: fn(cpu: usize),
    pub rt_bandwidth_tick: fn(cpu: usize, current: *mut Task),
    pub emit_grq_reports: fn(reports: &[GrqPendingReport]),
    pub pick_gen_publish: fn(cpu: usize, task: *mut Task),
    pub pick_next_task: fn(grq: &mut GlobalRunQueue, cpu_id: usize, prev: *mut Task) -> *mut Task,
    pub requeue_prev_locked: fn(grq: &mut GlobalRunQueue, prev: &mut Task),
    pub g_diag_idle_bit: fn(cpu: usize) -> bool,
    pub cpu_tick_stale_ms: fn(cpu: usize) -> u64,
    pub lp_idle_lie_report: fn(cpu: usize),
    pub sched_slice_to_ms: fn(slice_ns: u64) -> u32,
    pub sched_ev_drain: fn(budget: usize) -> usize,
    pub note_task_freed: fn(task: *mut crate::process::Task),
    pub report_poisoned_rq_entry: fn(cpu: usize, raw: usize),
    pub ww_idle_phase: fn(cpu: usize) -> u32,
    pub sched_cs_phase: fn(cpu: usize) -> u32,
    pub wake_disp: fn() -> u32,
    pub wake_idle_mask: fn() -> u32,
    pub class_nonempty: fn() -> u32,
    pub pt_task: fn(cpu: usize) -> usize,
    pub pt_code: fn(cpu: usize) -> u32,
    pub pt_jif: fn(cpu: usize) -> u64,
    pub psledger_task: fn(cpu: usize) -> usize,
    pub psledger_cpu: fn(cpu: usize) -> u32,
    pub psledger_jif: fn(cpu: usize) -> u64,
    pub psledger_pk: fn(cpu: usize) -> u32,
    pub ww_watch: fn(cpu: usize) -> usize,
    pub sched_calls: fn(cpu: usize) -> u64,
}

impl SchedHooks {
    pub const EMPTY: SchedHooks = SchedHooks {
        enabled: || Default::default(),
        r56_print_pid: |prefix, pid, suffix| Default::default(),
        sched_clock: || Default::default(),
        pick_trace: |task, code| Default::default(),
        pick_gen_supersede: |cpu, task| Default::default(),
        change_task_policy: |task, new_policy, rt_prio| Default::default(),
        ww_record: |task, code| Default::default(),
        orphan_watch_confirm: |task, pid, now, threshold| Default::default(),
        pick_trace_peek: |task| Default::default(),
        yield_cpu: || Default::default(),
        enqueue_task_locked: |grq, task| Default::default(),
        ww_watch_add: |task| Default::default(),
        abort_next_release_orphans: |cpu, next| Default::default(),
        r56_report_switch_abort: |cpu, pid| Default::default(),
        sched_renice_locked: |task, niceval| Default::default(),
        pick_gen_retire: |cpu| Default::default(),
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
        dfx_claim_inject: || Default::default(),
        dfx_oncpu_inject: || Default::default(),
        ww_pick_walks: || Default::default(),
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
        set_enabled: |on| Default::default(),
        monopolist_census: |now_j| Default::default(),
        runq_strand_census: |now_j| Default::default(),
        ffrt_wait_census: || Default::default(),
        rt_wedge_dump_tick: |cpu| Default::default(),
        rt_bandwidth_tick: |cpu, current| Default::default(),
        emit_grq_reports: |reports| Default::default(),
        pick_gen_publish: |cpu, task| Default::default(),
        pick_next_task: |grq, cpu_id, prev| Default::default(),
        requeue_prev_locked: |grq, prev| Default::default(),
        g_diag_idle_bit: |cpu| Default::default(),
        cpu_tick_stale_ms: |cpu| Default::default(),
        lp_idle_lie_report: |cpu| Default::default(),
        sched_slice_to_ms: |slice_ns| Default::default(),
        sched_ev_drain: |budget| Default::default(),
        note_task_freed: |task| Default::default(),
        report_poisoned_rq_entry: |cpu, raw| Default::default(),
        ww_idle_phase: |_| Default::default(),
        sched_cs_phase: |_| Default::default(),
        wake_disp: || Default::default(),
        wake_idle_mask: || Default::default(),
        class_nonempty: || Default::default(),
        pt_task: |_| Default::default(),
        pt_code: |_| Default::default(),
        pt_jif: |_| Default::default(),
        psledger_task: |_| Default::default(),
        psledger_cpu: |_| Default::default(),
        psledger_jif: |_| Default::default(),
        psledger_pk: |_| Default::default(),
        ww_watch: |_| Default::default(),
        sched_calls: |_| Default::default(),
    };
}

pub static SCHED_HOOKS: crate::safety::slot::OnceSlot<SchedHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (the policy crate at boot).
pub fn sched_register_hooks(hooks: SchedHooks) {
    let _ = SCHED_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> SchedHooks {
    *SCHED_HOOKS.get().unwrap_or(&SchedHooks::EMPTY)
}

// The lib-side call surface.

#[inline]
pub fn enabled() -> bool { (hooks().enabled)() }

#[inline]
pub fn r56_print_pid(prefix: &[u8], pid: u32, suffix: &[u8]) { (hooks().r56_print_pid)(prefix, pid, suffix) }

#[inline]
pub fn sched_clock() -> u64 { (hooks().sched_clock)() }

#[inline]
pub fn pick_trace(task: *mut Task, code: u32) { (hooks().pick_trace)(task, code) }

#[inline]
pub fn pick_gen_supersede(cpu: usize, task: *mut Task) { (hooks().pick_gen_supersede)(cpu, task) }

#[inline]
pub fn change_task_policy(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32) { (hooks().change_task_policy)(task, new_policy, rt_prio) }

#[inline]
pub fn ww_record(task: *mut Task, code: u32) { (hooks().ww_record)(task, code) }

#[inline]
pub fn orphan_watch_confirm(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool { (hooks().orphan_watch_confirm)(task, pid, now, threshold) }

#[inline]
pub fn pick_trace_peek(task: *mut Task) -> Option<(u32, u64)> { (hooks().pick_trace_peek)(task) }

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

#[inline]
pub fn pick_gen_retire(cpu: usize) { (hooks().pick_gen_retire)(cpu) }

#[inline]
pub fn psledger_consume(task: *mut Task) -> bool { (hooks().psledger_consume)(task) }

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
pub fn dfx_claim_inject() -> usize { (hooks().dfx_claim_inject)() }

#[inline]
pub fn dfx_oncpu_inject() -> usize { (hooks().dfx_oncpu_inject)() }

#[inline]
pub fn ww_pick_walks() -> u64 { (hooks().ww_pick_walks)() }

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
pub fn set_enabled(on: bool) { (hooks().set_enabled)(on) }

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
pub fn pick_gen_publish(cpu: usize, task: *mut Task) { (hooks().pick_gen_publish)(cpu, task) }

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
pub fn ww_idle_phase(cpu: usize) -> u32 { (hooks().ww_idle_phase)(cpu) }

#[inline]
pub fn sched_cs_phase(cpu: usize) -> u32 { (hooks().sched_cs_phase)(cpu) }

#[inline]
pub fn wake_disp() -> u32 { (hooks().wake_disp)() }

#[inline]
pub fn wake_idle_mask() -> u32 { (hooks().wake_idle_mask)() }

#[inline]
pub fn class_nonempty() -> u32 { (hooks().class_nonempty)() }

#[inline]
pub fn pt_task(cpu: usize) -> usize { (hooks().pt_task)(cpu) }

#[inline]
pub fn pt_code(cpu: usize) -> u32 { (hooks().pt_code)(cpu) }

#[inline]
pub fn pt_jif(cpu: usize) -> u64 { (hooks().pt_jif)(cpu) }

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
pub fn sched_calls(cpu: usize) -> u64 { (hooks().sched_calls)(cpu) }
