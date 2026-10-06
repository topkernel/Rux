//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 architecture support (QEMU q35, multiboot1 boot protocol)

pub mod boot;
pub mod pt_regs;
pub mod trap;
pub mod context;
pub mod cpu;
pub mod mm;
pub mod smp;
pub mod ipi;
pub mod process;
pub mod thread;
pub mod uaccess;

use crate::println;

pub fn arch_init() {
    init();
}

pub fn init() {
    println!("arch: Initializing x86_64 architecture...");

    // Set up the IDT, TSS, GDT (user + TSS entries) and the syscall MSRs.
    trap::init();
    trap::init_syscall();

    println!("arch: Architecture initialization [DONE]");
}

pub fn enable_interrupts() {
    // SAFETY: setting IF via STI is safe in any context that intends to
    // enable interrupts.
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    println!("arch: Interrupts enabled (STI)");
}

/// Get current CPU number.
///
/// Bring-up uses a single CPU: the boot CPU is always CPU 0. Secondary
/// CPUs (SIPI) will publish their number through the per-CPU array
/// before entering Rust (X86-TODO: SMP).
pub fn cpu_id() -> u64 {
    0
}
