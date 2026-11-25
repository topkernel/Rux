//! The exec stack-zero ring, kept FRAMEWORK-side when process/exec.rs
//! flipped to the process service crate: the riscv64 page-fault
//! path (arch/riscv64/mm/exception.rs) walks STACKZERO_RING on the
//! fault path — a hot framework read that cannot ride a hook row.
//! The crate's exec policy WRITES it through this module (downward,
//! pub statics); the fault handler reads it directly.

#![forbid(unsafe_code)]
// FORENSIC: initial-stack zeroing ledger (see the zeroing site below).
// ZeroEntry — the definitions live in process/types.rs; every impl and
// function that uses them stays in this file. Re-exported so the
// existing `process::exec_ring::…` paths are unchanged.
pub use super::types::{ZeroEntry};

impl ZeroEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            base: core::sync::atomic::AtomicU64::new(0),
            len: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const ZERO_NEW: ZeroEntry = ZeroEntry::new();
pub static STACKZERO_RING: [ZeroEntry; 128] = [ZERO_NEW; 128];
pub static STACKZERO_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
