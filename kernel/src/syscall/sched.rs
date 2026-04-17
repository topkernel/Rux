//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Scheduling-related system calls
//!
//! Includes: futex, sched_yield, getpriority, setpriority

use super::*;

/// PRIO_PROCESS - Process priority
pub const PRIO_PROCESS: i32 = 0;
/// PRIO_PGRP - Process group priority (not currently supported)
pub const PRIO_PGRP: i32 = 1;
/// PRIO_USER - User priority (not currently supported)
pub const PRIO_USER: i32 = 2;

/// MIN_NICE - Minimum nice value
pub const MIN_NICE: i32 = -20;
/// MAX_NICE - Maximum nice value
pub const MAX_NICE: i32 = 19;

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
pub fn sys_futex(args: SyscallArgs) -> i64 {
    // Use complete implementation in sync/futex.rs
    crate::sync::sys_futex_handler(&args) as i64
}

/// sys_sched_yield - Yield CPU
///
/// Current thread voluntarily yields CPU, allowing other threads to run
///
/// # Returns
/// Always returns 0
pub fn sys_sched_yield(_args: SyscallArgs) -> i64 {
    crate::sched::yield_cpu();
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
pub fn sys_getpriority(args: SyscallArgs) -> i64 {
    let which = args[0] as i32;
    let who = args[1] as u32;

    // Only support PRIO_PROCESS
    if which != PRIO_PROCESS {
        return -(errno::EINVAL as i64);
    }

    let target_pid = if who == 0 {
        // who = 0 means current process
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        who
    };

    // Find target process
    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);  // Process does not exist
    }

    // Return nice value + 20 (convert to 1-40 range)
    // SAFETY: task is validated non-null above; nice() reads the task's nice field.
    let nice = unsafe { (*task).nice() };
    (nice + 20) as i64
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
pub fn sys_setpriority(args: SyscallArgs) -> i64 {
    let which = args[0] as i32;
    let who = args[1] as u32;
    let niceval = args[2] as i32;

    // Only support PRIO_PROCESS
    if which != PRIO_PROCESS {
        return -(errno::EINVAL as i64);
    }

    // Check nice value range
    let niceval = niceval.clamp(MIN_NICE, MAX_NICE);

    let target_pid = if who == 0 {
        // who = 0 means current process
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        who
    };

    // Find target process
    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);  // Process does not exist
    }

    // Permission check: require CAP_SYS_NICE to change another process's priority
    let current_pid = crate::process::current_pid();
    if target_pid != current_pid {
        if !crate::security::capable(crate::security::CAP_SYS_NICE) {
            return -(errno::EPERM as i64);
        }
    }

    // Set nice value
    // SAFETY: task is validated non-null above; set_nice only writes to task's nice field.
    unsafe {
        (*task).set_nice(niceval);
    }

    0
}

// ============================================================================
// Real-Time and Deadline Scheduling Syscalls
// ============================================================================

/// Scheduling policies
pub const SCHED_NORMAL: i32 = 0;
pub const SCHED_FIFO: i32 = 1;
pub const SCHED_RR: i32 = 2;
pub const SCHED_BATCH: i32 = 3;
pub const SCHED_IDLE: i32 = 5;
pub const SCHED_DEADLINE: i32 = 6;

/// struct sched_param for RT scheduling
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SchedParam {
    pub sched_priority: i32,
}

/// struct sched_attr for deadline scheduling
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SchedAttr {
    size: u32,
    sched_policy: u32,
    sched_flags: u64,
    sched_nice: i32,
    sched_priority: u32,
    sched_runtime: u64,
    sched_deadline: u64,
    sched_period: u64,
    sched_util_min: u32,
    sched_util_max: u32,
}

