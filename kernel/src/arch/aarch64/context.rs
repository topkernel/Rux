//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 context switching.
//!
//! __switch_to asm: callee-saved register swap (x19-x28, sp, lr) with
//! EAGER FPSIMD save/restore inline (see thread.rs for the discipline
//! rationale). The wrapper mirrors the riscv64 backend's steps:
//! pick-state abort check → prev FPU bookkeeping → switch_mm (TTBR0_EL1
//! root + ASID) → __switch_to.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::process::task::Task;

// Field offsets for the asm (thread::ThreadStruct)
use super::thread::{
    THREAD_FPU, THREAD_RA, THREAD_S0, THREAD_SP, THREAD_TP_VALUE, THREAD_USER_FPSIMD,
};


// ============================================================================
// EL0 FPSIMD save/restore (user-state integrity across kernel FP usage)
// ============================================================================

// Kernel FP discipline: compiler-generated code (core/alloc rlibs) may
// use NEON, which would corrupt the USER's live v0-v31 across a trap.
// trap.S calls el0_entry_fpsimd_save at every EL0->EL1 entry and
// el0_exit_fpsimd_restore just before eret; the faithful user image
// lives in task.thread.user_fpsimd. Both helpers use ONLY x16/x17 as
// scratch (their frame slots are already saved at entry / not yet
// loaded at exit) and need no stack.



// ============================================================================
// Per-CPU variable for prev task
// ============================================================================

pub(crate) static CPU_PREV_TASK: [core::sync::atomic::AtomicU64; crate::arch::aarch64::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::arch::aarch64::MAX_CPUS];

#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::arch::aarch64::MAX_CPUS {
        CPU_PREV_TASK[cpu].store(prev as u64, core::sync::atomic::Ordering::Relaxed);
    }
}


// ============================================================================
// switch_mm (TTBR0_EL1 + ASID)
// ============================================================================

/// Switch the user address space root (TTBR0_EL1) with an ASID.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn switch_mm(next_ppn: u64, asid: u16) {
    crate::arch::arch_floor::aarch64::context::switch_mm(next_ppn, asid)
}

/// Get the current composed "satp" (mode/ASID/TTBR0 root PPN)
#[inline]
pub fn get_current_satp() -> u64 {
    let satp = super::mm::pagetable::get_satp();
    satp.bits()
}

// ============================================================================
// High-level context_switch
// ============================================================================

// IRREDUCIBLE F-ROW (F-CS-A1): the register/stack switch itself —
// reference kernel arch/arm64/kernel/entry.S cpu_switch_to; the callee-saved register file + sp swap has
// no safe shape (Rust owns the frame).
extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}
/// Context switch wrapper (see the module doc; steps mirror the riscv64
///
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn context_switch(prev: &mut Task, next: &mut Task) {
    crate::arch::arch_floor::aarch64::context::context_switch(prev, next)
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: aarch64_switch_abort moved to arch::arch_floor::aarch64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::context::aarch64_switch_abort;
// INV-ARCH note: __switch_mm_linear moved to arch::arch_floor::aarch64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::context::__switch_mm_linear;
// INV-ARCH note: get_prev_task moved to arch::arch_floor::aarch64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::context::get_prev_task;
