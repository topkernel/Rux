//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/loadavg - System load average

use alloc::vec::Vec;
use alloc::format;

/// Compute load averages from the task count and CPU count.
///
/// Returns (load1, load5, load15) — currently simplified to
/// running_tasks / max_cpus as a rough approximation.
fn get_load_avg(running: u64, nr_cpus: u64) -> (f64, f64, f64) {
    let load = running as f64 / nr_cpus as f64;
    (load, load, load)
}

/// Generate /proc/loadavg content
///
/// Format: <load1> <load5> <load15> <running>/<total> <last_pid>
pub fn generate() -> Vec<u8> {
    use crate::process::pid_hash;
    use crate::config::MAX_CPUS;

    let (pids, count, _truncated) = pid_hash::pid_hash_collect_all();

    let total_tasks = count as u64;
    // Approximate running tasks: assume at least 1 (current)
    let running_tasks = 1u64;

    let last_pid = if count > 0 {
        *pids.iter().max().unwrap_or(&1) as u64
    } else {
        1
    };

    let nr_cpus = MAX_CPUS as u64;
    let (load1, load5, load15) = get_load_avg(running_tasks, nr_cpus);

    let content = format!(
        "{:.2} {:.2} {:.2} {}/{} {}\n",
        load1, load5, load15, running_tasks, total_tasks, last_pid
    );

    content.into_bytes()
}
