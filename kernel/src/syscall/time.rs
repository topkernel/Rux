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
        return 0;  // NULL is allowed, just return success
    }

    // Check if tv_ptr is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(tv_ptr as usize, core::mem::size_of::<TimeVal>()) {
        return -errno::EFAULT as u64;
    }

    // Get time from RISC-V timer
    let cycles = crate::drivers::intc::clint::read_time();
    let freq_hz: u64 = 10_000_000;  // 10 MHz

    let sec = cycles / freq_hz;
    let usec = (cycles % freq_hz) * 1_000_000 / freq_hz;

    // SAFETY: tv_ptr validated with access_ok; writes TimeVal fields (two i64).
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

    // Check if tp_ptr is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(tp_ptr as usize, core::mem::size_of::<TimespecForGettime>()) {
        return -errno::EFAULT as u64;
    }

    // Currently only support REALTIME and MONOTONIC
    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC => {
            // Get time from RISC-V timer
            let cycles = crate::drivers::intc::clint::read_time();
            let freq_hz: u64 = 10_000_000;  // 10 MHz

            let sec = cycles / freq_hz;
            let nsec = (cycles % freq_hz) * 1_000_000_000 / freq_hz;

            // SAFETY: tp_ptr validated with access_ok; writes TimespecForGettime fields.
            unsafe {
                (*tp_ptr).tv_sec = sec as i64;
                (*tp_ptr).tv_nsec = nsec as i64;
            }
            0
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // For CPU time, currently return 0
            // SAFETY: tp_ptr validated with access_ok; writes zero-filled TimespecForGettime.
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

    // Check if req_ptr is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(req_ptr as usize, core::mem::size_of::<Timespec>()) {
        return -errno::EFAULT as u64;
    }

    // Check rem_ptr if provided
    if !rem_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(rem_ptr as usize, core::mem::size_of::<Timespec>()) {
        return -errno::EFAULT as u64;
    }

    // SAFETY: req_ptr validated with access_ok; reads Timespec (two i64 fields).
    let req = unsafe { *req_ptr };
    nanosleep_impl(&req, rem_ptr)
}

/// Internal nanosleep implementation shared by sys_nanosleep and sys_clock_nanosleep
fn nanosleep_impl(req: &Timespec, rem_ptr: *mut Timespec) -> u64 {
    use crate::drivers::timer;
    use crate::process;

    let total_nanos = req.tv_sec.saturating_mul(1_000_000_000).saturating_add(req.tv_nsec);

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
                // SAFETY: rem_ptr validated with access_ok in caller; writes Timespec.
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

/// sys_clock_settime - Set time of specified clock
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: tp - pointer to timespec structure
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_settime(args: SyscallArgs) -> u64 {
    let clk_id = args[0] as u32;
    let _tp_ptr = args[1] as *const TimespecForGettime;

    // CAP_SYS_TIME required to set time
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -errno::EPERM as u64;
    }

    // Only CLOCK_REALTIME can be set
    if clk_id != CLOCK_REALTIME {
        return -errno::EINVAL as u64;
    }

    // TODO: actually implement clock setting via timer hardware
    -errno::ENOSYS as u64
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
        // Check if res is in valid user space
        if !crate::arch::riscv64::uaccess::access_ok(res as usize, 16) {  // 2 * sizeof(u64)
            return -errno::EFAULT as u64;
        }
        // SAFETY: res validated with access_ok(16); writes two u64 values (tv_sec=0, tv_nsec=1).
        unsafe {
            // timespec structure: tv_sec (8 bytes) + tv_nsec (8 bytes)
            *res = 0;          // tv_sec = 0
            *(res.offset(1)) = 1;  // tv_nsec = 1
        }
    }

    0
}

