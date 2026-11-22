//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 FLIP SEAM: the fs-service hook table (the NetHooks pattern).
//! The framework's per-File op hooks (pipe/fifo/memfd), the lifecycle
//! notify fanout, and the ops-identity probes dispatch through THIS
//! nullable table — no-op until the rux-fs crate registers at boot
//! (rux-bin calls its register_all before kmain, the composition
//! contract). The framework never names a service leaf.

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
