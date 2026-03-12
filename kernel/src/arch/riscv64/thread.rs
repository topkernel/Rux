//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V thread structure (thread_struct)
//!
//! Stores architecture-specific thread state:
//! - Callee-saved registers (ra, sp, s0-s11)
//! - sstatus.SUM bit
//! - FPU state
//! - Vector extension state
//! - Debug registers
//! - Other architecture-specific state

use core::arch::asm;

/// FPU state size (32 64-bit registers)
const FPU_STATE_SIZE: usize = 32;

/// sstatus.SUM bit mask
pub const SR_SUM: u64 = 1 << 18;

/// Thread structure - stores architecture-specific thread state
///
/// Layout compatible for context switching
#[repr(C)]
#[derive(Debug, Clone)]
pub struct ThreadStruct {
    // ==================== Context switch fields ====================

    /// Callee-saved register - Return address (x1)
    pub ra: u64,

    /// Callee-saved register - Stack pointer (x2)
    pub sp: u64,

    /// Callee-saved registers - s0-s11 (x8, x9, x18-x27)
    pub s: [u64; 12],

    /// sstatus.SUM bit (user memory access enable)
    ///
    /// Saved/restored during context switch, allows kernel to access user memory
    pub sum: u64,

    // ==================== FPU/Vector state ====================

    /// FPU state (f0-f31)
    ///
    /// RISC-V F extension: 32 floating-point registers
    /// Each 64-bit (double precision)
    pub fpu: [u64; FPU_STATE_SIZE],

    /// FPU control status register (fcsr)
    pub fcsr: u32,

    /// Vector extension state (V extension)
    ///
    /// TODO: Implement V extension support
    /// struct __riscv_v_ext_state
    pub vstate_valid: bool,

    // ==================== Other state ====================

    /// Thread local storage (TLS) pointer
    ///
    /// Set by set_tid_address syscall
    pub tp_value: u64,

    /// Current exception frame pointer (for signal handling)
    pub exception_sp: u64,

    /// Debug flag
    pub debug_flag: bool,
}

impl ThreadStruct {
    /// Create new thread structure
    pub const fn new() -> Self {
        Self {
            // Context switch fields
            ra: 0,
            sp: 0,
            s: [0; 12],
            sum: 0,

            // FPU/Vector state
            fpu: [0; FPU_STATE_SIZE],
            fcsr: 0,
            vstate_valid: false,

            // Other state
            tp_value: 0,
            exception_sp: 0,
            debug_flag: false,
        }
    }

    /// Save FPU state
    ///
    /// # Safety
    /// Must be called in correct context
    #[inline]
    pub unsafe fn save_fpu(&mut self) {
        // Check if FS field is Initial or Clean
        let sstatus: u64;
        asm!("csrr {}, sstatus", out(reg) sstatus);

        let fs = (sstatus >> 13) & 0x3;
        if fs == 0 {
            // FS = Off, no need to save
            return;
        }

        // Save floating-point registers f0-f31
        asm!(
            "fsd f0, 0*8({0})",
            "fsd f1, 1*8({0})",
            "fsd f2, 2*8({0})",
            "fsd f3, 3*8({0})",
            "fsd f4, 4*8({0})",
            "fsd f5, 5*8({0})",
            "fsd f6, 6*8({0})",
            "fsd f7, 7*8({0})",
            "fsd f8, 8*8({0})",
            "fsd f9, 9*8({0})",
            "fsd f10, 10*8({0})",
            "fsd f11, 11*8({0})",
            "fsd f12, 12*8({0})",
            "fsd f13, 13*8({0})",
            "fsd f14, 14*8({0})",
            "fsd f15, 15*8({0})",
            "fsd f16, 16*8({0})",
            "fsd f17, 17*8({0})",
            "fsd f18, 18*8({0})",
            "fsd f19, 19*8({0})",
            "fsd f20, 20*8({0})",
            "fsd f21, 21*8({0})",
            "fsd f22, 22*8({0})",
            "fsd f23, 23*8({0})",
            "fsd f24, 24*8({0})",
            "fsd f25, 25*8({0})",
            "fsd f26, 26*8({0})",
            "fsd f27, 27*8({0})",
            "fsd f28, 28*8({0})",
            "fsd f29, 29*8({0})",
            "fsd f30, 30*8({0})",
            "fsd f31, 31*8({0})",
            in(reg) self.fpu.as_mut_ptr(),
            options(nostack)
        );

        // Save fcsr
        asm!("frcsr {0}", out(reg) self.fcsr);
    }

