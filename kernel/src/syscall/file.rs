//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 文件系统相关系统调用
//!
//! 包含：open, openat, close, fstat, getdents64, mkdir, rmdir, unlink, readlinkat, lseek, chdir, getcwd, umask

use super::*;

/// sys_open - 打开文件 (遗留接口，包装到 openat)
///
/// # 参数
/// - args[0]: pathname - 文件路径
/// - args[1]: flags - 打开标志
/// - args[2]: mode - 创建模式
///
/// # 返回
/// 成功返回文件描述符，失败返回负错误码
pub fn sys_open(args: SyscallArgs) -> u64 {
    // open(pathname, flags, mode) 等价于 openat(AT_FDCWD, pathname, flags, mode)
    // AT_FDCWD = -100
    const AT_FDCWD: i64 = -100;

    let openat_args = [
        AT_FDCWD as u64,  // dirfd = AT_FDCWD
        args[0],          // pathname
        args[1],          // flags
        args[2],          // mode
        0, 0
    ];
    sys_openat(openat_args)
}

/// sys_openat - 打开文件
pub fn sys_openat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;
    let mode = args[3] as u32;

    const O_DIRECTORY: u32 = 0o00200000;
    const AT_FDCWD: i32 = -100;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 读取文件名
    let filename = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // 构造完整路径
    let full_path: alloc::borrow::Cow<str> = if filename_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(filename_str)
    } else if dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(cwd) {
                let mut path = alloc::string::String::with_capacity(cwd_str.len() + filename_str.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(filename_str);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(filename_str)
            }
        } else {
            alloc::borrow::Cow::Borrowed(filename_str)
        }
    } else {
        alloc::borrow::Cow::Borrowed(filename_str)
    };

    let final_path = full_path.as_ref();

    // 检查是否是打开目录
    if (flags & O_DIRECTORY) != 0 {
        match crate::fs::vfs::file_opendir(final_path, flags) {
            Ok(fd) => fd as u64,
            Err(e) => e as i64 as u64
        }
    } else {
        match crate::fs::file_open(final_path, flags, mode) {
            Ok(fd) => fd as u64,
            Err(e) => e as i64 as u64
        }
    }
}

/// sys_close - 关闭文件描述符
pub fn sys_close(args: SyscallArgs) -> u64 {
    use crate::fs::close_file_fd;
    let fd = args[0] as usize;

    unsafe {
        match close_file_fd(fd) {
            Ok(()) => 0,
            Err(e) => e as u32 as u64,
        }
    }
}

/// sys_fstat - 获取文件状态
pub fn sys_fstat(args: SyscallArgs) -> u64 {
    use crate::fs::{file_stat, Stat};

    let fd = args[0] as usize;
    let statbuf = args[1] as *mut Stat;

    // 检查 statbuf 指针有效性
    if statbuf.is_null() {
        return -errno::EFAULT as u64;
    }

    // 创建临时 stat 结构
    let mut stat = Stat::new();

    // 调用 VFS 层的 file_stat
    match file_stat(fd, &mut stat) {
        Ok(()) => {
            // 将 stat 结构复制到用户空间
            unsafe {
                *statbuf = stat;
            }
            0  // 成功
        }
        Err(errno) => {
            errno as u64  // 返回错误码
        }
    }
}

/// sys_getdents64 - 读取目录项
pub fn sys_getdents64(args: SyscallArgs) -> u64 {
    use crate::fs::vfs::file_getdents64;

    let fd = args[0] as usize;
    let dirp = args[1] as *mut u8;
    let count = args[2] as usize;

    // 检查指针有效性
    if dirp.is_null() {
        return -errno::EFAULT as u64;
    }

    if count == 0 {
        return -errno::EINVAL as u64;
    }

    // 创建临时缓冲区
    let mut buffer = alloc::vec::Vec::with_capacity(count);
    unsafe {
        buffer.set_len(count);
    }

    // 调用 VFS 层
    let result = file_getdents64(fd, &mut buffer, count);
    match result {
        Ok(bytes_read) => {
            // 将数据复制到用户空间
            unsafe {
                let sstatus: u64;
                let sum_bit: u64 = 0x40000;  // SUM 位 (bit 18)
                core::arch::asm!(
                    "csrr {sstatus}, sstatus",
                    "or {tmp}, {sstatus}, {sum}",
                    "csrw sstatus, {tmp}",
                    sstatus = out(reg) sstatus,
                    tmp = out(reg) _,
                    sum = in(reg) sum_bit,
                );

                // 复制数据到用户空间
                core::ptr::copy_nonoverlapping(buffer.as_ptr(), dirp, bytes_read);

                // 恢复 sstatus
                core::arch::asm!(
                    "csrw sstatus, {sstatus}",
                    sstatus = in(reg) sstatus,
                );
            }
            bytes_read as u64
        }
        Err(errno) => {
            errno as i64 as u64
        }
    }
}

