//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process exit and wait implementation
//!
//! Linux equivalent: kernel/exit.c
//!
//! - do_exit: Process termination (exit_mm, exit_files, exit_notify)
//! - release_task: Reap zombie child resources
//! - do_wait / do_wait_nonblock: Wait for child process state change

use crate::errno;
use crate::config::MAX_TASKS;
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
pub(crate) unsafe fn release_task(task: *mut Task) {
    // Remove from PID hash table before freeing resources
    crate::process::pid_hash::pid_hash_remove((*task).pid());

    // Free kernel stack
    (*task).free_kernel_stack();

    // Clear Arc references (this will decrement reference counts)
    (*task).set_address_space(None);
    (*task).set_fdtable(None);
    (*task).signal = None;
    (*task).set_fs(None);

    // Free PID
    crate::process::pid::free_pid((*task).pid());

    // Note: The task struct itself is in the static task pool,
    // so we don't free it. It will be reused when alloc_task_slot()
    // wraps around or when we implement proper task slot management.
}

/// Process exit (Linux: do_exit)
///
/// Called when a process terminates (sys_exit, fatal signal, etc.).
/// This function never returns.
///
/// Steps (matching Linux do_exit):
/// 1. Set exit code
/// 2. exit_mm: Release address space
/// 3. exit_files: Release file descriptor table
/// 4. Set ZOMBIE state, remove from run queue
/// 5. exit_notify: Send SIGCHLD to parent, wake parent's wait queue
/// 6. Release kernel big lock
/// 7. schedule() (never returns)
pub fn do_exit(exit_code: i32) -> ! {
    use crate::signal::Signal;
    use crate::config::MAX_CPUS;

    if let Some(rq) = crate::sched::this_cpu_rq() {
        unsafe {
            let mut rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                // No current process, halt directly
                loop {
                    asm!("wfi", options(nomem, nostack));
                }
            }

            let current_pid = (*current).pid();
            let parent_pid = (*current).ppid();

            crate::pr_info!("exit: pid={}, exit_code={}, ppid={}",
                current_pid, exit_code, parent_pid);

            // Set exit code (Linux: tsk->exit_code = code)
            (*current).set_exit_code(exit_code);

            // ===== exit_mm: Release address space =====
            // Linux: exit_mm() sets current->mm = NULL and calls mmput()
            // Setting address_space to None decrements Arc refcount
            (*current).set_address_space(None);
            (*current).clear_active_mm();

            // ===== exit_files: Release file descriptor table =====
            // Linux: exit_files() sets current->files = NULL
            // Setting fdtable to None decrements Arc refcount
            (*current).set_fdtable(None);

            // Set process state to Zombie (Linux: exit_notify sets EXIT_ZOMBIE)
            (*current).set_state(TaskState::new(TaskState::ZOMBIE));

            // Remove from run queue (but keep in parent's children list for wait())
            // do_wait() uses for_each_child() to find zombie children
            for i in 0..MAX_TASKS {
                if rq_inner.tasks[i] == current {
                    rq_inner.tasks[i] = core::ptr::null_mut();
                    rq_inner.nr_running -= 1;
                    break;
                }
            }

            // Release run queue lock
            drop(rq_inner);

            // ===== exit_notify: Send SIGCHLD to parent =====
            // Linux: do_notify_parent() sends SIGCHLD
            if parent_pid != 0 {
                let _ = crate::signal::send_signal(parent_pid, Signal::SIGCHLD as i32);

                // Wake up parent's wait_chldexit queue (Linux: __wake_up_parent)
                let parent = crate::process::pid_hash::pid_hash_lookup(parent_pid);
                if !parent.is_null() {
                    // Wake parent's child exit wait queue
                    (*parent).wait_chldexit.wake_up_all();
                }
            }

            // Release kernel big lock (must release when process exits, otherwise other processes can't acquire lock)
            crate::sync::kernel_lock_release();

            // ===== do_task_dead: Final schedule, never returns =====
            // Linux: do_task_dead() calls __schedule() with TASK_DEAD
            crate::sched::schedule();

            // Never reached here
            loop {
                asm!("wfi", options(nomem, nostack));
            }
        }
    } else {
        // No run queue, halt directly
        loop {
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }
        }
    }
}

