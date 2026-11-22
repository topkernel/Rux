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

pub mod edges;
pub mod sched_hooks;
pub mod entities;
pub mod reapdrain;
pub mod sched;
// pub mod sched_policy; — MOVED to the rux-sched-policy service crate (S1b)
// pub mod sched_ev; — MOVED to the rux-sched-policy service crate (S1b)
pub mod class;
// pub mod rt; — MOVED to the rux-sched-policy service crate (S1b)
// pub mod deadline; — MOVED to the rux-sched-policy service crate (S1b)
// pub mod stop_task; — MOVED to the rux-sched-policy service crate (S1b)
// pub mod idle; — MOVED to the rux-sched-policy service crate (S1b)
// pub mod fair; — MOVED to the rux-sched-policy service crate (S1b)
pub mod cgroup;
// pub mod wakewatch; — MOVED to the rux-sched-policy service crate (S1b)
pub mod side_edges;
// pub mod stats; — MOVED to the rux-sched-policy service crate (S1b)

pub use sched::{
    current,
    w2_refusal_census,
    current_is_dying,
    get_current_pid,
    get_current_ppid,
    find_task_by_pid,
    alloc_task_slot,
    free_task_slot,
    enqueue_task,
    dequeue_task,
    dequeue_if_enqueued,
    wake_up_enqueue,
    wake_up_enqueue_nested,
    wake_up_enqueue_opt,
    steer_task_cpu,
    change_task_policy,
    init,
    schedule,
    schedule_dead_never_returns,
    cpu_rq,
    this_cpu_rq,
    load_balance,
    resched_curr,
    resched_cpu,
    claim_idle_cpu_for,
    claim_idle_cpu_for_prefer,
    preempt_check_wake,
    preempt_check_wake_sync,
    // 105th wake-dispatch census latches (bindertrace consumer):
    // readers use sched_hooks::wake_disp()/wake_idle_mask().
    grq_idle_mask,
    grq_nr_running_now,
    wake_up_process,
    wake_up_process_sync,
    yield_cpu,
    for_each_task,
    for_each_task_try,
    defer_exit_notify,
    deferred_exit_notify_slots,
    // Preemptive scheduling support
    need_resched,
    set_need_resched,
    cond_resched,
    scheduler_tick,
    // SMP multi-core support
    cpu_idle_loop,
    init_secondary,
    idle_task_ptr,
};

// Re-export process lifecycle functions from process::exit for backward compatibility
pub use crate::process::exit::{do_exit, do_wait, do_wait_nonblock};

// Re-export send_signal from signal module for backward compatibility
pub use crate::signal::send_signal;

// Re-export get_current_fdtable from process::task for backward compatibility
pub use crate::process::task::get_current_fdtable;

// Export CFS-related types (now in fair module)
pub use entities::{
    SchedEntity,
    LoadWeight,
    NICE_0_LOAD,
    SchedRtEntity,
    SchedDlEntity,
    RR_TIMESLICE_TICKS,
    DL_DEFAULT_PERIOD_NS,
    DL_DEFAULT_RUNTIME_NS,
    DL_BW_UNIT,
    PRIO_TO_WEIGHT,
    PRIO_TO_WMULT,
};
pub use entities::{
    CfsRunQueue,
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
pub use entities::{RtRunQueue, MAX_RT_PRIO};

// Export deadline scheduler types
pub use entities::DlRunQueue;

// Export fair scheduler
// FAIR_SCHED_CLASS: class chain via sched_hooks::fair_sched_class()

// Export idle scheduler
// IDLE_SCHED_CLASS: via sched_hooks::idle_sched_class()

// Export stop scheduler
// STOP_SCHED_CLASS: via sched_hooks::stop_sched_class()

// Export MAX_CPUS directly from config
pub use crate::config::MAX_CPUS;
