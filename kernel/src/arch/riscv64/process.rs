//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V process/thread management architecture-specific functions
//!
//! Note: the ACTIVE fork path is the GENERIC copy_thread in
//! process/fork.rs (frame at Task::pt_regs(), entry via
//! process::thread_set_entry). The arch copy_thread below is kept for
//! reference and has no callers; its epc handling matches the live path
//! (the parent frame's epc is ALREADY past the ecall — trap.rs's
//! handle_syscall advanced it — so no extra skip here).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::arch::pt_regs::{PtRegs, SR_PIE, SR_FS_INITIAL, SR_FS as SR_FS_MASK};
use crate::arch::mm::VirtAddr;
use crate::process::task::Task;
use core::arch::asm;

/// Start new user program
///
/// Set initial state for user process:
/// - PC points to program entry point
/// - SP points to user stack top
/// - Clear other general purpose registers
/// - Set sstatus (user mode, enable interrupts, FPU initial state)
///
/// # Arguments
/// - `regs`: PtRegs to modify
/// - `pc`: Program entry address
/// - `sp`: User stack pointer
///
/// # Example
/// ```ignore
/// let mut regs = PtRegs::default();
/// start_thread(&mut regs, entry_point, stack_top);
/// // Now regs can be used to return from trap to user program
/// ```
#[inline]
pub fn start_thread(regs: &mut PtRegs, pc: u64, sp: u64) {
    // Set PC and SP
    regs.epc = pc;
    regs.sp = sp;

    // Clear argument registers (a0-a7)
    regs.a0 = 0;
    regs.a1 = 0;
    regs.a2 = 0;
    regs.a3 = 0;
    regs.a4 = 0;
    regs.a5 = 0;
    regs.a6 = 0;
    regs.a7 = 0;

    // Clear return address
    regs.ra = 0;

    // Set sstatus:
    // - SPP = 0: Return to user mode
    // - SPIE = 1: Enable interrupts
    // - FS = INITIAL: FPU in initial state
    // SUM is deliberately NOT set (SUM convergence, review SEC): the kernel
    // keeps sstatus.SUM=0 and reaches user memory only via the uaccess
    // exception-table paths. Burning SUM into the user-saved status here
    // leaked a SUM=1 sstatus back into the kernel on every return-to-kernel
    // transition.
    regs.status = SR_PIE | SR_FS_INITIAL;

    // Clear cause and badaddr
    regs.cause = 0;
    regs.badaddr = 0;

    // Set orig_a0 to 0
    regs.orig_a0 = 0;
}

// (arch copy_thread DELETED, round 6: dead code — the live fork path is
// the generic copy_thread in process/fork.rs, writing the frame at
// Task::pt_regs(); this reference twin had no callers since that landed)
/// Clean up thread state
///
/// Collected into `arch::arch_floor` (INV-ARCH-PROCESS-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_thread() {
    crate::arch::arch_floor::riscv64::process::flush_thread()
}

/// Get current process's PtRegs
///
/// Returns register state saved at trap entry for current process
#[inline]
pub fn current_pt_regs() -> *const PtRegs {
    crate::arch::trap::current_pt_regs()
}
/// Get task's PtRegs
///
/// Collected into `arch::arch_floor` (INV-ARCH-PROCESS-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn task_pt_regs(task: *const Task) -> *const PtRegs {
    crate::arch::arch_floor::riscv64::process::task_pt_regs(task)
}

/// Get user stack pointer
///
/// Extract user stack pointer from PtRegs
#[inline]
pub fn user_stack_pointer(regs: &PtRegs) -> u64 {
    regs.sp
}

/// Set user stack pointer
///
/// Modify user stack pointer in PtRegs
#[inline]
pub fn set_user_stack_pointer(regs: &mut PtRegs, sp: u64) {
    regs.sp = sp;
}

/// Get instruction pointer
///
/// Extract program counter from PtRegs
#[inline]
pub fn instruction_pointer(regs: &PtRegs) -> u64 {
    regs.epc
}

/// Set instruction pointer
///
/// Modify program counter in PtRegs
#[inline]
pub fn set_instruction_pointer(regs: &mut PtRegs, pc: u64) {
    regs.epc = pc;
}

/// Check if address is in user space
///
/// RISC-V Sv39: User space address 0x0000_0000 - 0x003F_FFFF_FFFF
///
/// # Arguments
/// - `addr`: Address to check
///
/// # Returns
/// Returns true if in user space, false otherwise
#[inline]
pub fn is_user_address(addr: u64) -> bool {
    // Sv39: User address high 25 bits must be all 0 or all 1
    // User space: 0x0000_0000_0000_0000 - 0x0000_003F_FFFF_FFFF
    let addr_virt = VirtAddr::new(addr);
    addr_virt.bits() < 0x0040_0000_0000
}

// (copy_from_user/copy_to_user delegation wrappers DELETED, round 6:
// zero callers — users go through super::uaccess or sync::edges)
