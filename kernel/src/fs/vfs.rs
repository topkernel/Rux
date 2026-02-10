//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 虚拟文件系统 (VFS) 核心功能

use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::errno;
use crate::fs::file::{File, FileFlags, FileOps, get_file_fd, close_file_fd, get_file_fd_install};
use crate::fs::rootfs::{RootFSNode, get_rootfs};
use crate::fs::Stat;
use crate::println;

/// VFS 全局状态
struct VfsState {
    root_inode: Option<Arc<()>>,  // 将来替换为实际的 root inode
    initialized: bool,
}

static mut VFS_STATE: VfsState = VfsState {
    root_inode: None,
    initialized: false,
};

/// 初始化 VFS
pub fn init() {
    use crate::console::putchar;
    const MSG1: &[u8] = b"vfs: Initializing Virtual File System...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // 测试 Arc 功能
    let _test_arc = Arc::new(42i32);
    const MSG2: &[u8] = b"vfs: Arc test passed\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    unsafe {
        VFS_STATE.initialized = true;
    }

    const MSG4: &[u8] = b"vfs: VFS layer initialized [OK]\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }
}

/// 打开文件 (Linux sys_openat 接口)
///
/// 对应 Linux 的 do_sys_openat (fs/open.c)
///
/// # 参数
/// - filename: 文件名（必须是绝对路径）
/// - flags: O_RDONLY (0), O_WRONLY (1), O_RDWR (2), O_CREAT (0o100), O_EXCL (0o200), O_TRUNC (0o1000)
/// - mode: 文件权限（创建时使用，当前未实现）
///
/// # 返回
/// 成功返回文件描述符，失败返回错误码
///
/// # 支持的标志
/// - O_RDONLY/O_WRONLY/O_RDWR: 读写模式
/// - O_CREAT: 文件不存在时创建
/// - O_EXCL: 与 O_CREAT 一起使用，文件已存在时返回错误
/// - O_TRUNC: 截断文件为空
pub fn file_open(filename: &str, flags: u32, _mode: u32) -> Result<usize, i32> {
    unsafe {
        // 1. 获取 RootFS 超级块
        let sb_ptr = get_rootfs();
        if sb_ptr.is_null() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        let sb = &*sb_ptr;

        // 提取标志位
        let o_creat = (flags & FileFlags::O_CREAT) != 0;
        let o_excl = (flags & FileFlags::O_EXCL) != 0;
        let o_trunc = (flags & FileFlags::O_TRUNC) != 0;

        // 2. 查找文件节点
        let (node, _was_created) = match sb.lookup(filename) {
            Some(n) => {
                // 文件已存在
                if o_excl && o_creat {
                    // O_EXCL + O_CREAT：文件已存在，返回错误
                    return Err(errno::Errno::FileExists.as_neg_i32());
                }
                (n, false)
            }
            None => {
                // 文件不存在
                if o_creat {
                    // 创建新文件
                    if let Err(e) = sb.create_file(filename, Vec::new()) {
                        return Err(e);
                    }
                    // 重新查找刚创建的文件
                    match sb.lookup(filename) {
                        Some(n) => (n, true),
                        None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
                    }
                } else {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        };

        // 4. 检查是否是目录（目录不能打开为文件）
        if node.is_dir() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        // 5. 处理 O_TRUNC：截断文件
        if o_trunc {
            // TODO: 实现文件截断功能
            // 需要修改 RootFSNode 的 data 为空 Vec
            // 由于 RootFSNode 使用不可变引用，暂时无法实现
            // 可以在未来添加内部可变性支持
        }

        // 6. 创建 File 对象
        let file_flags = FileFlags::new(flags);
        let file = Arc::new(File::new(file_flags));

        // 7. 设置文件操作
        file.set_ops(&ROOTFS_FILE_OPS);

        // 8. 将 RootFSNode 指针存储为私有数据
        // 注意：这里使用裸指针，生命周期由 RootFS 管理
        let node_ptr = node.as_ref() as *const RootFSNode as *mut u8;
        file.set_private_data(node_ptr);

        // 9. 分配文件描述符
        match get_file_fd_install(file) {
            Some(fd) => Ok(fd),
            None => Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
        }
    }
}

/// 关闭文件 (Linux sys_close 接口)
///
/// 对应 Linux 的 sys_close (fs/open.c)
///
/// # 参数
/// - fd: 文件描述符
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
pub fn file_close(fd: usize) -> Result<(), i32> {
    unsafe {
        // 使用 close_file_fd 关闭文件描述符
        // 这会：
        // 1. 检查文件描述符有效性
        // 2. 调用文件的 close 操作
        // 3. 释放文件描述符
        close_file_fd(fd)
    }
}

/// 读取文件 (Linux sys_read 接口)
///
/// 对应 Linux 的 sys_read (fs/read_write.c)
///
/// # 参数
/// - fd: 文件描述符
/// - buf: 缓冲区
/// - count: 要读取的字节数
///
/// # 返回
/// 成功返回读取的字节数，失败返回错误码
pub fn file_read(fd: usize, buf: &mut [u8], count: usize) -> Result<usize, i32> {
    unsafe {
        // 获取文件对象
        match get_file_fd(fd) {
            Some(file) => {
                // Arc 自动 Deref 到 File
                let file_ref: &File = &*file;
                let buf_ptr = buf.as_mut_ptr();
                let read_count = count.min(buf.len());

                // 调用文件的 read 操作
                let result = file_ref.read(buf_ptr, read_count);
                if result < 0 {
                    Err(result as i32)
                } else {
                    Ok(result as usize)
                }
            }
            None => {
                Err(errno::Errno::BadFileNumber.as_neg_i32())
            }
        }
    }
}

/// 写入文件 (Linux sys_write 接口)
///
/// 对应 Linux 的 sys_write (fs/read_write.c)
///
/// # 参数
/// - fd: 文件描述符
/// - buf: 缓冲区
/// - count: 要写入的字节数
///
/// # 返回
/// 成功返回写入的字节数，失败返回错误码
pub fn file_write(fd: usize, buf: &[u8], count: usize) -> Result<usize, i32> {
    unsafe {
        // 获取文件对象
        match get_file_fd(fd) {
            Some(file) => {
                // Arc 自动 Deref 到 File
                let file_ref: &File = &*file;
                let buf_ptr = buf.as_ptr();
                let write_count = count.min(buf.len());

                // 调用文件的 write 操作
                let result = file_ref.write(buf_ptr, write_count);
                if result < 0 {
                    Err(result as i32)
                } else {
                    Ok(result as usize)
                }
            }
            None => {
                Err(errno::Errno::BadFileNumber.as_neg_i32())
            }
        }
    }
}

/// 获取文件状态信息 (Linux sys_fstat 接口)
///
/// 对应 Linux 的 sys_fstat (fs/stat.c)
///
/// # 参数
/// - fd: 文件描述符
/// - stat: 输出参数，存储文件状态信息
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
///
/// # 功能
/// 获取打开文件的状态信息，包括：
/// - 文件类型（常规文件、目录、字符设备等）
/// - 文件大小
/// - 权限
/// - inode 号
/// - 时间戳等
pub fn file_stat(fd: usize, stat: &mut Stat) -> Result<(), i32> {
    unsafe {
        // 获取文件对象
        match get_file_fd(fd) {
            Some(file) => {
                // Arc 自动 Deref 到 File
                let file_ref: &File = &*file;

                // 从 private_data 获取 RootFSNode 指针
                let data_opt = &*file_ref.private_data.get();
                if let Some(node_ptr) = *data_opt {
                    let node = &*(node_ptr as *const RootFSNode);

                    // 填充 stat 结构
                    stat.st_dev = 0;  // RootFS 没有设备概念
                    stat.st_ino = node.ino;
                    stat.st_nlink = 1;  // 默认硬链接数为 1
                    stat.st_uid = 0;   // root 用户
                    stat.st_gid = 0;   // root 组
                    stat.st_rdev = 0;

                    // 文件大小
                    if let Some(ref data) = node.data {
                        stat.st_size = data.len() as i64;
                        // 计算块数 (512字节块)
                        stat.st_blocks = (data.len() as u64 + 511) / 512;
                    } else {
                        stat.st_size = 0;
                        stat.st_blocks = 0;
                    }

                    stat.st_blksize = 4096;  // 4KB 块大小

                    // 文件类型和权限
                    if node.is_dir() {
                        stat.set_directory();
                        // 目录权限: rwxr-xr-x (0o755)
                        stat.set_mode(0o755);
                    } else {
                        stat.set_regular_file();
                        // 文件权限: rw-r--r-- (0o644)
                        stat.set_mode(0o644);
                    }

                    // 时间戳 (当前使用 0，未来可以实现真实时间戳)
                    stat.st_atime = 0;
                    stat.st_atime_nsec = 0;
                    stat.st_mtime = 0;
                    stat.st_mtime_nsec = 0;
                    stat.st_ctime = 0;
                    stat.st_ctime_nsec = 0;

                    Ok(())
                } else {
                    // 没有 private_data，可能是管道或字符设备
                    // TODO: 处理其他文件类型
                    Err(errno::Errno::BadFileNumber.as_neg_i32())
                }
            }
            None => {
                Err(errno::Errno::BadFileNumber.as_neg_i32())
            }
        }
    }
}

/// fcntl 命令常量
///
/// 对应 Linux 的 fcntl 命令 (fcntl.h)
pub mod fcntl {
    /// 复制文件描述符
    pub const F_DUPFD: usize = 0;

    /// 获取 close-on-exec 标志
    pub const F_GETFD: usize = 1;

    /// 设置 close-on-exec 标志
    pub const F_SETFD: usize = 2;

    /// 获取文件状态标志
    pub const F_GETFL: usize = 3;

    /// 设置文件状态标志
    pub const F_SETFL: usize = 4;

    /// FD_CLOEXEC 标志值
    pub const FD_CLOEXEC: usize = 1;
}

/// 文件控制 (Linux fcntl 接口)
///
/// 对应 Linux 的 sys_fcntl (fs/fcntl.c)
///
/// # 参数
/// - fd: 文件描述符
/// - cmd: fcntl 命令
/// - arg: 命令参数
///
/// # 返回
/// 成功返回命令相关的值，失败返回错误码
///
/// # 支持的命令
/// - F_DUPFD (0) - 复制文件描述符，arg 指定最小 fd
/// - F_GETFD (1) - 获取 close-on-exec 标志
/// - F_SETFD (2) - 设置 close-on-exec 标志
/// - F_GETFL (3) - 获取文件状态标志
/// - F_SETFL (4) - 设置文件状态标志
pub fn file_fcntl(fd: usize, cmd: usize, arg: usize) -> Result<usize, i32> {
    use crate::fs::file::{get_file_fd, get_file_fd_install};

    unsafe {
        match cmd {
            // F_DUPFD: 复制文件描述符
            fcntl::F_DUPFD => {
                // 获取原文件
                let old_file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // 分配新的文件描述符（>= arg）
                let min_fd = arg;
                let new_fd = match get_file_fd_install(old_file) {
                    Some(fd) if fd >= min_fd => fd,
                    Some(_fd) => {
                        // TODO: 实现 fd 重定向以支持 F_DUPFD 的 arg 参数
                        // 当前简化实现：直接返回分配的 fd
                        return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
                    }
                    None => return Err(errno::Errno::TooManyOpenFiles.as_neg_i32()),
                };

                Ok(new_fd)
            }

            // F_GETFD: 获取 close-on-exec 标志
            fcntl::F_GETFD => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                let cloexec = file.get_cloexec();
                Ok(if cloexec { fcntl::FD_CLOEXEC } else { 0 })
            }

            // F_SETFD: 设置 close-on-exec 标志
            fcntl::F_SETFD => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // arg 的 bit 0 表示 FD_CLOEXEC
                let cloexec = (arg & fcntl::FD_CLOEXEC) != 0;
                file.set_cloexec(cloexec);

                Ok(0)  // 成功返回 0
            }

            // F_GETFL: 获取文件状态标志
            fcntl::F_GETFL => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // 返回文件状态标志（访问模式）
                Ok(file.flags.bits() as usize)
            }

            // F_SETFL: 设置文件状态标志
            fcntl::F_SETFL => {
                let file = match get_file_fd(fd) {
                    Some(f) => f,
                    None => return Err(errno::Errno::BadFileNumber.as_neg_i32()),
                };

                // 只允许设置部分标志（O_NONBLOCK, O_APPEND, O_ASYNC 等）
                // 不允许改变访问模式（O_RDONLY, O_WRONLY, O_RDWR）
                const SETFL_FLAGS: u32 = crate::fs::file::FileFlags::O_APPEND
                    | crate::fs::file::FileFlags::O_NONBLOCK
                    | crate::fs::file::FileFlags::O_SYNC
                    | crate::fs::file::FileFlags::O_DSYNC;

                // 保留访问模式
                let accmode = file.flags.bits() & crate::fs::file::FileFlags::O_ACCMODE;
                // 设置新标志
                let new_flags = accmode | (arg as u32 & SETFL_FLAGS);

                // 使用 unsafe 设置标志（FileFlags 不是 Mutex，需要直接赋值）
                unsafe {
                    let flags_ptr = &file.flags as *const FileFlags as *mut FileFlags;
                    (*flags_ptr).set_bits(new_flags);
                }

                Ok(0)  // 成功返回 0
            }

            // 不支持的命令
            _ => {
                println!("file_fcntl: unsupported cmd {}", cmd);
                Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
            }
        }
    }
}

