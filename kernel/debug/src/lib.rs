//! The OBSERVABILITY SERVICE CRATE (crate-flip rung: debug): the
//! movable report/decode/publish layer of the kernel's diagnostic
//! tools, under a crate-level unsafe-deny boundary.
//!
//! The flip's hot/cold split (measured before the move): every
//! per-event RECORD marker — syscost enter/exit/tick_*, fsw note_*/pc_*,
//! mmprof add/bump/now, allocwatch guard, softlockup check/touch,
//! cctx/forkprof/unixrd markers, reaper's on() gate — STAYS in the
//! framework crate (kernel/core/src/debug/) because framework hot paths
//! (trap, scheduler tick, fork/exit, bio) call them directly and must
//! not gain fn-pointer dispatch (the S1 lesson: +25.1% p2_ctxsw until
//! devirtualized). THIS crate owns everything that FORMATS, DECODES or
//! PUBLISHES those counters: the report generators, the dump kthreads,
//! the wedge census, the print arms, and the tracefs file set. It calls
//! DOWN into the framework for the statics (pub peek surfaces) and is
//! called UP only through kernel/core/src/debug/debug_hooks.rs — 18 cold rows
//! (boot init, shutdown/cadence dumps, procfs readers, arm-gated event
//! prints) — plus the direct sibling-dep surface debug::scmtap.
//!
//! Sole-copy state: the moved code lives in this crate; the framework
//! files keep only the hot halves and the historical-name wrappers
//! that dispatch the rows.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the moved files' crate::X paths resolve unchanged (the net recipe).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, fs, interrupt, mm,
    net, printk, process, safety, sched, security, signal, sync, syscall, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
// The ipc service tree (cold-four lane): the merged view (the moved
// binder diagnostics + the framework's binder re-export) re-exported at
// this root — the sibling-service dependency (obs -> ipc) is one-way.
pub use ::ipc;
extern crate alloc;

// The moved cold halves (sibling names — the framework keeps each
// module's hot half under the same historical name; crate::debug::<m>
// resolves to the framework's, these are the crate's report layers).
mod syscost;
mod fsw;
mod schb;
mod mmprof;
mod reaper;
mod driftwatch;
mod filerd;
mod softlockup;
mod lostpark;
mod spinwatch;
/// The hilog census crate half (root-cold lane): the unix send markers,
/// sender tables, and the periodic dump; the kmsg marker + the census
/// gate stay framework-side (kernel/core/src/debug/hilogcensus.rs).
pub mod hilogcensus;
/// The SCM_RIGHTS relay tap — moved WHOLE (every fn is an arm-gated
/// print; syscall and fs call it directly through the sibling
/// service dep, no hook rows on the socket paths).
pub mod scmtap;
/// The tracefs control files — moved WHOLE (boot-published via the
/// trace_files_register row; the trace_event= boot parameter via
/// trace_files_init_from_cmdline).
pub mod trace_files;

/// The boot-time registration (bin calls this before
/// rust_main_impl): the DebugHooks table's rows. OnceSlot publication
/// only — no locks, no per-CPU state (registration runs from
/// rust_main, before arch::smp::init()). THE composition contract.
pub fn register_all() {
    debug::debug_hooks::debug_register_hooks(debug::debug_hooks::DebugHooks {
        syscost_dump: syscost::dump,
        hilog_dump: hilogcensus::dump,
        fsw_dump: fsw::dump,
        mmprof_report_line: mmprof::report_line,
        schb_init: schb::init,
        mmprof_ensure_dump_thread: mmprof::ensure_dump_thread,
        spinwatch_compat_arm: spinwatch::compat_arm,
        trace_files_register: trace_files::register,
        trace_files_init_from_cmdline: trace_files::init_from_cmdline,
        driftwatch_report: driftwatch::report,
        filerd_generate: filerd::generate,
        softlockup_report: softlockup::report,
        lostpark_dump_once: lostpark::dump_once,
        schb_oom_anat_dump: schb::oom_anat_dump,
        reaper_note_nameset: reaper::note_nameset,
        reaper_on_zombie: reaper::on_zombie,
        reaper_on_reap_wait4: reaper::on_reap_wait4,
        reaper_on_reap_waitid: reaper::on_reap_waitid,
        reaper_poll_note: reaper::poll_note,
    });
}

/// The guest-test entry (make test): this rung moved no guest-test
/// bodies (the tools have no in-kernel test modules; the suite's
/// totals ride on the unchanged in-framework tests). Wired for the
/// SERVICE-TEST SEAM shape so a future moved test has its home.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {}
