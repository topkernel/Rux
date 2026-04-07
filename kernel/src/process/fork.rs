//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Process creation (fork/clone) implementation
//!
//! Fork implementation:
//! - pt_regs is stored at the TOP of kernel stack (not heap allocated)
//! - child's thread.sp points to pt_regs
//! - child's thread.ra points to ret_from_fork

use crate::process::task::{Task, SchedPolicy, Pid};
use crate::fs::FdTable;
use crate::process::pid::alloc_pid;
use crate::arch::riscv64::pt_regs::PtRegs;

// ============================================================================
// Clone flags
// ============================================================================

/// Share address space (threads)
pub const CLONE_VM: u64 = 0x00000100;
/// Share filesystem info
pub const CLONE_FS: u64 = 0x00000200;
/// Share file descriptor table
pub const CLONE_FILES: u64 = 0x00000400;
/// Share signal handlers
pub const CLONE_SIGHAND: u64 = 0x00000800;
/// Set TLS
pub const CLONE_SETTLS: u64 = 0x00080000;
/// Set child TID in parent
pub const CLONE_PARENT_SETTID: u64 = 0x00100000;
/// Clear TID on child exit
pub const CLONE_CHILD_CLEARTID: u64 = 0x00200000;
/// Set TID in child
pub const CLONE_CHILD_SETTID: u64 = 0x01000000;
/// Same thread group
pub const CLONE_THREAD: u64 = 0x00010000;
/// vfork semantics
pub const CLONE_VFORK: u64 = 0x00004000;

/// Clone arguments structure
pub struct CloneArgs {
    /// Clone flags
    pub flags: u64,
    /// New stack pointer (0 means use parent stack)
    pub stack: u64,
    /// TID pointer in parent (CLONE_PARENT_SETTID)
    pub parent_tid: *mut i32,
    /// TID pointer in child (CLONE_CHILD_SETTID, CLONE_CHILD_CLEARTID)
    pub child_tid: *mut i32,
    /// TLS pointer (CLONE_SETTLS)
    pub tls: u64,
}

/// Create child process
///
/// # Returns
/// - Some(pid): PID of child process (returned in parent)
/// - None: Creation failed
pub fn do_fork() -> Option<Pid> {
    do_clone(CloneArgs {
        flags: 0,
        stack: 0,
        parent_tid: core::ptr::null_mut(),
        child_tid: core::ptr::null_mut(),
        tls: 0,
    })
}

/// copy_thread - thread context copy
///
/// Sets up child's context so it will return to user mode via ret_from_fork.
///
/// Key points:
/// 1. pt_regs is stored at kernel stack top (not heap allocated)
/// 2. child->thread.sp = pt_regs (stack pointer points to saved registers)
/// 3. child->thread.ra = ret_from_fork (return address for context switch)
/// 4. child pt_regs.a0 = 0 (fork returns 0 in child)
///
/// # Arguments
/// - task: Child task to set up
/// - args: Clone arguments
/// - parent_regs: Parent's current pt_regs
///
/// # Returns
/// - Some(()) on success
/// - None on failure
fn copy_thread(task: &mut Task, args: &CloneArgs, parent_regs: &PtRegs) -> Option<()> {
    // Get child's pt_regs at kernel stack top
    let child_regs = task.pt_regs();
    if child_regs.is_null() {
        return None;
    }

    // SAFETY: child_regs was returned by task.pt_regs() which points to allocated space
    // at the top of the child's kernel stack. parent_regs is the current task's valid
    // trap frame. We write a complete PtRegs struct to the child's stack.
    unsafe {
        // Copy parent's pt_regs to child
        core::ptr::write(child_regs, *parent_regs);

        // Get mutable reference to child's pt_regs
        let regs = &mut *child_regs;

        // ===== Clear callee-saved registers =====
        // CRITICAL: Clear callee-saved registers (s0-s11) for child task
        {
            let thread = task.thread_mut();
            thread.s.fill(0);
        }

        // ===== pt_regs is COPIED from parent =====
        // Copy parent's pt_regs (including s0-s11) to child.
        // The child inherits parent's callee-saved register values.
        // This is CORRECT because:
        // 1. s0-s11 are callee-saved, so they're preserved across function calls
        // 2. The fork wrapper's caller expects s0-s11 to be unchanged
        // 3. Only a0 (return value) is different in child (a0=0)
        // DO NOT clear pt_regs.s0-s11 - child should inherit parent's values!

        // Child process return value is 0
        regs.a0 = 0;
        regs.orig_a0 = 0;

        // Clear SPP bit to ensure child returns to user mode
        // SPP = bit 8 in sstatus
        const SR_SPP: u64 = 1 << 8;
        regs.status &= !SR_SPP;

        // Use new stack if specified (CLONE_VM | CLONE_SETTLS uses this)
        if args.stack != 0 {
            regs.sp = args.stack;
        }

        // Set TLS if requested
        if args.flags & CLONE_SETTLS != 0 {
            regs.tp = args.tls;
        }

        // Set up thread struct for context switch
        extern "C" {
            fn ret_from_fork();
        }

        let thread = task.thread_mut();
        thread.ra = ret_from_fork as u64;  // Return address = ret_from_fork
        thread.sp = child_regs as u64;     // Stack pointer = pt_regs address

        // Callee-saved registers (s0-s11) are cleared to 0 above.
        // This is correct because:
        // 1. Child's user-space callee-saved registers are in pt_regs (inherited from parent)
        // 2. Child's kernel-space callee-saved registers start at 0 (clean slate)
        // 3. When child is scheduled in, __switch_to restores zeros to s0-s11
        // 4. When child returns to user mode, s0-s11 are restored from pt_regs
    }

    Some(())
}

