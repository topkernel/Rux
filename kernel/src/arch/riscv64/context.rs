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

use crate::process::task::Task;
use crate::process::Task as ProcessTask;
use core::arch::asm;

/// sstatus.SUM bit mask
pub const SR_SUM: u64 = 1 << 18;

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

// ============================================================================
// __switch_to
// ============================================================================

core::arch::global_asm!(
    r#"
.section .text.__switch_to
.align 2

.global __switch_to
.type __switch_to, @function
__switch_to:
    # a0 = prev task, a1 = next task
    # Calculate thread struct pointers
    li    a4, {task_thread}
    add   a3, a0, a4
    add   a4, a1, a4

    # Save prev's context
    sd    ra,  {thread_ra}(a3)
    sd    sp,  {thread_sp}(a3)
    sd    s0,  {thread_s0} + 0*8(a3)
    sd    s1,  {thread_s0} + 1*8(a3)
    sd    s2,  {thread_s0} + 2*8(a3)
    sd    s3,  {thread_s0} + 3*8(a3)
    sd    s4,  {thread_s0} + 4*8(a3)
    sd    s5,  {thread_s0} + 5*8(a3)
    sd    s6,  {thread_s0} + 6*8(a3)
    sd    s7,  {thread_s0} + 7*8(a3)
    sd    s8,  {thread_s0} + 8*8(a3)
    sd    s9,  {thread_s0} + 9*8(a3)
    sd    s10, {thread_s0} + 10*8(a3)
    sd    s11, {thread_s0} + 11*8(a3)

    # Save sstatus.SUM bit
    csrr  t0, sstatus
    sd    t0, {thread_sum}(a3)

    # Restore next's context
    # First restore SUM bit (use t0 as temp)
    ld    t0, {thread_sum}(a4)
    li    t1, {sr_sum}
    and   t0, t0, t1
    csrs  sstatus, t0

    # Now restore callee-saved registers (s0 last)
    ld    ra,  {thread_ra}(a4)
    ld    sp,  {thread_sp}(a4)
    ld    s11, {thread_s0} + 11*8(a4)
    ld    s10, {thread_s0} + 10*8(a4)
    ld    s9,  {thread_s0} + 9*8(a4)
    ld    s8,  {thread_s0} + 8*8(a4)
    ld    s7,  {thread_s0} + 7*8(a4)
    ld    s6,  {thread_s0} + 6*8(a4)
    ld    s5,  {thread_s0} + 5*8(a4)
    ld    s4,  {thread_s0} + 4*8(a4)
    ld    s3,  {thread_s0} + 3*8(a4)
    ld    s2,  {thread_s0} + 2*8(a4)
    ld    s1,  {thread_s0} + 1*8(a4)
    ld    s0,  {thread_s0} + 0*8(a4)

    # Update tp = next task
    mv    tp, a1

    ret
.size __switch_to, . - __switch_to
"#,
    task_thread = const core::mem::offset_of!(Task, thread),
    thread_ra = const core::mem::offset_of!(crate::arch::riscv64::thread::ThreadStruct, ra),
    thread_sp = const core::mem::offset_of!(crate::arch::riscv64::thread::ThreadStruct, sp),
    thread_s0 = const core::mem::offset_of!(crate::arch::riscv64::thread::ThreadStruct, s),
    thread_sum = const core::mem::offset_of!(crate::arch::riscv64::thread::ThreadStruct, sum),
    sr_sum = const SR_SUM,
);

// ============================================================================
// Per-CPU variable for prev task
// ============================================================================

static CPU_PREV_TASK: [core::sync::atomic::AtomicU64; 4] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < 4 {
        CPU_PREV_TASK[cpu].store(prev as u64, core::sync::atomic::Ordering::Relaxed);
    }
}

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

// ============================================================================
// switch_mm
// ============================================================================

#[inline]
pub unsafe fn switch_mm(next_ppn: u64) {
    let satp = (8u64 << 60) | next_ppn;
    asm!("csrw satp, {}", in(reg) satp, options(nostack));
    asm!("sfence.vma zero, zero", options(nostack));
}

#[inline]
pub fn get_current_satp() -> u64 {
    let satp: u64;
    unsafe {
        asm!("csrr {}, satp", out(reg) satp, options(nomem, nostack));
    }
    satp
}

// ============================================================================
// High-level context_switch
// ============================================================================

extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

/// Context switch wrapper function (Linux-style)
///
/// Flow (exactly like Linux):
/// 1. Save prev FPU state
/// 2. switch_mm() - switch page table if address space changed
/// 3. __switch_to() - switch registers
/// 4. Restore next FPU state (must be AFTER __switch_to!)
///
/// # Arguments
/// - `prev`: Previous task (being switched out)
/// - `next`: Next task (being switched in)
///
/// # Safety
/// Must be called with interrupts disabled (caller's responsibility)
///
/// # Note
/// The caller (schedule) must ensure interrupts are disabled before calling.
/// After __switch_to, the stack pointer changes, so we must use tp (thread pointer)
/// to get the current task for FPU restoration.
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Step 1: Save prev FPU state
    prev.thread_mut().fpu_save_for_switch();

    // Store prev task for ret_from_fork
    set_prev_task(prev as *mut Task);

    // Step 2: switch_mm() - Switch address space FIRST
    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        let current_satp = get_current_satp();
        let current_ppn = current_satp & 0xFFFFFFFFFFFFF;

        if current_ppn != next_ppn {
            switch_mm(next_ppn);
        }
    }

    // Step 3: __switch_to() - Switch registers SECOND
    //
    // CRITICAL: After __switch_to returns, we're in next's context.
    // - sp has changed to next's kernel stack
    // - All caller-saved registers (a0-a7, t0-t6) may contain garbage
    // - Function parameters (prev, next) are NO LONGER VALID
    __switch_to(prev, next);

    // Step 4: Restore FPU state
    // Get current task from tp (set by __switch_to)
    // We can't use the `next` parameter here - it's no longer valid!
    let current = current_task();
    current.thread_mut().restore_fpu();
}
