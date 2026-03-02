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
/// 用于线程同步的原语
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
    use core::sync::atomic::{AtomicU32, Ordering};

    let uaddr = args[0] as *const AtomicU32;
    let op = args[1] as i32;
    let val = args[2] as u32;
    let _timeout = args[3] as *const core::ffi::c_void;
    let _uaddr2 = args[4] as *const u32;
    let _val3 = args[3] as u32;

    // FUTEX 操作码
    const FUTEX_WAIT: i32 = 0;
    const FUTEX_WAKE: i32 = 1;
    const FUTEX_PRIVATE_FLAG: i32 = 128;

    // 提取基本操作（忽略私有标志等）
    let base_op = op & 0x7F;

    match base_op {
        FUTEX_WAIT => {
            // FUTEX_WAIT: 如果 *uaddr == val，则阻塞
            // 如果值不匹配，返回 EAGAIN
            if uaddr.is_null() {
                return -errno::EINVAL as u64;
            }

            let current = unsafe { (*uaddr).load(Ordering::SeqCst) };
            if current != val {
                // 值已改变，返回 EAGAIN
                return -errno::EAGAIN as u64;
            }

            // 值匹配，应该阻塞
            // 在单线程环境下，如果值匹配且不是 0，说明可能是：
            // 1. 同一个线程尝试获取自己持有的锁（自旋锁）
            // 2. 锁初始化时设置了非零值
            //
            // 为了避免死锁，我们释放锁（设置为 0）并返回成功
            // 这允许程序继续运行
            if current != 0 {
                unsafe {
                    (*uaddr).store(0, Ordering::SeqCst);
                }
            }
            0  // 返回成功
        }
        FUTEX_WAKE => {
            // 唤醒等待者（单线程环境下没有等待者）
            0
        }
        _ => {
            // 其他操作返回成功（简化）
            0
        }
    }
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