/// I/O 多路复用 (Linux ppoll 接口)
///
/// 对应 Linux 的 sys_ppoll (fs/select.c)
pub fn io_poll(_fds: *mut u8, _nfds: usize, _timeout_ms: i32) -> Result<usize, i32> {
    // TODO: 实现 I/O 多路复用
    // 需要实现：
    // - 等待文件描述符就绪
    // - 支持超时
    // - 返回就绪的文件描述符数量
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
}

/// 创建目录 (Linux sys_mkdir 接口)
///
/// 对应 Linux 的 sys_mkdirat (fs/namei.c)
///
/// # 参数
/// - pathname: 目录路径
/// - mode: 目录权限
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
///
/// # Linux 系统调用号
/// - RISC-V: 77 (mkdirat), 但我们实现简化的 mkdir
pub fn file_mkdir(pathname: &str, mode: u32) -> Result<(), i32> {
    unsafe {
        // 获取 RootFS 超级块
        let sb_ptr = get_rootfs();
        if sb_ptr.is_null() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        let sb = &*sb_ptr;

        // 调用 RootFS 创建目录
        sb.create_dir(pathname, mode)
    }
}

/// 删除目录 (Linux sys_rmdir 接口)
///
/// 对应 Linux 的 sys_rmdir (fs/namei.c)
///
/// # 参数
/// - pathname: 目录路径
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
///
/// # Linux 系统调用号
/// - RISC-V: 79
pub fn file_rmdir(pathname: &str) -> Result<(), i32> {
    unsafe {
        // 获取 RootFS 超级块
        let sb_ptr = get_rootfs();
        if sb_ptr.is_null() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        let sb = &*sb_ptr;

        // 调用 RootFS 删除目录
        sb.rmdir(pathname)
    }
}

