//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 FLIP SEAM: the fs-service hook table (the NetHooks pattern).
//! The framework's per-File op hooks (pipe/fifo/memfd), the lifecycle
//! notify fanout, and the ops-identity probes dispatch through THIS
//! nullable table — no-op until the fs crate registers at boot
//! (bin calls its register_all before kmain, the composition
//! contract). The framework never names a service leaf.
//!
//! This is the rung-3 table only (pipe/fifo/inotify/memfd/selinuxfs).
//! The substrate/boot leaves that moved later (fs-substrate tranche:
//! tmpfs, procfs, rootfs, devfs, sysfs, debugfs, tracefs, cgroup, pty,
//! tty, char_dev) cross through `crate::api::fs` — three tables split
//! by lifecycle (crate-plan §3a rule 1), including every leaf's
//! mount-table registration.

#![forbid(unsafe_code)]
use crate::fs::file::File;
use crate::fs::inode::Inode;

/// The fs-service hook set.
#[derive(Clone, Copy)]
pub struct FsHooks {
    // ---- lifecycle notify (inotify) ----
    pub notify: fn(Option<&Inode>, Option<&Inode>, u32, Option<&[u8]>, u32),
    pub notify_file: fn(&File, u32),
    pub notify_file_close: fn(&File),
    pub alloc_cookie: fn() -> u32,
    // ---- pipe per-File ops ----
    pub pipe_stat: fn(&File, &mut crate::fs::Stat) -> Option<i32>,
    pub pipe_get_sz: fn(&File) -> Option<usize>,
    pub pipe_set_sz: fn(&File, usize) -> Result<usize, i32>,
    pub pipe_oasync_register: fn(&File, i32),
    pub pipe_oasync_unregister: fn(u64),
    /// The futex park-dump snapshot: (fill, capacity, r_open, w_open).
    pub pipe_diag_snapshot: fn(&File) -> Option<(usize, usize, bool, bool)>,
    // ---- fifo ----
    pub fifo_forget: fn(u64, u64),
    /// The special-open path: peer resolution + open, one call.
    pub fifo_open: fn(u64, u64, u32) -> Result<alloc::sync::Arc<File>, i32>,
    // ---- memfd seals ----
    pub memfd_add_seals: fn(&File, u32) -> Result<(), i32>,
    pub memfd_get_seals: fn(&File) -> Result<u32, i32>,
    pub memfd_name_by_ino: fn(u64) -> Option<&'static str>,
    // ---- ops-identity probes (fd-kind census + futex dumps) ----
    pub is_pipe_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_fifo_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_inotify_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_memfd_ops: fn(&'static crate::fs::FileOps) -> bool,
    // ---- misc leaf surfaces ----
    /// fsctx's selinuxfs root-inode factory.
    pub selinuxfs_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    /// procfs sysctl: /proc/sys/fs/inotify/max_queued_events value.
    pub inotify_max_queued_events: fn() -> usize,
    pub inotify_set_max_queued_events: fn(usize),
    /// procfs fdinfo: an inotify fd's watch rows.
    pub inotify_fdinfo_watches: fn(&File) -> Option<alloc::vec::Vec<(i32, u64, u64, u32)>>,
}

impl FsHooks {
    pub const EMPTY: FsHooks = FsHooks {
        notify: |_, _, _, _, _| {},
        notify_file: |_, _| {},
        notify_file_close: |_| {},
        alloc_cookie: || 0,
        pipe_stat: |_, _| None,
        pipe_get_sz: |_| None,
        pipe_set_sz: |_, _| Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
        pipe_oasync_register: |_, _| {},
        pipe_oasync_unregister: |_| {},
        pipe_diag_snapshot: |_| None,
        fifo_forget: |_, _| {},
        fifo_open: |_, _, _| Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        memfd_add_seals: |_, _| Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
        memfd_get_seals: |_| Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
        memfd_name_by_ino: |_| None,
        is_pipe_ops: |_| false,
        is_fifo_ops: |_| false,
        is_inotify_ops: |_| false,
        is_memfd_ops: |_| false,
        selinuxfs_root_inode: || None,
        inotify_max_queued_events: || 16384,
        inotify_set_max_queued_events: |_| {},
        inotify_fdinfo_watches: |_| None,
    };
}

pub static FS_HOOKS: crate::safety::slot::OnceSlot<FsHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins).
pub fn fs_register_hooks(hooks: FsHooks) {
    let _ = FS_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static FsHooks {
    FS_HOOKS.get().unwrap_or(&FsHooks::EMPTY)
}

pub fn notify(parent: Option<&Inode>, inode: Option<&Inode>, mask: u32, name: Option<&[u8]>, cookie: u32) {
    (hooks().notify)(parent, inode, mask, name, cookie)
}
pub fn notify_file(file: &File, mask: u32) {
    (hooks().notify_file)(file, mask)
}
pub fn notify_file_close(file: &File) {
    (hooks().notify_file_close)(file)
}
pub fn alloc_cookie() -> u32 {
    (hooks().alloc_cookie)()
}

// ---- ergonomic accessors (the net_hooks shape) ----
pub fn is_pipe_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_pipe_ops)(ops) }
pub fn is_fifo_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_fifo_ops)(ops) }
pub fn is_inotify_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_inotify_ops)(ops) }
pub fn is_memfd_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_memfd_ops)(ops) }
pub fn pipe_diag_snapshot(file: &File) -> Option<(usize, usize, bool, bool)> { (hooks().pipe_diag_snapshot)(file) }
pub fn memfd_name_by_ino(ino: u64) -> Option<&'static str> { (hooks().memfd_name_by_ino)(ino) }