/// Create child process/thread
///
/// # Arguments
/// - args: Clone arguments
///
/// # Returns
/// - Some(pid): PID of child process/thread (returned in parent)
/// - None: Creation failed
pub fn do_clone(args: CloneArgs) -> Option<Pid> {
    use crate::arch::riscv64::trap::current_pt_regs;

    // SAFETY: current is the parent task's raw pointer, valid throughout clone.
    // task_ptr is freshly allocated by alloc_task_slot(). All modifications to
    // child task fields are done before it is enqueued, so no concurrent access.
    unsafe {
        // Get current task (parent process)
        let current = crate::sched::current()?;
        let current_ptr = current as *mut Task;

        // Get parent's current PtRegs (saved during trap handling)
        let parent_pt_regs = current_pt_regs();
        if parent_pt_regs.is_null() {
            return None;
        }

        // Allocate task slot from scheduler
        // Note: alloc_task_slot calls new_task_at which already allocates kernel stack
        let task_ptr = crate::sched::alloc_task_slot()?;
        let pid = (*task_ptr).pid();

        crate::pr_info!("fork: parent={}, child={}, flags={:#x}",
            (*current).pid(), pid, args.flags);

        // Add child to parent's children list
        (*current_ptr).add_child(task_ptr);

        // === copy_thread: Set up child's context ===
        let parent_regs = &*parent_pt_regs;
        if copy_thread(&mut *task_ptr, &args, parent_regs).is_none() {
            (*task_ptr).free_kernel_stack();
            crate::sched::free_task_slot(task_ptr);
            return None;
        }

        // Copy signal mask
        (*task_ptr).sigmask = (*current_ptr).sigmask;

        // Inherit process group and session from parent
        (*task_ptr).set_pgid((*current_ptr).pgid());
        (*task_ptr).set_sid((*current_ptr).sid());

        // === copy_files: Copy/share file descriptor table ===
        if args.flags & CLONE_FILES != 0 {
            // CLONE_FILES: Share file descriptor table (threads)
            // Clone the Arc to share the same FdTable
            if let Some(parent_fdtable) = (*current_ptr).fdtable_arc() {
                (*task_ptr).set_fdtable(Some(parent_fdtable));
            } else {
                // Parent has no fdtable, create new one
                let child_fdtable = alloc::sync::Arc::new(FdTable::new());
                (*task_ptr).set_fdtable(Some(child_fdtable));
                if let Some(fdtable) = (*task_ptr).try_fdtable() {
                    crate::init::init_std_fds_for_task(fdtable);
                }
            }
        } else {
            // Copy file descriptor table (fork semantics)
            let child_fdtable = alloc::sync::Arc::new(FdTable::new());

            // Copy all file descriptors from parent to child
            if let Some(parent_fdtable) = (*current_ptr).try_fdtable() {
                for fd in 0..1024 {
                    if let Some(file) = parent_fdtable.get_file(fd) {
                        // Copy the Arc to the child's fdtable
                        let _ = child_fdtable.install_fd(fd, file);
                    }
                }
            }

            (*task_ptr).set_fdtable(Some(child_fdtable));
        }

        // === copy_mm: Copy/share address space ===
        if args.flags & CLONE_VM != 0 {
            // CLONE_VM: Share address space (threads)
            // Clone the Arc to share the same AddressSpace
            if let Some(parent_as) = (*current_ptr).address_space_arc() {
                // Increment mm_users reference count
                parent_as.mm_users_inc();
                (*task_ptr).set_address_space(Some(parent_as));
            } else {
                crate::sched::free_task_slot(task_ptr);
                return None;
            }
        } else {
            // Copy address space (COW)
            let parent_addr_space = (*current_ptr).address_space();
            if let Some(parent_as) = parent_addr_space {
                match parent_as.fork() {
                    Ok(child_as) => {
                        (*task_ptr).set_address_space(Some(alloc::sync::Arc::new(child_as)));
                    }
                    Err(_e) => {
                        crate::sched::free_task_slot(task_ptr);
                        return None;
                    }
                }
            } else {
                crate::sched::free_task_slot(task_ptr);
                return None;
            }
        }

        // Copy brk value
        let parent_brk = (*current_ptr).get_brk();
        (*task_ptr).set_brk(parent_brk);

        // === CLONE_FS: Share filesystem info ===
        if args.flags & CLONE_FS != 0 {
            // CLONE_FS: Share filesystem info (cwd, root, umask)
            // Clone the Arc to share the same FsStruct
            if let Some(parent_fs) = (*current_ptr).fs_arc() {
                (*task_ptr).set_fs(Some(parent_fs));
            }
        } else {
            // Copy filesystem info (child gets its own FsStruct with same cwd)
            let parent_cwd = (*current_ptr).get_cwd();
            (*task_ptr).set_cwd(&parent_cwd);
        }

        // === CLONE_PARENT_SETTID: Set child TID in parent ===
        if args.flags & CLONE_PARENT_SETTID != 0 && !args.parent_tid.is_null() {
            // Verify pointer is writable
            if crate::arch::riscv64::uaccess::access_ok(args.parent_tid as usize, 4) {
                *args.parent_tid = pid as i32;
            }
        }

        // === CLONE_CHILD_SETTID: Set TID in child ===
        if args.flags & CLONE_CHILD_SETTID != 0 && !args.child_tid.is_null() {
            // Set clear_child_tid (will be cleared when child exits)
            (*task_ptr).set_clear_child_tid(args.child_tid);

            // Set TID in child's memory
            // This will be written when child runs
            // Simplified implementation: write directly here
            if crate::arch::riscv64::uaccess::access_ok(args.child_tid as usize, 4) {
                *args.child_tid = pid as i32;
            }
        }

        // === CLONE_CHILD_CLEARTID: Clear TID when child exits ===
        if args.flags & CLONE_CHILD_CLEARTID != 0 && !args.child_tid.is_null() {
            (*task_ptr).set_clear_child_tid(args.child_tid);
        }

        // === CLONE_THREAD: Same thread group ===
        if args.flags & CLONE_THREAD != 0 {
            // CLONE_THREAD: Same thread group
            // Child shares the same tgid (thread group ID) as parent
            let parent_tgid = (*current_ptr).tgid();
            (*task_ptr).set_tgid(parent_tgid);
        }
        // else: tgid = pid (already set in Task::new)

        // === CLONE_SIGHAND: Share signal handlers ===
        if args.flags & CLONE_SIGHAND != 0 {
            // CLONE_SIGHAND: Share signal handlers (threads)
            // Clone the Arc to share the same SignalStruct
            if let Some(parent_signal) = (*current_ptr).signal_arc() {
                (*task_ptr).set_signal(Some(parent_signal));
            }
        } else {
            // For normal fork: copy parent's signal handlers
            // Clone the Arc - child gets a copy of the signal struct
            if let Some(parent_signal) = (*current_ptr).signal.as_ref() {
                // Clone the inner SignalStruct and wrap in new Arc
                let child_signal = alloc::sync::Arc::new((**parent_signal).clone());
                (*task_ptr).signal = Some(child_signal);
            }
            // Also copy signal mask
            (*task_ptr).sigmask = (*current_ptr).sigmask;
        }

        // Copy SEM_UNDO table (child inherits parent's adjustments)
        {
            let parent_undo = (*current_ptr).sem_undo.lock();
            if !parent_undo.is_empty() {
                (*task_ptr).sem_undo.lock().extend_from_slice(&parent_undo);
            }
        }

        // Copy credentials from parent
        *(*task_ptr).cred_mut() = (*current_ptr).cred().clone();

        // Add new task to run queue
        crate::sched::enqueue_task(&mut *task_ptr);

        Some(pid)
    }
}
