//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 IPIs. Single-CPU bring-up: shootdowns are local-only; SMP
//! (LAPIC IPI) lands with the SIPI phase.

/// Initialize IPI support
pub fn init() {}

/// Ask other CPUs to flush TLB entries. X86-TODO(SMP): LAPIC
/// broadcast + tlb_gen handshake; single-CPU now (local flush already
/// happened at the call sites).
pub fn flush_tlb_others(_start: u64, _end: u64) {}

/// Run a function on all (other) CPUs. X86-TODO(SMP).
pub fn smp_call_function<F: Fn() + Send + Sync + 'static>(_func: F) {
    // single CPU: nothing to broadcast to
}