    /// Restore FPU state
    ///
    /// # Safety
    /// Must be called in correct context
    #[inline]
    pub unsafe fn restore_fpu(&mut self) {
        // Restore fcsr
        asm!("fscsr {0}", in(reg) self.fcsr);

        // Restore floating-point registers f0-f31
        asm!(
            "fld f0, 0*8({0})",
            "fld f1, 1*8({0})",
            "fld f2, 2*8({0})",
            "fld f3, 3*8({0})",
            "fld f4, 4*8({0})",
            "fld f5, 5*8({0})",
            "fld f6, 6*8({0})",
            "fld f7, 7*8({0})",
            "fld f8, 8*8({0})",
            "fld f9, 9*8({0})",
            "fld f10, 10*8({0})",
            "fld f11, 11*8({0})",
            "fld f12, 12*8({0})",
            "fld f13, 13*8({0})",
            "fld f14, 14*8({0})",
            "fld f15, 15*8({0})",
            "fld f16, 16*8({0})",
            "fld f17, 17*8({0})",
            "fld f18, 18*8({0})",
            "fld f19, 19*8({0})",
            "fld f20, 20*8({0})",
            "fld f21, 21*8({0})",
            "fld f22, 22*8({0})",
            "fld f23, 23*8({0})",
            "fld f24, 24*8({0})",
            "fld f25, 25*8({0})",
            "fld f26, 26*8({0})",
            "fld f27, 27*8({0})",
            "fld f28, 28*8({0})",
            "fld f29, 29*8({0})",
            "fld f30, 30*8({0})",
            "fld f31, 31*8({0})",
            in(reg) self.fpu.as_ptr(),
            options(nostack)
        );
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
}

impl Default for ThreadStruct {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize FPU
///
/// Called when process first uses FPU
#[inline]
pub unsafe fn fpu_init() {
    // Set sstatus.FS = Initial (01)
    let mut sstatus: u64;
    asm!("csrr {}, sstatus", out(reg) sstatus);
    sstatus = (sstatus & !(0x3 << 13)) | (0x1 << 13);
    asm!("csrw sstatus, {}", in(reg) sstatus);

    // Zero all floating-point registers
    asm!(
        "fcvt.d.l f0, zero",
        "fcvt.d.l f1, zero",
        "fcvt.d.l f2, zero",
        "fcvt.d.l f3, zero",
        "fcvt.d.l f4, zero",
        "fcvt.d.l f5, zero",
        "fcvt.d.l f6, zero",
        "fcvt.d.l f7, zero",
        "fcvt.d.l f8, zero",
        "fcvt.d.l f9, zero",
        "fcvt.d.l f10, zero",
        "fcvt.d.l f11, zero",
        "fcvt.d.l f12, zero",
        "fcvt.d.l f13, zero",
        "fcvt.d.l f14, zero",
        "fcvt.d.l f15, zero",
        "fcvt.d.l f16, zero",
        "fcvt.d.l f17, zero",
        "fcvt.d.l f18, zero",
        "fcvt.d.l f19, zero",
        "fcvt.d.l f20, zero",
        "fcvt.d.l f21, zero",
        "fcvt.d.l f22, zero",
        "fcvt.d.l f23, zero",
        "fcvt.d.l f24, zero",
        "fcvt.d.l f25, zero",
        "fcvt.d.l f26, zero",
        "fcvt.d.l f27, zero",
        "fcvt.d.l f28, zero",
        "fcvt.d.l f29, zero",
        "fcvt.d.l f30, zero",
        "fcvt.d.l f31, zero",
        options(nostack)
    );

    // Zero fcsr
    asm!("fscsr zero");
}

// ==================== Offset constants (for assembly use) ====================

/// ThreadStruct field offsets
#[allow(dead_code)]
pub mod thread_offsets {
    use super::*;

    pub const THREAD_RA: usize = core::mem::offset_of!(ThreadStruct, ra);
    pub const THREAD_SP: usize = core::mem::offset_of!(ThreadStruct, sp);
    pub const THREAD_S0: usize = core::mem::offset_of!(ThreadStruct, s);
    pub const THREAD_SUM: usize = core::mem::offset_of!(ThreadStruct, sum);
    pub const THREAD_FPU: usize = core::mem::offset_of!(ThreadStruct, fpu);
    pub const THREAD_FCSR: usize = core::mem::offset_of!(ThreadStruct, fcsr);
    pub const THREAD_TP_VALUE: usize = core::mem::offset_of!(ThreadStruct, tp_value);
}

/// Export offset constants
pub use thread_offsets::*;
