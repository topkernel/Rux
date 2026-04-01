//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Taint Bitmask
//!
//! Taint flags for tracking kernel state anomalies.
//! Used by WARN, BUG, panic, softlockup, and other DFX modules.

use core::sync::atomic::{AtomicU32, Ordering};

bitflags::bitflags! {
    /// Kernel taint flags
    ///
    /// Each flag indicates a specific class of anomaly. Once set, flags are never cleared.
    /// The taint string can be read via `/proc/sys/kernel/tainted`.
    #[derive(Debug, Clone, Copy)]
    pub struct TaintFlags: u32 {
        /// Proprietary module was loaded
        const PROPRIETARY_MODULE = 0x0001;
        /// Module was force-loaded
        const FORCED_MODULE      = 0x0002;
        /// Unsafe SMP processor detected
        const UNSAFE_SMP         = 0x0004;
        /// Module was force-unloaded
        const FORCED_RMMOD       = 0x0008;
        /// Machine check error occurred
        const MACHINE_CHECK      = 0x0010;
        /// Bad page reference
        const BAD_PAGE           = 0x0020;
        /// User-set taint flag
        const USER               = 0x0040;
        /// Kernel has died (OOPS)
        const DIE                = 0x0080;
        /// ACPI table overridden
        const OVERRIDDEN_ACPI    = 0x0100;
        /// Warning occurred (WARN)
        const WARN               = 0x0200;
        /// Crappy module loaded
        const CRAP               = 0x0400;
        /// Firmware workaround active
        const FIRMWARE_WORKAROUND= 0x0800;
        /// Out-of-tree module loaded
        const OOT_MODULE         = 0x1000;
        /// Unsigned module loaded
        const UNSIGNED_MODULE    = 0x2000;
        /// Soft lockup occurred
        const SOFTLOCKUP         = 0x4000;
        /// Live patch applied
        const LIVEPATCH          = 0x8000;
    }
}

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
/// Each character position represents one taint flag.
/// 'G' = good (not tainted for this flag), specific letter = tainted.
///
/// Output format: 16 characters, e.g. "GWFGWFGWFGWFGWFGW"
/// The string is written to the provided buffer (must be >= 17 bytes for 16 chars + null).
///
/// # Returns
/// Number of characters written (excluding null terminator).
pub fn taint_string(buf: &mut [u8]) -> usize {
    const TAINT_CHARS: &[u8; 16] = b"GFfUvPDdCAcNIOsl";
    const UNTAINTED_CHARS: &[u8; 16] = b"GGGGGGGGGGGGGGGG";

    let mask = get_taints();
    let len = 16.min(buf.len());

    for i in 0..len {
        let bit = 1u32 << i;
        if mask & bit != 0 {
            buf[i] = TAINT_CHARS[i];
        } else {
            buf[i] = UNTAINTED_CHARS[i];
        }
    }

    len
}

/// Get taint string as a fixed-size array (no allocation needed).
pub fn taint_string_arr() -> [u8; 16] {
    let mut buf = [0u8; 16];
    taint_string(&mut buf);
    buf
}