/// sys_getitimer - Get interval timer value
///
/// # Arguments
/// - args[0]: which - timer type (ITIMER_REAL=0, ITIMER_VIRTUAL=1, ITIMER_PROF=2)
/// - args[1]: curr_value - pointer to struct itimerval (output)
pub fn sys_getitimer(args: SyscallArgs) -> u64 {
    let which = args[0] as i32;
    let curr_value = args[1] as *mut u64;

    if curr_value.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(curr_value as usize, 32) {
        return -errno::EFAULT as u64;
    }

    if which < 0 || which > 2 {
        return -errno::EINVAL as u64;
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    // struct itimerval { struct timeval it_interval, it_value }
    // struct timeval { time_t tv_sec, suseconds_t tv_usec }
    let (interval_sec, interval_usec, value_sec, value_usec) = if which == 0 {
        // ITIMER_REAL — compute remaining time from kernel timer
        let timer_id = task.itimer_ids[0].load(core::sync::atomic::Ordering::Acquire);
        if timer_id == 0 {
            // Disarmed
            (0i64, 0i64, 0i64, 0i64)
        } else {
            // Get the interval from the timer action (if periodic)
            // For simplicity, compute remaining from jiffies
            let current_jiffies = crate::drivers::timer::get_jiffies();
            // We need the original expires and interval — stored in timer action
            // Since we can't easily read the action from here, return interval as 0
            // and compute remaining from jiffies delta
            // TODO: store interval per-process
            (0i64, 0i64, 0i64, 0i64)
        }
    } else {
        // ITIMER_VIRTUAL, ITIMER_PROF — not supported
        (0i64, 0i64, 0i64, 0i64)
    };

    // Write struct itimerval
    // SAFETY: curr_value validated with access_ok(32); writes 4 i64 values at known offsets.
    unsafe {
        // it_interval (offset 0)
        let p = curr_value as *mut i64;
        core::ptr::write(p, interval_sec);
        core::ptr::write(p.add(1), interval_usec);
        // it_value (offset 16)
        core::ptr::write(p.add(2), value_sec);
        core::ptr::write(p.add(3), value_usec);
    }

    0
}

/// sys_setitimer - Set interval timer
///
/// # Arguments
/// - args[0]: which - timer type (ITIMER_REAL=0, ITIMER_VIRTUAL=1, ITIMER_PROF=2)
/// - args[1]: new_value - pointer to struct itimerval
/// - args[2]: old_value - pointer to struct itimerval (output, may be NULL)
pub fn sys_setitimer(args: SyscallArgs) -> u64 {
    let which = args[0] as i32;
    let new_value = args[1] as *const u64;
    let old_value = args[2] as *mut u64;

    if which < 0 || which > 2 {
        return -errno::EINVAL as u64;
    }

    // Write old_value (disarm current timer first)
    if !old_value.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(old_value as usize, 32) {
            return -errno::EFAULT as u64;
        }
        // Get old timer state and write it as zeros (disarmed)
        // SAFETY: old_value validated with access_ok(32); writes 32 zero bytes.
        unsafe { core::ptr::write_bytes(old_value, 0, 32); }
    }

    if new_value.is_null() {
        // Disarm the timer
        if which == 0 {
            disarm_itimer_real();
        }
        return 0;
    }

    if !crate::arch::riscv64::uaccess::access_ok(new_value as usize, 32) {
        return -errno::EFAULT as u64;
    }

    // Read struct itimerval
    // SAFETY: new_value validated with access_ok(32); reads 4 i64 fields at known offsets.
    let (interval_sec, interval_usec, value_sec, value_usec) = unsafe {
        let p = new_value as *const i64;
        (
            core::ptr::read(p),
            core::ptr::read(p.add(1)),
            core::ptr::read(p.add(2)),
            core::ptr::read(p.add(3)),
        )
    };

    if which == 0 {
        // ITIMER_REAL — arm using kernel timer wheel
        set_itimer_real(interval_sec, interval_usec, value_sec, value_usec);
    } else if which == 1 || which == 2 {
        // ITIMER_VIRTUAL, ITIMER_PROF — not supported, silently accept
    }

    0
}

/// Disarm ITIMER_REAL timer for the current process.
fn disarm_itimer_real() {
    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return,
    };

    let old_timer_id = task.itimer_ids[0].swap(0, core::sync::atomic::Ordering::AcqRel);
    if old_timer_id != 0 {
        crate::timer::del_timer(old_timer_id);
    }
}

