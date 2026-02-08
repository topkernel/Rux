//! 虚拟文件系统 (VFS) 核心功能

use crate::collection::SimpleArc;
use crate::errno;
use crate::fs::file::{File, get_file_fd, close_file_fd};

/// VFS 全局状态
struct VfsState {
    root_inode: Option<SimpleArc<()>>,  // 将来替换为实际的 root inode
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

    // 测试 SimpleArc 功能
    match SimpleArc::new(42i32) {
        Some(_) => {
            const MSG2: &[u8] = b"vfs: SimpleArc test passed\n";
            for &b in MSG2 {
                unsafe { putchar(b); }
            }
        }
        None => {
            const MSG3: &[u8] = b"vfs: SimpleArc test failed\n";
            for &b in MSG3 {
                unsafe { putchar(b); }
            }
        }
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
/// - filename: 文件名
/// - flags: O_RDONLY (0), O_WRONLY (1), O_RDWR (2)
/// - mode: 文件权限（创建时使用）
///
/// # 返回
/// 成功返回文件描述符，失败返回错误码
pub fn file_open(_filename: &str, _flags: u32, _mode: u32) -> Result<usize, i32> {
    // TODO: 实现真正的文件打开
    // 需要实现：
    // - 路径解析（使用 RootFS::lookup）
    // - 权限检查
    // - 文件查找
    // - 创建文件对象
    // - 分配文件描述符（使用 FdTable::alloc_fd）
    //
    // 注意：这需要与 RootFS 和文件描述符管理集成
    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
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
                // File::read 需要裸指针和显式类型
                let file_ref: &File = &*(file.as_ref() as *const File);
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
                // File::write 需要裸指针和显式类型
                let file_ref: &File = &*(file.as_ref() as *const File);
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

/// 文件控制 (Linux fcntl 接口)
///
/// 对应 Linux 的 sys_fcntl (fs/fcntl.c)
pub fn file_fcntl(_fd: usize, _cmd: usize, _arg: usize) -> Result<usize, i32> {
    // TODO: 实现真正的 fcntl 操作
    // 需要实现：
    // - F_DUPFD (dup/dup2)
    // - F_GETFD/F_SETFD (close-on-exec flag)
    // - F_GETFL/F_SETFL (文件状态标志)
    // - F_GETLK/F_SETLK (文件锁)
    Err(errno::Errno::FunctionNotImplemented.as_neg_i32())
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
