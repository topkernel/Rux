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
pub mod wait;

pub use task::Task;
pub use fork::do_fork;
pub use pid::{alloc_pid, free_pid, PID_INIT, PID_SWAPPER, PID_MAX_LIMIT};

/// Get current process ID
pub fn current_pid() -> u32 {
    crate::sched::get_current_pid()
}

/// Get current parent process ID
pub fn current_ppid() -> u32 {
    crate::sched::get_current_ppid()
}

/// Get current task reference
///
/// Returns None if no current task is set (e.g., during early boot)
pub fn current_task() -> Option<&'static mut Task> {
    crate::sched::current()
}

/// Find task by PID
///
/// Searches through the scheduler's task list to find a task with the given PID.
/// This is a simplified implementation that only checks the current task and init.
///
/// TODO: Implement a proper PID hash table for O(1) lookup
pub fn find_task_by_pid(pid: u32) -> Option<&'static mut Task> {
    // Check if it's the current task
    if current_pid() == pid {
        return current_task();
    }

    // TODO: Search task list
    // For now, we only support looking up current task
    None
}
