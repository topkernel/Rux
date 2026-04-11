//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process exit and wait implementation
//!
//! - do_exit: Process termination (exit_mm, exit_files, exit_notify)
//! - release_task: Reap zombie child resources
//! - do_wait / do_wait_nonblock: Wait for child process state change

use crate::errno;
use crate::process::task::{Pid, Task, TaskState};
use core::arch::asm;

/// Release task resources when being reaped by parent
///
/// This function is called by do_wait() when reaping a zombie child.
/// It frees all resources associated with the task:
/// - Kernel stack
/// - Address space (Arc reference)
/// - File descriptor table (Arc reference)
/// - Signal struct (Arc reference)
/// - Filesystem info (Arc reference)
/// - PID
///
/// # Safety
/// Caller must ensure task is in ZOMBIE state and not currently running
// SAFETY: Caller guarantees task is a zombie and no CPU is running it. The RCU
/// grace period in synchronize_rcu() ensures no readers hold stale references.
pub(crate) unsafe fn release_task(task: *mut Task) {
    // Remove from PID hash table before freeing resources
    crate::process::pid_hash::pid_hash_remove((*task).pid());

    // Wait for any RCU readers that may still be traversing this task's
    // pid_hash_links node to finish before we free the task memory.
    crate::sync::rcu::synchronize_rcu();

    // Detach from parent's children list (must happen before freeing task memory)
    let parent_ptr = (*task).parent_ptr();
    if let Some(parent) = parent_ptr {
        (*parent).remove_child(task);
    }

    // Free kernel stack
    (*task).free_kernel_stack();

    // Clear Arc references (this will decrement reference counts)
    (*task).set_address_space(None);
    (*task).set_fdtable(None);
    (*task).signal = None;
    (*task).set_fs(None);

    // Free PID
    crate::process::pid::free_pid((*task).pid());

    // Free Task struct back to kernel heap
    crate::sched::free_task_slot(task);
}

/// Process exit
///
/// Called when a process terminates (sys_exit, fatal signal, etc.).
/// This function never returns.
///
/// Steps:
/// 1. Set exit code
/// 2. exit_mm: Release address space
/// 3. exit_files: Release file descriptor table
/// 4. Set ZOMBIE state, remove from run queue
/// 5. exit_notify: Send SIGCHLD to parent, wake parent's wait queue
/// 6. Release kernel big lock
/// 7. schedule() (never returns)
pub fn do_exit(exit_code: i32) -> ! {
    use crate::signal::Signal;

    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => {
            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                unsafe { asm!("wfi", options(nomem, nostack)); }
            }
        }
    };

    // SAFETY: current is the raw pointer to the calling task, guaranteed valid since
    // we are the currently executing task and will never return from this function.
    unsafe {
        let current_pid = (*current).pid();
        let parent_pid = (*current).ppid();

        crate::pr_info!("exit: pid={}, exit_code={}, ppid={}",
            current_pid, exit_code, parent_pid);

        // Set exit code
        (*current).set_exit_code(exit_code);

        // ===== exit_mm: Release address space =====
        // Iterate VMAs to detach shared memory segments (decrement nattch)
        if let Some(as_ref) = (*current).address_space_arc() {
            let vma_mgr = as_ref.vma_read();
            for vma in vma_mgr.iter() {
                if vma.vma_type() == crate::mm::vma::VmaType::SharedMemory {
                    let shmid = vma.file_fd();
                    if shmid >= 0 {
                        crate::ipc::sysv_shm::shm_detach_vma(shmid);
                    }
                }
            }
        }
        (*current).set_address_space(None);
        (*current).clear_active_mm();

        // ===== exit_files: Release file descriptor table =====
        (*current).set_fdtable(None);

        // ===== Clean up futex waiters =====
        crate::sync::futex::futex_cleanup(current);

        // ===== Clean up POSIX MQ fd entries =====
        crate::ipc::posix_mq::mq_fds_cleanup(current);

        // ===== Reverse SEM_UNDO adjustments =====
        crate::ipc::sysv_sem::sem_undo_exit(current);

        // Set process state to Zombie
        (*current).set_state(TaskState::new(TaskState::ZOMBIE));

        // Dequeue from global run queue
        crate::sched::dequeue_task(&*current);

        // ===== exit_notify: Send SIGCHLD to parent =====
        if parent_pid != 0 {
            let _ = crate::signal::send_signal(parent_pid, Signal::SIGCHLD as i32);

            // Wake up parent's wait_chldexit queue
            let parent = crate::process::pid_hash::pid_hash_lookup(parent_pid);
            if !parent.is_null() {
                (*parent).wait_chldexit.wake_up_all();
            }
        }

        // ===== do_task_dead: Final schedule, never returns =====
        crate::sched::schedule();

        loop {
            // SAFETY: `wfi` is a plain hint instruction with no side effects.
            asm!("wfi", options(nomem, nostack));
        }
    }
}

