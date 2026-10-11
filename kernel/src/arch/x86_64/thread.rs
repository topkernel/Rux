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

    /// ABI initial image — what Linux's fpstate_init hands a fresh task.
    ///
    /// fxsave64 layout: FCW at 0 (u16), abridged FTW at 4 (u8), MXCSR at
    /// 24 (u32).  All-zero bytes decode as FCW=0 and MXCSR=0, i.e. every
    /// x87 and SSE exception UNMASKED — fxrstor of a zeroed image leaves
    /// the task one inexact libm result away from a #MF/#XM trap loop.
    /// The ABI initial state masks all exceptions: FCW=0x037F, FTW=0xFF
    /// (all registers empty), MXCSR=0x1F80 (masked, round-to-nearest).
    /// MXCSR_MASK (offset 28) is ignored by fxrstor.
    pub const fn initial() -> Self {
        let mut a = FxsaveArea { bytes: [0; 512] };
        a.bytes[0] = 0x7F; // FCW lo
        a.bytes[1] = 0x03; // FCW hi  (0x037F)
        a.bytes[4] = 0xFF; // abridged FTW: all registers empty
        a.bytes[24] = 0x80; // MXCSR lo
        a.bytes[25] = 0x1F; // MXCSR hi (0x1F80)
        a
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
    /// A task with no saved image (never ran FP since fork/exec) gets the
    /// deterministic ABI initial state instead of the previous task's
    /// registers — cross-task numeric pollution plus information leakage
    /// otherwise (review ARCH-H1 parity with the riscv64 twin).  fninit
    /// fixes the x87 side but NOT MXCSR, so the fallback must also load
    /// the default MXCSR: the live register still belongs to whichever
    /// task ran last on this CPU.
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU.
    pub unsafe fn restore_fpu(&mut self) {
        // SAFETY: fxrstor64 reads the 512-byte image written by
        // fpu_save_for_switch (or built by mark_fpu_clean); the fallback
        // below establishes the ABI initial state when no image exists.
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
                    "ldmxcsr ({mxcsr})",
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
                    mxcsr = in(reg) &MXCSR_DEFAULT,
                    options(att_syntax, nostack),
                );
            }
        }
    }

    /// Mark the FPU area as a valid ABI-initial image (fork/exec parity
    /// with riscv64's SR_FS_CLEAN semantics).  Must NOT be a plain zeroed
    /// image: fxrstor would load FCW=0/MXCSR=0 and unmask every FP
    /// exception (see FxsaveArea::initial).
    pub fn mark_fpu_clean(&mut self) {
        self.fpu = FxsaveArea::initial();
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

        // fninit alone leaves MXCSR at its reset-unknown value; load the
        // ABI default explicitly so the boot CPU starts fully masked too.
        // (ldmxcsr is m32-only — no register form.)
        core::arch::asm!(
            "fninit",
            "ldmxcsr [{mxcsr}]",
            mxcsr = in(reg) &MXCSR_DEFAULT,
            options(nostack, preserves_flags),
        );
    }
}

/// ABI-default MXCSR (all exceptions masked, round-to-nearest); kept in
/// .rodata so ldmxcsr has an m32 operand without touching the stack.
static MXCSR_DEFAULT: u32 = 0x1F80;
