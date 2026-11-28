//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Stack Trace and Register Dump
//!
//! Extracted from the inline panic handler in `main.rs`.
//! Provides reusable `dump_stack()`, `dump_regs()`, and `dump_csrs()` APIs
//! for use by WARN, BUG, softlockup, hung_task, and other DFX modules.
//!
//! All output goes directly to UART via the lockless batched writer
//! (bypasses printk ring buffer), making these safe to call from
//! crash/panic contexts.

#![forbid(unsafe_code)]
use core::fmt;
use core::fmt::Write;

// ============================================================================
// ConsoleWriter — lockless UART output for crash/panic contexts
// ============================================================================

/// Lockless console writer that bypasses printk and CONSOLE_LOGLEVEL.
///
/// Writes staged bytes through the lockless batched TX path. Safe to
/// use in panic/OOPS contexts where normal printk infrastructure may be
/// unavailable or recursive.
pub struct ConsoleWriter {
    /// Whether this writer holds the printk console emit lock (bounded
    /// best-effort acquire in `new()`): keeps diagnostic reports from
    /// interleaving mid-line with an in-flight printk record on another
    /// CPU (OH b140: "…len BUG: soft lockup" garble). On timeout —
    /// e.g. a wedged holder, or this CPU's own interrupted task holds
    /// the lock — output proceeds locklessly exactly as before.
    emit_locked: bool,
    /// Staged-byte buffer (perf-console): write_str fragments coalesce
    /// here and flush through ONE batched TX pass per 128 bytes instead
    /// of one register-IO pair per byte — a softlockup stack dump is
    /// thousands of bytes through this writer.
    buf: [u8; 128],
    len: usize,
}

impl ConsoleWriter {
    pub fn new() -> Self {
        ConsoleWriter {
            emit_locked: crate::printk::try_console_emit_lock(),
            buf: [0u8; 128],
            len: 0,
        }
    }

    /// Flush staged bytes through the lockless batched TX path
    /// (putchar_no_lock's batched sibling — safe in crash contexts).
    fn flush(&mut self) {
        if self.len > 0 {
            let uart = crate::console::Uart::new();
            uart.puts_batch(&self.buf[..self.len]);
            self.len = 0;
        }
    }
}

