//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/cmdline - Kernel boot command line
//!
//! Reference: Linux fs/proc/cmdline.c

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/cmdline content
///
/// Shows the kernel boot parameters passed to the kernel.
pub fn generate() -> Vec<u8> {
    use crate::cmdline;

    match cmdline::get_cmdline() {
        Some(bootargs) if !bootargs.is_empty() => {
            format!("{}\n", bootargs).into_bytes()
        }
        _ => {
            // Default cmdline if none was provided
            b"BOOT_IMAGE=/boot/rux console=ttyS0\n".to_vec()
        }
    }
}

/// Get cmdline without trailing newline (for internal use)
pub fn get_cmdline() -> alloc::string::String {
    use crate::cmdline;

    match cmdline::get_cmdline() {
        Some(bootargs) if !bootargs.is_empty() => alloc::string::String::from(bootargs),
        _ => alloc::string::String::from("BOOT_IMAGE=/boot/rux console=ttyS0"),
    }
}
