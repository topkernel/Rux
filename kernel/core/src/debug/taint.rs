//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Taint Bitmask
//!
//! Taint flags for tracking kernel state anomalies.
//! Used by WARN, BUG, panic, softlockup, and other DEBUG modules.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU32, Ordering};

// TaintFlags — the definition moved to debug/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use super::types::{TaintFlags};

/// Global kernel taint bitmask
static TAINT_MASK: AtomicU32 = AtomicU32::new(0);

/// Add a taint flag to the kernel taint bitmask.
///
/// Once set, taint flags are never cleared.
pub fn add_taint(flag: TaintFlags) {
    TAINT_MASK.fetch_or(flag.bits(), Ordering::Release);
}

/// Get the current taint bitmask value.
pub fn get_taints() -> u32 {
    TAINT_MASK.load(Ordering::Acquire)
}

/// Check if a specific taint flag is set.
pub fn tainted(flag: TaintFlags) -> bool {
    (get_taints() & flag.bits()) != 0
}

/// Convert taint bitmask to character string.
///
/// Linux renders the taint as a 16-column line ("Tainted: G        W
///          "): the first column is the proprietary-module flag — 'P'
/// when tainted, 'G' ("good") otherwise — and every other column holds
/// its flag letter when the flag is set, a space when it is not. The
/// letters are Linux's:
///
/// | bit  | letter | flag                 | bit   | letter | flag               |
/// |------|--------|----------------------|-------|--------|--------------------|
/// | 1    | P      | proprietary module   | 256   | A      | ACPI override      |
/// | 2    | F      | forced module        | 512   | W      | warning            |
/// | 4    | S      | unsafe SMP           | 1024  | C      | crappy module      |
/// | 8    | R      | forced rmmod        | 2048  | I      | firmware workaround|
/// | 16   | M      | machine check        | 4096  | O      | out-of-tree module |
/// | 32   | B      | bad page             | 8192  | E      | unsigned module    |
/// | 64   | U      | user                 | 16384 | L      | soft lockup        |
/// | 128  | D      | kernel died          | 32768 | K      | livepatch          |
///
/// The string is written to the provided buffer (must be >= 17 bytes for
/// 16 chars + null).
///
/// # Returns
/// Number of characters written (excluding null terminator).
pub fn taint_string(buf: &mut [u8]) -> usize {
    const TAINT_CHARS: &[u8; 16] = b"PFSRMBUDWACIOELK";

    let mask = get_taints();
    let len = 16.min(buf.len());

    for i in 0..len {
        let bit = 1u32 << i;
        buf[i] = if mask & bit != 0 {
            TAINT_CHARS[i]
        } else if i == 0 {
            b'G' // "good" — Linux's first column
        } else {
            b' '
        };
    }

    len
}

/// Get taint string as a fixed-size array (no allocation needed).
pub fn taint_string_arr() -> [u8; 16] {
    let mut buf = [0u8; 16];
    taint_string(&mut buf);
    buf
}