/// sys_sched_setscheduler - Set scheduling policy and parameters
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: policy - scheduling policy
/// - args[2]: param - pointer to sched_param
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn sys_sched_setscheduler(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let policy = args[1] as i32;
    let param_ptr = args[2] as *const SchedParam;

    // Validate policy
    if !matches!(policy, SCHED_NORMAL | SCHED_FIFO | SCHED_RR | SCHED_BATCH | SCHED_IDLE | SCHED_DEADLINE) {
        return -(errno::EINVAL as i64);
    }

    // Read param from userspace
    if param_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(param_ptr as usize, core::mem::size_of::<SchedParam>()) {
        return -(errno::EFAULT as i64);
    }
    let mut param = core::mem::MaybeUninit::<SchedParam>::uninit();
    // SAFETY: param_ptr is access_ok-validated; SchedParam is repr(C) plain data.
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(
            param.as_mut_ptr() as *mut u8,
            param_ptr as *const u8,
            core::mem::size_of::<SchedParam>(),
        )
    };
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }
    let param = unsafe { param.assume_init() };

    // Find target task
    let target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // Permission check: real-time policies require CAP_SYS_NICE for other processes
    if (policy == SCHED_FIFO || policy == SCHED_RR) && target_pid != crate::process::current_pid() {
        if !crate::security::capable(crate::security::CAP_SYS_NICE) {
            return -(errno::EPERM as i64);
        }
    }

    // Convert policy and apply
    // SAFETY: task is validated non-null above; we have exclusive access via scheduler lock.
    unsafe {
        let task_ref = &mut *task;
        let new_policy = match policy {
            SCHED_NORMAL => crate::process::task::SchedPolicy::Normal,
            SCHED_FIFO => {
                // Set RT priority
                task_ref.set_rt_priority(param.sched_priority as u32);
                crate::process::task::SchedPolicy::Fifo
            }
            SCHED_RR => {
                task_ref.set_rt_priority(param.sched_priority as u32);
                crate::process::task::SchedPolicy::Rr
            }
            SCHED_BATCH => crate::process::task::SchedPolicy::Batch,
            SCHED_IDLE => crate::process::task::SchedPolicy::Idle,
            SCHED_DEADLINE => crate::process::task::SchedPolicy::Deadline,
            _ => return -(errno::EINVAL as i64),
        };
        task_ref.set_policy(new_policy);
    }

    0
}

/// sys_sched_getscheduler - Get scheduling policy
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
///
/// # Returns
/// Policy number on success, negative error code on failure
pub fn sys_sched_getscheduler(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;

    let target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // SAFETY: task is validated non-null above; policy() reads the task's scheduling policy.
    let policy = unsafe { (*task).policy() };
    match policy {
        crate::process::task::SchedPolicy::Normal => SCHED_NORMAL as i64,
        crate::process::task::SchedPolicy::Fifo => SCHED_FIFO as i64,
        crate::process::task::SchedPolicy::Rr => SCHED_RR as i64,
        crate::process::task::SchedPolicy::Batch => SCHED_BATCH as i64,
        crate::process::task::SchedPolicy::Idle => SCHED_IDLE as i64,
        crate::process::task::SchedPolicy::Deadline => SCHED_DEADLINE as i64,
    }
}

/// sys_sched_setparam - Set scheduling parameters
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: param - pointer to sched_param
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn sys_sched_setparam(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let param_ptr = args[1] as *const SchedParam;

    if param_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(param_ptr as usize, core::mem::size_of::<SchedParam>()) {
        return -(errno::EFAULT as i64);
    }
    let mut param = core::mem::MaybeUninit::<SchedParam>::uninit();
    // SAFETY: param_ptr is access_ok-validated; SchedParam is repr(C) plain data.
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(
            param.as_mut_ptr() as *mut u8,
            param_ptr as *const u8,
            core::mem::size_of::<SchedParam>(),
        )
    };
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }
    let param = unsafe { param.assume_init() };

    let target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // Set RT priority if RT task
    // SAFETY: task is validated non-null above; we have exclusive access via scheduler lock.
    unsafe {
        let task_ref = &mut *task;
        let policy = task_ref.policy();
        if matches!(policy, crate::process::task::SchedPolicy::Fifo | crate::process::task::SchedPolicy::Rr) {
            task_ref.set_rt_priority(param.sched_priority as u32);
        }
    }

    0
}

