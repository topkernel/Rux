//! Stack Trace and Register Dump
//!
//! Extracted from the inline panic handler in `main.rs`.
//! Provides reusable `dump_stack()`, `dump_regs()`, and `dump_csrs()` APIs
//! for use by WARN, BUG, softlockup, hung_task, and other DFX modules.
//!
//! All output goes directly to UART via `putchar_no_lock` (bypasses printk
//! ring buffer), making these safe to call from crash/panic contexts.

use core::fmt;
use core::fmt::Write;
use crate::console::putchar_no_lock;

// ============================================================================
// ConsoleWriter — lockless UART output for crash/panic contexts
// ============================================================================

/// Lockless console writer that bypasses printk and CONSOLE_LOGLEVEL.
///
/// Uses `putchar_no_lock` directly. Safe to use in panic/OOPS contexts
/// where normal printk infrastructure may be unavailable or recursive.
pub struct ConsoleWriter;

impl ConsoleWriter {
    pub const fn new() -> Self {
        ConsoleWriter
    }
}

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                unsafe { putchar_no_lock(b'\r'); }
            }
            unsafe { putchar_no_lock(b); }
        }
        Ok(())
    }
}

// ============================================================================
// Frame Pointer Stack Walk
// ============================================================================

/// Walk frame pointers and call the callback for each frame.
///
/// Reads the current `s0` (frame pointer) via inline asm, then iterates
/// the frame pointer chain: `[fp]` = saved_fp, `[fp+8]` = return_addr.
///
/// # Arguments
/// * `cb` - Callback called as `cb(program_counter, frame_pointer)` for each frame.
///
/// # Safety
/// Reads from memory via frame pointer chain. Caller must ensure the stack
/// is in a valid state (not corrupted). Validates alignment and address range.
pub fn walk_stack_trace(cb: &mut dyn FnMut(u64, u64)) {
    let mut fp: u64;
    unsafe {
        core::arch::asm!("mv {}, s0", out(reg) fp, options(nomem, nostack));
    }

    let mut frame_count = 0u32;
    while fp != 0 && frame_count < 32 {
        // Validate fp: must be 8-byte aligned and in kernel address range
        if fp < 0x8000_0000 || fp > 0xFFFF_FFFF_FFFF_FFFF || fp % 8 != 0 {
            break;
        }

        unsafe {
            let fp_val = *(fp as *const u64);
            let ret_addr = *((fp + 8) as *const u64);

            // Validate return address
            if ret_addr == 0 {
                break;
            }

            cb(ret_addr, fp);

            // Check if next fp is valid (should be > current fp or 0)
            if fp_val <= fp {
                break;
            }
            fp = fp_val;
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
    let mut ra: u64;
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }

    let _ = w.write_str("Call trace:\n");
    let _ = write!(w, "  [<{:016x}>] (current)\n", ra);

    walk_stack_trace(&mut |pc, _fp| {
        let _ = write!(w, "  [<{:016x}>]\n", pc);
    });

    let _ = w.write_str("\n");
}

// ============================================================================
// Register and CSR Dump
// ============================================================================

/// RISC-V register names in order (x1..x31)
const REG_NAMES: [&str; 31] = [
    "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0",
    "s1", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
    "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8",
    "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

/// Save all integer registers to a stack buffer via inline assembly.
///
/// Returns an array of 31 u64 values representing x1..x31.
pub fn save_regs() -> [u64; 31] {
    let mut regs: [u64; 31] = [0; 31];
    unsafe {
        core::arch::asm!(
            "sd ra,  0*8({buf})",
            "sd sp,  1*8({buf})",
            "sd gp,  2*8({buf})",
            "sd tp,  3*8({buf})",
            "sd t0,  4*8({buf})",
            "sd t1,  5*8({buf})",
            "sd t2,  6*8({buf})",
            "sd s0,  7*8({buf})",
            "sd s1,  8*8({buf})",
            "sd a0,  9*8({buf})",
            "sd a1,  10*8({buf})",
            "sd a2,  11*8({buf})",
            "sd a3,  12*8({buf})",
            "sd a4,  13*8({buf})",
            "sd a5,  14*8({buf})",
            "sd a6,  15*8({buf})",
            "sd a7,  16*8({buf})",
            "sd s2,  17*8({buf})",
            "sd s3,  18*8({buf})",
            "sd s4,  19*8({buf})",
            "sd s5,  20*8({buf})",
            "sd s6,  21*8({buf})",
            "sd s7,  22*8({buf})",
            "sd s8,  23*8({buf})",
            "sd s9,  24*8({buf})",
            "sd s10, 25*8({buf})",
            "sd s11, 26*8({buf})",
            "sd t3,  27*8({buf})",
            "sd t4,  28*8({buf})",
            "sd t5,  29*8({buf})",
            "sd t6,  30*8({buf})",
            buf = inout(reg) regs.as_mut_ptr() => _,
            options(nostack, preserves_flags)
        );
    }
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
pub fn dump_csrs() {
    let mut w = ConsoleWriter::new();

    let (sstatus, scause, stval, sepc): (u64, u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "csrr {0}, sstatus",
            "csrr {1}, scause",
            "csrr {2}, stval",
            "csrr {3}, sepc",
            out(reg) sstatus, out(reg) scause, out(reg) stval, out(reg) sepc,
            options(nomem, nostack)
        );
    }

    let _ = write!(w, "Sstatus: {:016x}\n", sstatus);
    let _ = write!(w, "Scause : {:016x}\n", scause);
    let _ = write!(w, "Stval  : {:016x}\n", stval);
    let _ = write!(w, "Sepc   : {:016x}\n\n", sepc);
}
