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
    schedule,
    enqueue_task,
    dequeue_task,
    do_fork,
    yield_cpu,
    current,
    get_current_pid,
    get_current_ppid,
    find_task_by_pid,
    get_current_fdtable,
    init_std_fds,
    send_signal,
    send_signal_self,
    handle_pending_signals,
    check_and_handle_signals,
    do_exit,
    do_wait,
    load_balance,
    init,
    init_per_cpu_rq,
    this_cpu_rq,
    cpu_rq,
    RunQueue,
};

pub use pid::{
    alloc_pid,
    free_pid,
    PID_MAX_LIMIT,
    PID_SWAPPER,
    PID_INIT,
};
