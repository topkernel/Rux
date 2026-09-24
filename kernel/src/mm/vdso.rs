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
//!   24  u64  wall_epoch_s   REALTIME = monotonic + this
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
pub const VDSO_BASE: u64 = 0x3FBE_C000_0000;

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

/// Seqlock word (offset 0 of the data page). Odd = writer in progress.
static VDSO_SEQ: AtomicU32 = AtomicU32::new(0);

/// Refresh the data page snapshot (timer tick / clock-set paths).
///
/// Seqlock protocol: seq |= 1 (begin) → write fields → seq += 1 (end,
/// even). Readers retry while seq is odd or changed across the read.
pub fn vdso_data_tick() {
    let _ = VDSO_SEQ.fetch_or(1, Ordering::Release);
    let cycles = crate::drivers::timer::read_time();
    // 10 MHz source: 100 ns per cycle, exact.
    let mono_ns = cycles.saturating_mul(100);
    let epoch = crate::syscall::time::wall_epoch_offset_secs();
    // SAFETY: the page is only written here under the seqlock; user
    // readers validate via the seq word before trusting the fields.
    unsafe {
        let base = VDSO_DATA_PAGE.0.as_mut_ptr();
        core::ptr::write_volatile(base.add(8) as *mut u64, cycles);
        core::ptr::write_volatile(base.add(16) as *mut u64, mono_ns);
        core::ptr::write_volatile(base.add(24) as *mut u64, epoch);
        // tz fields (32/36) stay zero.
    }
    VDSO_SEQ.fetch_add(1, Ordering::Release);
}

/// Physical addresses of the two vDSO pages (data, code) for the exec
/// mapping. `vdso_init()` must have run (code page built).
pub fn vdso_pages_phys() -> (u64, u64) {
    let data_va = core::ptr::addr_of!(VDSO_DATA_PAGE) as *const u8 as u64;
    let code_va = core::ptr::addr_of!(VDSO_CODE_PAGE) as *const u8 as u64;
    let d = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(data_va),
    );
    let c = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(code_va),
    );
    (d.bits(), c.bits())
}

// ============================================================================
// Code page — mini RV64 assembler
// ============================================================================

/// The vDSO ELF page (4 KiB aligned), built once by `vdso_init()`.
#[repr(align(4096))]
struct VdsoCodePage([u8; 4096]);

static mut VDSO_CODE_PAGE: VdsoCodePage = VdsoCodePage([0; 4096]);

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

/// t5 += (rdcycle - t4) * 100 (ns interpolation at 10 MHz).
fn emit_interpolate(a: &mut Asm) {
    a.emit(i_type(0xC00, ZERO, 2, A7, 0x73)); // rdcycle a7
    a.emit(r_type(0x20, T4, A7, 0, A7, 0x33)); // sub a7, a7, t4
    a.emit(i_type(100, ZERO, 0, T1, 0x13)); // li t1, 100
    a.emit(r_type(1, T1, A7, 0, A7, 0x33)); // mul a7, a7, t1
    a.emit(r_type(0, A7, T5, 0, T5, 0x33)); // add t5, t5, a7
}

fn emit_ret(a: &mut Asm) {
    a.emit(i_type(0, RA, 0, ZERO, 0x67)); // ret
}

