//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The rung-5 SEAM BODIES (ext4 side): the Ext4Hooks table's rows,
//! filled from THIS tree during the in-tree interim (ext4::init
//! registers) and by the ext4 crate's register_all after the
//! flip. Folded rows (fallocate epilogue, writeback resolve+flush,
//! kmsg archive) carry the framework code that used to sit at the
//! framework call sites — moved VERBATIM, one row per family.
//!
//! SELF-RECURSION RULE (the S1 boot-wedge lesson): no row body calls
//! the framework WRAPPER of its own slot (e.g. the journal rows call
//! jbd2 directly, never fs::ext4_hooks::journal_dirty).

use crate::drivers::blkdev::GenDisk;
use crate::errno;
use crate::fs::bio;
use crate::fs::ext4_hooks::{self, Ext4Hooks};
use crate::fs::file::File;
use crate::fs::inode::{Inode, INodeOps};
use crate::fs::jbd2;
use crate::process::unsafe_edges as proc_edges;

fn mount_root_row(disk: &'static GenDisk) -> Result<(), i32> {
    super::mount_ext4(disk)
}

fn root_inode_row() -> alloc::sync::Arc<Inode> {
    super::create_root_inode()
}

fn mount_instance_row(
    disk: &'static GenDisk,
    ro: bool,
) -> Result<(alloc::sync::Arc<Inode>, bool), i32> {
    super::mount_loop_instance(disk, ro)
}

