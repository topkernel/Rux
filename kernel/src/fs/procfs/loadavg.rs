//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/loadavg - System load average
//!
//! Reference: Linux fs/proc/loadavg.c

use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

/// Load average sample interval in seconds
const LOAD_SAMPLE_INTERVAL: u64 = 5;

/// Number of samples for load average calculation
/// 1 minute = 60/5 = 12 samples
/// 5 minutes = 300/5 = 60 samples
/// 15 minutes = 900/5 = 180 samples
const SAMPLES_1MIN: usize = 12;
const SAMPLES_5MIN: usize = 60;
const SAMPLES_15MIN: usize = 180;

/// Global load average state
static LOAD_STATE: LoadAvgState = LoadAvgState::new();

/// Load average state
struct LoadAvgState {
    /// Running tasks count history (circular buffer)
    running_history: AtomicU64,
    /// Total tasks count history (circular buffer)
    total_history: AtomicU64,
    /// Last sample time
    last_sample: AtomicU64,
    /// Current running tasks
    running_tasks: AtomicU64,
    /// Current total tasks
    total_tasks: AtomicU64,
    /// Last PID allocated
    last_pid: AtomicU64,
}

impl LoadAvgState {
    const fn new() -> Self {
        Self {
            running_history: AtomicU64::new(0),
            total_history: AtomicU64::new(0),
            last_sample: AtomicU64::new(0),
            running_tasks: AtomicU64::new(0),
            total_tasks: AtomicU64::new(1),
            last_pid: AtomicU64::new(1),
        }
    }

    /// Update load average sample
    fn update(&self) {
        use super::uptime::get_uptime_ms;

        let now = get_uptime_ms() / 1000;  // Convert to seconds
        let last = self.last_sample.load(Ordering::Relaxed);

        if now - last >= LOAD_SAMPLE_INTERVAL {
            self.last_sample.store(now, Ordering::Relaxed);
            // Update running/total counts from scheduler
            // For now, we just use placeholder values
        }
    }

    /// Get load averages (1, 5, 15 minute)
    fn get_load_avg(&self) -> (f64, f64, f64) {
        // Simplified implementation: return 0 load
        // TODO: Implement proper exponential decay average
        (0.0, 0.0, 0.0)
    }

    /// Get running and total task counts
    fn get_task_counts(&self) -> (u64, u64) {
        let running = self.running_tasks.load(Ordering::Relaxed);
        let total = self.total_tasks.load(Ordering::Relaxed);
        (running, total)
    }

    /// Get last allocated PID
    fn get_last_pid(&self) -> u64 {
        self.last_pid.load(Ordering::Relaxed)
    }
}

/// Generate /proc/loadavg content
///
/// Format: <load1> <load5> <load15> <running>/<total> <last_pid>
pub fn generate() -> Vec<u8> {
    LOAD_STATE.update();

    let (load1, load5, load15) = LOAD_STATE.get_load_avg();
    let (running, total) = LOAD_STATE.get_task_counts();
    let last_pid = LOAD_STATE.get_last_pid();

    // Format: "0.00 0.00 0.00 1/64 12345"
    let content = format!(
        "{:.2} {:.2} {:.2} {}/{} {}\n",
        load1, load5, load15, running, total, last_pid
    );

    content.into_bytes()
}

/// Update task counts (called by scheduler)
pub fn update_task_counts(running: u64, total: u64) {
    LOAD_STATE.running_tasks.store(running, Ordering::Relaxed);
    LOAD_STATE.total_tasks.store(total, Ordering::Relaxed);
}

/// Update last PID (called by process allocator)
pub fn update_last_pid(pid: u64) {
    LOAD_STATE.last_pid.store(pid, Ordering::Relaxed);
}
