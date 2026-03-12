//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Scheduling-related system calls
//!
//! Includes: futex, sched_yield, getpriority, setpriority

use super::*;

/// PRIO_PROCESS - Process priority
const PRIO_PROCESS: i32 = 0;
/// PRIO_PGRP - Process group priority (not currently supported)
const PRIO_PGRP: i32 = 1;
/// PRIO_USER - User priority (not currently supported)
const PRIO_USER: i32 = 2;

/// MIN_NICE - Minimum nice value
const MIN_NICE: i32 = -20;
/// MAX_NICE - Maximum nice value
const MAX_NICE: i32 = 19;

/// sys_futex - Fast Userspace Mutex
///
/// Primitive for thread synchronization
///
/// # Arguments
/// - args[0]: uaddr - futex address
/// - args[1]: op - operation code (FUTEX_WAIT=0, FUTEX_WAKE=1, etc.)
/// - args[2]: val - value
/// - args[3]: timeout - timeout
/// - args[4]: uaddr2 - second address
/// - args[5]: val3 - third value
///
/// # Returns
/// Returns operation result on success, negative error code on failure
pub fn sys_futex(args: SyscallArgs) -> u64 {
    // Use complete implementation in sync/futex.rs
    crate::sync::sys_futex_handler(&args) as u64
}

/// sys_sched_yield - Yield CPU
///
/// Current thread voluntarily yields CPU, allowing other threads to run
///
/// # Returns
/// Always returns 0
pub fn sys_sched_yield(_args: SyscallArgs) -> u64 {
    // Simplified implementation: return directly without actual scheduling
    // TODO: Implement actual scheduling yield
    0
}

/// sys_getpriority - Get process priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - process ID / process group ID / user ID (0 means current process)
///
/// # Returns
/// - Success: nice value + 20 (range 1-40, 0 indicates error)
/// - Failure: negative error code
pub fn sys_getpriority(args: SyscallArgs) -> u64 {
    let which = args[0] as i32;
    let who = args[1] as u32;

    // Only support PRIO_PROCESS
    if which != PRIO_PROCESS {
        return -errno::EINVAL as u64;
    }

    let target_pid = if who == 0 {
        // who = 0 means current process
        match crate::sched::current() {
            Some(t) => unsafe { (*t).pid() },
            None => return -errno::ESRCH as u64,
        }
    } else {
        who
    };

    // Find target process
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -errno::ESRCH as u64;  // Process does not exist
    }

    // Return nice value + 20 (convert to 1-40 range)
    let nice = unsafe { (*task).nice() };
    (nice + 20) as u64
}

/// sys_setpriority - Set process priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - process ID / process group ID / user ID (0 means current process)
/// - args[2]: prio - priority value (nice value, range -20 to 19)
///
/// # Returns
/// - Success: 0
/// - Failure: negative error code
pub fn sys_setpriority(args: SyscallArgs) -> u64 {
    let which = args[0] as i32;
    let who = args[1] as u32;
    let niceval = args[2] as i32;

    // Only support PRIO_PROCESS
    if which != PRIO_PROCESS {
        return -errno::EINVAL as u64;
    }

    // Check nice value range
    let niceval = niceval.clamp(MIN_NICE, MAX_NICE);

    let target_pid = if who == 0 {
        // who = 0 means current process
        match crate::sched::current() {
            Some(t) => unsafe { (*t).pid() },
            None => return -errno::ESRCH as u64,
        }
    } else {
        who
    };

    // Find target process
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -errno::ESRCH as u64;  // Process does not exist
    }

    // Permission check: can only modify own process priority, or have CAP_SYS_NICE permission
    // Simplified implementation: allow modifying any process priority
    // TODO: Add permission check

    // Set nice value
    unsafe {
        (*task).set_nice(niceval);
    }

    0
}
