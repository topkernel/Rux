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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

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
    /// Runs in the task's own context on the local CPU (a call on a
    /// foreign struct would snapshot the WRONG live state — a logic bug
    /// the switch protocol prevents; memory-wise the target is this
    /// struct's own typed array).
    // IRREDUCIBLE F-ROW (F-XFPU-2): xsaveopt64/xsave64/fxsave64 — same
    // class as F-XFPU-1 (kernel fpu/*.S save paths).



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
///
/// Collected into `arch::arch_floor` (INV-ARCH-THREAD-15): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn fpu_init() {
    crate::arch::arch_floor::x86_64::thread::fpu_init()
}

/// ABI-default MXCSR (all exceptions masked, round-to-nearest); kept in
/// .rodata so ldmxcsr has an m32 operand without touching the stack.
pub(crate) static MXCSR_DEFAULT: u32 = 0x1F80;

/// Whether fpu_init enabled XCR0[AVX] on this machine — gates the only
/// VEX instruction outside the xsave/xrstor pair (vzeroall in the
/// no-image restore fallback). Set once at boot, read on every such
/// restore; a plain bool load is fine for that.
pub(crate) static AVX_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Whether CR4.OSXSAVE (and XCR0) are programmed on this machine: the
/// xsave/xrstor instructions #UD until OSXSAVE is set, and early-boot
/// task switches can run before fpu_init — those use the legacy
/// fxsave/fxrstor pair instead. Monotonic: once set it never clears.
pub(crate) static XSAVE_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// User XSAVE area size for the components XCR0 enables (CPUID.0xD.0:EBX,
/// captured by fpu_init right after programming XCR0 — the leaf reports
/// the size as a function of the live register). 0 until fpu_init ran.
pub(crate) static USER_XSTATE_SIZE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// fpstate area size an x86_64 rt_sigframe must reserve below the
/// ucontext — Linux `xstate_sigframe_size()` parity: the enabled user
/// components' XSAVE area plus FP_XSTATE_MAGIC2 (the trailing u32 a
/// user restore walks to validate the fpstate blob). Parts without
/// XSAVE reserve the 512-byte FXSAVE image (Linux's use_fxsr sizing).
pub fn xstate_sigframe_size() -> u64 {
    match USER_XSTATE_SIZE.load(core::sync::atomic::Ordering::Relaxed) {
        0 => 512,
        n => n as u64 + 4,
    }
}

/// XSAVEOPT available (CPUID.0xD.1:EAX[0]): fpu_save_for_switch uses
/// xsaveopt64, which skips writing components the task has not touched
/// since its xrstor switch-in. Per-CPU modified/init tracking makes this
/// correct across migrations: the first switch-out on a new CPU always
/// writes the full set (tracking cold), and the saved image lives in the
/// task's OWN buffer — a skipped write leaves the buffer holding exactly
/// what that same task's last restore loaded (closegap lane).
pub(crate) static XSAVEOPT_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub(crate) fn avx_enabled() -> bool {
    AVX_ENABLED.load(core::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn xsave_ready() -> bool {
    XSAVE_READY.load(core::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn xsaveopt_ready() -> bool {
    XSAVEOPT_READY.load(core::sync::atomic::Ordering::Relaxed)
}
