//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! vDSO (virtual dynamic shared object) — P2.
//!
//! A two-page user mapping installed by every execve:
//!
//! ```text
//!   VDSO_BASE        +0    data page  (RW)  kernel-updated time base
//!   VDSO_BASE+0x1000 +1    code page  (RX)  minimal ELF with 3 symbols
//! ```
//!
//! `AT_SYSINFO_EHDR` points at the code page; musl/glibc parse the ELF's
//! dynamic symbol table and call the functions directly, making
//! clock_gettime/gettimeofday/clock_getres user-space fast paths.
//!
//! Time source: the 10 MHz `time` CSR (100 ns per cycle exactly — no
//! mult/shift fixed-point needed, `ns = base_ns + (cycle - base_cycle) *
//! 100`). The kernel refreshes {seq, base_cycle, base_ns} from the timer
//! tick under a seqlock; the vDSO functions interpolate from the current
//! `rdcycle` and retry when the seqlock moved mid-read.
//!
//! Data page layout (offsets in bytes):
//!   0   u32  seq            (odd = write in progress)
//!   8   u64  base_cycles    time CSR at the last refresh
//!   16  u64  base_mono_ns   monotonic ns at the last refresh
//!   24  u64  wall_epoch_ns  REALTIME = monotonic + this (ns)
//!   32  u32  tz_minuteswest (always 0: no timezone model)
//!   36  u32  tz_dsttime
//!
//! Functions (RV64 code, built below by a mini assembler):
//!   __vdso_clock_gettime(clk, struct timespec*)       0 / -ENOSYS
//!   __vdso_gettimeofday(struct timeval*, struct tz*)  0
//!   __vdso_clock_getres(clk, struct timespec*)        0 / -ENOSYS
//!
//! Supported clocks: REALTIME(0), MONOTONIC(1), MONOTONIC_RAW(4),
//! BOOTTIME(7), TAI(11 = REALTIME + 37). Everything else returns
//! -ENOSYS so libc falls back to the real syscall.

use core::sync::atomic::{AtomicU32, Ordering};

/// Fixed user mapping address (INTERP_BASE - 4 MiB, clear of the dynamic
/// linker's base).
pub const VDSO_BASE: u64 = 0x3FBE_C000_00;

/// auxv AT_SYSINFO_EHDR value: the code (ELF) page.
pub fn vdso_ehdr() -> u64 {
    VDSO_BASE + 4096
}

// ============================================================================
// Data page
// ============================================================================

/// The shared vDSO data page (4 KiB aligned), refreshed by
/// `vdso_data_tick()`.
#[repr(align(4096))]
struct VdsoDataPage([u8; 4096]);

static mut VDSO_DATA_PAGE: VdsoDataPage = VdsoDataPage([0; 4096]);

/// The seqlock word IS data-page offset 0 (odd = write in progress).
///
/// It must be accessed THROUGH the page pointer, never through a
/// separate static: the user-side vDSO code reads page[0] and retries
/// while it is odd or changed across the read. The original separate
/// VDSO_SEQ static never aliased the page, so page[0] stayed 0 forever,
/// no reader ever retried, and a reader descheduled between its
/// base_tsc and base_mono_ns loads paired a stale tsc with a fresh mono
/// — a vDSO clock AHEAD of the syscall clock by the whole preemption
/// gap (LTP clock_gettime04: "Time travelled backwards (-454618735 ns)"
/// on every clock, only on loads noisy enough to deschedule the reader
/// for hundreds of ms).
// SAFETY: only written from the timer-tick refresh paths below.
unsafe fn vdso_seq() -> &'static AtomicU32 {
    unsafe { &*(VDSO_DATA_PAGE.0.as_ptr() as *const AtomicU32) }
}

/// Refresh the data page snapshot (timer tick / clock-set paths).
///
/// Seqlock protocol: seq |= 1 (begin) → write fields → seq += 1 (end,
/// even). Readers retry while seq is odd or changed across the read.
#[cfg(not(feature = "x86_64"))]
pub fn vdso_data_tick() {
    let cycles = crate::drivers::timer::read_time();
    // 10 MHz source: 100 ns per cycle, exact.
    let mono_ns = cycles.saturating_mul(100);
    let epoch_ns = crate::syscall::time::wall_epoch_offset_ns();
    // SAFETY: the page is only written here under the seqlock; user
    // readers validate via the seq word before trusting the fields.
    unsafe {
        let seq = vdso_seq();
        seq.fetch_or(1, Ordering::Release);
        let base = VDSO_DATA_PAGE.0.as_mut_ptr();
        core::ptr::write_volatile(base.add(8) as *mut u64, cycles);
        core::ptr::write_volatile(base.add(16) as *mut u64, mono_ns);
        core::ptr::write_volatile(base.add(24) as *mut u64, epoch_ns);
        // tz fields (32/36) stay zero.
        seq.fetch_add(1, Ordering::Release);
    }
}

/// x86_64 twin (106th: the vDSO lever — 21.3M clock_gettime syscalls
/// per OH boot at ~3 µs each ≈ 6.6% of the whole box).
///
/// The user side reads the RAW TSC (`rdtsc`), so the published pair is
/// (raw base_tsc, kernel mono ns at that instant) plus a TSC→ns
/// mult/shift for the interpolation `ns = base + ((tsc - base) *
/// mult) >> 32`. Data page x86 extensions: mult u64 @40, shift u32 @44
/// (always 32, matching the kernel's TSC_MULSHIFT), res_ns u32 @48.
///
/// Known, accepted skews (same class as the reference's vDSO):
/// - `read_time()` applies per-CPU vCPU TSC-offset corrections and a
///   monotonic ratchet; a user `rdtsc` on another CPU than the tick's
///   carries the inter-CPU TSC offset delta (µs-class under KVM's
///   invariant TSC; the historical ms-class offsets were the shift-12
///   mult bug, fixed).
/// - base_tsc and base_mono_ns are two reads, not one atomic snapshot;
///   the error is a per-tick constant offset well under a jiffy.
#[cfg(feature = "x86_64")]
pub fn vdso_data_tick() {
    let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    if hz < 1_000_000 {
        return; // TSC not calibrated yet — keep the primed snapshot
    }
    let mult = ((1u128 << 32) * 1_000_000_000u128 / (hz as u128)) as u64;
    let tsc = crate::arch::cpu::read_counter();
    let mono_ns = crate::drivers::timer::monotonic_ns_since_boot();
    let epoch_ns = crate::syscall::time::wall_epoch_offset_ns();
    // SAFETY: the page is only written here under the seqlock; user
    // readers validate via the seq word before trusting the fields.
    unsafe {
        let seq = vdso_seq();
        seq.fetch_or(1, Ordering::Release);
        let base = VDSO_DATA_PAGE.0.as_mut_ptr();
        core::ptr::write_volatile(base.add(8) as *mut u64, tsc);
        core::ptr::write_volatile(base.add(16) as *mut u64, mono_ns);
        core::ptr::write_volatile(base.add(24) as *mut u64, epoch_ns);
        core::ptr::write_volatile(base.add(40) as *mut u64, mult);
        core::ptr::write_volatile(base.add(44) as *mut u32, 32u32);
        core::ptr::write_volatile(base.add(48) as *mut u32, 1u32);
        seq.fetch_add(1, Ordering::Release);
    }
}

