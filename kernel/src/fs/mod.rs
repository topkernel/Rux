//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Virtual File System (VFS)

pub mod file;
pub mod unsafe_edges;
pub mod event_notify;
pub mod fdreap;
pub mod inode;
pub mod dentry;
// pub mod pipe; — MOVED to the rux-fs service crate (rung 3)
// pub mod char_dev; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub mod elf;
pub mod buffer;
pub mod bio;
pub mod vfs;
pub mod path;
pub mod permission;
pub mod superblock;
pub mod mount;
pub mod mount_table;
pub mod notify_hooks;
pub mod file_hooks;
pub mod leaf_glue;
pub mod leaf_hooks;
pub mod perf_ctr;
pub mod lock_hooks;
pub mod fs_hooks;
pub mod notify_events;
// pub mod rootfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
// pub mod tmpfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
// pub mod ext4; — MOVED to the rux-ext4 service crate (rung 5)
pub mod stat;
// pub mod procfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
// pub mod sysfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
// pub mod debugfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
// pub mod tracefs; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub mod dev_t;
// pub mod devfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub mod fs_struct;
// pub mod jbd2; — MOVED to the rux-ext4 service crate (rung 5)
pub mod page_cache;
pub mod readahead;
pub mod io_completion;
pub mod locks;
// pub mod inotify; — MOVED to the rux-fs service crate (rung 3)
// pub mod tty; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub mod vt;
// pub mod pty; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub mod xattr;
// pub mod memfd; — MOVED to the rux-fs service crate (rung 3)
pub mod pidfd;
// pub mod selinuxfs; — MOVED to the rux-fs service crate (rung 3)
// pub mod fifo; — MOVED to the rux-fs service crate (rung 3)
// pub mod cgroup; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub mod fsctx;
// The rung-5 ext4 seam: the framework's ext4/jbd2 hook table (the
// service crate registers at boot; the ext4/jbd2 module lines above
// stay until the rux-ext4 flip deletes them).
pub mod ext4_hooks;
pub mod ext4_edges;
pub mod journal_seam;

pub use file::{File, FileFlags, FileOps, FdTable, get_file_fd, close_file_fd, close_file_fd_report, set_cloexec_fd};
pub use fs_struct::FsStruct;
pub use stat::{copy_stat_to_user, user_stat_size, Stat};
// pub use pipe::create_pipe; — MOVED to the rux-fs service crate (rung 3)
// pub use char_dev::CharDev; — MOVED to the rux-fs service crate (fs-substrate tranche)
// pub use rootfs::get_rootfs; — MOVED to the rux-fs service crate (fs-substrate tranche)
pub use vfs::{file_open, file_close, file_stat, file_fcntl, fcntl, file_mkdir, file_rmdir, file_unlink, file_link, stat_file_by_path};

/// Read a file's bytes from the ramfs root (init.rs's config probe).
/// The body lives in the rux-fs crate (rootfs leaf); forwards through
/// the leaf interface.
pub fn read_file_from_rootfs(filename: &str) -> Option<alloc::vec::Vec<u8>> {
    leaf_glue::rootfs::read_file(filename)
}

