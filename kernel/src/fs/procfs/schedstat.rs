//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/schedstat — per-CPU scheduler statistics (Linux's
//! kernel/sched/stats.c show_schedstat).
//!
//! Linux's format (version 17):
//!
//! ```text
//! version 17
//! timestamp 4389098214
//! cpu0 0 0 0 0 0 0 11207196640531 372241431890 24641264
//! ```
//!
//! The per-CPU fields are, in order: `yld_count` (sched_yield() calls
//! served by this CPU — kernel/sched/syscalls.c
//! `schedstat_inc(rq->yld_count)`), a literal `0` (printed as a
//! constant since the schedstats rework), `sched_count` (scheduler
//! invocations, incremented in `__schedule`), `sched_goidle` (times
//! the idle task was picked), `ttwu_count`/`ttwu_local` (wakeups, total
//! and same-CPU), and three runqueue accumulators in nanoseconds:
//! `rq_cpu_time`, `run_delay`, `pcount`.
//!
//! The timestamp is JIFFIES, not nanoseconds — Linux prints
//! `jiffies` there (verified on the reference host: the value advances
//! at the scheduler tick rate, 1000/s at HZ=1000).
//!
//! Rux has no runqueue-delay instrumentation, so the fields it cannot
//! measure report zero — which is also what a modern Linux reports for
//! yld_count/goidle/ttwu on a mostly-idle box.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Voluntary context switches (yields) per CPU.
static YLD_COUNT: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Scheduler invocations per CPU.
static SCHED_COUNT: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Count one sched_yield() served by `cpu` (Linux's rq->yld_count,
/// incremented only in the sched_yield paths — kernel/sched/syscalls.c).
pub fn note_yield(cpu: usize) {
    if cpu < crate::config::MAX_CPUS {
        YLD_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Count one scheduler invocation on `cpu`.
pub fn note_sched_run(cpu: usize) {
    if cpu < crate::config::MAX_CPUS {
        SCHED_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Generate /proc/schedstat content.
pub fn generate() -> Vec<u8> {
    let num_cpus = crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS);
    let mut out = String::new();

    out.push_str("version 17\n");
    // Linux's timestamp is the jiffy counter, not nanoseconds
    // (kernel/sched/stats.c: seq_printf(seq, "timestamp %lu\n", jiffies)).
    out.push_str(&format!(
        "timestamp {}\n",
        crate::drivers::timer::get_jiffies()
    ));

    for cpu in 0..num_cpus {
        let yld = YLD_COUNT[cpu].load(Ordering::Relaxed);
        // Field 2 is a literal "0" in the format string, like Linux's
        // "cpu%d %u 0 %u %u %u %u %llu %llu %lu".
        out.push_str(&format!(
            "cpu{} {} 0 {} {} {} {} {} {} {}\n",
            cpu,
            yld,                                      // yld_count
            SCHED_COUNT[cpu].load(Ordering::Relaxed), // sched_count
            0,                                        // sched_goidle
            0,                                        // ttwu_count
            0,                                        // ttwu_local
            0,                                        // rq_cpu_time (ns)
            0,                                        // run_delay (ns)
            0,                                        // pcount
        ));
    }

    out.into_bytes()
}
