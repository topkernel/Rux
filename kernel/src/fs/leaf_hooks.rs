//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The substrate/boot leaf interface tables (fs-substrate tranche): the
//! framework -> fs crossing for the eleven leaves that left the
//! framework (debugfs, tracefs, cgroup, sysfs, char_dev, tty, pty,
//! rootfs, tmpfs, devfs, procfs). THREE tables, one per lifecycle
//! (crate-plan §3a rule 1 — split by concern, never a mega-table; FsHooks
//! stays the rung-3 pipe/fifo/inotify/memfd table):
//!
//!   LeafBootHooks      boot + mount lifecycle: lib.rs's boot sequence,
//!                      fsctx's mount arms, the mount table, initrd
//!   LeafPopulateHooks  kernel-side population: drivers, dfx, the trace
//!                      subsystem, the module loader, printk
//!   LeafPathHooks      per-syscall / per-fault dispatch: the VFS core,
//!                      mm policy, vt, loop_dev, coredump, console input
//!
//! NOTHING here runs at rust_main time. Registration only publishes the
//! tables (plain Copy values, no allocation); each row body runs when the
//! framework calls it. The mount registration in particular is invoked by
//! mount_table::register_builtin_handlers at ITS boot point — calling it
//! at rust_main time wedges x86_64 before the console exists (the boot.S
//! 'R'-beacon reboot loop).
//!
//! crate-plan §3a rule 2 (static dispatch first on data paths): the path
//! table carries the ops-table IDENTITIES the call sites compare inline
//! (leaf_glue), so no identity probe on a per-fault or per-ioctl path
//! costs an indirect call; the two devfs logic rows reached from the
//! file-fault path are gated inline on the same identity / on fs_id ==
//! FS_ID_DEVFS, so every other filesystem crosses zero times per fault.
//!
//! Row signatures carry the leaf's native types (crate-plan §3a rule 3);
//! the adapters in leaf_glue contain no logic.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::dev_t::DevNo;
use crate::fs::file::File;
use crate::fs::file::FileOps;
use crate::fs::inode::{Inode, InodeMode, INodeOps};
use crate::fs::leaf_glue::{InputEventReg, RamfsView, TmpfsTagKind};
use crate::fs::Stat;
use crate::safety::slot::OnceSlot;

type ShowFn = Box<dyn Fn() -> Vec<u8> + Send + Sync>;
type StoreFn = Box<dyn Fn(&[u8]) -> i32 + Send + Sync>;

// ============================================================================
// 1. Boot + mount lifecycle
// ============================================================================