/// __vdso_clock_gettime(clk=a0, tp=a1).
fn emit_clock_gettime(off: u32) -> alloc::vec::Vec<u32> {
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
    a.emit(i_type(24, T2, 3, A6, 0x03)); // ld a6, 24(t2) epoch secs
    a.emit(r_type(0, S2, A6, 0, A6, 0x33)); // add a6, a6, s2
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
fn emit_gettimeofday(off: u32) -> alloc::vec::Vec<u32> {
    let mut a = Asm::new();
    emit_data_base(&mut a, off);
    a.br(bne_skel(A0, ZERO), L_GV_TV);
    a.jal_rel(L_GV_TZ);
    a.label(L_GV_TV);
    emit_seqlock_read(&mut a, L_GV_RETRY);
    emit_interpolate(&mut a);
    a.emit(i_type(24, T2, 3, A6, 0x03)); // ld a6, 24(t2) epoch secs
    a.emit(i_type(1000, ZERO, 0, T1, 0x13)); // li t1, 1000
    a.emit(r_type(1, T1, T5, 5, T4, 0x33)); // divu t4, t5, t1 → usec
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
fn emit_clock_getres(off: u32) -> alloc::vec::Vec<u32> {
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
    let f1 = emit_clock_gettime(F_CLOCK_GETTIME);
    let f2 = emit_gettimeofday(F_GETTIMEOFDAY);
    let f3 = emit_clock_getres(F_CLOCK_GETRES);
    let put = |page: &mut [u8; 4096], off: u32, words: &[u32]| {
        for (i, w) in words.iter().enumerate() {
            let p = off as usize + i * 4;
            page[p..p + 4].copy_from_slice(&w.to_le_bytes());
        }
    };
    put(&mut page, F_CLOCK_GETTIME, &f1);
    put(&mut page, F_GETTIMEOFDAY, &f2);
    put(&mut page, F_CLOCK_GETRES, &f3);

    // ---- dynstr @ 0x200 ----
    const STRTAB_OFF: usize = 0x200;
    const NAMES: [&str; 3] = [
        "__vdso_clock_gettime",
        "__vdso_gettimeofday",
        "__vdso_clock_getres",
    ];
    let mut name_offs = [0usize; 3];
    let mut strtab_len = 1usize; // leading NUL (null symbol name)
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
        strtab_len = p - STRTAB_OFF;
    }

    // ---- dynsym @ 0x180: null + 3 STT_GLOBAL FUNC syms (24B each) ----
    const SYMTAB_OFF: usize = 0x180;
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
    const HASH_OFF: usize = 0x140;
    page[HASH_OFF..HASH_OFF + 4].copy_from_slice(&2u32.to_le_bytes());
    page[HASH_OFF + 4..HASH_OFF + 8].copy_from_slice(&4u32.to_le_bytes());
    for i in 0..2 {
        page[HASH_OFF + 8 + i * 4..HASH_OFF + 12 + i * 4]
            .copy_from_slice(&u32::MAX.to_le_bytes());
    }
    for i in 0..4 {
        page[HASH_OFF + 16 + i * 4..HASH_OFF + 20 + i * 4]
            .copy_from_slice(&u32::MAX.to_le_bytes());
    }
    for i in 0..3 {
        let b = (elf_hash(NAMES[i].as_bytes()) % 2) as usize;
        let bucket = u32::from_le_bytes(
            page[HASH_OFF + 8 + b * 4..HASH_OFF + 12 + b * 4].try_into().unwrap(),
        );
        if bucket == u32::MAX {
            page[HASH_OFF + 8 + b * 4..HASH_OFF + 12 + b * 4]
                .copy_from_slice(&((i + 1) as u32).to_le_bytes());
        } else {
            let mut cur = bucket as usize;
            loop {
                let next = u32::from_le_bytes(
                    page[HASH_OFF + 16 + cur * 4..HASH_OFF + 20 + cur * 4].try_into().unwrap(),
                );
                if next == u32::MAX {
                    page[HASH_OFF + 16 + cur * 4..HASH_OFF + 20 + cur * 4]
                        .copy_from_slice(&((i + 1) as u32).to_le_bytes());
                    break;
                }
                cur = next as usize;
            }
        }
    }

    // ---- PT_DYNAMIC @ 0xC0 (6 entries × 16B = 96B) ----
    const DYN_OFF: usize = 0xC0;
    let dyn_entries: [(u64, u64); 6] = [
        (4, HASH_OFF as u64),   // DT_HASH
        (5, SYMTAB_OFF as u64), // DT_STRTAB
        (6, STRTAB_OFF as u64), // DT_SYMTAB
        (10, strtab_len as u64), // DT_STRSZ
        (11, 24),               // DT_SYMENT
        (0, 0),                 // DT_NULL
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
        96,
        96,
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
        e[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
        e[20..24].copy_from_slice(&1u32.to_le_bytes()); // version
        e[24..32].copy_from_slice(&(F_CLOCK_GETTIME as u64).to_le_bytes()); // entry
        e[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
        e[40..48].copy_from_slice(&0u64.to_le_bytes()); // shoff
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
