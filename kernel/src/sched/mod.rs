//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 调度器模块
//!
//!
//! - 调度类 (sched_class): fair, rt, idle, deadline
//! - 运行队列 (rq): 每个 CPU 一个 rq
//! - 调度实体 (sched_entity): fair 调度单位
//! - 调度入口: schedule() -> __schedule() -> context_switch()
//!
//! 当前实现: CFS (Completely Fair Scheduler)

pub mod sched;
pub mod pid;
pub mod cfs;

pub use sched::{
    current,
    get_current_pid,
    get_current_ppid,
    find_task_by_pid,
    get_current_fdtable,
    do_exit,
    do_wait,
    do_wait_nonblock,
    alloc_task_slot,
    free_task_slot,
    enqueue_task,
    init,
    schedule,
    send_signal,
    cpu_rq,
    this_cpu_rq,
    load_balance,
    resched_curr,
    resched_cpu,
    wake_up_process,
    // 抢占式调度支持
    need_resched,
    set_need_resched,
    scheduler_tick,
    // SMP 多核支持
    cpu_idle_loop,
};

// 导出 CFS 相关类型
pub use cfs::{
    SchedEntity,
    CfsRunQueue,
    LoadWeight,
    NICE_0_LOAD,
    SCHED_MIN_GRANULARITY_NS,
    SCHED_LATENCY_NS,
    sched_clock,
    sched_slice_to_ms,
    ms_to_ns,
};

// 直接从配置导出 MAX_CPUS
pub use crate::config::MAX_CPUS;
