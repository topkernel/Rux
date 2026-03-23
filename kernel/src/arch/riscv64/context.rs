//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit context switching
//!
//! Linux-style context switch implementation:
//! - Save/restore callee-saved registers (ra, sp, s0-s11) to/from task.thread
//! - Save/restore SUM bit from sstatus
//! - Update tp register to point to next task
//!
//! Reference: Linux arch/riscv/kernel/entry.S __switch_to()

use crate::process::task::{Task, task_offsets::TASK_THREAD};
use super::thread::{thread_offsets::{THREAD_RA, THREAD_SP, THREAD_S0, THREAD_SUM}, SR_SUM};
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Per-CPU variable to store the previous task pointer during context switch.
/// This is used by ret_from_fork to get the prev task without corrupting s1.
/// Each CPU has its own slot indexed by CPU ID.
static CPU_PREV_TASK: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Set the prev task pointer for the current CPU
#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < 4 {
        CPU_PREV_TASK[cpu].store(prev as u64, Ordering::Relaxed);
    }
}

/// Get the prev task pointer for the current CPU (used by ret_from_fork)
#[inline]
#[no_mangle]
pub extern "C" fn get_prev_task() -> *mut Task {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < 4 {
        CPU_PREV_TASK[cpu].load(Ordering::Relaxed) as *mut Task
    } else {
        core::ptr::null_mut()
    }
}

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

// ============================================================================
// Linux-style __switch_to
// ============================================================================

/// Linux-style context switch
///
/// Saves prev's callee-saved registers to prev->thread, then restores
/// next's callee-saved registers from next->thread.
///
/// Arguments:
///   a0 = prev task pointer
///   a1 = next task pointer
///
/// Saved/Restored registers:
///   - ra (return address)
///   - sp (stack pointer)
///   - s0-s11 (callee-saved registers)
///   - sstatus.SUM bit
///
/// Reference: Linux arch/riscv/kernel/entry.S:386-436
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.__switch_to"]
pub unsafe extern "C" fn __switch_to(prev: *mut Task, next: *mut Task) {
    core::arch::naked_asm!(
        // Arguments: a0 = prev, a1 = next
        //
        // Save prev's context to prev->thread
        // t0 = &prev->thread
        "add   t0, a0, {thread_offset}",

        // Save ra, sp
        "sd    ra, {ra_off}(t0)",
        "sd    sp, {sp_off}(t0)",

        // Save s0-s11 (callee-saved registers)
        // s[0] = s0/fp, s[1] = s1, ..., s[11] = s11
        "sd    s0, 0*8 + {s0_off}(t0)",
        "sd    s1, 1*8 + {s0_off}(t0)",
        "sd    s2, 2*8 + {s0_off}(t0)",
        "sd    s3, 3*8 + {s0_off}(t0)",
        "sd    s4, 4*8 + {s0_off}(t0)",
        "sd    s5, 5*8 + {s0_off}(t0)",
        "sd    s6, 6*8 + {s0_off}(t0)",
        "sd    s7, 7*8 + {s0_off}(t0)",
        "sd    s8, 8*8 + {s0_off}(t0)",
        "sd    s9, 9*8 + {s0_off}(t0)",
        "sd    s10, 10*8 + {s0_off}(t0)",
        "sd    s11, 11*8 + {s0_off}(t0)",

        // Save sstatus.SUM bit to prev->thread.sum
        "csrr  t1, sstatus",
        "sd    t1, {sum_off}(t0)",

        // Restore next's context from next->thread
        // t0 = &next->thread
        "add   t0, a1, {thread_offset}",

        // Restore sstatus.SUM bit
        // Note: andi cannot be used because SR_SUM (2^18) exceeds 12-bit immediate range
        "ld    t1, {sum_off}(t0)",
        "li    t2, {sr_sum}",
        "and   t1, t1, t2",
        "csrs  sstatus, t1",

        // Restore ra, sp
        "ld    ra, {ra_off}(t0)",
        "ld    sp, {sp_off}(t0)",

        // Restore s0-s11
        "ld    s0, 0*8 + {s0_off}(t0)",
        "ld    s1, 1*8 + {s0_off}(t0)",
        "ld    s2, 2*8 + {s0_off}(t0)",
        "ld    s3, 3*8 + {s0_off}(t0)",
        "ld    s4, 4*8 + {s0_off}(t0)",
        "ld    s5, 5*8 + {s0_off}(t0)",
        "ld    s6, 6*8 + {s0_off}(t0)",
        "ld    s7, 7*8 + {s0_off}(t0)",
        "ld    s8, 8*8 + {s0_off}(t0)",
        "ld    s9, 9*8 + {s0_off}(t0)",
        "ld    s10, 10*8 + {s0_off}(t0)",
        "ld    s11, 11*8 + {s0_off}(t0)",

        // Update tp = next task
        "mv    tp, a1",

        "ret",

        // Constants
        thread_offset = const TASK_THREAD,
        ra_off = const THREAD_RA,
        sp_off = const THREAD_SP,
        s0_off = const THREAD_S0,
        sum_off = const THREAD_SUM,
        sr_sum = const SR_SUM,
    );
}

// ============================================================================
// Context switch with page table switch
// ============================================================================

