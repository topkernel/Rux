//! The PROCESS SERVICE CRATE (crate-flip rung: process): the movable
//! process policy — fork/clone, the wait4/waitid family, exec's ELF
//! loading policy, ptrace, coredump, reboot, kthread spawn/stop, and
//! the pid allocator — under a crate-level unsafe-forbid boundary.
//!
//! What stayed in the framework (the rung's measured cut — the
//! shelving wall was TYPES and hot-path mechanisms, not unsafe):
//! - `process/task.rs`, `task_ref.rs`, `unsafe_edges.rs`: the Task
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
//! - `process/process_hooks.rs`: the seam — three cold tables
//!   (ProcessHooks / KthreadHooks / PidHooks), OnceSlot-published.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root
// so the service files' crate::X paths resolve unchanged (the net
// recipe). `process` is NOT in this list — this crate's own union
// module below owns that path. `sbi` is riscv64-only in the framework.
pub use rux::{
    arch, boot_edges, cmdline, config, console, debug, drivers, errno, fs,
    interrupt, ipc, list, mm, net, perf, safety, sched, security, signal,
    syscall, trace,
};
// The sync module comes from sync's UNION module (sync-upper
// lane): the framework's sync plus the moved futex diagnostics at
// their historical crate::sync::futex::X paths (exit_teardown's
// rwfz_dump_tgid calls).
pub use ::sync::sync;
// The timer service tree (cold-four lane: timer): the merged view
// (framework timekeeping core + the moved report halves) re-exported at
// this root — the sibling-service dependency (process -> timer) is
// one-way, the ipc/fs precedent.
pub use ::timer::timer;
#[cfg(feature = "riscv64")]
pub use rux::sbi;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

/// The process tree, nested under crate::process (the historical
/// paths): the service files (THIS crate's compilation copies) plus
/// the framework-staying modules re-exported — the union the sibling
/// services consumed when process was one framework module (the
/// ipc recipe: syscall/mm/sched/drivers point their `process`
/// re-export at THIS module).
pub mod process {
    // the service files (zero-move step: #[path]-includes of the
    // files living in the framework tree; they are this crate's sole
    // compilation copies — the framework's mod.rs no longer declares
    // them)
    pub mod fork;
    pub mod kthread;
    pub mod pid;
    pub mod ptrace;
    pub mod coredump;
    pub mod reboot;
    pub mod task_helpers;

    // exec: the ELF policy (this crate) merged with the
    // framework-side stack-zero ring at the historical path — so
    // crate::process::exec::X keeps resolving for both halves' users.
    #[path = "exec.rs"]
    pub mod exec_policy;
    pub mod exec {
        pub use crate::process::exec_policy::*;
        pub use rux::process::exec_ring::*;
    }

    // exit: the wait4/waitid family + the TEARDOWN family (this crate)
    // merged with the framework-side exit mechanism (do_exit /
    // do_exit_group / wake_group_chldexit — hot-name-called by the
    // traps, signal delivery and the scheduler reap path; they reach
    // the teardown through the ExitHooks rows).
    pub mod exit_wait;
    pub mod exit_teardown;
    pub mod exit {
        pub use crate::process::exit_wait::*;
        pub use crate::process::exit_teardown::*;
        pub use rux::process::exit::*;
    }

    // The old framework mod.rs's re-exports at their historical paths
    // (crate::process::do_fork, the pid knobs/consts).
    pub use crate::process::fork::do_fork;
    pub use crate::process::task_helpers::{thread_clear_callee_saved, set_user_tls};
    pub use crate::process::pid::{
        alloc_pid, free_pid, PID_INIT, PID_SWAPPER, PID_MAX_LIMIT, PID_MAX_DEFAULT,
        RESERVED_PIDS,
    };

    // The framework-staying modules at the historical paths.
    pub use rux::process::{
        task, task_ref, unsafe_edges, pid_hash, wait, ns, exec_ring, ptrace_core,
    };
    pub use rux::process::{
        Task, current_pid, current_ppid, current_pgid, current_task,
        find_task_by_pid, thread_set_entry,
    };

