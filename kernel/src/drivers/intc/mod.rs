//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 中断控制器驱动
//!
//! 支持 GICv3（ARM64）、PLIC（RISC-V64）和 CLINT（RISC-V64）

#[cfg(feature = "aarch64")]
pub mod gicv3;

#[cfg(feature = "riscv64")]
pub mod plic;

#[cfg(feature = "riscv64")]
pub mod clint;

// 根据平台导出对应的中断控制器
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
