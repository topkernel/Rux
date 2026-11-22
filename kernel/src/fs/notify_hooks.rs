//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 NOTIFY SEAM: the framework-side notify hook table
//! (micro-core-design §4). The VFS framework calls inode/file
//! lifecycle notifications through THIS table; inotify registers its
//! implementations at boot (via mount_table::register_builtin_handlers,
//! the same seam point). When rux-fs-svc flips, inotify's registration
//! moves into the inotify crate — the framework never names it.
//!
//! The fns are thin trampolines today (inotify linked in); the crate
//! flip swaps the trampoline body for a registration slot.

pub(crate) mod ino_events {
    //! The inotify event constants (IN_*) — re-exported so vfs.rs/file.rs
    /// keep their `ino::IN_*` spelling unchanged (zero-diff rename).
    pub use crate::fs::inotify::inotify_events::*;
}

/// Parent+inode lifecycle event (fsnotify shape).
pub(crate) fn notify(
    parent: Option<&crate::fs::inode::Inode>,
    inode: Option<&crate::fs::inode::Inode>,
    mask: u32,
    name: Option<&[u8]>,
    cookie: u32,
) {
    crate::fs::inotify::hook_notify(parent, inode, mask, name, cookie);
}

/// File access/modify event (read/write paths).
pub(crate) fn notify_file(file: &crate::fs::file::File, mask: u32) {
    crate::fs::inotify::hook_notify_file(file, mask);
}

/// File close event.
pub(crate) fn notify_file_close(file: &crate::fs::file::File) {
    crate::fs::inotify::hook_notify_file_close(file);
}

/// Event-pairing cookie (rename/move).
pub(crate) fn alloc_cookie() -> u32 {
    crate::fs::inotify::hook_alloc_cookie()
}
