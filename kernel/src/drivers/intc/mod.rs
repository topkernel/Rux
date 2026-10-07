//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Interrupt controller driver
//!
//! Supports GICv3 (ARM64), PLIC (RISC-V64), and CLINT (RISC-V64)

#[cfg(feature = "aarch64")]
pub mod gicv3;

#[cfg(feature = "riscv64")]
pub mod plic;

#[cfg(feature = "riscv64")]
pub mod clint;

#[cfg(feature = "x86_64")]
pub mod apic;

// Export corresponding interrupt controller based on platform
#[cfg(feature = "aarch64")]
pub use gicv3::*;


#[cfg(feature = "aarch64")]
pub fn init() {
    gicv3::init();
}

#[cfg(feature = "riscv64")]
pub fn init() {
    plic::init();
    clint::init();
}

// x86_64: the 8259 PIC is programmed by the arch trap bring-up; the
// local APIC (IPIs, per-CPU timer, SMP) comes up here, with the PIC
// kept alive in virtual-wire mode through LVT LINT0.
#[cfg(feature = "x86_64")]
pub fn init() {
    apic::init();
}
