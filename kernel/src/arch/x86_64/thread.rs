//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 thread state: callee-saved registers + FPU (fxsave) + FS/GS
//! base (TLS).
//!
//! Bring-up uses FXSR (512-byte fxsave area) with CR4.OSFXSR set and
//! CR4.OSXSAVE left clear — glibc's ifunc selection honors the CPUID
//! OSXSAVE bit and stays on SSE variants. XSAVE/AVX is a later upgrade
//! (area grows to xsave layout; enable OSXSAVE + XCR0[2] then).

/// Callee-saved GPRs for __switch_to
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CalleeSaved {
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    /// Return address for __switch_to's ret (ret_from_fork for new tasks)
    pub ret_addr: u64,
}

/// fxsave area (512 bytes, 16-byte aligned required)
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FxsaveArea {
    pub bytes: [u8; 512],
}

impl FxsaveArea {
    pub const fn zeroed() -> Self {
        FxsaveArea { bytes: [0; 512] }
    }
}

/// Thread structure — arch-specific task state
#[repr(C)]
#[derive(Clone)]
pub struct ThreadStruct {
    /// Callee-saved registers + switch return address
    pub callee: CalleeSaved,
    /// Kernel stack pointer (switch-in restores this)
    pub sp: u64,
    /// FPU state (fxsave image)
    pub fpu: FxsaveArea,
    /// FPU state valid (restored lazily on switch-in)
    pub fpu_valid: bool,
    /// TLS pointers (per-task MSRs)
    pub fs_base: u64,
    pub gs_base: u64,
    /// set_tid_address value (interface parity)
    pub tp_value: u64,
    /// Current exception frame pointer (for signal handling)
    pub exception_sp: u64,
    pub debug_flag: bool,
}

impl ThreadStruct {
    pub const fn new() -> Self {
        ThreadStruct {
            callee: CalleeSaved { rbx: 0, rbp: 0, r12: 0, r13: 0, r14: 0, r15: 0, ret_addr: 0 },
            sp: 0,
            fpu: FxsaveArea::zeroed(),
            fpu_valid: false,
            fs_base: 0,
            gs_base: 0,
            tp_value: 0,
            exception_sp: 0,
            debug_flag: false,
        }
    }

    /// Save FPU state before a context switch (fxsave).
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU.
    pub unsafe fn fpu_save_for_switch(&mut self) {
        // X86-TODO(agent x86-trap): fxsave64 [self.fpu]
        self.fpu_valid = true;
    }

    /// Restore FPU state after a switch-in (fxrstor).
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU.
    pub unsafe fn restore_fpu(&mut self) {
        // X86-TODO(agent x86-trap): fxrstor64 [self.fpu] if fpu_valid
    }

    /// Mark the FPU area as a valid zeroed image (fork/exec parity with
    /// riscv64's SR_FS_CLEAN semantics).
    pub fn mark_fpu_clean(&mut self) {
        self.fpu = FxsaveArea::zeroed();
        self.fpu_valid = true;
    }

    /// Get TLS pointer
    pub fn tp(&self) -> u64 {
        self.tp_value
    }

    /// Set TLS pointer
    pub fn set_tp(&mut self, tp: u64) {
        self.tp_value = tp;
    }
}

impl Default for ThreadStruct {
    fn default() -> Self {
        Self::new()
    }
}

/// Enable FPU at boot: CR4.OSFXSR + OSXMMEEXCPT, EM/MP cleared in CR0.
///
/// # Safety
/// Must be called once per CPU before any FP instruction.
pub unsafe fn fpu_init() {
    // X86-TODO(agent x86-trap)
}
