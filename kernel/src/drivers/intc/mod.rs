//! 中断控制器驱动
//!
//! 支持 GICv3（ARM64）和 PLIC（RISC-V64）

#[cfg(feature = "aarch64")]
pub mod gicv3;

#[cfg(feature = "riscv64")]
pub mod plic;

// 根据平台导出对应的中断控制器
#[cfg(feature = "aarch64")]
pub use gicv3::*;

#[cfg(feature = "riscv64")]
pub use plic::*;
