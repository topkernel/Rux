//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/softirqs — per-CPU softirq counters (Linux's
//! fs/proc/softirqs.c).
//!
//! Linux's layout: a header with one column per CPU, then one row per
//! softirq vector with the vector name and its per-CPU counts:
//!
//! ```text
//!                     CPU0       CPU1
//!           HI:          1          1
//!        TIMER:     1270274     650138
//! ```
//!
//! The vector names and their order are Linux's — Rux's softirq module
//! already uses the same vector set (Hi, Timer, NetTx, NetRx, Block,
//! IrqPoll, Tasklet, Sched, Hrtimer, Rcu).

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

/// The vector names, in Linux's order.
const NAMES: [&str; crate::interrupt::softirq::NR_SOFTIRQS] = [
    "HI", "TIMER", "NET_TX", "NET_RX", "BLOCK", "IRQ_POLL", "TASKLET", "SCHED", "HRTIMER", "RCU",
];

/// Generate /proc/softirqs content.
pub fn generate() -> Vec<u8> {
    let num_cpus = crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS);
    let mut out = String::new();

    // Header: the vector-name column, then one column per CPU.
    out.push_str(&format!("{:>13}", ""));
    for cpu in 0..num_cpus {
        out.push_str(&format!("{:>11}", format!("CPU{}", cpu)));
    }
    out.push('\n');

    for (vec, name) in NAMES.iter().enumerate() {
        out.push_str(&format!("{:>13}", format!("{}:", name)));
        for cpu in 0..num_cpus {
            let count = crate::interrupt::softirq::raised_count(cpu, vec);
            out.push_str(&format!("{:>11}", count));
        }
        out.push('\n');
    }

    out.into_bytes()
}

/// Total softirq raises across all CPUs and vectors (Linux's
/// /proc/stat "softirq" line is per-vector; this is the grand total
/// used by the stat file).
pub fn total_raised() -> u64 {
    let num_cpus = crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS);
    let mut total = 0u64;
    for vec in 0..crate::interrupt::softirq::NR_SOFTIRQS {
        for cpu in 0..num_cpus {
            total = total.saturating_add(crate::interrupt::softirq::raised_count(cpu, vec));
        }
    }
    total
}

/// Read one per-CPU counter (kept here so the stat file does not have
/// to know the softirq module's layout).
pub fn raised_count(cpu: usize, vec: usize) -> u64 {
    crate::interrupt::softirq::SOFTIRQ_RAISED[cpu][vec].load(Ordering::Relaxed)
}
