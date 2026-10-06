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

    /// Save FPU state before a context switch (fxsave64 into thread).
    ///
    /// Unconditional: x86_64 does no lazy-FPU tracking (no CR0.TS games),
    /// so every switch-out just snapshots the live state.
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU, with the FPU
    /// enabled (CR4.OSFXSR set by `fpu_init`).
    pub unsafe fn fpu_save_for_switch(&mut self) {
        // SAFETY: fxsave64 writes 512 bytes at the given address; the
        // FxsaveArea field is 16-byte aligned (repr(align(16)) and the
        // struct layout keeps the field 16-aligned when the Task itself
        // is 16-aligned, which the allocator honors).
        unsafe {
            core::arch::asm!(
                "fxsave64 [{ptr}]",
                ptr = in(reg) self.fpu.bytes.as_mut_ptr(),
                options(nostack, preserves_flags),
            );
        }
        self.fpu_valid = true;
    }

    /// Restore FPU state after a switch-in (fxrstor64).
    ///
    /// A task with no saved image (never ran FP since fork/exec) gets a
    /// deterministic zeroed FPU instead of the previous task's registers —
    /// cross-task numeric pollution plus information leakage otherwise
    /// (review ARCH-H1 parity with the riscv64 twin).
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU.
    pub unsafe fn restore_fpu(&mut self) {
        // SAFETY: fxrstor64 reads the 512-byte image written by
        // fpu_save_for_switch (or zeroed by mark_fpu_clean); fninit +
        // pxor give the ABI initial state when no image exists.
        unsafe {
            if self.fpu_valid {
                core::arch::asm!(
                    "fxrstor64 [{ptr}]",
                    ptr = in(reg) self.fpu.bytes.as_ptr(),
                    options(nostack, preserves_flags),
                );
            } else {
                core::arch::asm!(
                    "fninit",
                    "pxor %xmm0, %xmm0",
                    "pxor %xmm1, %xmm1",
                    "pxor %xmm2, %xmm2",
                    "pxor %xmm3, %xmm3",
                    "pxor %xmm4, %xmm4",
                    "pxor %xmm5, %xmm5",
                    "pxor %xmm6, %xmm6",
                    "pxor %xmm7, %xmm7",
                    "pxor %xmm8, %xmm8",
                    "pxor %xmm9, %xmm9",
                    "pxor %xmm10, %xmm10",
                    "pxor %xmm11, %xmm11",
                    "pxor %xmm12, %xmm12",
                    "pxor %xmm13, %xmm13",
                    "pxor %xmm14, %xmm14",
                    "pxor %xmm15, %xmm15",
                    options(att_syntax, nostack),
                );
            }
        }
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

/// Enable FPU at boot: CR4.OSFXSR + OSXMMEEXCPT set, CR0.EM cleared and
/// CR0.MP set (the classic DOS-era emulation bits), then a clean x87
/// initial state.  CR4.OSXSAVE stays clear on purpose (see the module
/// comment): glibc's ifunc selection then stays on SSE code paths.
///
/// # Safety
/// Must be called once per CPU before any FP instruction.
pub unsafe fn fpu_init() {
    // SAFETY: RMW of CR4/CR0 feature bits — no memory effects; fxsave
    // area untouched.  fninit establishes the x87 initial state.
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1 << 9) | (1 << 10); // OSFXSR | OSXMMEEXCPT
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));

        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1 << 2); // clear EM (no emulation)
        cr0 |= 1 << 1; // set MP (monitor coprocessor)
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));

        core::arch::asm!("fninit", options(nostack, preserves_flags));
    }
}
