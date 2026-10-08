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

pub mod task;
pub mod fork;
pub mod pid;
pub mod pid_hash;
pub mod wait;
pub mod exit;
pub mod exec;
pub mod kthread;
pub mod ptrace;
pub mod coredump;
pub mod ns;

pub use task::Task;
pub use fork::do_fork;
pub use pid::{alloc_pid, free_pid, PID_INIT, PID_SWAPPER, PID_MAX_LIMIT, PID_MAX_DEFAULT, RESERVED_PIDS};

// ==================== Arch ThreadStruct bridges ====================
// Small cfg helpers so generic process code never touches arch-specific
// ThreadStruct field layouts directly.

/// Zero the kernel-side callee-saved registers of a fresh task (the
/// user-side copies live in its pt_regs).
pub fn thread_clear_callee_saved(thread: &mut crate::arch::thread::ThreadStruct) {
    #[cfg(feature = "riscv64")]
    {
        thread.s.fill(0);
    }
    #[cfg(feature = "x86_64")]
    {
        thread.callee = Default::default();
    }
}

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
    #[cfg(feature = "x86_64")]
    {
        thread.callee.ret_addr = entry;
    }
}

/// Set the task's user TLS pointer (riscv64: tp register in pt_regs;
/// x86_64: FS base restored on switch-in).
pub fn set_user_tls(task: &mut Task, tls: u64) {
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: pt_regs() returns the task's outermost trap frame.
        unsafe {
            let regs = task.pt_regs();
            if !regs.is_null() {
                (*regs).tp = tls;
            }
        }
    }
    #[cfg(feature = "x86_64")]
    {
        task.thread_mut().fs_base = tls;
    }
}

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
