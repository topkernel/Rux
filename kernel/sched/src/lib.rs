//! The SCHED POLICY SERVICE CRATE (crate-flip S1b): the scheduling
//! classes (stop/dl/rt/fair/idle), the pick/enqueue/wake-placement/
//! census/diagnostic walks — under a crate-level unsafe-forbid
//! boundary. The framework keeps sched/sched.rs (the CORE half: GRQ +
//! guards, the switch machinery, current/tp, NEED_RESCHED, task-slot
//! lifecycle, the walkers) and api/sched.rs (the hook table);
//! drivers and arch consume the policy through those.
//!
//! Sole-copy state (the rung-2 recipe's final step): the policy files
//! LIVE in this crate (src/), flat under its root; the framework keeps sched.rs (the
//! core half), class.rs (the dispatch contract), entities.rs (the
//! storage), api/sched.rs (the seam), edges/side_edges/reapdrain.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the proven shape
// from syscall and net).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, fs, interrupt, list, mm,
     safety, sched, sync, timer, trace,
};
// The process service tree (crate-flip rung: process): the merged
// view (moved policy + the framework-staying process modules) —
// crate::process::X paths keep resolving, one-way dependency.
pub use ::process;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The scheduling classes and the policy walks.
// The policy files, flat at the crate root (seed-core-design 2c.2).
pub mod sched_policy;
pub mod cgroup_policy;
pub mod fair;
pub mod rt;
pub mod deadline;
pub mod idle;
pub mod stop_task;
pub mod stats;
pub mod sched_ev;
pub mod wakewatch;


fn fair_class_fn() -> &'static dyn crate::sched::hotpath::SchedClass {
    &fair::FAIR_SCHED_CLASS
}

fn rt_class_fn() -> &'static dyn crate::sched::hotpath::SchedClass {
    &rt::RT_SCHED_CLASS
}

fn dl_class_fn() -> &'static dyn crate::sched::hotpath::SchedClass {
    &deadline::DL_SCHED_CLASS
}

fn idle_class_fn() -> &'static dyn crate::sched::hotpath::SchedClass {
    &idle::IDLE_SCHED_CLASS
}

fn stop_class_fn() -> &'static dyn crate::sched::hotpath::SchedClass {
    &stop_task::STOP_SCHED_CLASS
}


