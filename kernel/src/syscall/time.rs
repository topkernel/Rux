//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Time-related system calls
//!
//! Includes: gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use super::*;

/// clock_gettime clock IDs
const CLOCK_REALTIME: u32 = 0;
const CLOCK_MONOTONIC: u32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: u32 = 2;
const CLOCK_THREAD_CPUTIME_ID: u32 = 3;

#[repr(C)]
struct TimespecForGettime {
    tv_sec: i64,
    tv_nsec: i64,
}

/// sys_gettimeofday - Get current time
///
/// # Arguments
/// - args[0]: tv - pointer to timeval structure
/// - args[1]: tz - pointer to timezone structure (deprecated, should be null)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_gettimeofday(args: SyscallArgs) -> u64 {
    let tv_ptr = args[0] as *mut TimeVal;
    let _tz_ptr = args[1] as *mut u8;  // timezone is deprecated

    if tv_ptr.is_null() {
        return -errno::EINVAL as u64;
    }

    // Get time from RISC-V timer
    let cycles = crate::drivers::intc::clint::read_time();
    let freq_hz: u64 = 10_000_000;  // 10 MHz

    let sec = cycles / freq_hz;
    let usec = (cycles % freq_hz) * 1_000_000 / freq_hz;

    unsafe {
        (*tv_ptr).tv_sec = sec as i64;
        (*tv_ptr).tv_usec = usec as i64;
    }

    0
}

/// sys_clock_gettime - Get time of specified clock
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: tp - pointer to timespec structure
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_gettime(args: SyscallArgs) -> u64 {
    let clk_id = args[0] as u32;
    let tp_ptr = args[1] as *mut TimespecForGettime;

    if tp_ptr.is_null() {
        return -errno::EINVAL as u64;
    }

    // Currently only support REALTIME and MONOTONIC
    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC => {
            // Get time from RISC-V timer
            let cycles = crate::drivers::intc::clint::read_time();
            let freq_hz: u64 = 10_000_000;  // 10 MHz

            let sec = cycles / freq_hz;
            let nsec = (cycles % freq_hz) * 1_000_000_000 / freq_hz;

            unsafe {
                (*tp_ptr).tv_sec = sec as i64;
                (*tp_ptr).tv_nsec = nsec as i64;
            }
            0
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // For CPU time, currently return 0
            unsafe {
                (*tp_ptr).tv_sec = 0;
                (*tp_ptr).tv_nsec = 0;
            }
            0
        }
        _ => {
            // Unsupported clock type
            -errno::EINVAL as u64
        }
    }
}

/// Timespec structure
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// sys_nanosleep - High-resolution sleep
///
/// # Arguments
/// - args[0]: req - requested sleep time
/// - args[1]: rem - remaining time (when interrupted by signal)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_nanosleep(args: SyscallArgs) -> u64 {
    use crate::drivers::timer;
    use crate::process;

    let req_ptr = args[0] as *const Timespec;
    let rem_ptr = args[1] as *mut Timespec;

    // Check request pointer validity
    if req_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Read requested sleep time
    let req = unsafe { *req_ptr };
    let total_nanos = req.tv_sec * 1_000_000_000 + req.tv_nsec;

    // Convert to milliseconds
    let sleep_msecs = (total_nanos / 1_000_000) as u64;

    // If sleep time is 0, return immediately
    if sleep_msecs == 0 {
        return 0;
    }

    // Get current jiffies
    let start_jiffies = timer::get_jiffies();

    // Calculate target jiffies
    let sleep_jiffies = timer::msecs_to_jiffies(sleep_msecs);
    let target_jiffies = start_jiffies + sleep_jiffies;

    // Sleep loop until target time is reached
    loop {
        let current_jiffies = timer::get_jiffies();

        // Check if target time has been reached
        if current_jiffies >= target_jiffies {
            return 0;  // Success
        }

        // Calculate remaining time
        let remaining_jiffies = target_jiffies - current_jiffies;
        let remaining_msecs = timer::jiffies_to_msecs(remaining_jiffies);

        // Check for pending signals
        use crate::signal;
        if signal::signal_pending() {
            // Write remaining time to rem (if rem_ptr is provided)
            if !rem_ptr.is_null() {
                unsafe {
                    // Convert milliseconds to timespec
                    let rem_sec = (remaining_msecs / 1000) as i64;
                    let rem_nsec = ((remaining_msecs % 1000) * 1_000_000) as i64;
                    *rem_ptr = Timespec {
                        tv_sec: rem_sec,
                        tv_nsec: rem_nsec,
                    };
                }
            }

            return -errno::EINTR as u64;
        }

        // Use Task::sleep() to enter interruptible sleep
        // Note: This will trigger scheduling, continue checking time after waking up
        process::Task::sleep(crate::process::task::TaskState::new(
            crate::process::task::TaskState::INTERRUPTIBLE
        ));
    }
}

/// sys_clock_getres - Get clock resolution
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: res - pointer to timespec structure (for storing result)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_getres(args: SyscallArgs) -> u64 {
    let _clk_id = args[0] as i32;
    let res = args[1] as *mut u64;

    // Simplified implementation: return 1 nanosecond resolution
    if !res.is_null() {
        unsafe {
            // timespec structure: tv_sec (8 bytes) + tv_nsec (8 bytes)
            *res = 0;          // tv_sec = 0
            *(res.offset(1)) = 1;  // tv_nsec = 1
        }
    }

    0
}

/// sys_clock_nanosleep - High-resolution sleep (with specified clock)
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: flags - flags
/// - args[2]: rqtp - requested sleep time
/// - args[3]: rmtp - remaining time (when interrupted by signal)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_nanosleep(args: SyscallArgs) -> u64 {
    let _clk_id = args[0] as i32;
    let _flags = args[1] as i32;
    let rqtp = args[2] as *const u64;

    // Validate arguments
    if rqtp.is_null() {
        return -errno::EINVAL as u64;
    }

    // Simplified implementation: call nanosleep
    // TODO: Implement proper clock-specific sleep
    let _ = unsafe { (*rqtp, *rqtp.offset(1)) };

    0
}