impl Drop for ConsoleWriter {
    fn drop(&mut self) {
        self.flush();
        if self.emit_locked {
            crate::printk::console_emit_unlock();
        }
    }
}

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                // Newline expansion (the OPOST discipline of the old
                // per-byte path) staged as the translated pair.
                if self.len + 2 > self.buf.len() {
                    self.flush();
                }
                self.buf[self.len] = b'\r';
                self.buf[self.len + 1] = b'\n';
                self.len += 2;
            } else {
                if self.len == self.buf.len() {
                    self.flush();
                }
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

// ============================================================================
// Frame Pointer Stack Walk
// ============================================================================

/// Read the current frame pointer register (`s0` on riscv64, `rbp` on
/// x86_64). The raw register reads are the audited edge's INV-FOR-4
/// family (forensics_edges::fp_read_raw).
#[inline]
pub fn current_frame_pointer() -> u64 {
    crate::debug::forensics_edges::fp_read_raw()
}

/// Return address of the calling function (for "current" trace
/// entries). Raw reads are the audited edge's INV-FOR-4 family
/// (forensics_edges::caller_ra_read_raw).
#[inline]
pub(crate) fn caller_return_address() -> u64 {
    crate::debug::forensics_edges::caller_ra_read_raw()
}

/// Unwind one frame: given a frame pointer, return (return_addr, next_fp).
///
/// riscv64 frame layout: [fp] = saved_fp, [fp+8] = return_addr.
/// x86_64 frame layout:  [rbp] = saved_rbp, [rbp+8] = return_addr.
/// The frame-word reads are the audited edge's INV-FOR-2 arm.
#[inline]
fn unwind_frame(fp: u64) -> Option<(u64, u64)> {
    crate::debug::forensics_edges::unwind_frame_raw(fp)
}

/// Walk frame pointers and call the callback for each frame.
///
/// Reads the current frame pointer via inline asm, then iterates the frame
/// pointer chain: `[fp]` = saved_fp, `[fp+8]` = return_addr.
///
/// # Arguments
/// * `cb` - Callback called as `cb(program_counter, frame_pointer)` for each frame.
///
/// # Safety
/// Reads from memory via frame pointer chain. Caller must ensure the stack
/// is in a valid state (not corrupted). Validates alignment and address range.
pub fn walk_stack_trace(cb: &mut dyn FnMut(u64, u64)) {
    let mut fp: u64 = current_frame_pointer();

    let mut frame_count = 0u32;
    while fp != 0 && frame_count < 32 {
        // Validate fp: must be 8-byte aligned and inside a window the
        // kernel actually maps — the old `>= 0x8000_0000` lower bound
        // let a junk frame pointer like 0x4f30_0000_edfe_0dd8 through (8
        // aligned, inside the giant band, but unmapped): the WARN path
        // faulted the kernel mid-print (KERNPANIC re-entered during
        // dump, coverage-suite boot 2026-11-28). Two windows are legal
        // per arch: the IMAGE half (boot/init stacks + text) and the
        // DIRECT MAP (task stacks from the compact-stack slab / heap
        // live here). Per-arch constants mirror the memory-layout
        // modules' PAGE_OFFSET / KERNEL_LINK_ADDR. Anything else cannot
        // be a frame.
        #[cfg(feature = "riscv64")]
        {
            const IMG_LO: u64 = 0xffff_ffff_8000_0000;
            const IMG_HI: u64 = 0xffff_ffff_8200_0000;
            const MAP_LO: u64 = 0xffff_ffd6_0000_0000;
            const MAP_HI: u64 = 0xffff_ffe0_0000_0000; // 64 GiB direct map
            let ok = (fp >= IMG_LO && fp < IMG_HI) || (fp >= MAP_LO && fp < MAP_HI);
            if !ok || fp % 8 != 0 {
                break;
            }
        }
        #[cfg(feature = "x86_64")]
        {
            const IMG_LO: u64 = 0xffff_ffff_8000_0000;
            const IMG_HI: u64 = 0xffff_ffff_8200_0000;
            const MAP_LO: u64 = 0xffff_8880_0000_0000;
            const MAP_HI: u64 = 0xffff_c880_0000_0000; // 64 TiB direct map
            let ok = (fp >= IMG_LO && fp < IMG_HI) || (fp >= MAP_LO && fp < MAP_HI);
            if !ok || fp % 8 != 0 {
                break;
            }
        }
        #[cfg(feature = "aarch64")]
        {
            const IMG_LO: u64 = 0xffff_ffff_8000_0000;
            const IMG_HI: u64 = 0xffff_ffff_ff00_0000;
            const MAP_LO: u64 = 0xffff_c000_0000_0000;
            const MAP_HI: u64 = 0xffff_e000_0000_0000; // linear map half
            let ok = (fp >= IMG_LO && fp < IMG_HI) || (fp >= MAP_LO && fp < MAP_HI);
            if !ok || fp % 8 != 0 {
                break;
            }
        }
        #[cfg(not(any(feature = "riscv64", feature = "x86_64", feature = "aarch64")))]
        {
            let _ = fp;
            break;
        }

        if let Some((ret_addr, next_fp)) = unwind_frame(fp) {
            cb(ret_addr, fp);

            // Check if next fp is valid (should be > current fp or 0)
            if next_fp <= fp {
                break;
            }
            fp = next_fp;
        } else {
            break;
        }
        frame_count += 1;
    }
}

/// Print formatted stack trace to console (bypasses printk ring buffer).
///
/// Output format:
/// ```text
/// Call trace:
///   [<ffffffff80001234>] (current)
///   [<ffffffff80005678>]
///   [<ffffffff80009abc>]
/// ```
pub fn dump_stack() {
    let mut w = ConsoleWriter::new();

    // Print current ra first
    let ra: u64 = caller_return_address();

    // Linux's classic frame format: the address, then the function name
    // with the offset and the function size (Linux 4.x and still what
    // most people read); "?" when the address resolves to no symbol.
    let _ = w.write_str("Call Trace:\n");
    let _ = write!(
        w,
        "  [<{:016x}>] {} (current)\n",
        ra,
        crate::debug::ksyms::format_symbol(ra)
    );

    walk_stack_trace(&mut |pc, _fp| {
        let _ = write!(
            w,
            "  [<{:016x}>] {}\n",
            pc,
            crate::debug::ksyms::format_symbol(pc)
        );
    });

    let _ = w.write_str("\n");
}

// ============================================================================
// Register and CSR Dump
// ============================================================================

/// Register names in order matching `save_regs()` layout
#[cfg(feature = "riscv64")]
const REG_NAMES: [&str; 31] = [
    "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0",
    "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
    "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8",
    "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

/// x86_64: only the callee-saved GPRs are captured (slots 0..6); the
/// remaining slots stay zero.
#[cfg(feature = "x86_64")]
const REG_NAMES: [&str; 31] = [
    "rbx", "rbp", "r12", "r13", "r14", "r15", "", "",
    "", "", "", "", "", "", "", "",
    "", "", "", "", "", "", "", "",
    "", "", "", "", "", "", "",
];

/// aarch64: save_regs captures x0..x30 in slot order
#[cfg(feature = "aarch64")]
const REG_NAMES: [&str; 31] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7",
    "x8", "x9", "x10", "x11", "x12", "x13", "x14", "x15",
    "x16", "x17", "x18", "x19", "x20", "x21", "x22", "x23",
    "x24", "x25", "x26", "x27", "x28", "x29", "x30",
];

/// aarch64: capture x0..x30 into the dump array (x0 at slot 0).
/// The register spill is the audited edge's INV-FOR-4 family.
#[cfg(feature = "aarch64")]
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
    crate::debug::forensics_edges::save_regs_raw(&mut regs);
    regs
}

