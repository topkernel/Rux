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
    // Linux's schedstat yld_count: incremented in the sched_yield paths
    // only (kernel/sched/syscalls.c schedstat_inc(rq->yld_count)).
    crate::fs::procfs::schedstat::note_yield(crate::arch::cpu_id() as usize);
    rux::sched::hotpath::yield_cpu();
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

    match which {
        PRIO_PROCESS => {
            let target_pid = if who == 0 {
                // who = 0 means current process
                match rux::sched::hotpath::current() {
                    Some(t) => t.pid(),
                    None => return -(errno::ESRCH as i64),
                }
            } else {
                who
            };

            // Find target process (syscall-context short read: TaskBorrow).
            let Some(task) = rux::process::types::TaskBorrow::lookup(target_pid) else {
                return -(errno::ESRCH as i64);  // Process does not exist
            };

            // Linux ABI returns 20 - nice (nice -20 → 40, nice 19 → 1). The old
            // nice + 20 encoding made glibc compute the negated nice value
            // (review M-12).
            let nice = task.nice();
            (20 - nice) as i64
        }
        // Group/user-wide queries (review批次1: previously EINVAL). who==0
        // selects the caller's own pgid/uid; the result is the HIGHEST
        // priority (lowest nice) among all matching tasks.
        PRIO_PGRP | PRIO_USER => {
            let key = if who == 0 {
                match rux::sched::hotpath::current() {
                    Some(t) => {
                        if which == PRIO_PGRP { t.pgid() } else { t.cred().uid }
                    }
                    None => return -(errno::ESRCH as i64),
                }
            } else {
                who
            };
            let mut best_nice: Option<i32> = None;
            rux::process::hotpath::pid_hash_for_each_task(|t| {
                // The callback runs under the bucket lock (typed walk).
                let matches =
                    if which == PRIO_PGRP { t.pgid() == key } else { t.cred().uid == key };
                if matches {
                    let n = t.nice();
                    best_nice = Some(match best_nice {
                        Some(b) if b <= n => b,
                        _ => n,
                    });
                }
            });
            match best_nice {
                Some(n) => (20 - n) as i64,
                None => -(errno::ESRCH as i64),
            }
        }
        _ => -(errno::EINVAL as i64),
    }
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

    // Check nice value range
    let niceval = niceval.clamp(MIN_NICE, MAX_NICE);

    // Collect the target PIDs for the selector (review批次1: PRIO_PGRP /
    // PRIO_USER were previously EINVAL).
    let mut target_pids: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    match which {
        PRIO_PROCESS => {
            target_pids.push(if who == 0 {
                match rux::sched::hotpath::current() {
                    Some(t) => t.pid(),
                    None => return -(errno::ESRCH as i64),
                    }
            } else {
                who
            });
        }
        PRIO_PGRP | PRIO_USER => {
            let key = if who == 0 {
                match rux::sched::hotpath::current() {
                    Some(t) => {
                        if which == PRIO_PGRP { t.pgid() } else { t.cred().uid }
                    }
                    None => return -(errno::ESRCH as i64),
                }
            } else {
                who
            };
            // Collect under the bucket lock, apply after it drops (renice
            // takes the GRQ lock — never nest it inside a pid-hash bucket).
            rux::process::hotpath::pid_hash_for_each_task(|t| {
                // Callback runs under the bucket lock (typed walk).
                let matches =
                    if which == PRIO_PGRP { t.pgid() == key } else { t.cred().uid == key };
                if matches {
                    target_pids.push(t.pid());
                }
            });
            if target_pids.is_empty() {
                return -(errno::ESRCH as i64);
            }
        }
        _ => return -(errno::EINVAL as i64),
    }

    let current_pid = ::process::current_pid();
    let mut last_err: i64 = 0;
    let mut applied = 0usize;

    for target_pid in target_pids {
        // Pin the task across the renice (a concurrent exit could otherwise
        // free it between lookup and use). TaskRef owns the pin: every
        // `continue` below releases it via Drop — the four manual
        // task_put sites collapsed into the handle.
        let Some(mut task) = rux::process::types::TaskRef::lookup(target_pid) else {
            last_err = -(errno::ESRCH as i64);
            continue;
        };

        // Permission checks (Linux setpriority):
        // - another process's priority requires CAP_SYS_NICE (EPERM);
        // - RAISING priority (lowering nice) requires CAP_SYS_NICE even for
        //   self (EACCES) — previously any self-renice passed (review批次1).
        let cur_nice = task.nice();
        if target_pid != current_pid {
            if !crate::security::capable(crate::security::CAP_SYS_NICE) {
                last_err = -(errno::EPERM as i64);
                continue;
            }
        } else if niceval < cur_nice
            && !crate::security::capable(crate::security::CAP_SYS_NICE)
        {
            last_err = -(errno::EACCES as i64);
            continue;
        }

        // Set nice value
        // R25-6: renice under the GRQ lock with load_weight rebalance — the
        // plain write let CFS enqueue/dequeue fetch_add/sub mismatched weights
        // (load_weight wrapped -> garbage sched_slice) and raced the runqueue.
        rux::sched::hotpath::sched_renice_locked(&mut *task, niceval);
        applied += 1;
    }

    if applied == 0 && last_err != 0 {
        last_err
    } else {
        0
    }
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

    // Negative pids are EINVAL before any lookup (Linux). The policy may
    // carry SCHED_RESET_ON_FORK (0x40000000): accepted, stored, and cleared
    // in forked children (LTP sched_setscheduler04).
    const SCHED_RESET_ON_FORK_FLAG: i32 = 0x4000_0000;
    let reset_on_fork = policy & SCHED_RESET_ON_FORK_FLAG != 0;
    let policy = policy & !SCHED_RESET_ON_FORK_FLAG;
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }

    // Validate policy
    if !matches!(policy, SCHED_NORMAL | SCHED_FIFO | SCHED_RR | SCHED_BATCH | SCHED_IDLE | SCHED_DEADLINE) {
        return -(errno::EINVAL as i64);
    }

    // Read param from userspace
    if param_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(param_ptr as usize, core::mem::size_of::<SchedParam>()) {
        return -(errno::EFAULT as i64);
    }
    let mut param = SchedParam { sched_priority: 0 };
    // Typed exception-table copy; SchedParam is repr(C) plain data.
    let uncopied =
        crate::arch::uaccess_safe::copy_in_t(&mut param, param_ptr as usize);
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }

    // Find target task
    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    let mut task = match ::process::find_task_by_pid(target_pid) {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Permission check: moving ANY task (including self) ONTO an RT/DL
    // policy requires CAP_SYS_NICE — self could otherwise lock up every
    // CPU with SCHED_FIFO (review syscallb-H12/P06).
    if matches!(policy, SCHED_FIFO | SCHED_RR | SCHED_DEADLINE)
        && !crate::security::capable(crate::security::CAP_SYS_NICE)
    {
        return -(errno::EPERM as i64);
    }

    // Priority validation (review PROC-P06): RT priorities live in
    // [1,99]; every other policy requires 0.
    if matches!(policy, SCHED_FIFO | SCHED_RR) {
        if !(1..=99).contains(&param.sched_priority) {
            return -(errno::EINVAL as i64);
        }
    } else if param.sched_priority != 0 {
        return -(errno::EINVAL as i64);
    }

    // Store the RESET_ON_FORK request where fork() can see it.
    task.set_sched_reset_on_fork(reset_on_fork);
    // Convert and apply with run-queue migration under the GRQ lock
    // (review PROC-P07).
    let new_policy = match policy {
        SCHED_NORMAL => rux::process::types::SchedPolicy::Normal,
        SCHED_FIFO => rux::process::types::SchedPolicy::Fifo,
        SCHED_RR => rux::process::types::SchedPolicy::Rr,
        SCHED_BATCH => rux::process::types::SchedPolicy::Batch,
        SCHED_IDLE => rux::process::types::SchedPolicy::Idle,
        SCHED_DEADLINE => rux::process::types::SchedPolicy::Deadline,
        _ => return -(errno::EINVAL as i64),
    };
    // change_task_policy takes the GRQ lock internally (still raw-ptr
    // typed: it also serves pinned/raw callers).
    rux::sched::hotpath::change_task_policy(
        task as *mut rux::process::types::Task,
        new_policy,
        param.sched_priority as u32,
    );

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

    // Negative pids are EINVAL before any lookup.
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }

    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    let Some(task) = rux::process::types::TaskBorrow::lookup(target_pid) else {
        return -(errno::ESRCH as i64);
    };

    let policy = task.policy();
    match policy {
        rux::process::types::SchedPolicy::Normal => SCHED_NORMAL as i64,
        rux::process::types::SchedPolicy::Fifo => SCHED_FIFO as i64,
        rux::process::types::SchedPolicy::Rr => SCHED_RR as i64,
        rux::process::types::SchedPolicy::Batch => SCHED_BATCH as i64,
        rux::process::types::SchedPolicy::Idle => SCHED_IDLE as i64,
        rux::process::types::SchedPolicy::Deadline => SCHED_DEADLINE as i64,
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

    // Negative pids are EINVAL before any lookup (LTP sched_setparam04
    // passes -1; sched_setparam05 passes a reaped/unused pid → ESRCH).
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }
    if param_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(param_ptr as usize, core::mem::size_of::<SchedParam>()) {
        return -(errno::EFAULT as i64);
    }
    let mut param = SchedParam { sched_priority: 0 };
    // Typed exception-table copy; SchedParam is repr(C) plain data.
    let uncopied = crate::arch::uaccess_safe::copy_in_t(&mut param, param_ptr as usize);
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }

    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
            }
    } else {
        pid
    };

    let mut task = match ::process::find_task_by_pid(target_pid) {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // R32-F4b: this used to write rt_priority directly. rt_rq keys its
    // per-priority lists AND its bitmap by rt_priority, so mutating the
    // field of a linked task made the next dequeue compute the WRONG prio
    // index — it cleared another priority's bitmap bit and left
    // highest_prio stale (phantom runnable priorities / lost tasks). It
    // also skipped the CAP_SYS_NICE gate and the [1,99] range check that
    // sys_sched_setscheduler enforces. Route through change_task_policy()
    // (dequeue + field update + re-enqueue under the GRQ lock) and mirror
    // the setscheduler validation.
    let policy = task.policy();
    if matches!(policy, rux::process::types::SchedPolicy::Fifo | rux::process::types::SchedPolicy::Rr) {
        // RT parameter changes require CAP_SYS_NICE, mirroring the RT
        // policy-switch check in sys_sched_setscheduler.
        if !crate::security::capable(crate::security::CAP_SYS_NICE) {
            return -(errno::EPERM as i64);
        }
        // Priority validation (review PROC-P06): RT priorities live in [1,99].
        if !(1..=99).contains(&param.sched_priority) {
            return -(errno::EINVAL as i64);
        }
        // change_task_policy takes the GRQ lock internally and
        // re-positions the task in the RT priority lists.
        rux::sched::hotpath::change_task_policy(task, policy, param.sched_priority as u32);
    } else if param.sched_priority != 0 {
        // Non-RT policies must carry priority 0 (review PROC-P06).
        return -(errno::EINVAL as i64);
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

    // Negative pids are EINVAL before any lookup (LTP sched_getparam03
    // passes -1 and expects EINVAL, not ESRCH).
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }
    if param_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(param_ptr as usize, core::mem::size_of::<SchedParam>()) {
        return -(errno::EFAULT as i64);
    }

    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
            }
    } else {
        pid
    };

    let Some(task) = rux::process::types::TaskBorrow::lookup(target_pid) else {
        return -(errno::ESRCH as i64);
    };

    let priority = task.rt_priority() as i32;
    let out = SchedParam { sched_priority: priority };
    let uncopied = crate::arch::uaccess_safe::copy_out_t(param_ptr as usize, &out);
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
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
    let flags = args[3] as u32;

    // Linux sched_getattr rejects ANY nonzero flags with EINVAL (LTP
    // sched_getattr02 case 4 passes flags=1000).
    if flags != 0 {
        return -(errno::EINVAL as i64);
    }
    // Negative pids are EINVAL before any lookup (Linux find_task_by_pid).
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }
    if attr_ptr.is_null() || size == 0 {
        return -(errno::EINVAL as i64);
    }
    // Linux requires size >= the minimum sched_attr (first 48 bytes).
    if (size as usize) < 48 {
        return -(errno::EINVAL as i64);
    }

    // Only copy min(user_size, struct_size) bytes, matching Linux ABI.
    let copy_len = core::cmp::min(size as usize, core::mem::size_of::<SchedAttr>());
    if !crate::arch::uaccess::access_ok(attr_ptr as usize, copy_len) {
        return -(errno::EFAULT as i64);
    }

    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
            }
    } else {
        pid
    };

    let Some(task) = rux::process::types::TaskBorrow::lookup(target_pid) else {
        return -(errno::ESRCH as i64);
    };

    {
        let task_ref = &*task;
        let policy = task_ref.policy();

        let attr = SchedAttr {
            size: core::cmp::min(size, core::mem::size_of::<SchedAttr>() as u32),
            sched_policy: match policy {
                rux::process::types::SchedPolicy::Normal => SCHED_NORMAL as u32,
                rux::process::types::SchedPolicy::Fifo => SCHED_FIFO as u32,
                rux::process::types::SchedPolicy::Rr => SCHED_RR as u32,
                rux::process::types::SchedPolicy::Batch => SCHED_BATCH as u32,
                rux::process::types::SchedPolicy::Idle => SCHED_IDLE as u32,
                rux::process::types::SchedPolicy::Deadline => SCHED_DEADLINE as u32,
            },
            sched_flags: 0,
            sched_nice: task_ref.nice(),
            sched_priority: task_ref.rt_priority(),
            sched_runtime: task_ref.dl_entity().dl_runtime.load(core::sync::atomic::Ordering::Acquire),
            sched_deadline: {
                // Report the user-programmed relative deadline, not the
                // scheduler's internal absolute one (LTP sched_getattr01).
                let dl = task_ref.dl_entity();
                let user_dl = dl.attr_deadline.load(core::sync::atomic::Ordering::Acquire);
                if user_dl != 0 { user_dl } else { dl.dl_period.load(core::sync::atomic::Ordering::Acquire) }
            },
            sched_period: task_ref.dl_entity().dl_period.load(core::sync::atomic::Ordering::Acquire),
            sched_util_min: 0,
            sched_util_max: core::u32::MAX,
        };
        // Header-truncated out-copy (copy_len <= size_of::<SchedAttr>()):
        // the head of the live local's byte image (INV-CBYTES-1).
        let src = &crate::safety::bytes::bytes_of(&attr)[..copy_len];
        let uncopied = crate::arch::uaccess_safe::UserSlice::new(attr_ptr as usize, copy_len)
            .copy_out(src);
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
    let flags = args[2] as u32;

    // Linux sched_setattr accepts only the known SCHED_FLAG_* bits (mask
    // 0x7f); anything else is EINVAL (LTP sched_setattr01 case 4 passes
    // flags=1000).
    const SCHED_FLAG_KNOWN: u32 = 0x7f;
    if flags & !SCHED_FLAG_KNOWN != 0 {
        return -(errno::EINVAL as i64);
    }
    // Negative pids are EINVAL before any lookup.
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }
    if attr_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    // Read the user-declared size first (review批次1: setattr 不按 size 截读).
    // Linux copies only attr->size bytes: an older/smaller user struct must
    // not be over-read (the rest of the kernel copy stays zero).
    if !crate::arch::uaccess::access_ok(attr_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }
    let mut size_word = [0u8; 4];
    // Typed exception-table copy (access_ok(8) above covers these 4).
    if crate::arch::uaccess_safe::copy_in(&mut size_word, attr_ptr as usize) != 0 {
        return -(errno::EFAULT as i64);
    }
    let user_size = u32::from_le_bytes(size_word) as usize;
    if user_size == 0 {
        return -(errno::EINVAL as i64);
    }
    // A user struct LARGER than the kernel's is E2BIG (Linux sched_copy_attr).
    if user_size > core::mem::size_of::<SchedAttr>() {
        return -(errno::E2BIG as i64);
    }
    if !crate::arch::uaccess::access_ok(attr_ptr as usize, user_size) {
        return -(errno::EFAULT as i64);
    }
    let mut attr_storage = [0u8; core::mem::size_of::<SchedAttr>()];
    // Typed exception-table copy: attr_ptr is access_ok-validated for
    // user_size; copying into a zeroed stack buffer leaves any fields
    // beyond user_size as zero.
    let uncopied = crate::arch::uaccess_safe::UserSlice::new(attr_ptr as usize, user_size)
        .copy_in(&mut attr_storage[..user_size]);
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }
    // attr_storage is a fully-initialized byte buffer of exactly
    // sizeof(SchedAttr) and SchedAttr is repr(C) plain data
    // (INV-CBYTES-2 unaligned rebuild; from_bytes checks the bound).
    let attr = crate::safety::bytes::from_bytes::<SchedAttr>(&attr_storage)
        .unwrap_or(SchedAttr {
            size: 0,
            sched_policy: 0,
            sched_flags: 0,
            sched_nice: 0,
            sched_priority: 0,
            sched_runtime: 0,
            sched_deadline: 0,
            sched_period: 0,
            sched_util_min: 0,
            sched_util_max: 0,
        });

    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
        }
    } else {
        pid
    };

    let task = match ::process::find_task_by_pid(target_pid) {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // R32-F4: this used to write policy/priority/DL fields directly. That
    // bypassed change_task_policy()'s class-queue migration under the GRQ
    // lock — a queued CFS task stayed linked on cfs_rq while claiming Fifo,
    // so __schedule's rt_rq dequeue was a no-op and the task was stranded
    // off every class queue (unrunnable) — and skipped BOTH the
    // CAP_SYS_NICE gate and the parameter validation that
    // sys_sched_setscheduler enforces: any task could self-promote to
    // SCHED_FIFO 99 and starve every CPU, or program nonsense DL params
    // (runtime 0 throttles the task forever). Mirror the validated
    // setscheduler path exactly.
    let policy = attr.sched_policy as i32;
    if !matches!(policy, SCHED_NORMAL | SCHED_FIFO | SCHED_RR | SCHED_BATCH | SCHED_IDLE | SCHED_DEADLINE) {
        return -(errno::EINVAL as i64);
    }

    // Permission check: moving ANY task (including self) ONTO an RT/DL
    // policy requires CAP_SYS_NICE — mirrors sys_sched_setscheduler
    // (review syscallb-H12/P06).
    if matches!(policy, SCHED_FIFO | SCHED_RR | SCHED_DEADLINE)
        && !crate::security::capable(crate::security::CAP_SYS_NICE)
    {
        return -(errno::EPERM as i64);
    }

    // Priority validation (review PROC-P06): RT priorities live in
    // [1,99]; every other policy requires 0.
    if matches!(policy, SCHED_FIFO | SCHED_RR) {
        if !(1..=99).contains(&(attr.sched_priority as i32)) {
            return -(errno::EINVAL as i64);
        }
    } else if attr.sched_priority != 0 {
        return -(errno::EINVAL as i64);
    }

    // Deadline admission (Linux: 0 < runtime <= deadline <= period, with
    // period == 0 meaning period = deadline). runtime 0 permanently
    // throttles the task; runtime > deadline breaks the CBS
    // replenishment invariant.
    let dl_period = if policy == SCHED_DEADLINE && attr.sched_period == 0 {
        attr.sched_deadline
    } else {
        attr.sched_period
    };
    if policy == SCHED_DEADLINE
        && (attr.sched_runtime == 0
            || attr.sched_deadline == 0
            || attr.sched_runtime > attr.sched_deadline
            || attr.sched_deadline > dl_period)
    {
        return -(errno::EINVAL as i64);
    }

    let new_policy = match policy {
        SCHED_NORMAL => rux::process::types::SchedPolicy::Normal,
        SCHED_FIFO => rux::process::types::SchedPolicy::Fifo,
        SCHED_RR => rux::process::types::SchedPolicy::Rr,
        SCHED_BATCH => rux::process::types::SchedPolicy::Batch,
        SCHED_IDLE => rux::process::types::SchedPolicy::Idle,
        SCHED_DEADLINE => rux::process::types::SchedPolicy::Deadline,
        _ => return -(errno::EINVAL as i64),
    };

    // SAFETY: task is validated non-null above. Program the DL parameters
    // BEFORE the policy switch so change_task_policy's re-enqueue
    // (update_deadline + replenish_runtime) already sees them; they are
    // inert while the task is not on the Deadline policy.
    if new_policy == rux::process::types::SchedPolicy::Deadline {
        let dl = task.dl_entity_mut();
        dl.dl_runtime.store(attr.sched_runtime, core::sync::atomic::Ordering::Release);
        dl.dl_period.store(dl_period, core::sync::atomic::Ordering::Release);
        // Keep the user's relative deadline for sched_getattr readback —
        // the scheduler's absolute `deadline` is internal state (LTP
        // sched_getattr01 compares all three values).
        dl.attr_deadline.store(attr.sched_deadline, core::sync::atomic::Ordering::Release);
    }

    // Apply the policy switch with run-queue migration under the GRQ lock
    // (review PROC-P07). change_task_policy takes the GRQ lock internally.
    rux::sched::hotpath::change_task_policy(
        task as *mut rux::process::types::Task,
        new_policy,
        attr.sched_priority,
    );

    // Set nice for normal tasks: route through the locked renice helper
    // (R31-3c: bare set_nice raced the CFS accounting).
    if matches!(new_policy, rux::process::types::SchedPolicy::Normal | rux::process::types::SchedPolicy::Batch) {
        rux::sched::hotpath::sched_renice_locked(
            task,
            attr.sched_nice.clamp(MIN_NICE, MAX_NICE),
        );
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
    #[derive(Clone, Copy)]
struct TimeSpec {
        tv_sec: i64,
        tv_nsec: i64,
    }

    let pid = args[0] as u32;
    let ts_ptr = args[1] as *mut TimeSpec;

    // Negative pids are EINVAL before any lookup (LTP
    // sched_rr_get_interval03 passes -1).
    if (args[0] as i64) < 0 {
        return -(errno::EINVAL as i64);
    }
    if ts_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(ts_ptr as usize, core::mem::size_of::<TimeSpec>()) {
        return -(errno::EFAULT as i64);
    }

    let target_pid = if pid == 0 {
        match rux::sched::hotpath::current() {
            Some(t) => t.pid(),
            None => return -(errno::ESRCH as i64),
            }
    } else {
        pid
    };

    let Some(task) = rux::process::types::TaskBorrow::lookup(target_pid) else {
        return -(errno::ESRCH as i64);
    };

    // Linux reports the real RR timeslice only for SCHED_RR tasks; every
    // other policy gets {0, 0} (LTP sched_rr_get_interval02 runs with
    // SCHED_FIFO and requires an all-zero timespec).
    let policy = task.policy();
    let ts = if policy == rux::process::types::SchedPolicy::Rr {
        TimeSpec { tv_sec: 0, tv_nsec: 100_000_000 }
    } else {
        TimeSpec { tv_sec: 0, tv_nsec: 0 }
    };
    let uncopied = crate::arch::uaccess_safe::copy_out_t(ts_ptr as usize, &ts);
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
    if !crate::arch::uaccess::access_ok(mask_ptr as usize, size) {
        return -(errno::EFAULT as i64);
    }

    // Resolve the target: self (pid 0 / own pid) or any existing task.
    // A missing pid is ESRCH (LTP setaffinity01 case 3); changing ANOTHER
    // task's affinity without CAP_SYS_NICE is EPERM (case 4, run as an
    // unprivileged user against a root child).
    // SAFETY: sched::current() returns the running task's pointer.
    let current = match rux::sched::hotpath::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };
    let task: &mut rux::process::types::Task = if pid == 0 || pid as u32 == ::process::current_pid() {
        current
    } else {
        let Some(target) = ::process::find_task_by_pid(pid) else {
            return -(errno::ESRCH as i64);
        };
        let same = core::ptr::eq(target, current as &rux::process::types::Task);
        if !same && !crate::security::capable(crate::security::CAP_SYS_NICE) {
            return -(errno::EPERM as i64);
        }
        target
    };

    // Read the mask word by word; an UNREADABLE word is EFAULT, not a
    // silent zero (LTP setaffinity01 case 1 passes a bad address).
    let ncpus = crate::config::MAX_CPUS;
    // The mask must name CPUs that actually exist at runtime. Linux
    // rejects a mask disjoint from cpu_possible_mask with EINVAL;
    // accepting bits for absent CPUs starves the caller forever — the
    // RT/CFS pick paths skip a task whose cpus_allowed has no bit for
    // any online CPU. OH's render_service pins its main thread to
    // "the last CPU" from userspace topology info (mask 0x4 = CPU2 on
    // the SMP=2 boot); the old MAX_CPUS-only clamp let that through and
    // RS sat runnable-but-never-picked after opening /dev/dri/card0 —
    // the SA:10 / bootanimation / launcher chain all died on that
    // starvation.
    let online = {
        // s14 fleet catch (ltp-b r2 canary): the mask must name CPUs
        // that are actually ONLINE at runtime. The old cfg-split
        // trusted apic::cpu_count() (MADT-discovered) on x86_64 and
        // config::MAX_CPUS on riscv64 — on a boot with fewer CPUs than
        // MAX_CPUS (fleet lanes: -smp 2 with MAX_CPUS=4) a mask like
        // 0x8 named a nonexistent hart and passed validation; the task
        // then stranded forever (every pick refuses it, skip why=3),
        // wake paths IPI'd the dead hart ("send_ipi to hart 3 failed,
        // error=-3"), and one such leaked LTP helper produced a
        // 185-report PICK-SKIP-ALL storm plus the WAKE-STRANDED grid
        // burst behind it. num_started_cpus() is the started-flag
        // count on both arches — the exact set the scheduler can pick
        // on. Linux rejects a mask disjoint from the online set with
        // EINVAL; so do we (below).
        let n = crate::arch::smp::num_started_cpus();
        if n == 0 { 1 } else { n }
    };
    let online_mask: u32 = if online >= 32 { u32::MAX } else { (1u32 << online) - 1 };
    let mask_words = core::cmp::min(size / core::mem::size_of::<usize>(), 8);
    let mut stored_mask: u32 = 0;
    for i in 0..mask_words {
        // Typed exception-table read (SUM=0 safe); an UNREADABLE word is
        // EFAULT, not a silent zero.
        let word = match crate::arch::uaccess_safe::UserPtr::<usize>::new(
            mask_ptr as usize + i * core::mem::size_of::<usize>(),
        )
        .read()
        {
            Some(w) => w,
            None => return -(errno::EFAULT as i64),
        };
        let bits_to_check = core::cmp::min(core::mem::size_of::<usize>() * 8, ncpus);
        for bit in 0..bits_to_check {
            let cpu = i * core::mem::size_of::<usize>() * 8 + bit;
            if cpu < ncpus && (word & (1 << bit)) != 0 {
                stored_mask |= 1 << cpu;
            }
        }
    }
    // Disjoint from the online set -> EINVAL (Linux semantics; the
    // "empty mask" LTP case is subsumed: an empty mask is disjoint too).
    stored_mask &= online_mask;
    if stored_mask == 0 {
        return -(errno::EINVAL as i64);
    }
    task.set_cpus_allowed(stored_mask);
    // Linux semantics: when the new mask removes the CPU a task is
    // currently running (or enqueued) on, the task must migrate promptly
    // — not at its next natural preemption. Without this, getcpu()
    // right after sched_setaffinity() reports the stale CPU (LTP
    // getcpu01: pin to the highest CPU, expect getcpu to agree).
    // For the caller itself: request a reschedule on trap return (the
    // pick path skips tasks whose affinity excludes the picking CPU, so
    // __schedule() parks it and an allowed CPU picks it up) and poke the
    // lowest allowed CPU so the pickup is immediate. For another task:
    // poking its current CPU is enough — its next schedule() will move it.
    {
        let this = crate::arch::cpu_id() as usize;
        if stored_mask & (1u32 << this) == 0 {
            let target = stored_mask.trailing_zeros() as usize;
            let task_is_current =
                pid == 0 || pid as u32 == ::process::current_pid();
            if task_is_current {
                // Direct migration: schedule() here (a valid syscall-context
                // sleep point) — the pick path skips tasks whose affinity
                // excludes the picking CPU, so this CPU parks the caller
                // and only an allowed CPU picks it back up. Poking the
                // target first makes the pickup immediate.
                // (The trap-exit preempt path cannot be relied on: its
                // TASK_TI_PREEMPT check reads a straddled 8-byte load and
                // effectively never preempts — see trap.S.)
                if target < crate::config::MAX_CPUS && target != this {
                    rux::sched::hotpath::resched_cpu(target);
                }
                rux::sched::hotpath::schedule();
                // Post-conditions don't matter: whether we ran on an
                // allowed CPU before or after this point, getcpu() now
                // reports an allowed CPU.
            } else {
                let ti_cpu = task.ti_cpu() as usize;
                if ti_cpu < crate::config::MAX_CPUS && ti_cpu != this {
                    rux::sched::hotpath::resched_cpu(ti_cpu);
                } else if target < crate::config::MAX_CPUS {
                    rux::sched::hotpath::resched_cpu(target);
                }
            }
        }
    }
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

    if !crate::arch::uaccess::access_ok(mask_ptr as usize, ret_len) {
        return -(errno::EFAULT as i64);
    }

    // Resolve the target task (self, or an existing pid).
    // SAFETY: sched::current() returns the running task's pointer.
    let task = if pid == 0 || pid == ::process::current_pid() {
        match rux::sched::hotpath::current() {
            Some(t) => t,
            None => return -(errno::ESRCH as i64),
        }
    } else {
        let Some(target) = ::process::find_task_by_pid(pid) else {
            return -(errno::ESRCH as i64);
        };
        target
    };

    // Report the STORED affinity mask (review批次1: previously always
    // all-CPUs regardless of what setaffinity installed).
    // The STORED affinity mask (cpus_allowed() is an atomic read).
    let allowed = task.cpus_allowed();
    let mut kernel_mask = alloc::vec![0u8; bytes_needed];
    for cpu in 0..ncpus {
        if allowed & (1 << cpu) != 0 {
            kernel_mask[cpu / 8] |= 1 << (cpu % 8);
        }
    }

    let uncopied = crate::arch::uaccess_safe::UserSlice::new(mask_ptr as usize, ret_len)
        .copy_out(&kernel_mask[..ret_len]);
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
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
        // SCHED_DEADLINE has a single (0) static priority (LTP
        // sched_get_priority_max01 covers it).
        SCHED_DEADLINE => 0,
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
        SCHED_DEADLINE => 0,
        SCHED_FIFO | SCHED_RR => 1,
        _ => -(errno::EINVAL as i64),
    }
}