/// Physical addresses of the two vDSO pages (data, code) for the exec
/// mapping. `vdso_init()` must have run (code page built).
#[cfg(not(feature = "x86_64"))]
pub fn vdso_pages_phys() -> (u64, u64) {
    let data_va = core::ptr::addr_of!(VDSO_DATA_PAGE) as *const u8 as u64;
    let code_va = core::ptr::addr_of!(VDSO_CODE_PAGE) as *const u8 as u64;
    let d = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(data_va),
    );
    let c = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(code_va),
    );
    (d.bits(), c.bits())
}

/// x86_64 twin: the two statics live in the kernel image mapping,
/// whose link identity `VMA - KERNEL_LINK_BASE == LMA` holds for the
/// whole image (the __START_KERNEL_map trick — see memory_layout.rs),
/// so the physical address is plain arithmetic, no linear-map lookup
/// (the riscv64 blocker — statics outside the linear map — does not
/// exist here; see the disabled riscv block in exec.rs).
///
/// The frames are memblock-reserved (the boot code reserves [0, real
/// image end] on x86_64), so every mm teardown skips them via
/// `is_foreign_user_frame` — the same machinery that keeps the GEM/fb
/// aliases alive. `map_page`'s LEAF-LOW-HIT scribble tripwire allows
/// exactly these two frames (mmu_init.rs).
#[cfg(feature = "x86_64")]
pub fn vdso_pages_phys() -> (u64, u64) {
    let data_va = core::ptr::addr_of!(VDSO_DATA_PAGE) as *const u8 as u64;
    let code_va = core::ptr::addr_of!(VDSO_CODE_PAGE) as *const u8 as u64;
    let base = crate::arch::mm::memory_layout::KERNEL_LINK_BASE as u64;
    (data_va - base, code_va - base)
}

/// The two user-aliased vDSO physical frames (mmu_init's LEAF-LOW-HIT
/// exclusion test).
#[cfg(feature = "x86_64")]
pub fn is_vdso_phys(phys: u64) -> bool {
    let (d, c) = vdso_pages_phys();
    phys == d || phys == c
}

// ============================================================================
// Code page — mini RV64 assembler
// ============================================================================

/// The vDSO ELF page (4 KiB aligned), built once by `vdso_init()`.
#[repr(align(4096))]
struct VdsoCodePage([u8; 4096]);

static mut VDSO_CODE_PAGE: VdsoCodePage = VdsoCodePage([0; 4096]);

#[cfg(not(feature = "x86_64"))]
mod rv {
    // The original RV64 mini-assembler + function emitters. On
    // aarch64 the vDSO mapping stays disabled, so the RV encoding
    // rides along inert (same state as before the x86_64 port).
    use alloc::vec::Vec;

    /// Tiny two-pass label assembler (u32 words, forward/backward branches).
    struct Asm {
        words: alloc::vec::Vec<u32>,
        labels: [Option<u32>; 16],
        /// (insn index, label id, is_b_type)
        fixups: alloc::vec::Vec<(usize, usize, bool)>,
    }

    impl Asm {
        fn new() -> Self {
            Self {
                words: alloc::vec::Vec::new(),
                labels: [None; 16],
                fixups: alloc::vec::Vec::new(),
            }
        }
        fn label(&mut self, l: usize) {
            self.labels[l] = Some(self.words.len() as u32);
        }
        fn emit(&mut self, w: u32) {
            self.words.push(w);
        }
        /// Emit a branch-format word (beq/bne skeleton) targeting `l`.
        fn br(&mut self, w: u32, l: usize) {
            self.fixups.push((self.words.len(), l, true));
            self.words.push(w);
        }
        /// Emit a `j label` targeting `l`.
        fn jal_rel(&mut self, l: usize) {
            self.fixups.push((self.words.len(), l, false));
            self.words.push((0u32 << 7) | 0x6F);
        }
        fn finish(mut self) -> alloc::vec::Vec<u32> {
            for (idx, l, is_b) in self.fixups.drain(..) {
                let target = self.labels[l].expect("vdso: undefined label") as i64;
                let off = (target - idx as i64) as u32;
                self.words[idx] |= if is_b { b_imm(off) } else { j_imm(off) };
            }
            self.words
        }
    }

    // ---- encoders (RV64I + M) ----

