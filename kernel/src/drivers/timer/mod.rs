//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Timer driver

#[cfg(feature = "riscv64")]
pub mod riscv64;
#[cfg(feature = "riscv64")]
pub use riscv64::*;

#[cfg(feature = "x86_64")]
pub mod x86_64;
#[cfg(feature = "x86_64")]
pub use x86_64::*;