/// sys_sched_getparam - Get scheduling parameters
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: param - pointer to sched_param (output)
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn sys_sched_getparam(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let param_ptr = args[1] as *mut SchedParam;

    if param_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(param_ptr as usize, core::mem::size_of::<SchedParam>()) {
        return -(errno::EFAULT as i64);
    }

    let target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // SAFETY: task is validated non-null; param_ptr is access_ok-validated above.
    unsafe {
        let task_ref = &*task;
        let priority = task_ref.rt_priority() as i32;
        let out = SchedParam { sched_priority: priority };
        let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
            param_ptr as *mut u8,
            &out as *const SchedParam as *const u8,
            core::mem::size_of::<SchedParam>(),
        );
        if uncopied != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_sched_getattr - Get scheduling attributes
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: attr - pointer to sched_attr (output)
/// - args[2]: size - size of sched_attr structure
/// - args[3]: flags - flags (unused)
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn sys_sched_getattr(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let attr_ptr = args[1] as *mut SchedAttr;
    let size = args[2] as u32;
    let _flags = args[3] as u32;

    if attr_ptr.is_null() || size == 0 {
        return -(errno::EINVAL as i64);
    }
    // Linux requires size >= the minimum sched_attr (first 48 bytes).
    if (size as usize) < 48 {
        return -(errno::EINVAL as i64);
    }

    // Only copy min(user_size, struct_size) bytes, matching Linux ABI.
    let copy_len = core::cmp::min(size as usize, core::mem::size_of::<SchedAttr>());
    if !crate::arch::riscv64::uaccess::access_ok(attr_ptr as usize, copy_len) {
        return -(errno::EFAULT as i64);
    }

    let target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // SAFETY: task is validated non-null; attr_ptr is access_ok-validated above.
    unsafe {
        let task_ref = &*task;
        let policy = task_ref.policy();

        let attr = SchedAttr {
            size: core::cmp::min(size, core::mem::size_of::<SchedAttr>() as u32),
            sched_policy: match policy {
                crate::process::task::SchedPolicy::Normal => SCHED_NORMAL as u32,
                crate::process::task::SchedPolicy::Fifo => SCHED_FIFO as u32,
                crate::process::task::SchedPolicy::Rr => SCHED_RR as u32,
                crate::process::task::SchedPolicy::Batch => SCHED_BATCH as u32,
                crate::process::task::SchedPolicy::Idle => SCHED_IDLE as u32,
                crate::process::task::SchedPolicy::Deadline => SCHED_DEADLINE as u32,
            },
            sched_flags: 0,
            sched_nice: task_ref.nice(),
            sched_priority: task_ref.rt_priority(),
            sched_runtime: task_ref.dl_entity().dl_runtime.load(core::sync::atomic::Ordering::Acquire),
            sched_deadline: task_ref.dl_entity().deadline.load(core::sync::atomic::Ordering::Acquire),
            sched_period: task_ref.dl_entity().dl_period.load(core::sync::atomic::Ordering::Acquire),
            sched_util_min: 0,
            sched_util_max: core::u32::MAX,
        };
        let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
            attr_ptr as *mut u8,
            &attr as *const SchedAttr as *const u8,
            copy_len,
        );
        if uncopied != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_sched_setattr - Set scheduling attributes
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: attr - pointer to sched_attr
/// - args[2]: flags - flags (unused)
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn sys_sched_setattr(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let attr_ptr = args[1] as *const SchedAttr;
    let _flags = args[2] as u32;

    if attr_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(attr_ptr as usize, core::mem::size_of::<SchedAttr>()) {
        return -(errno::EFAULT as i64);
    }
    let mut attr = core::mem::MaybeUninit::<SchedAttr>::uninit();
    // SAFETY: attr_ptr is access_ok-validated; SchedAttr is repr(C) plain data.
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(
            attr.as_mut_ptr() as *mut u8,
            attr_ptr as *const u8,
            core::mem::size_of::<SchedAttr>(),
        )
    };
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }
    let attr = unsafe { attr.assume_init() };

    let target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // SAFETY: find_task_by_pid returns a valid pointer when non-null; checked below.
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // SAFETY: task is validated non-null above; we have exclusive access via scheduler lock.
    unsafe {
        let task_ref = &mut *task;

        // Set policy
        let new_policy = match attr.sched_policy as i32 {
            SCHED_NORMAL => crate::process::task::SchedPolicy::Normal,
            SCHED_FIFO => crate::process::task::SchedPolicy::Fifo,
            SCHED_RR => crate::process::task::SchedPolicy::Rr,
            SCHED_BATCH => crate::process::task::SchedPolicy::Batch,
            SCHED_IDLE => crate::process::task::SchedPolicy::Idle,
            SCHED_DEADLINE => crate::process::task::SchedPolicy::Deadline,
            _ => return -(errno::EINVAL as i64),
        };
        task_ref.set_policy(new_policy);

        // Set nice for normal tasks
        if matches!(new_policy, crate::process::task::SchedPolicy::Normal | crate::process::task::SchedPolicy::Batch) {
            task_ref.set_nice(attr.sched_nice);
        }

        // Set RT priority
        if matches!(new_policy, crate::process::task::SchedPolicy::Fifo | crate::process::task::SchedPolicy::Rr) {
            task_ref.set_rt_priority(attr.sched_priority);
        }

        // Set deadline parameters
        if new_policy == crate::process::task::SchedPolicy::Deadline {
            let dl = task_ref.dl_entity_mut();
            dl.dl_runtime.store(attr.sched_runtime, core::sync::atomic::Ordering::Release);
            dl.dl_period.store(attr.sched_period, core::sync::atomic::Ordering::Release);
        }
    }

    0
}

/// sys_sched_rr_get_interval - Get RR time slice
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: ts - pointer to timespec (output)
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn sys_sched_rr_get_interval(args: SyscallArgs) -> i64 {
    #[repr(C)]
    struct TimeSpec {
        tv_sec: i64,
        tv_nsec: i64,
    }

    let pid = args[0] as u32;
    let ts_ptr = args[1] as *mut TimeSpec;

    if ts_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(ts_ptr as usize, core::mem::size_of::<TimeSpec>()) {
        return -(errno::EFAULT as i64);
    }

    let _target_pid = if pid == 0 {
        match crate::sched::current() {
            // SAFETY: sched::current() returns a valid Task pointer when Some.
            Some(t) => unsafe { (*t).pid() },
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    // Return default RR timeslice (100ms)
    let ts = TimeSpec { tv_sec: 0, tv_nsec: 100_000_000 };
    // SAFETY: ts_ptr is access_ok-validated above.
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_to_user(
            ts_ptr as *mut u8,
            &ts as *const TimeSpec as *const u8,
            core::mem::size_of::<TimeSpec>(),
        )
    };
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }

    0
}

