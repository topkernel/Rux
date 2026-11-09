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

/// Copy thread state (fork)
///
/// Create initial state for child process:
/// - Copy parent's register state
/// - Set child return value to 0 (a0 = 0)
/// - Set return address to ret_from_fork

// The arch copy_thread was deleted (review4 arch-L): zero callers, and
// it heap-allocated the child's PtRegs — a revived call would push trap
// frames below a HEAP address instead of the kernel stack. The live path
// is the GENERIC copy_thread in process/fork.rs.

/// Clean up thread state
///
/// Called during execve to clean up old thread state:
/// - Clear FPU state (frm: round to nearest, fflags: cleared)
/// - Clear vector extension state
/// - Other architecture-specific cleanup
#[inline]
pub fn flush_thread() {
    // Get current task
    if let Some(current) = crate::sched::current() {
        // SAFETY: current() returns a valid pointer to the current task_struct.
        // We are the current task, so no concurrent mutation of thread state occurs.
        unsafe {
            let thread = (*current).thread_mut();

            // Clear FPU state
            thread.fpu.fill(0);
            thread.fcsr = 0;
            thread.fs = crate::arch::pt_regs::SR_FS_OFF as u32;

            // Clear vector state: the new image starts from the
            // initial vector state (vstate_valid=false keeps VS=Off;
            // its first vector instruction lazily re-initializes).
            thread.vstate_valid = false;
            thread.vs = 0;
            thread.vcsrs = [0; 4];
            thread.vregs = [0; crate::arch::riscv64::vext::VREGS_BYTES];

            // Drop the LIVE FPU as well (review ARCH, FP residue): without
            // this, the next context switch-out would record the live FS
            // (still Clean/Dirty from the old image) into thread.fs and
            // resurrect the old image's FP registers after execve — leaking
            // data across images. With the live FS=Off, the new image's
            // first FP instruction traps and goes through the fpu_init
            // path (zeroed registers).
            let sstatus: u64;
            asm!("csrr {}, sstatus", out(reg) sstatus);
            let new_sstatus = sstatus & !SR_FS_MASK & !crate::arch::riscv64::vext::SR_VS;
            asm!("csrw sstatus, {}", in(reg) new_sstatus);
        }
    }
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
/// # Arguments
/// - `task`: Task structure pointer
///
/// # Returns
/// Task's PtRegs pointer
///
/// # Note
/// For running task, should use current_pt_regs()
/// This function is mainly for getting forked child's PtRegs
#[inline]
pub fn task_pt_regs(task: *const Task) -> *const PtRegs {
    // SAFETY: Caller must provide a valid Task pointer. fork_pt_regs() returns the
    // PtRegs pointer stored by copy_thread(), which remains valid for the task lifetime.
    unsafe {
        // Task structure's fork_child field stores PtRegs pointer
        (*task).fork_pt_regs()
    }
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

/// Read user space data
///
/// Safely read data from user space, returns error if access fails
///
/// # Arguments
/// - `to`: Destination buffer (kernel space)
/// - `from`: Source address (user space)
/// - `count`: Number of bytes to read
///
/// # Returns
/// Returns 0 on success, returns uncopied bytes (positive) or negative error code on failure
pub unsafe fn copy_from_user(
    to: *mut u8,
    from: *const u8,
    count: usize,
) -> isize {
    // Use exception table version from uaccess module
    let uncopied = super::uaccess::copy_from_user(to, from, count);
    uncopied as isize
}

/// Write user space data
///
/// Safely write data to user space, returns error if access fails
///
/// # Arguments
/// - `to`: Destination address (user space)
/// - `from`: Source data (kernel space)
/// - `count`: Number of bytes to write
///
/// # Returns
/// Returns 0 on success, returns unwritten bytes (positive) or negative error code on failure
pub unsafe fn copy_to_user(
    to: *mut u8,
    from: *const u8,
    count: usize,
) -> isize {
    // Use exception table version from uaccess module
    let uncopied = super::uaccess::copy_to_user(to, from, count);
    uncopied as isize
}
