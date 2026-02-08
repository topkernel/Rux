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
pub mod vfs;
pub mod path;
pub mod superblock;
pub mod mount;
pub mod rootfs;

pub use file::{File, FileFlags, FileOps, FdTable, get_file_fd, get_file_fd_install, close_file_fd, REG_FILE_OPS, REG_RO_FILE_OPS};
pub use inode::{Inode, InodeMode, INodeOps, make_reg_inode, make_reg_inode_with_data, make_char_inode, make_dir_inode, make_fifo_inode};
pub use inode::{icache_lookup, icache_add, icache_remove, icache_stats};
pub use dentry::{Dentry, DentryState, make_root_dentry};
pub use dentry::{dcache_lookup, dcache_add, dcache_remove, dcache_stats};
pub use pipe::{Pipe, pipe_read, pipe_write, create_pipe};
pub use char_dev::{CharDev, uart_read, uart_write};
pub use elf::{Elf64Ehdr, Elf64Phdr, ElfLoader, ElfError};
pub use buffer::{Page, AddressSpace, FileBuffer, PAGE_SIZE};
pub use vfs::{
    file_open, init as vfs_init,
    // FileSystemType, SuperBlock, NameiData, namei_flags,
    // LinuxDirent64, d_type,
    // sys_mkdir, sys_rmdir, sys_getdents64,
    // FS_REGISTRY, LEGACY_FS_REGISTRY  // Temporarily disabled for debugging
};
pub use path::{Path, PathComponent, NameiData, namei_flags, PathComponents};
pub use superblock::{SuperBlock, SuperBlockFlags, FileSystemType, FsContext, register_filesystem, unregister_filesystem, get_fs_type, do_mount, do_umount};
pub use mount::{VfsMount, MntNamespace, MntFlags, MsFlags, get_init_namespace, create_namespace, clone_namespace, MountTreeIter};
pub use rootfs::{RootFSNode, RootFSType, RootFSSuperBlock, ROOTFS_FS_TYPE, init_rootfs, ROOTFS_MAGIC, get_rootfs};

/// 从 RootFS 读取文件内容（辅助函数）
///
/// 用于 execve 系统调用等需要读取完整文件的场景
///
/// # 参数
/// - `filename`: 文件名（绝对路径）
///
/// # 返回
/// 成功返回 Some(数据)，失败返回 None
pub fn read_file_from_rootfs(filename: &str) -> Option<alloc::vec::Vec<u8>> {
    use alloc::vec::Vec;
    use crate::println;

    // 简化实现：直接访问全局 RootFS
    // 注意：这是临时方案，未来应该通过 VFS 接口访问

    // 获取 RootFS 实例
    let rootfs = unsafe { get_rootfs() };
    if rootfs.is_null() {
        println!("read_file_from_rootfs: rootfs not initialized");
        return None;
    }

    // 查找文件
    let node = unsafe { (*rootfs).lookup(filename) };
    let node = match node {
        Some(n) => n,
        None => {
            println!("read_file_from_rootfs: file not found: {}", filename);
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
        println!("read_file_from_rootfs: file has no data");
        None
    }
}

