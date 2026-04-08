//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V process/thread management architecture-specific functions
//!
//! Main functions:
//! - `start_thread`: Start new program with execve
//! - `copy_thread`: Copy thread state with fork
//! - `flush_thread`: Clean up thread state

use crate::arch::riscv64::pt_regs::{PtRegs, SR_PIE, SR_SPP, SR_SUM, SR_FS_INITIAL};
use crate::arch::riscv64::mm::VirtAddr;
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
    // - SUM = 1: Allow S-mode to access user memory
    // - FS = INITIAL: FPU in initial state
    regs.status = SR_PIE | SR_SUM | SR_FS_INITIAL;

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
///
/// # Arguments
/// - `child`: Child process task structure
/// - `parent_regs`: Parent's PtRegs
///
/// # Returns
/// Returns child's PtRegs pointer on success, None on failure
///
/// # Note
/// Memory allocated by this function is caller's responsibility to free
pub unsafe fn copy_thread(
    child: *mut Task,
    parent_regs: &PtRegs,
) -> Option<*mut PtRegs> {
    use alloc::alloc::{alloc, Layout};

    // Allocate memory for child's PtRegs
    let pt_regs_size = core::mem::size_of::<PtRegs>();
    let layout = Layout::from_size_align(pt_regs_size, 16).ok()?;

    let mem_ptr = alloc(layout);
    if mem_ptr.is_null() {
        return None;
    }

    let child_regs = mem_ptr as *mut PtRegs;

    // Copy parent's register state
    // Note: epc + 4 to skip ecall instruction
    core::ptr::write(child_regs, PtRegs {
        epc: parent_regs.epc + 4,     // Skip ecall instruction
        ra: parent_regs.ra,
        sp: parent_regs.sp,           // User stack pointer
        gp: parent_regs.gp,           // Global pointer
        tp: parent_regs.tp,           // Thread pointer (TLS)
        t0: parent_regs.t0,
        t1: parent_regs.t1,
        t2: parent_regs.t2,
        s0: parent_regs.s0,
        s1: parent_regs.s1,
        a0: 0,                        // Child return value is 0
        a1: parent_regs.a1,
        a2: parent_regs.a2,
        a3: parent_regs.a3,
        a4: parent_regs.a4,
        a5: parent_regs.a5,
        a6: parent_regs.a6,
        a7: parent_regs.a7,
        s2: parent_regs.s2,
        s3: parent_regs.s3,
        s4: parent_regs.s4,
        s5: parent_regs.s5,
        s6: parent_regs.s6,
        s7: parent_regs.s7,
        s8: parent_regs.s8,
        s9: parent_regs.s9,
        s10: parent_regs.s10,
        s11: parent_regs.s11,
        t3: parent_regs.t3,
        t4: parent_regs.t4,
        t5: parent_regs.t5,
        t6: parent_regs.t6,
        status: parent_regs.status,   // sstatus
        badaddr: parent_regs.badaddr, // stval
        cause: parent_regs.cause,     // scause
        orig_a0: 0,                   // Child orig_a0 = 0
    });

    // Set child's fork info
    (*child).set_fork_child(child_regs);

    // Set up thread struct for context switch
    extern "C" {
        fn ret_from_fork();
    }

    let thread = (*child).thread_mut();
    thread.ra = ret_from_fork as u64;  // Return address = ret_from_fork
    thread.sp = child_regs as u64;     // Stack pointer = pt_regs address

    Some(child_regs)
}

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
            thread.fs = crate::arch::riscv64::pt_regs::SR_FS_OFF as u32;

            // Clear vector state (TODO: implement when V extension is supported)
            thread.vstate_valid = false;
        }
    }
}

/// Get current process's PtRegs
///
/// Returns register state saved at trap entry for current process
#[inline]
pub fn current_pt_regs() -> *const PtRegs {
    crate::arch::riscv64::trap::current_pt_regs()
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
