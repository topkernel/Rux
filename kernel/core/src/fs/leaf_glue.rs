//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Framework-side adapters for the substrate/boot leaf interface tables
//! (`crate::api::fs` — the fs-substrate tranche). Each adapter forwards
//! to its row; the generic-closure population fns box their closures
//! here so the kernel's call sites keep their pre-trace shape
//! (`debugfs::create_file(path, mode, show, store)`). A call site differs
//! from the pre-trace code only in its `use` path (`crate::debugfs`
//! -> `rux::fs::leaf_glue::debugfs`).
//!
//! crate-plan §3a rule 2 (static dispatch first on data paths) is
//! implemented in two places here:
//!   * the identity probes — `tmpfs::is_tmpfs_file`, `devfs::is_loop_ops`,
//!     `procfs::is_piddir_ops` and the devfs-registry gate — compare the
//!     ops table against the identity published in the path table,
//!     INLINE: no file kind pays an indirect call;
//!   * the two devfs logic rows reached from the file-fault path
//!     (`reg_current_size`, `shared_fault_file_page`) are called only
//!     after an inline gate, so a fault on any other filesystem crosses
//!     zero times.
//! Each gate restates the first check of its row body (or the exact
//! identity check the pre-trace framework code performed at that site) —
//! zero behavior change by construction.

#![forbid(unsafe_code)]
use alloc::string::String;
use alloc::vec::Vec;

use super::file::File;
use super::file::FileOps;
use super::inode::Inode;
use crate::api::fs::{boot_hooks, path_hooks, populate_hooks};

/// Identity compare: `ops` is the exact table published for this slot.
#[inline]
fn is_ops(ops: Option<&'static FileOps>, identity: &'static FileOps) -> bool {
    ops.is_some_and(|o| core::ptr::eq(o as *const FileOps, identity as *const FileOps))
}

/// The description's inode carries `fs_id` (inode-less descriptions do
/// not).
#[inline]
fn inode_fs_id_is(file: &File, fs_id: u64) -> bool {
    file.get_inode().is_some_and(|i| i.fs_id == fs_id)
}

// ---- debugfs population (dfx instruments, lib.rs's tracing dir) ----

pub mod debugfs {
    use super::*;

    pub fn create_dir(path: &str) -> bool {
        (populate_hooks().debugfs_create_dir)(path)
    }

    pub fn create_file<F, S>(path: &str, mode: u32, show: F, store: Option<S>) -> bool
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
        S: Fn(&[u8]) -> i32 + Send + Sync + 'static,
    {
        (populate_hooks().debugfs_create_file)(
            path,
            mode,
            alloc::boxed::Box::new(show),
            store.map(|s| {
                alloc::boxed::Box::new(s) as alloc::boxed::Box<dyn Fn(&[u8]) -> i32 + Send + Sync>
            }),
        )
    }

    pub fn init_debugfs() -> Result<(), i32> {
        (boot_hooks().debugfs_init)()
    }

    pub fn debugfs_root_present() -> bool {
        (boot_hooks().debugfs_root_present)()
    }

    pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
        (boot_hooks().debugfs_create_root_inode)()
    }

    pub fn mount_debugfs() -> Result<(), i32> {
        (boot_hooks().debugfs_mount)()
    }
}

// ---- tracefs population (the trace subsystem's file declarations) ----

pub mod tracefs {
    use super::*;

    pub fn create_dir(path: &str) -> bool {
        (populate_hooks().tracefs_create_dir)(path)
    }

    pub fn create_file<F, S>(path: &str, mode: u32, show: F, store: Option<S>) -> bool
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
        S: Fn(&[u8]) -> i32 + Send + Sync + 'static,
    {
        (populate_hooks().tracefs_create_file)(
            path,
            mode,
            alloc::boxed::Box::new(show),
            store.map(|s| {
                alloc::boxed::Box::new(s) as alloc::boxed::Box<dyn Fn(&[u8]) -> i32 + Send + Sync>
            }),
        )
    }

    pub fn create_stream_file<F>(path: &str, mode: u32, show: F) -> bool
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
    {
        (populate_hooks().tracefs_create_stream_file)(path, mode, alloc::boxed::Box::new(show))
    }

