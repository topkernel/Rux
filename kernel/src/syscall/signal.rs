//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Signal-related system calls
//!
//! Includes: rt_sigaction, rt_sigprocmask, rt_sigreturn, sigaltstack, sigpending

use super::*;

/// sys_rt_sigprocmask - Examine and change blocked signals
///
/// # Arguments
/// - args[0]: how - operation mode
///   - SIG_BLOCK (0): Add signals in set to blocked mask
///   - SIG_UNBLOCK (1): Remove signals in set from blocked mask
///   - SIG_SETMASK (2): Set blocked mask to set
/// - args[1]: set - new signal mask pointer
/// - args[2]: oldset - pointer to return old signal mask
/// - args[3]: sigsetsize - signal set size (must be 8)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_rt_sigprocmask(args: SyscallArgs) -> u64 {
    let how = args[0] as i32;
    let set_ptr = args[1] as *const u64;  // SigSet is u64
    let oldset_ptr = args[2] as *mut u64;
    let sigsetsize = args[3] as usize;

    // Validate sigsetsize
    if sigsetsize != 8 {
        return -errno::EINVAL as u64;
    }

    // Validate how parameter
    use crate::signal::sigprocmask_how;
    if how != sigprocmask_how::SIG_BLOCK
        && how != sigprocmask_how::SIG_UNBLOCK
        && how != sigprocmask_how::SIG_SETMASK
    {
        return -errno::EINVAL as u64;
    }

    // Validate pointer alignment (u64 requires 8-byte alignment)
    if !set_ptr.is_null() && (set_ptr as usize) % 8 != 0 {
        return -errno::EINVAL as u64;
    }
    if !oldset_ptr.is_null() && (oldset_ptr as usize) % 8 != 0 {
        return -errno::EINVAL as u64;
    }

    // Read new signal mask
    let new_mask = if !set_ptr.is_null() {
        // Validate user pointer
        if !crate::arch::riscv64::uaccess::access_ok(set_ptr as usize, 8) {
            return -errno::EFAULT as u64;
        }
        unsafe { *set_ptr }
    } else {
        0
    };

    // Get current process runqueue
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    // Get current signal mask
    let old_mask = unsafe { (*current).sigmask };

    // Set new signal mask
    let result_mask = match how {
        sigprocmask_how::SIG_BLOCK => {
            // Add signals to blocked mask
            old_mask | new_mask
        }
        sigprocmask_how::SIG_UNBLOCK => {
            // Remove signals from blocked mask
            old_mask & !new_mask
        }
        sigprocmask_how::SIG_SETMASK => {
            // Set new blocked mask
            new_mask
        }
        _ => old_mask, // Should not reach here
    };

    // Update current process signal mask
    unsafe {
        (*current).sigmask = result_mask;
    }

    // Return old signal mask
    if !oldset_ptr.is_null() {
        // Validate user pointer
        if !crate::arch::riscv64::uaccess::access_ok(oldset_ptr as usize, 8) {
            return -errno::EFAULT as u64;
        }
        unsafe {
            *oldset_ptr = old_mask;
        }
    }

    0  // Success
}

/// sys_rt_sigaction - Set/get signal handling action
///
/// # Arguments
/// - signum: signal number
/// - act: new signal handling action (can be null)
/// - oldact: save old signal handling action (can be null)
/// - sigsetsize: size of sigset_t
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_rt_sigaction(args: SyscallArgs) -> u64 {
    use crate::signal::{SigAction, Signal};

    let signum = args[0] as i32;
    let act_ptr = args[1] as *const SigAction;
    let oldact_ptr = args[2] as *mut SigAction;
    let sigsetsize = args[3] as usize;

    // Validate sigsetsize
    if sigsetsize != 8 {
        return -errno::EINVAL as u64;
    }

    // Validate signal number
    if signum < 1 || signum > 64 {
        return -errno::EINVAL as u64;
    }

    // SIGKILL and SIGSTOP cannot be caught or ignored
    if signum == Signal::SIGKILL as i32 || signum == Signal::SIGSTOP as i32 {
        return -errno::EINVAL as u64;
    }

    // Get current process
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

        // Save old signal handling action
        if !oldact_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(oldact_ptr as usize, core::mem::size_of::<SigAction>()) {
                return -errno::EFAULT as u64;
            }
            if let Some(old_action) = sig_struct.get_action(signum) {
                *oldact_ptr = old_action;
            } else {
                *oldact_ptr = SigAction::new();
            }
        }

        // Set new signal handling action
        if !act_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(act_ptr as usize, core::mem::size_of::<SigAction>()) {
                return -errno::EFAULT as u64;
            }
            let new_action = *act_ptr;
            match sig_struct.set_action(signum, new_action) {
                Ok(_) => 0,  // Success
                Err(_) => -errno::EINVAL as u64,
            }
        } else {
            0  // Success (just query)
        }
    }
}