/// 删除文件 (Linux sys_unlink 接口)
///
/// 对应 Linux 的 sys_unlinkat (fs/namei.c)
///
/// # 参数
/// - pathname: 文件路径
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
///
/// # Linux 系统调用号
/// - RISC-V: 74 (unlinkat), 但我们实现简化的 unlink
pub fn file_unlink(pathname: &str) -> Result<(), i32> {
    unsafe {
        // 获取 RootFS 超级块
        let sb_ptr = get_rootfs();
        if sb_ptr.is_null() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        let sb = &*sb_ptr;

        // 调用 RootFS 删除文件
        sb.unlink(pathname)
    }
}

// ============================================================================
// RootFS 文件操作 (对应 Linux 的 regular file operations)
// ============================================================================

/// RootFS 文件读取操作
///
/// 对应 Linux 的 generic_file_read (mm/filemap.c)
fn rootfs_file_read(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        // 从 private_data 获取 RootFSNode 指针
        let data_opt = &*file.private_data.get();
        if let Some(node_ptr) = *data_opt {
            let node = &*(node_ptr as *const RootFSNode);

            // 获取当前文件位置
            let offset = file.get_pos() as usize;

            // 检查是否有数据
            if let Some(ref data) = node.data {
                let available: usize = data.len().saturating_sub(offset);
                let to_read = buf.len().min(available);

                if to_read > 0 {
                    // 复制数据到缓冲区
                    buf[..to_read].copy_from_slice(&data[offset..offset + to_read]);

                    // 更新文件位置
                    file.set_pos((offset + to_read) as u64);

                    to_read as isize
                } else {
                    0  // EOF
                }
            } else {
                0  // 目录或无数据
            }
        } else {
            -9  // EBADF
        }
    }
}

