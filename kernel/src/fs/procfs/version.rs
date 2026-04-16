//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/version - Kernel version information

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/version content
///
/// Format: Rux version <version> (root@riscv64) (rustc <version>) <version>
pub fn generate() -> Vec<u8> {
    use crate::config;

    format!("Rux version {} (root@riscv64) (rustc {}) {}\n",
        config::KERNEL_VERSION,
        option_env!("RUSTC_VERSION").unwrap_or("unknown"),
        config::KERNEL_VERSION
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
