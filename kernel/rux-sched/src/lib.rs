//! The SCHED POLICY SERVICE CRATE (crate-flip S1b): the scheduling
//! classes (stop/dl/rt/fair/idle), the pick/enqueue/wake-placement/
//! census/diagnostic walks — under a crate-level unsafe-forbid
//! boundary. The framework keeps sched/sched.rs (the CORE half: GRQ +
//! guards, the switch machinery, current/tp, NEED_RESCHED, task-slot
//! lifecycle, the walkers) and sched/sched_hooks.rs (the hook table);
//! drivers and arch consume the policy through those.
//!
//! Sole-copy state (the rung-2 recipe's final step): the policy files
//! LIVE in this crate (src/sched/); the framework keeps sched.rs (the
//! core half), class.rs (the dispatch contract), entities.rs (the
//! storage), sched_hooks.rs (the seam), edges/side_edges/reapdrain.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the proven shape
// from rux-syscall and rux-net).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, fs, interrupt, list, mm,
     safety, sched, sync, timer, trace,
};
// The process service tree (crate-flip rung: process): the merged
// view (moved policy + the framework-staying process modules) —
// crate::process::X paths keep resolving, one-way dependency.
pub use rux_process::process;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The policy tree at crate::sched_policy (the historical crate::sched::
// paths stay valid through the framework's re-export chain).
pub mod sched_policy {
    #[path = "sched_policy.rs"] pub mod sched_policy;
    #[path = "fair.rs"] pub mod fair;
    #[path = "rt.rs"] pub mod rt;
    #[path = "deadline.rs"] pub mod deadline;
    #[path = "idle.rs"] pub mod idle;
    #[path = "stop_task.rs"] pub mod stop_task;
    #[path = "stats.rs"] pub mod stats;
    #[path = "sched_ev.rs"] pub mod sched_ev;
    #[path = "wakewatch.rs"] pub mod wakewatch;

    // The framework globs (the service files' bare-name refs).
    pub use crate::arch::*;
    pub use crate::config;
    pub use crate::debug::*;
    pub use crate::drivers::*;
    pub use crate::errno;
    pub use crate::fs::*;
    pub use crate::mm::*;
    pub use crate::process::*;
    pub use crate::safety::*;
    pub use crate::sched::*;
    pub use crate::sync::*;
    pub use crate::timer::*;
    pub use crate::trace;
}


fn fair_class_fn() -> &'static dyn crate::sched::class::SchedClass {
    &sched_policy::fair::FAIR_SCHED_CLASS
}

fn rt_class_fn() -> &'static dyn crate::sched::class::SchedClass {
    &sched_policy::rt::RT_SCHED_CLASS
}

fn dl_class_fn() -> &'static dyn crate::sched::class::SchedClass {
    &sched_policy::deadline::DL_SCHED_CLASS
}

fn idle_class_fn() -> &'static dyn crate::sched::class::SchedClass {
    &sched_policy::idle::IDLE_SCHED_CLASS
}