/// Blocking wait for child process state change
///
/// # Arguments
/// * `pid` - PID to wait for (-1 = any child, >0 = specific PID)
/// * `status_ptr` - User pointer to store exit status
/// * `options` - Wait options (WUNTRACED, etc.)
///
/// # Returns
/// * `Ok(child_pid)` - Child PID that changed state
/// * `Err(-ECHILD)` - No children
/// * `Err(-EINTR)` - Interrupted by signal
pub fn do_wait(pid: i32, status_ptr: *mut i32, options: i32) -> Result<Pid, i32> {
    use crate::process::wait::WaitQueueEntry;

    // SAFETY: current is the calling task's raw pointer, valid throughout the wait loop.
    // The task sleeps via schedule() in INTERRUPTIBLE state so it won't be freed.
    unsafe {
        let current = match crate::sched::current() {
            Some(c) => c as *mut Task,
            None => return Err(errno::Errno::NoChild.as_neg_i32()),
        };

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        // Create wait queue entry
        let wait_entry = WaitQueueEntry::new(current, false);

        // Add to wait_chldexit queue
        (*current).wait_chldexit.add(wait_entry);

        loop {
            let mut found_child = false;
            let mut zombie_child: Option<*mut Task> = None;
            let mut stopped_child: Option<*mut Task> = None;

            const WUNTRACED: i32 = 0x00000002;

            // Iterate over children
            (*current).for_each_child(|child_ptr| {
                let child = &*child_ptr;

                // Check if it's the specified PID (if specified)
                if pid > 0 && child.pid() != pid as u32 {
                    return;
                }

                found_child = true;

                // Check if it's in Zombie state
                if child.state() == TaskState::new(TaskState::ZOMBIE) {
                    zombie_child = Some(child_ptr);
                } else if options & WUNTRACED != 0
                    && child.state() == TaskState::new(TaskState::STOPPED)
                {
                    stopped_child = Some(child_ptr);
                }
            });

            // If found zombie child, reap it
            if let Some(child_ptr) = zombie_child {
                // Remove from wait queue before returning
                (*current).wait_chldexit.remove(current);

                let child = &*child_ptr;
                let child_pid = child.pid();
                let raw_exit = child.exit_code();

                // Encode exit status per waitpid ABI:
                // - Normal exit: status = (exit_code & 0xFF) << 8  (WIFEXITED, WEXITSTATUS)
                // - Killed by signal: status = |signal_number|      (WIFSIGNALED, WTERMSIG)
                let status: i32 = if raw_exit >= 0 {
                    (((raw_exit as u32) & 0xFF) << 8) as i32
                } else {
                    (-(raw_exit as i32) as u32 & 0x7F) as i32
                };

                // Write exit status safely using copy_to_user
                if !status_ptr.is_null() {
                    let _uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                        status_ptr as *mut u8,
                        &status as *const i32 as *const u8,
                        core::mem::size_of::<i32>()
                    );
                }

                // Release task resources (kernel stack, Arc refs, PID)
                // Note: remove_child is done inside release_task()
                release_task(child_ptr);

                return Ok(child_pid);
            }

            // If found stopped child (WUNTRACED), report it
            if let Some(child_ptr) = stopped_child {
                // Remove from wait queue before returning
                (*current).wait_chldexit.remove(current);

                let child = &*child_ptr;
                let child_pid = child.pid();
                let stop_sig = child.stop_signal();

                // Encode stopped status: (stop_signal << 8) | 0x7F
                let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;

                // Write status safely using copy_to_user
                if !status_ptr.is_null() {
                    let _uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                        status_ptr as *mut u8,
                        &status as *const i32 as *const u8,
                        core::mem::size_of::<i32>()
                    );
                }

                // Note: stopped child is NOT reaped, it stays in children list
                return Ok(child_pid);
            }

            // No zombie or stopped child found
            if found_child {
                // Set state to INTERRUPTIBLE
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));

                // Check for pending signals before sleeping
                use crate::signal;
                if signal::signal_pending() {
                    // Remove from wait queue and return EINTR
                    (*current).wait_chldexit.remove(current);
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
                }

                // Schedule other processes
                crate::sched::schedule();

                // Back to RUNNING state
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            } else {
                // No child processes at all
                (*current).wait_chldexit.remove(current);
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}

