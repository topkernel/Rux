//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! aarch64 backend — QEMU `virt` bring-up.
//!
//! Target profile: Cortex-A72, GICv3, PL011, virtio-mmio, 4-level
//! translation tables (48-bit VA), 4K granule, architected timer.
//! The bring-up plan and stage gates live in
//! docs/development/aarch64-port-plan.md (living spine).

pub mod boot;
pub mod context;
pub mod cpu;
pub mod ipi;
pub mod mm;
pub mod process;
pub mod pt_regs;
pub mod smp;
pub mod thread;
pub mod trap;
pub mod uaccess;

use crate::println;

pub fn arch_init() {
    init();
}

pub fn init() {
    println!("arch: Initializing AArch64 architecture...");

    // Bring-up order (mirrors the other backends):
    //   1. trap vectors (VBAR_EL1) + exception routing
    //   2. MMU on (boot.S early tables → full kernel map in mm::init)
    //   3. IRQ chip (GICv3 — stage 3)
    //   4. timer (CNTVCTL arming — stage 2/3 tick bring-up)
    //   5. SMP (PSCI CPU_ON — stage 3)
    trap::init();

    // Disable interrupts (DAIF mask bits set)
    crate::arch::cpu::disable_irq();
    println!("arch: Interrupts masked (DAIF)");

    println!("arch: boot cpu = {}", cpu_id());

    println!("arch: Architecture initialization [DONE]");
}

pub fn enable_interrupts() {
    crate::arch::cpu::enable_irq();
    println!("arch: EL1 interrupts enabled");
}

/// Current CPU index (MPIDR-based per-CPU index via the TPIDR_EL1
/// protocol — see smp::cpu_id)
pub fn cpu_id() -> u64 {
    smp::cpu_id() as u64
}