    // The framework's test counters at the `super::` path the moved
    // guest-test modules' imports expect (the service-test seam
    // contract: same pass/fail counters as the in-framework suite).
    #[cfg(feature = "unit-test")]
    pub use rux::tests::{test_pass, test_fail, test_skip, test_group_start};
    #[cfg(feature = "unit-test")]
    pub use rux::{test_assert_eq, test_println};

    // The guest-test modules that followed the files (the framework's
    // tests/mod.rs rows are gated with the move note). Service-internal
    // tests live CENTRALLY in kernel/src/tests/services/.
    // (tests/wait4.rs is NOT taken: it was already gated off in the
    // framework suite before this flip — dead since the syscall rung —
    // and resurrecting it is not this rung's change.)
    #[cfg(feature = "unit-test")]
    #[path = "../../../src/tests/services/fork.rs"]
    pub mod test_fork;
    #[cfg(feature = "unit-test")]
    #[path = "../../../src/tests/services/boundary.rs"]
    pub mod test_boundary;
    #[cfg(feature = "unit-test")]
    #[path = "../../../src/tests/services/pid_test.rs"]
    pub mod test_pid;
    #[cfg(feature = "unit-test")]
    #[path = "../../../src/tests/services/smp_schedule.rs"]
    pub mod test_smp_schedule;
}

/// The boot-time registration (bin calls this before
/// rust_main_impl, after the sched/mm rows this crate's policy
/// consumes): the three process seam tables. OnceSlot-only — no
/// Spinlock, no per-CPU state (the x86_64 unit-test kernel lesson).
pub fn register_all() {
    use rux::process::process_hooks;
    process_hooks::process_register_hooks(process_hooks::ProcessHooks {
        do_execve_elf: process::exec_policy::do_execve_elf,
        wait4_park_audit: process::exit_wait::wait4_park_audit,
        signal_makes_core: process::coredump::signal_makes_core,
        do_coredump: process::coredump::do_coredump,
        panic_reboot: process::reboot::panic_reboot,
        force_reboot: process::reboot::force_reboot,
        force_power_off: process::reboot::force_power_off,
        ctrl_alt_del_latch: process::reboot::ctrl_alt_del_latch,
        cad_deliver_pending: process::reboot::cad_deliver_pending,
        init_exited_fallback_cascade: process::reboot::init_exited_fallback_cascade,
    });
    process_hooks::kthread_register_hooks(process_hooks::KthreadHooks {
        kthread_run: process::kthread::kthread_run,
        kthread_run_on_cpu: process::kthread::kthread_run_on_cpu,
        kthread_should_stop: process::kthread::kthread_should_stop,
        kthread_stop: process::kthread::kthread_stop,
        kthread_name: process::kthread::kthread_name,
    });
    process_hooks::exit_register_hooks(process_hooks::ExitHooks {
        exit_robust_list: process::exit_teardown::exit_robust_list,
        exit_clear_child_tid: process::exit_teardown::exit_clear_child_tid,
        kill_other_threads: process::exit_teardown::kill_other_threads,
        wait_for_thread_group_death: process::exit_teardown::wait_for_thread_group_death,
        sweep_dead_threads: process::exit_teardown::sweep_dead_threads,
        reparent_children_to_init: process::exit_teardown::reparent_children_to_init,
    });
    process_hooks::pid_register_hooks(process_hooks::PidHooks {
        alloc_pid: process::pid::alloc_pid,
        free_pid: process::pid::free_pid,
        pid_max_live: process::pid::pid_max_live,
        set_pid_max_live: process::pid::set_pid_max_live,
        last_pid: process::pid::last_pid,
        set_last_pid: process::pid::set_last_pid,
    });
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so
/// pass/fail lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    rux::tests::test_group_start("fork");
    process::test_fork::test_fork();
    rux::tests::test_group_start("boundary");
    process::test_boundary::test_boundary();
    rux::tests::test_group_start("pid");
    process::test_pid::test_pid();
    rux::tests::test_group_start("smp_schedule");
    process::test_smp_schedule::test_smp_schedule();
}
