//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Scheduler Module
//!
//!
//! - Scheduling classes (sched_class): stop, deadline, rt, fair, idle
//! - Run queues (rq): one rq per CPU
//! - Scheduling entities (sched_entity): fair, rt, deadline
//! - Scheduling entry: schedule() -> __schedule() -> context_switch()
//!
//! Scheduling class hierarchy (highest to lowest priority):
//! 1. stop_sched_class - CPU hotplug/migration
//! 2. dl_sched_class - Deadline (EDF + CBS)
//! 3. rt_sched_class - Real-time (FIFO/RR)
//! 4. fair_sched_class - CFS
//! 5. idle_sched_class - Per-CPU idle task

pub mod sched;
pub mod class;
pub mod rt;
pub mod deadline;
pub mod stop_task;
pub mod idle;
pub mod fair;

pub use sched::{
    current,
    get_current_pid,
    get_current_ppid,
    find_task_by_pid,
    alloc_task_slot,
    free_task_slot,
    enqueue_task,
    dequeue_task,
    init,
    schedule,
    cpu_rq,
    this_cpu_rq,
    load_balance,
    resched_curr,
    resched_cpu,
    wake_up_process,
    yield_cpu,
    for_each_task,
    defer_exit_notify,
    // Preemptive scheduling support
    need_resched,
    set_need_resched,
    scheduler_tick,
    // SMP multi-core support
    cpu_idle_loop,
    init_secondary,
};

// Re-export process lifecycle functions from process::exit for backward compatibility
pub use crate::process::exit::{do_exit, do_wait, do_wait_nonblock};

// Re-export send_signal from signal module for backward compatibility
pub use crate::signal::send_signal;

// Re-export get_current_fdtable from process::task for backward compatibility
pub use crate::process::task::get_current_fdtable;

// Export CFS-related types (now in fair module)
pub use fair::{
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

// Export scheduling class types
pub use class::{
    SchedClass,
    SchedClassId,
    SchedClassIter,
    task_sched_class,
    ENQUEUE_WAKEUP,
    ENQUEUE_HEAD,
    DEQUEUE_SLEEP,
};

// Export RT scheduler types
pub use rt::{
    RtRunQueue,
    SchedRtEntity,
    RT_SCHED_CLASS,
    MAX_RT_PRIO,
    RR_TIMESLICE_TICKS,
};

// Export deadline scheduler types
pub use deadline::{
    DlRunQueue,
    SchedDlEntity,
    DL_SCHED_CLASS,
    DL_DEFAULT_PERIOD_NS,
    DL_DEFAULT_RUNTIME_NS,
};

// Export fair scheduler
pub use fair::FAIR_SCHED_CLASS;

// Export idle scheduler
pub use idle::IDLE_SCHED_CLASS;

// Export stop scheduler
pub use stop_task::STOP_SCHED_CLASS;

// Export MAX_CPUS directly from config
pub use crate::config::MAX_CPUS;
