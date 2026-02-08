//! 进程管理模块
//!
//! 本模块实现进程管理功能，完全遵循 Linux 内核的进程模型：
//! - `task`: 进程控制块 (task_struct)
//! - `list`: 双向链表 (include/linux/types.h::list_head)
//! - `wait`: 等待队列 (kernel/wait.c)
//! - `test`: 进程测试
//! - `usermod`: 用户模式管理

pub mod task;
pub mod list;
pub mod test;
pub mod usermod;
pub mod wait;

pub use task::Task;

/// 获取当前进程的PID
pub fn current_pid() -> u32 {
    crate::sched::get_current_pid()
}

/// 获取当前进程的父进程PID
pub fn current_ppid() -> u32 {
    crate::sched::get_current_ppid()
}
