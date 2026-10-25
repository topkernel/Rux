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
const VSETVLI_E8M1: u32 = 0x0c00_7057;
/// vmv.v.i v<n>, 0
const fn vmv_v_i0(vd: u32) -> u32 {
    0x5e00_3057 | (vd << 7)
}
/// vs1r.v v<n>, (a1) — whole-register store, VLEN/8 bytes.
const fn vs1r_v(vd: u32) -> u32 {
    0x0285_8027 | (vd << 7)
}
/// vl1r.v v<n>, (a1) — whole-register load.
const fn vl1r_v(vd: u32) -> u32 {
    0x0285_8007 | (vd << 7)
}
/// vsetvl zero, t0(x5), t1(x6) — restore an exact saved vl/vtype pair.
const VSETVL_T0_T1: u32 = 0x8062_f057;
/// csrr x5, vlenb
const CSRR_X5_VLENB: u32 = 0xc220_22f3;
/// csrr x5, vl
const CSRR_X5_VL: u32 = 0xc200_22f3;
/// csrr x5, vtype
const CSRR_X5_VTYPE: u32 = 0xc210_22f3;
/// csrr x5, vstart
const CSRR_X5_VSTART: u32 = 0x0080_22f3;
/// csrr x5, vcsr
const CSRR_X5_VCSR: u32 = 0x00f0_22f3;
/// csrw vstart, x7(t2)
const CSRW_VSTART_X7: u32 = 0x0083_9073;
/// csrw vcsr, x28(t3)
const CSRW_VCSR_X28: u32 = 0x00fe_1073;
/// csrw vstart, x0
const CSRW_VSTART_X0: u32 = 0x0080_1073;
/// csrw vcsr, x0
const CSRW_VCSR_X0: u32 = 0x00f0_1073;

/// Hart V capability, primed from the FDT ISA string at boot (see
/// prime_hart_has_v). misa is NOT readable in S-mode (it trapped
/// illegal-instruction on the verify boot — 2026-10-25), so the
/// device tree is the capability source, exactly like the reference
/// kernel's hwcap derivation. Harts without V leave the cache clear
/// and vector binaries correctly SIGILL.
static HAS_V: AtomicU8 = AtomicU8::new(0);

pub fn hart_has_v() -> bool {
    HAS_V.load(Ordering::Relaxed) == 2
}

/// Parse the boot DTB's /cpus ISA string and cache the V capability.
/// Called once from main.rs on the early mapping, before any user task
/// exists. Late hart_has_v() callers (the illegal-instruction trap
/// path) only read the cache.
pub fn prime_hart_has_v(dtb_phys: u64) {
    let has = unsafe { crate::cmdline::parse_cpu_isa_has_v(dtb_phys) };
    HAS_V.store(if has { 2 } else { 1 }, Ordering::Relaxed);
}

/// True when the hart's vector length fits the fixed per-task buffer.
///
/// NOTE: the vector CSRs themselves (vlenb included) are gated by
/// sstatus.VS — reading vlenb with VS=Off traps illegal-instruction
/// (kernel-mode, caught on the first verify boot). The caller must
/// have already set VS=Initial (a pure CSR read does not dirty the
/// state, so VS can still be dropped back to Off on a mismatch).
fn vlen_fits() -> bool {
    let vlenb: u64;
    unsafe { asm!(".word {}", const CSRR_X5_VLENB, out("x5") vlenb) };
    vlenb == VLENB
}

/// Drop VS back to Off (vlen probe rejected the hart).
///
/// # Safety
/// VS must currently be Initial (not Dirty — nothing may have touched
/// the registers between the probe and this call).
#[inline]
unsafe fn vs_disable() {
    let sstatus: u64;
    asm!("csrr {}, sstatus", out(reg) sstatus);
    let new_sstatus = sstatus & !SR_VS;
    asm!("csrw sstatus, {}", in(reg) new_sstatus);
}

static VLEN_WARNED: AtomicBool = AtomicBool::new(false);

