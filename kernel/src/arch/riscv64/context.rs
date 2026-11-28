//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V 64-bit context switching
//!
//! Context switch implementation:
//! - switch_mm(): Switch page table (write satp) - called FIRST
//! - __switch_to(): Switch registers (ra, sp, s0-s11) - called SECOND
//!
//! Reference: kernel/sched/core.c context_switch()
//!
//! # Safety Invariants — Context Switch Atomicity
//!
//! - **INV-CS-1**: `__switch_to` saves and restores all callee-saved registers
//!   (ra, sp, s0–s11). The caller-saved registers are already saved by the
//!   compiler in the caller's stack frame before `schedule()` is invoked.
//!
//! - **INV-CS-2**: FPU state (if `Fs != Off`) is saved/restored via
//!   `fpu_save_for_switch` / `__fstate_restore` around the register switch.
//!
//! - **INV-CS-3**: The MMU (satp) is switched via `switch_mm()` **before**
//!   `__switch_to` runs, so the new task's page table is active when its
//!   registers are restored.
//!
//! - **INV-CS-4**: `__switch_to` does NOT enable SIE (interrupts). The caller
//!   (`__schedule`) manages SIE via `lock_irqsave()` / `restore_irq()`. Each
//!   task has its own saved IRQ state in `__schedule()`'s stack frame, and
//!   after context_switch the new task restores its own IRQ state. This matches
//!   the reference implementation where `__switch_to` only restores SUM, never SIE.
//!
//! - **INV-CS-5**: After `__switch_to`, the previous task's stack pointer
//!   (`prev_sp`) is stale; the only valid way to retrieve the current task
//!   is via the `tp` (thread pointer) register, as set by `__switch_to`.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::process::task::Task;
use crate::process::Task as ProcessTask;
use core::arch::asm;
use super::mm::PageTable;
use super::mm::PageTableEntry;

/// sstatus.SUM bit mask
pub const SR_SUM: u64 = 1 << 18;
/// Get current task pointer from tp register
///
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-18): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn current_task() -> &'static mut Task {
    crate::arch::arch_floor::riscv64::context::current_task()
}

// ============================================================================
// __switch_to
// ============================================================================


// R65-timer: __switch_to releases the ownership qword with a single
// aligned 8-byte store of 0xFFFFFFFF_00000000. Pin the two layout facts
// that store assumes at compile time — ti_on_cpu is 8-byte aligned (the
// `sd` must be naturally aligned) and running_on_cpu sits at +4 inside
// the same qword — so a future Task field reorder fails the build here
// instead of silently corrupting a neighboring field.
pub(crate) const _: () = {
    assert!(core::mem::offset_of!(Task, ti_on_cpu) % 8 == 0);
    assert!(
        core::mem::offset_of!(Task, running_on_cpu)
            == core::mem::offset_of!(Task, ti_on_cpu) + 4
    );
};


// ============================================================================
// Per-CPU variable for prev task
// ============================================================================

pub(crate) static CPU_PREV_TASK: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        CPU_PREV_TASK[cpu].store(prev as u64, core::sync::atomic::Ordering::Relaxed);
    }
}


// ============================================================================
// switch_mm
// ============================================================================

/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-13): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn switch_mm(next_ppn: u64, asid: u16) {
    crate::arch::arch_floor::riscv64::context::switch_mm(next_ppn, asid)
}
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_current_satp() -> u64 {
    crate::arch::arch_floor::riscv64::context::get_current_satp()
}

// ============================================================================
// High-level context_switch
// ============================================================================

// IRREDUCIBLE F-ROW (F-CS-R1): the register/stack switch itself —
// reference kernel arch/riscv/kernel/entry.S __switch_to; the callee-saved register file + sp swap has
// no safe shape (Rust owns the frame).
extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}
/// Context switch wrapper function
///
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn context_switch(prev: &mut Task, next: &mut Task) {


                                                    // Thin contract wrapper (IRQs-off + locals-invalid-after-switch stay
                                                    // the caller-visible protocol); the body below is safe Rust over the
                                                    // audited switch edges.
                                                    context_switch_body(prev, next)

}
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn context_switch_body(prev: &mut Task, next: &mut Task) {
    crate::arch::arch_floor::riscv64::context::context_switch_body(prev, next)
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: riscv_switch_abort moved to arch::arch_floor::riscv64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::context::riscv_switch_abort;
// INV-ARCH note: __switch_mm_linear moved to arch::arch_floor::riscv64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::context::__switch_mm_linear;
// INV-ARCH note: get_prev_task moved to arch::arch_floor::riscv64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::context::get_prev_task;
// INV-ARCH note: rux_switchin_bad_ra moved to arch::arch_floor::riscv64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::context::rux_switchin_bad_ra;