    fn r_type(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
        (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
    }
    fn i_type(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
        ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
    }
    fn s_type(imm: i32, rs2: u32, rs1: u32, f3: u32, op: u32) -> u32 {
        let imm = imm as u32 & 0xFFF;
        ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 0x1F) << 7) | op
    }
    /// B-type immediate bits to OR into a branch skeleton.
    fn b_imm(off: u32) -> u32 {
        ((off & 0x1000) << 19) | ((off & 0x7E0) << 20) | ((off & 0x1E) << 7) | ((off & 0x800) >> 4)
    }
    /// J-type immediate bits to OR into a jal skeleton.
    fn j_imm(off: u32) -> u32 {
        ((off & 0x100000) << 11) | ((off & 0x7FE) << 20) | ((off & 0x800) << 9) | (off & 0xFF000)
    }
    fn u_type(imm20: u32, rd: u32, op: u32) -> u32 {
        ((imm20 & 0xFFFFF) << 12) | (rd << 7) | op
    }
    fn beq_skel(rs1: u32, rs2: u32) -> u32 {
        r_type(0, rs2, rs1, 0, 0, 0x63)
    }
    fn bne_skel(rs1: u32, rs2: u32) -> u32 {
        r_type(0, rs2, rs1, 1, 0, 0x63)
    }

    // registers
    const ZERO: u32 = 0;
    const RA: u32 = 1;
    const T0: u32 = 5;
    const T1: u32 = 6;
    const T2: u32 = 7;
    const T3: u32 = 28;
    const T4: u32 = 29;
    const T5: u32 = 30;
    const T6: u32 = 31;
    const A0: u32 = 10;
    const A1: u32 = 11;
    const A6: u32 = 16;
    const A7: u32 = 17;
    const S2: u32 = 18;

    // labels (each used by exactly ONE label() site — no re-binding)
    const L_CGI_RETRY_W: usize = 0;
    const L_CGI_MONO: usize = 1;
    const L_CGI_WALL: usize = 2;
    const L_CGI_TAI: usize = 3;
    const L_CGI_SPLIT: usize = 4;
    const L_CGI_RETRY_M: usize = 5;
    const L_GV_TV: usize = 6;
    const L_GV_RETRY: usize = 7;
    const L_GV_STORE: usize = 8;
    const L_GV_TZ: usize = 9;
    const L_GV_TZF: usize = 10;
    const L_RES_OK: usize = 11;
    const L_RES_FILL: usize = 12;

    /// `li rd, imm` (any positive 32-bit value).
    fn emit_li(a: &mut Asm, rd: u32, v: u32) {
        let lo = ((v as i32) << 20) >> 20; // sign-extended low 12 bits
        let hi = ((v as i64 - lo as i64) >> 12) as u32;
        let mut used = false;
        if hi != 0 {
            a.emit(u_type(hi, rd, 0x37)); // lui
            used = true;
        }
        if lo != 0 || !used {
            if used {
                a.emit(i_type(lo, rd, 0, rd, 0x1B)); // addiw (32-bit, sign-ext)
            } else {
                a.emit(i_type(lo, ZERO, 0, rd, 0x13)); // addi from x0
            }
        }
    }

    /// Prologue: compute the DATA page base into t2. `pc_off` is the emitting
    /// function's offset inside the code page (auipc reads its own PC).
    fn emit_data_base(a: &mut Asm, pc_off: u32) {
        a.emit(u_type(0xFFFFF, T2, 0x17)); // auipc t2, -1 → data + pc_off
        emit_li(a, T6, pc_off);
        a.emit(r_type(0x20, T6, T2, 0, T2, 0x33)); // sub t2, t2, t6 → data base
    }

    /// Seqlock-read under `retry` label: t4 = base_cycles, t5 = base_mono_ns.
    /// Clobbers t3, a6.
    fn emit_seqlock_read(a: &mut Asm, retry: usize) {
        a.label(retry);
        a.emit(i_type(0, T2, 2, T3, 0x03)); // lw t3, 0(t2)     seq
        a.emit(i_type(1, T3, 7, T4, 0x13)); // andi t4, t3, 1
        a.br(bne_skel(T4, ZERO), retry); // bnez t4, retry
        a.emit(0x0FF0_000F); // fence iorw, iorw (acquire)
        a.emit(i_type(8, T2, 3, T4, 0x03)); // ld t4, 8(t2)    base_cycles
        a.emit(i_type(16, T2, 3, T5, 0x03)); // ld t5, 16(t2)  base_mono_ns
        a.emit(i_type(0, T2, 2, A6, 0x03)); // lw a6, 0(t2)    seq2
        a.br(bne_skel(T3, A6), retry); // bne t3, a6, retry
    }

    /// t5 += (rdtime - t4) * 100 (ns interpolation at 10 MHz).
    ///
    /// IMPORTANT: read the SAME counter the kernel snapshots into the data
    /// page (CLINT `time`, CSR 0xC01 — see vdso_data_tick). The old code used
    /// `rdcycle` (0xC00), a different counter whose rate only approximately
    /// matches the CLINT under QEMU TCG; the two drifted by seconds under
    /// host load and vdso time() ran away from kernel wall_secs() (mtime
    /// source) — LTP utime01/02/03 before/after windows failed. U-mode
    /// access is enabled by scounteren.TM set in arch init.
    fn emit_interpolate(a: &mut Asm) {
        a.emit(i_type(0xC01, ZERO, 2, A7, 0x73)); // rdtime a7
        a.emit(r_type(0x20, T4, A7, 0, A7, 0x33)); // sub a7, a7, t4
        a.emit(i_type(100, ZERO, 0, T1, 0x13)); // li t1, 100
        a.emit(r_type(1, T1, A7, 0, A7, 0x33)); // mul a7, a7, t1
        a.emit(r_type(0, A7, T5, 0, T5, 0x33)); // add t5, t5, a7
    }

    fn emit_ret(a: &mut Asm) {
        a.emit(i_type(0, RA, 0, ZERO, 0x67)); // ret
    }

    /// __vdso_clock_gettime(clk=a0, tp=a1).
    pub fn emit_clock_gettime(off: u32) -> alloc::vec::Vec<u32> {
        let mut a = Asm::new();
        emit_data_base(&mut a, off);
        a.emit(i_type(0, ZERO, 0, S2, 0x13)); // li s2, 0 (TAI extra secs)
        a.br(beq_skel(A0, ZERO), L_CGI_WALL); // REALTIME
        a.emit(i_type(1, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_MONO);
        a.emit(i_type(4, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_MONO);
        a.emit(i_type(7, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_MONO);
        a.emit(i_type(11, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_TAI);
        a.emit(i_type(-38, ZERO, 0, A0, 0x13)); // li a0, -ENOSYS
        emit_ret(&mut a);
        a.label(L_CGI_TAI);
        a.emit(i_type(37, ZERO, 0, S2, 0x13)); // li s2, 37
        a.label(L_CGI_WALL);
        emit_seqlock_read(&mut a, L_CGI_RETRY_W);
        emit_interpolate(&mut a);
        // Wall clock: add the ns-precise epoch offset to the interpolated
        // monotonic ns BEFORE the sec/nsec split (a seconds-only epoch lost
        // sub-second clock_settime deltas — LTP clock_settime01).
        a.emit(i_type(24, T2, 3, A6, 0x03)); // ld a6, 24(t2) epoch ns
        a.emit(r_type(0, A6, T5, 0, T5, 0x33)); // add t5, t5, a6
        a.emit(r_type(0, S2, ZERO, 0, A6, 0x33)); // add a6, s2, 0 (TAI secs)
        a.jal_rel(L_CGI_SPLIT);
        a.label(L_CGI_MONO);
        emit_seqlock_read(&mut a, L_CGI_RETRY_M);
        emit_interpolate(&mut a);
        a.emit(i_type(0, ZERO, 0, A6, 0x13)); // li a6, 0
        a.jal_rel(L_CGI_SPLIT);
        // L_SPLIT: ns → (sec + a6 extra, nsec), store timespec at (a1).
        a.label(L_CGI_SPLIT);
        emit_li(&mut a, T1, 1_000_000_000);
        a.emit(r_type(1, T1, T5, 5, A7, 0x33)); // divu a7, t5, t1
        a.emit(r_type(1, T1, T5, 7, T4, 0x33)); // remu t4, t5, t1
        a.emit(r_type(0, A6, A7, 0, A7, 0x33)); // add a7, a7, a6
        a.emit(s_type(0, A7, A1, 3, 0x23)); // sd a7, 0(a1)
        a.emit(s_type(8, T4, A1, 3, 0x23)); // sd t4, 8(a1)
        a.emit(i_type(0, ZERO, 0, A0, 0x13)); // li a0, 0
        emit_ret(&mut a);
        a.finish()
    }

    /// __vdso_gettimeofday(tv=a0, tz=a1).
    pub fn emit_gettimeofday(off: u32) -> alloc::vec::Vec<u32> {
        let mut a = Asm::new();
        emit_data_base(&mut a, off);
        a.br(bne_skel(A0, ZERO), L_GV_TV);
        a.jal_rel(L_GV_TZ);
        a.label(L_GV_TV);
        emit_seqlock_read(&mut a, L_GV_RETRY);
        emit_interpolate(&mut a);
        // Wall clock with ns-precise epoch: total ns → (sec, usec).
        a.emit(i_type(24, T2, 3, A6, 0x03)); // ld a6, 24(t2) epoch ns
        a.emit(r_type(0, A6, T5, 0, T5, 0x33)); // add t5, t5, a6
        emit_li(&mut a, T1, 1_000_000_000);
        a.emit(r_type(1, T1, T5, 5, T4, 0x33)); // divu t4, t5, t1 → sec
        a.emit(r_type(1, T1, T5, 7, A6, 0x33)); // remu a6, t5, t1 → rem ns
        a.emit(i_type(1000, ZERO, 0, T1, 0x13)); // li t1, 1000
        a.emit(r_type(1, T1, A6, 5, A6, 0x33)); // divu a6, a6, t1 → usec
        a.label(L_GV_STORE);
        a.emit(s_type(0, A6, A0, 3, 0x23)); // sd a6, 0(a0) tv_sec
        a.emit(s_type(8, T4, A0, 3, 0x23)); // sd t4, 8(a0) tv_usec
        a.label(L_GV_TZ);
        a.br(bne_skel(A1, ZERO), L_GV_TZF);
        a.emit(i_type(0, ZERO, 0, A0, 0x13)); // li a0, 0
        emit_ret(&mut a);
        a.label(L_GV_TZF);
        a.emit(i_type(32, T2, 2, T3, 0x03)); // lw t3, 32(t2) tz_minuteswest
        a.emit(s_type(0, T3, A1, 2, 0x23)); // sw t3, 0(a1)
        a.emit(s_type(4, ZERO, A1, 2, 0x23)); // sw x0, 4(a1)
        a.emit(i_type(0, ZERO, 0, A0, 0x13)); // li a0, 0
        emit_ret(&mut a);
        a.finish()
    }

    /// __vdso_clock_getres(clk=a0, res=a1): {0, 100ns}.
    pub fn emit_clock_getres(off: u32) -> alloc::vec::Vec<u32> {
        let mut a = Asm::new();
        emit_data_base(&mut a, off);
        for clk in [0u32, 1, 4, 7, 11] {
            a.emit(i_type(clk as i32, ZERO, 0, T0, 0x13));
            a.br(beq_skel(A0, T0), L_RES_OK);
        }
        a.emit(i_type(-38, ZERO, 0, A0, 0x13)); // li a0, -ENOSYS
        emit_ret(&mut a);
        a.label(L_RES_OK);
        a.br(bne_skel(A1, ZERO), L_RES_FILL);
        a.emit(i_type(0, ZERO, 0, A0, 0x13));
        emit_ret(&mut a);
        a.label(L_RES_FILL);
        a.emit(s_type(0, ZERO, A1, 3, 0x23)); // sd x0, 0(a1)  tv_sec = 0
        a.emit(i_type(100, ZERO, 0, T0, 0x13)); // li t0, 100
        a.emit(s_type(8, T0, A1, 3, 0x23)); // sd t0, 8(a1)  tv_nsec = 100
        a.emit(i_type(0, ZERO, 0, A0, 0x13));
        emit_ret(&mut a);
        a.finish()
    }
}

// ============================================================================
// x86_64 code page emitters (106th vDSO lever)
// ============================================================================
//
// Three position-dependent functions, hand-encoded System V AMD64:
//   __vdso_clock_gettime(rdi=clk, rsi=tp)   -> 0 / -ENOSYS
//   __vdso_gettimeofday (rdi=tv,  rsi=tz)   -> 0
//   __vdso_clock_getres (rdi=clk, rsi=res)  -> 0 / -ENOSYS
//
// Register contract per function: r11 = data page base (RIP-relative
// LEA, so the code page works at whatever VDSO_BASE it is mapped at),
// r10 = TAI extra seconds, rax/rcx/rdx/r8/r9 scratch. Only
// caller-saved registers are touched — no stack frame at all.
//
// Time math mirrors the kernel's TSC path (drivers/timer/x86_64.rs):
// LFENCE;RDTSC for an ordered counter read, then
//   ns = base_mono_ns + ((tsc - base_tsc) * mult) >> 32
// with mult/shift published per tick in the data page (offsets 40/44;
// shift is always 32, emitted as an immediate in SHRD).
#[cfg(feature = "x86_64")]
mod x86 {
    use alloc::vec::Vec;

    /// Byte assembler with rel8 label fixups (every branch in the three
    /// functions is short — the bodies are < 200 bytes).
    struct Asm {
        bytes: Vec<u8>,
        labels: [Option<usize>; 8],
        /// (insn index, label, byte position of the rel8 slot)
        fixups: Vec<(usize, usize, usize)>,
    }

    impl Asm {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                labels: [None; 8],
                fixups: Vec::new(),
            }
        }
        fn b(&mut self, bytes: &[u8]) {
            self.bytes.extend_from_slice(bytes);
        }
        fn label(&mut self, l: usize) {
            self.labels[l] = Some(self.bytes.len());
        }
        fn jcc(&mut self, op: u8, l: usize) {
            self.fixups.push((usize::MAX, l, self.bytes.len() + 1));
            self.b(&[op, 0]);
        }
        fn finish(mut self) -> Vec<u8> {
            for (_, l, at) in self.fixups.drain(..) {
                let target = self.labels[l].expect("vdso x86: undefined label") as i64;
                let rel = target - (at as i64 + 1);
                assert!(
                    (-128..=127).contains(&rel),
                    "vdso x86: rel8 out of range ({rel})"
                );
                self.bytes[at] = rel as u8;
            }
            self.bytes
        }
    }

    /// `lea r11, [rip - (0x1000 + off + 7)]` — r11 = the DATA page base
    /// (one page below this function's page).
    fn lea_data_base(a: &mut Asm, off: u32) {
        let d: i32 = -((off + 7 + 0x1000) as i32);
        a.b(&[0x4C, 0x8D, 0x1D]);
        a.b(&d.to_le_bytes());
    }

    // ---- shared instruction fragments (see the table above) ----
    fn seqlock_read(a: &mut Asm, retry: usize) {
        a.label(retry);
        a.b(&[0x41, 0x8B, 0x13]); // mov edx,[r11]      seq
        a.b(&[0xF6, 0xC2, 0x01]); // test dl,1
        a.jcc(0x75, retry); //     jne retry
        a.b(&[0x4D, 0x8B, 0x43, 0x08]); // mov r8,[r11+8]   base_tsc
        a.b(&[0x4D, 0x8B, 0x4B, 0x10]); // mov r9,[r11+16]  base_mono_ns
        a.b(&[0x49, 0x8B, 0x4B, 0x28]); // mov rcx,[r11+40] mult (inside the
        //                              seq window: mult can change when the
        //                              TSC is re-calibrated; a stale-mult
        //                              pairing is a torn read)
        a.b(&[0x41, 0x39, 0x13]); //     cmp [r11],edx    seq recheck
        a.jcc(0x75, retry); //     jne retry
    }
    /// rax = interpolated ns from (r8 base_tsc, r9 base_mono_ns, rcx mult).
    fn interpolate(a: &mut Asm) {
        a.b(&[0x0F, 0xAE, 0xE8]); // lfence (ordered rdtsc)
        a.b(&[0x0F, 0x31]); //     rdtsc
        a.b(&[0x48, 0xC1, 0xE2, 0x20]); // shl rdx,32
        a.b(&[0x48, 0x09, 0xD0]); //     or rax,rdx
        a.b(&[0x4C, 0x29, 0xC0]); //     sub rax,r8       delta tsc
        a.b(&[0x48, 0xF7, 0xE1]); //     mul rcx          rdx:rax
        a.b(&[0x48, 0x0F, 0xAC, 0xD0, 0x20]); // shrd rax,rdx,32
        a.b(&[0x4C, 0x01, 0xC8]); //     add rax,r9       + base_mono_ns
    }
    /// ns in rax -> timespec {rax+r10 sec, rdx nsec} stored at rsi.
    fn split_store(a: &mut Asm) {
        a.b(&[0xB9, 0x00, 0xCA, 0x9A, 0x3B]); // mov ecx,1000000000
        a.b(&[0x31, 0xD2]); //     xor edx,edx
        a.b(&[0x48, 0xF7, 0xF1]); // div rcx
        a.b(&[0x4C, 0x01, 0xD0]); // add rax,r10       TAI secs
        a.b(&[0x48, 0x89, 0x06]); // mov [rsi],rax
        a.b(&[0x48, 0x89, 0x56, 0x08]); // mov [rsi+8],rdx
        a.b(&[0x31, 0xC0]); //     xor eax,eax
        a.b(&[0xC3]); //     ret
    }
    fn dispatch(a: &mut Asm, wall: usize, mono: usize, tai: usize) {
        a.b(&[0x48, 0x85, 0xFF]); // test rdi,rdi     REALTIME(0)
        a.jcc(0x74, wall);
        a.b(&[0x48, 0x83, 0xFF, 0x01]); // cmp rdi,1  MONOTONIC
        a.jcc(0x74, mono);
        a.b(&[0x48, 0x83, 0xFF, 0x04]); // cmp rdi,4  MONOTONIC_RAW
        a.jcc(0x74, mono);
        a.b(&[0x48, 0x83, 0xFF, 0x07]); // cmp rdi,7  BOOTTIME
        a.jcc(0x74, mono);
        a.b(&[0x48, 0x83, 0xFF, 0x0B]); // cmp rdi,11 TAI
        a.jcc(0x74, tai);
        a.b(&[0xB8, 0xDA, 0xFF, 0xFF, 0xFF]); // mov eax,-38
        a.b(&[0xC3]); //     ret
    }

    const L_WALL: usize = 0;
    const L_MONO: usize = 1;
    const L_TAI: usize = 2;
    const L_RETRY_W: usize = 3;
    const L_RETRY_M: usize = 4;
    const L_SPLIT: usize = 5;

    pub fn emit_clock_gettime(off: u32) -> Vec<u8> {
        let mut a = Asm::new();
        lea_data_base(&mut a, off);
        a.b(&[0x45, 0x31, 0xD2]); // xor r10d,r10d   extra secs = 0
        dispatch(&mut a, L_WALL, L_MONO, L_TAI);
        a.label(L_TAI);
        a.b(&[0x41, 0xBA, 0x25, 0x00, 0x00, 0x00]); // mov r10d,37
        a.label(L_WALL);
        seqlock_read(&mut a, L_RETRY_W);
        interpolate(&mut a);
        a.b(&[0x49, 0x03, 0x43, 0x18]); // add rax,[r11+24]  + epoch ns
        a.b(&[0xEB, 0x00]); //     jmp L_SPLIT (fixup-free: patched below)
        // Patch the short jmp target manually (single use, no label
        // machinery needed): displacement to the split body.
        let split_at = a.bytes.len();
        a.label(L_MONO);
        seqlock_read(&mut a, L_RETRY_M);
        interpolate(&mut a);
        let after_mono = a.bytes.len();
        let rel = (after_mono as i64 - split_at as i64) as i8;
        a.bytes[split_at - 1] = rel as u8;
        a.label(L_SPLIT);
        split_store(&mut a);
        a.finish()
    }

    const L_TV: usize = 0;
    const L_RETRY: usize = 1;
    const L_TAIL: usize = 2;
    const L_RET0: usize = 3;

    pub fn emit_gettimeofday(off: u32) -> Vec<u8> {
        let mut a = Asm::new();
        lea_data_base(&mut a, off);
        a.b(&[0x48, 0x85, 0xFF]); // test rdi,rdi
        a.jcc(0x75, L_TV); //     jne L_TV
        // rdi == NULL: skip the tv store, do the tz tail.
        a.b(&[0xEB, 0x00]); //     jmp L_TAIL (patched below)
        let tail_jmp_at = a.bytes.len();
        a.label(L_TV);
        seqlock_read(&mut a, L_RETRY);
        interpolate(&mut a);
        a.b(&[0x49, 0x03, 0x43, 0x18]); // add rax,[r11+24]  + epoch ns
        a.b(&[0xB9, 0x00, 0xCA, 0x9A, 0x3B]); // mov ecx,1000000000
        a.b(&[0x31, 0xD2]); //     xor edx,edx
        a.b(&[0x48, 0xF7, 0xF1]); // div rcx          sec / rem-ns
        a.b(&[0x48, 0x89, 0x07]); // mov [rdi],rax    tv_sec
        a.b(&[0x48, 0x89, 0xD0]); // mov rax,rdx
        a.b(&[0x31, 0xD2]); //     xor edx,edx
        a.b(&[0xB9, 0xE8, 0x03, 0x00, 0x00]); // mov ecx,1000
        a.b(&[0x48, 0xF7, 0xF1]); // div rcx          usec
        a.b(&[0x48, 0x89, 0x47, 0x08]); // mov [rdi+8],rax tv_usec
        let tail_target = a.bytes.len();
        let rel = (tail_target as i64 - tail_jmp_at as i64) as i8;
        a.bytes[tail_jmp_at - 1] = rel as u8;
        a.label(L_TAIL);
        a.b(&[0x48, 0x85, 0xF6]); // test rsi,rsi
        a.jcc(0x74, L_RET0); //    je L_RET0
        a.b(&[0x48, 0xC7, 0x06, 0x00, 0x00, 0x00, 0x00]); // mov qword [rsi],0
        a.b(&[0x48, 0xC7, 0x46, 0x08, 0x00, 0x00, 0x00, 0x00]); // mov qword [rsi+8],0
        a.label(L_RET0);
        a.b(&[0x31, 0xC0]); //     xor eax,eax
        a.b(&[0xC3]); //     ret
        a.finish()
    }

    const L_OK: usize = 0;
    const L_RET0G: usize = 1;

    pub fn emit_clock_getres(off: u32) -> Vec<u8> {
        let mut a = Asm::new();
        lea_data_base(&mut a, off);
        for clk in [0u8, 1, 4, 7, 11] {
            if clk == 0 {
                a.b(&[0x48, 0x85, 0xFF]); // test rdi,rdi
                a.jcc(0x74, L_OK);
            } else {
                a.b(&[0x48, 0x83, 0xFF, clk]); // cmp rdi,clk
                a.jcc(0x74, L_OK);
            }
        }
        a.b(&[0xB8, 0xDA, 0xFF, 0xFF, 0xFF]); // mov eax,-38
        a.b(&[0xC3]); //     ret
        a.label(L_OK);
        a.b(&[0x48, 0x85, 0xF6]); // test rsi,rsi
        a.jcc(0x74, L_RET0G); //    je L_RET0G
        a.b(&[0x48, 0xC7, 0x06, 0x00, 0x00, 0x00, 0x00]); // mov qword [rsi],0
        a.b(&[0x41, 0x8B, 0x43, 0x30]); // mov eax,[r11+48] res_ns
        a.b(&[0x48, 0x89, 0x46, 0x08]); // mov [rsi+8],rax
        a.label(L_RET0G);
        a.b(&[0x31, 0xC0]); //     xor eax,eax
        a.b(&[0xC3]); //     ret
        a.finish()
    }
}

// ============================================================================
// ELF construction
// ============================================================================

/// SysV ELF hash (DT_HASH chains).
fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in name {
        h = (h << 4).wrapping_add(b as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// Build the code page: ELF header + phdrs + dynamic + DT_HASH + dynsym
/// + dynstr + the three functions.
fn build_code_page() -> [u8; 4096] {
    let mut page = [0u8; 4096];

    // ---- function bodies (fixed offsets) ----
    const F_CLOCK_GETTIME: u32 = 0x800;
    const F_GETTIMEOFDAY: u32 = 0x920;
    const F_CLOCK_GETRES: u32 = 0x9C0;
    #[cfg(not(feature = "x86_64"))]
    {
        let f1 = rv::emit_clock_gettime(F_CLOCK_GETTIME);
        let f2 = rv::emit_gettimeofday(F_GETTIMEOFDAY);
        let f3 = rv::emit_clock_getres(F_CLOCK_GETRES);
        let put = |page: &mut [u8; 4096], off: u32, words: &[u32]| {
            for (i, w) in words.iter().enumerate() {
                let p = off as usize + i * 4;
                page[p..p + 4].copy_from_slice(&w.to_le_bytes());
            }
        };
        put(&mut page, F_CLOCK_GETTIME, &f1);
        put(&mut page, F_GETTIMEOFDAY, &f2);
        put(&mut page, F_CLOCK_GETRES, &f3);
    }
    #[cfg(feature = "x86_64")]
    {
        let f1 = x86::emit_clock_gettime(F_CLOCK_GETTIME);
        let f2 = x86::emit_gettimeofday(F_GETTIMEOFDAY);
        let f3 = x86::emit_clock_getres(F_CLOCK_GETRES);
        let putb = |page: &mut [u8; 4096], off: u32, bytes: &[u8]| {
            let p = off as usize;
            page[p..p + bytes.len()].copy_from_slice(bytes);
        };
        putb(&mut page, F_CLOCK_GETTIME, &f1);
        putb(&mut page, F_GETTIMEOFDAY, &f2);
        putb(&mut page, F_CLOCK_GETRES, &f3);
    }

    // ---- dynstr @ 0x240 ----
    const STRTAB_OFF: usize = 0x240;
    const NAMES: [&str; 3] = [
        "__vdso_clock_gettime",
        "__vdso_gettimeofday",
        "__vdso_clock_getres",
    ];
    let mut name_offs = [0usize; 3];
    let mut strtab_len = 1usize; // leading NUL (null symbol name)
    // rwlock-livelock (Go vdso crash): the version strings live in the
    // same strtab — vda_name offsets into it.
    let mut vda_soname_off = 0usize;
    let mut vda_linux26_off = 0usize;
    {
        page[STRTAB_OFF] = 0;
        let mut p = STRTAB_OFF + 1;
        for (i, n) in NAMES.iter().enumerate() {
            name_offs[i] = p - STRTAB_OFF;
            for &b in n.as_bytes() {
                page[p] = b;
                p += 1;
            }
            page[p] = 0;
            p += 1;
        }
        vda_soname_off = p - STRTAB_OFF;
        for &b in b"linux-vdso.so.1" {
            page[p] = b;
            p += 1;
        }
        page[p] = 0;
        p += 1;
        vda_linux26_off = p - STRTAB_OFF;
        for &b in b"LINUX_2.6" {
            page[p] = b;
            p += 1;
        }
        page[p] = 0;
        p += 1;
        strtab_len = p - STRTAB_OFF;
    }

    // ---- SHT_GNU_versym @ 0x2A0: null sym -> 0, the 3 funcs -> ndx 2 ----
    // (LINUX_2.6, the only symbol version; mirrors the reference vDSO's
    // version-scripted dynsym.)
    const VERSYM_OFF: usize = 0x2A0;
    page[VERSYM_OFF..VERSYM_OFF + 2].copy_from_slice(&0u16.to_le_bytes());
    for i in 0..3 {
        let p = VERSYM_OFF + (i + 1) * 2;
        page[p..p + 2].copy_from_slice(&2u16.to_le_bytes());
    }

    // ---- SHT_GNU_verdef @ 0x2C0 (Go vdso crash fix, 2026-11-14) ----
    // Without DT_VERDEF, Go's runtime.vdsoFindVersion walks a NULL
    // verdef pointer and dies at its first clock read (snapd on the
    // ub2404 image: SEGV at vdsoFindVersion+0x33, cr2=0x2 — the
    // vd_flags load off def==nil). glibc is fine either way (an absent
    // DT_VERSYM makes check_match accept the symbol unversioned), but
    // Go REQUIRES the section to exist. Two entries, mirroring Linux
    // (arch/x86/entry/vdso): ndx 1 = BASE soname, ndx 2 = LINUX_2.6 —
    // the version vdsoFindVersion matches for its symbol lookups.
    const VERDEF_OFF: usize = 0x2C0;
    let h_linux26 = elf_hash(b"LINUX_2.6");
    let h_soname = elf_hash(b"linux-vdso.so.1");
    let mut put_vd = |page: &mut [u8; 4096], at: usize, ver: u16, flags: u16, ndx: u16, cnt: u16, hash: u32, aux: u32, next: u32, name: u32| {
        page[at..at + 2].copy_from_slice(&ver.to_le_bytes());
        page[at + 2..at + 4].copy_from_slice(&flags.to_le_bytes());
        page[at + 4..at + 6].copy_from_slice(&ndx.to_le_bytes());
        page[at + 6..at + 8].copy_from_slice(&cnt.to_le_bytes());
        page[at + 8..at + 12].copy_from_slice(&hash.to_le_bytes());
        page[at + 12..at + 16].copy_from_slice(&aux.to_le_bytes());
        page[at + 16..at + 20].copy_from_slice(&next.to_le_bytes());
        let vd_aux_at = at + aux as usize;
        page[vd_aux_at..vd_aux_at + 4].copy_from_slice(&name.to_le_bytes());
        page[vd_aux_at + 4..vd_aux_at + 8].copy_from_slice(&0u32.to_le_bytes());
    };
    // Entry 1 @ +0: BASE soname; aux (Verdaux) at +20; next entry at +28.
    put_vd(
        &mut page,
        VERDEF_OFF,
        1,    // vd_version
        1,    // vd_flags = VER_FLG_BASE
        1,    // vd_ndx
        1,    // vd_cnt
        h_soname,
        20,   // vd_aux: Verdaux directly after this Verdef
        28,   // vd_next: entry2 follows aux (20 + 8)
        vda_soname_off as u32,
    );
    // Entry 2 @ +28: LINUX_2.6 (the matchable version), last in chain.
    put_vd(
        &mut page,
        VERDEF_OFF + 28,
        1, // vd_version
        0, // vd_flags: no BASE — vdsoFindVersion matches this one
        2, // vd_ndx (what DT_VERSYM words above reference)
        1, // vd_cnt
        h_linux26,
        20, // vd_aux
        0,  // vd_next: end of chain
        vda_linux26_off as u32,
    );

    // ---- dynsym @ 0x1C0: null + 3 STT_GLOBAL FUNC syms (24B each) ----
    const SYMTAB_OFF: usize = 0x1C0;
    let sym_values = [F_CLOCK_GETTIME, F_GETTIMEOFDAY, F_CLOCK_GETRES];
    for i in 0..3 {
        let p = SYMTAB_OFF + (i + 1) * 24;
        page[p..p + 4].copy_from_slice(&(name_offs[i] as u32).to_le_bytes());
        page[p + 4] = 0x12; // (STB_GLOBAL << 4) | STT_FUNC
        page[p + 6..p + 8].copy_from_slice(&1u16.to_le_bytes()); // st_shndx
        page[p + 8..p + 16].copy_from_slice(&(sym_values[i] as u64).to_le_bytes());
        // st_size 0 (informational)
    }

    // ---- DT_HASH @ 0x140: nbucket=2, nchain=4, bucket[2], chain[4] ----
    // TERMINATOR IS 0, never 0xFFFFFFFF (106th fix): SysV hash lookups
    // walk `for (i = bucket[h % nbucket]; i; i = chain[i])` — 0 (STN_UNDEF)
    // is the empty-bucket and end-of-chain marker (musl ldso and glibc
    // both). The first cut used u32::MAX sentinels and pid 1 segfaulted
    // at its first vdso lookup: chain 0xFFFFFFFF * sizeof(Elf64_Sym)=24
    // = 0x17_FFFF_FFE8 past the symtab — the exact CR2 delta decoded
    // from the USEGV-FOR dump (ripbytes were musl's hash loop). The
    // u32::MAX convention had never been exercised: the riscv64 mapping
    // it was written for stayed disabled.
    const HASH_OFF: usize = 0x180;
    page[HASH_OFF..HASH_OFF + 4].copy_from_slice(&2u32.to_le_bytes());
    page[HASH_OFF + 4..HASH_OFF + 8].copy_from_slice(&4u32.to_le_bytes());
    // buckets and chains stay ZERO-initialized (0 = empty/end).
    for i in 0..3 {
        let b = (elf_hash(NAMES[i].as_bytes()) % 2) as usize;
        let bucket = u32::from_le_bytes(
            page[HASH_OFF + 8 + b * 4..HASH_OFF + 12 + b * 4].try_into().unwrap(),
        );
        if bucket == 0 {
            page[HASH_OFF + 8 + b * 4..HASH_OFF + 12 + b * 4]
                .copy_from_slice(&((i + 1) as u32).to_le_bytes());
        } else {
            let mut cur = bucket as usize;
            loop {
                let next = u32::from_le_bytes(
                    page[HASH_OFF + 16 + cur * 4..HASH_OFF + 20 + cur * 4].try_into().unwrap(),
                );
                if next == 0 {
                    page[HASH_OFF + 16 + cur * 4..HASH_OFF + 20 + cur * 4]
                        .copy_from_slice(&((i + 1) as u32).to_le_bytes());
                    break;
                }
                cur = next as usize;
            }
        }
    }

    // ---- PT_DYNAMIC @ 0xC0 (9 entries + NULL × 16B = 160B) ----
    // 0xC0+160 = 0x160, clear of HASH @ 0x180 (the 3 added version
    // tags pushed the old 96-byte table into the hash table's slot).
    const DYN_OFF: usize = 0xC0;
    // DT_STRTAB/DT_SYMTAB values were SWAPPED until 2026-11-14
    // (arena-mmap lane): DT_SYMTAB pointed at the string table, so
    // every consumer that resolves symbols through the dynamic table
    // (musl __vdsosym — clock_gettime/gettimeofday fast paths) parsed
    // string bytes as Elf64_Sym entries, matched nothing, and silently
    // fell back to the real syscalls. The clock_gettime fallback turned
    // the FFRT arena pattern's user-mode vDSO store (SEGV → handler
    // installs the page → retry succeeds, reference-kernel behavior)
    // into a plain syscall EFAULT instead — one root of the OH mmi
    // service death family (rip inside the libc time path storing to a
    // PROT_NONE arena page).
    let dyn_entries: [(u64, u64); 10] = [
        (4, HASH_OFF as u64),   // DT_HASH
        (5, STRTAB_OFF as u64), // DT_STRTAB
        (6, SYMTAB_OFF as u64), // DT_SYMTAB
        (10, strtab_len as u64), // DT_STRSZ
        (11, 24),               // DT_SYMENT
        // Version tags (Go vdso crash fix): Go's runtime.vdsoFindVersion
        // dereferences DT_VERDEF unconditionally — a missing section is a
        // nil pointer and every Go binary dies at its first clock read
        // (snapd: SEGV at vdsoFindVersion+0x33, cr2=0x2). glibc is
        // indifferent (unversioned symbols are accepted when DT_VERSYM
        // is absent; with it present the LINUX_2.6 match is exact).
        // Tag values (elf.h): VERDEF 0x6ffffffc, VERDEFNUM 0x6ffffffd,
        // VERSYM 0x6ffffff0. Boot H caught the first cut using 0x6ffffffe
        // (DT_VERNEED!) for VERSYM: ld.so parses the vDSO's dynamic table
        // during startup dependency processing, read the versym array as
        // a Verneed with vn_version garbage and refused to load init
        // ("unsupported version 0 of Verneed record" -> poweroff).
        (0x6ffffffc, VERDEF_OFF as u64),   // DT_VERDEF
        (0x6ffffffd, 2),                   // DT_VERDEFNUM
        (0x6ffffff0, VERSYM_OFF as u64),   // DT_VERSYM
        (0, 0),                            // DT_NULL
        (0, 0),                            // DT_NULL (padding keeps 160B)
    ];
    for (i, (tag, val)) in dyn_entries.iter().enumerate() {
        let p = DYN_OFF + i * 16;
        page[p..p + 8].copy_from_slice(&tag.to_le_bytes());
        page[p + 8..p + 16].copy_from_slice(&val.to_le_bytes());
    }

    // ---- program headers @ 0x40 (Elf64_Phdr = 56 bytes each) ----
    const PHDR_OFF: usize = 0x40;
    let mut put_ph = |page: &mut [u8; 4096], p: usize, t: u32, fl: u32, o: u64, v: u64, fs: u64, ms: u64, al: u64| {
        page[p..p + 4].copy_from_slice(&t.to_le_bytes());
        page[p + 4..p + 8].copy_from_slice(&fl.to_le_bytes());
        page[p + 8..p + 16].copy_from_slice(&o.to_le_bytes());
        page[p + 16..p + 24].copy_from_slice(&v.to_le_bytes());
        page[p + 24..p + 32].copy_from_slice(&v.to_le_bytes()); // paddr = vaddr
        page[p + 32..p + 40].copy_from_slice(&fs.to_le_bytes());
        page[p + 40..p + 48].copy_from_slice(&ms.to_le_bytes());
        page[p + 48..p + 56].copy_from_slice(&al.to_le_bytes());
    };
    // PT_LOAD: whole page, R+X, vaddr 0 (load bias = AT_SYSINFO_EHDR).
    put_ph(&mut page, PHDR_OFF, 1, 5, 0, 0, 4096, 4096, 4096);
    // PT_DYNAMIC: R, at DYN_OFF.
    put_ph(
        &mut page,
        PHDR_OFF + 56,
        2,
        4,
        DYN_OFF as u64,
        DYN_OFF as u64,
        160,
        160,
        8,
    );

    // ---- ELF64 header ----
    {
        let e = &mut page[0..64];
        e[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        e[4] = 2; // ELFCLASS64
        e[5] = 1; // ELFDATA2LSB
        e[6] = 1; // EV_CURRENT
        e[7] = 0; // ELFOSABI_NONE
        e[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        // e_machine must match the target: loaders sanity-check it against
        // their own arch before trusting the vDSO image. Mirrors
        // fs::elf::Elf64Ehdr::check_machine.
        #[cfg(feature = "aarch64")]
        let e_machine: u16 = crate::fs::elf::ElfMachine::EM_AARCH64 as u16;
        #[cfg(feature = "riscv64")]
        let e_machine: u16 = crate::fs::elf::ElfMachine::EM_RISCV as u16;
        #[cfg(feature = "x86_64")]
        let e_machine: u16 = crate::fs::elf::ElfMachine::EM_X86_64 as u16;
        e[18..20].copy_from_slice(&e_machine.to_le_bytes());
        e[20..24].copy_from_slice(&1u32.to_le_bytes()); // version
        e[24..32].copy_from_slice(&(F_CLOCK_GETTIME as u64).to_le_bytes()); // entry
        e[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
        e[40..48].copy_from_slice(&0u64.to_le_bytes()); // shoff
        // e_flags 0 is honest on all three targets: riscv 0 = base ISA with
        // soft-float conventions (the emitted code is integer-only, so no
        // float-ABI byte matters), and aarch64/x86_64 define no e_flags
        // bits at all.
        e[48..52].copy_from_slice(&0u32.to_le_bytes()); // flags
        e[52..54].copy_from_slice(&64u16.to_le_bytes()); // ehsize
        e[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
        e[56..58].copy_from_slice(&2u16.to_le_bytes()); // phnum
        e[58..60].copy_from_slice(&64u16.to_le_bytes()); // shentsize
        e[60..62].copy_from_slice(&0u16.to_le_bytes()); // shnum
        e[62..64].copy_from_slice(&0u16.to_le_bytes()); // shstrndx
    }

    page
}

// ============================================================================
// Init
// ============================================================================

/// Build the vDSO ELF code page and prime the data page. Called once
/// during boot (single-threaded) before the first execve.
pub fn vdso_init() {
    // SAFETY: boot-time single-threaded init; the code page is only read
    // afterwards (user mappings are R+X), and the data page writes are
    // seqlock-guarded.
    unsafe {
        VDSO_CODE_PAGE.0 = build_code_page();
    }
    // Prime the snapshot so pre-first-tick readers see sane values.
    vdso_data_tick();
}