/// fsctx's old "ext2"|"ext3"|"ext4" match arm, verbatim.
fn fsctx_mount_instance_row(src: &str, ro: bool) -> Result<(alloc::sync::Arc<Inode>, bool), i32> {
    // "/dev/loopN"
    if src.starts_with("/dev/loop") {
        let idx: Option<u32> = src["/dev/loop".len()..].parse().ok();
        if let Some(idx) = idx {
            let disk = crate::drivers::loop_dev::loop_disk(idx as usize)
                .ok_or(errno::Errno::NoSuchDevice.as_neg_i32())?;
            let (root, effective_ro) = super::mount_loop_instance(disk, ro)?;
            return Ok((root, effective_ro));
        }
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    // Named disk ("/dev/vdb", "/dev/block/vdc", bare "vdb").
    let trimmed = src
        .trim_start_matches("/dev/block/")
        .trim_start_matches("/dev/")
        .trim_matches('/');
    let disk = crate::drivers::virtio::get_pci_gen_disk_by_name(trimmed);
    if let Some(disk) = disk {
        let (root, effective_ro) = super::mount_loop_instance(disk, ro)?;
        return Ok((root, effective_ro));
    }
    Err(errno::Errno::NoSuchDevice.as_neg_i32())
}

fn fallocate_row(inode: &Inode, mode: i32, offset: u64, len: u64) -> Result<(), i32> {
    use super::FALLOC_FL_KEEP_SIZE;

    let fs_ptr = inode
        .private_data
        .ok_or(errno::Errno::FunctionNotImplemented.as_neg_i32())?;
    // Only ext4 inodes carry a filesystem pointer we can dispatch
    // through; verify via the ops table identity (the legacy None-ops
    // default IS the ext4 table — preserved from the old dispatch).
    if !core::ptr::eq(
        inode.ops.unwrap_or(&super::EXT4_INODE_OPS) as *const _,
        &super::EXT4_INODE_OPS as *const _,
    ) {
        return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
    }
    super::ext4_fallocate_on(
        inode.ino as u32,
        mode,
        offset,
        len,
        Some(fs_ptr as *const super::Ext4FileSystem),
    )?;

    // Cache coherence (the old vfs.rs epilogue, moved verbatim):
    // ext4_fallocate writes the on-disk inode directly — the VFS
    // inode's size and the cached Ext4Inode box (inode.sb) still hold
    // the pre-allocation size, so a subsequent read(2) through ANY
    // open fd saw EOF at the old size (reads of a fallocated file
    // returned 0 bytes). Refresh both caches. PUNCH included
    // (falloc-wedge lane, 2026-11-16): the punch rebuilds the on-disk
    // extent tree, but reads resolve through the cached Ext4Inode box
    // — a stale cached tree kept serving the punched blocks' old bytes
    // (LTP fallocate06's clear-check). Size refresh stays gated on
    // !KEEP_SIZE (a KEEP_SIZE punch must not move the published size);
    // the map/block-count refresh is unconditional.
    //
    // NO page-cache invalidation here (falloc-wedge root cause,
    // 2026-11-16): this epilogue used to call invalidate_inode on the
    // WHOLE inode after refreshing the map from disk. For a file with
    // still-dirty DELAYED-ALLOCATION pages (written before the
    // fallocate, not yet flushed) that dropped the only copy of
    // already-acknowledged writes while the refreshed map reverted to
    // the on-disk (blockless) state — reads returned zeros and the
    // later fsync had nothing left to flush (punchprobe3). The
    // allocate path never changes the content of existing pages; the
    // punch path owns its own RANGE-bounded cache discipline inside
    // ext4_fallocate_on.
    //
    // Size refresh never SHRINKS the live size: buffered writes
    // publish the VFS size immediately while the disk inode lags
    // (delayed allocation), so the freshly-read disk size can be
    // SMALLER than the live one — storing it would logically truncate
    // the file and turn the dirty tail into past-EOF discards at the
    // next flush (the same data-loss family, size-regression face).
    {
        let keep_size = mode & FALLOC_FL_KEEP_SIZE != 0;
        // fs_ptr2 is the ext4 instance from inode.private_data (the
        // typed view rides boxed_ref); inode.sb is the Box<Ext4Inode>
        // cached at open (the exclusive view rides boxed_mut).
        let fs2 = crate::syscall::edges::boxed_ref(
            fs_ptr as *const super::Ext4FileSystem,
        );
        if let Ok(fresh) = fs2.read_inode(inode.ino as u32) {
            if !keep_size {
                let live = inode.size.load(core::sync::atomic::Ordering::Acquire);
                if fresh.size > live {
                    inode.size.store(fresh.size, core::sync::atomic::Ordering::Release);
                }
            }
            if let Some(ptr) = inode.sb {
                let cached: &mut super::inode::Ext4Inode =
                    crate::syscall::edges::boxed_mut(ptr as *mut super::inode::Ext4Inode);
                cached.block = fresh.block;
                if !keep_size && fresh.size > cached.size {
                    cached.size = fresh.size;
                }
                cached.blocks = fresh.blocks;
                cached.mtime = fresh.mtime;
                cached.ctime = fresh.ctime;
            }
        }
    }
    Ok(())
}

fn default_inode_ops_row() -> Option<&'static INodeOps> {
    Some(&super::EXT4_INODE_OPS)
}

fn is_ext4_inode_ops_row(ops: &INodeOps) -> bool {
    core::ptr::eq(ops as *const _, &super::EXT4_INODE_OPS as *const _)
}

fn is_ext4_lookup_row(ops: &INodeOps) -> bool {
    ops.lookup == Some(super::ext4_lookup)
}

fn is_ext4_file_row(file: &File) -> bool {
    file.get_ops()
        .is_some_and(|ops| core::ptr::eq(ops as *const _, &super::file::EXT4_FILE_OPS as *const _))
}

/// The folded writeback probe: resolve_writeback_inode +
/// ext4_flush_dirty_inode (page_cache's three sites called exactly this
/// pair).
fn writeback_flush_row(fs_id: u64, ino: u64, first_index: u64, last_index: u64) -> usize {
    match super::resolve_writeback_inode(fs_id, ino) {
        Some(inode) => super::file::ext4_flush_dirty_inode(&inode, first_index, last_index),
        None => 0,
    }
}

