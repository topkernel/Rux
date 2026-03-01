//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IO 相关系统调用
//!
//! 包含：read, write, writev, dup, dup2, fcntl, ioctl, flock, pipe2

use super::*;

/// iovec 结构体 (用于 writev/readv)
#[repr(C)]
struct Iovec {
    iov_base: *const u8,
    iov_len: usize,
}

/// sys_read - 从文件描述符读取数据
///
/// # 参数
/// - args[0]: fd - 文件描述符
/// - args[1]: buf - 目标缓冲区指针
/// - args[2]: count - 读取字节数
///
/// # 返回
/// 成功返回读取的字节数，失败返回负错误码
pub fn sys_read(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;

    // 检查缓冲区地址是否在用户空间
    let buf_addr = buf as usize;
    if buf_addr < 0x10000 {
        return -errno::EFAULT as u64;
    }
    if buf_addr >= 0x8000_0000 {
        return -errno::EFAULT as u64;
    }

    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                let result = file.read(buf, count);
                if result < 0 {
                    result as u32 as u64
                } else {
                    result as u64
                }
            }
            None => -errno::EBADF as u64
        }
    }
}

/// sys_write - 向文件描述符写入数据
///
/// # 参数
/// - args[0]: fd - 文件描述符
/// - args[1]: buf - 源缓冲区指针
/// - args[2]: count - 写入字节数
///
/// # 返回
/// 成功返回写入的字节数，失败返回负错误码
pub fn sys_write(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;

    // 检查缓冲区地址是否在用户空间
    let buf_addr = buf as usize;
    if buf_addr < 0x10000 || buf_addr >= 0x8000_0000 {
        return -errno::EFAULT as u64;
    }

    unsafe {
        // 特殊处理 stdout (1) 和 stderr (2)
        if fd == 1 || fd == 2 {
            use crate::console::putchar;
            let slice = core::slice::from_raw_parts(buf, count);
            for &b in slice {
                if b == b'\n' {
                    putchar(b'\r');
                }
                putchar(b);
            }
            return count as u64;
        }

        match get_file_fd(fd) {
            Some(file) => {
                let result = file.write(buf, count);
                if result < 0 {
                    result as u32 as u64
                } else {
                    result as u64
                }
            }
            None => -errno::EBADF as u64
        }
    }
}

