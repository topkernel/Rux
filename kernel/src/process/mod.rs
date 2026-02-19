//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 进程管理模块
//!
//! 本模块实现进程管理功能，完全...
//! - `task`: 进程控制块 (task_struct)
//! - `wait`: 等待队列 (kernel/wait.c)
//! - `test`: 进程测试
//! - `usermod`: 用户模式管理

pub mod task;
pub mod list;
pub mod test;
pub mod usermod;
pub mod wait;

pub use task::Task;

pub fn current_pid() -> u32 {
    crate::sched::get_current_pid()
}

pub fn current_ppid() -> u32 {
    crate::sched::get_current_ppid()
}
