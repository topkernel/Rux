//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V V extension (RVV 1.0) task support.
//!
//! Before this module, sstatus.VS was never written: it stays Off for
//! every task, and per the privileged spec ANY vector instruction traps
//! illegal-instruction while VS=Off — in every privilege mode. Static
//! binaries produced by the musl toolchain auto-vectorize ordinary
//! loops (memcmp-style e8/m1 code), so on a V-capable hart the kernel
//! silently SIGILL'd binaries the hardware could execute — the
//! "execsig text corruption" anomaly class (fleet evidence
//! 2026-10-25-heallast + sweep3): what looked like garbage-filled exec
//! pages was a valid vsetvli/vector loop mapped byte-exact from the
//! file, faulting on its FIRST vector instruction.
//!
//! Design: mirror the FPU (sstatus.FS) discipline exactly, because the
//! two extensions share the same enable/trap/save-restore semantics:
//! - Fresh tasks run with VS=Off; the first vector instruction traps
//!   and handle_illegal_instruction lazily enables VS and zeroes the
//!   live vector state (initial-state semantics, no cross-task
//!   register pollution or information leakage).
//! - Context switch saves the vector state only when the hardware
//!   marks it Dirty, reloads it on switch-in, and leaves VS=Off for
//!   tasks that never touched vectors.
//! - flush_thread (execve) and fork inherit/reset vector state like
//!   the FPU fields.
//!
//! Buffer layout: `vcsrs: [u64; 4]` = [vl, vtype, vstart, vcsr],
//! `vregs: [u8; 512]` = v0..v31, VLEN bytes each. VLEN is fixed by the
//! buffer at 128 bits (vlenb == 16): the QEMU fleet default. On a hart
//! with vlenb != 16 the kernel keeps VS=Off (vector binaries SIGILL,
//! with a one-line kernel notice) rather than corrupting state with a
//! short buffer; widening the buffer is a follow-up.
//!
//! All vector instructions are emitted as raw `.word` encodings: the
//! kernel is compiled for riscv64gc (no +v) and the assembler would
//! reject vector mnemonics. Encodings verified against
//! riscv64-linux-gnu-as -march=rv64gcv (see each constant).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// sstatus.VS field mask (bits 10:9) and values — see pt_regs for the
/// FS twins; duplicated here so this module owns its field.
pub const SR_VS: u64 = 3 << 9;
pub const SR_VS_OFF: u64 = 0;
pub const SR_VS_INITIAL: u64 = 1 << 9;
pub const SR_VS_CLEAN: u64 = 2 << 9;
pub const SR_VS_DIRTY: u64 = 3 << 9;

/// v0..v31 storage: 32 registers x VLEN/8 bytes. VLEN fixed at 128.
pub const VLENB: u64 = 16;
pub const VREGS_BYTES: usize = 32 * VLENB as usize;

// ---- verified instruction encodings (rv64gcv) ----
/// vsetvli zero, zero, e8, m1, ta, ma  — vl = VLMAX = vlenb bytes.
pub(crate) const VSETVLI_E8M1: u32 = 0x0c00_7057;
/// vmv.v.i v<n>, 0
pub(crate) const fn vmv_v_i0(vd: u32) -> u32 {
    0x5e00_3057 | (vd << 7)
}
/// vs1r.v v<n>, (a1) — whole-register store, VLEN/8 bytes.
pub(crate) const fn vs1r_v(vd: u32) -> u32 {
    0x0285_8027 | (vd << 7)
}
/// vl1r.v v<n>, (a1) — whole-register load.
pub(crate) const fn vl1r_v(vd: u32) -> u32 {
    0x0285_8007 | (vd << 7)
}
/// vsetvl zero, t0(x5), t1(x6) — restore an exact saved vl/vtype pair.
pub(crate) const VSETVL_T0_T1: u32 = 0x8062_f057;
/// csrr x5, vlenb
pub(crate) const CSRR_X5_VLENB: u32 = 0xc220_22f3;
/// csrr x5, vl
pub(crate) const CSRR_X5_VL: u32 = 0xc200_22f3;
/// csrr x5, vtype
pub(crate) const CSRR_X5_VTYPE: u32 = 0xc210_22f3;
/// csrr x5, vstart
pub(crate) const CSRR_X5_VSTART: u32 = 0x0080_22f3;
/// csrr x5, vcsr
pub(crate) const CSRR_X5_VCSR: u32 = 0x00f0_22f3;
/// csrw vstart, x7(t2)
pub(crate) const CSRW_VSTART_X7: u32 = 0x0083_9073;
/// csrw vcsr, x28(t3)
pub(crate) const CSRW_VCSR_X28: u32 = 0x00fe_1073;
/// csrw vstart, x0
pub(crate) const CSRW_VSTART_X0: u32 = 0x0080_1073;
/// csrw vcsr, x0
pub(crate) const CSRW_VCSR_X0: u32 = 0x00f0_1073;