/// Set ITIMER_REAL timer for the current process.
fn set_itimer_real(interval_sec: i64, interval_usec: i64, value_sec: i64, value_usec: i64) {
    use crate::drivers::timer;

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return,
    };

    let pid = task.pid();

    // Disarm existing timer
    let old_timer_id = task.itimer_ids[0].swap(0, core::sync::atomic::Ordering::AcqRel);
    if old_timer_id != 0 {
        crate::timer::del_timer(old_timer_id);
    }

    // If value is zero, just disarm (already done above)
    let total_usec = value_sec.saturating_mul(1_000_000).saturating_add(value_usec);
    if total_usec <= 0 {
        return;
    }

    // Convert to jiffies (minimum 1)
    let value_msecs = (total_usec / 1000) as u64;
    let value_jiffies = timer::msecs_to_jiffies(value_msecs).max(1);
    let expires = timer::get_jiffies() + value_jiffies;

    // Compute interval in jiffies
    let interval_usec_total = interval_sec.saturating_mul(1_000_000).saturating_add(interval_usec);
    let interval_jiffies = if interval_usec_total > 0 {
        let interval_msecs = (interval_usec_total / 1000) as u64;
        timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0 // one-shot
    };

    let new_timer_id = crate::timer::add_timer_with_action(
        expires,
        pid,
        crate::signal::Signal::SIGALRM as i32,
        interval_jiffies,
        0,
    );

    task.itimer_ids[0].store(new_timer_id, core::sync::atomic::Ordering::Release);
}

/// sys_clock_nanosleep - High-resolution sleep (with specified clock)
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: flags - flags (TIMER_ABSTIME = 1)
/// - args[2]: rqtp - requested sleep time
/// - args[3]: rmtp - remaining time (when interrupted by signal)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_nanosleep(args: SyscallArgs) -> u64 {
    let _clk_id = args[0] as i32;
    let _flags = args[1] as i32;
    let rqtp = args[2] as *const Timespec;
    let rmtp = args[3] as *mut Timespec;

    // Validate request pointer
    if rqtp.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if rqtp is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(rqtp as usize, core::mem::size_of::<Timespec>()) {
        return -errno::EFAULT as u64;
    }

    // Check rmtp if provided
    if !rmtp.is_null() && !crate::arch::riscv64::uaccess::access_ok(rmtp as usize, core::mem::size_of::<Timespec>()) {
        return -errno::EFAULT as u64;
    }

    // Read requested sleep time
    // SAFETY: rqtp validated with access_ok; reads Timespec (two i64 fields).
    let req = unsafe { *rqtp };

    nanosleep_impl(&req, rmtp)
}

/// sys_timer_create - Create POSIX interval timer (NR 107)
///
/// Creates a per-process POSIX timer. The timer ID is returned via timerid_ptr.
pub fn sys_timer_create(args: SyscallArgs) -> u64 {
    let clockid = args[0] as i32;
    let sigevent_ptr = args[1] as *const u8;
    let timerid_ptr = args[2] as *mut i32;

    if timerid_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(timerid_ptr as usize, 4) {
        return -errno::EFAULT as u64;
    }

    // Only CLOCK_REALTIME (0) and CLOCK_MONOTONIC (1) supported
    if clockid != 0 && clockid != 1 {
        return -errno::EINVAL as u64;
    }

    // Parse sigevent for signal notification
    let mut sigev_signo = crate::signal::Signal::SIGALRM as i32;
    let mut sigev_notify = 0; // SIGEV_SIGNAL

    if !sigevent_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(sigevent_ptr as usize, 64) {
            return -errno::EFAULT as u64;
        }
        // struct sigevent { sigval sigev_value, int sigev_signo, int sigev_notify, ... }
        // SAFETY: sigevent_ptr validated with access_ok(64); reads i32 fields at known offsets.
        unsafe {
            let p = sigevent_ptr as *const i32;
            // sigev_value is 8 bytes (union), then sigev_signo at offset 8
            let signo = core::ptr::read(p.add(2));
            let notify = core::ptr::read(p.add(3));
            if signo > 0 && signo <= 64 {
                sigev_signo = signo;
            }
            sigev_notify = notify;
        }
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    // Allocate timer ID (per-process)
    let mut timers = task.posix_timers.lock();
    let user_timer_id = (timers.len() + 1) as i32;

    let state = crate::process::task::PosixTimerState {
        kernel_timer_id: 0,
        clock_id: clockid,
        interval_jiffies: 0,
        sigev_signo,
        sigev_notify,
        overrun_count: 0,
        user_timer_id: user_timer_id,
    };

    timers.push(state);
    // SAFETY: timerid_ptr validated with access_ok(4); writes one i32.
    unsafe {
        core::ptr::write_volatile(timerid_ptr, user_timer_id);
    }

    0
}