/// The boot/mount lifecycle rows: lib.rs's boot sequence, fsctx's mount
/// arms, the mount table and initrd reach the leaves through these.
/// Frequency: once at boot, or once per mount(2) of one of these types.
#[derive(Clone, Copy)]
pub struct LeafBootHooks {
    /// Every leaf's mount-table registration in one entry (the ext4
    /// `mount_rows_register` shape). Invoked by
    /// mount_table::register_builtin_handlers — at that boot point only
    /// (see the module doc).
    pub mount_rows_register: fn(),
    // ---- debugfs ----
    pub debugfs_init: fn() -> Result<(), i32>,
    /// debugfs_root().is_some() — fsctx's lazy-init probe.
    pub debugfs_root_present: fn() -> bool,
    pub debugfs_create_root_inode: fn() -> Arc<Inode>,
    /// lib.rs boot's mount_debugfs().
    pub debugfs_mount: fn() -> Result<(), i32>,
    // ---- tracefs ----
    pub tracefs_init: fn() -> Result<(), i32>,
    /// tracefs_root().is_some() — fsctx's lazy-init probe.
    pub tracefs_root_present: fn() -> bool,
    pub tracefs_create_root_inode: fn() -> Arc<Inode>,
    /// lib.rs boot's tracefs mount_at() (the two canonical locations).
    pub tracefs_mount_at: fn(&str) -> Result<(), i32>,
    // ---- cgroup ----
    pub cgroup_init: fn() -> Result<(), i32>,
    pub cgroup_create_root_inode: fn() -> Arc<Inode>,
    pub cgroup_mount: fn(&str) -> Result<(), i32>,
    // ---- sysfs ----
    pub sysfs_init: fn() -> Result<(), i32>,
    pub sysfs_mount: fn() -> Result<(), i32>,
    pub sysfs_create_root_inode: fn() -> Arc<Inode>,
    // ---- procfs ----
    pub procfs_init: fn() -> Result<(), i32>,
    pub procfs_mount: fn() -> Result<(), i32>,
    /// get_procfs_sb().is_some() — the boot site's presence check.
    pub procfs_sb_present: fn() -> bool,
    pub procfs_create_root_inode: fn() -> Arc<Inode>,
    // ---- devfs ----
    pub devfs_init: fn(),
    /// The mounted /dev root; None before init (the pre-trace
    /// `get_root_entry()` presence check at the boot site).
    pub devfs_create_root_inode: fn() -> Option<Arc<Inode>>,
    // ---- tmpfs ----
    pub tmpfs_ensure_registered: fn(),
    /// fsctx's mount-instance factory; the tag selects the type-ops twin
    /// a fresh devpts/securityfs/mqueue root is retagged with.
    pub tmpfs_create_mount_instance: fn(Option<&str>, TmpfsTagKind) -> Arc<Inode>,
    // ---- rootfs ----
    pub rootfs_init: fn() -> Result<(), i32>,
    pub rootfs_create_root_inode: fn() -> Arc<Inode>,
    /// The RAM root as the framework's path-shaped view — initrd's
    /// cpio unpack target and the tests' seed/verify handle.
    pub rootfs_view: fn() -> Option<&'static dyn RamfsView>,
    /// init.rs's config-file probe (fs::read_file_from_rootfs).
    pub rootfs_read_file: fn(&str) -> Option<Vec<u8>>,
    // ---- initrd (the boot unpack, crate-side since the root-cold lane;
    // both rows fire once per boot from initrd::load — cold sites) ----
    /// The gzip (RFC 1952) decompressor: Err carries the historical
    /// `&'static str` reason verbatim.
    pub initrd_gunzip: fn(&[u8]) -> Result<Vec<u8>, &'static str>,
    /// The cpio (newc) unpacker over the RAM-root view.
    pub initrd_unpack_newc:
        fn(&[u8], &'static dyn RamfsView) -> Result<crate::initrd::CpioStats, &'static str>,
    // ---- char_dev ----
    /// The boot task's stdio (init.rs's init_std_fds_for_task body).
    pub char_dev_init_std_fds: fn(&crate::fs::FdTable),
}

impl LeafBootHooks {
    /// The pre-registration table: every row is the harmless no-op. The
    /// fs crate publishes the real one before kmain, so this is only
    /// ever read by builds that link the framework without the crate.
    pub const EMPTY: LeafBootHooks = LeafBootHooks {
        mount_rows_register: || {},
        debugfs_init: absent,
        debugfs_root_present: || false,
        debugfs_create_root_inode: empty_root,
        debugfs_mount: absent,
        tracefs_init: absent,
        tracefs_root_present: || false,
        tracefs_create_root_inode: empty_root,
        tracefs_mount_at: |_| Err(ABSENT),
        cgroup_init: absent,
        cgroup_create_root_inode: empty_root,
        cgroup_mount: |_| Err(ABSENT),
        sysfs_init: absent,
        sysfs_mount: absent,
        sysfs_create_root_inode: empty_root,
        procfs_init: absent,
        procfs_mount: absent,
        procfs_sb_present: || false,
        procfs_create_root_inode: empty_root,
        devfs_init: || {},
        devfs_create_root_inode: || None,
        tmpfs_ensure_registered: || {},
        tmpfs_create_mount_instance: empty_root_2,
        rootfs_init: absent,
        rootfs_create_root_inode: empty_root,
        rootfs_view: || None,
        rootfs_read_file: |_| None,
        initrd_gunzip: initrd_gunzip_absent,
        initrd_unpack_newc: initrd_unpack_absent,
        char_dev_init_std_fds: |_| {},
    };
}

/// ENOENT as a negative errno: the EMPTY answer of every fallible row.
const ABSENT: i32 = -2;

/// The EMPTY root: a bare directory inode with no ops.
fn empty_root() -> Arc<Inode> {
    Arc::new(Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755)))
}

fn empty_root_2(_: Option<&str>, _: TmpfsTagKind) -> Arc<Inode> {
    empty_root()
}

fn absent() -> Result<(), i32> {
    Err(ABSENT)
}

/// The EMPTY initrd rows: an unregistered crate means no image can be
/// unpacked — the historical string-error shape (`load()` propagates it
/// to the boot print verbatim).
fn initrd_gunzip_absent(_: &[u8]) -> Result<Vec<u8>, &'static str> {
    Err("initrd: rux-fs not registered")
}

fn initrd_unpack_absent(
    _: &[u8],
    _: &'static dyn RamfsView,
) -> Result<crate::initrd::CpioStats, &'static str> {
    Err("initrd: rux-fs not registered")
}

// ============================================================================
// 2. Kernel-side population
// ============================================================================