/// Hart V capability, primed from the FDT ISA string at boot (see
/// prime_hart_has_v). misa is NOT readable in S-mode (it trapped
/// illegal-instruction on the verify boot — 2026-10-25), so the
/// device tree is the capability source, exactly like the reference
/// kernel's hwcap derivation. Harts without V leave the cache clear
/// and vector binaries correctly SIGILL.
pub(crate) static HAS_V: AtomicU8 = AtomicU8::new(0);

pub fn hart_has_v() -> bool {
    HAS_V.load(Ordering::Relaxed) == 2
}
/// Parse the boot DTB's /cpus ISA string and cache the V capability.
///
/// Collected into `arch::arch_floor` (INV-ARCH-VEXT-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn prime_hart_has_v(dtb_phys: u64) {
    crate::arch::arch_floor::riscv64::vext::prime_hart_has_v(dtb_phys)
}
/// True when the hart's vector length fits the fixed per-task buffer.
///
/// Collected into `arch::arch_floor` (INV-ARCH-VEXT-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn vlen_fits() -> bool {
    crate::arch::arch_floor::riscv64::vext::vlen_fits()
}
/// Drop VS back to Off (vlen probe rejected the hart).
///
/// Collected into `arch::arch_floor` (INV-ARCH-VEXT-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn vs_disable() {
    crate::arch::arch_floor::riscv64::vext::vs_disable()
}

pub(crate) static VLEN_WARNED: AtomicBool = AtomicBool::new(false);
/// First vector use on this task: enable VS in the LIVE sstatus and
///
/// Collected into `arch::arch_floor` (INV-ARCH-VEXT-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn vstate_first_use_init() -> bool {
    crate::arch::arch_floor::riscv64::vext::vstate_first_use_init()
}
/// Save the live vector state into `csrs` (vl, vtype, vstart, vcsr)
///
/// Collected into `arch::arch_floor` (INV-ARCH-VEXT-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn vstate_save(csrs: &mut [u64; 4], vregs: &mut [u8; super::vext::VREGS_BYTES]) {
    crate::arch::arch_floor::riscv64::vext::vstate_save(csrs, vregs)
}
/// Reload a saved vector state. Must run with VS Off or Initial for the
///
/// Collected into `arch::arch_floor` (INV-ARCH-VEXT-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn vstate_restore(csrs: &[u64; 4], vregs: &[u8; super::vext::VREGS_BYTES]) {
    crate::arch::arch_floor::riscv64::vext::vstate_restore(csrs, vregs)
}

/// Decode whether a faulting instruction is a vector instruction.
/// `instr16` is the first halfword at `epc`; `is_compressed` says it is
/// a 16-bit form (no compressed vector instructions exist, so those
/// are never vector). For 32-bit forms:
/// - opcode 1010111 (OP-V): every vector compute/config instruction
///   (vsetvli, vmv.v, arithmetic, reductions, ...).
/// - opcode 0000111 / 0100111 (LOAD-FP / STORE-FP) with width (funct3)
///   000/100/101/110/111: vector memory ops (8/128/256/512-bit EEW,
///   whole-register ld/st, and segment forms). Widths 001/010/011 are
///   scalar FP (FLH/FLW/FLD) and are left to the FPU first-use path.
///
/// `instr32` must be the full 32-bit word at `epc` when not compressed.
pub fn is_vector_instr32(instr32: u32) -> bool {
    let opcode = instr32 & 0x7F;
    let width = (instr32 >> 12) & 0x7;
    opcode == 0x57
        || ((opcode == 0x07 || opcode == 0x27)
            && matches!(width, 0 | 4 | 5 | 6 | 7))
}
