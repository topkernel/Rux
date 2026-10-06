//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 context switching.
//!
//! # Safety Invariants — Context Switch Atomicity (x86 port of the
//! riscv64 twin's INV-CS-* series)
//!
//! - **INV-CS-1**: `__switch_to` saves/restores the callee-saved GPRs
//!   (rbx, rbp, r12–r15) and rsp; caller-saved registers are already
//!   spilled in `schedule()`'s frame.
//!
//! - **INV-CS-2**: FPU state is saved/restored via `fpu_save_for_switch`
//!   / `restore_fpu` (fxsave64/fxrstor64) around the register switch.
//!
//! - **INV-CS-3**: CR3 is switched in `context_switch` BEFORE
//!   `__switch_to`, so the new task's page table is active when its
//!   registers are restored.  User PML4s share the kernel half
//!   (PML4[256..511] links), so the switch is safe from anywhere.
//!
//! - **INV-CS-4**: `__switch_to` does NOT enable IF.  The caller
//!   (`__schedule`) owns interrupt state via lock_irqsave/restore_irq.
//!
//! - **INV-CS-5**: After `__switch_to`, the previous task's stack is
//!   stale; the current task is only reachable via the per-CPU slot
//!   (`arch::smp::current_task_ptr`), published inside `__switch_to`.
//!
//! Layout note: fork builds child thread.sp = stack_top - 168 (the
//! complete user frame) and thread.callee.ret_addr = ret_from_fork, so
//! the first switch into the child "returns" into ret_from_fork which
//! pops the frame back to user mode.

use crate::process::task::Task;
use super::cpu::{read_cr3, rdmsr, wrmsr, write_cr3};

/// MSR_FS_BASE — per-task TLS pointer.
const MSR_FS_BASE: u32 = 0xC000_0100;

extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

/// Switch the address-space root (interface parity with the riscv64
/// twin's switch_mm; called from exec/exit paths outside context_switch).
///
/// # Safety
/// `next_ppn` must be a live page-table root PPN.  `asid` is ignored —
/// PCID is disabled at bring-up (see mm/asid.rs) and the CR3 reload
/// flushes non-global translations by itself.
pub unsafe fn switch_mm(next_ppn: u64, _asid: u16) {
    // SAFETY: caller guarantees a valid PML4 physical address.
    unsafe { write_cr3(super::mm::asid::build_satp(0, next_ppn)) };
}

// ============================================================================
// __switch_to
// ============================================================================

core::arch::global_asm!(
    r#"
.section .text.__switch_to
.align 8

.global __switch_to
.type __switch_to, @function
__switch_to:
    # rdi = prev task, rsi = next task

    # 1. Save prev's callee-saved registers + sp into prev->thread.
    #    (Everything is spilled through memory, no scratch needed.)
    movq %rbx, {task_thread}+{callee_rbx}(%rdi)
    movq %rbp, {task_thread}+{callee_rbp}(%rdi)
    movq %r12, {task_thread}+{callee_r12}(%rdi)
    movq %r13, {task_thread}+{callee_r13}(%rdi)
    movq %r14, {task_thread}+{callee_r14}(%rdi)
    movq %r15, {task_thread}+{callee_r15}(%rdi)
    # Return-address protocol (x86 keeps the return address on the
    # stack, the twin's ra is a register): stash the live return address
    # into callee.ret_addr and thread.sp = entry_rsp + 8 (the stack
    # position ABOVE the return-address slot).  The resume side's
    # `push callee.ret_addr; ret` then re-enters context_switch with
    # exactly the post-call rsp; for newborn tasks fork sets
    # thread.sp = stack_top - 168 (frame base) and ret_addr =
    # ret_from_fork, so the same push/ret lands on the child frame.
    movq 0(%rsp), %rax
    movq %rax, {task_thread}+{callee_ret}(%rdi)
    leaq 8(%rsp), %rax
    movq %rax, {task_thread}+{thread_sp}(%rdi)

    # 2. prev/next survive the helper calls in callee-saved registers
    #    (safe: their interrupted values were just stored in prev->thread).
    movq %rdi, %rbx            # rbx = prev
    movq %rsi, %r12            # r12 = next

    # ABI: entry rsp == 8 (mod 16); the resume point is already saved, so
    # aligning down here is free scratch space for the C helper calls.
    subq $8, %rsp

    # 3. Publish: current = next (release), prev->on_cpu = 0 (R8-1b
    #    parity — prev is fully saved above and now pickable), and
    #    TSS.rsp0 = next's kernel stack top so the next user entry lands
    #    at stack_top - 168.
    movq %rbx, %rdi
    movq %r12, %rsi
    call x86_switch_publish
    movq %r12, %rdi
    call x86_update_rsp0

    # 4. Restore next's context.  r12 (holding `next`) is reloaded LAST.
    movq {task_thread}+{callee_r13}(%r12), %r13
    movq {task_thread}+{callee_r14}(%r12), %r14
    movq {task_thread}+{callee_r15}(%r12), %r15
    movq {task_thread}+{callee_rbp}(%r12), %rbp
    movq {task_thread}+{thread_sp}(%r12), %rsp
    movq {task_thread}+{callee_ret}(%r12), %rax
    movq {task_thread}+{callee_rbx}(%r12), %rbx
    movq {task_thread}+{callee_r12}(%r12), %r12

    # 5. "Return" into the next task's saved return address —
    #    ret_from_fork for a newborn child.
    pushq %rax
    ret

.size __switch_to, . - __switch_to
"#,
    task_thread = const core::mem::offset_of!(Task, thread),
    callee_rbx = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, rbx),
    callee_rbp = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, rbp),
    callee_r12 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r12),
    callee_r13 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r13),
    callee_r14 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r14),
    callee_r15 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r15),
    callee_ret = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, ret_addr),
    thread_sp = const core::mem::offset_of!(crate::arch::thread::ThreadStruct, sp),
    options(att_syntax),
);