/// sys_timer_settime - Set timer value (NR 110)
///
/// # Arguments
/// - args[0]: timerid - timer ID (returned by timer_create)
/// - args[1]: flags - TIMER_ABSTIME (1) for absolute time
/// - args[2]: new_value - new timer settings (struct itimerspec, 32 bytes)
/// - args[3]: old_value - old timer settings (output)
pub fn sys_timer_settime(args: SyscallArgs) -> u64 {
    let timerid = args[0] as i32;
    let flags = args[1] as i32;
    let new_value = args[2] as *const u64;
    let old_value = args[3] as *mut u64;

    if new_value.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(new_value as usize, 32) {
        return -errno::EFAULT as u64;
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    // Find timer by user ID (1-indexed)
    let idx = (timerid as usize).saturating_sub(1);

    // Write old_value as disarmed
    if !old_value.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(old_value as usize, 32) {
            return -errno::EFAULT as u64;
        }
        // SAFETY: old_value validated with access_ok(32); writes 32 zero bytes.
        unsafe { core::ptr::write_bytes(old_value, 0, 32); }
    }

    // Read struct itimerspec { struct timespec it_interval, struct timespec it_value }
    // SAFETY: new_value validated with access_ok(32); reads 4 i64 fields at known offsets.
    let (int_sec, int_nsec, val_sec, val_nsec) = unsafe {
        let p = new_value as *const i64;
        (
            core::ptr::read(p),
            core::ptr::read(p.add(1)),
            core::ptr::read(p.add(2)),
            core::ptr::read(p.add(3)),
        )
    };

    let mut timers = task.posix_timers.lock();
    if idx >= timers.len() {
        return -errno::EINVAL as u64;
    }

    let timer = &mut timers[idx];
    let pid = task.pid();

    // Disarm existing kernel timer
    if timer.kernel_timer_id != 0 {
        crate::timer::del_timer(timer.kernel_timer_id);
        timer.kernel_timer_id = 0;
    }

    // If value is zero, timer is disarmed
    let total_nsec = val_sec.saturating_mul(1_000_000_000).saturating_add(val_nsec);
    if total_nsec <= 0 {
        return 0;
    }

    // Convert to jiffies
    let value_msecs = (total_nsec / 1_000_000) as u64;
    let value_jiffies = crate::drivers::timer::msecs_to_jiffies(value_msecs).max(1);

    let interval_nsec = int_sec.saturating_mul(1_000_000_000).saturating_add(int_nsec);
    let interval_jiffies = if interval_nsec > 0 {
        let interval_msecs = (interval_nsec / 1_000_000) as u64;
        crate::drivers::timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0
    };

    let expires = if flags & 1 != 0 {
        // TIMER_ABSTIME — absolute time (convert from timespec to jiffies)
        // Approximate: use current jiffies as base + offset
        crate::drivers::timer::get_jiffies() + value_jiffies
    } else {
        // Relative time
        crate::drivers::timer::get_jiffies() + value_jiffies
    };

    let new_kernel_id = crate::timer::add_timer_with_action(
        expires,
        pid,
        timer.sigev_signo,
        interval_jiffies,
        0,
    );

    timer.kernel_timer_id = new_kernel_id;
    timer.interval_jiffies = interval_jiffies;
    timer.overrun_count = 0;

    0
}