/// RootFS 文件写入操作
///
/// 对应 Linux 的 generic_file_write (mm/filemap.c)
fn rootfs_file_write(file: &File, _buf: &[u8]) -> isize {
    unsafe {
        // 从 private_data 获取 RootFSNode 指针
        let data_opt = &*file.private_data.get();
        if data_opt.is_some() {
            // 注意：我们需要可变引用来修改数据
            // 但这里是不可变操作，所以暂时返回错误
            // TODO: 需要 RootFSNode 支持内部可变性
            -9  // EBADF - RootFS 暂时只读
        } else {
            -9  // EBADF
        }
    }
}

/// RootFS 文件定位操作
///
/// 对应 Linux 的 generic_file_llseek (fs/read_write.c)
fn rootfs_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    // 获取当前文件位置
    let current_pos = file.get_pos() as isize;

    // 获取文件大小
    let file_size = unsafe {
        let data_opt = &*file.private_data.get();
        if let Some(node_ptr) = *data_opt {
            let node = &*(node_ptr as *const RootFSNode);
            node.data.as_ref().map_or(0isize, |d: &Vec<u8>| d.len() as isize)
        } else {
            return -9;  // EBADF
        }
    };

    let new_pos = match whence {
        0 => offset,              // SEEK_SET
        1 => current_pos + offset, // SEEK_CUR
        2 => file_size + offset,   // SEEK_END
        _ => return -22,           // EINVAL - 无效的 whence
    };

    if new_pos < 0 {
        return -22;  // EINVAL - 负的位置无效
    }

    file.set_pos(new_pos as u64);
    new_pos
}

/// RootFS 文件关闭操作
fn rootfs_file_close(_file: &File) -> i32 {
    // RootFS 节点由 RootFS 管理，这里不需要特殊处理
    0
}

/// RootFS 文件操作表
///
/// 对应 Linux 的 generic_file_ro_fops (只读文件)
static ROOTFS_FILE_OPS: FileOps = FileOps {
    read: Some(rootfs_file_read),
    write: Some(rootfs_file_write),  // 暂时返回 EBADF
    lseek: Some(rootfs_file_lseek),
    close: Some(rootfs_file_close),
};
