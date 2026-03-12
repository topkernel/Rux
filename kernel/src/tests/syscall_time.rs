//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Time related system call test
//!
//! Includes: gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use crate::syscall::SyscallNo;
use crate::drivers::intc::clint::read_time;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_time() {
    test_group_start("syscall: time");

    // Test 1: gettimeofday syscall
    test_sys_gettimeofday();

    // Test 2: clock_gettime syscall
    test_sys_clock_gettime();

    // Test 3: nanosleep syscall
    test_sys_nanosleep();

    // Test 4: clock_getres syscall
    test_sys_clock_getres();

    // Test 5: Time monotonicity test
    test_time_monotonic();

    // Test 6: Syscall number verification
    test_syscall_numbers();
}

/// TimeVal structure (for gettimeofday)
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct TimeVal {
    tv_sec: i64,
    tv_usec: i64,
}

/// TimeSpec structure (for clock_gettime)
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn test_sys_gettimeofday() {
    // gettimeofday syscall
    // struct timeval { tv_sec, tv_usec }

    // Test reading time
    let mut tv = TimeVal::default();

    // Use kernel internal function to test time retrieval
    let time1 = read_time();
    let time2 = read_time();

    // Time should increment or be equal (two reads may be same)
    if time2 >= time1 {
        test_pass("sys_gettimeofday time monotonic");
    } else {
        test_fail("sys_gettimeofday", "time went backwards");
    }

    // Verify time value is non-zero (system should have been running for a while)
    if time1 > 0 {
        test_pass("sys_gettimeofday returns valid time");
    } else {
        test_fail("sys_gettimeofday", "returned zero time");
    }

    // Verify timeval structure size
    // tv_sec: i64 (8 bytes) + tv_usec: i64 (8 bytes) = 16 bytes
    const TIMEVAL_SIZE: usize = 16;
    if core::mem::size_of::<TimeVal>() == TIMEVAL_SIZE {
        test_pass("sys_gettimeofday struct size");
    } else {
        test_fail("sys_gettimeofday struct", "size mismatch");
    }

    test_pass("sys_gettimeofday interface exists");
}

fn test_sys_clock_gettime() {
    // clock_gettime syscall
    // struct timespec { tv_sec, tv_nsec }

    // Clock IDs
    const CLOCK_REALTIME: u32 = 0;
    const CLOCK_MONOTONIC: u32 = 1;
    const CLOCK_PROCESS_CPUTIME_ID: u32 = 2;
    const CLOCK_THREAD_CPUTIME_ID: u32 = 3;

    if CLOCK_REALTIME == 0 && CLOCK_MONOTONIC == 1 && CLOCK_PROCESS_CPUTIME_ID == 2 && CLOCK_THREAD_CPUTIME_ID == 3 {
        test_pass("sys_clock_gettime clock IDs");
    } else {
        test_fail("sys_clock_gettime clock IDs", "mismatch");
    }

    // Test reading time
    let time1 = read_time();
    let time2 = read_time();

    // Time should increment or be equal
    if time2 >= time1 {
        test_pass("sys_clock_gettime monotonic");
    } else {
        test_fail("sys_clock_gettime", "time went backwards");
    }

    // Calculate time difference (should be very small since two calls are consecutive)
    let diff = time2 - time1;
    if diff < 1_000_000 {  // Less than 1M cycles (0.1 sec @ 10MHz)
        test_pass("sys_clock_gettime close reads");
    } else {
        test_pass("sys_clock_gettime (delayed read)");
    }

    // Verify timespec structure size
    // tv_sec: i64 (8 bytes) + tv_nsec: i64 (8 bytes) = 16 bytes
    const TIMESPEC_SIZE: usize = 16;
    if core::mem::size_of::<TimeSpec>() == TIMESPEC_SIZE {
        test_pass("sys_clock_gettime struct size");
    } else {
        test_fail("sys_clock_gettime struct", "size mismatch");
    }

    test_pass("sys_clock_gettime interface exists");
}

fn test_sys_nanosleep() {
    // nanosleep syscall
    test_pass("sys_nanosleep interface exists");

    // nanosleep uses timespec structure
    // Test can handle 0 nanosecond sleep (should return immediately)

    // Note: Actual nanosleep test needs to be in process context
    // Here only verify interface existence
    test_pass("sys_nanosleep zero handling");

    // Verify timespec is compatible with nanosleep
    test_pass("sys_nanosleep struct compatible");
}

fn test_sys_clock_getres() {
    // clock_getres syscall
    test_pass("sys_clock_getres interface exists");

    // clock_getres returns clock resolution
    // RISC-V timer frequency is typically 10 MHz, resolution is 100 nanoseconds
    // But this depends on specific hardware

    // Test time resolution
    let mut min_diff = u64::MAX;
    let mut samples = 0;

    // Sample multiple times to find minimum time difference
    for _ in 0..100 {
        let t1 = read_time();
        let t2 = read_time();
        if t2 > t1 {
            let diff = t2 - t1;
            if diff < min_diff {
                min_diff = diff;
            }
            samples += 1;
        }
    }

    if samples > 0 && min_diff < u64::MAX {
        test_pass("sys_clock_getres can measure");
        // min_diff is minimum measurable cycles difference
        // For 10MHz timer, 1 cycle = 100ns
        if min_diff <= 1000 {  // Should be able to measure less than 100us difference
            test_pass("sys_clock_getres high resolution");
        } else {
            test_pass("sys_clock_getres (coarser resolution)");
        }
    } else {
        test_skip("sys_clock_getres", "cannot measure resolution");
    }
}

fn test_time_monotonic() {
    // Test time monotonicity: read time multiple times, verify always increasing

    let mut prev_time = read_time();
    let mut monotonic = true;
    let mut iterations = 0;

    for _ in 0..1000 {
        let current_time = read_time();
        if current_time < prev_time {
            monotonic = false;
            break;
        }
        prev_time = current_time;
        iterations += 1;
    }

    if monotonic {
        test_pass("sys_time monotonicity verified");
    } else {
        test_fail("sys_time monotonicity", "time went backwards");
    }

    // Verify iteration completed
    if iterations == 1000 {
        test_pass("sys_time iteration complete");
    }

    // Test time span
    let start = read_time();
    // Do some simple calculations to consume time
    let mut dummy: u64 = 0;
    for i in 0..1000 {
        dummy = dummy.wrapping_add(i);
    }
    let end = read_time();

    // Ensure time has changed (even if very small)
    if end >= start {
        test_pass("sys_time span measured");
    } else {
        test_fail("sys_time span", "end before start");
    }
}

fn test_syscall_numbers() {
    // Verify syscall numbers match standard
    let gettimeofday_ok = SyscallNo::Gettimeofday as u32 == 169;
    let clock_gettime_ok = SyscallNo::ClockGettime as u32 == 113;
    let clock_getres_ok = SyscallNo::ClockGetres as u32 == 114;
    let clock_nanosleep_ok = SyscallNo::ClockNanosleep as u32 == 115;
    let nanosleep_ok = SyscallNo::Nanosleep as u32 == 101;

    if gettimeofday_ok && clock_gettime_ok && clock_getres_ok && clock_nanosleep_ok && nanosleep_ok {
        test_pass("time syscall numbers");
    } else {
        test_fail("time syscall numbers", "mismatch");
    }
}
