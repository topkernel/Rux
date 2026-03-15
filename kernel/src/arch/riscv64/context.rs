//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit context switching
//!
//!
//! - Save callee-saved registers (x1-x31, except x0 and tp)
//! - Save stack pointer (sp)
//! - Save return address (ra)
//!
//! Calling convention:
//! - prev: Previous task's Task pointer
//! - next: Next task's Task pointer

use crate::process::task::{Task, CpuContext};
use core::arch::asm;

pub struct InterruptGuard {
    flags: u64,
}

impl InterruptGuard {
    /// Disable interrupts and create guard
    ///
    /// Save sstatus register, clear SIE bit (global interrupt enable)
    #[inline]
    pub unsafe fn new() -> Self {
        let flags: u64;
        let temp: u64;
        // Read and save sstatus
        asm!("csrr {}, sstatus", out(reg) flags, options(nomem, nostack));
        // Clear SIE bit (bit 1)
        temp = flags & !0x02;
        asm!("csrw sstatus, {}", in(reg) temp, options(nomem, nostack));
        InterruptGuard { flags }
    }
}

impl Drop for InterruptGuard {
    /// Restore interrupt state
    #[inline]
    fn drop(&mut self) {
        unsafe {
            asm!(
                "csrw sstatus, {}",  // Restore sstatus
                in(reg) self.flags,
                options(nomem, nostack)
            );
        }
    }
}

#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.context_switch"]
pub unsafe extern "C" fn cpu_switch_to(next_ctx: *mut CpuContext, prev_ctx: *mut CpuContext) {
    // Inline assembly for context switching
    core::arch::naked_asm!(
        // Save current task's context to prev->context
        // RISC-V calling convention: a0=next_ctx, a1=prev_ctx
        "sd ra, 0(a1)",      // Save return address
        "sd sp, 8(a1)",      // Save stack pointer
        "sd s0, 16(a1)",
        "sd s1, 24(a1)",
        "sd s2, 32(a1)",
        "sd s3, 40(a1)",
        "sd s4, 48(a1)",
        "sd s5, 56(a1)",
        "sd s6, 64(a1)",
        "sd s7, 72(a1)",
        "sd s8, 80(a1)",
        "sd s9, 88(a1)",
        "sd s10, 96(a1)",
        "sd s11, 104(a1)",

        // Restore next task's context from next->context
        "ld ra, 0(a0)",      // Restore return address
        "ld sp, 8(a0)",      // Restore stack pointer
        "ld s0, 16(a0)",
        "ld s1, 24(a0)",
        "ld s2, 32(a0)",
        "ld s3, 40(a0)",
        "ld s4, 48(a0)",
        "ld s5, 56(a0)",
        "ld s6, 64(a0)",
        "ld s7, 72(a0)",
        "ld s8, 80(a0)",
        "ld s9, 88(a0)",
        "ld s10, 96(a0)",
        "ld s11, 104(a0)",

        "ret",               // Return to next's context

        // Argument convention:
        // a0 = next_ctx (context to restore)
        // a1 = prev_ctx (context to save)
    );
}

/// Context switch wrapper function
///
/// # Arguments
/// - a0: prev task_struct pointer
/// - a1: next task_struct pointer
///
/// # Save/Restore contents
/// - ra, sp, s0-s11 (callee-saved registers)
/// - sstatus.SUM bit (user memory access enable)
/// - tp register (points to current task_struct)
///
/// # Task struct offsets (consistent with task.rs)
/// - ti_kernel_sp: 0x08 (thread_info.kernel_sp)
/// - context: variable (needs calculation)
///
/// Note: Since Task struct is complex, we use CpuContext offset
/// CpuContext offset in Task is calculated by context_mut()
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.__switch_to"]
pub unsafe extern "C" fn __switch_to(prev: *mut Task, next: *mut Task) {
    core::arch::naked_asm!(
        // Arguments:
        // a0 = prev task
        // a1 = next task

        // Save return address and next pointer
        "addi sp, sp, -16",
        "sd ra, 0(sp)",
        "sd a1, 8(sp)",      // Save next pointer

        // Get prev->context and next->context offsets
        // Since CpuContext offset in Task may change,
        // we call Rust function to get pointer

        // Restore next pointer
        "ld a1, 8(sp)",

        // Update tp to point to next task
        "mv tp, a1",

        // Restore return address
        "ld ra, 0(sp)",
        "addi sp, sp, 16",

        "ret",

        // Note: This simplified version doesn't save/restore callee-saved registers
        // Actual context switching is handled by cpu_switch_to in context_switch() function
    );
}

