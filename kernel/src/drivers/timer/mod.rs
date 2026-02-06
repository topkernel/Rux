//! 定时器驱动

#[cfg(feature = "aarch64")]
pub mod armv8;
#[cfg(feature = "aarch64")]
pub use armv8::*;

#[cfg(feature = "riscv64")]
pub mod riscv64;
#[cfg(feature = "riscv64")]
pub use riscv64::*;
