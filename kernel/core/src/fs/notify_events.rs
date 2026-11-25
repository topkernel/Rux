//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The inotify event constants (IN_*) — FRAMEWORK-side ABI constants
//! (rung-3 flip: the inotify leaf moved to fs; the constants
//! stay with the framework's notify_hooks so vfs.rs/file.rs keep their
//! `ino::IN_*` spellings and the crate re-imports them).

#![forbid(unsafe_code)]
pub mod inotify_events {
    /// File was accessed (read).
    pub const IN_ACCESS: u32 = 0x0000_0001;
    /// File was modified.
    pub const IN_MODIFY: u32 = 0x0000_0002;
    /// Metadata changed.
    pub const IN_ATTRIB: u32 = 0x0000_0004;
    /// Writable file was closed.
    pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
    /// Unwritable file was closed.
    pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
    /// File was opened.
    pub const IN_OPEN: u32 = 0x0000_0020;
    /// File moved out of watched dir.
    pub const IN_MOVED_FROM: u32 = 0x0000_0040;
    /// File moved into watched dir.
    pub const IN_MOVED_TO: u32 = 0x0000_0080;
    /// File/dir created in watched dir.
    pub const IN_CREATE: u32 = 0x0000_0100;
    /// File/dir deleted from watched dir.
    pub const IN_DELETE: u32 = 0x0000_0200;
    /// Watched file/dir was itself deleted.
    pub const IN_DELETE_SELF: u32 = 0x0000_0400;
    /// Watched file/dir was itself moved.
    pub const IN_MOVE_SELF: u32 = 0x0000_0800;

    /// Watch was removed (explicitly or automatically). Always delivered.
    pub const IN_IGNORED: u32 = 0x0000_8000;
    /// Event queue overflowed.
    pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
    /// Filesystem containing the watched inode was unmounted.
    pub const IN_UNMOUNT: u32 = 0x0000_2000;

    /// Subject of the event is a directory (modifier, not maskable).
    pub const IN_ISDIR: u32 = 0x4000_0000;
    /// Add to the existing mask instead of replacing it.
    pub const IN_MASK_ADD: u32 = 0x2000_0000;
    /// Don't report events for children after they were unlinked from the
    /// watched directory.
    pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
    /// Remove the watch after one event.
    pub const IN_ONESHOT: u32 = 0x8000_0000;

    /// Bits a user may ask to watch.
    pub const IN_ALL_EVENTS: u32 = IN_ACCESS
        | IN_MODIFY
        | IN_ATTRIB
        | IN_CLOSE_WRITE
        | IN_CLOSE_NOWRITE
        | IN_OPEN
        | IN_MOVED_FROM
        | IN_MOVED_TO
        | IN_CREATE
        | IN_DELETE
        | IN_DELETE_SELF
        | IN_MOVE_SELF;
}