/// Publish the switch (called from `__switch_to` between the prev-save
/// and the next-restore).  Release semantics make prev's saved context
/// visible to any CPU that observes the current-task slot change —
/// the x86-TSO analogue of the riscv64 `fence rw,rw; on_cpu=0` pair.
///
/// # Safety
/// Called only from `__switch_to` with the prev context fully saved.
#[no_mangle]
pub unsafe extern "C" fn x86_switch_publish(prev: *mut Task, next: *mut Task) {
    if !prev.is_null() {
        // SAFETY: prev is not running anywhere anymore (we are on the
        // switch path); the bool write only needs release ordering.
        unsafe {
            (*prev).ti_on_cpu.store(false, core::sync::atomic::Ordering::Release);
        }
    }
    crate::arch::smp::set_current_task_ptr(next as u64);
}

/// Set TSS.rsp0 (and only that) to the task's kernel stack top.
/// Called from `__switch_to`; also usable by the boot path.
///
/// # Safety
/// `task` must be a valid Task (or null).
#[no_mangle]
pub unsafe extern "C" fn x86_update_rsp0(task: *mut Task) {
    let top = if task.is_null() {
        0
    } else {
        // SAFETY: get_kernel_stack returns the stack top for live tasks;
        // Option's niche encodes None as 0.
        unsafe { (*task).get_kernel_stack().map_or(0, |p| p as u64) }
    };
    super::trap::set_tss_rsp0(top);
}

// ============================================================================
// Per-CPU prev task (for ret_from_fork's schedule_tail call)
// ============================================================================

static CPU_PREV_TASK: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        CPU_PREV_TASK[cpu].store(prev as u64, core::sync::atomic::Ordering::Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn get_prev_task() -> *mut Task {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        CPU_PREV_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed) as *mut Task
    } else {
        core::ptr::null_mut()
    }
}

// ============================================================================
// High-level context_switch
// ============================================================================

/// Context switch wrapper (same protocol as the riscv64 twin):
///
/// 1. save prev FPU (fxsave into thread)
/// 2. switch_mm via CR3 if the address space changed
/// 3. save/restore per-task FS base (TLS)
/// 4. `__switch_to` (registers, stacks, per-CPU current, TSS.rsp0)
/// 5. restore the incoming task's FPU
///
/// # Safety
/// Must be called with interrupts disabled (caller's responsibility).
/// After `__switch_to` all locals are invalid; the incoming task is
/// resolved through `sched::current()` (the per-CPU slot).
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Step 1: save prev FPU state
    prev.thread_mut().fpu_save_for_switch();

    // Store prev task for ret_from_fork's schedule_tail(prev)
    set_prev_task(prev as *mut Task);

    // Step 2: switch_mm — user PML4s share the kernel half, so a CR3
    // write from anywhere keeps the kernel mapped (unlike the riscv64
    // twin, no linear-map region dance is needed).
    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        if read_cr3() >> 12 != next_ppn {
            // PCID is disabled at bring-up: asid is always 0 and the CR3
            // reload itself flushes non-global translations.
            write_cr3(super::mm::asid::build_satp(0, next_ppn));
        }
    } else {
        // Next is a kernel thread — switch to the kernel root so a
        // dying user mm (dropped by do_exit) can be freed under our feet.
        let kernel_ppn = super::mm::mmu_init::root_page_table_ppn();
        if read_cr3() >> 12 != kernel_ppn {
            write_cr3(super::mm::asid::build_satp(0, kernel_ppn));
        }
    }

    // Step 3: FS base (TLS pointer).  Saved from the live MSR so a task
    // switched out inside a set_thread_area window resumes correctly.
    let fs = rdmsr(MSR_FS_BASE);
    prev.thread_mut().fs_base = fs;
    wrmsr(MSR_FS_BASE, next.thread().fs_base);

    // Step 4: registers + stacks + per-CPU current + TSS.rsp0.
    //
    // WARNING: after this call all locals are INVALID (callee-saved
    // registers and rsp belong to the incoming task).
    __switch_to(prev, next);

    // Step 5: restore the incoming task's FPU (review ARCH-H1 parity):
    // resolved via sched::current() — the per-CPU slot __switch_to just
    // published — never through the stale `next` reference.
    if let Some(cur) = crate::sched::current() {
        // SAFETY: cur is the task now running on this CPU.
        unsafe {
            (*cur).thread_mut().restore_fpu();
        }
    }
}
