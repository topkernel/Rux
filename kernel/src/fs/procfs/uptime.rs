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
    // Pure integer fixed-point: the kernel runs with sstatus.FS = Off (no
    // FPU context management yet — review ARCH-H1), so f64 math here
    // traps as an illegal instruction in kernel mode and used to panic
    // the whole kernel on `cat /proc/uptime`.
    const TIMER_FREQ: u64 = 10_000_000;
    let cycles = read_time_cycles();

    // seconds with two decimals, scaled by 100
    let secs_x100 = cycles / (TIMER_FREQ / 100);
    let (up_w, up_f) = (secs_x100 / 100, secs_x100 % 100);

    // TODO: Track actual idle time per CPU. Approximate as uptime * ncpus.
    let ncpus = crate::arch::riscv64::smp::num_started_cpus() as u64;
    let idle_x100 = secs_x100 * ncpus;
    let (id_w, id_f) = (idle_x100 / 100, idle_x100 % 100);

    let content = format!("{}.{} {}.{}\n", up_w, up_f, id_w, id_f);
    content.into_bytes()
}

/// Get uptime in seconds (integer, truncated)
///
/// Uses RISC-V timer to calculate uptime.
/// QEMU virt machine clock frequency is 10 MHz.
pub fn get_uptime_secs() -> u64 {
    // QEMU virt machine clock frequency
    const TIMER_FREQ: u64 = 10_000_000;

    let cycles = read_time_cycles();
    cycles / TIMER_FREQ
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
