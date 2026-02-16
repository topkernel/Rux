//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 虚拟文件系统 (VFS)
//!
//! 完全遵循 Linux 内核的 VFS 设计：
//! - `file`: 文件对象和文件描述符管理 (fs/file.c)
//! - `inode`: 索引节点管理 (fs/inode.c)
//! - `dentry`: 目录项管理 (fs/dcache.c)
//! - `pipe`: 管道文件系统 (fs/pipe.c)
//! - `elf`: ELF 加载器 (fs/binfmt_elf.c)

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
pub mod superblock;
pub mod mount;
pub mod rootfs;
pub mod ext4;
pub mod stat;
pub mod procfs;

pub use file::{File, FileFlags, FileOps, FdTable, get_file_fd, close_file_fd};
pub use stat::Stat;
pub use pipe::create_pipe;
pub use char_dev::CharDev;
pub use rootfs::get_rootfs;
pub use vfs::{file_open, file_close, file_stat, file_fcntl, fcntl, file_mkdir, file_rmdir, file_unlink, file_link};

pub fn read_file_from_rootfs(filename: &str) -> Option<alloc::vec::Vec<u8>> {
    use alloc::vec::Vec;
    use crate::println;

    // 简化实现：直接访问全局 RootFS
    // 注意：这是临时方案，未来应该通过 VFS 接口访问

    // 获取 RootFS 实例
    let rootfs = unsafe { get_rootfs() };
    if rootfs.is_null() {
        return None;
    }

    // 查找文件
    let node = unsafe { (*rootfs).lookup(filename) };
    let node = match node {
        Some(n) => n,
        None => {
            return None;
        }
    };

    // 读取文件数据
    if let Some(ref data) = node.data {
        let mut buffer = Vec::new();
        // 复制数据到 Vec
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

