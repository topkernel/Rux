#[cfg(feature = "aarch64")]
pub mod aarch64;

#[cfg(feature = "x86_64")]
pub mod x86_64;

#[cfg(feature = "riscv64")]
pub mod riscv64;

#[cfg(feature = "aarch64")]
pub use aarch64::*;

#[cfg(feature = "x86_64")]
pub use x86_64::*;

#[cfg(feature = "riscv64")]
pub use riscv64::*;
