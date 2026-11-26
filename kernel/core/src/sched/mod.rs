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

pub mod r#unsafe;
// The scheduler hook table (SchedHooks) is crate::api::sched.
pub mod entities;
pub mod reapdrain;
pub mod sched;
// pub mod sched_policy; — MOVED to the sched service crate (S1b)
// pub mod sched_ev; — MOVED to the sched service crate (S1b)
pub mod class;
// pub mod rt; — MOVED to the sched service crate (S1b)
// pub mod deadline; — MOVED to the sched service crate (S1b)
// pub mod stop_task; — MOVED to the sched service crate (S1b)
// pub mod idle; — MOVED to the sched service crate (S1b)
// pub mod fair; — MOVED to the sched service crate (S1b)
pub mod cgroup;
// pub mod wakewatch; — MOVED to the sched service crate (S1b)
// pub mod stats; — MOVED to the sched service crate (S1b)

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
    // readers use wake_disp()/wake_idle_mask() (framework-native since
    // the devirt lane).
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
    // Framework-native observability accessors (sched-seam devirt
    // lane): the former hook rows whose bodies are plain state touches.
    pick_trace,
    pick_trace_peek,
    enabled,
    set_enabled,
    class_nonempty,
    class_nonempty_set,
    class_nonempty_clear,
    sched_cs_phase,
    sched_cs_phase_set,
    sched_calls,
    sched_calls_inc,
    ww_idle_phase,
    ww_idle_phase_set,
    wake_disp,
    wake_disp_set,
    wake_idle_mask,
    wake_idle_mask_set,
    ww_pick_walks,
    ww_pick_walk_inc,
    ww_oncpu_heals,
    ww_oncpu_heal_inc,
    ww_claim_heals,
    ww_claim_heal_inc,
    tl_poison_inserts,
    tl_poison_insert_inc,
    tl_poison_erases,
    tl_poison_erase_inc,
    sched_hist_armed,
};

// Re-export of process::exit::{do_exit, do_wait, do_wait_nonblock} REMOVED
// (process rung): zero callers at the flip; do_exit/do_exit_group stay
// reachable at crate::process::hotpath::exit::, the wait family moved to the
// process crate.

// Re-export send_signal from signal module for backward compatibility
pub use crate::signal::send_signal;

// Re-export get_current_fdtable from process::task for backward compatibility
pub use crate::process::hotpath::get_current_fdtable;

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

// Shared sched observability state (sched-seam devirt lane): plain
// counters/phase stamps the sched census code reads — framework
// storage, policy-visible through this re-export (the PICK_GEN shape).
pub use entities::{
    PT_TASK,
    PT_CODE,
    PT_JIF,
    CLASS_NONEMPTY,
    WW_IDLE_PHASE,
    SCHED_CS_PHASE,
    SCHED_CALLS,
    WAKE_DISP,
    WAKE_IDLE_MASK,
    SCHEDSTATS,
    WW_PICK_WALKS,
    WW_ONCPU_HEALS,
    WW_CLAIM_HEALS,
};

// Export deadline scheduler types
pub use entities::DlRunQueue;

// Export fair scheduler
// FAIR_SCHED_CLASS: class chain via crate::api::sched::fair_sched_class()

// Export idle scheduler
// IDLE_SCHED_CLASS: via crate::api::sched::idle_sched_class()

// Export stop scheduler
// STOP_SCHED_CLASS: via crate::api::sched::stop_sched_class()

// Export MAX_CPUS directly from config
pub use crate::config::MAX_CPUS;
