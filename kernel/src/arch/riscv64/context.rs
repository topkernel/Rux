//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit context switching
//!
//! Linux-style context switch implementation:
//! - switch_mm(): Switch page table (write satp) - called FIRST
//! - __switch_to(): Switch registers (ra, sp, s0-s11) - called SECOND
//!
//! Reference: Linux kernel/sched/core.c context_switch()
//!            Linux arch/riscv/kernel/entry.S __switch_to()

use crate::process::task::{Task, task_offsets::TASK_THREAD};
use super::thread::{thread_offsets::{THREAD_RA, THREAD_SP, THREAD_S0, THREAD_SUM}, SR_SUM};
use core::arch::asm;

/// Get current task pointer from tp register
///
/// After __switch_to, the tp register contains a pointer to the current task.
/// This function reads tp and returns it as a Task reference.
///
/// # Safety
/// This function is safe to call after __switch_to has set tp to a valid task pointer.
#[inline]
fn current_task() -> &'static mut Task {
    unsafe {
        let tp: u64;
        asm!("mv {}, tp", out(reg) tp, options(nomem, nostack, pure));
        // tp is guaranteed to be a valid task pointer after __switch_to
        &mut *(tp as *mut Task)
    }
}

/// Per-CPU variable to store the previous task pointer during context switch.
/// This is used by ret_from_fork to get the prev task without corrupting s1.
static CPU_PREV_TASK: [core::sync::atomic::AtomicU64; 4] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

/// Set the prev task pointer for the current CPU
#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < 4 {
        CPU_PREV_TASK[cpu].store(prev as u64, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Get the prev task pointer for the current CPU (used by ret_from_fork)
#[inline]
#[no_mangle]
pub extern "C" fn get_prev_task() -> *mut Task {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < 4 {
        CPU_PREV_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed) as *mut Task
    } else {
        core::ptr::null_mut()
    }
}

pub struct InterruptGuard {
    flags: u64,
}

impl InterruptGuard {
    /// Disable interrupts and create guard
    #[inline]
    pub unsafe fn new() -> Self {
        let flags: u64;
        let temp: u64;
        asm!("csrr {}, sstatus", out(reg) flags, options(nomem, nostack));
        temp = flags & !0x02;
        asm!("csrw sstatus, {}", in(reg) temp, options(nomem, nostack));
        InterruptGuard { flags }
    }
}

impl Drop for InterruptGuard {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            asm!("csrw sstatus, {}", in(reg) self.flags, options(nomem, nostack));
        }
    }
}

// ============================================================================
// Linux-style switch_mm - Switch page table (satp)
// ============================================================================

/// Switch address space (Linux: arch/riscv/mm/context.c switch_mm())
///
/// This function switches the page table by writing to satp CSR.
/// Must be called BEFORE __switch_to().
///
/// # Arguments
/// - `next_ppn`: Root page table physical page number for next task
///
/// # Safety
/// Must be called with interrupts disabled
#[inline]
pub unsafe fn switch_mm(next_ppn: u64) {
    // Linux: csr_write(CSR_SATP, virt_to_pfn(mm->pgd) | satp_mode);
    // satp format: [MODE:63-60][ASID:59-44][PPN:43-0]
    // MODE = 8 for Sv39
    let satp = (8u64 << 60) | next_ppn;

    // Write satp
    asm!("csrw satp, {}", in(reg) satp, options(nostack));

    // Flush TLB (Linux: local_flush_tlb_all_asid(0) for noasid case)
    // Using sfence.vma with rs1=0, rs2=0 flushes all TLB entries
    asm!("sfence.vma zero, zero", options(nostack));
}

/// Get current satp value
#[inline]
pub fn get_current_satp() -> u64 {
    let satp: u64;
    unsafe {
        asm!("csrr {}, satp", out(reg) satp, options(nomem, nostack));
    }
    satp
}

