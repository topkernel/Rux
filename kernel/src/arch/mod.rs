//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
#[cfg(feature = "aarch64")]
pub mod aarch64;

#[cfg(feature = "x86_64")]
pub mod x86_64;

#[cfg(feature = "riscv64")]
pub mod riscv64;

// 导出各架构的 trap 模块（用于 main.rs 的 arch::trap::init()）
#[cfg(feature = "aarch64")]
pub use aarch64::trap;

#[cfg(feature = "x86_64")]
pub use x86_64::trap;

#[cfg(feature = "riscv64")]
pub use riscv64::trap;

// 导出各架构的 smp 模块（用于 main.rs 的 arch::smp::init()）
#[cfg(feature = "riscv64")]
pub use riscv64::smp;

// 导出各架构的 ipi 模块（用于 trap.rs 的 crate::arch::ipi::handle_ipi()）
#[cfg(feature = "riscv64")]
pub use riscv64::ipi;

// 导出各架构的 cpu_id 函数（用于 sched.rs 的 crate::arch::cpu_id()）
#[cfg(feature = "aarch64")]
pub use aarch64::cpu::cpu_id;

#[cfg(feature = "riscv64")]
pub use smp::cpu_id;

// 导出各架构的 context 模块
#[cfg(feature = "aarch64")]
pub use aarch64::context;

#[cfg(feature = "x86_64")]
pub use x86_64::context;

#[cfg(feature = "riscv64")]
pub use riscv64::context::{self, context_switch};

// 导出各架构的 syscall 模块
#[cfg(feature = "aarch64")]
pub use aarch64::syscall;

#[cfg(feature = "x86_64")]
pub use x86_64::syscall;

#[cfg(feature = "riscv64")]
pub use riscv64::syscall;

// 导出各架构的 mm 模块
#[cfg(feature = "aarch64")]
pub use aarch64::mm;

#[cfg(feature = "x86_64")]
pub use x86_64::mm;

#[cfg(feature = "riscv64")]
pub use riscv64::mm;