/// Context switch with address space change
///
/// This function atomically switches page tables and performs context switch.
/// All operations are in assembly to ensure no Rust code executes between
/// page table switch and context save/restore.
///
/// Arguments:
///   a0 = prev task pointer
///   a1 = next task pointer
///   a2 = new_satp (satp value to switch to)
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.__switch_to_with_satp"]
pub unsafe extern "C" fn __switch_to_with_satp(prev: *mut Task, next: *mut Task, new_satp: u64) {
    core::arch::naked_asm!(
        // Arguments: a0 = prev, a1 = next, a2 = new_satp
        //
        // Save prev's context to prev->thread FIRST (before page table switch)
        "add   t0, a0, {thread_offset}",

        // Save ra, sp
        "sd    ra, {ra_off}(t0)",
        "sd    sp, {sp_off}(t0)",

        // Save s0-s11
        "sd    s0, 0*8 + {s0_off}(t0)",
        "sd    s1, 1*8 + {s0_off}(t0)",
        "sd    s2, 2*8 + {s0_off}(t0)",
        "sd    s3, 3*8 + {s0_off}(t0)",
        "sd    s4, 4*8 + {s0_off}(t0)",
        "sd    s5, 5*8 + {s0_off}(t0)",
        "sd    s6, 6*8 + {s0_off}(t0)",
        "sd    s7, 7*8 + {s0_off}(t0)",
        "sd    s8, 8*8 + {s0_off}(t0)",
        "sd    s9, 9*8 + {s0_off}(t0)",
        "sd    s10, 10*8 + {s0_off}(t0)",
        "sd    s11, 11*8 + {s0_off}(t0)",

        // Save sstatus.SUM bit
        "csrr  t1, sstatus",
        "sd    t1, {sum_off}(t0)",

        // ===== Switch page table =====
        "csrw  satp, a2",
        "sfence.vma",

        // ===== Restore next's context =====
        // t0 = &next->thread
        "add   t0, a1, {thread_offset}",

        // Restore sstatus.SUM bit
        // Note: andi cannot be used because SR_SUM (2^18) exceeds 12-bit immediate range
        "ld    t1, {sum_off}(t0)",
        "li    t2, {sr_sum}",
        "and   t1, t1, t2",
        "csrs  sstatus, t1",

        // Restore ra, sp
        "ld    ra, {ra_off}(t0)",
        "ld    sp, {sp_off}(t0)",

        // Restore s0-s11
        "ld    s0, 0*8 + {s0_off}(t0)",
        "ld    s1, 1*8 + {s0_off}(t0)",
        "ld    s2, 2*8 + {s0_off}(t0)",
        "ld    s3, 3*8 + {s0_off}(t0)",
        "ld    s4, 4*8 + {s0_off}(t0)",
        "ld    s5, 5*8 + {s0_off}(t0)",
        "ld    s6, 6*8 + {s0_off}(t0)",
        "ld    s7, 7*8 + {s0_off}(t0)",
        "ld    s8, 8*8 + {s0_off}(t0)",
        "ld    s9, 9*8 + {s0_off}(t0)",
        "ld    s10, 10*8 + {s0_off}(t0)",
        "ld    s11, 11*8 + {s0_off}(t0)",

        // Update tp = next task
        "mv    tp, a1",

        "ret",

        // Constants
        thread_offset = const TASK_THREAD,
        ra_off = const THREAD_RA,
        sp_off = const THREAD_SP,
        s0_off = const THREAD_S0,
        sum_off = const THREAD_SUM,
        sr_sum = const SR_SUM,
    );
}

// ============================================================================
// High-level context_switch function
// ============================================================================

/// Context switch wrapper function
///
/// This function combines FPU context switch, address space switch, and
/// register context switch.
///
/// # Arguments
/// - `prev`: Previous task (being switched out)
/// - `next`: Next task (being switched in)
///
/// # Safety
/// Must be called with proper locking and interrupt state management
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Disable interrupts in SMP environment to prevent race conditions during context switch
    let _irq_guard = InterruptGuard::new();

    // ===== FPU context switch (Linux-style) =====
    // Save prev task's FPU state and disable FPU
    prev.thread_mut().fpu_save_for_switch();

    // Store prev task for ret_from_fork (used by newly forked children)
    set_prev_task(prev as *mut Task);

    // ===== Switch address space =====
    // Get next task's address space and switch to it
    if let Some(next_mm) = next.address_space() {
        let next_satp = (8u64 << 60) | next_mm.root_ppn();
        let current_satp: u64;
        core::arch::asm!(
            "csrr {0}, satp",
            out(reg) current_satp,
            options(nomem, nostack)
        );

        // Only switch if address space is different
        if current_satp != next_satp {
            // Allocate ASID if not already allocated
            let asid = next_mm.alloc_asid().unwrap_or(0);
            let satp_with_asid = next_satp | ((asid as u64) << 44);

            // Call __switch_to_with_satp with page table switch
            __switch_to_with_satp(prev, next, satp_with_asid);
        } else {
            // Same address space, just do context switch without page table change
            __switch_to(prev, next);
        }
    } else {
        // No address space (e.g., kernel thread), just do context switch
        __switch_to(prev, next);
    }

    // ===== Below executes in next task's context =====
    // We return here when we're scheduled back in

    // Restore next task's FPU state (only if it has FPU state)
    next.thread_mut().restore_fpu();

    // InterruptGuard drops here, automatically restores interrupt state
}

