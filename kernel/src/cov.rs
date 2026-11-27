//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Whole-kernel coverage dump (feature `cov-dump`).
//!
//! With `RUSTFLAGS="-C instrument-coverage -Zno-profiler-runtime"` the
//! compiler instruments every function with coverage counters and emits
//! the `__llvm_prf_*` sections but references no profile runtime — this
//! module (via the vendored minicov crate, the no_std port of the
//! compiler-rt profiling runtime) supplies the missing runtime and
//! serializes a standard `.profraw` at the end of the suite run.
//!
//! Transport: hex over the test suite's UART stream. The suite hook
//! (kernel/src/tests/mod.rs) calls [`dump_to_console`] after the summary
//! print; the boot log already captures every byte, so the host side
//! (scripts/coverage-report.sh) extracts the hex between the
//! [cov-begin]/[cov-end] markers and decodes it — no extra QEMU device,
//! no filesystem dependency (the suite runs before any rootfs mount).
//!
//! Everything here is feature-gated: default builds compile none of it,
//! and normal builds are untouched.
//!
//! Zero-unsafe (mainline minimal-first collection, 2026-11-27): the one
//! minicov capture edge lives in kernel/src/unsafe.rs
//! (INV-COV-CAPTURE-1).
#![forbid(unsafe_code)]

#![allow(dead_code)]

use alloc::vec::Vec;

const HEX: &[u8; 16] = b"0123456789abcdef";
/// Hex characters per emitted line (log-line-based host extraction).
const LINE_HEX: usize = 2048;

/// Raw console line writer (same byte stream the unit-test suite's
/// test_println produces: batched UART bytes, '\n' then '\r'). Works in
/// ANY boot shape — the in-kernel suite AND the userspace-test boot
/// (init=/test/...), where test_println does not exist.
fn cov_line(bytes: &[u8]) {
    let mut out = alloc::vec::Vec::with_capacity(bytes.len() + 2);
    out.extend_from_slice(bytes);
    out.push(b'\n');
    out.push(b'\r');
    let mut uart = crate::console::lock();
    uart.puts_batch(&out);
}

/// KCOV-alignment trigger: a write to /proc/sys/kernel/cov_dump from a
/// userspace test process captures the whole-kernel profile and emits it
/// on the console (the reference exposes coverage through debugfs to the
/// driving process; the console-hex variant keeps our extraction pipeline
/// identical across boot shapes).
pub fn trigger_write(input: &[u8]) -> i32 {
    match dump_to_console() {
        Ok(_) => input.len() as i32,
        Err(e) => e,
    }
}

/// Read side of the trigger file: "0\n" (dump-on-write knob).
pub fn trigger_generate() -> alloc::vec::Vec<u8> {
    alloc::vec![b'0', b'\n']
}

/// Capture the whole-program profile into a `.profraw` byte buffer.
///
/// SAFETY: capture_coverage walks the `__llvm_prf_*` sections that rustc
/// emitted for this image; it is not thread-safe, which is fine here —
/// the suite hook runs it once on the test CPU after all other test work
/// has completed. The buffer is plain heap memory; the runtime writes
/// bytes into it through the CoverageWriter impl and never retains
/// references.
fn capture_profile() -> Result<Vec<u8>, i32> {
    if !minicov::coverage_enabled() {
        // Built with cov-dump but without instrument-coverage: the
        // sections are empty and the profile would carry no data.
        return Err(-5); // -EIO: caller prints a clear diagnostic
    }
    let mut buf = Vec::new();
    crate::boot_edges::capture_coverage(&mut buf)?;
    Ok(buf)
}

/// Emit the captured profile over the suite console as hex between
/// marker lines. Returns the byte count emitted, or a negative errno.
pub fn dump_to_console() -> Result<usize, i32> {
    // Quiesce window (byte-level stream integrity): printk's deferred
    // drain (tick/idle) can interleave WHOLE or PARTIAL lines into the
    // raw UART stream mid-dump and corrupt the host-side hex decode.
    // Drain everything pending NOW, synchronously, then hold the
    // deferred drain off for the duration of the dump.
    while crate::printk::console_drain_pending(64) > 0 {}
    CONSOLE_DUMP_ACTIVE.store(true, core::sync::atomic::Ordering::Release);
    let result = dump_inner();
    CONSOLE_DUMP_ACTIVE.store(false, core::sync::atomic::Ordering::Release);
    result
}

/// Drain-suppression flag: while a coverage dump owns the console, the
/// deferred drain skips (lines stay in the ring; the next drain after
/// the dump emits them).
static CONSOLE_DUMP_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[inline]
pub fn console_dump_active() -> bool {
    CONSOLE_DUMP_ACTIVE.load(core::sync::atomic::Ordering::Acquire)
}

fn dump_inner() -> Result<usize, i32> {
    let buf = capture_profile()?;
    let hdr = alloc::format!("[cov-begin] {} bytes", buf.len());
    cov_line(hdr.as_bytes());
    let mut line = Vec::with_capacity(LINE_HEX);
    for b in buf.iter() {
        line.push(HEX[(b >> 4) as usize]);
        line.push(HEX[(b & 0xf) as usize]);
        if line.len() == LINE_HEX {
            // SAFETY: line is valid UTF-8 by construction (hex digits).
            cov_line(&line);
            line.clear();
        }
    }
    if !line.is_empty() {
        // SAFETY: as above.
        cov_line(&line);
    }
    cov_line(b"[cov-end]");
    Ok(buf.len())
}
