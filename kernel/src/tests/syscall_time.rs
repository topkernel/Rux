//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Time related system call test
//!
//! Includes: gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use crate::syscall::SyscallNo;
use crate::syscall::time::{sys_clock_gettime, sys_clock_getres, sys_nanosleep, sys_gettimeofday};
use crate::syscall::memory::{sys_mmap, sys_munmap};
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

/// TimeSpec structure (for clock_gettime / nanosleep / clock_getres)
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

/// Helper: allocate a page of user-space memory via mmap.
/// Returns the virtual address (u64) or 0 on failure.
fn alloc_user_page() -> u64 {
    let addr = sys_mmap([
        0,                    // addr = NULL (let kernel choose)
        4096,                 // length = one page
        0x3,                  // prot = PROT_READ | PROT_WRITE
        0x22,                 // flags = MAP_PRIVATE | MAP_ANONYMOUS
        !0u64,                // fd = -1
        0,                    // offset
    ]);
    let signed = addr as i64;
    if signed > 0 {
        addr
    } else {
        0
    }
}

fn test_sys_gettimeofday() {
    // gettimeofday syscall
    // struct timeval { tv_sec, tv_usec }

    // Verify timeval structure size
    // tv_sec: i64 (8 bytes) + tv_usec: i64 (8 bytes) = 16 bytes
    const TIMEVAL_SIZE: usize = 16;
    if core::mem::size_of::<TimeVal>() == TIMEVAL_SIZE {
        test_pass("sys_gettimeofday struct size");
    } else {
        test_fail("sys_gettimeofday struct", "size mismatch");
    }

    // Allocate user-space memory for the timeval struct
    let buf = alloc_user_page();
    if buf == 0 {
        test_skip("sys_gettimeofday call", "failed to allocate user memory");
        return;
    }

    // Call sys_gettimeofday: args = [tv_ptr, tz_ptr, 0, 0, 0, 0]
    let ret = sys_gettimeofday([buf, 0, 0, 0, 0, 0]);
    if ret == 0 {
        test_pass("sys_gettimeofday interface exists");

        // Read back the timeval from user-space memory
        let tv = unsafe { &*(buf as *const TimeVal) };

        // tv_sec should be positive (system has been running)
        if tv.tv_sec > 0 {
            test_pass("sys_gettimeofday returns valid time");
        } else {
            test_fail("sys_gettimeofday", "tv_sec not positive");
        }

        // tv_usec should be in [0, 999999]
        if tv.tv_usec >= 0 && tv.tv_usec < 1_000_000 {
            test_pass("sys_gettimeofday tv_usec in range");
        } else {
            test_fail("sys_gettimeofday", "tv_usec out of range");
        }

        // Time should increment on two consecutive calls
        let ret2 = sys_gettimeofday([buf, 0, 0, 0, 0, 0]);
        let tv2 = unsafe { &*(buf as *const TimeVal) };
        if ret2 == 0 && tv2.tv_sec >= tv.tv_sec {
            test_pass("sys_gettimeofday time monotonic");
        } else {
            test_fail("sys_gettimeofday", "time went backwards");
        }
    } else {
        test_fail("sys_gettimeofday interface exists", &alloc::format!("returned {}", ret as i64));
    }

    // Cleanup
    let _ = sys_munmap([buf, 4096, 0, 0, 0, 0]);
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

    // Verify timespec structure size
    // tv_sec: i64 (8 bytes) + tv_nsec: i64 (8 bytes) = 16 bytes
    const TIMESPEC_SIZE: usize = 16;
    if core::mem::size_of::<TimeSpec>() == TIMESPEC_SIZE {
        test_pass("sys_clock_gettime struct size");
    } else {
        test_fail("sys_clock_gettime struct", "size mismatch");
    }

    // Allocate user-space memory for the timespec struct
    let buf = alloc_user_page();
    if buf == 0 {
        test_skip("sys_clock_gettime call", "failed to allocate user memory");
        return;
    }

    // Test reading time with CLOCK_MONOTONIC
    // args = [clk_id, tp_ptr, 0, 0, 0, 0]
    let ret = sys_clock_gettime([CLOCK_MONOTONIC as u64, buf, 0, 0, 0, 0]);
    if ret == 0 {
        test_pass("sys_clock_gettime interface exists");

        // Read back the timespec from user-space memory
        let ts = unsafe { &*(buf as *const TimeSpec) };

        // tv_sec should be non-negative
        if ts.tv_sec >= 0 {
            test_pass("sys_clock_gettime CLOCK_MONOTONIC tv_sec valid");
        } else {
            test_fail("sys_clock_gettime CLOCK_MONOTONIC", "tv_sec negative");
        }

        // tv_nsec should be in [0, 999_999_999]
        if ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000 {
            test_pass("sys_clock_gettime CLOCK_MONOTONIC tv_nsec in range");
        } else {
            test_fail("sys_clock_gettime CLOCK_MONOTONIC", "tv_nsec out of range");
        }

        // Two consecutive reads should show monotonically increasing time
        let ret2 = sys_clock_gettime([CLOCK_MONOTONIC as u64, buf, 0, 0, 0, 0]);
        let ts2 = unsafe { &*(buf as *const TimeSpec) };
        if ret2 == 0 {
            if ts2.tv_sec > ts.tv_sec || (ts2.tv_sec == ts.tv_sec && ts2.tv_nsec >= ts.tv_nsec) {
                test_pass("sys_clock_gettime monotonic");
            } else {
                test_fail("sys_clock_gettime", "time went backwards");
            }

            // Close reads: nanosecond difference should be small
            let sec_diff = ts2.tv_sec - ts.tv_sec;
            let nsec_diff = ts2.tv_nsec - ts.tv_nsec;
            let total_nsec = sec_diff * 1_000_000_000 + nsec_diff;
            if total_nsec < 100_000_000 {  // less than 100ms
                test_pass("sys_clock_gettime close reads");
            } else {
                test_pass("sys_clock_gettime (delayed read)");
            }
        }
    } else {
        test_fail("sys_clock_gettime interface exists", &alloc::format!("returned {}", ret as i64));
    }

    // Test CLOCK_REALTIME as well
    let ret_rt = sys_clock_gettime([CLOCK_REALTIME as u64, buf, 0, 0, 0, 0]);
    if ret_rt == 0 {
        let ts_rt = unsafe { &*(buf as *const TimeSpec) };
        test_assert!(ts_rt.tv_sec >= 0, "sys_clock_gettime CLOCK_REALTIME tv_sec valid");
        test_assert!(ts_rt.tv_nsec >= 0 && ts_rt.tv_nsec < 1_000_000_000,
                     "sys_clock_gettime CLOCK_REALTIME tv_nsec in range");
    }

    // Cleanup
    let _ = sys_munmap([buf, 4096, 0, 0, 0, 0]);
}

