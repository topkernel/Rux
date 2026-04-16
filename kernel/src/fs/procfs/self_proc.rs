//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/self - Symlink to current process directory
//!
//! /proc/self is a symbolic link that points to /proc/[pid] where
//! [pid] is the PID of the process reading the link.

use alloc::vec::Vec;
use alloc::format;

/// Get the link target for /proc/self
///
/// Returns the PID number of the current process.
pub fn get_self_link() -> Vec<u8> {
    use crate::process::current_pid;
    let pid = current_pid();
    format!("{}", pid).into_bytes()
}

/// Generate /proc/self content (same as link target)
pub fn generate() -> Vec<u8> {
    get_self_link()
}
