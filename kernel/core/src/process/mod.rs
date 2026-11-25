//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Process management module
//!
//! This module implements process management functionality.
//! - `task`: Process control block (task_struct)
//! - `fork`: Process creation
//! - `wait`: Wait queues

// pub mod reboot; — moved to the process service crate (the file
// now physically lives in kernel/process/src/).
pub mod task;
pub mod task_ext;
pub mod task_ref;
// pub mod fork; — moved to the process service crate (crate-flip
// rung: process); the crate's process module re-exports it.
// pub mod pid; — moved to the process service crate.
pub mod pid_hash;
pub mod wait;
pub mod exit;
// pub mod exec; — moved to the process service crate; the
// STACKZERO ring statics the riscv64 fault path walks stay
// framework-side here (hot read, no hook row).
pub mod exec_ring;
// pub mod kthread; — moved to the process service crate.
// The wrapper surface for sibling crates and framework callers
// (the hook-table indirection lives in process_hooks; these are the
// #[inline] entry points).
pub use process_hooks::{kthread_run, kthread_run_on_cpu, kthread_should_stop};
// pub mod ptrace; — moved to the process service crate; the two
// entry points the trap files call by name stay core-side:
pub mod ptrace_core;
// pub mod coredump; — moved to the process service crate.
pub mod ns;
pub mod r#unsafe;
// The flip seam: the framework's cold door into the process service
// crate (three tables — lifecycle, kthread, pid; see the file header).
pub mod process_hooks;

pub use task::Task;

// ==================== Arch ThreadStruct bridges ====================
// Small cfg helpers so generic process code never touches arch-specific
// ThreadStruct field layouts directly.

/// Zero the kernel-side callee-saved registers of a fresh task (the

/// Seed a fresh task's context-switch state: the first switch-in
/// "returns" into `entry` with `sp` as the kernel stack pointer.
pub fn thread_set_entry(
    thread: &mut crate::arch::thread::ThreadStruct,
    entry: u64,
    sp: u64,
) {
    thread.sp = sp;
    #[cfg(feature = "riscv64")]
    {
        thread.ra = entry;
    }
    #[cfg(feature = "aarch64")]
    {
        // Same shape as riscv64: __switch_to returns into thread.ra with
        // sp = thread.sp. The fork child's ra = ret_from_fork, whose
        // first switch-in runs the user-exit loop into the seeded
        // pt_regs frame (child erets to EL0 with x0 = 0).
        thread.ra = entry;
    }
    #[cfg(feature = "x86_64")]
    {
        thread.callee.ret_addr = entry;
    }
}

/// Set the task's user TLS pointer (riscv64: tp register in pt_regs;
/// x86_64: FS base restored on switch-in; aarch64: TPIDR_EL0 saved in

/// Get current process ID
pub fn current_pid() -> u32 {
    crate::sched::get_current_pid()
}

/// Get current parent process ID
pub fn current_ppid() -> u32 {
    crate::sched::get_current_ppid()
}

/// Get current process group ID
pub fn current_pgid() -> u32 {
    // SAFETY: sched::current() returns a valid pointer to the currently running task.
    crate::sched::current().map_or(0, |t| unsafe { (*t).pgid() })
}

/// Get current task reference
///
/// Returns None if no current task is set (e.g., during early boot)
pub fn current_task() -> Option<&'static mut Task> {
    crate::sched::current()
}

/// Find task by PID
///
/// Uses the PID hash table for O(log N) lookup.
/// Works for all task states (running, sleeping, zombie).
pub fn find_task_by_pid(pid: u32) -> Option<&'static mut Task> {
    let ptr = pid_hash::pid_hash_lookup(pid);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: pid_hash_lookup returns a valid Task pointer from the PID hash table.
        // The pointer remains valid as long as the task is alive (not reaped).
        Some(unsafe { &mut *ptr })
    }
}