/// Non-blocking wait for child process
///
/// # Arguments
/// * `pid` - PID to wait for (-1 = any child, >0 = specific PID)
/// * `status_ptr` - User pointer to store exit status
///
/// # Returns
/// * `Ok(child_pid)` - Child PID that has exited
/// * `Err(-ECHILD)` - No children
/// * `Err(-EAGAIN)` - Children exist but none have exited (sys_wait4 converts to 0)
pub fn do_wait_nonblock(pid: i32, status_ptr: *mut i32) -> Result<Pid, i32> {
    // SAFETY: current is the calling task's raw pointer, valid throughout this call.
    unsafe {
        let current = match crate::sched::current() {
            Some(c) => c as *mut Task,
            None => return Err(errno::Errno::NoChild.as_neg_i32()),
        };

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let mut found_child = false;
        let mut zombie_ptr: Option<*mut Task> = None;

        // Scan children to find a zombie — do NOT modify the list during iteration
        (*current).for_each_child(|child_ptr| {
            let child = &*child_ptr;

            if pid > 0 && child.pid() != pid as u32 {
                return;
            }

            found_child = true;

            if child.state() == TaskState::new(TaskState::ZOMBIE) && zombie_ptr.is_none() {
                zombie_ptr = Some(child_ptr);
            }
        });

        if let Some(child_ptr) = zombie_ptr {
            let child = &*child_ptr;
            let child_pid = child.pid();
            let raw_exit = child.exit_code();

            let status: i32 = if raw_exit >= 0 {
                (((raw_exit as u32) & 0xFF) << 8) as i32
            } else {
                (-(raw_exit as i32) as u32 & 0x7F) as i32
            };

            if !status_ptr.is_null() {
                if crate::arch::riscv64::uaccess::copy_to_user(
                    status_ptr as *mut u8,
                    &status as *const i32 as *const u8,
                    core::mem::size_of::<i32>(),
                ) != 0 {
                    return Err(crate::errno::Errno::BadAddress.as_neg_i32());
                }
            }

            release_task(child_ptr);
            return Ok(child_pid);
        }

        if found_child {
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            Err(errno::Errno::NoChild.as_neg_i32())
        }
    }
}

/// waitid options (match Linux wait.h)
const WNOHANG: i32     = 0x00000001;
const WUNTRACED: i32   = 0x00000002;
const WSTOPPED: i32    = WUNTRACED;
const WEXITED: i32     = 0x00000004;
const WCONTINUED: i32  = 0x00000008;
const WNOWAIT: i32     = 0x01000000;

/// waitid idtype
const P_ALL: i32  = 0;
const P_PID: i32  = 1;
const P_PGID: i32 = 2;

/// CLD_* si_code values for siginfo_t
const CLD_EXITED: i32    = 1;
const CLD_KILLED: i32    = 2;
const CLD_DUMPED: i32    = 3;
const CLD_STOPPED: i32   = 5;
const CLD_CONTINUED: i32 = 6;

/// Write waitid siginfo_t fields to user memory.
///
/// Only writes the fields used by waitid (si_signo, si_errno, si_code,
/// si_pid, si_uid, si_status), leaving the rest of the 128-byte
/// siginfo_t untouched.
// SAFETY: Caller must ensure infop points to a valid, writable user buffer of at
/// least 128 bytes. copy_to_user handles user-space access safely.
unsafe fn write_siginfo(
    infop: *mut u8,
    si_code: i32,
    si_pid: u32,
    si_uid: u32,
    si_status: i32,
) {
    use crate::arch::riscv64::uaccess::copy_to_user;
    let base = infop;
    let four = 4u32;

    let signo = 17i32; // SIGCHLD
    let _ = copy_to_user(base, &signo as *const i32 as *const u8, 4);
    let _ = copy_to_user(base.add(4), &0i32 as *const i32 as *const u8, 4);
    let _ = copy_to_user(base.add(8), &si_code as *const i32 as *const u8, 4);
    // offset 12: padding
    let _ = copy_to_user(base.add(16), &si_pid as *const u32 as *const u8, 4);
    let _ = copy_to_user(base.add(20), &si_uid as *const u32 as *const u8, 4);
    let _ = copy_to_user(base.add(24), &si_status as *const i32 as *const u8, 4);
}

