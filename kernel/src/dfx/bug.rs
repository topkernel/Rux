//! BUG and WARN Macros
//!
//! Linux-compatible BUG/WARN infrastructure for the Rux kernel.
//!
//! - `WARN()`: Non-fatal warning. Logs message + stack trace + taints kernel.
//! - `BUG()`:  Fatal. Logs message + stack trace, then calls `panic!()`.
//!
//! Reference: Linux `include/asm-generic/bug.h`, `kernel/panic.c`

use core::fmt::Write;
use crate::dfx::backtrace::ConsoleWriter;
use crate::dfx::taint;
use crate::dfx::backtrace;

/// Non-fatal warning.
///
/// Prints a Linux-compatible "cut here" warning header, the file:line location,
/// the condition string, CPU/PID info, stack trace, and taint string.
/// Taints the kernel with `TAINT_WARN`.
///
/// Called by the `warn_on!` macro — do not call directly in normal code.
#[track_caller]
pub fn warn(file: &str, line: u32, condition: &str) {
    let mut w = ConsoleWriter::new();

    // "cut here" marker
    let _ = w.write_str("------------[ cut here ]------------\n");

    // WARNING header
    let _ = write!(w, "WARNING: {}:{}\n", file, line);
    let _ = write!(w, "  condition \"{}\" was true\n", condition);

    // CPU/PID info
    let cpu = crate::arch::cpu_id() as u32;
    let pid = crate::sched::get_current_pid();
    let _ = write!(w, "CPU: {} PID: {}\n", cpu, pid);

    // Stack trace
    backtrace::dump_stack();

    // Taint string
    let mut taint_buf = [0u8; 16];
    taint::taint_string(&mut taint_buf);
    let taint_str = unsafe { core::str::from_utf8_unchecked(&taint_buf) };
    let _ = write!(w, "Tainted: {}\n", taint_str);

    // End marker
    let _ = w.write_str("---[ end trace 0000000000 ]---\n");

    // Taint the kernel
    taint::add_taint(taint::TaintFlags::WARN);
}

/// Fatal BUG.
///
/// Prints a BUG header with location, then triggers `panic!()`.
/// This is the terminal path — the kernel will not continue.
///
/// Called by the `bug_on!` macro — do not call directly in normal code.
#[track_caller]
pub fn bug(file: &str, line: u32) {
    let mut w = ConsoleWriter::new();

    // "cut here" marker
    let _ = w.write_str("------------[ cut here ]------------\n");

    // BUG header
    let _ = write!(w, "kernel BUG at {}:{}!\n", file, line);

    // CPU/PID info
    let cpu = crate::arch::cpu_id() as u32;
    let pid = crate::sched::get_current_pid();
    let _ = write!(w, "CPU: {} PID: {}\n", cpu, pid);

    // Stack trace
    backtrace::dump_stack();

    // Taint the kernel
    taint::add_taint(taint::TaintFlags::DIE);

    // Trigger panic — this never returns
    panic!("BUG: kernel BUG at {}:{}", file, line);
}

/// WARN_ON_ONCE support: per-callsite `AtomicBool` to ensure a warning
/// fires only once per boot.
///
/// # Safety
/// The `flag` must point to a valid `AtomicBool` that outlives the program
/// (typically a `static`).
pub fn warn_on_once_check(flag: &core::sync::atomic::AtomicBool) -> bool {
    // Try to set the flag from false to true.
    // If already true, return false (already warned).
    use core::sync::atomic::Ordering;
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
}
