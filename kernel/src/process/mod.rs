//! 进程管理模块
//!
//! 本模块实现进程管理功能，完全遵循 Linux 内核的进程模型：
//! - `task`: 进程控制块 (task_struct)
//! - `sched`: 调度器 (kernel/sched/core.c)
//! - `pid`: PID 管理和 PID 分配器

pub mod task;
pub mod sched;
pub mod pid;
pub mod test;
pub mod usermod;

pub use task::{Task, TaskState, Pid, SchedPolicy};
pub use sched::{schedule, enqueue_task, dequeue_task, do_fork};
pub use test::test_fork;
pub use usermod::test_user_program;

/// 获取当前进程的PID
pub fn current_pid() -> u32 {
    sched::get_current_pid()
}

/// 获取当前进程的父进程PID
pub fn current_ppid() -> u32 {
    sched::get_current_ppid()
}