/// klogd's /var/log/kmsg resolver (the old printk resolve_file body;
/// the ino cache moved crate-side — the framework kept only the
/// flush-offset bookkeeping).
fn kmsg_resolve_row() -> Option<u32> {
    use core::sync::atomic::{AtomicU64, Ordering};
    static KMSG_INO: AtomicU64 = AtomicU64::new(0);

    let fs_ptr = super::get_ext4_fs()?;
    // SAFETY: fs_ptr is the mount's instance word (get_ext4_fs's
    // non-null GLOBAL) — resolved through the framework edge
    // (INV-FS-GLOBALSB-1 provenance).
    let fs = crate::fs::unsafe_edges::static_ref_unchecked(fs_ptr as *mut super::Ext4FileSystem);
    let cached = KMSG_INO.load(Ordering::Relaxed);
    if cached != 0 {
        // Still alive? A deleted file's inode read fails; reset and
        // fall through to re-create next time.
        match fs.read_inode(cached as u32) {
            Ok(_) => return Some(cached as u32),
            Err(_) => {
                KMSG_INO.store(0, Ordering::Relaxed);
                return None;
            }
        }
    }
    if let Ok((ino, _inode)) = fs.lookup_path("/var/log/kmsg") {
        KMSG_INO.store(ino as u64, Ordering::Relaxed);
        return Some(ino);
    }
    match super::create_file("/var/log/kmsg", 0o644) {
        Ok(vfs_inode) => {
            KMSG_INO.store(vfs_inode.ino as u64, Ordering::Relaxed);
            Some(vfs_inode.ino as u32)
        }
        Err(_) => None,
    }
}

fn kmsg_write_at_row(ino: u32, offset: u64, data: &[u8]) -> isize {
    let Some(fs_ptr) = super::get_ext4_fs() else {
        return 0;
    };
    // SAFETY: same provenance as kmsg_resolve_row.
    let fs = crate::fs::unsafe_edges::static_ref_unchecked(fs_ptr as *mut super::Ext4FileSystem);
    super::file::ext4_kernel_write_at(fs, ino, offset, data)
}

fn journal_dirty_row(handle: *mut (), bh: *mut bio::BufferHead) {
    // The slot is set/cleared by this task around the enclosing ext4
    // operation; the typed view rides boxed_mut.
    if !handle.is_null() {
        let h = handle as *mut jbd2::journal::Handle;
        let _ = jbd2::jbd2_journal_dirty_metadata(crate::syscall::edges::boxed_mut(h), bh);
    }
}

fn journal_forget_row(handle: *mut (), bh: *mut bio::BufferHead) {
    // As journal_dirty_row.
    if !handle.is_null() {
        let h = handle as *mut jbd2::journal::Handle;
        let _ = jbd2::jbd2_journal_forget(crate::syscall::edges::boxed_mut(h), bh);
    }
}

/// The exit-path journal unwind (the old exit.rs body): drop this
/// task's transaction refcount and clear the slot (INV-PROC-JOURNAL-1).
fn exit_journal_unwind_row() {
    let Some(cur) = crate::sched::current() else { return };
    if let Some(handle) = proc_edges::journal_handle::<jbd2::journal::Handle>(cur) {
        if let Some(txn) = handle.h_transaction.clone() {
            txn.t_updates.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
        }
        proc_edges::clear_journal_handle(cur);
    }
}

