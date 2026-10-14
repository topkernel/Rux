//! aarch64 backend — bring-up skeleton.
//!
//! Status: interface scaffolding only. Nothing here boots. The bring-up
//! order and the x86_64→aarch64 porting map live in
//! docs/development/multiarch-gnome-ubuntu-plan.md.
//!
//! Target profile: QEMU `virt` machine, Cortex-A72, GICv3, virtio-mmio,
//! 4-level translation tables (48-bit VA), 4K granule.

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

pub fn arch_init() {
    init();
}

pub fn init() {
    // Bring-up order (mirrors the x86_64 backend):
    //   1. trap vectors (VBAR_EL1) + exception routing
    //   2. MMU on (identity → kernel map switch), kernel linear map
    //   3. IRQ chip (GICv3 dist/redistributor/CPU interface)
    //   4. timer (CNTFRQ/CNTP + GTIMER)
    //   5. SMP (PSCI CPU_ON via SMC/HVC, per-CPU areas)
    unimplemented!("aarch64 backend bring-up")
}

pub fn enable_interrupts() {
    // msr daifclr, #0xf — clear DAIF mask bits
    unimplemented!("aarch64 backend bring-up")
}

/// Current CPU number (MPIDR-based per-CPU index).
pub fn cpu_id() -> u64 {
    unimplemented!("aarch64 backend bring-up")
}
