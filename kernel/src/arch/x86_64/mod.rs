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

    // Default-deny for direct kernel-side user-pointer dereferences:
    // user memory is reachable ONLY through the uaccess primitives,
    // matching the riscv64 SUM-clear semantics.
    //
    // SMAP ARMING IS NOT ATTEMPTED (sec-r1, 2026-11-02): actually
    // setting CR4.SMAP at any pre-IDT stage triple-faults the machine
    // on the first kernel data access after the store — the kernel
    // dereferences at least one U/S-mapped page in early boot. The old
    // CPUID bug (G1: EDX[20] read instead of EBX[20]) meant the arm
    // path never executed anywhere, which is why this latent fault was
    // invisible. cpu::smap_report() now prints the REAL armed state
    // (CPUID capability + live CR4) on every boot so the gap cannot
    // pass silently again; root-causing the triple fault is filed as
    // its own work item (run/sec-r1-notes.md).

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
/// Read from the GS-based per-CPU area (kernel GS base = &PerCpu[cpu],
/// established by smp::init before any code that can call this; APs get
/// theirs from the trampoline before executing any Rust).
pub fn cpu_id() -> u64 {
    smp::gs_cpu_id()
}
