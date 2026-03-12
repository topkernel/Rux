//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/version - Kernel version information
//!
//! Reference: Linux fs/proc/version.c

use alloc::vec::Vec;
use alloc::format;

/// Generate /proc/version content
///
/// Format: Rux version <version> (<build info>) <compiler info>
pub fn generate() -> Vec<u8> {
    use crate::config::KERNEL_VERSION;

    let rustc_version = option_env!("RUSTC_VERSION").unwrap_or("unknown");
    let build_time = option_env!("BUILD_TIME").unwrap_or("unknown");

    let content = format!(
        "Rux version {} (riscv64)\n\
         Compiled with Rust {}\n\
         Build time: {}\n\
         Copyright (c) 2026 Fei Wang\n",
        KERNEL_VERSION,
        rustc_version,
        build_time
    );

    content.into_bytes()
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