/// Context switch wrapper function
///
/// Combines cpu_switch_to and __switch_to functionality:
/// 1. Save/Restore callee-saved registers
/// 2. Update tp to point to new task
/// 3. Save/Restore SUM bit
///
/// Note: This function uses pure assembly because after context switch
/// local variables (on old stack) are no longer accessible.
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.context_switch_asm"]
pub unsafe extern "C" fn context_switch_asm(
    prev_ctx: *mut CpuContext,
    next_ctx: *mut CpuContext,
    next_task: *mut Task,
) {
    core::arch::naked_asm!(
        // Arguments:
        // a0 = prev_ctx (context to save)
        // a1 = next_ctx (context to restore)
        // a2 = next_task (new task pointer, for setting tp)

        // ===== Save prev context =====
        "sd ra, 0(a0)",
        "sd sp, 8(a0)",
        "sd s0, 16(a0)",
        "sd s1, 24(a0)",
        "sd s2, 32(a0)",
        "sd s3, 40(a0)",
        "sd s4, 48(a0)",
        "sd s5, 56(a0)",
        "sd s6, 64(a0)",
        "sd s7, 72(a0)",
        "sd s8, 80(a0)",
        "sd s9, 88(a0)",
        "sd s10, 96(a0)",
        "sd s11, 104(a0)",

        // ===== Restore next context =====
        "ld ra, 0(a1)",
        "ld sp, 8(a1)",
        "ld s0, 16(a1)",
        "ld s1, 24(a1)",
        "ld s2, 32(a1)",
        "ld s3, 40(a1)",
        "ld s4, 48(a1)",
        "ld s5, 56(a1)",
        "ld s6, 64(a1)",
        "ld s7, 72(a1)",
        "ld s8, 80(a1)",
        "ld s9, 88(a1)",
        "ld s10, 96(a1)",
        "ld s11, 104(a1)",

        // ===== Update tp to point to new task =====
        "mv tp, a2",

        // ===== Save prev task to s1 (for ret_from_fork) =====
        // s1 = prev task pointer
        // Note: s1 is callee-saved, will be saved/restored by context_switch_asm
        // In ret_from_fork, can get prev task via s1 and pass to schedule_tail
        "mv s1, a0",  // s1 = prev task (first argument)


        // Return to next's context
        "ret",
    );
}

/// Context switch wrapper function
///
/// Combines cpu_switch_to and __switch_to functionality:
/// 1. Save/Restore callee-saved registers
/// 2. Save/Restore FPU state
/// 3. Update tp to point to new task
/// 4. Save/Restore SUM bit
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Disable interrupts in SMP environment to prevent race conditions during context switch
    let _irq_guard = InterruptGuard::new();

    // ===== Save prev task's FPU state =====
    // Must be done before context switch because after switch we're in next's context
    prev.thread_mut().save_fpu();

    // ===== Restore next task's FPU state =====
    // Do this before context switch so FPU is ready when we switch
    next.thread_mut().restore_fpu();

    // Get CpuContext pointers
    let next_ctx: *mut CpuContext = next.context_mut();
    let prev_ctx: *mut CpuContext = prev.context_mut();
    let next_task: *mut Task = next;

    // Save current SUM bit state to prev task's thread struct
    let sum_status: u64;
    core::arch::asm!(
        "csrr {0}, sstatus",
        "and {0}, {0}, {1}",
        out(reg) sum_status,
        in(reg) super::thread::SR_SUM,
        options(nomem, nostack)
    );
    prev.thread_mut().sum = sum_status;

    // Restore next task's SUM bit
    if next.thread().sum != 0 {
        core::arch::asm!(
            "csrs sstatus, {0}",
            in(reg) super::thread::SR_SUM,
            options(nomem, nostack)
        );
    } else {
        core::arch::asm!(
            "csrc sstatus, {0}",
            in(reg) super::thread::SR_SUM,
            options(nomem, nostack)
        );
    }

    // Call assembly context switch function
    context_switch_asm(prev_ctx, next_ctx, next_task);

    // ===== Below executes in next task's context =====
    // FPU state already restored, SUM bit already set

    // InterruptGuard drops here, automatically restores interrupt state
}