// ---------------------------------------------------------------------------
// Joined into this file from fs/leaf_hooks.rs (pass 1b, seed-core-design 2c.4:
// the interface tables of one subsystem in one file). Everything below
// is that file as it was, except for what the join itself needed (see
// the lines marked `join:`). Its header was a module doc comment and is
// kept as plain comments; its own #![forbid(unsafe_code)] line is gone
// because the one at the top of this file covers the whole module.
// Two edits were forced by the join, both to the joined part's imports:
// its own `use crate::fs::file::File;` is dropped, and `Inode` leaves
// `use crate::fs::inode::{..}` (the first half imports both).
// ---------------------------------------------------------------------------
// MIT License
//
// Copyright (c) 2026 Fei Wang
//
// The substrate/boot leaf interface tables (fs-substrate tranche): the
// framework -> fs crossing for the eleven leaves that left the
// framework (debugfs, tracefs, cgroup, sysfs, char_dev, tty, pty,
// rootfs, tmpfs, devfs, procfs). THREE tables, one per lifecycle
// (crate-plan §3a rule 1 — split by concern, never a mega-table; FsHooks
// stays the rung-3 pipe/fifo/inotify/memfd table):
//
//   LeafBootHooks      boot + mount lifecycle: lib.rs's boot sequence,
//                      fsctx's mount arms, the mount table, initrd
//   LeafPopulateHooks  kernel-side population: drivers, dfx, the trace
//                      subsystem, the module loader, printk
//   LeafPathHooks      per-syscall / per-fault dispatch: the VFS core,
//                      mm policy, vt, loop_dev, coredump, console input
//
// NOTHING here runs at rust_main time. Registration only publishes the
// tables (plain Copy values, no allocation); each row body runs when the
// framework calls it. The mount registration in particular is invoked by
// mount_table::register_builtin_handlers at ITS boot point — calling it
// at rust_main time wedges x86_64 before the console exists (the boot.S
// 'R'-beacon reboot loop).
//
// crate-plan §3a rule 2 (static dispatch first on data paths): the path
// table carries the ops-table IDENTITIES the call sites compare inline
// (leaf_glue), so no identity probe on a per-fault or per-ioctl path
// costs an indirect call; the two devfs logic rows reached from the
// file-fault path are gated inline on the same identity / on fs_id ==
// FS_ID_DEVFS, so every other filesystem crosses zero times per fault.
//
// Row signatures carry the leaf's native types (crate-plan §3a rule 3);
// the adapters in leaf_glue contain no logic.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::dev_t::DevNo;
use crate::fs::file::FileOps;
use crate::fs::inode::{InodeMode, INodeOps};
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

// ---- LocksHooks: the locks-family cold rows (from fs/lock_hooks.rs) ----
use crate::fs::locks::LockOwner;
// The fcntl lock arms plus the slow release bodies the framework
// half's count gates arm; the forwarding wrappers stay in
// fs/lock_hooks.rs until pass 2 files them as hot-path code.
/// The locks-family cold rows: the fcntl lock arms plus the slow
/// release bodies the framework half's count gates arm. Signatures
/// carry the family's native types (crate-plan §3a rule 3).
#[derive(Clone, Copy)]
pub struct LocksHooks {
    // ---- slow release bodies (armed by the framework half's count
    // gates; run only while locks/leases are actually held) ----
    /// flock table release for a dying description (FLOCK_COUNT != 0).
    pub flock_release_slow: fn(u64),
    /// OFD record-lock release for a dying description.
    pub ofd_release_slow: fn(u64),
    /// POSIX close-event release for the closing tgid on an inode key.
    pub posix_release_slow: fn((u64, u64)),
    /// Lease release for a dying description (LEASE_COUNT != 0).
    pub lease_release_slow: fn(u64),
    /// The lease-break machinery (a conflicting lease exists).
    pub break_lease_slow: fn(&crate::fs::inode::Inode, bool) -> Result<(), crate::errno::Errno>,
    // ---- fcntl arms (cold: only lock-taking syscalls reach them) ----
    /// F_GETLK conflict probe.
    pub posix_test_lock: fn(&crate::fs::file::File, LockOwner, bool, u64, u64) -> Option<crate::fs::locks::LockConflict>,
    /// F_SETLK/W core (kind is a F_*_KIND byte).
    pub posix_set_lock: fn(&crate::fs::file::File, LockOwner, u8, u64, u64, bool) -> Result<(), crate::errno::Errno>,
    /// F_SETLEASE / F_GETLEASE.
    pub set_lease: fn(&crate::fs::file::File, i16) -> Result<(), crate::errno::Errno>,
    pub get_lease: fn(&crate::fs::file::File) -> i16,
}

impl LocksHooks {
    /// The pre-registration table: no locks exist without the crate, so
    /// every slow body is a no-op and every setter reports EINVAL (the
    /// pre-flip behavior of an unregistered family).
    pub const EMPTY: LocksHooks = LocksHooks {
        flock_release_slow: |_| {},
        ofd_release_slow: |_| {},
        posix_release_slow: |_| {},
        lease_release_slow: |_| {},
        break_lease_slow: |_, _| Ok(()),
        posix_test_lock: |_, _, _, _, _| None,
        posix_set_lock: |_, _, _, _, _, _| Err(crate::errno::Errno::InvalidArgument),
        set_lease: |_, _| Err(crate::errno::Errno::InvalidArgument),
        // F_UNLCK (i16): no lease is ever held.
        get_lease: |_| 2,
    };
}

pub static LOCKS_HOOKS: OnceSlot<LocksHooks> = OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins).
pub fn register_locks_hooks(hooks: LocksHooks) {
    let _ = LOCKS_HOOKS.publish(hooks);
}
