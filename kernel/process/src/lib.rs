//! The PROCESS SERVICE CRATE (crate-flip rung: process): the movable
//! process policy — fork/clone, the wait4/waitid family, exec's ELF
//! loading policy, ptrace, coredump, reboot, kthread spawn/stop, and
//! the pid allocator — under a crate-level unsafe-forbid boundary.
//!
//! What stayed in the framework (the rung's measured cut — the
//! shelving wall was TYPES and hot-path mechanisms, not unsafe):
//! - `process/task.rs`, `task_ref.rs`, `unsafe.rs`: the Task
//!   type + slot mechanics (65+ external type users; inherent impls
//!   live with the type) and the audited edges.
//! - `process/pid_hash.rs`: `pid_hash_lookup_pinned` sits on the
//!   scheduler's hot paths.
//! - `process/wait.rs`: `WaitQueueHead` is embedded in the framework
//!   sync primitives (mutex/condvar/semaphore) and 19 service files.
//! - `process/ns.rs`: the six namespace TYPES are Task fields and
//!   framework fs consumed (MountEntry, NsKind, NS_FILE_OPS) — types
//!   cannot cross a service boundary upward.
//! - `process/exit.rs` (the exit/reap MECHANISM): do_exit /
//!   do_exit_group / wake_group_chldexit / release_task are called
//!   from the arch traps, signal delivery and the scheduler reap
//!   path — hot, so no fn-pointer row sits in front of them; the
//!   wait4/waitid family (cold, syscall-layer only) moved here as
//!   `exit_wait`.
//! - `process/exec_ring.rs`: the STACKZERO ring the riscv64 fault
//!   path walks (hot read; the crate's exec policy writes it).
//! - `api/process.rs`: the seam — four cold tables
//!   (ProcessHooks / KthreadHooks / PidHooks / NsHooks),
//!   OnceSlot-published.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root
// so the service files' crate::X paths resolve unchanged (the net
// recipe). `process` is NOT in this list: this crate's own modules live
// flat at the root and own it. `sbi` is riscv64-only in the framework.
pub use rux::{
    arch, boot_edges, cmdline, config, console, debug, drivers, errno, fs,
    interrupt, ipc, list, mm, net, perf, safety, sched, security, signal,
    syscall, trace,
};
// The framework's sync module plus the moved futex diagnostics, merged
// at the sync crate's root (crate::sync::futex::X paths — exit_teardown's
// rwfz_dump_tgid calls).
pub use ::sync;
// The timer service tree (cold-four lane: timer): the merged view
// (framework timekeeping core + the moved report halves) re-exported at
// this root — the sibling-service dependency (process -> timer) is
// one-way, the ipc/fs precedent.
pub use ::timer;
#[cfg(feature = "riscv64")]
pub use rux::sbi;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The service files (THIS crate's compilation copies), flat at the
// crate root (seed-core-design 2c.2).
pub mod fork;
pub mod kthread;
pub mod pid;
pub mod ptrace;
pub mod coredump;
pub mod reboot;
pub mod task_helpers;
pub mod exec_policy;
// p23-shrink-process: the movable ns policy, the movable thread-flag
// (TIF_MEMDIE + the alloc-failure latch), and the securebits value
// whose only consumers are the syscall layer.
pub mod ns;
pub mod thread_flags;
pub mod securebits;

// exec: the ELF policy (this crate) merged with the
// framework-side stack-zero ring at the historical path — so
// crate::exec::X keeps resolving for both halves' users.
pub mod exec {
    pub use crate::exec_policy::*;
    pub use rux::process::{hotpath::STACKZERO_RING, types::ZeroEntry};
}

// exit: the wait4/waitid family + the TEARDOWN family (this crate)
// merged with the framework-side exit mechanism (do_exit /
// do_exit_group / wake_group_chldexit — hot-name-called by the
// traps, signal delivery and the scheduler reap path; they reach
// the teardown through the ExitHooks rows).
pub mod exit_wait;
pub mod exit_teardown;
pub mod exit {
    pub use crate::exit_wait::*;
    pub use crate::exit_teardown::*;
    pub use rux::process::{hotpath::{do_exit, do_exit_group, ERESTARTSYS, wake_group_chldexit}};
}

// The old framework mod.rs's re-exports at their historical paths
// (crate::do_fork, the pid knobs/consts).
pub use crate::fork::do_fork;
pub use crate::ns::*;
pub use crate::task_helpers::{
    current_pgid, thread_clear_callee_saved, thread_set_entry, set_user_tls,
};
pub use crate::pid::{
    alloc_pid, free_pid, PID_INIT, PID_SWAPPER, PID_MAX_LIMIT, PID_MAX_DEFAULT,
    RESERVED_PIDS,
};

// The framework-staying modules at the historical paths.
pub use rux::process::{
    types, hotpath, r#unsafe};
pub use rux::process::{
    types::Task, hotpath::current_pid, hotpath::current_ppid, hotpath::current_task,
    hotpath::find_task_by_pid,
};

// The framework's test counters at the crate root, where the moved
// guest-test modules' imports expect them (the service-test seam
// contract: same pass/fail counters as the in-framework suite).
#[cfg(feature = "unit-test")]
pub use rux::tests::{test_pass, test_fail, test_skip, test_group_start};
#[cfg(feature = "unit-test")]
pub use rux::{test_assert_eq, test_println};

// The guest-test modules that followed the files (the framework's
// tests/mod.rs rows are gated with the move note). Service-internal
// tests live CENTRALLY in kernel/core/src/tests/services/.
// (tests/wait4.rs is NOT taken: it was already gated off in the
// framework suite before this flip — dead since the syscall rung —
// and resurrecting it is not this rung's change.)
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/fork.rs"]
pub mod test_fork;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/boundary.rs"]
pub mod test_boundary;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/pid_test.rs"]
pub mod test_pid;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/smp_schedule.rs"]
pub mod test_smp_schedule;

/// The boot-time registration (bin calls this before
/// rust_main_impl, after the sched/mm rows this crate's policy
/// consumes): the three process seam tables. OnceSlot-only — no
/// Spinlock, no per-CPU state (the x86_64 unit-test kernel lesson).
pub fn register_all() {
    rux::api::process::process_register_hooks(rux::api::process::ProcessHooks {
        do_execve_elf: exec_policy::do_execve_elf,
        wait4_park_audit: exit_wait::wait4_park_audit,
        signal_makes_core: coredump::signal_makes_core,
        do_coredump: coredump::do_coredump,
        panic_reboot: reboot::panic_reboot,
        force_reboot: reboot::force_reboot,
        force_power_off: reboot::force_power_off,
        ctrl_alt_del_latch: reboot::ctrl_alt_del_latch,
        cad_deliver_pending: reboot::cad_deliver_pending,
        init_exited_fallback_cascade: reboot::init_exited_fallback_cascade,
        thread_set_entry: task_helpers::thread_set_entry,
        current_pgid: task_helpers::current_pgid,
        memdie_latch: thread_flags::memdie_latch,
    });
    rux::api::process::kthread_register_hooks(rux::api::process::KthreadHooks {
        kthread_run: kthread::kthread_run,
        kthread_run_on_cpu: kthread::kthread_run_on_cpu,
        kthread_should_stop: kthread::kthread_should_stop,
        kthread_stop: kthread::kthread_stop,
        kthread_name: kthread::kthread_name,
    });
    rux::api::process::exit_register_hooks(rux::api::process::ExitHooks {
        exit_robust_list: exit_teardown::exit_robust_list,
        exit_clear_child_tid: exit_teardown::exit_clear_child_tid,
        kill_other_threads: exit_teardown::kill_other_threads,
        wait_for_thread_group_death: exit_teardown::wait_for_thread_group_death,
        sweep_dead_threads: exit_teardown::sweep_dead_threads,
        reparent_children_to_init: exit_teardown::reparent_children_to_init,
    });
    rux::api::process::pid_register_hooks(rux::api::process::PidHooks {
        alloc_pid: pid::alloc_pid,
        free_pid: pid::free_pid,
        pid_max_live: pid::pid_max_live,
        set_pid_max_live: pid::set_pid_max_live,
        last_pid: pid::last_pid,
        set_last_pid: pid::set_last_pid,
        pid_hash_init: pid::pid_hash_init,
    });
    rux::api::process::ns_register_hooks(rux::api::process::NsHooks {
        ns_track_vfsmount: ns::ns_track_vfsmount,
        in_init_mnt_ns: ns::in_init_mnt_ns,
        current_mnt_ns: ns::current_mnt_ns,
        resolve_local_pid_to_global: ns::resolve_local_pid_to_global,
        ns_register_mount: ns::ns_register_mount,
        ns_get_mounts: ns::ns_get_mounts,
        build_ns_file: ns::build_ns_file,
        current_ipc_ns: ns::current_ipc_ns,
        translate_sender_pid_ns: ns::translate_sender_pid_ns,
    });
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so
/// pass/fail lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    rux::tests::test_group_start("fork");
    test_fork::test_fork();
    rux::tests::test_group_start("boundary");
    test_boundary::test_boundary();
    rux::tests::test_group_start("pid");
    test_pid::test_pid();
    rux::tests::test_group_start("smp_schedule");
    test_smp_schedule::test_smp_schedule();
}
