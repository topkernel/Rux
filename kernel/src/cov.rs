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

#![allow(dead_code)]

use crate::test_println;
use alloc::vec::Vec;

const HEX: &[u8; 16] = b"0123456789abcdef";
/// Hex characters per emitted line (log-line-based host extraction).
const LINE_HEX: usize = 2048;

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
    unsafe {
        minicov::capture_coverage(&mut buf).map_err(|_| -5)?;
    }
    Ok(buf)
}

/// Emit the captured profile over the suite console as hex between
/// marker lines. Returns the byte count emitted, or a negative errno.
pub fn dump_to_console() -> Result<usize, i32> {
    let buf = capture_profile()?;
    test_println!("[cov-begin] {} bytes", buf.len());
    let mut line = Vec::with_capacity(LINE_HEX);
    for b in buf.iter() {
        line.push(HEX[(b >> 4) as usize]);
        line.push(HEX[(b & 0xf) as usize]);
        if line.len() == LINE_HEX {
            // SAFETY: line is valid UTF-8 by construction (hex digits).
            test_println!("{}", core::str::from_utf8(&line).unwrap_or(""));
            line.clear();
        }
    }
    if !line.is_empty() {
        // SAFETY: as above.
        test_println!("{}", core::str::from_utf8(&line).unwrap_or(""));
    }
    test_println!("[cov-end]");
    Ok(buf.len())
}
