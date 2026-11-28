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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::process::task::Task;
use super::cpu::{read_cr3, rdmsr, wrmsr, write_cr3};

/// MSR_FS_BASE — per-task TLS pointer.
pub(crate) const MSR_FS_BASE: u32 = 0xC000_0100;
/// MSR_KERNEL_GS_BASE — the SWAPGS shadow (task's user GS base).
pub(crate) const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

// IRREDUCIBLE F-ROW (F-CS-X1): the register/stack switch itself —
// reference kernel arch/x86/kernel/entry.S __switch_to (callee-saved save/restore); the callee-saved register file + sp swap has
// no safe shape (Rust owns the frame).
extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

// R65-timer twin (riscv64 context.rs pins the same facts): __switch_to
// releases the ownership qword with ONE aligned 8-byte store of
// 0xffffffff00000000 (ti_on_cpu=false at byte 0, running_on_cpu=-1 at
// +4). Pin the two layout facts that store assumes at compile time, so
// a future Task field reorder fails the build instead of silently
// clobbering a neighboring field.
pub(crate) const _: () = {
    assert!(core::mem::offset_of!(Task, ti_on_cpu) % 8 == 0);
    assert!(
        core::mem::offset_of!(Task, running_on_cpu)
            == core::mem::offset_of!(Task, ti_on_cpu) + 4
    );
};
/// Switch the address-space root (interface parity with the riscv64
///
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-26): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn switch_mm(next_ppn: u64, asid: u16) {


                                       // SAFETY: caller guarantees a valid PML4 physical address; the
                                       // tagging/fallback discipline lives in asid::switch_root.
                                        crate::arch::x86_64::mm::asid::switch_root(next_ppn, asid) ;

}

// ============================================================================
// __switch_to
// ============================================================================




// ============================================================================
// Per-CPU prev task (for ret_from_fork's schedule_tail call)
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
// High-level context_switch
// ============================================================================
/// Context switch wrapper (same protocol as the riscv64 twin):
///
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-21): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn context_switch(prev: &mut Task, next: &mut Task) {


                                                    // Thin contract wrapper (IRQs-off + locals-invalid-after-switch stay
                                                    // the caller-visible protocol); the body below is safe Rust over the
                                                    // audited switch edges.
                                                    context_switch_body(prev, next)

}
/// Collected into `arch::arch_floor` (INV-ARCH-CONTEXT-20): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn context_switch_body(prev: &mut Task, next: &mut Task) {
    crate::arch::arch_floor::x86_64::context::context_switch_body(prev, next)
}

/// This CPU's index for the per-CPU cctx switch stamp (the context.rs
/// wrapper does not carry cpu_id through __switch_to).
#[inline]
pub(crate) fn cpu_of() -> usize {
    crate::arch::cpu_id() as usize
}

// ============================================================================
// Raw-uart bounded reporters (the publish/abort guards' printers)
//
// The switch-path guards print through putchar_no_lock (no locks, no
// allocation — the switch may run with IRQs off on a foreign stack).
// Every one of them used to carry its own nibble loop; this is the one
// shared pair (context-publish discipline, unsafe-slim round 5 lane L3).
// ============================================================================

/// One line fragment: bytes straight to the raw UART.
#[inline]
pub(crate) fn putfrag_no_lock(msg: &[u8]) {
    use crate::console::putchar_no_lock as putchar;
    for &b in msg {
        putchar(b);
    }
}

/// `v` as hex (no 0x, fixed 16 nibbles) straight to the raw UART.
#[inline]
pub(crate) fn puthex_no_lock(v: u64) {
    use crate::console::putchar_no_lock as putchar;
    let mut sh: i32 = 64;
    while sh > 0 {
        sh -= 4;
        let nb = ((v >> sh) & 0xF) as u8;
        putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
    }
}

/// `v` as hex, fixed 8 nibbles.
#[inline]
pub(crate) fn puthex32_no_lock(v: u64) {
    use crate::console::putchar_no_lock as putchar;
    let mut v = v;
    for _ in 0..8 {
        // One NIBBLE per iteration — mask to 4 bits (the r41v4 overflow
        // class: any byte >= 0xA9 overflow-panicked the 'a'+n-10 arm from
        // inside the context-switch path).
        let n = ((v >> 28) as u8) & 0xF;
        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        v <<= 4;
    }
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: x86_switch_abort moved to arch::arch_floor::x86_64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::context::x86_switch_abort;
// INV-ARCH note: get_prev_task moved to arch::arch_floor::x86_64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::context::get_prev_task;
// INV-ARCH note: x86_update_rsp0 moved to arch::arch_floor::x86_64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::context::x86_update_rsp0;
// INV-ARCH note: x86_switch_publish moved to arch::arch_floor::x86_64::context (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::context::x86_switch_publish;
