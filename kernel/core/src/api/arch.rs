//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ARCH COLD SEAM (arch-cold lane): the cold diagnostic rows the
//! FRAMEWORK's arch trap paths use to reach the arch service
//! crate's forensic/report halves. The hot callers (trap bodies, the
//! tick accounting, the panic printers) keep their mechanism
//! framework-side — no fn pointer sits between a hot site and its
//! callee. Everything routed here was verified cold per the §1a.2 stay
//! rules: every row fires only AFTER rust_main registration (the rows
//! publish there), on paths that are already µs–ms scale or one-shot
//! latched:
//!
//! - `user_segv_forensics` / `gp_regmem_dump` / `gp_mmhistory_dump`:
//!   the fatal user #GP/#PF death report — the process exits
//!   immediately after; the report is one bounded dump per death.
//! - `scribble_foreign_entry`: the cross-stack corruption report — one
//!   raw serial dump, then the CPU parks (the park stays
//!   framework-side at the call site).
//! - `segv_hist_age`: the dying-task exit print's history lookup.
//! - `stime_profile_top` / `utime_ripsample_report`: report formatters
//!   over framework-side sampler statics (the samplers stay on the
//!   tick path; arm-gated).
//! - `remote_backtrace`: the remote-BT requester (deadlock/softlockup
//!   hunters); the IRQ-work capture halves stay framework-side.
//! - `sigill_mapping_forensics`: the riscv64 SIGILL death mapping dump.
//!
//! An unfilled row is a DEGRADED mode, not a misbuild (the process-
//! hooks' panic-on-empty shape is wrong for pure diagnostics): the
//! no-op default means "no report", the kill/exit path itself is
//! framework mechanism and never rides a row. Registration runs from
//! rust_main via arch::register_all — OnceSlot-only publication
//! (no Spinlock, no per-CPU state; the INV-SVCTEST-1 lesson).
//!
//! THE SELF-RECURSION RULE (the S1 lesson, binding): a registered row
//! body must never call the wrapper that dispatches the same row.

#![forbid(unsafe_code)]
use crate::arch::pt_regs::PtRegs;
use crate::process::types::Task;

/// The arch cold diagnostic rows (all cold; see the module doc).
#[derive(Clone, Copy)]
pub struct ArchDiagHooks {
    /// x86_64: cross-stack foreign-entry corruption report (one raw
    /// serial dump; the caller parks the CPU afterwards).
    pub scribble_foreign_entry: fn(cpu: usize, vector: u64, regs: *mut PtRegs, tp: *mut Task, frame: usize),
    /// x86_64: fatal user SIGSEGV death report (registers, failing-
    /// syscall tail, mm-op rings, stack words, VMA attribution).
    pub user_segv_forensics: fn(regs: &PtRegs, fault_addr: u64),
    /// x86_64: #GP register-set memory/PTE dump (page-walk checked).
    pub gp_regmem_dump: fn(regs: &PtRegs),
    /// x86_64: the victim tgid's mm-syscall history at user-#GP time.
    pub gp_mmhistory_dump: fn(regs: &PtRegs),
    /// x86_64: age of a tid's most recent delivered SIGSEGV (+cr2).
    pub segv_hist_age: fn(tid: u32) -> Option<(u64, u64)>,
    /// x86_64: stime histogram snapshot for KILLDUMP.
    pub stime_profile_top: fn(focus: bool) -> alloc::vec::Vec<([u8; 16], u64, u64)>,
    /// x86_64: the 60 s ripsample report (arm-gated inside).
    pub utime_ripsample_report: fn(),
    /// x86_64: request + print a remote CPU's backtrace (hunters).
    pub remote_backtrace: fn(target: usize, why: &str) -> bool,
    /// riscv64: fatal SIGILL mapping dump (VMA table + attribution).
    pub sigill_mapping_forensics: fn(pid: usize, epc: usize),
}

fn noop_scribble_foreign_entry(_: usize, _: u64, _: *mut PtRegs, _: *mut Task, _: usize) {}
fn noop_user_segv_forensics(_: &PtRegs, _: u64) {}
fn noop_gp_regmem_dump(_: &PtRegs) {}
fn noop_gp_mmhistory_dump(_: &PtRegs) {}
fn noop_segv_hist_age(_: u32) -> Option<(u64, u64)> {
    None
}
fn noop_stime_profile_top(_: bool) -> alloc::vec::Vec<([u8; 16], u64, u64)> {
    alloc::vec::Vec::new()
}
fn noop_utime_ripsample_report() {}
fn noop_remote_backtrace(_: usize, _: &str) -> bool {
    false
}
fn noop_sigill_mapping_forensics(_: usize, _: usize) {}

impl ArchDiagHooks {
    /// The degraded default: no reports (pre-registration boots and
    /// unregistered unit-test kernels).
    pub const NOOP: ArchDiagHooks = ArchDiagHooks {
        scribble_foreign_entry: noop_scribble_foreign_entry,
        user_segv_forensics: noop_user_segv_forensics,
        gp_regmem_dump: noop_gp_regmem_dump,
        gp_mmhistory_dump: noop_gp_mmhistory_dump,
        segv_hist_age: noop_segv_hist_age,
        stime_profile_top: noop_stime_profile_top,
        utime_ripsample_report: noop_utime_ripsample_report,
        remote_backtrace: noop_remote_backtrace,
        sigill_mapping_forensics: noop_sigill_mapping_forensics,
    };
}

pub static ARCH_DIAG_HOOKS: crate::safety::slot::OnceSlot<ArchDiagHooks> =
    crate::safety::slot::OnceSlot::new();

/// The dispatch accessor every framework wrapper uses. Unpublished
/// reads the NOOP default (diagnostics degrade to silence; the
/// surrounding kill/exit/park mechanism is framework-side and never
/// rides a row).
pub fn arch_diag() -> ArchDiagHooks {
    *ARCH_DIAG_HOOKS.get().unwrap_or(&ArchDiagHooks::NOOP)
}
