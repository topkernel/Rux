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
pub fn sys_rt_sigprocmask(args: SyscallArgs) -> i64 {
    let how = args[0] as i32;
    let set_ptr = args[1] as *const u64;  // SigSet is u64
    let oldset_ptr = args[2] as *mut u64;
    let sigsetsize = args[3] as usize;

    // Validate sigsetsize
    if sigsetsize != 8 {
        return -(errno::EINVAL as i64);
    }

    // Validate how parameter
    use crate::signal::sigprocmask_how;
    if how != sigprocmask_how::SIG_BLOCK
        && how != sigprocmask_how::SIG_UNBLOCK
        && how != sigprocmask_how::SIG_SETMASK
    {
        return -(errno::EINVAL as i64);
    }

    // Note: no alignment check — Linux performs none (get_user/put_user
    // handle unaligned sigset pointers; musl passes stack pointers).

    // Read new signal mask
    let new_mask = if !set_ptr.is_null() {
        // Validate user pointer
        if !crate::arch::riscv64::uaccess::access_ok(set_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        // Exception-table copy: unmapped page → EFAULT, not a kernel fault.
        match unsafe { crate::arch::riscv64::uaccess::get_user(set_ptr) } {
            Some(v) => v,
            None => return -(errno::EFAULT as i64),
        }
    } else {
        0
    };

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    // Get current signal mask
    // SAFETY: current from sched::current() is a valid Task pointer for the running task.
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

    // SIGKILL (9) and SIGSTOP (19) can never be blocked (POSIX/Linux):
    // strip their bits so the process stays killable/stoppable.
    let result_mask = result_mask & !((1u64 << 8) | (1u64 << 18));

    // Update current process signal mask
    // SAFETY: current is the running task's Task pointer from sched::current().
    unsafe {
        (*current).sigmask = result_mask;
    }

    // Return old signal mask
    if !oldset_ptr.is_null() {
        // Validate user pointer
        if !crate::arch::riscv64::uaccess::access_ok(oldset_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        // Exception-table copy: unmapped page → EFAULT, not a kernel fault.
        if !unsafe { crate::arch::riscv64::uaccess::put_user(oldset_ptr, old_mask) } {
            return -(errno::EFAULT as i64);
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
/// User-space `struct sigaction` on the RISC-V 64 ABI: 32 bytes with
/// sa_restorer at offset 16 and sa_mask at offset 24. The kernel's
/// internal SigAction (24 bytes, mask at 16) used to be copied verbatim,
/// so libc's sa_mask reads landed on the restorer pointer and oldact
/// wrote the kernel mask into the user sa_restorer field (review 5.4b).
#[repr(C)]
struct SigActionUser {
    sa_handler: usize,
    sa_flags: u64,
    sa_restorer: usize,
    sa_mask: u64,
}

pub fn sys_rt_sigaction(args: SyscallArgs) -> i64 {
    use crate::signal::{SigAction, Signal};

    let signum = args[0] as i32;
    let act_ptr = args[1] as *const SigActionUser;
    let oldact_ptr = args[2] as *mut SigActionUser;
    let sigsetsize = args[3] as usize;

    // Validate sigsetsize
    if sigsetsize != 8 {
        return -(errno::EINVAL as i64);
    }

    // Validate signal number
    if signum < 1 || signum > 64 {
        return -(errno::EINVAL as i64);
    }

    // SIGKILL and SIGSTOP cannot be caught or ignored
    if signum == Signal::SIGKILL as i32 || signum == Signal::SIGSTOP as i32 {
        return -(errno::EINVAL as i64);
    }

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    // SAFETY: current is the running task's Task pointer from sched::current();
    // oldact_ptr/act_ptr validated with access_ok where non-null.
    unsafe {
        let signal_struct = (*current).signal.as_mut();
        if signal_struct.is_none() {
            return -(errno::EINVAL as i64);
        }
        let sig_struct = signal_struct.unwrap();

        // Save old signal handling action (converted to the user ABI layout)
        if !oldact_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(oldact_ptr as usize, core::mem::size_of::<SigActionUser>()) {
                return -(errno::EFAULT as i64);
            }
            // Exception-table copy: unmapped page → EFAULT, not a kernel fault.
            let old_action = sig_struct.get_action(signum).unwrap_or_else(SigAction::new);
            let user_action = SigActionUser {
                sa_handler: old_action.sa_handler,
                sa_flags: old_action.sa_flags.bits(),
                sa_restorer: old_action.sa_restorer,
                sa_mask: old_action.sa_mask,
            };
            let src = &user_action as *const SigActionUser as *const u8;
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    oldact_ptr as *mut u8,
                    src,
                    core::mem::size_of::<SigActionUser>(),
                )
            };
            if uncopied > 0 {
                return -(errno::EFAULT as i64);
            }
        }

        // Set new signal handling action (parsed from the user ABI layout)
        if !act_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(act_ptr as usize, core::mem::size_of::<SigActionUser>()) {
                return -(errno::EFAULT as i64);
            }
            // Exception-table copy: unmapped page → EFAULT, not a kernel fault.
            let mut user_action = SigActionUser {
                sa_handler: 0,
                sa_flags: 0,
                sa_restorer: 0,
                sa_mask: 0,
            };
            let dst = &mut user_action as *mut SigActionUser as *mut u8;
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    dst,
                    act_ptr as *const u8,
                    core::mem::size_of::<SigActionUser>(),
                )
            };
            if uncopied > 0 {
                return -(errno::EFAULT as i64);
            }
            // sa_mask is stored verbatim: masking SIGKILL/SIGSTOP in a
            // handler's sa_mask is LEGAL (POSIX) — the bits simply have no
            // effect at delivery time (setup_frame re-strips the two
            // unblockable bits when composing the runtime mask). Stripping
            // here corrupted oldact round-trips (review batch-1).
            let new_action = SigAction {
                sa_handler: user_action.sa_handler,
                sa_flags: crate::signal::SigFlags::new(user_action.sa_flags),
                sa_mask: user_action.sa_mask,
                sa_restorer: user_action.sa_restorer,
            };
            match sig_struct.set_action(signum, new_action) {
                Ok(_) => 0,  // Success
                Err(_) => -(errno::EINVAL as i64),
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
pub fn sys_rt_sigreturn(regs: &mut crate::arch::riscv64::pt_regs::PtRegs) -> i64 {
    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    // SAFETY: current is the running task's Task pointer; sigframe_addr was set by
    // signal delivery and restore_sigcontext expects a valid Task pointer.
    unsafe {
        let frame_addr = (*current).sigframe_addr;

        // A zero frame address means rt_sigreturn was invoked without an
        // active signal frame (forged/direct ecall). Treat it exactly like
        // a corrupt frame (review IPC-L): force SIGSEGV instead of falling
        // through to ecall+4 with whatever registers the caller passed.
        let ok = if frame_addr != 0 {
            crate::signal::restore_sigcontext(current, frame_addr, regs)
        } else {
            false
        };
        if !ok {
            let pid = crate::process::current_pid();
            let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
        }

        // Return original return value saved in signal frame
        // Usually the value returned from interrupted system call (a0 = x10)
        // Note: restore_sigcontext has already restored regs, so just return regs.a0
        regs.a0 as i64
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
pub fn sys_sigpending(args: SyscallArgs) -> i64 {
    let set_ptr = args[0] as *mut u64;
    let sigsetsize = args[1] as usize;

    // Validate sigsetsize
    if sigsetsize != 8 {
        return -(errno::EINVAL as i64);
    }

    if set_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(set_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    // SAFETY: current is the running task's Task pointer; set_ptr validated with access_ok(8).
    unsafe {
        let pending = (*current).pending.get_all();
        let blocked = (*current).sigmask;
        // R7-D6: POSIX/Linux sigpending returns pending AND blocked (the
        // complement was written before — a blocked-and-pending signal
        // read as not-pending, breaking sigwait-style polling).
        let pending_and_blocked = pending & blocked;

        // Exception-table write (the old naked store could not produce
        // EFAULT on a bad page — it took a kernel fault instead).
        if !crate::arch::riscv64::uaccess::put_user(set_ptr, pending_and_blocked) {
            return -(errno::EFAULT as i64);
        }
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
pub fn sys_sigaltstack(args: SyscallArgs) -> i64 {
    use crate::signal::{SignalStack, ss_flags};

    let ss_ptr = args[0] as *const SignalStack;
    let old_ss_ptr = args[1] as *mut SignalStack;

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    // SAFETY: current is the running task's Task pointer; ss_ptr/old_ss_ptr validated
    // with access_ok where non-null; all user copies go through the
    // exception-table helpers.
    unsafe {
        // Save old signal stack configuration
        if !old_ss_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(old_ss_ptr as usize, core::mem::size_of::<SignalStack>()) {
                return -(errno::EFAULT as i64);
            }
            let old_ss = (*current).sigstack;
            if crate::arch::riscv64::uaccess::copy_to_user(
                old_ss_ptr as *mut u8,
                &old_ss as *const SignalStack as *const u8,
                core::mem::size_of::<SignalStack>(),
            ) != 0 {
                return -(errno::EFAULT as i64);
            }
        }

        // Set new signal stack configuration
        if !ss_ptr.is_null() {
            // Validate user pointer
            if !crate::arch::riscv64::uaccess::access_ok(ss_ptr as usize, core::mem::size_of::<SignalStack>()) {
                return -(errno::EFAULT as i64);
            }
            let mut new_ss = core::mem::MaybeUninit::<SignalStack>::zeroed();
            if crate::arch::riscv64::uaccess::copy_from_user(
                new_ss.as_mut_ptr() as *mut u8,
                ss_ptr as *const u8,
                core::mem::size_of::<SignalStack>(),
            ) != 0 {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: fully initialized by the copy above.
            let new_ss = new_ss.assume_init();

            // Check if currently executing on signal stack
            if (*current).sigstack.is_on_stack() {
                return -(errno::EBUSY as i64);  // Signal stack in use
            }

            // Validate new stack size
            if (new_ss.ss_flags as u32 & ss_flags::SS_DISABLE) == 0 {
                if new_ss.ss_size < crate::signal::MINSIGSTKSZ as u64 {
                    return -(errno::EINVAL as i64);  // Stack too small
                }
            }

            (*current).sigstack = new_ss;
        }
    }

    0  // Success
}

/// sys_signalfd4 - Create file descriptor for signal notifications
///
/// # Arguments
/// - args[0]: fd - existing signalfd (or -1 to create new)
/// - args[1]: mask - pointer to signal mask
/// - args[2]: flags - SFD_CLOEXEC, SFD_NONBLOCK
pub fn sys_signalfd4(args: SyscallArgs) -> i64 {
    let _fd = args[0] as i32;
    let mask_ptr = args[1] as *const u64;
    let _flags = args[2] as i32;

    if mask_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(mask_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }

    // signalfd requires full signal fd infrastructure
    -(errno::ENOSYS as i64)
}

/// sys_restart_syscall - Restart a system call after interruption
///
/// This syscall is used internally by the kernel to restart
/// interrupted system calls. Userspace should not call it directly.
pub fn sys_restart_syscall(_args: SyscallArgs) -> i64 {
    0
}

/// sys_rt_sigsuspend - Wait for a signal
///
/// # Arguments
/// - args[0]: mask - pointer to signal mask (u64)
/// - args[1]: sigsetsize - size of signal set (must be 8)
pub fn sys_rt_sigsuspend(args: SyscallArgs) -> i64 {
    let mask_ptr = args[0] as *const u64;
    let sigsetsize = args[1] as usize;

    if sigsetsize != 8 {
        return -(errno::EINVAL as i64);
    }
    if mask_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(mask_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }

    // Exception-table copy: unmapped page → EFAULT, not a kernel fault.
    let new_mask = match unsafe { crate::arch::riscv64::uaccess::get_user(mask_ptr) } {
        Some(v) => v,
        None => return -(errno::EFAULT as i64),
    };
    // SIGKILL/SIGSTOP can never be blocked, not even via sigsuspend.
    let new_mask = new_mask & !((1u64 << 8) | (1u64 << 18));

    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    // SAFETY: current is the running task's Task pointer from sched::current().
    unsafe {
        // Atomically set new mask and wait
        let old_mask = (*current).sigmask;
        (*current).sigmask = new_mask;

        // Sleep until a signal is delivered.
        // The race between checking pending and sleeping is resolved by
        // checking pending AFTER setting state to INTERRUPTIBLE but BEFORE
        // calling schedule(). If a signal arrives between the check and
        // schedule(), the wakeup will find us in INTERRUPTIBLE and skip us,
        // but the next iteration will see the pending signal.
        loop {
            let pending = (*current).pending.get_all();
            let blocked = (*current).sigmask;
            if pending & !blocked != 0 {
                // Signal pending. Do NOT restore the old mask here: the
                // delivery filter on the way back to userspace must see the
                // SUSPEND mask so the signal is actually delivered; the old
                // mask is reinstated just before the handler runs (see
                // check_and_deliver_signals) and again by sigreturn.
                (*current).sigmask_restore = old_mask;
                (*current).sigmask_restore_valid = true;
                return -(errno::EINTR as i64);
            }
            // Set state BEFORE re-checking to close the race window.
            // If a signal arrives here, the signal delivery path will see
            // INTERRUPTIBLE and call wake_up_process(), which sets us back
            // to RUNNING before schedule() yields the CPU.
            (*current).set_state(crate::process::task::TaskState::new(
                crate::process::task::TaskState::INTERRUPTIBLE
            ));
            // Re-check after setting state (signal may have arrived)
            let pending2 = (*current).pending.get_all();
            if pending2 & !blocked != 0 {
                // Signal arrived while we were preparing to sleep
                (*current).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING
                ));
                // R8-5 (NEW-C2): undo a concurrent wake enqueue.
                crate::sched::dequeue_task(&*current);
                (*current).sigmask_restore = old_mask;
                (*current).sigmask_restore_valid = true;
                return -(errno::EINTR as i64);
            }
            crate::sched::schedule();
        }
    }
}

/// sys_tkill - Send signal to a thread
///
/// # Arguments
/// - args[0]: tid - Thread ID (same as PID for single-threaded processes)
/// - args[1]: sig - Signal number
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_tkill(args: SyscallArgs) -> i64 {
    let tid = args[0] as u32;
    let sig = args[1] as i32;

    // Validate signal number
    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }

    // Find target task (also needed for the sig==0 permission probe)
    // SAFETY: find_task_by_pid returns null if tid not found; result checked below.
    let task = unsafe { crate::sched::find_task_by_pid(tid) };
    if task.is_null() {
        return -(errno::ESRCH as i64);
    }

    // Permission check for every signal, including the sig==0 probe
    // (Linux do_tkill → check_kill_permission).
    // SAFETY: task is non-null and stable while referenced.
    if !unsafe { crate::security::can_send_signal((*task).cred()) } {
        return -(errno::EPERM as i64);
    }

    // Signal 0 is for permission/existence checking only
    if sig == 0 {
        return 0;
    }

    // Send signal using the existing send_signal function.
    // tkill is THREAD-directed: no group spread. Err values from
    // send_signal are already negative errnos — the old `-(e as i64)`
    // double-negated them into positive garbage return values.
    match crate::signal::send_signal(tid, sig) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}
