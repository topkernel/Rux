//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 信号相关系统调用
//!
//! 包含：rt_sigaction, rt_sigprocmask, rt_sigreturn, sigaltstack, sigpending

use super::*;

/// sys_rt_sigprocmask - 检查和更改阻塞的信号
///
/// # 参数
/// - args[0]: how - 操作方式
///   - SIG_BLOCK (0): 将 set 中的信号添加到阻塞掩码
///   - SIG_UNBLOCK (1): 从阻塞掩码中删除 set 中的信号
///   - SIG_SETMASK (2): 设置阻塞掩码为 set
/// - args[1]: set - 新信号掩码指针
/// - args[2]: oldset - 用于返回旧信号掩码的指针
/// - args[3]: sigsetsize - 信号集大小 (必须为 8)
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_rt_sigprocmask(args: SyscallArgs) -> u64 {
    let how = args[0] as i32;
    let set_ptr = args[1] as *const u64;  // SigSet is u64
    let oldset_ptr = args[2] as *mut u64;
    let sigsetsize = args[3] as usize;

    // 验证 sigsetsize
    if sigsetsize != 8 {
        return -errno::EINVAL as u64;
    }

    // 验证 how 参数
    use crate::signal::sigprocmask_how;
    if how != sigprocmask_how::SIG_BLOCK
        && how != sigprocmask_how::SIG_UNBLOCK
        && how != sigprocmask_how::SIG_SETMASK
    {
        return -errno::EINVAL as u64;
    }

    // 验证指针对齐 (u64 需要 8 字节对齐)
    if !set_ptr.is_null() && (set_ptr as usize) % 8 != 0 {
        return -errno::EINVAL as u64;
    }
    if !oldset_ptr.is_null() && (oldset_ptr as usize) % 8 != 0 {
        return -errno::EINVAL as u64;
    }

    // 读取新的信号掩码
    let new_mask = if !set_ptr.is_null() {
        unsafe { *set_ptr }
    } else {
        0
    };

    // 获取当前进程的 runqueue
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    // 获取当前信号掩码
    let old_mask = unsafe { (*current).sigmask };

    // 设置新的信号掩码
    let result_mask = match how {
        sigprocmask_how::SIG_BLOCK => {
            // 添加信号到阻塞掩码
            old_mask | new_mask
        }
        sigprocmask_how::SIG_UNBLOCK => {
            // 从阻塞掩码删除信号
            old_mask & !new_mask
        }
        sigprocmask_how::SIG_SETMASK => {
            // 设置新的阻塞掩码
            new_mask
        }
        _ => old_mask, // 不应该到达这里
    };

    // 更新当前进程的信号掩码
    unsafe {
        (*current).sigmask = result_mask;
    }

    // 返回旧的信号掩码
    if !oldset_ptr.is_null() {
        unsafe {
            *oldset_ptr = old_mask;
        }
    }

    0  // 成功
}

/// sys_rt_sigaction - 设置/获取信号处理动作
///
/// # 参数
/// - signum: 信号编号
/// - act: 新的信号处理动作（可为 null）
/// - oldact: 保存旧的信号处理动作（可为 null）
/// - sigsetsize: sigset_t 的大小
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_rt_sigaction(args: SyscallArgs) -> u64 {
    use crate::signal::{SigAction, Signal};

    let signum = args[0] as i32;
    let act_ptr = args[1] as *const SigAction;
    let oldact_ptr = args[2] as *mut SigAction;
    let sigsetsize = args[3] as usize;

    // 验证 sigsetsize
    if sigsetsize != 8 {
        return -errno::EINVAL as u64;
    }

    // 验证信号编号
    if signum < 1 || signum > 64 {
        return -errno::EINVAL as u64;
    }

    // SIGKILL 和 SIGSTOP 不能被捕获或忽略
    if signum == Signal::SIGKILL as i32 || signum == Signal::SIGSTOP as i32 {
        return -errno::EINVAL as u64;
    }

    // 获取当前进程
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    unsafe {
        let signal_struct = (*current).signal.as_mut();
        if signal_struct.is_none() {
            return -errno::EINVAL as u64;
        }
        let sig_struct = signal_struct.unwrap();

        // 保存旧的信号处理动作
        if !oldact_ptr.is_null() {
            if let Some(old_action) = sig_struct.get_action(signum) {
                *oldact_ptr = *old_action;
            } else {
                *oldact_ptr = SigAction::new();
            }
        }

        // 设置新的信号处理动作
        if !act_ptr.is_null() {
            let new_action = *act_ptr;
            match sig_struct.set_action(signum, new_action) {
                Ok(_) => 0,  // 成功
                Err(_) => -errno::EINVAL as u64,
            }
        } else {
            0  // 成功（只是查询）
        }
    }
}

