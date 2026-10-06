//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 context switching.
//!
//! X86-TODO(agent x86-trap): implement. Contract:
//!
//! - `__switch_to(prev, next)` (global_asm, .text.__switch_to): save
//!   rbx, rbp, r12-r15, rsp into prev->thread; load next->thread's;
//!   publish CURRENT_TASKS[cpu] = next (release); set TSS.rsp0 =
//!   next's kernel stack top; ret.
//! - `context_switch(prev, next)`: save prev FPU (fxsave into thread),
//!   switch_mm via CR3 if address space changed (user PML4s share the
//!   kernel half — PML4[256..511] links — so the switch is safe from
//!   anywhere), save/restore per-task FS base (wrfsr/rdfsr), then
//!   `__switch_to`. Do NOT enable IF in the asm (caller owns IRQ state).
//!
//! Layout note: fork builds child thread.sp = stack_top - sizeof(PtRegs)
//! and thread.ret_addr = ret_from_fork, so the first switch into the
//! child "returns" into ret_from_fork which pops the frame to user mode.

use crate::process::task::Task;

extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

/// High-level context switch (same protocol as riscv64).
///
/// # Safety
/// Must be called with interrupts disabled.
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // X86-TODO(agent x86-trap): FPU save + CR3 switch + FS base + __switch_to
    let _ = (prev, next);
    unreachable!("x86_64 context_switch not yet implemented (agent x86-trap)")
}
