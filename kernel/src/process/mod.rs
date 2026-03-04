//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 进程管理模块
//!
//! 本模块实现进程管理功能，完全...
//! - `task`: 进程控制块 (task_struct)
//! - `fork`: 进程创建 (kernel/fork.c)
//! - `wait`: 等待队列 (kernel/wait.c)
//! - `usermod`: 用户模式管理

pub mod task;
pub mod fork;
pub mod pid;
pub mod usermod;
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