/// sys_rt_sigreturn - 从信号处理函数返回
///
/// 恢复信号处理前的上下文，由信号处理函数返回时调用
///
/// # 参数
/// * `regs` - PtRegs 指针，用于恢复完整的用户上下文
///
/// # 返回
/// 返回信号中断前的系统调用返回值
pub fn sys_rt_sigreturn(regs: &mut crate::arch::riscv64::pt_regs::PtRegs) -> u64 {
    // 获取当前进程
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    unsafe {
        let frame_addr = (*current).sigframe_addr;

        // 恢复信号上下文到 PtRegs
        if frame_addr != 0 {
            crate::signal::restore_sigcontext(current, frame_addr, regs);
        }

        // 返回保存在信号帧中的原始返回值
        // 通常是从被中断的系统调用返回的值 (a0 = x10)
        // 注意：restore_sigcontext 已经恢复了 regs，所以直接返回 regs.a0
        regs.a0
    }
}

/// sys_sigpending - 获取待处理信号
///
/// # 参数
/// - set: 用于存储待处理信号的信号集指针
/// - sigsetsize: sigset_t 的大小
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_sigpending(args: SyscallArgs) -> u64 {
    let set_ptr = args[0] as *mut u64;
    let sigsetsize = args[1] as usize;

    // 验证 sigsetsize
    if sigsetsize != 8 {
        return -errno::EINVAL as u64;
    }

    if set_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 获取当前进程
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    unsafe {
        // 获取待处理信号（pending & ~blocked）
        let pending = (*current).pending.get_all();
        let blocked = (*current).sigmask;
        let deliverable = pending & !blocked;

        *set_ptr = deliverable;
    }

    0  // 成功
}

/// sys_sigaltstack - 设置/获取备用信号栈
///
/// # 参数
/// - ss: 新的信号栈配置（可为 null）
/// - old_ss: 保存旧的信号栈配置（可为 null）
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_sigaltstack(args: SyscallArgs) -> u64 {
    use crate::signal::{SignalStack, ss_flags};

    let ss_ptr = args[0] as *const SignalStack;
    let old_ss_ptr = args[1] as *mut SignalStack;

    // 获取当前进程
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    unsafe {
        // 保存旧的信号栈配置
        if !old_ss_ptr.is_null() {
            *old_ss_ptr = (*current).sigstack;
        }

        // 设置新的信号栈配置
        if !ss_ptr.is_null() {
            let new_ss = *ss_ptr;

            // 检查是否正在信号栈上执行
            if (*current).sigstack.is_on_stack() {
                return -errno::EBUSY as u64;  // 正在使用信号栈
            }

            // 验证新栈的大小
            if (new_ss.ss_flags & ss_flags::SS_DISABLE) == 0 {
                if new_ss.ss_size < crate::signal::MINSIGSTKSZ as u64 {
                    return -errno::EINVAL as u64;  // 栈太小
                }
            }

            (*current).sigstack = new_ss;
        }
    }

    0  // 成功
}