/// Blocking wait for child process state change (Linux: do_wait)
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

    unsafe {
        let current = if let Some(rq) = crate::sched::this_cpu_rq() {
            rq.lock().current
        } else {
            return Err(errno::Errno::NoChild.as_neg_i32());
        };

        if current.is_null() {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        // Create wait queue entry (Linux: init_waitqueue_func_entry)
        let wait_entry = WaitQueueEntry::new(current, false);

        // Add to wait_chldexit queue (Linux: add_wait_queue)
        (*current).wait_chldexit.add(wait_entry);

        loop {
            let mut found_child = false;
            let mut zombie_child: Option<*mut Task> = None;
            let mut stopped_child: Option<*mut Task> = None;

            const WUNTRACED: i32 = 0x00000002;

            // Use for_each_child to iterate over children (Linux-style)
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

                // Encode exit status per Linux waitpid ABI:
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

                // Remove from parent's children list
                (*current).remove_child(child_ptr);

                // Release task resources (kernel stack, Arc refs, PID)
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
                // Set state to INTERRUPTIBLE (Linux: set_current_state(TASK_INTERRUPTIBLE))
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));

                // Check for pending signals before sleeping
                use crate::signal;
                if signal::signal_pending() {
                    // Remove from wait queue and return EINTR
                    (*current).wait_chldexit.remove(current);
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());
                }

                // Release kernel lock before schedule
                crate::sync::kernel_lock_release();

                // Schedule other processes (Linux: schedule())
                crate::sched::schedule();

                // Re-acquire kernel lock after wakeup
                crate::sync::kernel_lock_acquire();

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

/// Non-blocking wait for child process (Linux: wait4 with WNOHANG)
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
    use crate::config::MAX_CPUS;

    unsafe {
        let current = if let Some(rq) = crate::sched::this_cpu_rq() {
            rq.lock().current
        } else {
            // No runqueue, means uninitialized, return ECHILD directly
            return Err(errno::Errno::NoChild.as_neg_i32());
        };

        if current.is_null() {
            // current is null (possibly called from non-process context), return ECHILD
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        // Return ECHILD because idle task has no child processes
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let mut found_child = false;

        // Traverse all CPU run queues to find zombie child processes
        for cpu_id in 0..MAX_CPUS {
            if let Some(rq) = crate::sched::cpu_rq(cpu_id) {
                let mut rq_inner = rq.lock();

                for i in 0..MAX_TASKS {
                    let task_ptr = rq_inner.tasks[i];
                    if task_ptr.is_null() {
                        continue;
                    }

                    let task = &*task_ptr;

                    // Check if it's a child process
                    if task.ppid() != current_pid {
                        continue;
                    }

                    found_child = true;

                    // Check if it's the specified PID (if specified)
                    if pid > 0 && task.pid() != pid as u32 {
                        continue;
                    }

                    // Check if it's in Zombie state
                    if task.state() == TaskState::new(TaskState::ZOMBIE) {
                        let child_pid = task.pid();
                        let raw_exit = task.exit_code();

                        // Encode exit status per Linux waitpid ABI
                        let status: i32 = if raw_exit >= 0 {
                            (((raw_exit as u32) & 0xFF) << 8) as i32
                        } else {
                            (-(raw_exit as i32) as u32 & 0x7F) as i32
                        };

                        // Write exit status
                        if !status_ptr.is_null() {
                            *status_ptr = status;
                        }

                        // Remove from run queue
                        rq_inner.tasks[i] = core::ptr::null_mut();
                        rq_inner.nr_running -= 1;

                        // Release run queue lock BEFORE calling release_task to avoid deadlock
                        drop(rq_inner);

                        // Release task resources (kernel stack, Arc refs, PID)
                        release_task(task_ptr);

                        return Ok(child_pid);
                    }
                }
            }
        }

        // Has child processes but none have exited yet
        if found_child {
            // Return EAGAIN (-11), sys_wait4 will convert it to 0
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            // No child processes
            // Return ECHILD (-10)
            Err(errno::Errno::NoChild.as_neg_i32())
        }
    }
}
