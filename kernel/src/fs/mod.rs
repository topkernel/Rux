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

pub use file::{File, FileFlags, FileOps, FdTable, get_file_fd, get_file_fd_install, close_file_fd, REG_FILE_OPS, REG_RO_FILE_OPS};
pub use inode::{Inode, InodeMode, INodeOps, make_reg_inode, make_reg_inode_with_data, make_char_inode, make_dir_inode, make_fifo_inode};
pub use dentry::{Dentry, DentryState, make_root_dentry};
pub use pipe::{Pipe, pipe_read, pipe_write};
pub use char_dev::{CharDev, uart_read, uart_write};
pub use elf::{Elf64Ehdr, Elf64Phdr, ElfLoader, ElfError};
pub use buffer::{Page, AddressSpace, FileBuffer, PAGE_SIZE};
pub use vfs::{
    file_open, init as vfs_init,
    FileSystemType, SuperBlock, NameiData, namei_flags,
    LinuxDirent64, d_type,
    sys_mkdir, sys_rmdir, sys_getdents64,
    FS_REGISTRY, LEGACY_FS_REGISTRY
};