fn stop_class_fn() -> &'static dyn crate::sched::class::SchedClass {
    &sched_policy::stop_task::STOP_SCHED_CLASS
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
    use rux::sched::sched_hooks::{SchedHooks, sched_register_hooks};
    sched_register_hooks(SchedHooks {
            r56_print_pid: sched_policy::sched_policy::r56_print_pid,
            change_task_policy: sched_policy::sched_policy::change_task_policy,
            ww_record: sched_policy::sched_policy::ww_record,
            orphan_watch_confirm: sched_policy::sched_policy::orphan_watch_confirm,
            yield_cpu: sched_policy::sched_policy::yield_cpu,
            enqueue_task_locked: sched_policy::sched_policy::enqueue_task_locked,
            ww_watch_add: sched_policy::sched_policy::ww_watch_add,
            abort_next_release_orphans: sched_policy::sched_policy::abort_next_release_orphans,
            r56_report_switch_abort: sched_policy::sched_policy::r56_report_switch_abort,
            sched_renice_locked: sched_policy::sched_policy::sched_renice_locked,
            psledger_consume: sched_policy::sched_policy::psledger_consume,
            steer_task_cpu: sched_policy::sched_policy::steer_task_cpu,
            sched_can_stop_tick: sched_policy::sched_policy::sched_can_stop_tick,
            dl_throttle: sched_policy::sched_policy::dl_throttle,
            ensure_linked_locked: sched_policy::sched_policy::ensure_linked_locked,
            check_cfs_preempt: sched_policy::sched_policy::check_cfs_preempt,
            check_rt_preempt: sched_policy::sched_policy::check_rt_preempt,
            check_dl_preempt: sched_policy::sched_policy::check_dl_preempt,
            orphan_watch_clear: sched_policy::sched_policy::orphan_watch_clear,
            pick_giveups: sched_policy::fair::pick_giveups,
            ww_slot_mismatch_counters: sched_policy::sched_policy::ww_slot_mismatch_counters,
            ww_sched_snapshot: sched_policy::sched_policy::ww_sched_snapshot,
            ww_stalled_cpu_state: sched_policy::sched_policy::ww_stalled_cpu_state,
            debug_claim_inject: sched_policy::sched_policy::debug_claim_inject,
            debug_oncpu_inject: sched_policy::sched_policy::debug_oncpu_inject,
            ww_heals: sched_policy::fair::ww_heals,
            ww_subgate_counts: sched_policy::wakewatch::ww_subgate_counts,
            ww_rt_pick_stats: sched_policy::wakewatch::ww_rt_pick_stats,
            heartbeat: sched_policy::wakewatch::heartbeat,
            stallwatch_tick: sched_policy::wakewatch::stallwatch_tick,
            preempt_check_wake: sched_policy::sched_policy::preempt_check_wake,
            preempt_check_wake_sync: sched_policy::sched_policy::preempt_check_wake_sync,
            w2_refusal_census: sched_policy::sched_policy::w2_refusal_census,
            task_sched_class: sched_policy::class::task_sched_class,
            ww_note_slot_mismatch: sched_policy::sched_policy::ww_note_slot_mismatch,
            monopolist_census: sched_policy::sched_policy::monopolist_census,
            runq_strand_census: sched_policy::sched_policy::runq_strand_census,
            ffrt_wait_census: sched_policy::sched_policy::ffrt_wait_census,
            rt_wedge_dump_tick: sched_policy::sched_policy::rt_wedge_dump_tick,
            rt_bandwidth_tick: sched_policy::sched_policy::rt_bandwidth_tick,
            emit_grq_reports: sched_policy::sched_policy::emit_grq_reports,
            pick_next_task: sched_policy::sched_policy::pick_next_task,
            requeue_prev_locked: sched_policy::sched_policy::requeue_prev_locked,
            g_diag_idle_bit: sched_policy::sched_policy::g_diag_idle_bit,
            cpu_tick_stale_ms: sched_policy::sched_policy::cpu_tick_stale_ms,
            lp_idle_lie_report: sched_policy::sched_policy::lp_idle_lie_report,
            sched_ev_drain: sched_policy::sched_ev::sched_ev_drain,
            note_task_freed: sched_policy::wakewatch::note_task_freed,
            report_poisoned_rq_entry: sched_policy::sched_policy::report_poisoned_rq_entry,
            psledger_task: |cpu| sched_policy::sched_policy::PSLEDGER_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed),
            psledger_cpu: |cpu| sched_policy::sched_policy::PSLEDGER_CPU[cpu].load(core::sync::atomic::Ordering::Relaxed),
            psledger_jif: |cpu| sched_policy::sched_policy::PSLEDGER_JIF[cpu].load(core::sync::atomic::Ordering::Relaxed),
            psledger_pk: |cpu| sched_policy::sched_policy::PSLEDGER_PK[cpu].load(core::sync::atomic::Ordering::Relaxed),
            ww_watch: |cpu| sched_policy::sched_policy::WW_WATCH[cpu].load(core::sync::atomic::Ordering::Relaxed),
            ww_watch_slots: || sched_policy::sched_policy::WW_WATCH
                .iter()
                .map(|w| w.load(core::sync::atomic::Ordering::Relaxed))
                .collect(),
            load_balance: sched_policy::sched_policy::load_balance,
            psledger_note_publish: sched_policy::sched_policy::psledger_note_publish,
            psledger_peek: sched_policy::sched_policy::psledger_peek,
            sched_hist_ring: sched_policy::sched_policy::sched_hist_ring,
            sched_hist_refuse: sched_policy::sched_policy::sched_hist_refuse,
            sched_hist_link: sched_policy::sched_policy::sched_hist_link,
            sched_hist_pick: sched_policy::sched_policy::sched_hist_pick,
            ww_cfs_geometry: sched_policy::sched_policy::ww_cfs_geometry,
            ww_cfs_tie_mass: sched_policy::sched_policy::ww_cfs_tie_mass,
            ww_cfs_tie_mass_vr: sched_policy::sched_policy::ww_cfs_tie_mass_vr,
            ww_tail_fill: sched_policy::sched_policy::ww_tail_fill,
            ww_append_dump: sched_policy::sched_policy::ww_append_dump,
            sched_ev: sched_policy::sched_ev::sched_ev,
            stale_oncpu_healed: sched_policy::fair::stale_oncpu_healed,
            fair_sched_class: fair_class_fn,
            rt_sched_class: rt_class_fn,
            dl_sched_class: dl_class_fn,
            idle_sched_class: idle_class_fn,
            stop_sched_class: stop_class_fn,            vr_canary: |rq, task, site, vr| sched_policy::fair::vr_canary(rq, task, site, vr),
            sched_clock: || crate::sched::entities::sched_clock(),
            sched_slice_to_ms: |ns| crate::sched::entities::sched_slice_to_ms(ns),
    });
}

/// Guest-test modules that moved with the S1b crate flip (SERVICE-TEST
/// SEAM): the fair pins (the sched half of the old framework
/// mm_sched_pins) and the grq-10ns timeline driver. This crate is
/// #![forbid(unsafe_code)]: the timeline driver reaches the engine's
/// raw-node internals only through the framework's SAFE debug_* helpers
/// (sched/edges.rs).
#[cfg(feature = "unit-test")]
// Service-internal tests live CENTRALLY in kernel/src/tests/services/
// (the project test-location rule); compiled HERE for crate visibility
// (top-level #[path] resolves against this crate's src/).
#[cfg(feature = "unit-test")]
#[path = "../../src/tests/services/fair_pins.rs"]
pub mod fair_pins;
#[cfg(feature = "unit-test")]
#[path = "../../src/tests/services/fair_timeline.rs"]
pub mod fair_timeline;

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (rux-bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    crate::fair_pins::test_fair_pins();
    crate::fair_timeline::test_fair_timeline();
}