/// The ext-family mount_table row (the old do_mount head-arm + the
/// "ext4"|"ext3" match arm, verbatim order: loop special case first,
/// then named-disk, then the legacy boot-disk path; non-loop ext2
/// keeps the old EINVAL fall-through).
pub fn mount_instance_table_row(
    source: &str,
    target: &str,
    fs_type: &str,
    _data: Option<&str>,
    mnt_flags: crate::fs::mount::MntFlags,
) -> Result<(), i32> {
    // ext2/ext4 from a LOOP device: a second ext4 instance on the
    // loop's GenDisk (LTP mount/umount tests: losetup an mkfs.ext2
    // image, mount, verify, umount). The boot root keeps its own
    // instance. "ext3" is the ext4 driver's alias (reference >= 4.3:
    // ext3 mounts through ext4).
    if (fs_type == "ext2" || fs_type == "ext3" || fs_type == "ext4")
        && source.starts_with("/dev/loop")
    {
        let minor: Option<u32> = source["/dev/loop".len()..].parse().ok();
        if let Some(idx) = minor {
            let disk = crate::drivers::loop_dev::loop_disk(idx as usize)
                .ok_or(errno::Errno::NoSuchDevice.as_neg_i32())?;
            let (root, effective_ro) = super::mount_loop_instance(disk, mnt_flags.is_readonly())?;
            // Bring-up policy: the ext4 layer downgraded an rw request
            // (metadata_csum) — mirror it in the VFS mount flags so all
            // write paths fail EROFS at the VFS, not inside ext4.
            let mnt_flags = if effective_ro && !mnt_flags.is_readonly() {
                crate::fs::mount::MntFlags::new(mnt_flags.bits() | crate::fs::mount::MntFlags::MNT_READONLY)
            } else {
                mnt_flags
            };
            crate::fs::vfs::vfs_mount(target, root, mnt_flags);
            crate::fs::mount::register_mount(
                source,
                target,
                fs_type,
                if mnt_flags.is_readonly() { "ro" } else { "rw" },
            );
            return Ok(());
        }
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Non-loop ext2 answered EINVAL at the old match fall-through.
    if fs_type == "ext2" {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // SOURCE RESOLUTION (OH Phase 1b): the source names the disk —
    // "/dev/vdb", "/dev/block/vdc" (ueventd's node dir), or bare "vdb"
    // all resolve through the vdX letter. The named disk is mounted as
    // a SEPARATE instance (the loop-device discipline) when a root
    // ext4 is already live, so `mount /dev/block/vdb /usr` on an
    // initrd root gives /usr the SECOND disk, not a second view of the
    // first.
    let disk_of_source = |src: &str| -> Option<&'static GenDisk> {
        let trimmed = src
            .trim_start_matches("/dev/block/")
            .trim_start_matches("/dev/")
            .trim_matches('/');
        crate::drivers::virtio::get_pci_gen_disk_by_name(trimmed)
    };

    if let Some(disk) = disk_of_source(source) {
        if !super::is_mounted() {
            // First ext4 becomes THE root instance (full mount path,
            // journal replay — the OH init mounting /dev/block/vdb
            // before any other ext4).
            super::mount_ext4_flags(disk, mnt_flags.is_readonly()).map_err(|e| e as i32)?;
            crate::fs::vfs::vfs_mount(target, super::create_root_inode(), mnt_flags);
        } else {
            // Additional disks: separate instance (mount_loop_instance
            // discipline — no journal replay, like a fresh scratch
            // mount; the OH images are clean).
            let (root, effective_ro) =
                super::mount_loop_instance(disk, mnt_flags.is_readonly()).map_err(|e| e as i32)?;
            let mnt_flags = if effective_ro && !mnt_flags.is_readonly() {
                crate::fs::mount::MntFlags::new(mnt_flags.bits() | crate::fs::mount::MntFlags::MNT_READONLY)
            } else {
                mnt_flags
            };
            crate::fs::vfs::vfs_mount(target, root, mnt_flags);
        }
        crate::fs::mount::register_mount(
            source,
            target,
            fs_type,
            if mnt_flags.is_readonly() { "ro" } else { "rw" },
        );
        return Ok(());
    }

    // No source disk identity (anonymous "ext4" mount, LTP-style
    // mount /dev/vda on the boot disk): the legacy boot-disk path.
    // R14-12 (F22): refuse to re-run mount_ext4 on a live filesystem —
    // a second `mount -t ext4` rebuilt the Ext4, re-ran journal
    // recovery on a live journal, swapped GLOBAL_EXT4_FS under open
    // files and leaked the old instance.
    if super::is_mounted() {
        return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
    }
    let _fs = super::get_ext4_fs().ok_or(errno::Errno::NoSuchDevice.as_neg_i32())?;
    if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
        super::mount_ext4(disk).map_err(|e| e as i32)?;
    } else if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
        // get_device() hands out a &'static VirtIOBlkDevice
        // (VIRTIO_BLK static), so its disk field borrows for the
        // machine's lifetime.
        let disk: &'static GenDisk = &virtio_dev.disk;
        super::mount_ext4(disk).map_err(|e| e as i32)?;
    } else {
        return Err(errno::Errno::NoSuchDevice.as_neg_i32());
    }
    crate::fs::vfs::vfs_mount(target, super::create_root_inode(), mnt_flags);
    crate::fs::mount::register_mount(
        "/dev/vda",
        target,
        fs_type,
        if mnt_flags.is_readonly() { "ro" } else { "rw" },
    );
    Ok(())
}

