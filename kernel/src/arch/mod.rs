//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Architecture-specific code
//!
//! Currently supported architectures:
//! - **RISC-V (riscv64)** - Primary supported platform, enabled by default
//!
//! Unsupported architectures:
//! - aarch64 (ARM64) - Removed, not maintained
//! - x86_64 - Not implemented

// RISC-V architecture (currently the default and only supported architecture)
#[cfg(feature = "riscv64")]
pub mod riscv64;

// Export trap module
#[cfg(feature = "riscv64")]
pub use riscv64::trap;

// Export smp module
#[cfg(feature = "riscv64")]
pub use riscv64::smp;

// Export ipi module
#[cfg(feature = "riscv64")]
pub use riscv64::ipi;

// Export cpu_id function
#[cfg(feature = "riscv64")]
pub use riscv64::smp::cpu_id;

// Export context module
#[cfg(feature = "riscv64")]
pub use riscv64::context::{self, context_switch};

// syscall module has moved to kernel/src/syscall/

// Export mm module
#[cfg(feature = "riscv64")]
pub use riscv64::mm;