/// First vector use on this task: enable VS in the LIVE sstatus and
/// put the vector unit into a defined initial state (all registers
/// zero, vstart/vcsr zero, e8/m1 config). The caller is responsible
/// for enabling VS in the SAVED sstatus (so sret does not undo it) and
/// for marking the task's vstate buffer valid.
///
/// Returns false (and leaves VS Off) when the hart's VLEN does not fit
/// the fixed buffer — vector binaries keep SIGILLing in that case.
///
/// # Safety
/// Must be called from the current task's trap context (illegal-
/// instruction handler) on a hart that has V. Interrupts are disabled
/// in the trap entry path, so the zeroing is atomic w.r.t. switches.
pub unsafe fn vstate_first_use_init() -> bool {
    if !hart_has_v() {
        return false;
    }
    // Vector CSR access is VS-gated: enable first, then probe vlenb.
    // A rejected hart gets VS dropped back to Off (the probe only read
    // a CSR — the state is still Initial, never Dirty).
    unsafe { asm!("csrs sstatus, {mask}", mask = in(reg) SR_VS_INITIAL) };
    if !vlen_fits() {
        if !VLEN_WARNED.swap(true, Ordering::Relaxed) {
            crate::println!(
                "vext: hart VLEN != 128 (vlenb != {}); RVV tasks unsupported — SIGILL",
                VLENB
            );
        }
        unsafe { vs_disable() };
        return false;
    }
    // SAFETY: VS is now Initial; writing vector registers auto-dirties.
    asm!(
        ".word {vsetvli}",
        ".word {vsetvli}",
        ".word {v0}",  ".word {v1}",  ".word {v2}",  ".word {v3}",
        ".word {v4}",  ".word {v5}",  ".word {v6}",  ".word {v7}",
        ".word {v8}",  ".word {v9}",  ".word {v10}", ".word {v11}",
        ".word {v12}", ".word {v13}", ".word {v14}", ".word {v15}",
        ".word {v16}", ".word {v17}", ".word {v18}", ".word {v19}",
        ".word {v20}", ".word {v21}", ".word {v22}", ".word {v23}",
        ".word {v24}", ".word {v25}", ".word {v26}", ".word {v27}",
        ".word {v28}", ".word {v29}", ".word {v30}", ".word {v31}",
        ".word {vstart0}",
        ".word {vcsr0}",
        vsetvli = const VSETVLI_E8M1,
        v0 = const vmv_v_i0(0),  v1 = const vmv_v_i0(1),
        v2 = const vmv_v_i0(2),  v3 = const vmv_v_i0(3),
        v4 = const vmv_v_i0(4),  v5 = const vmv_v_i0(5),
        v6 = const vmv_v_i0(6),  v7 = const vmv_v_i0(7),
        v8 = const vmv_v_i0(8),  v9 = const vmv_v_i0(9),
        v10 = const vmv_v_i0(10), v11 = const vmv_v_i0(11),
        v12 = const vmv_v_i0(12), v13 = const vmv_v_i0(13),
        v14 = const vmv_v_i0(14), v15 = const vmv_v_i0(15),
        v16 = const vmv_v_i0(16), v17 = const vmv_v_i0(17),
        v18 = const vmv_v_i0(18), v19 = const vmv_v_i0(19),
        v20 = const vmv_v_i0(20), v21 = const vmv_v_i0(21),
        v22 = const vmv_v_i0(22), v23 = const vmv_v_i0(23),
        v24 = const vmv_v_i0(24), v25 = const vmv_v_i0(25),
        v26 = const vmv_v_i0(26), v27 = const vmv_v_i0(27),
        v28 = const vmv_v_i0(28), v29 = const vmv_v_i0(29),
        v30 = const vmv_v_i0(30), v31 = const vmv_v_i0(31),
        vstart0 = const CSRW_VSTART_X0,
        vcsr0 = const CSRW_VCSR_X0,
        options(nostack),
    );
    true
}

