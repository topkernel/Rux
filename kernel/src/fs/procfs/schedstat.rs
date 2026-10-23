//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/schedstat — per-CPU scheduler statistics (Linux's
//! fs/proc/schedstat.c).
//!
//! Linux's format (version 17):
//!
//! ```text
//! version 17
//! timestamp 4339673372
//! cpu0 0 0 0 0 0 0 11207196640531 372241431890 24641264
//! ```
//!
//! The per-CPU fields are, in order: `yld_count` (voluntary context
//! switches — the task yielded), `ld_count` (tasks moved between CPUs),
//! `lost_yld_count`, `yld_count_max`, `max_sched_delay`,
//! `max_sched_wakeup_delay`, `sched_count` (scheduler invocations),
//! `sched_goidle` and `ttwu_count` (wakeups).
//!
//! Rux has no load balancer and no scheduler-delay instrumentation, so
//! those fields report zero; the yield and scheduler-invocation counts
//! are real.

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

/// Count one voluntary context switch (a task that went to sleep) on
/// `cpu`.
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
    // Linux's timestamp is the nanoseconds at the time of the read.
    out.push_str(&format!(
        "timestamp {}\n",
        crate::drivers::timer::monotonic_ns_since_boot()
    ));

    for cpu in 0..num_cpus {
        let yld = YLD_COUNT[cpu].load(Ordering::Relaxed);
        out.push_str(&format!(
            "cpu{} {} {} {} {} {} {} {} {} {}\n",
            cpu,
            yld,        // yld_count
            0,          // ld_count — no load balancer in Rux
            0,          // lost_yld_count
            0,          // yld_count_max
            0,          // max_sched_delay
            0,          // max_sched_wakeup_delay
            SCHED_COUNT[cpu].load(Ordering::Relaxed),
            0,          // sched_goidle
            0,          // ttwu_count
        ));
    }

    out.into_bytes()
}
