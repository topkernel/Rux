//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 架构相关代码
//!
//! 当前支持的架构：
//! - **RISC-V (riscv64)** - 主要支持的平台，默认启用
//!
//! 暂不支持的架构：
//! - aarch64 (ARM64) - 已移除，暂不维护
//! - x86_64 - 未实现

// RISC-V 架构（当前默认且唯一支持的架构）
#[cfg(feature = "riscv64")]
pub mod riscv64;

// 导出 trap 模块
#[cfg(feature = "riscv64")]
pub use riscv64::trap;

// 导出 smp 模块
#[cfg(feature = "riscv64")]
pub use riscv64::smp;

// 导出 ipi 模块
#[cfg(feature = "riscv64")]
pub use riscv64::ipi;

// 导出 cpu_id 函数
#[cfg(feature = "riscv64")]
pub use riscv64::smp::cpu_id;

// 导出 context 模块
#[cfg(feature = "riscv64")]
pub use riscv64::context::{self, context_switch};

// 导出 syscall 模块
#[cfg(feature = "riscv64")]
pub use riscv64::syscall;

// 导出 mm 模块
#[cfg(feature = "riscv64")]
pub use riscv64::mm;
