//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/version - Kernel version information

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/version content
///
/// U2: `Linux version <release> (buildd@host) (compiler) #<build>` —
/// the exact prefix systemd/dpkg-style parsers tokenize on. The release
/// matches uname -r (KERNEL_VERSION-rux) so the two sources agree.
pub fn generate() -> Vec<u8> {
    format!(
        "Linux version {} (buildd@rux) (rustc {}) #1 SMP\n",
        get_release_string(),
        option_env!("RUSTC_VERSION").unwrap_or("unknown"),
    )
    .into_bytes()
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
