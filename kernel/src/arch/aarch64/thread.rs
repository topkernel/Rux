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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

/// FPSIMD register image size: 32 Q registers × 16 bytes = 64 × u64
pub(crate) const FPU_STATE_SIZE: usize = 64;

/// User-visible FPSIMD image (trap-entry capture; see ThreadStruct docs)
#[derive(Debug, Clone, Copy)]
pub struct UserFpsimdImage {
    /// v0..v31 (32 × 128-bit)
    pub vregs: [u64; FPU_STATE_SIZE],
    /// FPSR
    pub fpsr: u32,
    /// FPCR
    pub fpcr: u32,
}

impl Default for UserFpsimdImage {
    fn default() -> Self {
        Self { vregs: [0; FPU_STATE_SIZE], fpsr: 0, fpcr: 0 }
    }
}

impl UserFpsimdImage {
    /// const-context constructor (ThreadStruct::new is a const fn)
    pub const fn zeroed() -> Self {
        Self { vregs: [0; FPU_STATE_SIZE], fpsr: 0, fpcr: 0 }
    }
}

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

    /// FPSIMD registers v0-v31 (32 × 128-bit) — the LIVE image the
    /// eager __switch_to saves/restores at context-switch time. Kernel
    /// code may clobber FP registers mid-syscall (compiler-generated
    /// NEON in core/alloc rlibs), so this image is kernel-side state,
    /// NOT a faithful copy of the user's FP registers.
    pub fpu: [u64; FPU_STATE_SIZE],

    /// FPSIMD control register (FPCR)
    pub fpcr: u32,

    /// FPSIMD status register (FPSR)
    pub fpsr: u32,

    /// Saved FPU image validity (eager model: 1 = image live)
    pub fs: u32,

    /// USER FPSIMD image: v0-v31 + fpcr/fpsr captured at EL0->EL1 trap
    /// entry (trap.S -> el0_entry_fpsimd_save) and re-installed at
    /// eret (el0_exit_fpsimd_restore). This is the ONLY faithful copy
    /// of the user's FP state: kernel FP usage between trap entry and
    /// exit (NEON memcpy/fmt in core/alloc) is thereby made harmless —
    /// the alternative (general-regs-only codegen for the whole kernel)
    /// needs build-std + compiler-rt sources on every build host. The
    /// signal frame's fpsimd_context record is built FROM this buffer,
    /// and rt_sigreturn writes its restore INTO it.
    pub user_fpsimd: UserFpsimdImage,

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
            user_fpsimd: UserFpsimdImage::zeroed(),
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
    pub const THREAD_USER_FPSIMD: usize = core::mem::offset_of!(ThreadStruct, user_fpsimd);
    /// vregs base inside UserFpsimdImage (offset from ThreadStruct)
    pub const THREAD_USER_FPSIMD_VREGS: usize = THREAD_USER_FPSIMD;
    /// fpsr offset inside UserFpsimdImage (offset from ThreadStruct)
    pub const THREAD_USER_FPSIMD_FPSR: usize =
        THREAD_USER_FPSIMD + core::mem::offset_of!(UserFpsimdImage, fpsr);
    /// fpcr offset inside UserFpsimdImage (offset from ThreadStruct)
    pub const THREAD_USER_FPSIMD_FPCR: usize =
        THREAD_USER_FPSIMD + core::mem::offset_of!(UserFpsimdImage, fpcr);
    pub const THREAD_TP_VALUE: usize = core::mem::offset_of!(ThreadStruct, tp_value);
}

/// Export offset constants for assembly use
#[allow(dead_code)]
pub use thread_offsets::*;
