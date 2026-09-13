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

use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Load sample period in jiffies (5 seconds at HZ=100).
const LOAD_FREQ: u64 = 5 * crate::config::KERNEL_HZ as u64;

/// Exponential decay factors for 1-min, 5-min, 15-min windows.
/// Computed as exp(-LOAD_FREQ / (window * HZ)):
///   exp_1  = exp(-5/60)  ≈ 0.920044
///   exp_5  = exp(-5/300) ≈ 0.983471
///   exp_15 = exp(-5/900) ≈ 0.994459
///
/// Stored as fixed-point (0.32 format) for integer arithmetic.
/// Multiply by 2^32 to get the integer representation.
const EXP_1: u64  = (0.920044414676f64 * (1u64 << 32) as f64) as u64;
const EXP_5: u64  = (0.983471453846f64 * (1u64 << 32) as f64) as u64;
const EXP_15: u64 = (0.994459848005f64 * (1u64 << 32) as f64) as u64;
const EXP_FACTOR: [u64; 3] = [EXP_1, EXP_5, EXP_15];

/// Fixed-point (0.32 format) load average accumulators.
/// To get the float value: avenrun[i] / 2^32 ≈ avenrun[i] >> 16 / 65536.
static AVENRUN: [AtomicU64; 3] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Jiffies counter at last load sample.
static LAST_LOAD_JIFFIES: AtomicU64 = AtomicU64::new(0);

/// Running task count at last sample (for /proc/loadavg output).
static LAST_RUNNING: AtomicU32 = AtomicU32::new(0);

/// Total task count at last sample (for /proc/loadavg output).
static LAST_TOTAL: AtomicU32 = AtomicU32::new(0);

/// Update load average from scheduler_tick().
/// Call this from the timer tick path; it auto-throttles to LOAD_FREQ.
pub fn update_load_avg() {
    let now = crate::drivers::timer::riscv64::get_jiffies();
    let last = LAST_LOAD_JIFFIES.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < LOAD_FREQ {
        return;
    }

    // Try to claim the update slot. CAS avoids multiple CPUs updating simultaneously.
    if LAST_LOAD_JIFFIES.compare_exchange(last, now, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return;
    }

    // Count running tasks and total tasks.
    let (pids, count, _truncated) = crate::process::pid_hash::pid_hash_collect_all();
    let total = count as u64;

    let mut running: u64 = 0;
    for i in 0..count {
        let task = unsafe { crate::sched::find_task_by_pid(pids[i]) };
        if !task.is_null() {
            if unsafe { (*task).state().bits() == crate::process::task::TaskState::RUNNING } {
                running += 1;
            }
        }
    }

    LAST_RUNNING.store(running as u32, Ordering::Relaxed);
    LAST_TOTAL.store(total as u32, Ordering::Relaxed);

    // Exponential moving average: avenrun = avenrun * exp + nrun * (1 - exp)
    // In fixed-point: avenrun = avenrun * exp >> 32 + nrun * ((1<<32 - exp) >> 32)
    for i in 0..3 {
        let prev = AVENRUN[i].load(Ordering::Relaxed);
        let exp = EXP_FACTOR[i];
        // Fixed-point EMA: (prev*exp)>>32 keeps the full precision — the
        // old (prev>>32)*(exp>>32) truncated both words to ~0, so the
        // averages never moved off zero and increments rounded away
        // (review VFS-M11). The product is computed in u128: with
        // load > 1.0 (prev > 2^32) a u64 multiply would overflow.
        let decayed = (((prev as u128) * (exp as u128)) >> 32) as u64;
        let new_val = decayed + running * ((1u64 << 32) - exp);
        AVENRUN[i].store(new_val, Ordering::Relaxed);
    }
}

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
    use crate::process::pid_hash;

    let (pids, count, _truncated) = pid_hash::pid_hash_collect_all();

    let running = LAST_RUNNING.load(Ordering::Relaxed) as u64;
    let total = if count > 0 { count as u64 } else { 1 };

    let last_pid = if count > 0 {
        *pids.iter().max().unwrap_or(&1) as u64
    } else {
        1
    };

    let content = format!(
        "{} {} {} {}/{} {}\n",
        fmt_load(AVENRUN[0].load(Ordering::Relaxed)),
        fmt_load(AVENRUN[1].load(Ordering::Relaxed)),
        fmt_load(AVENRUN[2].load(Ordering::Relaxed)),
        running, total, last_pid
    );

    content.into_bytes()
}
