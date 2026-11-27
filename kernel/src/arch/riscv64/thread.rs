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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;
use super::pt_regs::{PtRegs, SR_FS_DIRTY, SR_FS_CLEAN, SR_FS_INITIAL, SR_FS_OFF, SR_FS};

/// FPU state size (32 64-bit registers)
pub(crate) const FPU_STATE_SIZE: usize = 32;

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

    /// Saved FPU status (sstatus.FS field)
    ///
    /// Saved before context switch, restored when task runs again
    /// Values: SR_FS_OFF, SR_FS_INITIAL, SR_FS_CLEAN, SR_FS_DIRTY
    pub fs: u32,

    /// Vector extension state (V extension, RVV 1.0)
    ///
    /// Mirrors the FPU discipline (see arch/riscv64/vext.rs): VS stays
    /// Off until the task's first vector instruction traps and is
    /// lazily enabled; the state is saved on switch-out when the
    /// hardware marks it Dirty and reloaded on switch-in.
    pub vstate_valid: bool,

    /// Saved sstatus.VS field (Off/Initial/Clean/Dirty)
    pub vs: u32,

    /// Saved vector CSRs: [vl, vtype, vstart, vcsr]
    pub vcsrs: [u64; 4],

    /// Saved vector registers v0..v31, VLEN/8 bytes each (VLEN=128)
    pub vregs: [u8; super::vext::VREGS_BYTES],

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

// IRREDUCIBLE F-ROW (F-CSR-R1, file-level): the sstatus CSR read/write
// pairs sprinkled through the FPU wrappers — reference kernel
// arch/riscv's csr.h CSRS/CSRC macros; CSR access has no safe shape.
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
            // INITIAL, not OFF: fresh tasks restore a zeroed FPU state on
            // their first switch-in (review ARCH-H1) — OFF would leave
            // their first FP instruction trapping as illegal.
            fs: SR_FS_INITIAL as u32,
            vstate_valid: false,
            vs: super::vext::SR_VS_OFF as u32,
            vcsrs: [0; 4],
            vregs: [0; super::vext::VREGS_BYTES],

            // Other state
            tp_value: 0,
            exception_sp: 0,
            debug_flag: false,
        }
    }




    /// Get saved FS status
    #[inline]
    pub fn get_fs(&self) -> u32 {
        self.fs
    }

    // ==================== Vector extension (RVV) ====================
    //
    // Same discipline as the FPU methods above; see arch/riscv64/vext.rs
    // for the rationale (sstatus.VS lazy enable + Dirty-gated saves).





    /// Mark the saved vector state Clean (post-fork inheritance).
    #[inline]
    pub fn mark_vstate_clean(&mut self) {
        self.vs = super::vext::SR_VS_CLEAN as u32;
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

    /// Mark the saved FPU image as a clean, valid state (fork/exec
    /// parity with the x86_64 backend).
    #[inline]
    pub fn mark_fpu_clean(&mut self) {
        self.fs = SR_FS_CLEAN as u32;
    }
}

impl Default for ThreadStruct {
    fn default() -> Self {
        Self::new()
    }
}
/// Initialize FPU
///
/// Collected into `arch::arch_floor` (INV-ARCH-THREAD-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn fpu_init() {
    crate::arch::arch_floor::riscv64::thread::fpu_init()
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
