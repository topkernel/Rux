//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/version - Kernel version information

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/version content
///
/// U2: `Linux version <release> (buildd@host) (compiler) #<build>` —
/// the exact prefix systemd/dpkg-style parsers tokenize on. The release
/// matches uname -r (KERNEL_VERSION-rux) so the two sources agree.
///
/// The build identity (commit, profile, build time) is appended in the
/// Linux `#<build>` slot's spirit: a running kernel can always be matched
/// against a commit, which is what tells a fresh binary from a stale one
/// (ttydev lane, 2026-11-16).
pub fn generate() -> Vec<u8> {
    format!(
        "Linux version {} (buildd@rux) (rustc {}) #{} {} {} SMP\n",
        get_release_string(),
        option_env!("RUSTC_VERSION").unwrap_or("unknown"),
        option_env!("RUX_GIT_COMMIT").unwrap_or("unknown"),
        option_env!("RUX_BUILD_PROFILE").unwrap_or("unknown"),
        option_env!("BUILD_TIME").unwrap_or("unknown"),
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