/// The kernel-population rows: drivers, dfx, the trace subsystem, the
/// module loader and printk create nodes through these. Frequency: probe
/// and boot time, plus one call per hotplug uevent. Cold.
#[derive(Clone, Copy)]
pub struct LeafPopulateHooks {
    // ---- debugfs ----
    pub debugfs_create_dir: fn(&str) -> bool,
    /// The boxed-closure form of debugfs::create_file (leaf_glue boxes
    /// the generic originals; the leaf's generic fn stays crate-side).
    pub debugfs_create_file: fn(&str, u32, ShowFn, Option<StoreFn>) -> bool,
    // ---- tracefs (same shapes as debugfs) ----
    pub tracefs_create_dir: fn(&str) -> bool,
    pub tracefs_create_file: fn(&str, u32, ShowFn, Option<StoreFn>) -> bool,
    pub tracefs_create_stream_file: fn(&str, u32, ShowFn) -> bool,
    // ---- sysfs ----
    /// /sys/module/<name> publish/unpublish (module load / unload).
    pub sysfs_publish_module: fn(&str),
    pub sysfs_unpublish_module: fn(&str),
    pub sysfs_register_drm_device: fn(&str),
    /// Returns the uevent SEQNUM (0 when sysfs is not up).
    pub sysfs_netdev_uevent: fn(&str, &str) -> u64,
    pub sysfs_register_block_disk_at: fn(&str, u32, u32, u64, &[(&str, &str)]) -> u64,
    pub sysfs_register_input_event: fn(&InputEventReg) -> u64,
    // ---- devfs ----
    pub devfs_mkdir: fn(&str) -> Result<(), ()>,
    pub devfs_mknod: fn(&str, DevNo, u32) -> Result<(), ()>,
    pub devfs_register_char_device: fn(DevNo, &'static FileOps) -> Result<(), ()>,
}

impl LeafPopulateHooks {
    pub const EMPTY: LeafPopulateHooks = LeafPopulateHooks {
        debugfs_create_dir: |_| false,
        debugfs_create_file: |_, _, _, _| false,
        tracefs_create_dir: |_| false,
        tracefs_create_file: |_, _, _, _| false,
        tracefs_create_stream_file: |_, _, _| false,
        sysfs_publish_module: |_| {},
        sysfs_unpublish_module: |_| {},
        sysfs_register_drm_device: |_| {},
        sysfs_netdev_uevent: |_, _| 0,
        sysfs_register_block_disk_at: |_, _, _, _, _| 0,
        sysfs_register_input_event: |_| 0,
        devfs_mkdir: |_| Err(()),
        devfs_mknod: |_, _, _| Err(()),
        devfs_register_char_device: |_, _| Err(()),
    };
}

// ============================================================================
// 3. Path dispatch
// ============================================================================

/// The per-syscall / per-fault rows plus the ops identities the call
/// sites compare inline. Each row notes the path it sits on; the ones on
/// a data path are gated inline in leaf_glue (crate-plan §3a rule 2) so
/// the common case never crosses.
#[derive(Clone, Copy)]
pub struct LeafPathHooks {
    // ---- ops-table identities: compared INLINE at the call sites (no
    // row call). The EMPTY values are tables no File/inode ever carries. ----
    /// tmpfs regular files (`is_tmpfs_file` — mm policy's fault routing).
    pub tmpfs_file_ops: &'static FileOps,
    /// devfs regular-registry files (the SHARED fault page handoff gate).
    pub devfs_reg_file_ops: &'static FileOps,
    /// /dev/loopN + loop-control (sys_ioctl's loop dispatch gate).
    pub loop_file_ops: &'static FileOps,
    /// /dev/console-class nodes (the test suite's console fd).
    pub devfs_condev_ops: &'static FileOps,
    /// procfs pid directories (the pidfd attach probe at dir open).
    pub proc_piddir_ops: &'static FileOps,
    /// The synthetic /proc/[pid]/fd directory table (open_procfs_dir).
    pub proc_fd_dir_ops: &'static INodeOps,
    // ---- console input (console.rs): per input BYTE at serial baud
    // rate and per generated signal — logic rows, not identity. ----
    /// The console tty's line discipline flags (ISIG/ECHO gating).
    pub tty_console_lflag: fn() -> u32,
    pub tty_console_echo_enabled: fn() -> bool,
    /// Serial input ingest.
    pub tty_console_receive_byte: fn(u8),
    /// The console tty's foreground process group (0 = none).
    pub tty_console_fg_pgid: fn() -> u32,
    pub tty_set_console_sink_quiet: fn(bool),
    // ---- per-syscall rows ----
    /// fstat on an INODE-LESS description only (after the pipe and
    /// socket probes) — regular files never reach it.
    pub char_dev_file_stat: fn(&File, &mut Stat) -> Option<i32>,
    /// vt's console-device backing for /dev/ttyN: read/write/poll.
    pub devfs_condev_read: fn(&File, &mut [u8]) -> isize,
    pub devfs_condev_write: fn(&File, &[u8]) -> isize,
    pub devfs_condev_poll: fn(&File, u16) -> u16,
    /// vfs_fallocate's tmpfs arm (None = not a tmpfs inode).
    pub tmpfs_fallocate: fn(&Inode, i32, u64, u64) -> Option<Result<(), i32>>,
    /// The two magic-link rows; both callers gate inline on
    /// fs_id == FS_ID_PROCFS before the call.
    pub procfs_is_valid_pid: fn(u64) -> bool,
    /// /proc/[pid]/fd/N magic-link resolution target.
    pub procfs_get_fd_file: fn(u64, u32) -> Option<Arc<File>>,
    /// coredump's core_pattern snapshot (the sysctl cell lives crate-side).
    pub procfs_core_pattern: fn() -> Vec<u8>,
    // ---- the file-fault path (mm policy, all three arches). Both rows
    // are gated INLINE in leaf_glue (devfs identity / fs_id == FS_ID_DEVFS)
    // — a fault on any other filesystem makes zero indirect calls. ----
    /// mm/policy's live-size probe for regular-registry files.
    pub devfs_reg_current_size: fn(&File) -> Option<u64>,
    /// mm/policy's SHARED fault page handoff.
    pub devfs_shared_fault_file_page: fn(&File, usize) -> Option<u64>,
}

impl LeafPathHooks {
    pub const EMPTY: LeafPathHooks = LeafPathHooks {
        tmpfs_file_ops: &NO_FILE_OPS,
        devfs_reg_file_ops: &NO_FILE_OPS,
        loop_file_ops: &NO_FILE_OPS,
        devfs_condev_ops: &NO_FILE_OPS,
        proc_piddir_ops: &NO_FILE_OPS,
        proc_fd_dir_ops: &NO_INODE_OPS,
        tty_console_lflag: || 0,
        tty_console_echo_enabled: || false,
        tty_console_receive_byte: |_| {},
        tty_console_fg_pgid: || 0,
        tty_set_console_sink_quiet: |_| {},
        char_dev_file_stat: |_, _| None,
        devfs_condev_read: |_, _| 0,
        devfs_condev_write: |_, _| 0,
        devfs_condev_poll: |_, _| 0,
        tmpfs_fallocate: |_, _, _, _| None,
        procfs_is_valid_pid: |_| false,
        procfs_get_fd_file: |_, _| None,
        procfs_core_pattern: || Vec::new(),
        devfs_reg_current_size: |_| None,
        devfs_shared_fault_file_page: |_, _| None,
    };
}

/// An all-empty FileOps no description ever carries: the EMPTY identity
/// (compares unequal to every real table).
static NO_FILE_OPS: FileOps = FileOps::builder().build();

/// An all-None INodeOps no inode ever carries: the EMPTY identity.
static NO_INODE_OPS: INodeOps = INodeOps {
    lookup: None,
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: None,
    readdir: None,
    readdir_fill: None,
    readdir_cacheable: false,
    open: None,
    permission: None,
    getattr: None,
    setattr: None,
    iget: None,
    destroy_inode: None,
};

// ============================================================================
// The /proc ino-space layout (vfs's magic-link dispatch)
// ============================================================================

/// First ino of the /proc/[pid]/fd/<N> magic-link range inside a pid's
/// ino block (pid * 10000 + offset + fd).
pub const PROCFS_FD_LINK_INO_OFF: u64 = 300;
/// First ino of the /proc/[pid]/ns/<kind> magic-link range.
pub const PROCFS_NS_LINK_INO_OFF: u64 = 410;

// ============================================================================
// Publication + accessors
// ============================================================================

pub static LEAF_BOOT_HOOKS: OnceSlot<LeafBootHooks> = OnceSlot::new();
pub static LEAF_POPULATE_HOOKS: OnceSlot<LeafPopulateHooks> = OnceSlot::new();
pub static LEAF_PATH_HOOKS: OnceSlot<LeafPathHooks> = OnceSlot::new();

/// The service crate's registration entries (idempotent: first wins).
pub fn register_boot_hooks(hooks: LeafBootHooks) {
    let _ = LEAF_BOOT_HOOKS.publish(hooks);
}
pub fn register_populate_hooks(hooks: LeafPopulateHooks) {
    let _ = LEAF_POPULATE_HOOKS.publish(hooks);
}
pub fn register_path_hooks(hooks: LeafPathHooks) {
    let _ = LEAF_PATH_HOOKS.publish(hooks);
}

#[inline]
pub fn boot_hooks() -> &'static LeafBootHooks {
    LEAF_BOOT_HOOKS.get().unwrap_or(&LeafBootHooks::EMPTY)
}

#[inline]
pub fn populate_hooks() -> &'static LeafPopulateHooks {
    LEAF_POPULATE_HOOKS.get().unwrap_or(&LeafPopulateHooks::EMPTY)
}

#[inline]
pub fn path_hooks() -> &'static LeafPathHooks {
    LEAF_PATH_HOOKS.get().unwrap_or(&LeafPathHooks::EMPTY)
}
