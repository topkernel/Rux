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
    /// Signal the parent gets at child exit (low CSIGNAL byte of the
    /// legacy clone flags, or clone3's own field). 0 = none.
    pub exit_signal: u8,
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
        exit_signal: 17, // SIGCHLD
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
    // CLONE_FS|CLONE_NEWNS is mutually exclusive (kernel/fork.c copy_fs):
    // the child cannot both SHARE the parent's fs info and get a private
    // copy of its mount namespace (LTP clone302 "fs-newns").
    if args.flags & CLONE_NEWNS != 0 && args.flags & CLONE_FS != 0 {
        crate::pr_warn!("clone: CLONE_NEWNS is incompatible with CLONE_FS");
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
    #[cfg(feature = "dfx-futex-trace")]
    {
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        let pid = crate::sched::current().map(|c| unsafe { (*c).pid() as u64 }).unwrap_or(0);
        taskdump_raw_line(b"FTX CLONE pid=");
        taskdump_dec(pid);
        taskdump_raw_line(b" flags=");
        let f = args.flags;
        let mut sh: i32 = 64;
        while sh > 0 {
            sh -= 4;
            let nb = ((f >> sh) & 0xF) as u8;
            taskdump_raw_line(&[(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 })]);
        }
        taskdump_raw_line(b" stack=");
        taskdump_dec(args.stack);
        taskdump_raw_line(b"\n");
    }
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

        // Get parent's current PtRegs.
        //
        // SMP fix (same family as the exec trap-frame fix): derive the frame
        // from THE TASK, never from the per-CPU current_pt_regs() slot. That
        // slot is indexed by tp->ti_cpu and is only valid for the exact trap
        // that last stored it; a fork racing a nested interrupt (or a task
        // whose ti_cpu was steered) read ANOTHER task's / a stale frame —
        // the child then resumed userspace with foreign registers (observed:
        // pipeline children issuing wait4/sigsuspend they never called, dash
        // passing a garbage sigsuspend mask pointer 0x26, and the resulting
        // permanent sigsuspend wedge). task.pt_regs() is the canonical
        // outermost user frame on THIS task's kernel stack; for a
        // syscall-context fork it is exactly the clone syscall's frame.
        let parent_pt_regs = (*current_ptr).pt_regs();
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
            if args.flags & CLONE_PARENT != 0 {
                // CLONE_PARENT: the child's parent is the CALLER'S parent
                // (Linux: real_parent of the caller). The child's getppid()
                // equals the caller's getppid(), and the CALLER cannot
                // wait for it — the grandparent reaps it (LTP clone08's
                // CLONE_PARENT case).
                let grandparent = (*current_ptr).parent_ptr().unwrap_or(current_ptr);
                (*grandparent).add_child(task_ptr);
            } else {
                (*current_ptr).add_child(task_ptr);
            }
        }

        // === copy_thread: Set up child's context ===
        let parent_regs = &*parent_pt_regs;
        if copy_thread(&mut *task_ptr, &args, parent_regs).is_none() {
            return unwind(task_ptr, enomem());
        }

        // Copy signal mask
        (*task_ptr).sigmask = (*current_ptr).sigmask;

        // Exit signal for the parent (clone CSIGNAL byte / clone3 field).
        // Threads never notify a parent this way, but record it anyway —
        // the notify path reads the field uniformly (LTP clone301:
        // clone3 exit_signal=SIGUSR2 must SIGUSR2 the parent, not SIGCHLD).
        (*task_ptr).set_exit_signal(args.exit_signal);

        // Inherit the executable path: a fork WITHOUT exec (daemon style)
        // keeps reporting the parent's program in /proc/[pid]/comm,
        // "Name:" of status and /proc/[pid]/exe — like Linux, where the
        // child's mm->exe_file ref is duplicated at fork.
        (*task_ptr).set_exe_path((*current_ptr).get_exe_path());

        // Inherit process group and session from parent
        (*task_ptr).set_pgid((*current_ptr).pgid());
        (*task_ptr).set_sid((*current_ptr).sid());

        // cgroup v2 (U1b): the child joins the parent's cgroup. The pids
        // controller limit is checked here (every ancestor with a limit
        // must have headroom — Linux fails fork with EAGAIN).
        if let Err(e) = crate::sched::cgroup::cgroup_on_fork(current_ptr, task_ptr) {
            return unwind(task_ptr, e);
        }

        // === CLONE_PARENT_SETTID / CLONE_CHILD_SETTID ===
        // CLONE_PARENT_SETTID is a PARENT-memory store (Linux put_user in
        // the parent) — done here, before the mm copy.
        // CLONE_CHILD_SETTID must land in the CHILD's memory only. With
        // CLONE_VM the two share one mm, so a parent-side store is the
        // child store; WITHOUT CLONE_VM (fork) the write is deferred to
        // after the COW mm copy below, where it goes through the child's
        // own page tables (see child_settid_pending). Writing it here in
        // the parent — as this code did before — mutated the PARENT's
        // copy of the word: glibc's fork() passes child_tidptr =
        // &THREAD_SELF->tid, so the parent's TCB tid field was overwritten
        // with the CHILD's pid. Every later recursive-mutex owner check in
        // the parent (glibc compares __owner against that TCB field)
        // mismatched, and any recursive re-lock of a mutex still held
        // across the fork took the contended slow path and slept forever
        // (Xorg's input_mutex around the xkbcomp System() call — the
        // permanent futex_wait(2) wedge).
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
        let mut child_settid_pending = false;
        if args.flags & CLONE_CHILD_SETTID != 0 && !args.child_tid.is_null() {
            if args.flags & CLONE_VM != 0 {
                // Shared address space: the child's memory IS this memory.
                let tid_val = pid as i32;
                if crate::arch::riscv64::uaccess::copy_to_user(
                    args.child_tid as *mut u8,
                    &tid_val as *const i32 as *const u8,
                    core::mem::size_of::<i32>(),
                ) != 0
                {
                    return unwind(task_ptr, efault());
                }
            } else {
                // Separate mm (fork): write after the COW copy, through
                // the child's page tables.
                child_settid_pending = true;
            }
        }

        /// Store the child's TID at `child_tid` in the CHILD's address
        /// space (Linux CLONE_CHILD_SETTID semantics). The child just got
        /// a COW copy of the parent's mm, so a parent-side copy_to_user
        /// would hit the PARENT's mapping. If the target leaf is still
        /// COW-shared, break the COW first (private copy for the child,
        /// bookkeeping charged to the child's mm), then write through the
        /// child's PTE via the linear map.
        ///
        /// # Safety
        /// `child_root` must be the root ppn of the new child's page
        /// tables; the child task has not run yet (no TLB entries).
        unsafe fn write_child_settid(
            child_root: u64,
            child_tid: *mut i32,
            tid: i32,
            child_task: *mut Task,
        ) -> bool {
            use crate::arch::riscv64::mm::memory_layout::{phys_to_virt, PhysAddr, PAGE_SIZE};
            use crate::arch::riscv64::mm::mmu_init::get_page_table_virt;
            use crate::arch::riscv64::mm::pagetable::PageTableEntry;
            use crate::arch::riscv64::mm::cow_flags;
            use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};

            let va = child_tid as u64;
            let vpn2 = ((va >> 30) & 0x1ff) as usize;
            let vpn1 = ((va >> 21) & 0x1ff) as usize;
            let vpn0 = ((va >> 12) & 0x1ff) as usize;
            let root = get_page_table_virt(child_root << 12);
            let pte2 = (*root).get(vpn2);
            if !pte2.is_valid() {
                return false;
            }
            let t1 = get_page_table_virt(pte2.ppn() << 12);
            let pte1 = (*t1).get(vpn1);
            if !pte1.is_valid() {
                return false;
            }
            let t0 = get_page_table_virt(pte1.ppn() << 12);
            let mut pte0 = (*t0).get(vpn0);
            if !pte0.is_valid() {
                return false;
            }

            // Break COW if the leaf is write-protected and COW-marked
            // (same mechanics as handle_cow_fault, but the copy is charged
            // to the CHILD's mm — that function books against
            // sched::current(), which here is the parent).
            if pte0.bits() & PageTableEntry::W == 0
                && pte0.bits() & cow_flags::COW != 0
            {
                let old_ppn = pte0.ppn();
                let old_page = pfn_to_page_mut(old_ppn as usize);
                let new_phys = match crate::arch::riscv64::mm::mm_ops::alloc_user_phys_page() {
                    Some(p) => p,
                    None => return false,
                };
                let new_ppn = (new_phys >> 12) as u64;
                let old_virt = phys_to_virt(PhysAddr((old_ppn << 12) as u64));
                let new_virt = phys_to_virt(PhysAddr(new_phys));
                core::ptr::copy_nonoverlapping(
                    old_virt.bits() as *const u8,
                    new_virt.bits() as *mut u8,
                    PAGE_SIZE as usize,
                );
                // W requires R (SV39 reserved encoding otherwise).
                let flags = pte0.bits()
                    & (PageTableEntry::V
                        | PageTableEntry::R
                        | PageTableEntry::X
                        | PageTableEntry::U
                        | PageTableEntry::G
                        | PageTableEntry::A
                        | PageTableEntry::D)
                    | PageTableEntry::W
                    | PageTableEntry::R;
                (*t0).set(vpn0, PageTableEntry::from_bits((new_ppn << 10) | flags));
                // Release the child's share of the old page.
                if !old_page.is_null() {
                    (*old_page).put_page();
                    (*old_page).dec_mapcount();
                }
                let new_page = pfn_to_page_mut(new_ppn as usize);
                if !new_page.is_null() {
                    (*new_page).set_flag(PageFlag::Anonymous);
                    // SwapBacked + LRU membership: keep the copy reclaimable
                    // via swap-out (vmscan scans LRU_INACTIVE_ANON only).
                    (*new_page).set_flag(PageFlag::SwapBacked);
                    (*new_page).set_index(va as usize / (PAGE_SIZE as usize));
                    (*new_page).inc_mapcount();
                }
                // Charge the copy to the CHILD's mm (not current()).
                if let Some(mm) = (*child_task).address_space() {
                    if !new_page.is_null() {
                        crate::mm::rmap::page_record_mapping(
                            &*new_page,
                            mm as *const _ as usize,
                            va as usize,
                        );
                        crate::mm::lru::page_add_anon_lru(&*new_page);
                    }
                    mm.add_rss(1);
                }
                pte0 = (*t0).get(vpn0);
            }

            if pte0.bits() & PageTableEntry::W == 0 {
                return false;
            }
            let base = phys_to_virt(PhysAddr((pte0.ppn() << 12) as u64));
            core::ptr::write_volatile(
                (base.as_usize() + (va & 0xfff) as usize) as *mut i32,
                tid,
            );
            // The child has never run, so no TLB can hold a stale entry;
            // fence for ordering only.
            core::arch::asm!("fence rw, rw");
            true
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
            // Copy file descriptor table (fork semantics) — single-lock
            // deep copy (see FdTable::fork_from; FD_CLOEXEC bits are part
            // of the snapshot, regression round 5's leak stays fixed).
            let child_fdtable = match (*current_ptr).try_fdtable() {
                Some(parent_fdtable) => {
                    alloc::sync::Arc::new(FdTable::fork_from(&*parent_fdtable))
                }
                None => alloc::sync::Arc::new(FdTable::new()),
            };

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
                        #[cfg(feature = "dfx-futex-trace")]
                        {
                            use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
                            let child_arc = alloc::sync::Arc::new(child_as);
                            let p_root = parent_as.root_ppn();
                            let c_root = child_arc.root_ppn();                            for va in [0x1c8000u64, 0x1c9000, 0x100000] {
                                unsafe {
                                    let p = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(p_root, va);
                                    let c = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(c_root, va);
                                    taskdump_raw_line(b"FTX-COWCHK parent=");
                                    taskdump_dec((*current_ptr).pid() as u64);
                                    taskdump_raw_line(b" child=");
                                    taskdump_dec((*task_ptr).pid() as u64);
                                    taskdump_raw_line(b" va=");
                                    taskdump_dec(va);
                                    taskdump_raw_line(b" p=");
                                    taskdump_dec(p.map(|x| x.0).unwrap_or(0));
                                    taskdump_raw_line(b"/pte:");
                                    taskdump_dec(p.map(|x| x.1).unwrap_or(0));
                                    taskdump_raw_line(b" c=");
                                    taskdump_dec(c.map(|x| x.0).unwrap_or(0));
                                    taskdump_raw_line(b"/pte:");
                                    taskdump_dec(c.map(|x| x.1).unwrap_or(0));
                                    taskdump_raw_line(b" refcnt=");
                                    let rf = p.and_then(|x| {
                                        let pg = crate::mm::page_desc::pfn_to_page(x.0 as usize);
                                        if pg.is_null() { None } else { Some(unsafe { (*pg).refcount() }) }
                                    });
                                    taskdump_dec(rf.unwrap_or(-1i32) as i64 as u64);
                                    taskdump_raw_line(b"\n");
                                }
                            }
                            (*task_ptr).set_address_space(Some(child_arc));
                            // CLONE_CHILD_SETTID (fork path): store the
                            // child's TID through the child's OWN page
                            // tables — see the comment at the flag check.
                            if child_settid_pending
                                && !unsafe { write_child_settid(c_root, args.child_tid, pid as i32, task_ptr) }
                            {
                                return unwind(task_ptr, efault());
                            }
                        }
                        #[cfg(not(feature = "dfx-futex-trace"))]
                        {
                            let child_arc = alloc::sync::Arc::new(child_as);
                            let c_root = child_arc.root_ppn();
                            // FORENSIC (fake-OOM family): record the fresh
                            // fork root against its OWNING child pid. The
                            // teardown witness in PtLedger::take then flags
                            // any free of this root from a context that is
                            // NOT the owner while the owner is still alive —
                            // the early-teardown seed.
                            crate::arch::riscv64::mm::mmu_init::register_fork_root(
                                c_root as u64,
                                pid as u32,
                            );
                            (*task_ptr).set_address_space(Some(child_arc));
                            if child_settid_pending
                                && !unsafe { write_child_settid(c_root, args.child_tid, pid as i32, task_ptr) }
                            {
                                return unwind(task_ptr, efault());
                            }
                        }
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
            // SCHED_RESET_ON_FORK (Linux __sched_fork): a child of a task
            // that requested reset-on-fork does NOT inherit the privileged
            // policy/priority — it lands on SCHED_OTHER with nice 0, and
            // the reset request itself is cleared (LTP sched_setscheduler04).
            if parent.sched_reset_on_fork() {
                child.set_policy(crate::process::task::SchedPolicy::Normal);
                child.set_nice(0);
                child.set_rt_priority(0);
                child.set_sched_reset_on_fork(false);
            } else {
                child.set_policy(parent.policy());
                // set_nice derives static_prio/normal_prio/prio AND updates the
                // CFS entity weight — must go through it, not raw field writes.
                child.set_nice(parent.nice());
                child.set_rt_priority(parent.rt_priority());
                child.set_sched_reset_on_fork(false);
            }
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

        // === U1c namespaces: default share the parent's namespace objects
        // (Arc clone); CLONE_NEW* bits give the child fresh ones. Fails the
        // clone with EPERM when a non-user ns was requested without
        // CAP_SYS_ADMIN. ===
        if let Err(e) = crate::process::ns::copy_namespaces(current_ptr, task_ptr, args.flags) {
            return unwind(task_ptr, e);
        }

        // === U1c seccomp: mode and filter are inherited across fork
        // (Linux copy_process → seccomp_dup). ===
        (*task_ptr).set_seccomp_mode((*current_ptr).seccomp_mode());
        if (*current_ptr).seccomp_mode() == 2 {
            let parent_filter = (*current_ptr).seccomp_filter.lock();
            *(*task_ptr).seccomp_filter.lock() = parent_filter.clone();
        }

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
                (*current_ptr).pid(), pid);            (*current).set_state(TaskState::new(TaskState::UNINTERRUPTIBLE));

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

        #[cfg(feature = "dfx-futex-trace")]
        {
            use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
            taskdump_raw_line(b"FTX CLONE-OK child=");
            taskdump_dec(pid as u64);
            taskdump_raw_line(b"\n");
        }

        Ok(pid)
    }
}