/// Save the live vector state into `csrs` (vl, vtype, vstart, vcsr)
/// and `vregs` (v0..v31, 16 bytes each). The save sequence itself
/// executes vector instructions (whole-register stores), so it must
/// run with VS non-Off; it temporarily switches the config to e8/m1
/// for a byte-exact register image and then restores the exact saved
/// vl/vtype/vstart/vcsr, leaving live == buffer.
///
/// # Safety
/// Caller must run on the owning task's hart with VS non-Off (Dirty).
/// `csrs` must be 8-byte aligned, [u64; 4]; `vregs` [u8; 512].
#[inline]
pub unsafe fn vstate_save(csrs: *mut u64, vregs: *mut u8) {
    asm!(
        "csrs sstatus, {mask}",           // ensure enabled (idempotent)
        ".word {rdvl}",  "sd x5, 0(x10)",
        ".word {rdvtype}", "sd x5, 8(x10)",
        ".word {rdvstart}", "sd x5, 16(x10)",
        ".word {rdvcsr}", "sd x5, 24(x10)",
        ".word {vsetvli}",                // e8/m1: byte-exact image
        // x11 walks vregs, 16 bytes per register
        ".word {s0}",  "addi x11, x11, 16",
        ".word {s1}",  "addi x11, x11, 16",
        ".word {s2}",  "addi x11, x11, 16",
        ".word {s3}",  "addi x11, x11, 16",
        ".word {s4}",  "addi x11, x11, 16",
        ".word {s5}",  "addi x11, x11, 16",
        ".word {s6}",  "addi x11, x11, 16",
        ".word {s7}",  "addi x11, x11, 16",
        ".word {s8}",  "addi x11, x11, 16",
        ".word {s9}",  "addi x11, x11, 16",
        ".word {s10}", "addi x11, x11, 16",
        ".word {s11}", "addi x11, x11, 16",
        ".word {s12}", "addi x11, x11, 16",
        ".word {s13}", "addi x11, x11, 16",
        ".word {s14}", "addi x11, x11, 16",
        ".word {s15}", "addi x11, x11, 16",
        ".word {s16}", "addi x11, x11, 16",
        ".word {s17}", "addi x11, x11, 16",
        ".word {s18}", "addi x11, x11, 16",
        ".word {s19}", "addi x11, x11, 16",
        ".word {s20}", "addi x11, x11, 16",
        ".word {s21}", "addi x11, x11, 16",
        ".word {s22}", "addi x11, x11, 16",
        ".word {s23}", "addi x11, x11, 16",
        ".word {s24}", "addi x11, x11, 16",
        ".word {s25}", "addi x11, x11, 16",
        ".word {s26}", "addi x11, x11, 16",
        ".word {s27}", "addi x11, x11, 16",
        ".word {s28}", "addi x11, x11, 16",
        ".word {s29}", "addi x11, x11, 16",
        ".word {s30}", "addi x11, x11, 16",
        ".word {s31}",
        // restore the exact saved config (live == buffer afterwards)
        "ld x5, 0(x10)",                  // vl
        "ld x6, 8(x10)",                  // vtype
        "ld x7, 16(x10)",                 // vstart
        "ld x28, 24(x10)",                // vcsr
        ".word {vsetvl}",
        ".word {wrvstart}",
        ".word {wrvcsr}",
        mask = in(reg) SR_VS_INITIAL,
        rdvl = const CSRR_X5_VL,
        rdvtype = const CSRR_X5_VTYPE,
        rdvstart = const CSRR_X5_VSTART,
        rdvcsr = const CSRR_X5_VCSR,
        vsetvli = const VSETVLI_E8M1,
        s0 = const vs1r_v(0),  s1 = const vs1r_v(1),  s2 = const vs1r_v(2),
        s3 = const vs1r_v(3),  s4 = const vs1r_v(4),  s5 = const vs1r_v(5),
        s6 = const vs1r_v(6),  s7 = const vs1r_v(7),  s8 = const vs1r_v(8),
        s9 = const vs1r_v(9),  s10 = const vs1r_v(10), s11 = const vs1r_v(11),
        s12 = const vs1r_v(12), s13 = const vs1r_v(13), s14 = const vs1r_v(14),
        s15 = const vs1r_v(15), s16 = const vs1r_v(16), s17 = const vs1r_v(17),
        s18 = const vs1r_v(18), s19 = const vs1r_v(19), s20 = const vs1r_v(20),
        s21 = const vs1r_v(21), s22 = const vs1r_v(22), s23 = const vs1r_v(23),
        s24 = const vs1r_v(24), s25 = const vs1r_v(25), s26 = const vs1r_v(26),
        s27 = const vs1r_v(27), s28 = const vs1r_v(28), s29 = const vs1r_v(29),
        s30 = const vs1r_v(30), s31 = const vs1r_v(31),
        vsetvl = const VSETVL_T0_T1,
        wrvstart = const CSRW_VSTART_X7,
        wrvcsr = const CSRW_VCSR_X28,
        in("x10") csrs,
        in("x11") vregs,
        out("x5") _, out("x6") _, out("x7") _, out("x28") _,
        options(nostack),
    );
}