/// Registration: fills the framework's SchedHooks table (the S1b data
/// seam). Called by the composition bin before the framework impl.
/// Devirt lane (2026-11-23): the observability rows whose bodies are
/// plain state touches (pick tracer, pick-generation ledger, phase
/// stamps, schedstat/wake-census/walk-census counters, class-nonempty
/// mask) left the table — that state lives framework-side
/// (sched/entities.rs) with direct framework accessors; only policy
/// rows and armed-gated diagnostics remain registered here.
pub fn register_all() {
    use rux::api::sched::{SchedHooks, sched_register_hooks};
    sched_register_hooks(SchedHooks {
            r56_print_pid: sched_policy::r56_print_pid,
            change_task_policy: sched_policy::change_task_policy,
            ww_record: sched_policy::ww_record,
            orphan_watch_confirm: sched_policy::orphan_watch_confirm,
            yield_cpu: sched_policy::yield_cpu,
            enqueue_task_locked: sched_policy::enqueue_task_locked,
            ww_watch_add: sched_policy::ww_watch_add,
            abort_next_release_orphans: sched_policy::abort_next_release_orphans,
            r56_report_switch_abort: sched_policy::r56_report_switch_abort,
            sched_renice_locked: sched_policy::sched_renice_locked,
            psledger_consume: sched_policy::psledger_consume,
            steer_task_cpu: sched_policy::steer_task_cpu,
            sched_can_stop_tick: sched_policy::sched_can_stop_tick,
            dl_throttle: sched_policy::dl_throttle,
            ensure_linked_locked: sched_policy::ensure_linked_locked,
            check_cfs_preempt: sched_policy::check_cfs_preempt,
            check_rt_preempt: sched_policy::check_rt_preempt,
            check_dl_preempt: sched_policy::check_dl_preempt,
            orphan_watch_clear: sched_policy::orphan_watch_clear,
            pick_giveups: fair::pick_giveups,
            ww_slot_mismatch_counters: sched_policy::ww_slot_mismatch_counters,
            ww_sched_snapshot: sched_policy::ww_sched_snapshot,
            ww_stalled_cpu_state: sched_policy::ww_stalled_cpu_state,
            debug_claim_inject: sched_policy::debug_claim_inject,
            debug_oncpu_inject: sched_policy::debug_oncpu_inject,
            ww_heals: fair::ww_heals,
            ww_subgate_counts: wakewatch::ww_subgate_counts,
            ww_rt_pick_stats: wakewatch::ww_rt_pick_stats,
            heartbeat: wakewatch::heartbeat,
            stallwatch_tick: wakewatch::stallwatch_tick,
            preempt_check_wake: sched_policy::preempt_check_wake,
            preempt_check_wake_sync: sched_policy::preempt_check_wake_sync,
            w2_refusal_census: sched_policy::w2_refusal_census,
            task_sched_class: crate::sched::hotpath::task_sched_class,
            ww_note_slot_mismatch: sched_policy::ww_note_slot_mismatch,
            monopolist_census: sched_policy::monopolist_census,
            runq_strand_census: sched_policy::runq_strand_census,
            ffrt_wait_census: sched_policy::ffrt_wait_census,
            rt_wedge_dump_tick: sched_policy::rt_wedge_dump_tick,
            rt_bandwidth_tick: sched_policy::rt_bandwidth_tick,
            emit_grq_reports: sched_policy::emit_grq_reports,
            pick_next_task: sched_policy::pick_next_task,
            requeue_prev_locked: sched_policy::requeue_prev_locked,
            g_diag_idle_bit: sched_policy::g_diag_idle_bit,
            cpu_tick_stale_ms: sched_policy::cpu_tick_stale_ms,
            lp_idle_lie_report: sched_policy::lp_idle_lie_report,
            sched_ev_drain: sched_ev::sched_ev_drain,
            note_task_freed: wakewatch::note_task_freed,
            report_poisoned_rq_entry: sched_policy::report_poisoned_rq_entry,
            psledger_task: |cpu| sched_policy::PSLEDGER_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed),
            psledger_cpu: |cpu| sched_policy::PSLEDGER_CPU[cpu].load(core::sync::atomic::Ordering::Relaxed),
            psledger_jif: |cpu| sched_policy::PSLEDGER_JIF[cpu].load(core::sync::atomic::Ordering::Relaxed),
            psledger_pk: |cpu| sched_policy::PSLEDGER_PK[cpu].load(core::sync::atomic::Ordering::Relaxed),
            ww_watch: |cpu| sched_policy::WW_WATCH[cpu].load(core::sync::atomic::Ordering::Relaxed),
            ww_watch_slots: || sched_policy::WW_WATCH
                .iter()
                .map(|w| w.load(core::sync::atomic::Ordering::Relaxed))
                .collect(),
            load_balance: sched_policy::load_balance,
            psledger_note_publish: sched_policy::psledger_note_publish,
            psledger_peek: sched_policy::psledger_peek,
            sched_hist_ring: sched_policy::sched_hist_ring,
            sched_hist_refuse: sched_policy::sched_hist_refuse,
            sched_hist_link: sched_policy::sched_hist_link,
            sched_hist_pick: sched_policy::sched_hist_pick,
            ww_cfs_geometry: sched_policy::ww_cfs_geometry,
            ww_cfs_tie_mass: sched_policy::ww_cfs_tie_mass,
            ww_cfs_tie_mass_vr: sched_policy::ww_cfs_tie_mass_vr,
            ww_tail_fill: sched_policy::ww_tail_fill,
            ww_append_dump: sched_policy::ww_append_dump,
            sched_ev: sched_ev::sched_ev,
            stale_oncpu_healed: fair::stale_oncpu_healed,
            fair_sched_class: fair_class_fn,
            rt_sched_class: rt_class_fn,
            dl_sched_class: dl_class_fn,
            idle_sched_class: idle_class_fn,
            stop_sched_class: stop_class_fn,            vr_canary: |rq, task, site, vr| fair::vr_canary(rq, task, site, vr),
            sched_clock: || crate::sched::hotpath::sched_clock(),
            sched_slice_to_ms: |ns| crate::sched::hotpath::sched_slice_to_ms(ns),
    });
}

/// Guest-test modules that moved with the S1b crate flip (SERVICE-TEST
/// SEAM): the fair pins (the sched half of the old framework
/// mm_sched_pins) and the grq-10ns timeline driver. This crate is
/// #![forbid(unsafe_code)]: the timeline driver reaches the engine's
/// raw-node internals only through the framework's SAFE debug_* helpers
/// (sched/unsafe.rs).
#[cfg(feature = "unit-test")]
// Service-internal tests live CENTRALLY in kernel/core/src/tests/services/
// (the project test-location rule); compiled HERE for crate visibility
// (top-level #[path] resolves against this crate's src/).
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/fair_pins.rs"]
pub mod fair_pins;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/fair_timeline.rs"]
pub mod fair_timeline;

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    crate::fair_pins::test_fair_pins();
    crate::fair_timeline::test_fair_timeline();
}
