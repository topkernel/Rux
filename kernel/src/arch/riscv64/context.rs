//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V 64-bit context switching
//!
//! context switch implementation

use crate::process::task::Task;
use crate::process::Task as ProcessTask;
use core::arch::asm;

/// sstatus.SUM bit mask
pub const SR_SUM: u64 = 1 << 18;

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

pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    prev.thread_mut().fpu_save_for_switch();
    next.thread_mut().restore_fpu();

    set_prev_task(prev as *mut Task);

    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        let current_satp = get_current_satp();
        let current_ppn = current_satp & 0xFFFFFFFFFFFFF;

        if current_ppn != next_ppn {
            switch_mm(next_ppn);
        }
    }

    __switch_to(prev, next);
}