    pub fn tracefs_root_present() -> bool {
        (boot_hooks().tracefs_root_present)()
    }

    pub fn init_tracefs() -> Result<(), i32> {
        (boot_hooks().tracefs_init)()
    }

    pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
        (boot_hooks().tracefs_create_root_inode)()
    }

    pub fn mount_at(mountpoint: &str) -> Result<(), i32> {
        (boot_hooks().tracefs_mount_at)(mountpoint)
    }
}

// ---- cgroup (lib.rs boot + fsctx mount arm) ----

pub mod cgroup {
    use super::*;

    pub fn init_cgroupfs() -> Result<(), i32> {
        (boot_hooks().cgroup_init)()
    }

    pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
        (boot_hooks().cgroup_create_root_inode)()
    }

    pub fn mount_cgroupfs(target: &str) -> Result<(), i32> {
        (boot_hooks().cgroup_mount)(target)
    }
}

// ---- sysfs (boot mount + driver/module registration) ----

pub mod sysfs {
    use super::*;

    pub fn init_sysfs() -> Result<(), i32> {
        (boot_hooks().sysfs_init)()
    }

    pub fn mount_sysfs() -> Result<(), i32> {
        (boot_hooks().sysfs_mount)()
    }

    pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
        (boot_hooks().sysfs_create_root_inode)()
    }

    pub fn publish_module(name: &str) {
        (populate_hooks().sysfs_publish_module)(name)
    }

    pub fn unpublish_module(name: &str) {
        (populate_hooks().sysfs_unpublish_module)(name)
    }

    pub fn register_drm_device(slot_name: &str) {
        (populate_hooks().sysfs_register_drm_device)(slot_name)
    }

    pub fn netdev_uevent(name: &str, action: &str) -> u64 {
        (populate_hooks().sysfs_netdev_uevent)(name, action)
    }

    pub fn register_block_disk_at(
        name: &str,
        major: u32,
        minor: u32,
        capacity_sectors: u64,
        chain: &[(&str, &str)],
    ) -> u64 {
        (populate_hooks().sysfs_register_block_disk_at)(name, major, minor, capacity_sectors, chain)
    }

    pub fn register_input_event(reg: &super::InputEventReg) -> u64 {
        (populate_hooks().sysfs_register_input_event)(reg)
    }
}

// ---- char_dev (the boot task's stdio) ----

pub mod char_dev {
    use super::*;

    pub fn init_std_fds_for_task(fdtable: &super::super::FdTable) {
        (boot_hooks().char_dev_init_std_fds)(fdtable)
    }

    /// fstat on an INODE-LESS description only (after the pipe and
    /// socket probes) — regular files never reach it.
    pub fn char_dev_file_stat(
        file: &File,
        stat: &mut super::super::Stat,
    ) -> Option<i32> {
        (path_hooks().char_dev_file_stat)(file, stat)
    }
}

// ---- tty (the console line discipline — console.rs's input path) ----

pub mod tty {
    use super::*;

    pub fn console_lflag() -> u32 {
        (path_hooks().tty_console_lflag)()
    }

    pub fn console_echo_enabled() -> bool {
        (path_hooks().tty_console_echo_enabled)()
    }

    pub fn console_receive_byte(c: u8) {
        (path_hooks().tty_console_receive_byte)(c)
    }

    pub fn console_fg_pgid() -> u32 {
        (path_hooks().tty_console_fg_pgid)()
    }

    pub fn set_console_sink_quiet(q: bool) {
        (path_hooks().tty_set_console_sink_quiet)(q)
    }
}

// ---- rootfs (the boot RAM root) ----

pub mod rootfs {
    use super::*;

    pub fn init_rootfs() -> Result<(), i32> {
        (boot_hooks().rootfs_init)()
    }

    pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
        (boot_hooks().rootfs_create_root_inode)()
    }

    /// The RAM root as the framework's path-shaped view (initrd's
    /// unpack target; the tests' seed/verify handle).
    pub fn sb_view() -> Option<&'static dyn RamfsView> {
        (boot_hooks().rootfs_view)()
    }

    /// init.rs's config-file probe (fs::read_file_from_rootfs).
    pub fn read_file(filename: &str) -> Option<Vec<u8>> {
        (boot_hooks().rootfs_read_file)(filename)
    }

    /// Presence + kind probe (the tests' verify step).
    pub fn lookup_kind(path: &str) -> Option<bool> {
        sb_view().and_then(|sb| sb.rfs_lookup_kind(path))
    }

    /// Seed a file into the RAM root (the tests' setup step).
    pub fn seed_file(path: &str, data: Vec<u8>) -> Option<()> {
        sb_view().map(|sb| {
            let _ = sb.rfs_seed_file(path, data);
        })
    }
}

// ---- devfs (device registry + node population + mm fault routing) ----

pub mod devfs {
    use super::*;

    pub fn init() {
        (boot_hooks().devfs_init)()
    }

    /// The mounted /dev root (None before init).
    pub fn create_root_inode() -> Option<alloc::sync::Arc<Inode>> {
        (boot_hooks().devfs_create_root_inode)()
    }

    pub fn mkdir(path: &str) -> Result<(), ()> {
        (populate_hooks().devfs_mkdir)(path)
    }

    pub fn mknod(path: &str, devno: super::super::dev_t::DevNo, mode: u32) -> Result<(), ()> {
        (populate_hooks().devfs_mknod)(path, devno, mode)
    }

    pub fn register_char_device(
        devno: super::super::dev_t::DevNo,
        ops: &'static FileOps,
    ) -> Result<(), ()> {
        (populate_hooks().devfs_register_char_device)(devno, ops)
    }

    /// The live size of a regular-registry devfs file (None for every
    /// other file). File-fault path: the inline fs_id gate is the row
    /// body's own first check, so non-devfs files never cross.
    #[inline]
    pub fn reg_current_size(file: &File) -> Option<u64> {
        if !inode_fs_id_is(file, super::super::inode::FS_ID_DEVFS) {
            return None;
        }
        (path_hooks().devfs_reg_current_size)(file)
    }

    /// The shared backing page of a regular-registry devfs file (None
    /// for every other file). File-fault path: the inline identity check
    /// is the one the pre-trace framework code performed at this site.
    #[inline]
    pub fn shared_fault_file_page(file: &File, file_off: usize) -> Option<u64> {
        if !is_ops(file.get_ops(), path_hooks().devfs_reg_file_ops) {
            return None;
        }
        (path_hooks().devfs_shared_fault_file_page)(file, file_off)
    }

    /// vt's console-device backing for /dev/ttyN (read/write/poll).
    pub fn condev_read(file: &File, buf: &mut [u8]) -> isize {
        (path_hooks().devfs_condev_read)(file, buf)
    }

    pub fn condev_write(file: &File, buf: &[u8]) -> isize {
        (path_hooks().devfs_condev_write)(file, buf)
    }

    pub fn condev_poll(file: &File, events: u16) -> u16 {
        (path_hooks().devfs_condev_poll)(file, events)
    }

    /// sys_ioctl's loop dispatch gate: INLINE identity compare.
    #[inline]
    pub fn is_loop_ops(ops: &'static FileOps) -> bool {
        is_ops(Some(ops), path_hooks().loop_file_ops)
    }

    /// The /dev/console-class ops table (the test suite's console fd).
    pub fn condev_ops() -> &'static FileOps {
        path_hooks().devfs_condev_ops
    }
}

// ---- procfs (magic links, mount rows) ----

pub mod procfs {
    use super::*;
    // The /proc ino-space layout consts (vfs's magic-link dispatch).
    pub use crate::api::fs::{PROCFS_FD_LINK_INO_OFF, PROCFS_NS_LINK_INO_OFF};

    pub fn init_procfs() -> Result<(), i32> {
        (boot_hooks().procfs_init)()
    }

    pub fn mount_procfs() -> Result<(), i32> {
        (boot_hooks().procfs_mount)()
    }

    /// The boot site's superblock presence check.
    pub fn sb_present() -> bool {
        (boot_hooks().procfs_sb_present)()
    }

    pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
        (boot_hooks().procfs_create_root_inode)()
    }

    /// pid-shape probe. Both callers gate inline on
    /// fs_id == FS_ID_PROCFS before calling.
    pub fn is_valid_pid(pid: u64) -> bool {
        (path_hooks().procfs_is_valid_pid)(pid)
    }

    /// /proc/[pid]/fd/N magic-link resolution target (same fs_id gate).
    pub fn get_fd_file(pid: u64, fd: u32) -> Option<alloc::sync::Arc<File>> {
        (path_hooks().procfs_get_fd_file)(pid, fd)
    }

    /// The pidfd-attach probe at directory open: INLINE identity compare
    /// (every opendir reaches this; no indirect call for other dirs).
    #[inline]
    pub fn is_piddir_ops(ops: &'static FileOps) -> bool {
        is_ops(Some(ops), path_hooks().proc_piddir_ops)
    }

    /// The synthetic /proc/[pid]/fd directory table (open_procfs_dir).
    pub fn fd_dir_ops() -> &'static super::super::inode::INodeOps {
        path_hooks().proc_fd_dir_ops
    }

    /// coredump's core_pattern snapshot.
    pub fn core_pattern() -> Vec<u8> {
        (path_hooks().procfs_core_pattern)()
    }
}

// ---- tmpfs (mount instances, fault routing, fallocate) ----

pub mod tmpfs {
    use super::*;

    pub fn ensure_registered() {
        (boot_hooks().tmpfs_ensure_registered)()
    }

    pub fn create_mount_instance(
        data: Option<&str>,
        tag: Option<TmpfsTagKind>,
    ) -> alloc::sync::Arc<Inode> {
        (boot_hooks().tmpfs_create_mount_instance)(data, tag.unwrap_or(TmpfsTagKind::None))
    }

    /// mm/policy's file-identity probe (SHARED-fault routing). File-fault
    /// path: INLINE identity compare against the published table — the
    /// pre-trace check, verbatim, with no crossing for any file kind.
    #[inline]
    pub fn is_tmpfs_file(file: &File) -> bool {
        is_ops(file.get_ops(), path_hooks().tmpfs_file_ops)
    }

    /// vfs_fallocate's tmpfs arm (None = not a tmpfs inode).
    pub fn fallocate(
        inode: &Inode,
        mode: i32,
        offset: u64,
        len: u64,
    ) -> Option<Result<(), i32>> {
        (path_hooks().tmpfs_fallocate)(inode, mode, offset, len)
    }
}

// The type definitions this file declared live in fs/types.rs; every impl
// and function that uses them stays in this file. Re-exported so the
// existing `fs::leaf_glue::…` paths are unchanged.
pub use super::types::{TmpfsTagKind, InputEventReg};

/// The RAM root as the framework sees it: the rootfs superblock's
/// path-shaped API as a framework-owned trait. The cpio unpacker stays
/// framework-side (initrd.rs) and the tests seed/verify through it; the
/// rootfs leaf (fs crate) implements it. Paths are plain strings —
/// no node handles cross the boundary.
pub trait RamfsView {
    fn rfs_mkdir(&self, path: &str) -> Result<(), i32>;
    /// A mount-point directory (with mode).
    fn rfs_create_dir(&self, path: &str, mode: u32) -> Result<(), i32>;
    fn rfs_symlink(&self, target: &str, path: &str) -> Result<(), i32>;
    fn rfs_link(&self, first: &str, path: &str) -> Result<(), i32>;
    fn rfs_create_file(&self, path: &str, data: Vec<u8>) -> Result<(), i32>;
    /// Replace-step for overwrite semantics: drop `base` under `parent`.
    fn rfs_remove_child(&self, parent: &str, base: &str);
    /// Apply permission bits + mtime to the node at `path`.
    fn rfs_set_node_meta(&self, path: &str, mode: u32, mtime: u64);
    /// Presence + kind probe (None = absent; Some(true) = directory).
    fn rfs_lookup_kind(&self, path: &str) -> Option<bool>;
    /// Seed a file (the tests' create_file path).
    fn rfs_seed_file(&self, path: &str, data: Vec<u8>) -> Result<(), i32>;
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
