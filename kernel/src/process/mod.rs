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

pub use task::Task;
pub use fork::do_fork;
pub use pid::{alloc_pid, free_pid, PID_INIT, PID_SWAPPER, PID_MAX_LIMIT, PID_MAX_DEFAULT, RESERVED_PIDS};

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
        Some(unsafe { &mut *ptr })
    }
}
