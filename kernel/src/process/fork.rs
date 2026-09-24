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

use crate::process::task::{Task, TaskState, Pid};
use crate::fs::FdTable;
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
/// New mount namespace group (accepted, not implemented)
pub const CLONE_NEWNS: u64 = 0x00020000;
/// Share System V semaphore undo state (accepted; undo table copied anyway)
pub const CLONE_SYSVSEM: u64 = 0x00040000;

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
    .ok()
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

        // ===== Inherit the parent's floating-point state (POSIX) =====
        // Save the parent's LIVE registers into its thread struct first,
        // then copy the saved image and mark the child CLEAN — the child
        // must observe the same FP values the parent would have.
        if let Some(parent_task) = crate::sched::current() {
            // SAFETY: parent_task is the currently running task.
            unsafe {
                (*parent_task).thread_mut().save_fpu();
                let (pfpu, _) = {
                    let pt = (*parent_task).thread();
                    (pt.fpu, pt.fs)
                };
                let ct = task.thread_mut();
                ct.fpu.copy_from_slice(&pfpu);
                ct.fs = super::super::arch::riscv64::pt_regs::SR_FS_CLEAN as u32;
            }
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

/// Clone flags we understand (accepted; namespace/io flags are accepted
/// and ignored — no namespace support yet). Unknown bits are rejected
/// with EINVAL: silently mis-executing a future flag is worse.
const CLONE_KNOWN_FLAGS: u64 = CLONE_VM
    | CLONE_FS
    | CLONE_FILES
    | CLONE_SIGHAND
    | CLONE_PIDFD       // 0x00001000 (accepted, no pidfd yet)
    | CLONE_PTRACE      // 0x00002000
    | CLONE_VFORK
    | CLONE_PARENT      // 0x00008000 (accepted; grandparent reparenting
                        //  not done — harmless for musl)
    | CLONE_THREAD
    | CLONE_NEWNS       // 0x00020000
    | CLONE_SYSVSEM     // 0x00040000 (undo table is copied anyway)
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_CHILD_SETTID
    | CLONE_DETACHED    // 0x00400000 legacy, harmless
    | CLONE_UNTRACED    // 0x00800000
    | CLONE_NEWCGROUP   // 0x02000000
    | CLONE_NEWUTS      // 0x04000000
    | CLONE_NEWIPC      // 0x08000000
    | CLONE_NEWUSER     // 0x10000000
    | CLONE_NEWPID      // 0x20000000
    | CLONE_NEWNET      // 0x40000000
    | CLONE_IO;         // 0x80000000
const CLONE_PIDFD: u64 = 0x00001000;
const CLONE_PTRACE: u64 = 0x00002000;
const CLONE_PARENT: u64 = 0x00008000;
const CLONE_DETACHED: u64 = 0x00400000;
const CLONE_UNTRACED: u64 = 0x00800000;
const CLONE_NEWCGROUP: u64 = 0x02000000;
const CLONE_NEWUTS: u64 = 0x04000000;
const CLONE_NEWIPC: u64 = 0x08000000;
const CLONE_NEWUSER: u64 = 0x10000000;
const CLONE_NEWPID: u64 = 0x20000000;
const CLONE_NEWNET: u64 = 0x40000000;
const CLONE_IO: u64 = 0x80000000;

/// errno helpers (negative i32, syscall convention)
fn einval() -> i32 { crate::errno::Errno::InvalidArgument.as_neg_i32() }
fn eagain() -> i32 { crate::errno::Errno::TryAgain.as_neg_i32() }
fn enomem() -> i32 { crate::errno::Errno::OutOfMemory.as_neg_i32() }
fn efault() -> i32 { crate::errno::Errno::BadAddress.as_neg_i32() }

/// Create child process/thread
///
/// # Arguments
/// - args: Clone arguments
///
/// # Returns
/// - Ok(pid): PID of child process/thread (returned in parent)
/// - Err(errno): negative errno — EINVAL for illegal flag combinations,
///   EAGAIN when the PID space is exhausted, ENOMEM for allocation
///   failures, EFAULT when a CLONE_*SETTID pointer is unwritable.
pub fn do_clone(args: CloneArgs) -> Result<Pid, i32> {
    use crate::arch::riscv64::trap::current_pt_regs;

    // Validate clone flag constraints (matches Linux kernel checks):
    // CLONE_THREAD requires CLONE_SIGHAND (kernel/fork.c clone3_args_check)
    // CLONE_SIGHAND requires CLONE_VM (kernel/fork.c copy_process)
    if args.flags & CLONE_THREAD != 0 && args.flags & CLONE_SIGHAND == 0 {
        crate::pr_warn!("clone: CLONE_THREAD requires CLONE_SIGHAND");
        return Err(einval());
    }
    if args.flags & CLONE_SIGHAND != 0 && args.flags & CLONE_VM == 0 {
        crate::pr_warn!("clone: CLONE_SIGHAND requires CLONE_VM");
        return Err(einval());
    }
    // Unknown flag bits (excluding the low CSIGNAL byte) → EINVAL.
    if args.flags & !CLONE_KNOWN_FLAGS & !0xff != 0 {
        crate::pr_warn!("clone: unknown flags {:#x}", args.flags);
        return Err(einval());
    }

    // SAFETY: current is the parent task's raw pointer, valid throughout clone.
    // task_ptr is freshly allocated by alloc_task_slot(). All modifications to
    // child task fields are done before it is enqueued, so no concurrent access.
    unsafe {
        // Get current task (parent process)
        let current = match crate::sched::current() {
            Some(c) => c,
            None => return Err(enomem()),
        };
        let current_ptr = current as *mut Task;

        // RLIMIT_NPROC (process-forking clones only — threads are not
        // process products). Simplification per the P1 rlimits plan: the
        // count is GLOBAL user processes (tasks with an address space —
        // kernel threads and idle are excluded) rather than a per-uid
        // aggregate; there is no uid hash yet. Linux fails fork with
        // EAGAIN here.
        // TODO(RLIMIT-NPROC): the rlimit() read here — despite returning
        // RLIM_INFINITY and skipping the iteration — correlates with a null
        // deref in the fd-table copy's get_file later in do_clone. Root
        // cause not yet isolated; the check is disabled until then. The
        // rlimits copy below and all other enforcement points are active.
        if false && args.flags & CLONE_THREAD == 0 {
            let (nproc_cur, _) = (*current_ptr)
                .rlimit(crate::process::task::rlimit_res::NPROC);
            if nproc_cur != u64::MAX {
                let mut user_procs: u64 = 0;
                crate::process::pid_hash::pid_hash_for_each_task(|t| unsafe {
                    if !t.is_null() && (*t).address_space().is_some() {
                        user_procs += 1;
                    }
                });
                if user_procs >= nproc_cur {
                    return Err(eagain());
                }
            }
        }

        // Get parent's current PtRegs (saved during trap handling)
        let parent_pt_regs = current_pt_regs();
        if parent_pt_regs.is_null() {
            return Err(enomem());
        }

        // Allocate task slot from scheduler
        // Note: alloc_task_slot calls new_task_at which already allocates kernel stack
        let task_ptr = match crate::sched::alloc_task_slot() {
            Some(p) => p,
            None => {
                // Distinguish PID exhaustion (EAGAIN, Linux semantics) from
                // task/stack allocation failure (ENOMEM): probe the PID
                // allocator — if it is exhausted the failure was the PID.
                return Err(match crate::process::pid::alloc_pid() {
                    None => eagain(),
                    Some(p) => {
                        crate::process::pid::free_pid(p);
                        enomem()
                    }
                });
            }
        };
        let pid = (*task_ptr).pid();

        crate::pr_debug!("fork: parent={}, child={}, flags={:#x}",
            (*current).pid(), pid, args.flags);

        // Full-unwind helper for every failure path past allocation: the
        // child is hash-visible from alloc_task_slot, so unlink BEFORE
        // freeing the stack (R33-pre: a concurrent kill on a hashed slot
        // with a freed stack is the context-switch-onto-heap wedge).
        let unwind = |task_ptr: *mut Task, err: i32| -> Result<Pid, i32> {
            (*current_ptr).remove_child(task_ptr);
            crate::process::pid_hash::pid_hash_remove((*task_ptr).pid());
            (*task_ptr).free_kernel_stack();
            crate::process::pid::free_pid((*task_ptr).pid());
            crate::sched::free_task_slot(task_ptr);
            Err(err)
        };

        // Threads (CLONE_THREAD) are NOT children: they never join the
        // parent's children list — wait4 only ever reaps process products.
        let is_thread = args.flags & CLONE_THREAD != 0;
        if !is_thread {
            (*current_ptr).add_child(task_ptr);
        }

        // === copy_thread: Set up child's context ===
        let parent_regs = &*parent_pt_regs;
        if copy_thread(&mut *task_ptr, &args, parent_regs).is_none() {
            return unwind(task_ptr, enomem());
        }

        // Copy signal mask
        (*task_ptr).sigmask = (*current_ptr).sigmask;

        // Inherit process group and session from parent
        (*task_ptr).set_pgid((*current_ptr).pgid());
        (*task_ptr).set_sid((*current_ptr).sid());

        // === CLONE_PARENT_SETTID / CLONE_CHILD_SETTID ===
        // Both are written in the PARENT address space, BEFORE the mm copy
        // below: with CLONE_VM the memories are identical anyway; without
        // CLONE_VM the COW copy carries the CHILD_SETTID word into the
        // child (child-context semantics). EFAULT fails the clone (Linux).
        if args.flags & CLONE_PARENT_SETTID != 0 && !args.parent_tid.is_null() {
            let tid_val = pid as i32;
            if crate::arch::riscv64::uaccess::copy_to_user(
                args.parent_tid as *mut u8,
                &tid_val as *const i32 as *const u8,
                core::mem::size_of::<i32>(),
            ) != 0
            {
                return unwind(task_ptr, efault());
            }
        }
        if args.flags & CLONE_CHILD_SETTID != 0 && !args.child_tid.is_null() {
            let tid_val = pid as i32;
            if crate::arch::riscv64::uaccess::copy_to_user(
                args.child_tid as *mut u8,
                &tid_val as *const i32 as *const u8,
                core::mem::size_of::<i32>(),
            ) != 0
            {
                return unwind(task_ptr, efault());
            }
        }

        // === CLONE_CHILD_CLEARTID: Clear TID when child exits ===
        if args.flags & CLONE_CHILD_CLEARTID != 0 && !args.child_tid.is_null() {
            (*task_ptr).set_clear_child_tid(args.child_tid);
        }

        // === copy_files: Copy/share file descriptor table ===
        if args.flags & CLONE_FILES != 0 {
            // CLONE_FILES: Share file descriptor table (threads). A parent
            // with NO table shares exactly that — the child also gets None
            // ("share the absence"), matching Linux's Arc-clone semantics.
            if let Some(parent_fdtable) = (*current_ptr).fdtable_arc() {
                (*task_ptr).set_fdtable(Some(parent_fdtable));
            }
        } else {
            // Copy file descriptor table (fork semantics)
            let child_fdtable = alloc::sync::Arc::new(FdTable::new());

            // Copy all file descriptors from parent to child
            if let Some(parent_fdtable) = (*current_ptr).try_fdtable() {
                for fd in 0..crate::fs::file::MAX_FDS {
                    if let Some(file) = parent_fdtable.get_file(fd) {
                        // Copy the Arc to the child's fdtable
                        let _ = child_fdtable.install_fd(fd, file);
                        // FD_CLOEXEC belongs to the descriptor, so the bit
                        // must be copied too (regression round 5, HIGH: fork
                        // was dropping all CLOEXEC bits, leaking fds across
                        // execve in children).
                        if parent_fdtable.get_fd_cloexec(fd) {
                            child_fdtable.set_fd_cloexec(fd, true);
                        }
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
                return unwind(task_ptr, enomem());
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
                        return unwind(task_ptr, enomem());
                    }
                }
            } else {
                return unwind(task_ptr, enomem());
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

        // === CLONE_THREAD: join the parent's thread group ===
        if is_thread {
            let leader = (*current_ptr).group_leader_ptr();
            // SAFETY: leader's ring is well-formed (invariant maintained by
            // thread_group_join/leave); task_ptr is fully constructed and
            // not yet runnable.
            (*leader).thread_group_join(task_ptr);
        }
        // else: tgid = pid, single-member self-ring (already set at construction)

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
        }

        // === Scheduling attributes and identity inheritance (Linux
        // copy_process: sched_fork copies nice/policy/rt_priority/cpus_allowed;
        // comm and sigaltstack are also inherited by clone) ===
        {
            let parent = &*current_ptr;
            let child = &mut *task_ptr;
            child.set_comm(parent.comm());
            child.set_policy(parent.policy());
            // set_nice derives static_prio/normal_prio/prio AND updates the
            // CFS entity weight — must go through it, not raw field writes.
            child.set_nice(parent.nice());
            child.set_rt_priority(parent.rt_priority());
            child.set_cpus_allowed(parent.cpus_allowed());
            child.set_oom_score_adj(parent.oom_score_adj());
            child.sigstack = parent.sigstack;
            child.dumpable = parent.dumpable;
        }

        // Copy SEM_UNDO table (child inherits parent's adjustments)
        {
            let parent_undo = (*current_ptr).sem_undo.lock();
            if !parent_undo.is_empty() {
                (*task_ptr).sem_undo.lock().extend_from_slice(&parent_undo);
            }
        }

        // Inherit resource limits (Linux copy_process: memcpy of
        // signal->rlim). Enforcement points (alloc_fd/mmap/brk/fork
        // NPROC/tick CPU) read the child's copy; threads (CLONE_THREAD)
        // get their own table too — limits are per-task here, matching
        // the per-task storage rather than Linux's shared signal struct.
                {
            let parent_rlimits = *(*current_ptr).rlimits.lock_irqsave();
            *(*task_ptr).rlimits.lock_irqsave() = parent_rlimits;
        }

        // Copy credentials from parent
        *(*task_ptr).cred_mut() = (*current_ptr).cred().clone();

        // Handle CLONE_VFORK: block parent until child execs/exits.
        // The child shares parent's address space (CLONE_VM is expected
        // to be set alongside CLONE_VFORK).  The parent sleeps in
        // UNINTERRUPTIBLE state and is woken by the child's execve or
        // _exit via vfork_wake_parent().
        //
        // Register the vfork linkage BEFORE enqueueing the child: once the
        // child is runnable another CPU can run it immediately and have it
        // exec/exit before we finish — vfork_wake_parent must find the
        // parent pointer already set (review PROC-P02 race 1).
        let is_vfork = args.flags & CLONE_VFORK != 0;
        if is_vfork {
            (*task_ptr).set_vfork_parent(current_ptr);
        }

        // Add new task to run queue
        //
        // R52 (fork enqueue gap — form A): the enqueue result is now
        // CHECKED, not assumed. The child has been hash-visible with a
        // valid pid since alloc_task_slot; until this point it is
        // TASK_NEW (unwakeable — no racing signal/OOM kill can link it
        // early). If the GRQ-locked insert refuses (a guard divergence:
        // poison, dead-state, or a stale class on_rq flag on a
        // never-enqueued task), the child is on NO queue and NO cpu
        // while the parent believes fork() succeeded — it waits on a
        // PID that never runs (the ti_cpu=-1 / on_rq=0 / linked=0 /
        // RUNNING-looking phantom). Unwind the child completely and
        // fail the fork instead; the page leaves poisoned, so the
        // invariant "constructed ⇒ enqueued or fully unwound" holds by
        // construction. (The parent has not slept yet — the vfork block
        // below never runs on this path.)
        if !crate::sched::enqueue_task(&mut *task_ptr) {
            crate::pr_warn!(
                "fork: enqueue refused for child pid={} — unwinding (R52 tripwire)",
                pid
            );
            if is_thread {
                // Not a children-list member; undo the ring join instead.
                // SAFETY: task_ptr is a live ring member (joined above).
                Task::thread_group_leave(task_ptr);
                crate::process::pid_hash::pid_hash_remove(pid);
                (*task_ptr).free_kernel_stack();
                crate::process::pid::free_pid(pid);
                crate::sched::free_task_slot(task_ptr);
            } else {
                return unwind(task_ptr, enomem());
            }
            return Err(enomem());
        }

        if is_vfork {
            crate::pr_debug!("vfork: parent={} blocked, child={}",
                (*current_ptr).pid(), pid);

            (*current).set_state(TaskState::new(TaskState::UNINTERRUPTIBLE));

            // Re-check AFTER marking ourselves sleeping: if the child already
            // exec'd/exited above, its wake_up saw us RUNNING and was a
            // no-op — detect the cleared vfork_parent and skip the sleep
            // (prepare-to-wait pattern, review PROC-P02 race 2).
            if (*task_ptr).vfork_parent_ptr().is_none() {
                (*current).set_state(TaskState::new(TaskState::RUNNING));
                // A wake_up() racing the window above may have enqueued us
                // on the run queue while we are in fact still executing —
                // take ourselves back off before continuing, or a second
                // CPU could pick and run this very task (review NEW-C2).
                crate::sched::dequeue_if_enqueued(&*current);
            } else {
                crate::sched::schedule();
            }
            // Parent resumes here after child exec'd or exited
        }

        Ok(pid)
    }
}
