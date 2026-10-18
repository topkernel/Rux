//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 thread state: callee-saved registers + FPU (xsave) + FS/GS
//! base (TLS).
//!
//! FPU context switching uses the XSAVE layout with CR4.OSXSAVE set
//! and XCR0 enabling x87/SSE/AVX (+OPMASK/ZMM when present). CPUID
//! advertises AVX to userspace on any modern host, and OH's musl
//! memcpy variants use VEX-encoded instructions without a glibc-style
//! OSXSAVE gate — with XCR0[AVX] clear every such instruction #UDs
//! and SIGILLs the process (b143: hdcd and processdump died this way
//! in a tight respawn loop; the #UD bytes were vmovaps/vmovups VEX).

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

/// XSAVE area. The legacy FXSAVE image occupies the first 512 bytes
/// (FCW at 0, FTW at 4, MXCSR at 24); the extended area (YMM at 576,
/// OPMASK/ZMM beyond) follows at CPU-enumerated offsets. 4096 bytes
/// covers every user component through Hi16_ZMM (2688 on AVX-512
/// parts) with the 64-byte alignment xsave/xrstor require.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct FxsaveArea {
    pub bytes: [u8; 4096],
}

impl FxsaveArea {
    pub const fn zeroed() -> Self {
        FxsaveArea { bytes: [0; 4096] }
    }

    /// ABI initial image — what Linux's fpstate_init hands a fresh task.
    ///
    /// Legacy header: FCW at 0 (u16), abridged FTW at 4 (u8), MXCSR at
    /// 24 (u32).  All-zero bytes decode as FCW=0 and MXCSR=0, i.e. every
    /// x87 and SSE exception UNMASKED — xrstor of a zeroed image leaves
    /// the task one inexact libm result away from a #MF/#XM trap loop.
    /// The ABI initial state masks all exceptions: FCW=0x037F, FTW=0xFF
    /// (all registers empty), MXCSR=0x1F80 (masked, round-to-nearest).
    /// XSTATE_BV (offset 512) stays 0: xrstor initializes every extended
    /// component whose bit is clear in the bitvector — exactly the ABI
    /// initial state for YMM/ZMM too.
    /// MXCSR_MASK (offset 28) is ignored by fxrstor/xrstor.
    pub const fn initial() -> Self {
        let mut a = FxsaveArea { bytes: [0; 4096] };
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

    /// Save FPU state before a context switch (xsave64 into thread).
    ///
    /// Unconditional: x86_64 does no lazy-FPU tracking (no CR0.TS games),
    /// so every switch-out just snapshots the live state. The imm8 mask
    /// 0xFF covers every user component bit XCR0 can hold (bits 0..7 —
    /// x87, SSE, AVX, OPMASK, ZMM_Hi256, Hi16_ZMM; PKRU/PT/AMX live in
    /// higher bits we never enable).
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU, with the FPU
    /// enabled (CR4.OSXSAVE set by `fpu_init`).
    pub unsafe fn fpu_save_for_switch(&mut self) {
        // SAFETY: xsave64 writes at most the enabled components into the
        // 4096-byte area; the FxsaveArea field is 64-byte aligned
        /// (repr(align(64)) and the Task allocator honors alignment).
        unsafe {
            core::arch::asm!(
                "xsave64 ({ptr})",
                ptr = in(reg) self.fpu.bytes.as_mut_ptr(),
                in("eax") 0xFFFF_FFFFu32,
                in("edx") 0xFFFF_FFFFu32,
                options(att_syntax, nostack, preserves_flags),
            );
        }
        self.fpu_valid = true;
    }

    /// Restore FPU state after a switch-in (xrstor64).
    ///
    /// A task with no saved image (never ran FP since fork/exec) gets the
    /// deterministic ABI initial state instead of the previous task's
    /// registers — cross-task numeric pollution plus information leakage
    /// otherwise (review ARCH-H1 parity with the riscv64 twin).  fninit
    /// fixes the x87 side but NOT MXCSR, so the fallback must also load
    /// the default MXCSR: the live register still belongs to whichever
    /// task ran last on this CPU. vzeroall wipes the YMM half for the
    /// same reason.
    ///
    /// # Safety
    /// Must run in the task's own context on the local CPU.
    pub unsafe fn restore_fpu(&mut self) {
        // SAFETY: xrstor64 reads the image written by fpu_save_for_switch
        // (or built by mark_fpu_clean — XSTATE_BV=0 initializes every
        // extended component); the fallback below establishes the ABI
        // initial state when no image exists.
        unsafe {
            if self.fpu_valid {
                core::arch::asm!(
                    "xrstor64 ({ptr})",
                    ptr = in(reg) self.fpu.bytes.as_ptr(),
                    in("eax") 0xFFFF_FFFFu32,
                    in("edx") 0xFFFF_FFFFu32,
                    options(att_syntax, nostack, preserves_flags),
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
                // Wipe the YMM half too — but only where XCR0 actually
                // enables AVX: vzeroall on a part (or XCR0 mask) without
                // it would itself #UD.
                if avx_enabled() {
                    // SAFETY: vzeroall only writes YMM registers.
                    unsafe {
                        core::arch::asm!("vzeroall", options(nostack, preserves_flags));
                    }
                }
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

/// Enable FPU at boot: CR4.OSFXSR + OSXMMEEXCPT + CR4.OSXSAVE set,
/// CR0.EM cleared and CR0.MP set (the classic DOS-era emulation bits),
/// XCR0 enabled for the user state components present (x87, SSE, AVX,
/// OPMASK, ZMM_Hi256, Hi16_ZMM — never MPX/PT/PKRU/AMX), then a clean
/// x87 initial state.
///
/// OSXSAVE + XCR0[AVX] MUST be on: CPUID advertises AVX to userspace
/// on every modern host, and OH's musl uses VEX-encoded instructions
/// without a glibc-style OSXSAVE gate — with XCR0[AVX] clear every
/// vmovaps/vmovups #UDs and SIGILLs the process (b143: hdcd and
/// processdump died in a respawn loop this way).
///
/// # Safety
/// Must be called once per CPU before any FP instruction.
pub unsafe fn fpu_init() {
    // SAFETY: RMW of CR4/CR0 feature bits — no memory effects; xsave
    // area untouched.  fninit establishes the x87 initial state.
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1 << 9) | (1 << 10); // OSFXSR | OSXMMEEXCPT
        // XSAVE enablement (guarded below by CPUID — the #NM handler
        // re-runs this on ancient parts without it).
        let (_, _, feat_ecx, _) = crate::arch::x86_64::cpu::cpuid(1, 0);
        let osxsave_ok = feat_ecx & (1 << 26) != 0 && feat_ecx & (1 << 27) != 0;
        if osxsave_ok {
            cr4 |= 1 << 18; // OSXSAVE
        }
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));

        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1 << 2); // clear EM (no emulation)
        cr0 |= 1 << 1; // set MP (monitor coprocessor)
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));