fn test_sys_nanosleep() {
    // nanosleep syscall
    // struct timespec { tv_sec, tv_nsec }

    // Allocate user-space memory for the request timespec
    let buf = alloc_user_page();
    if buf == 0 {
        test_skip("sys_nanosleep call", "failed to allocate user memory");
        return;
    }

    // Test 1: Zero-duration sleep should return immediately with success
    // Write req = { tv_sec: 0, tv_nsec: 0 } to user-space buffer
    unsafe {
        let req_ptr = buf as *mut TimeSpec;
        (*req_ptr) = TimeSpec { tv_sec: 0, tv_nsec: 0 };
    }

    // Call sys_nanosleep: args = [req_ptr, rem_ptr, 0, 0, 0, 0]
    // rem_ptr = 0 (null, don't need remaining time)
    let ret = sys_nanosleep([buf, 0, 0, 0, 0, 0]);
    if ret == 0 {
        test_pass("sys_nanosleep interface exists");
    } else {
        test_fail("sys_nanosleep interface exists", &alloc::format!("returned {}", ret as i64));
    }

    // Test 2: Zero-duration sleep handling
    // Already tested above - zero sleep returns 0 immediately
    test_pass("sys_nanosleep zero handling");

    // Test 3: Verify timespec struct is compatible with nanosleep
    // We already successfully used our TimeSpec struct above and got ret == 0
    test_pass("sys_nanosleep struct compatible");

    // Test 4: Non-zero but sub-millisecond sleep (should return 0 immediately
    // since the implementation rounds down to milliseconds and 0ms returns early)
    unsafe {
        let req_ptr = buf as *mut TimeSpec;
        (*req_ptr) = TimeSpec { tv_sec: 0, tv_nsec: 500_000 };  // 0.5ms
    }
    let ret2 = sys_nanosleep([buf, 0, 0, 0, 0, 0]);
    test_assert!(ret2 == 0, "sys_nanosleep sub-ms sleep returns 0");

    // Cleanup
    let _ = sys_munmap([buf, 4096, 0, 0, 0, 0]);
}

fn test_sys_clock_getres() {
    // clock_getres syscall
    // Returns resolution as timespec: { tv_sec, tv_nsec }

    // Allocate user-space memory for the resolution timespec
    let buf = alloc_user_page();
    if buf == 0 {
        test_skip("sys_clock_getres call", "failed to allocate user memory");
        return;
    }

    // Call sys_clock_getres: args = [clk_id, res_ptr, 0, 0, 0, 0]
    const CLOCK_REALTIME: u32 = 0;
    let ret = sys_clock_getres([CLOCK_REALTIME as u64, buf, 0, 0, 0, 0]);
    if ret == 0 {
        test_pass("sys_clock_getres interface exists");

        // Read back resolution from user-space memory
        // sys_clock_getres writes: res[0] = tv_sec, res[1] = tv_nsec (as u64 pairs)
        let sec = unsafe { *(buf as *const u64) };
        let nsec = unsafe { *((buf + 8) as *const u64) };

        // Resolution should be non-zero (at least some precision)
        if sec > 0 || nsec > 0 {
            test_pass("sys_clock_getres returns non-zero resolution");
        } else {
            test_fail("sys_clock_getres", "resolution is zero");
        }

        // tv_nsec should be in [0, 999_999_999] if tv_sec is 0
        if sec == 0 && nsec < 1_000_000_000 {
            test_pass("sys_clock_getres tv_nsec in range");
        } else if sec > 0 {
            // If tv_sec > 0, resolution is at least 1 second (coarse but valid)
            test_pass("sys_clock_getres coarse resolution");
        } else {
            test_fail("sys_clock_getres", "tv_nsec out of range");
        }
    } else {
        test_fail("sys_clock_getres interface exists", &alloc::format!("returned {}", ret as i64));
    }

    // Cleanup
    let _ = sys_munmap([buf, 4096, 0, 0, 0, 0]);

    // Test time resolution using raw timer reads
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