/// sys_mkdir - 创建目录
pub fn sys_mkdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;
    let _mode = args[1] as u32;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    match crate::fs::vfs::file_mkdir(pathname_str, 0o755) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_rmdir - 删除空目录
pub fn sys_rmdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    match crate::fs::vfs::file_rmdir(pathname_str) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_unlink - 删除文件
pub fn sys_unlink(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    match crate::fs::vfs::file_unlink(pathname_str) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_readlinkat - 读取符号链接
pub fn sys_readlinkat(args: SyscallArgs) -> u64 {
    let _dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let buf = args[2] as *mut u8;
    let bufsize = args[3] as usize;

    if pathname_ptr.is_null() || buf.is_null() {
        return -errno::EINVAL as u64;
    }

    // 读取路径
    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // 目前只支持读取 /proc/self/exe (返回程序路径)
    if pathname_str == "/proc/self/exe" {
        // 获取当前进程的程序路径
        if let Some(current) = crate::sched::current() {
            let exe_path = unsafe { (*current).get_exe_path() };

            if exe_path.len() >= bufsize {
                return -errno::ENAMETOOLONG as u64;
            }

            // 复制到用户缓冲区
            unsafe {
                core::ptr::copy_nonoverlapping(exe_path.as_ptr(), buf, exe_path.len());
            }

            return exe_path.len() as u64;
        }
    }

    // 不支持其他符号链接
    -errno::ENOENT as u64
}

/// sys_lseek - 设置文件偏移
pub fn sys_lseek(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;

    let fd = args[0] as i32;
    let offset = args[1] as isize;  // 使用 isize 而不是 i64
    let whence = args[2] as i32;

    unsafe {
        match get_file_fd(fd as usize) {
            Some(file) => {
                let result = file.lseek(offset, whence);
                // lseek 返回 isize，负值表示错误
                if result < 0 {
                    result as i64 as u64
                } else {
                    result as u64
                }
            }
            None => -errno::EBADF as u64
        }
    }
}

/// sys_chdir - 改变当前目录
pub fn sys_chdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // 验证目录是否存在
    match crate::fs::vfs::file_opendir(pathname_str, 0) {
        Ok(_) => {
            if let Some(current) = crate::sched::current() {
                unsafe {
                    (*current).set_cwd(pathname);
                }
            }
            0
        }
        Err(e) => e as i64 as u64,
    }
}

/// sys_getcwd - 获取当前工作目录
pub fn sys_getcwd(args: SyscallArgs) -> u64 {
    let buf = args[0] as *mut u8;
    let size = args[1] as usize;

    if buf.is_null() {
        return -errno::EFAULT as u64;
    }

    if let Some(current) = crate::sched::current() {
        let cwd = unsafe { (*current).get_cwd() };
        let cwd_len = cwd.len();

        if cwd_len >= size {
            return -errno::ERANGE as u64;
        }

        unsafe {
            core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, cwd_len);
            *buf.add(cwd_len) = 0;
        }

        return buf as u64;
    }

    -errno::ENOENT as u64
}

/// sys_umask - 设置文件模式创建掩码
pub fn sys_umask(args: SyscallArgs) -> u64 {
    let _new_mask = args[0] & 0o777;  // 只使用低 9 位

    // 由于我们目前没有完整的文件权限支持，
    // 简化实现：返回之前的掩码（假设为 022）
    // TODO: 将 new_mask 存储到进程结构中
    0o022u64  // 默认掩码
}