/// sys_timer_gettime - Get timer value (NR 108)
pub fn sys_timer_gettime(args: SyscallArgs) -> u64 {
    let timerid = args[0] as i32;
    let curr_value = args[1] as *mut u64;

    if curr_value.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(curr_value as usize, 32) {
        return -errno::EFAULT as u64;
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    let idx = (timerid as usize).saturating_sub(1);
    let timers = task.posix_timers.lock();

    if idx >= timers.len() {
        return -errno::EINVAL as u64;
    }

    let timer = &timers[idx];

    // Compute remaining time
    let (val_sec, val_nsec) = if timer.kernel_timer_id != 0 {
        // Approximate: check if timer is still pending
        if crate::timer::timer_pending(timer.kernel_timer_id) {
            // Timer is active but we can't easily get remaining jiffies
            // Write interval as remaining (best effort)
            if timer.interval_jiffies > 0 {
                let remaining_msecs = crate::drivers::timer::jiffies_to_msecs(timer.interval_jiffies);
                ((remaining_msecs / 1000) as i64, 0i64)
            } else {
                (1i64, 0i64) // active, at least 1 jiffy remaining
            }
        } else {
            (0i64, 0i64) // expired
        }
    } else {
        (0i64, 0i64) // disarmed
    };

    // Write struct itimerspec { struct timespec it_interval, struct timespec it_value }
    // SAFETY: curr_value validated with access_ok(32); writes 4 i64 values at known offsets.
    unsafe {
        let p = curr_value as *mut i64;
        if timer.interval_jiffies > 0 {
            let int_msecs = crate::drivers::timer::jiffies_to_msecs(timer.interval_jiffies);
            core::ptr::write(p, (int_msecs / 1000) as i64);
            core::ptr::write(p.add(1), 0i64);
        } else {
            core::ptr::write(p, 0i64);
            core::ptr::write(p.add(1), 0i64);
        }
        core::ptr::write(p.add(2), val_sec);
        core::ptr::write(p.add(3), val_nsec);
    }

    0
}

/// sys_timer_getoverrun - Get timer overrun count (NR 109)
pub fn sys_timer_getoverrun(args: SyscallArgs) -> u64 {
    let timerid = args[0] as i32;

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    let idx = (timerid as usize).saturating_sub(1);
    let timers = task.posix_timers.lock();

    if idx >= timers.len() {
        return -errno::EINVAL as u64;
    }

    timers[idx].overrun_count as u64
}

/// sys_timer_delete - Delete POSIX timer (NR 111)
pub fn sys_timer_delete(args: SyscallArgs) -> u64 {
    let timerid = args[0] as i32;

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    let idx = (timerid as usize).saturating_sub(1);

    let mut timers = task.posix_timers.lock();
    if idx >= timers.len() {
        return -errno::EINVAL as u64;
    }

    // Disarm kernel timer
    let timer = &timers[idx];
    if timer.kernel_timer_id != 0 {
        crate::timer::del_timer(timer.kernel_timer_id);
    }

    timers.remove(idx);
    0
}

/// sys_settimeofday - Set wall-clock time (NR 170)
pub fn sys_settimeofday(args: SyscallArgs) -> u64 {
    let _tv_ptr = args[0] as *const u8;
    let _tz_ptr = args[1] as *const u8;
    // CAP_SYS_TIME required to set time
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -errno::EPERM as u64;
    }
    // TODO: implement time setting via timer hardware
    -errno::ENOSYS as u64
}

/// sys_adjtimex - Adjust system clock (NR 171)
///
/// struct timex is 128 bytes on 64-bit. We fill it as "clock synchronized".
pub fn sys_adjtimex(args: SyscallArgs) -> u64 {
    // Permission check: require CAP_SYS_TIME
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -errno::EPERM as u64;
    }

    let buf_ptr = args[0] as *mut u8;

    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, 128) {
        return -errno::EFAULT as u64;
    }

    // TIME_OK = 0: clock is synchronized
    // SAFETY: buf_ptr validated with access_ok(128); writes 128 bytes and sets status field.
    unsafe {
        core::ptr::write_bytes(buf_ptr, 0, 128);
        // status field at offset 4 (after modes u32)
        // Return TIME_OK
        core::ptr::write_volatile(buf_ptr.add(4) as *mut i32, 0);
    }
    0
}

