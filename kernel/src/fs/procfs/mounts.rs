//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/mounts - Mounted filesystems
//!
//! Also provides /proc/filesystems content.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Generate /proc/mounts content
///
/// Format: <device> <mount_point> <fs_type> <options> 0 0
pub fn generate() -> Vec<u8> {
    let mounts = crate::fs::mount::get_mounts();
    let mut content = String::new();
    for (device, mount_point, fs_type, options) in mounts {
        content.push_str(&format!(
            "{} {} {} {} 0 0\n",
            device, mount_point, fs_type, options
        ));
    }
    content.into_bytes()
}

/// Generate /proc/filesystems content
///
/// Lists all supported filesystem types.
pub fn generate_filesystems() -> Vec<u8> {
    let mut content = String::new();

    // Filesystem types supported by Rux
    content.push_str("nodev\trootfs\n");
    content.push_str("nodev\tproc\n");
    content.push_str("nodev\tdevtmpfs\n");
    content.push_str("nodev\tdevpts\n");
    content.push_str("\text4\n");
    content.push_str("nodev\tsysfs\n");
    content.push_str("nodev\ttmpfs\n");

    content.into_bytes()
}

/// Generate /proc/mountinfo content (detailed mount info)
pub fn generate_mountinfo() -> Vec<u8> {
    let mut content = String::new();

    // Format: <id> <parent_id> <major>:<minor> <root> <mount_point> <options> - <fs_type> <source> <fs_options>
    // Root filesystem (id=1, parent=0)
    content.push_str("1 0 0:0 / / rw - rootfs rootfs rw\n");

    // /proc (id=2, parent=1)
    content.push_str("2 1 0:3 / /proc rw - proc proc rw\n");

    // /dev (id=3, parent=1)
    content.push_str("3 1 0:4 / /dev rw - devtmpfs devtmpfs rw\n");

    content.into_bytes()
}
