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

pub fn current_pid() -> u32 {
    crate::sched::get_current_pid()
}

pub fn current_ppid() -> u32 {
    crate::sched::get_current_ppid()
}
