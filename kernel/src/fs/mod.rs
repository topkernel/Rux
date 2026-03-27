//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Virtual File System (VFS)

pub mod file;
pub mod inode;
pub mod dentry;
pub mod pipe;
pub mod char_dev;
pub mod elf;
pub mod buffer;
pub mod bio;
pub mod vfs;
pub mod path;
pub mod permission;
pub mod superblock;
pub mod mount;
pub mod rootfs;
pub mod ext4;
pub mod stat;
pub mod procfs;
pub mod dev_t;
pub mod devfs;
pub mod fs_struct;
pub mod jbd2;

pub use file::{File, FileFlags, FileOps, FdTable, get_file_fd, close_file_fd};
pub use fs_struct::FsStruct;
pub use stat::Stat;
pub use pipe::create_pipe;
pub use char_dev::CharDev;
pub use rootfs::get_rootfs;
pub use vfs::{file_open, file_close, file_stat, file_fcntl, fcntl, file_mkdir, file_rmdir, file_unlink, file_link, stat_file_by_path};

pub fn read_file_from_rootfs(filename: &str) -> Option<alloc::vec::Vec<u8>> {
    use alloc::vec::Vec;
    use crate::println;

    // Simplified implementation: directly access global RootFS
    // Note: This is a temporary solution, should access through VFS interface in the future

    // Get RootFS instance
    let rootfs = unsafe { get_rootfs() };
    if rootfs.is_null() {
        return None;
    }

    // Lookup file
    let node = unsafe { (*rootfs).lookup(filename) };
    let node = match node {
        Some(n) => n,
        None => {
            return None;
        }
    };

    // Read file data
    if let Some(ref data) = node.data {
        let mut buffer = Vec::new();
        // Copy data to Vec
        unsafe {
            let len = data.len();
            if len > 0 {
                buffer.reserve(len);
                for i in 0..len {
                    buffer.push(*data.as_ptr().add(i));
                }
            }
        }
        Some(buffer)
    } else {
        None
    }
}

