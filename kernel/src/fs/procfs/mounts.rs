//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/mounts - Mounted filesystems
//!
//! Also provides /proc/filesystems and /proc/self/mountinfo content.
//!
//! U2: mountinfo is what systemd actually parses (mount_setup /
//! mount_table), so the boot layout is synthesized here rather than
//! depending on register_mount() having been called for every boot-time
//! mount — procfs, sysfs and devtmpfs are linked with vfs_mount()
//! directly during boot and would otherwise be missing from the
//! namespace view until a sys_mount re-registered them.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// One canonical mount entry for the synthesized boot layout.
struct BootMount {
    mount_id: u32,
    parent_id: u32,
    dev: &'static str,
    mount_point: &'static str,
    fs_type: &'static str,
    source: &'static str,
}

/// The boot layout systemd expects to find (U2).
///
/// - `/`      : ext4 on /dev/vda once the root disk is mounted, else rootfs
/// - `/dev`   : devtmpfs (dynamic device nodes)
/// - `/dev/pts`: devpts (pty slaves — dynamic since P0)
/// - `/proc`  : proc
/// - `/sys`   : sysfs
/// - `/run`   : tmpfs (systemd runtime state, mounted by main.rs)
///
/// `/dev/shm` and `/sys/fs/cgroup` come in through the live namespace
/// table (they are mounted via do_mount, which registers them).
fn boot_mounts() -> Vec<BootMount> {
    let root_is_ext4 = crate::fs::ext4::is_mounted();
    let mut out = Vec::new();
    out.push(BootMount {
        mount_id: 1,
        parent_id: 0,
        dev: if root_is_ext4 { "254:0" } else { "0:1" },
        mount_point: "/",
        fs_type: if root_is_ext4 { "ext4" } else { "rootfs" },
        source: if root_is_ext4 { "/dev/vda" } else { "rootfs" },
    });
    out.push(BootMount {
        mount_id: 2,
        parent_id: 1,
        dev: "0:5",
        mount_point: "/dev",
        fs_type: "devtmpfs",
        source: "devtmpfs",
    });
    out.push(BootMount {
        mount_id: 3,
        parent_id: 2,
        dev: "0:4",
        mount_point: "/dev/pts",
        fs_type: "devpts",
        source: "devpts",
    });
    out.push(BootMount {
        mount_id: 4,
        parent_id: 1,
        dev: "0:3",
        mount_point: "/proc",
        fs_type: "proc",
        source: "proc",
    });
    out.push(BootMount {
        mount_id: 5,
        parent_id: 1,
        dev: "0:6",
        mount_point: "/sys",
        fs_type: "sysfs",
        source: "sysfs",
    });
    out.push(BootMount {
        mount_id: 6,
        parent_id: 1,
        dev: "0:20",
        mount_point: "/run",
        fs_type: "tmpfs",
        source: "tmpfs",
    });
    out
}

/// Generate /proc/mounts content
///
/// Format: <device> <mount_point> <fs_type> <options> 0 0
pub fn generate() -> Vec<u8> {
    let mut content = String::new();

    // Synthesized boot layout first (stable order), then any
    // namespace-registered mounts not already covered.
    let boot = boot_mounts();
    let seen: Vec<&str> = boot.iter().map(|m| m.mount_point).collect();
    for m in boot.iter() {
        content.push_str(&format!(
            "{} {} {} rw,relatime 0 0\n",
            m.source, m.mount_point, m.fs_type
        ));
    }

    for (device, mount_point, fs_type, options) in crate::fs::mount::get_mounts() {
        if mount_point == "/" || seen.iter().any(|s| *s == mount_point) {
            continue; // "/" and boot-layout entries already emitted
        }
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
    content.push_str("nodev\tcgroup2\n");
    content.push_str("nodev\ttmpfs\n");

    content.into_bytes()
}

/// Generate /proc/self/mountinfo content (detailed mount info)
///
/// Format (systemd's parser, proc-mounts(5)):
/// `mount_id parent_id major:minor root mount_point mount_options
///  [optional_fields...] - fs_type mount_source super_options`
///
/// U2: the boot layout (/ /proc /sys /dev /dev/pts /run) is synthesized
/// so systemd always sees a complete hierarchy, merged with the live
/// namespace-registered mounts (/dev/shm, /sys/fs/cgroup, ...).
pub fn generate_mountinfo() -> Vec<u8> {
    let mut content = String::new();

    let boot = boot_mounts();
    let seen: Vec<&str> = boot.iter().map(|m| m.mount_point).collect();
    let mut next_id: u32 = 16;
    for m in boot.iter() {
        content.push_str(&format!(
            "{} {} {} / {} rw,relatime - {} {} rw\n",
            m.mount_id, m.parent_id, m.dev, m.mount_point, m.fs_type, m.source
        ));
    }

    for (device, mount_point, fs_type, options) in crate::fs::mount::get_mounts() {
        if mount_point == "/" || seen.iter().any(|s| *s == mount_point) {
            continue;
        }
        let mount_id = next_id;
        next_id += 1;
        // Parent of anything not under /dev/pts is the root mount (1);
        // nested mounts (e.g. /sys/fs/cgroup under /sys) keep their
        // textual path — systemd only needs a consistent DAG.
        let parent_id = 1u32;
        content.push_str(&format!(
            "{} {} 0:23 / {} rw,relatime - {} {} {}\n",
            mount_id, parent_id, mount_point, fs_type, device, options
        ));
    }

    content.into_bytes()
}
