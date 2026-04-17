//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/uptime - System uptime

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/uptime content
///
/// Format: "<uptime> <idle_time>"
/// Both values are in seconds with two decimal places.
pub fn generate() -> Vec<u8> {
    let uptime_secs = get_uptime_seconds();

    // Format: uptime idle_time
    // TODO: Track actual idle time per CPU. Approximate as uptime * ncpus.
    let num_cpus = crate::arch::riscv64::smp::num_started_cpus() as f64;
    let idle_secs = uptime_secs * num_cpus;
    let content = format!("{:.2} {:.2}\n", uptime_secs, idle_secs);

    content.into_bytes()
}

/// Get system uptime in seconds
///
/// Uses RISC-V timer to calculate uptime.
/// QEMU virt machine clock frequency is 10 MHz.
pub fn get_uptime_seconds() -> f64 {
    // QEMU virt machine clock frequency
    const TIMER_FREQ: u64 = 10_000_000;

    let cycles = read_time_cycles();
    cycles as f64 / TIMER_FREQ as f64
}

/// Get uptime in milliseconds
pub fn get_uptime_ms() -> u64 {
    const TIMER_FREQ: u64 = 10_000_000;
    const MS_PER_SEC: u64 = 1000;

    let cycles = read_time_cycles();
    cycles * MS_PER_SEC / TIMER_FREQ
}

/// Read time cycles from RISC-V timer
#[inline]
fn read_time_cycles() -> u64 {
    let cycles: u64;
    unsafe {
        core::arch::asm!(
            "rdtime {}",
            out(reg) cycles,
            options(nostack, readonly)
        );
    }
    cycles
}

/// Get boot time in cycles (for internal use)
pub fn boot_time_cycles() -> u64 {
    // Boot time is 0 in our system (we start counting from boot)
    0
}
