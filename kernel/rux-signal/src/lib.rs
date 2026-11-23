//! The SIGNAL POLICY SERVICE CRATE (crate-flip): the send-path queueing
//! core, the default-action table, the pid-ns init delivery filter, the
//! exec/fork handler-reset rules, the /proc SigIgn/SigCgt pair, and the
//! user-originated sender-stamping entries — under a crate-level
//! unsafe-forbid boundary. The framework keeps the MECHANISM (the
//! pending set and its queue on the task, the pt_regs frame build and
//! sigreturn restore, the wake path, the pinned pid-hash lookups, the
//! schedule loop) plus the hook table (kernel/src/signal_hooks.rs);
//! the composition bin registers this crate's rows before the framework
//! boots.
//!
//! HOT-PATH SHAPE (measured before the carve): the per-CHECK path
//! (check_and_deliver_signals on every syscall exit / interrupt
//! return) makes ZERO calls through the table — the pending test and
//! first_unmasked are direct methods on the framework's SigPending.
//! Rows fire only per DELIVERED signal (default_action_class on the
//! Default arm, init_delivery_filter for pid-1/ns-1 tasks) and on the
//! send syscalls. No fn-pointer rows were put on the delivery check.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the policy file's crate::X paths resolve unchanged (the net recipe).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, fs, interrupt, ipc,
    mm, process, safety, sched, security, sync, syscall, timer, trace,
};
pub use rux::{signal_hooks, signal_edges};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

/// The signal tree: everything the framework's signal module exports
/// (the moved policy calls crate::signal::send_signal_with_info and the
/// other mechanism entries down this path).
pub mod signal {
    pub use rux::signal::*;
}

// The carved policy tree. The file is NOT declared by the framework's
// module tree (kernel/src/lib.rs never names signal_policy) — this
// crate is its sole compilation home, under forbid.
#[path = "../../src/signal_policy.rs"]
pub mod signal_policy;

/// Registration: fills the framework's SignalHooks table (the signal
/// data seam). Called by the composition bin BEFORE the framework impl
/// — OnceSlot first-wins, so these rows prevail over any later in-tree
/// registration attempt. Publication is a plain OnceSlot store: no
/// lock, no per-CPU state (registration runs from rust_main, before
/// arch::smp::init()).
pub fn register_all() {
    rux::signal_hooks::signal_register_hooks(rux::signal_hooks::SignalHooks {
        send_signal_core: crate::signal_policy::send_signal_core,
        send_from_user: crate::signal_policy::send_signal_from_user,
        send_from_user_tkill: crate::signal_policy::send_signal_from_user_tkill,
        send_info: crate::signal_policy::send_signal_info,
        send_to_pgid: crate::signal_policy::send_signal_to_pgid,
        force_unblock_current: crate::signal_policy::force_unblock_current,
        default_action_class: crate::signal_policy::default_action_class,
        init_delivery_filter: crate::signal_policy::init_delivery_filter,
        flush_handlers: crate::signal_policy::flush_handlers,
        reset_all_handlers_default: crate::signal_policy::reset_all_handlers_default,
        ign_cgt_masks: crate::signal_policy::ign_cgt_masks,
    });
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (rux-bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests. Groups keep
/// their historical names ("signal handling", "rt_sigprocmask") so the
/// suite-size gate sees the same families.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    // The test bodies open their own groups (their historical shape:
    // "signal handling", "rt_sigprocmask") — same names, same counters.
    crate::signal_test::test_signal();
    crate::signal_procmask_test::test_sigprocmask();
}

// Service-internal tests live CENTRALLY in kernel/src/tests/services/.
#[cfg(feature = "unit-test")]
#[path = "../../src/tests/services/signal_test.rs"]
pub mod signal_test;
#[cfg(feature = "unit-test")]
#[path = "../../src/tests/services/signal_procmask_test.rs"]
pub mod signal_procmask_test;