/// sys_rt_sigreturn - Return from signal handler
///
/// Restore context before signal handling, called when signal handler returns
///
/// # Arguments
/// * `regs` - PtRegs pointer for restoring complete user context
///
/// # Returns
/// Returns system call return value before signal interruption
pub fn sys_rt_sigreturn(regs: &mut crate::arch::riscv64::pt_regs::PtRegs) -> u64 {
    // Get current process
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

        // Restore signal context to PtRegs
        if frame_addr != 0 {
            crate::signal::restore_sigcontext(current, frame_addr, regs);
        }

        // Return original return value saved in signal frame
        // Usually the value returned from interrupted system call (a0 = x10)
        // Note: restore_sigcontext has already restored regs, so just return regs.a0
        regs.a0
    }
}

/// sys_sigpending - Get pending signals
///
/// # Arguments
/// - set: pointer to signal set for storing pending signals
/// - sigsetsize: size of sigset_t
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_sigpending(args: SyscallArgs) -> u64 {
    let set_ptr = args[0] as *mut u64;
    let sigsetsize = args[1] as usize;

    // Validate sigsetsize
    if sigsetsize != 8 {
        return -errno::EINVAL as u64;
    }

    if set_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(set_ptr as usize, 8) {
        return -errno::EFAULT as u64;
    }

    // Get current process
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    unsafe {
        // Get pending signals (pending & ~blocked)
        let pending = (*current).pending.get_all();
        let blocked = (*current).sigmask;
        let deliverable = pending & !blocked;

        *set_ptr = deliverable;
    }

    0  // Success
}

/// sys_sigaltstack - Set/get alternate signal stack
///
/// # Arguments
/// - ss: new signal stack configuration (can be null)
/// - old_ss: save old signal stack configuration (can be null)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_sigaltstack(args: SyscallArgs) -> u64 {
    use crate::signal::{SignalStack, ss_flags};

    let ss_ptr = args[0] as *const SignalStack;
    let old_ss_ptr = args[1] as *mut SignalStack;

    // Get current process
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => return -errno::EPERM as u64,
    };

    let current = rq.lock().current;
    if current.is_null() {
        return -errno::EPERM as u64;
    }

    unsafe {
        // Save old signal stack configuration
        if !old_ss_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(old_ss_ptr as usize, core::mem::size_of::<SignalStack>()) {
                return -errno::EFAULT as u64;
            }
            *old_ss_ptr = (*current).sigstack;
        }

        // Set new signal stack configuration
        if !ss_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(ss_ptr as usize, core::mem::size_of::<SignalStack>()) {
                return -errno::EFAULT as u64;
            }
            let new_ss = *ss_ptr;

            // Check if currently executing on signal stack
            if (*current).sigstack.is_on_stack() {
                return -errno::EBUSY as u64;  // Signal stack in use
            }

            // Validate new stack size
            if (new_ss.ss_flags & ss_flags::SS_DISABLE) == 0 {
                if new_ss.ss_size < crate::signal::MINSIGSTKSZ as u64 {
                    return -errno::EINVAL as u64;  // Stack too small
                }
            }

            (*current).sigstack = new_ss;
        }
    }

    0  // Success
}

/// sys_tkill - Send signal to a thread
///
/// # Arguments
/// - args[0]: tid - Thread ID (same as PID for single-threaded processes)
/// - args[1]: sig - Signal number
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_tkill(args: SyscallArgs) -> u64 {
    let tid = args[0] as u32;
    let sig = args[1] as i32;

    // Validate signal number
    if sig < 0 || sig > 64 {
        return -errno::EINVAL as u64;
    }

    // Signal 0 is for permission checking only
    if sig == 0 {
        // Just check if process exists
        let task = unsafe { crate::sched::find_task_by_pid(tid) };
        if task.is_null() {
            return -errno::ESRCH as u64;
        }
        return 0;
    }

    // Find target task
    let task = unsafe { crate::sched::find_task_by_pid(tid) };
    if task.is_null() {
        return -errno::ESRCH as u64;
    }

    // Send signal using the existing send_signal function
    match crate::sched::send_signal(tid, sig) {
        Ok(()) => 0,
        Err(e) => -e as u64,
    }
}
