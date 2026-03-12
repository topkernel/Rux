//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Scheduler Module
//!
//!
//! - Scheduling classes (sched_class): fair, rt, idle, deadline
//! - Run queues (rq): one rq per CPU
//! - Scheduling entities (sched_entity): fair scheduling unit
//! - Scheduling entry: schedule() -> __schedule() -> context_switch()
//!
//! Current implementation: CFS (Completely Fair Scheduler)

pub mod sched;
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
    // Preemptive scheduling support
    need_resched,
    set_need_resched,
    scheduler_tick,
    // SMP multi-core support
    cpu_idle_loop,
};

// Export CFS-related types
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

// Export MAX_CPUS directly from config
pub use crate::config::MAX_CPUS;
