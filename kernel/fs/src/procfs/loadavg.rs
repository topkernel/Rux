//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/loadavg - System load average
//!
//! Implements exponentially-decaying load average matching the Linux kernel:
//!   avenrun[i] = avenrun[i] * exp(i) + nrun * (1 - exp(i))
//!
//! Updated every LOAD_FREQ (5*HZ) jiffies from scheduler_tick().

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Load sample period in jiffies (5 seconds at HZ=100).
// The EMA cells + the sampler moved framework-side (fs::perf_ctr —
// the scheduler tick calls update_load_avg directly, zero
// indirection); generate() reads the framework cells.
pub use rux::fs::hotpath::{AVENRUN, LAST_RUNNING, LAST_TOTAL, update_load_avg};

/// Render one fixed-point load value as "<whole>.<2 digits>" using pure
/// integer math. The kernel executes with sstatus.FS = Off (no FPU
/// context management yet — review ARCH-H1), so any f64 arithmetic here
/// traps as an illegal instruction in kernel mode; `cat /proc/loadavg`
/// used to panic the whole kernel.
fn fmt_load(fixed: u64) -> alloc::string::String {
    let whole = fixed >> 32;
    let frac = ((fixed & 0xFFFF_FFFF) * 100) >> 32; // two decimal digits
    format!("{}.{}", whole, frac)
}

/// Generate /proc/loadavg content
///
/// Format: <load1> <load5> <load15> <running>/<total> <last_pid>
pub fn generate() -> Vec<u8> {
    use crate::process::hotpath;

    let pids = hotpath::pid_hash_collect_all();

    // Linux nr_threads semantics: idle tasks (pid 0) are NOT counted —
    // Linux never allocates a pid for them. Rux hash-inserts MAX_CPUS
    // idle Tasks with pid 0; without the filter the total ran high by
    // the pre-created idle count on every read.
    let real = pids.iter().filter(|&&p| p != 0).count();

    let running = LAST_RUNNING.load(Ordering::Relaxed) as u64;
    let total = if real > 0 { real as u64 } else { 1 };

    let last_pid = pids.iter().copied().max().unwrap_or(1) as u64;

    let content = format!(
        "{} {} {} {}/{} {}\n",
        fmt_load(AVENRUN[0].load(Ordering::Relaxed)),
        fmt_load(AVENRUN[1].load(Ordering::Relaxed)),
        fmt_load(AVENRUN[2].load(Ordering::Relaxed)),
        running, total, last_pid
    );

    content.into_bytes()
}