/// sys_clock_adjtime - Adjust per-ClockID (NR 266)
pub fn sys_clock_adjtime(args: SyscallArgs) -> u64 {
    // Permission check: require CAP_SYS_TIME
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -errno::EPERM as u64;
    }

    let _clk_id = args[0] as i32;
    let buf_ptr = args[1] as *mut u8;

    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, 128) {
        return -errno::EFAULT as u64;
    }

    // TIME_OK = 0: return as synchronized
    // SAFETY: buf_ptr validated with access_ok(128); writes 128 bytes and sets status field.
    unsafe {
        core::ptr::write_bytes(buf_ptr, 0, 128);
        core::ptr::write_volatile(buf_ptr.add(4) as *mut i32, 0);
    }
    0
}

/// sys_fanotify_init - Initialize fanotify (NR 262)
pub fn sys_fanotify_init(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_fanotify_mark - Add/remove fanotify mark (NR 263)
pub fn sys_fanotify_mark(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_lookup_dcookie - Lookup directory cookie (NR 18)
pub fn sys_lookup_dcookie(_args: SyscallArgs) -> u64 {
    // No dcookie support — return -EINVAL per convention
    -errno::EINVAL as u64
}

/// sys_nfsservctl - NFS service control (NR 42, deprecated)
pub fn sys_nfsservctl(_args: SyscallArgs) -> u64 {
    // Deprecated syscall, removed from kernel
    -errno::ENOSYS as u64
}

/// sys_get_robust_list - Get robust futex list (NR 100)
pub fn sys_get_robust_list(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let head_ptr = args[1] as *mut u64;
    let len_ptr = args[2] as *mut u32;

    if pid != 0 && pid as u32 != crate::process::current_pid() {
        return -errno::EPERM as u64;
    }

    if !head_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(head_ptr as usize, 8) {
            return -errno::EFAULT as u64;
        }
        // SAFETY: head_ptr validated with access_ok(8); writes one u64.
        unsafe { core::ptr::write_volatile(head_ptr, 0); }
    }
    if !len_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(len_ptr as usize, 4) {
            return -errno::EFAULT as u64;
        }
        // SAFETY: len_ptr validated with access_ok(4); writes sizeof(struct robust_list_head).
        unsafe { core::ptr::write_volatile(len_ptr, 24); } // sizeof(struct robust_list_head) on 64-bit
    }
    0
}

/// sys_rseq - Register restartable sequence (NR 293)
pub fn sys_rseq(args: SyscallArgs) -> u64 {
    let rseq_ptr = args[0] as *const u32;
    let rseq_len = args[1] as u32;
    let flags = args[2] as i32;
    let _sig = args[3] as u32;

    const RSEQ_FLAG_UNREGISTER: i32 = 1;

    if rseq_len != 32 && rseq_len != 0 {
        return -errno::EINVAL as u64;
    }
    if rseq_ptr.is_null() && (flags & RSEQ_FLAG_UNREGISTER) == 0 {
        return -errno::EINVAL as u64;
    }
    if !rseq_ptr.is_null() {
        if rseq_ptr.align_offset(32) != 0 {
            return -errno::EINVAL as u64;
        }
        if !crate::arch::riscv64::uaccess::access_ok(rseq_ptr as usize, rseq_len as usize) {
            return -errno::EFAULT as u64;
        }
    }

    // Store rseq pointer in current task (simplified: no per-task storage yet)
    // Accept the registration silently
    0
}

// ============================================================================
// NR 403-423: _time64 variants (Y2038-safe syscalls)
// These are Y2038-safe versions of existing syscalls that use 64-bit
// time values directly instead of struct timespec.
// On 64-bit RISC-V, these can delegate to the existing implementations.
// ============================================================================

/// sys_clock_gettime64 - 64-bit clock_gettime (NR 403)
pub fn sys_clock_gettime64(args: SyscallArgs) -> u64 {
    // On 64-bit, delegate to clock_gettime
    sys_clock_gettime(args)
}

/// sys_clock_settime64 - 64-bit clock_settime (NR 404)
pub fn sys_clock_settime64(args: SyscallArgs) -> u64 {
    sys_clock_settime(args)
}