// ============================================================================
// Linux-style __switch_to - Switch registers (no satp)
// ============================================================================

/// Switch to next task (Linux: arch/riscv/kernel/entry.S __switch_to())
///
/// This function ONLY switches callee-saved registers.
/// Page table switch (satp) must be done BEFORE calling this via switch_mm().
///
/// Arguments:
///   a0 = prev task pointer (must be preserved for schedule_tail)
///   a1 = next task pointer
///
/// Callee-saved registers: ra, sp, s0-s11
/// Also saves/restores sstatus.SUM bit
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.__switch_to"]
pub unsafe extern "C" fn __switch_to(prev: *mut Task, next: *mut Task) {
    core::arch::naked_asm!(
        // Arguments: a0 = prev, a1 = next
        // IMPORTANT: Do NOT modify a0 - it must be preserved for schedule_tail
        //
        // Linux reference (arch/riscv/kernel/entry.S:386-436):
        //   li    a4, TASK_THREAD_RA
        //   add   a3, a0, a4
        //   add   a4, a1, a4
        //   REG_S ra, TASK_THREAD_RA_RA(a3)
        //   ...

        // ===== Compute thread pointers for both tasks =====
        // Linux: li a4, TASK_THREAD_RA; add a3, a0, a4; add a4, a1, a4
        // a3 = &prev->thread
        // a4 = &next->thread
        "add   a3, a0, {thread_offset}",
        "add   a4, a1, {thread_offset}",

        // ===== Save prev's context to prev->thread =====
        // Save ra, sp (Linux: REG_S ra, TASK_THREAD_RA_RA(a3))
        "sd    ra, {ra_off}(a3)",
        "sd    sp, {sp_off}(a3)",

        // Save s0-s11 (Linux: REG_S s0-s11, TASK_THREAD_S*_RA(a3))
        "sd    s0, 0*8 + {s0_off}(a3)",
        "sd    s1, 1*8 + {s0_off}(a3)",
        "sd    s2, 2*8 + {s0_off}(a3)",
        "sd    s3, 3*8 + {s0_off}(a3)",
        "sd    s4, 4*8 + {s0_off}(a3)",
        "sd    s5, 5*8 + {s0_off}(a3)",
        "sd    s6, 6*8 + {s0_off}(a3)",
        "sd    s7, 7*8 + {s0_off}(a3)",
        "sd    s8, 8*8 + {s0_off}(a3)",
        "sd    s9, 9*8 + {s0_off}(a3)",
        "sd    s10, 10*8 + {s0_off}(a3)",
        "sd    s11, 11*8 + {s0_off}(a3)",

        // Save sstatus.SUM bit (Linux: csrr s0, CSR_STATUS; REG_S s0, TASK_THREAD_SUM_RA(a3))
        "csrr  s0, sstatus",
        "sd    s0, {sum_off}(a3)",

        // ===== Restore next's context from next->thread =====

        // Restore sstatus.SUM bit (Linux: REG_L s0, TASK_THREAD_SUM_RA(a4); csrs CSR_STATUS, s0)
        "ld    s0, {sum_off}(a4)",
        "li    s1, {sr_sum}",
        "and   s0, s0, s1",
        "csrs  sstatus, s0",

        // Restore ra, sp (Linux: REG_L ra/sp, TASK_THREAD_RA/SP_RA(a4))
        "ld    ra, {ra_off}(a4)",
        "ld    sp, {sp_off}(a4)",

        // Restore s0-s11 (Linux: REG_L s0-s11, TASK_THREAD_S*_RA(a4))
        "ld    s0, 0*8 + {s0_off}(a4)",
        "ld    s1, 1*8 + {s0_off}(a4)",
        "ld    s2, 2*8 + {s0_off}(a4)",
        "ld    s3, 3*8 + {s0_off}(a4)",
        "ld    s4, 4*8 + {s0_off}(a4)",
        "ld    s5, 5*8 + {s0_off}(a4)",
        "ld    s6, 6*8 + {s0_off}(a4)",
        "ld    s7, 7*8 + {s0_off}(a4)",
        "ld    s8, 8*8 + {s0_off}(a4)",
        "ld    s9, 9*8 + {s0_off}(a4)",
        "ld    s10, 10*8 + {s0_off}(a4)",
        "ld    s11, 11*8 + {s0_off}(a4)",

        // Update tp = next task (Linux: move tp, a1)
        "mv    tp, a1",

        // Return with a0 preserved (for schedule_tail)
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
// Debug: Print offset values
// ============================================================================

/// Print offset constants for debugging (call once during boot)
pub fn print_offsets() {
    use crate::process::task::task_offsets::TASK_THREAD;
    use super::thread::thread_offsets::{THREAD_RA, THREAD_SP, THREAD_S0, THREAD_SUM};

    crate::println!("[OFFSETS] TASK_THREAD={:#x}", TASK_THREAD);
    crate::println!("[OFFSETS] THREAD_RA={:#x} THREAD_SP={:#x} THREAD_S0={:#x} THREAD_SUM={:#x}",
        THREAD_RA, THREAD_SP, THREAD_S0, THREAD_SUM);
    crate::println!("[OFFSETS] size_of::<ThreadStruct>={:#x}", core::mem::size_of::<super::thread::ThreadStruct>());
}

// ============================================================================
// High-level context_switch function (Linux: kernel/sched/core.c context_switch())
// ============================================================================

/// Context switch wrapper function (Linux-style)
///
/// Flow (exactly like Linux):
/// 1. Save prev FPU state
/// 2. switch_mm() - switch page table if address space changed
/// 3. __switch_to() - switch registers
/// 4. Restore next FPU state
///
/// # Arguments
/// - `prev`: Previous task (being switched out)
/// - `next`: Next task (being switched in)
///
/// # Safety
/// Must be called with interrupts disabled (caller's responsibility)
///
/// # Note
/// Unlike the previous implementation, this does NOT use InterruptGuard.
/// The caller (schedule) must ensure interrupts are disabled before calling.
/// This is because InterruptGuard stores data on the stack, but after __switch_to
/// the stack pointer changes, making the guard's data inaccessible.
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // NOTE: Interrupts must be disabled by caller (schedule holds spinlock)

    // ===== Step 1: Save prev FPU state =====
    prev.thread_mut().fpu_save_for_switch();

    // Store prev task for ret_from_fork
    set_prev_task(prev as *mut Task);

    // ===== Step 2: switch_mm() - Switch address space FIRST =====
    // Linux: if (!next->mm) { ... } else { switch_mm_irqs_off(prev->active_mm, next->mm, next); }
    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        let current_satp = get_current_satp();
        let current_ppn = current_satp & 0xFFFFFFFFFFFFF;

        // Only switch if address space is different (Linux: if (prev == next) return;)
        if current_ppn != next_ppn {
            // Linux-style switch_mm: write satp, then flush TLB
            switch_mm(next_ppn);
        }
    }
    // Note: kernel threads (next->mm == NULL) don't switch mm

    // ===== Step 3: __switch_to() - Switch registers SECOND =====
    // Linux: switch_to(prev, next, prev);
    //
    // CRITICAL: After __switch_to returns, we're in next's context.
    // - sp has changed to next's kernel stack
    // - All caller-saved registers (a0-a7, t0-t6) may contain garbage
    // - Function parameters (prev, next) are NO LONGER VALID
    //
    // To access the current task after the switch, we must use tp (thread pointer)
    // which __switch_to set to point to the next task.
    __switch_to(prev, next);

    // ===== Step 4: Restore FPU state =====
    // Get current task from tp (set by __switch_to)
    // We can't use the `next` parameter here - it's no longer valid!
    let current = current_task();
    current.thread_mut().restore_fpu();
}
