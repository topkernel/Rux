//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Process creation (fork/clone) implementation

use crate::process::task::{Task, SchedPolicy, Pid};
use crate::fs::FdTable;
use crate::process::pid::alloc_pid;

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
    use crate::arch::riscv64::pt_regs::PtRegs;

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
        let task_ptr = crate::sched::alloc_task_slot()?;
        let pid = (*task_ptr).pid();

        // Copy parent state to child
        (*task_ptr).set_parent(current_ptr);

        // === copy_thread: Copy PtRegs ===
        // Child process return value is 0 (a0 = 0)
        //
        // PtRegs layout:
        //   - Starts directly from epc, no extra 16-byte header needed
        let child_pt_regs: alloc::boxed::Box<PtRegs> = {
            let parent = &*parent_pt_regs;

            // Clear SPP bit to ensure child returns to user mode
            // SPP = bit 8, clearing it makes child return to U-mode
            const SR_SPP: u64 = 1 << 8;
            let child_status = parent.status & !SR_SPP;

            alloc::boxed::Box::new(PtRegs {
                epc: parent.epc,         // epc already +4 in trap handler, no need to add
                ra: parent.ra,
                sp: if args.stack != 0 { args.stack } else { parent.sp },  // New stack or parent stack
                gp: parent.gp,           // Global pointer
                tp: if args.flags & CLONE_SETTLS != 0 { args.tls } else { parent.tp },  // TLS
                t0: parent.t0,
                t1: parent.t1,
                t2: parent.t2,
                s0: parent.s0,
                s1: parent.s1,
                a0: 0,                   // Child process return value is 0
                a1: parent.a1,
                a2: parent.a2,
                a3: parent.a3,
                a4: parent.a4,
                a5: parent.a5,
                a6: parent.a6,
                a7: parent.a7,
                s2: parent.s2,
                s3: parent.s3,
                s4: parent.s4,
                s5: parent.s5,
                s6: parent.s6,
                s7: parent.s7,
                s8: parent.s8,
                s9: parent.s9,
                s10: parent.s10,
                s11: parent.s11,
                t3: parent.t3,
                t4: parent.t4,
                t5: parent.t5,
                t6: parent.t6,
                status: child_status,    // sstatus with SPP cleared
                badaddr: parent.badaddr, // stval
                cause: parent.cause,     // scause
                orig_a0: 0,              // Child orig_a0 = 0
            })
        };

        // Allocate memory for child's PtRegs
        use alloc::alloc::{alloc, Layout};
        let pt_regs_size = core::mem::size_of::<PtRegs>();
        let layout = Layout::from_size_align(pt_regs_size, 16).expect("Invalid layout");

        let mem_ptr = alloc(layout);
        if mem_ptr.is_null() {
            crate::sched::free_task_slot(task_ptr);
            return None;
        }

        // Copy PtRegs to allocated memory
        let pt_regs_ptr = mem_ptr as *mut PtRegs;
        core::ptr::write(pt_regs_ptr, *child_pt_regs);

        // Set child's fork info
        (*task_ptr).set_fork_child(pt_regs_ptr);

        // Allocate kernel stack for child
        // Child needs kernel stack when it enters kernel via trap after returning to user
        // alloc_kernel_stack will automatically set ti_kernel_sp
        if (*task_ptr).alloc_kernel_stack().is_none() {
            // Allocation failed, cleanup and return
            alloc::alloc::dealloc(mem_ptr, layout);
            crate::sched::free_task_slot(task_ptr);
            return None;
        }

        // Copy CPU context (callee-saved registers)
        let parent_ctx = (*current_ptr).context();
        let child_ctx = (*task_ptr).context_mut();
        *child_ctx = parent_ctx.clone();

        // Set child's entry point to ret_from_fork
        // Key: Set ra instead of pc!
        // After cpu_switch_to assembly restores ra and executes ret, it jumps to ra's address
        extern "C" {
            fn ret_from_fork();
        }
        child_ctx.ra = ret_from_fork as u64;  // ra = ret_from_fork
        child_ctx.sp = pt_regs_ptr as u64;    // sp points to child's PtRegs
        // Note: fork return value 0 is set in PtRegs.a0, restored by ret_from_fork

        // Copy signal mask
        (*task_ptr).sigmask = (*current_ptr).sigmask;

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
            // Copy file descriptor table
            let child_fdtable = alloc::sync::Arc::new(FdTable::new());
            (*task_ptr).set_fdtable(Some(child_fdtable));

            if let Some(fdtable) = (*task_ptr).try_fdtable() {
                crate::init::init_std_fds_for_task(fdtable);
            }
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
                    Err(_) => {
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
            // Note: If parent has no signal struct, child keeps its own (created in Task::new)
        }
        // else: child has its own signal handlers (already created in Task::new)

        // Add new task to run queue
        crate::sched::enqueue_task(&mut *task_ptr);

        Some(pid)
    }
}