        // XCR0 = every user component the CPU enumerates in leaf 0xD
        // ANDed with the imm8-reachable set {x87, SSE, AVX, OPMASK,
        // ZMM_Hi256, Hi16_ZMM}. Setting a bit the CPU does not support
        // is #GP, so the CPUID mask (leaf 0xD EAX) gates each one.
        if osxsave_ok {
            let (d_eax, _, _, _) = crate::arch::x86_64::cpu::cpuid(0xd, 0);
            let mut mask: u64 = 0x3; // x87 | SSE — always
            if d_eax & (1 << 2) != 0 {
                mask |= 1 << 2; // YMM (AVX)
                AVX_ENABLED.store(true, core::sync::atomic::Ordering::Relaxed);
            }
            if d_eax & (1 << 5) != 0 {
                mask |= 1 << 5; // OPMASK
            }
            if d_eax & (1 << 6) != 0 {
                mask |= 1 << 6; // ZMM_Hi256
            }
            if d_eax & (1 << 7) != 0 {
                mask |= 1 << 7; // Hi16_ZMM
            }
            // SAFETY: xsetbv with ECX=0 programs XCR0; EDX:EAX holds the
            // mask. Only the bits CPUID reported are set.
            core::arch::asm!(
                "xsetbv",
                in("eax") mask as u32,
                in("edx") (mask >> 32) as u32,
                in("ecx") 0u32,
                options(nostack, preserves_flags),
            );
        }

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

/// Whether fpu_init enabled XCR0[AVX] on this machine — gates the only
/// VEX instruction outside the xsave/xrstor pair (vzeroall in the
/// no-image restore fallback). Set once at boot, read on every such
/// restore; a plain bool load is fine for that.
static AVX_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

fn avx_enabled() -> bool {
    AVX_ENABLED.load(core::sync::atomic::Ordering::Relaxed)
}
