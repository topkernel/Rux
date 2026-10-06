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

// x86_64: the 8259 PIC lives in the arch trap/IRQ bring-up path
// (X86-TODO agent x86-trap); no intc driver to initialize here yet.
#[cfg(feature = "x86_64")]
pub fn init() {}