/// Reload a saved vector state. Must run with VS Off or Initial for the
/// outgoing image; enables VS (the loads auto-dirty it) and ends with
/// live == buffer (the caller then records VS=Clean).
///
/// # Safety
/// Caller must run on the owning task's hart. `csrs` 8-byte aligned
/// [u64; 4]; `vregs` [u8; 512] previously filled by [`vstate_save`].
#[inline]
pub unsafe fn vstate_restore(csrs: *const u64, vregs: *const u8) {
    asm!(
        "csrs sstatus, {mask}",           // enable before vector ops
        "ld x5, 0(x10)",                  // vl
        "ld x6, 8(x10)",                  // vtype
        "ld x7, 16(x10)",                 // vstart
        "ld x28, 24(x10)",                // vcsr
        ".word {vsetvli}",                // legal config before loads
        ".word {l0}",  "addi x11, x11, 16",
        ".word {l1}",  "addi x11, x11, 16",
        ".word {l2}",  "addi x11, x11, 16",
        ".word {l3}",  "addi x11, x11, 16",
        ".word {l4}",  "addi x11, x11, 16",
        ".word {l5}",  "addi x11, x11, 16",
        ".word {l6}",  "addi x11, x11, 16",
        ".word {l7}",  "addi x11, x11, 16",
        ".word {l8}",  "addi x11, x11, 16",
        ".word {l9}",  "addi x11, x11, 16",
        ".word {l10}", "addi x11, x11, 16",
        ".word {l11}", "addi x11, x11, 16",
        ".word {l12}", "addi x11, x11, 16",
        ".word {l13}", "addi x11, x11, 16",
        ".word {l14}", "addi x11, x11, 16",
        ".word {l15}", "addi x11, x11, 16",
        ".word {l16}", "addi x11, x11, 16",
        ".word {l17}", "addi x11, x11, 16",
        ".word {l18}", "addi x11, x11, 16",
        ".word {l19}", "addi x11, x11, 16",
        ".word {l20}", "addi x11, x11, 16",
        ".word {l21}", "addi x11, x11, 16",
        ".word {l22}", "addi x11, x11, 16",
        ".word {l23}", "addi x11, x11, 16",
        ".word {l24}", "addi x11, x11, 16",
        ".word {l25}", "addi x11, x11, 16",
        ".word {l26}", "addi x11, x11, 16",
        ".word {l27}", "addi x11, x11, 16",
        ".word {l28}", "addi x11, x11, 16",
        ".word {l29}", "addi x11, x11, 16",
        ".word {l30}", "addi x11, x11, 16",
        ".word {l31}",
        ".word {vsetvl}",                 // exact saved config
        ".word {wrvstart}",
        ".word {wrvcsr}",
        mask = in(reg) SR_VS_INITIAL,
        vsetvli = const VSETVLI_E8M1,
        l0 = const vl1r_v(0),  l1 = const vl1r_v(1),  l2 = const vl1r_v(2),
        l3 = const vl1r_v(3),  l4 = const vl1r_v(4),  l5 = const vl1r_v(5),
        l6 = const vl1r_v(6),  l7 = const vl1r_v(7),  l8 = const vl1r_v(8),
        l9 = const vl1r_v(9),  l10 = const vl1r_v(10), l11 = const vl1r_v(11),
        l12 = const vl1r_v(12), l13 = const vl1r_v(13), l14 = const vl1r_v(14),
        l15 = const vl1r_v(15), l16 = const vl1r_v(16), l17 = const vl1r_v(17),
        l18 = const vl1r_v(18), l19 = const vl1r_v(19), l20 = const vl1r_v(20),
        l21 = const vl1r_v(21), l22 = const vl1r_v(22), l23 = const vl1r_v(23),
        l24 = const vl1r_v(24), l25 = const vl1r_v(25), l26 = const vl1r_v(26),
        l27 = const vl1r_v(27), l28 = const vl1r_v(28), l29 = const vl1r_v(29),
        l30 = const vl1r_v(30), l31 = const vl1r_v(31),
        vsetvl = const VSETVL_T0_T1,
        wrvstart = const CSRW_VSTART_X7,
        wrvcsr = const CSRW_VCSR_X28,
        in("x10") csrs,
        in("x11") vregs,
        out("x5") _, out("x6") _, out("x7") _, out("x28") _,
        options(nostack),
    );
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
