//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Architecture-specific code
//!
//! # Architecture interface layer
//!
//! Code outside `arch/` must depend only on `crate::arch::<item>` — never on
//! a concrete backend (`crate::arch::...`). The backend is selected
//! by cargo feature; this module re-exports the backend's module tree so the
//! interface is a path-compatible set of items:
//!
//! - `arch::{uaccess, mm, cpu, pt_regs, smp, thread, trap, boot, ipi,
//!   context, process}` — per-arch implementations of the same surface
//! - `arch::{arch_init, init, enable_interrupts, cpu_id, context_switch}` —
//!   top-level entry points
//!
//! A new backend implements the same module tree with matching signatures;
//! adding a symbol to the interface requires both backends to provide it.

// ---------------------------------------------------------------------------
// Backends
// ---------------------------------------------------------------------------

#[cfg(feature = "riscv64")]
pub mod riscv64;

#[cfg(feature = "x86_64")]
pub mod x86_64;

// ---------------------------------------------------------------------------
// Interface: re-export the active backend's surface
// ---------------------------------------------------------------------------

#[cfg(feature = "riscv64")]
pub use riscv64::{
    arch_init, cpu_id, enable_interrupts, init, boot, context, cpu, ipi, mm, process, pt_regs,
    smp, thread, trap, uaccess,
};

#[cfg(feature = "x86_64")]
pub use x86_64::{
    arch_init, cpu_id, enable_interrupts, init, boot, context, cpu, ipi, mm, process, pt_regs,
    smp, thread, trap, uaccess,
};

/// Direct switch of context (see backend `context` module for the exact
/// semantics; both backends expose `context_switch` with the same signature).
#[cfg(feature = "riscv64")]
pub use riscv64::context::context_switch;

#[cfg(feature = "x86_64")]
pub use x86_64::context::context_switch;
