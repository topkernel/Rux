//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 调度器模块
//!
//! 完全遵循 Linux 内核的调度器设计 (kernel/sched/)
//!
//! Linux 调度器架构：
//! - 调度类 (sched_class): fair, rt, idle, deadline
//! - 运行队列 (rq): 每个 CPU 一个 rq
//! - 调度实体 (sched_entity): fair 调度单位
//! - 调度入口: schedule() -> __schedule() -> context_switch()
//!
//! 当前实现: 简单的 FIFO 调度器（可扩展为 CFS）

pub mod sched;
pub mod pid;

pub use sched::{
    current,
    get_current_pid,
    get_current_ppid,
    find_task_by_pid,
    get_current_fdtable,
    do_exit,
    do_wait,
    do_wait_nonblock,
    do_fork,
    init,
    schedule,
    send_signal,
    // 抢占式调度支持
    need_resched,
    set_need_resched,
    scheduler_tick,
};