/// sys_sched_setaffinity - Set CPU affinity
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: size - size of cpumask
/// - args[2]: mask - pointer to CPU mask
pub fn sys_sched_setaffinity(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let size = args[1] as usize;
    let mask_ptr = args[2] as *const usize;

    if size == 0 {
        return -(errno::EINVAL as i64);
    }
    if mask_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(mask_ptr as usize, size) {
        return -(errno::EFAULT as i64);
    }

    // Only self (pid=0) or current process
    if pid != 0 && pid as u32 != crate::process::current_pid() {
        return -(errno::ESRCH as i64);
    }

    // Validate that at least one CPU in the mask is online
    let ncpus = crate::config::MAX_CPUS;
    let mask_words = core::cmp::min(size / core::mem::size_of::<usize>(), 8);
    let mut has_online = false;
    for i in 0..mask_words {
        // SAFETY: mask_ptr is access_ok-validated for size bytes; i < mask_words stays in bounds.
        let word = unsafe { core::ptr::read_volatile(mask_ptr.add(i)) };
        // Check bits up to ncpus
        let bits_to_check = core::cmp::min(core::mem::size_of::<usize>() * 8, ncpus);
        for bit in 0..bits_to_check {
            let cpu = i * core::mem::size_of::<usize>() * 8 + bit;
            if cpu < ncpus && (word & (1 << bit)) != 0 {
                has_online = true;
                break;
            }
        }
        if has_online { break; }
    }
    if !has_online {
        return -(errno::EINVAL as i64);
    }

    // Accept the affinity mask (no per-task storage yet)
    0
}

/// sys_sched_getaffinity - Get CPU affinity
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: size - size of user cpumask buffer in bytes
/// - args[2]: mask - pointer to CPU mask (output)
///
/// # Returns
/// Number of bytes written to user buffer on success, negative errno on failure.
/// (Linux ABI: returns min(kernel_cpumask_bytes, user_size))
pub fn sys_sched_getaffinity(args: SyscallArgs) -> i64 {
    let pid = args[0] as u32;
    let size = args[1] as usize;
    let mask_ptr = args[2] as *mut u8;

    if mask_ptr.is_null() || size == 0 {
        return -(errno::EINVAL as i64);
    }

    // Compute kernel cpumask size: ceil(MAX_CPUS / 8) rounded up to usize alignment.
    let ncpus = crate::config::MAX_CPUS;
    let bytes_needed = ((ncpus + 7) / 8 + core::mem::size_of::<usize>() - 1)
        / core::mem::size_of::<usize>()
        * core::mem::size_of::<usize>();
    let ret_len = core::cmp::min(bytes_needed, size);

    if !crate::arch::riscv64::uaccess::access_ok(mask_ptr as usize, ret_len) {
        return -(errno::EFAULT as i64);
    }

    // Only pid 0 (self) supported for now; validate non-zero PID exists.
    if pid != 0 && pid != crate::process::current_pid() {
        // Check if the target PID actually exists
        let target = unsafe { crate::sched::find_task_by_pid(pid) };
        if target.is_null() {
            return -(errno::ESRCH as i64);
        }
    }

    // Build affinity mask: all CPUs allowed.
    let mut kernel_mask = alloc::vec![0u8; bytes_needed];
    for cpu in 0..ncpus {
        kernel_mask[cpu / 8] |= 1 << (cpu % 8);
    }

    unsafe {
        let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
            mask_ptr,
            kernel_mask.as_ptr(),
            ret_len,
        );
        if uncopied != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    ret_len as i64
}

/// sys_sched_get_priority_max - Get max static priority
///
/// # Arguments
/// - args[0]: policy - scheduling policy
pub fn sys_sched_get_priority_max(args: SyscallArgs) -> i64 {
    let policy = args[0] as i32;
    match policy {
        SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE => 0,
        SCHED_FIFO | SCHED_RR => 99,
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_sched_get_priority_min - Get min static priority
///
/// # Arguments
/// - args[0]: policy - scheduling policy
pub fn sys_sched_get_priority_min(args: SyscallArgs) -> i64 {
    let policy = args[0] as i32;
    match policy {
        SCHED_NORMAL | SCHED_BATCH | SCHED_IDLE => 0,
        SCHED_FIFO | SCHED_RR => 1,
        _ => -(errno::EINVAL as i64),
    }
}