/// sys_writev - 向文件描述符写入多个缓冲区
///
/// # 参数
/// - args[0]: fd - 文件描述符
/// - args[1]: iov - 指向 iovec 结构数组的指针
/// - args[2]: iovcnt - iovec 数组的长度
///
/// # 返回
/// 成功返回写入的总字节数，失败返回负错误码
pub fn sys_writev(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;

    const USER_START: usize = 0x10000;
    const USER_END: usize = 0x7fff_f000;

    let iov_addr = iov_ptr as usize;
    if iov_addr < USER_START || iov_addr >= USER_END {
        return -errno::EFAULT as u64;
    }

    let mut total_written: isize = 0;
    let mut has_valid_iov = false;

    unsafe {
        for i in 0..iovcnt {
            let iov = &*iov_ptr.add(i);

            let base = iov.iov_base as usize;
            if iov.iov_len > 0 && base >= USER_START && base < USER_END {
                has_valid_iov = true;
                let write_args = [fd as u64, iov.iov_base as u64, iov.iov_len as u64, 0, 0, 0];
                let result = sys_write(write_args);
                let result_i64 = result as i64;
                if result_i64 < 0 {
                    if total_written == 0 {
                        return result;
                    }
                    break;
                }
                total_written += result as isize;
            } else if iov.iov_len > 0 {
                return -errno::EFAULT as u64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as u64;
    }

    total_written as u64
}

/// sys_dup - 复制文件描述符
pub fn sys_dup(args: SyscallArgs) -> u64 {
    let _oldfd = args[0] as usize;
    // 简化实现：返回 EMFILE
    -errno::EMFILE as i64 as u64
}

/// sys_dup2 - 复制文件描述符到指定编号
pub fn sys_dup2(args: SyscallArgs) -> u64 {
    let _oldfd = args[0] as usize;
    let _newfd = args[1] as usize;
    // 简化实现：返回 EMFILE
    -errno::EMFILE as i64 as u64
}

/// sys_fcntl - 文件控制
pub fn sys_fcntl(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let cmd = args[1] as usize;
    let arg = args[2] as usize;

    match crate::fs::vfs::file_fcntl(fd, cmd, arg) {
        Ok(result) => result as u64,
        Err(errno) => errno as u64,
    }
}

/// sys_ioctl - IO 控制
pub fn sys_ioctl(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let request = args[1] as u32;
    let arg = args[2] as usize;

    // 特殊处理 framebuffer 设备 (fd >= 1000 为设备文件)
    if fd >= 1000 {
        let result = crate::drivers::gpu::fbdev_ioctl(request, arg) as i64;
        return result as u64;
    }

    // TTY ioctl 命令
    match request {
        // TCGETS - 获取终端属性 (0x5401)
        0x5401 => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            // 填充默认的 termios 结构
            unsafe {
                let ptr = arg as *mut u32;
                // c_iflag: ICRNL | IXON
                *ptr.offset(0) = 0x0100 | 0x0400;
                // c_oflag: OPOST | ONLCR
                *ptr.offset(1) = 0x0001 | 0x0004;
                // c_cflag: B38400 | CS8 | CREAD | HUPCL
                *ptr.offset(2) = 0x000F | 0x0030 | 0x0080 | 0x0400;
                // c_lflag: ICANON | ECHO | ECHOE | ECHOK | ISIG
                *ptr.offset(3) = 0x0100 | 0x0008 | 0x0010 | 0x0020 | 0x0001;
                // c_line
                *ptr.offset(4) = 0;
                // c_cc[19] - control characters
                let cc_ptr = ptr.offset(5) as *mut u8;
                *cc_ptr.offset(0) = 3;   // VINTR = ^C
                *cc_ptr.offset(1) = 28;  // VQUIT = ^\
                *cc_ptr.offset(2) = 127; // VERASE = DEL
                *cc_ptr.offset(3) = 21;  // VKILL = ^U
                *cc_ptr.offset(4) = 4;   // VEOF = ^D
                *cc_ptr.offset(5) = 0;   // VTIME
                *cc_ptr.offset(6) = 1;   // VMIN
            }
            0
        }
        // TCSETS, TCSETSW, TCSETSF - 设置终端属性
        0x5402 | 0x5403 | 0x5404 => {
            0  // 简化实现：忽略设置
        }
        // TIOCGWINSZ - 获取窗口大小 (0x5413)
        0x5413 => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            unsafe {
                let ptr = arg as *mut u16;
                *ptr.offset(0) = 25;  // ws_row
                *ptr.offset(1) = 80;  // ws_col
                *ptr.offset(2) = 0;   // ws_xpixel
                *ptr.offset(3) = 0;   // ws_ypixel
            }
            0
        }
        // TIOCSWINSZ - 设置窗口大小 (0x5414)
        0x5414 => {
            0  // 忽略设置
        }
        // FIONREAD - 获取可读字节数 (0x541B)
        0x541B => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            unsafe {
                let ptr = arg as *mut i32;
                *ptr = 0;  // 简化：返回 0
            }
            0
        }
        // 其他 TTY 命令
        _ if (request & 0xFF00) == 0x5400 => {
            0  // 简化：返回成功
        }
        // 其他命令
        _ => {
            // 对于 stdin/stdout/stderr，返回成功
            if fd >= 0 && fd <= 2 {
                0
            } else {
                -errno::ENOTTY as u64
            }
        }
    }
}

/// sys_flock - 文件锁（简化实现）
pub fn sys_flock(_args: SyscallArgs) -> u64 {
    // 简化实现：总是返回成功
    0
}

/// sys_pipe2 - 创建带有标志的管道
pub fn sys_pipe2(args: SyscallArgs) -> u64 {
    let pipefd = args[0] as *mut i32;
    let _flags = args[1] as u32;

    // 检查指针
    if pipefd.is_null() {
        return -errno::EFAULT as u64;
    }

    let pipefd_addr = pipefd as usize;
    if pipefd_addr < 0x10000 || pipefd_addr >= 0x8000_0000 {
        return -errno::EFAULT as u64;
    }

    // 创建管道
    let (read_file, write_file) = crate::fs::pipe::create_pipe();

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EMFILE as u64,
    };

    // 分配文件描述符
    let read_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    let write_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    // 安装文件到 fdtable
    if fdtable.install_fd(read_fd, read_file).is_err() {
        return -errno::EMFILE as u64;
    }
    if fdtable.install_fd(write_fd, write_file).is_err() {
        return -errno::EMFILE as u64;
    }

    unsafe {
        *pipefd = read_fd as i32;
        *pipefd.offset(1) = write_fd as i32;
    }
    0
}
