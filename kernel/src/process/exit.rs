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

    // Free heap-allocated PtRegs from fork.
    // copy_thread() allocates PtRegs on the heap for the child's first
    // context switch; clear_fork_child() keeps the pointer so we can free it here.
    let pt_regs_ptr = (*task).fork_pt_regs();
    if !pt_regs_ptr.is_null() {
        use alloc::alloc::{dealloc, Layout};
        let layout = Layout::from_size_align(
            core::mem::size_of::<crate::arch::riscv64::pt_regs::PtRegs>(), 16
        ).unwrap();
        dealloc(pt_regs_ptr as *mut u8, layout);
    }

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

        // ===== clear_child_tid: Write 0 and futex-wake (pthread_join support) =====
        // Linux: mm_release() does put_user(0, tsk->clear_child_tid) + FUTEX_WAKE.
        let tid_ptr = (*current).clear_child_tid();
        if !tid_ptr.is_null() {
            // Write 0 to the tid pointer in user memory
            let zero: i32 = 0;
            crate::arch::riscv64::uaccess::copy_to_user(
                tid_ptr as *mut u8,
                &zero as *const i32 as *const u8,
                core::mem::size_of::<i32>(),
            );
            // Wake any thread waiting on this futex (FUTEX_WAKE, 1 waiter)
            crate::sync::futex::futex_wake(
                tid_ptr as usize,
                crate::sync::futex::FUTEX_PRIVATE_FLAG as u32,
                1,
                0xffffffff,
            );
            (*current).set_clear_child_tid(core::ptr::null_mut());
        }

        // ===== Clean up futex waiters =====
        crate::sync::futex::futex_cleanup(current);

        // ===== Clean up POSIX MQ fd entries =====
        crate::ipc::posix_mq::mq_fds_cleanup(current);

        // ===== Disarm interval timers (ITIMER_REAL/VIRTUAL/PROF) =====
        for i in 0..3 {
            let old_id = (*current).itimer_ids[i].swap(0, core::sync::atomic::Ordering::AcqRel);
            if old_id != 0 {
                crate::timer::del_timer(old_id);
            }
        }

        // ===== Disarm POSIX timers =====
        {
            let mut timers = (*current).posix_timers.lock();
            for pt in timers.drain(..) {
                if pt.kernel_timer_id != 0 {
                    crate::timer::del_timer(pt.kernel_timer_id);
                }
            }
        }

        // ===== Reverse SEM_UNDO adjustments =====
        crate::ipc::sysv_sem::sem_undo_exit(current);

        // Set process state to Zombie
        (*current).set_state(TaskState::new(TaskState::ZOMBIE));

        // Dequeue from global run queue
        crate::sched::dequeue_task(&*current);

        // ===== exit_notify: Defer parent notification =====
        // CRITICAL: Do NOT wake the parent here.  If the parent runs on
        // another CPU it can reap (free_task_slot) this task before
        // schedule() switches us away, causing a use-after-free that
        // corrupts ti_cpu (initialised to -1 in Task::new).
        // Instead, store the parent PID in a per-CPU deferred slot;
        // __schedule processes it AFTER the context switch, when this
        // task is no longer running on any CPU.
        if parent_pid != 0 {
            crate::sched::defer_exit_notify(parent_pid);
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
                    && !child.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                {
                    stopped_child = Some(child_ptr);
                }
            });

            // If found zombie child, reap it
            if let Some(child_ptr) = zombie_child {
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

                // Mark this stop event as reported (only report once)
                child.stop_reported.store(true, core::sync::atomic::Ordering::Release);

                // Note: stopped child is NOT reaped, it stays in children list
                return Ok(child_pid);
            }

            // No zombie or stopped child found
            if found_child {
                // Atomically add to waitqueue AND set INTERRUPTIBLE.
                // Prevents lost-wakeup race where child exits between
                // add() and set_state(), marking the entry woken but
                // wake_up_process skips the actual wake (task still RUNNING).
                (*current).wait_chldexit.prepare_to_wait(current, false, true);

                // Recheck for zombie after prepare_to_wait (waker may have
                // fired between the initial scan and prepare_to_wait).  If
                // a child is already zombie, finish_wait restores RUNNING
                // state and we reap it immediately — no schedule() needed.
                {
                    let mut found_zombie = false;
                    (*current).for_each_child(|child_ptr| {
                        if pid > 0 && (*child_ptr).pid() != pid as u32 {
                            return;
                        }
                        if (*child_ptr).state() == TaskState::new(TaskState::ZOMBIE) {
                            found_zombie = true;
                        }
                    });
                    if found_zombie {
                        (*current).wait_chldexit.finish_wait(current);
                        continue; // re-enter loop to reap zombie
                    }
                }

                // Check for pending signals before sleeping
                use crate::signal;
                if signal::signal_pending() {
                    (*current).wait_chldexit.finish_wait(current);
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
                }

                // Enable interrupts before schedule(). We're in syscall context
                // (SIE=0). Without this, timer IRQ can't fire and the task
                // can never be rescheduled.
                crate::arch::riscv64::cpu::restore_irq(true);

                // Schedule other processes
                crate::sched::schedule();

                // After wakeup: state is RUNNING (set by wake_up_process).
                // Clean up waitqueue entry before next iteration.
                (*current).wait_chldexit.finish_wait(current);
            } else {
                // No child processes at all
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
                    let mut was_killed = false;
                    if raw_exit >= 0 {
                        result_code = raw_exit as i32;
                    } else {
                        result_code = (-raw_exit) as i32;
                        was_killed = true;
                    }
                    result_kind = if was_killed { 3 } else { 0 };
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
                } else if result_kind == 3 {
                    // Killed by signal
                    (CLD_KILLED, result_code)
                } else {
                    // Normal exit
                    (CLD_EXITED, result_code)
                };

                // Write siginfo to user
                write_siginfo(infop, si_code, child_pid, child_uid, si_status);

                // Reap zombie unless WNOWAIT
                if (result_kind == 0 || result_kind == 3) && options & WNOWAIT == 0 {
                    release_task(child_ptr);
                }

                // Clear stop signal unless WNOWAIT
                if result_kind == 1 && options & WNOWAIT == 0 {
                    (*child_ptr).set_stop_signal(0);
                }

                return Ok(());
            }

            // No matching child found
            if found_child {
                // Children exist but none in target state
                if options & WNOHANG != 0 {
                    return Err(errno::Errno::TryAgain.as_neg_i32());
                }

                // Atomically add to waitqueue AND set INTERRUPTIBLE.
                (*current).wait_chldexit.prepare_to_wait(current, false, true);

                if crate::signal::signal_pending() {
                    (*current).wait_chldexit.finish_wait(current);
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
                }

                // Enable interrupts before schedule() — we're in syscall
                // context (SIE=0). Without this, timer IRQ can't fire and
                // the task can never be rescheduled.
                crate::arch::riscv64::cpu::restore_irq(true);

                crate::sched::schedule();

                // After wakeup: state is RUNNING (set by wake_up_process).
                (*current).wait_chldexit.finish_wait(current);
            } else {
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}
