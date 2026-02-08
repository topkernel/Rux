//! 进程管理模块
//!
//! 本模块实现进程管理功能，完全遵循 Linux 内核的进程模型：
//! - `task`: 进程控制块 (task_struct)
//! - `wait`: 等待队列 (kernel/wait.c)
//! - `test`: 进程测试
//! - `usermod`: 用户模式管理

pub mod task;
pub mod test;
pub mod usermod;
pub mod wait;

pub use task::{Task, TaskState, Pid, SchedPolicy};
pub use wait::{WaitQueueHead, WaitQueueEntry, WakeUpHint};
pub use test::test_fork;
pub use usermod::test_user_program;

// Re-export scheduler functions for backward compatibility
pub use crate::sched::{schedule, enqueue_task, dequeue_task, do_fork};

/// 获取当前进程的PID
pub fn current_pid() -> u32 {
    crate::sched::get_current_pid()
}

/// 获取当前进程的父进程PID
pub fn current_ppid() -> u32 {
    crate::sched::get_current_ppid()
}
