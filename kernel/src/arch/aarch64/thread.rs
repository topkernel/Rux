//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 ThreadStruct — per-task architecture state.
//!
//! Layout mirrors the riscv64 backend's ThreadStruct (the generic
//! scheduler touches `.sp`/`.ra`/`.s` through cfg-gated helpers): the
//! callee-saved register block (x19-x28, sp, lr), FPSIMD state (32×128-bit
//! Q registers + FPCR/FPSR), and TLS.
//!
//! FPSIMD discipline: unlike riscv64 (lazy FS-bit trap model), the
//! aarch64 __switch_to asm EAGERLY saves/restores the FPSIMD state on
//! every switch. Cortex-a72 has no lazy-FP trap class worth wiring
//! (CPACR_FPEN traps exist but complicate EL0/EL1 distinction); the eager
//! image is 528 bytes of stp/ldp — correctness first (the riscv64 ARCH-H1
//! "save but never restore" lesson), a lazy refinement can come later
//! with measurements.

/// FPSIMD register image size: 32 Q registers × 16 bytes = 64 × u64
const FPU_STATE_SIZE: usize = 64;

/// Thread structure - stores architecture-specific thread state
#[repr(C)]
#[derive(Debug, Clone)]
pub struct ThreadStruct {
    // ==================== Context switch fields ====================

    /// Callee-saved return address (lr / x30)
    pub ra: u64,

    /// Callee-saved stack pointer (sp)
    pub sp: u64,

    /// Callee-saved registers x19-x28 (10 registers; the array keeps the
    /// riscv64 field name `s` for the generic cfg helpers)
    pub s: [u64; 12],

    // ==================== FPSIMD state ====================

    /// FPSIMD registers v0-v31 (32 × 128-bit)
    pub fpu: [u64; FPU_STATE_SIZE],

    /// FPSIMD control register (FPCR)
    pub fpcr: u32,

    /// FPSIMD status register (FPSR)
    pub fpsr: u32,

    /// Saved FPU image validity (eager model: 1 = image live)
    pub fs: u32,

    // ==================== Other state ====================

    /// Thread local storage (TLS) pointer
    pub tp_value: u64,

    /// Current exception frame pointer (for signal handling)
    pub exception_sp: u64,

    /// Debug flag
    pub debug_flag: bool,
}

/// Callee-saved register block (diagnostic parity with the x86_64
/// backend; dfx probes read `ret_addr`)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct CalleeSaved {
    pub ret_addr: u64,
    pub callee: [u64; 10],
}

impl ThreadStruct {
    /// Create new thread structure
    pub const fn new() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],

            fpu: [0; FPU_STATE_SIZE],
            fpcr: 0,
            fpsr: 0,
            // INITIAL: fresh tasks restore the zeroed FPSIMD image on
            // their first switch-in (riscv64 ARCH-H1 lesson parity)
            fs: 1,
            tp_value: 0,
            exception_sp: 0,
            debug_flag: false,
        }
    }

    /// Get TLS pointer
    #[inline]
    pub fn tp(&self) -> u64 {
        self.tp_value
    }

    /// Set TLS pointer
    #[inline]
    pub fn set_tp(&mut self, tp: u64) {
        self.tp_value = tp;
    }

    /// Mark the saved FPU image as a clean, valid state (fork/exec parity)
    #[inline]
    pub fn mark_fpu_clean(&mut self) {
        self.fs = 2; // CLEAN
    }

    /// Get saved FPU status
    #[inline]
    pub fn get_fs(&self) -> u32 {
        self.fs
    }

    /// Save the live FPSIMD state into this thread image (called by
    /// __switch_to asm via the field offsets; the Rust twin for stage-2
    /// FPU fault paths)
    // SAFETY: writes 512 bytes into self.fpu via volatile register moves.
    #[inline]
    pub unsafe fn fpu_save(&mut self) {
        unsafe {
            let dst = self.fpu.as_mut_ptr();
            core::arch::asm!(
                "stp q0, q1, [{d}], #32",
                "stp q2, q3, [{d}], #32",
                "stp q4, q5, [{d}], #32",
                "stp q6, q7, [{d}], #32",
                "stp q8, q9, [{d}], #32",
                "stp q10, q11, [{d}], #32",
                "stp q12, q13, [{d}], #32",
                "stp q14, q15, [{d}], #32",
                "stp q16, q17, [{d}], #32",
                "stp q18, q19, [{d}], #32",
                "stp q20, q21, [{d}], #32",
                "stp q22, q23, [{d}], #32",
                "stp q24, q25, [{d}], #32",
                "stp q26, q27, [{d}], #32",
                "stp q28, q29, [{d}], #32",
                "stp q30, q31, [{d}]",
                "mrs {fpcr}, fpcr",
                "mrs {fpsr}, fpsr",
                d = in(reg) dst,
                fpcr = out(reg) self.fpcr,
                fpsr = out(reg) self.fpsr,
                options(nostack)
            );
        }
        self.fs = 3; // DIRTY
    }

    /// Restore the FPSIMD state from this thread image
    // SAFETY: reads 512 bytes from self.fpu into live FP registers.
    #[inline]
    pub unsafe fn fpu_restore(&self) {
        unsafe {
            let src = self.fpu.as_ptr();
            let fpcr = self.fpcr as u64;
            let fpsr = self.fpsr as u64;
            core::arch::asm!(
                "ldp q0, q1, [{s}], #32",
                "ldp q2, q3, [{s}], #32",
                "ldp q4, q5, [{s}], #32",
                "ldp q6, q7, [{s}], #32",
                "ldp q8, q9, [{s}], #32",
                "ldp q10, q11, [{s}], #32",
                "ldp q12, q13, [{s}], #32",
                "ldp q14, q15, [{s}], #32",
                "ldp q16, q17, [{s}], #32",
                "ldp q18, q19, [{s}], #32",
                "ldp q20, q21, [{s}], #32",
                "ldp q22, q23, [{s}], #32",
                "ldp q24, q25, [{s}], #32",
                "ldp q26, q27, [{s}], #32",
                "ldp q28, q29, [{s}], #32",
                "ldp q30, q31, [{s}]",
                "msr fpcr, {fpcr}",
                "msr fpsr, {fpsr}",
                s = in(reg) src,
                fpcr = in(reg) fpcr,
                fpsr = in(reg) fpsr,
                options(nostack, readonly)
            );
        }
    }

    /// Interface parity with the riscv64 backend (no vector state on
    /// cortex-a72 — SVE arrives with a future CPU model, if ever)
    #[inline]
    pub fn mark_vstate_clean(&mut self) {}
}

impl Default for ThreadStruct {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== Field offsets (assembly use) ====================

/// Field offsets in ThreadStruct
#[allow(dead_code)]
pub mod thread_offsets {
    use super::*;

    pub const THREAD_RA: usize = core::mem::offset_of!(ThreadStruct, ra);
    pub const THREAD_SP: usize = core::mem::offset_of!(ThreadStruct, sp);
    pub const THREAD_S0: usize = core::mem::offset_of!(ThreadStruct, s);
    pub const THREAD_FPU: usize = core::mem::offset_of!(ThreadStruct, fpu);
    pub const THREAD_FPCR: usize = core::mem::offset_of!(ThreadStruct, fpcr);
    pub const THREAD_FPSR: usize = core::mem::offset_of!(ThreadStruct, fpsr);
    pub const THREAD_TP_VALUE: usize = core::mem::offset_of!(ThreadStruct, tp_value);
}

/// Export offset constants for assembly use
#[allow(dead_code)]
pub use thread_offsets::*;
