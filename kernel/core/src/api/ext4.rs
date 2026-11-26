//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-5 FLIP SEAM: the ext4/jbd2 service hook table (the FsHooks
//! pattern, second table — the OnceSlot is single-writer so the ext4
//! crate owns its own table; fs keeps FsHooks). Every framework
//! call INTO ext4 or jbd2 dispatches through THIS nullable table —
//! inert until the ext4 crate registers at boot (bin calls its
//! register_all before kmain; the in-tree interim registers from
//! ext4::init). The framework never names the ext4/jbd2 modules.
//!
//! HOT-PATH DISCIPLINE (the S1 lesson): no fn-pointer indirection was
//! added to the read/write DATA path. The rows that sit on warm paths
//! are (a) the journal tag on metadata block updates (one indirect
//! call per journaled metadata write, replacing two direct calls), and
//! (b) the writeback inode flush (per writeback inode, not per page).
//! The page-fault demand-populate path was devirtualized entirely: it
//! rides the new FileOps.fault vtable slot (a per-op-slot dispatch the
//! file layer already uses), NOT a hook row. The exthunt diagnostic
//! rows are arm-gated at the call sites (one tracepoint-arm load, no
//! indirect call, when the hunt is off).

#![forbid(unsafe_code)]
use crate::fs::types::BufferHead;
use crate::fs::types::{Inode, INodeOps};
use crate::fs::types::File;
use crate::drivers::types::GenDisk;
use alloc::sync::Arc;

