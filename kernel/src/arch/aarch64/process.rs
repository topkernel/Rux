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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

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

// (arch copy_thread DELETED, round 6: interface-parity twin with zero
// callers — the live fork path is the generic copy_thread in
// process/fork.rs, writing the frame at Task::pt_regs())
/// Clean up thread state (execve): clear FPSIMD image so the new image
///
/// Collected into `arch::arch_floor` (INV-ARCH-PROCESS-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_thread() {
    crate::arch::arch_floor::aarch64::process::flush_thread()
}

/// Get current process's PtRegs
#[inline]
pub fn current_pt_regs() -> *const PtRegs {
    crate::arch::trap::current_pt_regs()
}
/// Get task's PtRegs (fork child / stack-top frame)
///
/// Collected into `arch::arch_floor` (INV-ARCH-PROCESS-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn task_pt_regs(task: *const Task) -> *const PtRegs {
    crate::arch::arch_floor::aarch64::process::task_pt_regs(task)
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
