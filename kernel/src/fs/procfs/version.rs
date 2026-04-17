//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/version - Kernel version information

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/version content
///
/// Format matches Linux: <os> version <release> (<who@arch>) (<compiler>) #<build>
pub fn generate() -> Vec<u8> {
    use crate::config;

    format!("Rux version {} (root@riscv64) (rustc {}) #1 SMP\n",
        config::KERNEL_VERSION,
        option_env!("RUSTC_VERSION").unwrap_or("unknown"),
    ).into_bytes()
}

/// Get short version string (for uname)
pub fn get_version_string() -> &'static str {
    crate::config::KERNEL_VERSION
}

/// Get OS release string
pub fn get_release_string() -> alloc::string::String {
    use crate::config::KERNEL_VERSION;
    format!("{}-rux", KERNEL_VERSION)
}