/// The ext4 service hook set. Signatures copy the leaf's current fns
/// verbatim where a single fn moves; folded rows (fallocate, kmsg,
/// writeback) document what the crate-side body owns.
#[derive(Clone, Copy)]
pub struct Ext4Hooks {
    // ---- boot / mount lifecycle (cold) ----
    /// ext4::init — filesystem-type registration.
    pub init: fn(),
    /// mount_ext4: full root mount with journal replay.
    pub mount_root: fn(&'static GenDisk) -> Result<(), i32>,
    /// create_root_inode: the root dentry-tree inode.
    pub root_inode: fn() -> Arc<Inode>,
    /// is_mounted.
    pub mounted: fn() -> bool,
    /// unmount_ext4 (root unmount).
    pub unmount: fn(),
    /// mount_loop_instance: second-instance mount (loop/multi-disk).
    pub mount_instance: fn(&'static GenDisk, bool) -> Result<(Arc<Inode>, bool), i32>,
    /// loop_instance_invalidate (loop rebind drops the stale instance).
    pub loop_invalidate: fn(&GenDisk),
    /// Boot-conf read (init.rs): read a file from a disk before VFS.
    pub boot_read_file: fn(&'static GenDisk, &str) -> Option<alloc::vec::Vec<u8>>,
    /// fsctx switch_root's ext-family instance mount: resolves the
    /// source (loop idx or named disk) and mounts a second instance;
    /// returns the root inode (the whole old fsctx match arm).
    pub fsctx_mount_instance: fn(&str, bool) -> Result<(Arc<Inode>, bool), i32>,
    /// The mount-table row registration for ext4/ext3/ext2 — MUST run
    /// at the register_builtin_handlers boot point (mount table ready),
    /// never at rust_main time (the fs 'R'-loop conviction).
    pub mount_rows_register: fn(),

    // ---- super-op set (cold: sync/shutdown/remount) ----
    /// sync(2) ordered-data arm: flush the flushed files' DATA blocks.
    pub sync_flushed_data: fn(&[(u64, u64)]) -> usize,
    /// sync(2) ordered-data arm: persist the inode-table blocks.
    pub sync_inode_tables: fn(&[(u64, u64)]) -> usize,
    /// ext4_shutdown_sync (reboot/sysrq emergency sync).
    pub shutdown_sync: fn() -> Result<(), i32>,
    /// Remount ro-latch sync: the leaf's internal read_only follows the
    /// VFS remount flag ORed with its own metadata_csum downgrade.
    pub remount_ro_latch: fn(bool),
    /// fallocate (folded row): the leaf's ext4_fallocate_on PLUS the
    /// VFS-inode/Ext4Inode cache-refresh epilogue — both ext4-internal.
    pub fallocate: fn(&Inode, i32, u64, u64) -> Result<(), i32>,
    /// The legacy default inode-ops (inodes with ops == None).
    pub default_inode_ops: fn() -> Option<&'static INodeOps>,
    /// Identity probe: is this the ext4 inode-ops table? (registry-free
    /// shape — the leaf compares its own static.)
    pub is_ext4_inode_ops: fn(&INodeOps) -> bool,
    /// Negative-dentry memo gate: is `ops.lookup` the ext4 lookup fn?
    pub is_ext4_lookup: fn(&INodeOps) -> bool,
    /// O_DIRECT write probe: is this file an ext4 file? (file.rs write
    /// dispatch rides the FileOps.write_at slot; this row is only the
    /// residual identity check.)
    pub is_ext4_file: fn(&File) -> bool,

    // ---- writeback probe (warm: per writeback inode) ----
    /// Folded row: resolve_writeback_inode + ext4_flush_dirty_inode.
    pub writeback_flush: fn(fs_id: u64, ino: u64, first_index: u64, last_index: u64) -> usize,

    // ---- persistent kmsg archive (klogd; cold, one flush/period) ----
    /// Resolve (or create) /var/log/kmsg — returns the ext4 inode num.
    pub kmsg_resolve: fn() -> Option<u32>,
    /// ext4_kernel_write_at (the archive append).
    pub kmsg_write_at: fn(ino: u32, offset: u64, data: &[u8]) -> isize,

    // ---- journal seam (warm: per journaled metadata block) ----
    /// Journal tag for a dirty buffer: the crate resolves the current
    /// task's handle and calls jbd2_journal_dirty_metadata. `handle` is
    /// the task slot's raw pointer (type-erased).
    pub journal_dirty: fn(handle: *mut (), bh: *mut BufferHead),
    /// Journal forget (bio's victim-eviction path).
    pub journal_forget: fn(handle: *mut (), bh: *mut BufferHead),
    /// Exit-path journal unwind: drop this task's transaction refcount
    /// and clear the slot (INV-PROC-JOURNAL-1 stays crate-side).
    pub exit_journal_unwind: fn(),

    // ---- diagnostics (arm-gated rows: the call sites check the trace
    //      arm flag first; no indirect call when the hunt is off) ----
    pub exthunt_phys_watched: fn(u64) -> bool,
    pub exthunt_evict_unhash: fn(u64, bool),
    pub exthunt_evict_done: fn(u64, i32),
    pub exthunt_fab: fn(u64, bool, bool),
    pub exthunt_bc_blind: fn(u64),
    pub exthunt_rxbhit: fn(u64, *const u8, u8, u8),
    /// demand-read census (futex diagnostics readout).
    pub demand_read_stats: fn() -> (u64, u64, u64, u64),
}

impl Ext4Hooks {
    /// The inert default: every probe false, every mount busy, every
    /// counter zero. Semantics identical to "ext4 not yet registered"
    /// (pre-flip boots without the crate, and the window between
    /// rust_main and register_all).
    pub const EMPTY: Ext4Hooks = Ext4Hooks {
        init: || (),
        mount_root: |_| Err(crate::errno::Errno::NoSuchDevice.as_neg_i32()),
        root_inode: || unreachable!("ext4 root_inode before registration"),
        mounted: || false,
        unmount: || (),
        mount_instance: |_, _| Err(crate::errno::Errno::DeviceOrResourceBusy.as_neg_i32()),
        loop_invalidate: |_| (),
        boot_read_file: |_, _| None,
        fsctx_mount_instance: |_, _| Err(crate::errno::Errno::NoSuchDevice.as_neg_i32()),
        mount_rows_register: || (),
        sync_flushed_data: |_| 0,
        sync_inode_tables: |_| 0,
        shutdown_sync: || Ok(()),
        remount_ro_latch: |_| (),
        fallocate: |_, _, _, _| Err(crate::errno::Errno::OperationNotSupported.as_neg_i32()),
        default_inode_ops: || None,
        is_ext4_inode_ops: |_| false,
        is_ext4_lookup: |_| false,
        is_ext4_file: |_| false,
        writeback_flush: |_, _, _, _| 0,
        kmsg_resolve: || None,
        kmsg_write_at: |_, _, _| 0,
        journal_dirty: |_, _| (),
        journal_forget: |_, _| (),
        exit_journal_unwind: || (),
        exthunt_phys_watched: |_| false,
        exthunt_evict_unhash: |_, _| (),
        exthunt_evict_done: |_, _| (),
        exthunt_fab: |_, _, _| (),
        exthunt_bc_blind: |_| (),
        exthunt_rxbhit: |_, _, _, _| (),
        demand_read_stats: || (0, 0, 0, 0),
    };
}

pub static EXT4_HOOKS: crate::safety::slot::OnceSlot<Ext4Hooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins).
pub fn ext4_register_hooks(hooks: Ext4Hooks) {
    let _ = EXT4_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static Ext4Hooks {
    EXT4_HOOKS.get().unwrap_or(&Ext4Hooks::EMPTY)
}

// ---- ergonomic accessors (the fs_hooks shape) ----

pub fn init() {
    (hooks().init)()
}
pub fn mount_root(disk: &'static GenDisk) -> Result<(), i32> {
    (hooks().mount_root)(disk)
}
pub fn root_inode() -> Arc<Inode> {
    (hooks().root_inode)()
}
pub fn mounted() -> bool {
    (hooks().mounted)()
}
pub fn unmount() {
    (hooks().unmount)()
}
pub fn mount_instance(disk: &'static GenDisk, ro: bool) -> Result<(Arc<Inode>, bool), i32> {
    (hooks().mount_instance)(disk, ro)
}
pub fn loop_invalidate(disk: &GenDisk) {
    (hooks().loop_invalidate)(disk)
}
pub fn boot_read_file(disk: &'static GenDisk, path: &str) -> Option<alloc::vec::Vec<u8>> {
    (hooks().boot_read_file)(disk, path)
}
pub fn fsctx_mount_instance(src: &str, ro: bool) -> Result<(Arc<Inode>, bool), i32> {
    (hooks().fsctx_mount_instance)(src, ro)
}
pub fn mount_rows_register() {
    (hooks().mount_rows_register)()
}
pub fn sync_flushed_data(targets: &[(u64, u64)]) -> usize {
    (hooks().sync_flushed_data)(targets)
}
pub fn sync_inode_tables(targets: &[(u64, u64)]) -> usize {
    (hooks().sync_inode_tables)(targets)
}
pub fn shutdown_sync() -> Result<(), i32> {
    (hooks().shutdown_sync)()
}
pub fn remount_ro_latch(ro: bool) {
    (hooks().remount_ro_latch)(ro)
}
pub fn fallocate(inode: &Inode, mode: i32, offset: u64, len: u64) -> Result<(), i32> {
    (hooks().fallocate)(inode, mode, offset, len)
}
pub fn default_inode_ops() -> Option<&'static INodeOps> {
    (hooks().default_inode_ops)()
}
pub fn is_ext4_inode_ops(ops: &INodeOps) -> bool {
    (hooks().is_ext4_inode_ops)(ops)
}
pub fn is_ext4_lookup(ops: &INodeOps) -> bool {
    (hooks().is_ext4_lookup)(ops)
}
pub fn is_ext4_file(file: &File) -> bool {
    (hooks().is_ext4_file)(file)
}
pub fn writeback_flush(fs_id: u64, ino: u64, first_index: u64, last_index: u64) -> usize {
    (hooks().writeback_flush)(fs_id, ino, first_index, last_index)
}
pub fn kmsg_resolve() -> Option<u32> {
    (hooks().kmsg_resolve)()
}
pub fn kmsg_write_at(ino: u32, offset: u64, data: &[u8]) -> isize {
    (hooks().kmsg_write_at)(ino, offset, data)
}
pub fn journal_dirty(handle: *mut (), bh: *mut BufferHead) {
    (hooks().journal_dirty)(handle, bh)
}
pub fn journal_forget(handle: *mut (), bh: *mut BufferHead) {
    (hooks().journal_forget)(handle, bh)
}
pub fn exit_journal_unwind() {
    (hooks().exit_journal_unwind)()
}
pub fn exthunt_phys_watched(blocknr: u64) -> bool {
    (hooks().exthunt_phys_watched)(blocknr)
}
pub fn exthunt_evict_unhash(blocknr: u64, dirty: bool) {
    (hooks().exthunt_evict_unhash)(blocknr, dirty)
}
pub fn exthunt_evict_done(blocknr: u64, rc: i32) {
    (hooks().exthunt_evict_done)(blocknr, rc)
}
pub fn exthunt_fab(blocknr: u64, skipped_evicting: bool, sync_path: bool) {
    (hooks().exthunt_fab)(blocknr, skipped_evicting, sync_path)
}
pub fn exthunt_bc_blind(blocknr: u64) {
    (hooks().exthunt_bc_blind)(blocknr)
}
pub fn exthunt_rxbhit(blocknr: u64, buf: *const u8, b0: u8, b1: u8) {
    (hooks().exthunt_rxbhit)(blocknr, buf, b0, b1)
}
pub fn demand_read_stats() -> (u64, u64, u64, u64) {
    (hooks().demand_read_stats)()
}