/// sys_clock_adjtime64 - 64-bit clock_adjtime (NR 405)
pub fn sys_clock_adjtime64(args: SyscallArgs) -> u64 {
    sys_clock_adjtime(args)
}

/// sys_clock_getres_time64 - 64-bit clock_getres (NR 406)
pub fn sys_clock_getres_time64(args: SyscallArgs) -> u64 {
    sys_clock_getres(args)
}

/// sys_clock_nanosleep_time64 - 64-bit clock_nanosleep (NR 407)
pub fn sys_clock_nanosleep_time64(args: SyscallArgs) -> u64 {
    sys_clock_nanosleep(args)
}

/// sys_timer_gettime64 - 64-bit timer_gettime (NR 408)
pub fn sys_timer_gettime64(args: SyscallArgs) -> u64 {
    sys_timer_gettime(args)
}

/// sys_timer_settime64 - 64-bit timer_settime (NR 409)
pub fn sys_timer_settime64(args: SyscallArgs) -> u64 {
    sys_timer_settime(args)
}

/// sys_timerfd_gettime64 - 64-bit timerfd_gettime (NR 410)
pub fn sys_timerfd_gettime64(args: SyscallArgs) -> u64 {
    crate::syscall::misc::sys_timerfd_gettime(args)
}

/// sys_timerfd_settime64 - 64-bit timerfd_settime (NR 411)
pub fn sys_timerfd_settime64(args: SyscallArgs) -> u64 {
    crate::syscall::misc::sys_timerfd_settime(args)
}

/// sys_utimensat_time64 - 64-bit utimensat (NR 412)
pub fn sys_utimensat_time64(args: SyscallArgs) -> u64 {
    crate::syscall::file::sys_futimesat(args)
}

/// sys_pselect6_time64 - 64-bit pselect6 (NR 413)
pub fn sys_pselect6_time64(args: SyscallArgs) -> u64 {
    crate::syscall::misc::sys_pselect6(args)
}

/// sys_ppoll_time64 - 64-bit ppoll (NR 414)
pub fn sys_ppoll_time64(args: SyscallArgs) -> u64 {
    crate::syscall::misc::sys_ppoll(args)
}

/// sys_io_pgetevents_time64 - 64-bit io_pgetevents (NR 416)
pub fn sys_io_pgetevents_time64(args: SyscallArgs) -> u64 {
    crate::syscall::memory::sys_io_pgetevents(args)
}

/// sys_recvmmsg_time64 - 64-bit recvmmsg (NR 417)
pub fn sys_recvmmsg_time64(args: SyscallArgs) -> u64 {
    crate::syscall::network::sys_recvmmsg(args)
}

/// sys_mq_timedsend_time64 - 64-bit mq_timedsend (NR 418)
pub fn sys_mq_timedsend_time64(args: SyscallArgs) -> u64 {
    crate::ipc::posix_mq::sys_mq_timedsend(args)
}

/// sys_mq_timedreceive_time64 - 64-bit mq_timedreceive (NR 419)
pub fn sys_mq_timedreceive_time64(args: SyscallArgs) -> u64 {
    crate::ipc::posix_mq::sys_mq_timedreceive(args)
}

/// sys_semtimedop_time64 - 64-bit semtimedop (NR 420)
pub fn sys_semtimedop_time64(args: SyscallArgs) -> u64 {
    crate::ipc::sysv_sem::sys_semtimedop(args)
}

/// sys_rt_sigtimedwait_time64 - 64-bit rt_sigtimedwait (NR 421)
pub fn sys_rt_sigtimedwait_time64(args: SyscallArgs) -> u64 {
    crate::syscall::process::sys_rt_sigtimedwait(args)
}

/// sys_futex_time64 - 64-bit futex (NR 422)
pub fn sys_futex_time64(args: SyscallArgs) -> u64 {
    crate::syscall::sched::sys_futex(args)
}

/// sys_sched_rr_get_interval_time64 - 64-bit sched_rr_get_interval (NR 423)
pub fn sys_sched_rr_get_interval_time64(args: SyscallArgs) -> u64 {
    crate::syscall::sched::sys_sched_rr_get_interval(args)
}