fn mount_rows_register_row() {
    crate::fs::mount_table::register(
        &["ext4", "ext3", "ext2"],
        mount_instance_table_row,
    );
}

/// Register the hook table + the mount-table rows (idempotent:
/// OnceSlot first-wins; mount_table::register dedups by name set).
/// In-tree interim: called from ext4::init (boot order: bio::init ->
/// ext4::init -> ... -> mounts). At the flip: bin's register_all
/// owns this call.
pub fn register_all() {
    ext4_hooks::ext4_register_hooks(Ext4Hooks {
        init: super::init,
        mount_root: mount_root_row,
        root_inode: root_inode_row,
        mounted: super::is_mounted,
        unmount: super::unmount_ext4,
        mount_instance: mount_instance_row,
        loop_invalidate: super::loop_instance_invalidate,
        boot_read_file: super::read_file,
        fsctx_mount_instance: fsctx_mount_instance_row,
        mount_rows_register: mount_rows_register_row,
        sync_flushed_data: super::sync_flushed_data,
        sync_inode_tables: super::sync_inode_tables,
        shutdown_sync: super::ext4_shutdown_sync,
        remount_ro_latch: |ro| {
            // The old vfs.rs remount field surgery: keep the leaf's own
            // ro latch in sync (its op-layer EROFS guard reads
            // fs.read_only, sc21); a metadata_csum image stays read-only
            // regardless (mount-time downgrade policy: the write path
            // does not maintain checksums).
            if let Some(p) = super::get_ext4_fs() {
                // The fs instance lives for the mount; read_only is a
                // plain bool whose only writers are mount setup and
                // this remount path (views ride the framework edges).
                let fs: &mut super::Ext4FileSystem =
                    crate::syscall::edges::boxed_mut(p);
                let forced_ro = fs.metadata_csum;
                fs.read_only = ro || forced_ro;
            }
        },
        fallocate: fallocate_row,
        default_inode_ops: default_inode_ops_row,
        is_ext4_inode_ops: is_ext4_inode_ops_row,
        is_ext4_lookup: is_ext4_lookup_row,
        is_ext4_file: is_ext4_file_row,
        writeback_flush: writeback_flush_row,
        kmsg_resolve: kmsg_resolve_row,
        kmsg_write_at: kmsg_write_at_row,
        journal_dirty: journal_dirty_row,
        journal_forget: journal_forget_row,
        exit_journal_unwind: exit_journal_unwind_row,
        exthunt_phys_watched: super::extent::exthunt_phys_watched,
        exthunt_evict_unhash: super::extent::exthunt_evict_unhash,
        exthunt_evict_done: super::extent::exthunt_evict_done,
        exthunt_fab: super::extent::exthunt_fab,
        exthunt_bc_blind: super::extent::exthunt_bc_blind,
        exthunt_rxbhit: super::extent::exthunt_rxbhit,
        demand_read_stats: super::file::demand_read_stats,
    });
    // The mount-table rows ride the mount_rows_register row: the
    // framework invokes it at the register_builtin_handlers boot
    // point (allocator + mount table up) — the fs 'R'-loop
    // lesson. Registering anything here (rust_main time, pre-console)
    // wedged x86_64 in the 'R' beacon reboot loop.
}