/// Blocking waitid: wait for child process state change
///
/// # Arguments
/// * `idtype` - P_ALL (0), P_PID (1), or P_PGID (2)
/// * `id` - PID or PGID to wait for (ignored if P_ALL)
/// * `infop` - User pointer to siginfo_t
/// * `options` - WNOHANG | WEXITED | WSTOPPED | WCONTINUED | WNOWAIT
///
/// # Returns
/// * `Ok(())` - Success (child info written to infop)
/// * `Err(-ECHILD)` - No children
/// * `Err(-EINTR)` - Interrupted by signal
pub fn do_waitid(
    idtype: i32,
    id: i32,
    infop: *mut u8,
    options: i32,
) -> Result<(), i32> {
    use crate::process::wait::WaitQueueEntry;

    // Must specify at least one of WEXITED/WSTOPPED/WCONTINUED
    if options & (WEXITED | WSTOPPED | WCONTINUED) == 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // SAFETY: current is the calling task's raw pointer, valid throughout the wait loop.
    unsafe {
        let current = match crate::sched::current() {
            Some(c) => c as *mut Task,
            None => return Err(errno::Errno::NoChild.as_neg_i32()),
        };

        if (*current).pid() == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let wait_entry = WaitQueueEntry::new(current, false);
        (*current).wait_chldexit.add(wait_entry);

        loop {
            let mut found_child = false;
            let mut result_child: Option<*mut Task> = None;
            let mut result_code: i32 = 0;
            let mut result_kind: i32 = 0; // 0=zombie, 1=stopped, 2=continued

            (*current).for_each_child(|child_ptr| {
                let child = &*child_ptr;

                // idtype filter
                if idtype == P_PID && child.pid() != id as u32 {
                    return;
                }
                if idtype == P_PGID && child.pgid() != id as u32 {
                    return;
                }
                // P_ALL: match any child

                found_child = true;

                // Check for zombie (exited) — only if WEXITED
                if options & WEXITED != 0
                    && child.state() == TaskState::new(TaskState::ZOMBIE)
                {
                    result_child = Some(child_ptr);
                    let raw_exit = child.exit_code();
                    if raw_exit >= 0 {
                        result_code = raw_exit as i32;
                        result_kind = 0; // exited
                    } else {
                        result_code = (-raw_exit) as i32;
                        result_kind = 0; // killed
                    }
                }
                // Check for stopped — only if WSTOPPED
                else if options & WSTOPPED != 0
                    && child.state() == TaskState::new(TaskState::STOPPED)
                {
                    // Don't report if already reported (stop_signal == 0)
                    if child.stop_signal() != 0 {
                        result_child = Some(child_ptr);
                        result_code = child.stop_signal();
                        result_kind = 1; // stopped
                    }
                }
                // Check for continued — only if WCONTINUED
                else if options & WCONTINUED != 0 {
                    // A continued child would have been STOPPED with
                    // stop_signal == 0 (cleared by SIGCONT handler).
                    // For now this is a future enhancement.
                }
            });

            if let Some(child_ptr) = result_child {
                let child = &*child_ptr;
                let child_pid = child.pid();
                let child_uid = child.cred().uid;

                // Encode si_code and si_status
                let (si_code, si_status) = if result_kind == 1 {
                    // Stopped
                    (CLD_STOPPED, result_code)
                } else if result_code >= 0 {
                    // Normal exit
                    (CLD_EXITED, result_code)
                } else {
                    // Killed by signal
                    (CLD_KILLED, result_code)
                };

                // Write siginfo to user
                write_siginfo(infop, si_code, child_pid, child_uid, si_status);

                // Reap zombie unless WNOWAIT
                if result_kind == 0 && options & WNOWAIT == 0 {
                    release_task(child_ptr);
                }

                // Clear stop signal unless WNOWAIT
                if result_kind == 1 && options & WNOWAIT == 0 {
                    (*child_ptr).set_stop_signal(0);
                }

                (*current).wait_chldexit.remove(current);
                return Ok(());
            }

            // No matching child found
            if found_child {
                // Children exist but none in target state
                if options & WNOHANG != 0 {
                    (*current).wait_chldexit.remove(current);
                    return Err(errno::Errno::TryAgain.as_neg_i32());
                }

                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));

                if crate::signal::signal_pending() {
                    (*current).wait_chldexit.remove(current);
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
                }

                crate::sched::schedule();
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            } else {
                (*current).wait_chldexit.remove(current);
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}
