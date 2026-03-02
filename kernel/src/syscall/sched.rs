//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 调度相关系统调用
//!
//! 包含：futex, sched_yield, getpriority, setpriority

use super::*;

/// PRIO_PROCESS - 进程优先级
const PRIO_PROCESS: i32 = 0;
/// PRIO_PGRP - 进程组优先级（暂不支持）
const PRIO_PGRP: i32 = 1;
/// PRIO_USER - 用户优先级（暂不支持）
const PRIO_USER: i32 = 2;

/// MIN_NICE - 最小 nice 值
const MIN_NICE: i32 = -20;
/// MAX_NICE - 最大 nice 值
const MAX_NICE: i32 = 19;

/// sys_futex - Fast Userspace Mutex
///
/// 用于线程同步的原语，完全参考 Linux 实现
///
/// # 参数
/// - args[0]: uaddr - futex 地址
/// - args[1]: op - 操作码 (FUTEX_WAIT=0, FUTEX_WAKE=1, etc.)
/// - args[2]: val - 值
/// - args[3]: timeout - 超时
/// - args[4]: uaddr2 - 第二个地址
/// - args[5]: val3 - 第三个值
///
/// # 返回
/// 成功返回操作结果，失败返回负错误码
pub fn sys_futex(args: SyscallArgs) -> u64 {
    // 使用 sync/futex.rs 中的完整实现
    crate::sync::sys_futex_handler(&args) as u64
}

/// sys_sched_yield - 让出 CPU
///
/// 当前线程主动让出 CPU，允许其他线程运行
///
/// # 返回
/// 总是返回 0
pub fn sys_sched_yield(_args: SyscallArgs) -> u64 {
    // 简化实现：直接返回，不做实际调度
    // TODO: 实现真正的调度让出
    0
}

/// sys_getpriority - 获取进程优先级
///
/// # 参数
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - 进程 ID / 进程组 ID / 用户 ID（0 表示当前进程）
///
/// # 返回
/// - 成功：nice 值 + 20（范围 1-40，0 表示错误）
/// - 失败：负的错误码
pub fn sys_getpriority(args: SyscallArgs) -> u64 {
    let which = args[0] as i32;
    let who = args[1] as u32;

    // 只支持 PRIO_PROCESS
    if which != PRIO_PROCESS {
        return -errno::EINVAL as u64;
    }

    let target_pid = if who == 0 {
        // who = 0 表示当前进程
        match crate::sched::current() {
            Some(t) => unsafe { (*t).pid() },
            None => return -errno::ESRCH as u64,
        }
    } else {
        who
    };

    // 查找目标进程
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -errno::ESRCH as u64;  // 进程不存在
    }

    // 返回 nice 值 + 20（转换为 1-40 范围）
    let nice = unsafe { (*task).nice() };
    (nice + 20) as u64
}

/// sys_setpriority - 设置进程优先级
///
/// # 参数
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - 进程 ID / 进程组 ID / 用户 ID（0 表示当前进程）
/// - args[2]: prio - 优先级值（nice 值，范围 -20 到 19）
///
/// # 返回
/// - 成功：0
/// - 失败：负的错误码
pub fn sys_setpriority(args: SyscallArgs) -> u64 {
    let which = args[0] as i32;
    let who = args[1] as u32;
    let niceval = args[2] as i32;

    // 只支持 PRIO_PROCESS
    if which != PRIO_PROCESS {
        return -errno::EINVAL as u64;
    }

    // 检查 nice 值范围
    let niceval = niceval.clamp(MIN_NICE, MAX_NICE);

    let target_pid = if who == 0 {
        // who = 0 表示当前进程
        match crate::sched::current() {
            Some(t) => unsafe { (*t).pid() },
            None => return -errno::ESRCH as u64,
        }
    } else {
        who
    };

    // 查找目标进程
    let task = unsafe { crate::sched::find_task_by_pid(target_pid) };
    if task.is_null() {
        return -errno::ESRCH as u64;  // 进程不存在
    }

    // 权限检查：只能修改自己进程的优先级，或者有 CAP_SYS_NICE 权限
    // 简化实现：允许修改任意进程的优先级
    // TODO: 添加权限检查

    // 设置 nice 值
    unsafe {
        (*task).set_nice(niceval);
    }

    0
}