/// Save all integer registers to a stack buffer via inline assembly.
///
/// Returns an array of 31 u64 values representing x1..x31.
/// The register spill is the audited edge's INV-FOR-4 family.
#[cfg(feature = "riscv64")]
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
    crate::debug::forensics_edges::save_regs_raw(&mut regs);
    regs
}

/// x86_64 register capture: spill the callee-saved GPRs (rbx is reserved
/// in LLVM, so push/spill/pop is the reliable way to read it).
/// The register spill is the audited edge's INV-FOR-4 family.
#[cfg(feature = "x86_64")]
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
    crate::debug::forensics_edges::save_regs_raw(&mut regs);
    regs
}

/// Print register dump captured via inline assembly.
///
/// Captures all integer registers (x1..x31) and prints them 4 per line.
pub fn dump_regs_inline() {
    let regs = save_regs();
    dump_regs(&regs);
}

/// Print register dump from a saved array using the global REG_NAMES.
pub fn dump_regs(regs: &[u64; 31]) {
    let mut w = ConsoleWriter::new();

    let _ = w.write_str("Registers:\n");
    for (i, chunk) in regs.chunks(4).enumerate() {
        let _ = w.write_str("  ");
        for (j, val) in chunk.iter().enumerate() {
            let name_idx = i * 4 + j;
            let _ = write!(w, "{:4}: {:016x}  ", REG_NAMES[name_idx], val);
        }
        let _ = w.write_str("\n");
    }
    let _ = w.write_str("\n");
}

/// Read and print CSR state (sstatus, scause, stval, sepc).
/// The CSR reads are the audited edge's INV-FOR-4 family.
#[cfg(feature = "riscv64")]
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();

    let (sstatus, scause, stval, sepc) = crate::debug::forensics_edges::trap_regs_read_raw();

    let _ = write!(w, "Sstatus: {:016x}\n", sstatus);
    let _ = write!(w, "Scause : {:016x}\n", scause);
    let _ = write!(w, "Stval  : {:016x}\n", stval);
    let _ = write!(w, "Sepc   : {:016x}\n\n", sepc);
}

/// x86_64: print the fault/control registers instead of RISC-V CSRs.
#[cfg(feature = "x86_64")]
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();
    let _ = write!(w, "CR2    : {:016x}\n", crate::arch::cpu::read_cr2());
    let _ = write!(w, "CR3    : {:016x}\n\n", crate::arch::cpu::read_cr3());
}

/// aarch64: print the exception syndrome registers instead of RISC-V CSRs.
/// The system-register reads are the audited edge's INV-FOR-4 family.
#[cfg(feature = "aarch64")]
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();
    let (esr, elr, far, spsr) = crate::debug::forensics_edges::trap_regs_read_raw();
    let _ = write!(w, "ESR    : {:016x}\n", esr);
    let _ = write!(w, "ELR    : {:016x}\n", elr);
    let _ = write!(w, "FAR    : {:016x}\n", far);
    let _ = write!(w, "SPSR   : {:016x}\n\n", spsr);
}
