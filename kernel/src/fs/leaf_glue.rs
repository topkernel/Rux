//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Framework-side glue for the substrate/boot leaf filesystems that
//! moved into the rux-fs service crate (the fs-substrate tranche):
//! generic-closure shims over the FsHooks fn-pointer rows (the boxing
//! happens here so the kernel's population call sites keep their
//! ergonomic generic shape) plus the DTOs both sides must name
//! (InputEventReg — a plain-data record, framework-owned so the
//! drivers stay framework-typed).
//!
//! Zero behavior change: each shim forwards to the registered row;
//! before the crate flips these were direct calls into the leaf.
//! Call sites keep their `debugfs::create_dir(...)` / `sysfs::...`
//! shape — only the `use` line changes (`crate::fs::debugfs` ->
//! `crate::fs::leaf_glue::debugfs`).

use alloc::string::String;
use alloc::vec::Vec;

use super::fs_hooks::hooks;

// ---- debugfs population (dfx instruments, lib.rs tracing dir) ----

pub mod debugfs {
    use super::*;

    pub fn create_dir(path: &str) -> bool {
        (hooks().debugfs_create_dir)(path)
    }

    pub fn create_file<F, S>(path: &str, mode: u32, show: F, store: Option<S>) -> bool
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
        S: Fn(&[u8]) -> i32 + Send + Sync + 'static,
    {
        (hooks().debugfs_create_file)(
            path,
            mode,
            Some(alloc::boxed::Box::new(show)),
            store.map(|s| alloc::boxed::Box::new(s) as alloc::boxed::Box<dyn Fn(&[u8]) -> i32 + Send + Sync>),
        )
    }

    pub fn init_debugfs() -> Result<(), i32> {
        (hooks().debugfs_init)()
    }

    pub fn debugfs_root_present() -> bool {
        (hooks().debugfs_root_present)()
    }

    pub fn create_root_inode() -> Option<alloc::sync::Arc<super::super::inode::Inode>> {
        (hooks().debugfs_create_root_inode)()
    }

    pub fn mount_debugfs() -> Result<(), i32> {
        (hooks().debugfs_mount_boot)()
    }
}

// ---- tracefs population (the trace subsystem's file declarations) ----

pub mod tracefs {
    use super::*;

    pub fn create_dir(path: &str) -> bool {
        (hooks().tracefs_create_dir)(path)
    }

    pub fn create_file<F, S>(path: &str, mode: u32, show: F, store: Option<S>) -> bool
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
        S: Fn(&[u8]) -> i32 + Send + Sync + 'static,
    {
        (hooks().tracefs_create_file)(
            path,
            mode,
            Some(alloc::boxed::Box::new(show)),
            store.map(|s| alloc::boxed::Box::new(s) as alloc::boxed::Box<dyn Fn(&[u8]) -> i32 + Send + Sync>),
        )
    }

    pub fn create_stream_file<F>(path: &str, mode: u32, show: F) -> bool
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
    {
        (hooks().tracefs_create_stream_file)(path, mode, Some(alloc::boxed::Box::new(show)))
    }

    pub fn tracefs_root_present() -> bool {
        (hooks().tracefs_root_present)()
    }

    pub fn init_tracefs() -> Result<(), i32> {
        (hooks().tracefs_init)()
    }

    pub fn create_root_inode() -> Option<alloc::sync::Arc<super::super::inode::Inode>> {
        (hooks().tracefs_create_root_inode)()
    }

    pub fn mount_at(mountpoint: &str) -> Result<(), i32> {
        (hooks().tracefs_mount_at)(mountpoint)
    }
}

// ---- cgroup (lib.rs boot + fsctx mount arm) ----

pub mod cgroup {
    use super::*;

    pub fn init_cgroupfs() -> Result<(), i32> {
        (hooks().cgroup_init_cgroupfs)()
    }

    pub fn create_root_inode() -> Option<alloc::sync::Arc<super::super::inode::Inode>> {
        (hooks().cgroup_create_root_inode)()
    }

    pub fn mount_cgroupfs(target: &str) -> Result<(), i32> {
        (hooks().cgroup_mount_cgroupfs)(target)
    }
}

// ---- sysfs registration (drivers + module loader) ----

pub mod sysfs {
    use super::*;

    pub fn publish_module(name: &str) {
        (hooks().sysfs_publish_module)(name)
    }

    pub fn unpublish_module(name: &str) {
        (hooks().sysfs_unpublish_module)(name)
    }

    pub fn register_drm_device(slot_name: &str) {
        (hooks().sysfs_register_drm_device)(slot_name)
    }

    pub fn netdev_uevent(name: &str, action: &str) -> u64 {
        (hooks().sysfs_netdev_uevent)(name, action)
    }

    pub fn register_block_disk_at(
        name: &str,
        major: u32,
        minor: u32,
        capacity_sectors: u64,
        chain: &[(&str, &str)],
    ) -> u64 {
        (hooks().sysfs_register_block_disk_at)(name, major, minor, capacity_sectors, chain)
    }

    pub fn register_input_event(reg: &super::InputEventReg) -> u64 {
        (hooks().sysfs_register_input_event)(reg)
    }
}

/// The input-device registration record (evdev -> sysfs uevent + tree).
/// Framework-owned DTO: the drivers fill it, the rux-fs sysfs leaf
/// consumes it through the hook row (plain data — no leaf types cross
/// the seam). The uevent environment block builder lives with it.
#[derive(Debug, Clone)]
pub struct InputEventReg {
    /// input_dev kobject name ("input0")
    pub input_name: String,
    /// event node name ("event0")
    pub event_name: String,
    pub major: u32,
    pub minor: u32,
    /// input device name ("VirtIO Keyboard") — NAME= env + name attr
    pub dev_name: String,
    pub bustype: u32,
    pub vendor: u32,
    pub product: u32,
    pub version: u32,
    /// EV= bitmap string (Linux input_print_bitmap format: space-separated
    /// lowercase-hex 64-bit words, most-significant first)
    pub ev: String,
    /// KEY= bitmap string, empty -> line omitted (type not supported)
    pub key: String,
    /// REL= bitmap string, empty -> omitted
    pub rel: String,
    /// ABS= bitmap string, empty -> omitted
    pub abs: String,
}

impl InputEventReg {
    /// The Linux `input_dev_uevent()` environment block for the input_dev
    /// kobject (PRODUCT/NAME/PROP/EV/KEY/REL/ABS in kernel emit order).
    pub fn dev_env(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = Vec::new();
        env.push((
            String::from("PRODUCT"),
            alloc::format!(
                "{:x}/{:x}/{:x}/{:x}",
                self.bustype,
                self.vendor,
                self.product,
                self.version
            ),
        ));
        // Linux quotes NAME in the uevent environment (NAME="%s").
        env.push((
            String::from("NAME"),
            alloc::format!("\"{}\"", self.dev_name),
        ));
        env.push((String::from("PROP"), String::from("0")));
        env.push((String::from("EV"), self.ev.clone()));
        if !self.key.is_empty() {
            env.push((String::from("KEY"), self.key.clone()));
        }
        if !self.rel.is_empty() {
            env.push((String::from("REL"), self.rel.clone()));
        }
        if !self.abs.is_empty() {
            env.push((String::from("ABS"), self.abs.clone()));
        }
        env
    }
}
