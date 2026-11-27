//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit architecture support
//!
//! Supports RISC-V 64-bit (RV64GC) architecture


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

pub use crate::arch::arch_floor::riscv64 as unsafe_edges;
pub mod boot;
pub mod pt_regs;
pub mod trap;
pub mod context;
pub mod cpu;
// syscall module has moved to kernel/src/syscall/
pub mod mm;
pub mod smp;
pub mod ipi;
pub mod process;
pub mod thread;
pub mod vext;
pub mod uaccess;

use crate::println;
use core::arch::asm;



pub fn arch_init() {
    init();
}
/// Collected into `arch::arch_floor` (INV-ARCH-MOD-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init() {
    crate::arch::arch_floor::riscv64::root::init()
}

pub(crate) fn print_cpu_info() {
    // mhartid/mimpid/marchid are M-mode CSRs and trap when read from S-mode.
    // There is no reliable S-mode view of the implementation IDs (Linux gets
    // them from the device tree), so report the boot hart only.
    println!("arch: boot hart = {}", cpu_id());
}
/// Collected into `arch::arch_floor` (INV-ARCH-MOD-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_interrupts() {
    crate::arch::arch_floor::riscv64::root::enable_interrupts()
}

/// Get current CPU (hart) ID
///
/// Design:
/// - Early boot phase: tp = hart_id (small value)
/// - After scheduler runs: tp = task_struct pointer, hart_id stored in task_struct.ti_cpu
///
/// Determine current mode by checking tp value range:
/// - If tp < 0x1000, consider it as hart_id (early boot)
/// - Otherwise consider it as task_struct pointer
///
/// In S-mode, we cannot access mhartid CSR (only accessible from M-mode).
// IRREDUCIBLE F-ROW (F-CSR-R2): the mod-level cpu_id/init CSR register
// reads — reference arch/riscv's csr accesses; no safe CSR shape.
/// Collected into `arch::arch_floor` (INV-ARCH-MOD-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn cpu_id() -> u64 {
    crate::arch::arch_floor::riscv64::root::cpu_id()
}
