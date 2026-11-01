//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 process support — thread setup and register helpers.
//!
//! Note: the active fork path is the GENERIC copy_thread in
//! process/fork.rs (pt_regs at kernel-stack top, thread seeding through
//! process::thread_set_entry). The arch copy_thread here is kept for
//! interface parity with the other backends.

use super::mm::memory_layout::VirtAddr;
use super::pt_regs::{
    PtRegs, SPSR_DAIF, SPSR_M_EL0T, SPSR_M_MASK,
};
use crate::process::task::Task;

/// Initialize a user thread's register state (exec entry)
#[inline]
pub fn start_thread(regs: &mut PtRegs, pc: u64, sp: u64) {
    regs.pc = pc;
    regs.sp = sp;

    // Clear argument registers (x0-x7)
    for i in 0..8 {
        regs.regs[i] = 0;
    }
    // Clear lr
    regs.regs[30] = 0;

    // PSTATE for return to EL0: mode EL0t, interrupts UNMASKED
    // (the riscv64 twin sets SPIE=1 — same "return with irqs on" policy)
    regs.pstate = SPSR_M_EL0T & SPSR_M_MASK;
    regs.pstate &= !SPSR_DAIF;

    regs.esr = 0;
    regs.far = 0;
    regs.orig_x0 = 0;
    regs.syscallno = u64::MAX; // -1: not in a syscall (Linux arm64 convention)
    regs.frame_pad = [0; 2];
}

/// Copy thread state (fork) — interface parity; the active path is the
/// generic copy_thread in process/fork.rs (see module doc)
pub unsafe fn copy_thread(
    child: *mut Task,
    parent_regs: &PtRegs,
) -> Option<*mut PtRegs> {
    use alloc::alloc::{alloc, Layout};

    let layout = Layout::from_size_align(core::mem::size_of::<PtRegs>(), 16).ok()?;
    let mem_ptr = alloc(layout);
    if mem_ptr.is_null() {
        return None;
    }
    let child_regs = mem_ptr as *mut PtRegs;

    // Copy the parent's state; child x0 = 0 (fork return). NO pc advance:
    // the SVC exception leaves ELR (the frame's pc) at the instruction
    // AFTER the svc — the architectural return address — so advancing
    // here would skip one user instruction in the child (the exact bug
    // family documented in trap.rs handle_syscall; parity with the live
    // generic copy_thread in process/fork.rs, which copies verbatim).
    let mut frame = *parent_regs;
    frame.regs[0] = 0;
    frame.pc = parent_regs.pc;
    frame.orig_x0 = 0;
    core::ptr::write(child_regs, frame);

    (*child).set_fork_child(child_regs);

    extern "C" {
        fn ret_from_fork();
    }
    let thread = (*child).thread_mut();
    thread.ra = ret_from_fork as u64;
    thread.sp = child_regs as u64;

    Some(child_regs)
}

/// Clean up thread state (execve): clear FPSIMD image so the new image
/// starts from a zero FP state (the riscv64 "FP residue" lesson), and
/// reset the user TLS pointer — the new image's TLS is installed by its
/// own libc startup, and until then a stale foreign base is worse than
/// zero (a fault at ~(base + slot) reads as memory corruption).
#[inline]
pub fn flush_thread() {
    if let Some(current) = crate::sched::current() {
        // SAFETY: we are the current task; no concurrent mutation of our
        // thread state occurs.
        unsafe {
            let thread = (*current).thread_mut();
            thread.fpu.fill(0);
            thread.fpcr = 0;
            thread.fpsr = 0;
            thread.fs = 1; // INITIAL
            // The USER image too: a fresh exec'd program must not observe
            // the previous image's FP registers through the eret-tail
            // restore (el0_exit_fpsimd_restore) or a signal frame.
            thread.user_fpsimd = Default::default();
            // TLS: saved image AND the live register (no context switch
            // is guaranteed between here and the new image's first user
            // instruction, so the register itself must be cleared too).
            thread.tp_value = 0;
        }
        // SAFETY: writing our own user TLS register to 0; no side
        // effects beyond the register value.
        unsafe {
            core::arch::asm!("msr tpidr_el0, xzr", options(nostack, preserves_flags));
        }
    }
}

/// Get current process's PtRegs
#[inline]
pub fn current_pt_regs() -> *const PtRegs {
    crate::arch::trap::current_pt_regs()
}

/// Get task's PtRegs (fork child / stack-top frame)
#[inline]
pub fn task_pt_regs(task: *const Task) -> *const PtRegs {
    // SAFETY: caller provides a valid Task pointer; fork_pt_regs()
    // returns the pointer stored by copy_thread, valid for the task
    // lifetime.
    unsafe { (*task).fork_pt_regs() }
}

/// Get user stack pointer
#[inline]
pub fn user_stack_pointer(regs: &PtRegs) -> u64 {
    regs.sp
}

/// Set user stack pointer
#[inline]
pub fn set_user_stack_pointer(regs: &mut PtRegs, sp: u64) {
    regs.sp = sp;
}

/// Get instruction pointer
#[inline]
pub fn instruction_pointer(regs: &PtRegs) -> u64 {
    regs.pc
}

/// Set instruction pointer
#[inline]
pub fn set_instruction_pointer(regs: &mut PtRegs, pc: u64) {
    regs.pc = pc;
}

/// Check if address is in user space (TTBR0 low half, below TASK_SIZE)
#[inline]
pub fn is_user_address(addr: u64) -> bool {
    let addr_virt = VirtAddr::new(addr);
    addr_virt.bits() < super::mm::memory_layout::TASK_SIZE as u64
}

/// arch_prctl — aarch64 has no FS/GS bases; accept the generic no-ops
pub fn sys_arch_prctl(_code: i64, _addr: u64) -> i64 {
    -22 // -EINVAL
}
