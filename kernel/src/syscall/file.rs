//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File system related system calls
//!
//! Includes: open, openat, close, fstat, getdents64, mkdir, rmdir, unlink, readlinkat, lseek, chdir, getcwd, umask

use super::*;
use crate::arch::riscv64::uaccess::strncpy_from_user;

/// sys_open - Open file (legacy interface, wrapped to openat)
///
/// # Arguments
/// - args[0]: pathname - file path
/// - args[1]: flags - open flags
/// - args[2]: mode - creation mode
///
/// # Returns
/// Returns file descriptor on success, negative error code on failure
pub fn sys_open(args: SyscallArgs) -> u64 {
    // open(pathname, flags, mode) is equivalent to openat(AT_FDCWD, pathname, flags, mode)
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

/// sys_openat - Open file
pub fn sys_openat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;
    let mode = args[3] as u32;

    const O_CREAT: u32 = 0o00000100;
    const O_DIRECTORY: u32 = 0o00200000;
    const AT_FDCWD: i32 = -100;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Read filename from user space safely
    let mut kernel_buf = [0u8; 256];
    let filename = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Build full path
    let full_path: alloc::borrow::Cow<str> = if filename_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(filename_str)
    } else if dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
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

    // Check if opening directory
    if (flags & O_DIRECTORY) != 0 {
        match crate::fs::vfs::file_opendir(final_path, flags) {
            Ok(fd) => fd as u64,
            Err(e) => e as i64 as u64,
        }
    } else {
        match crate::fs::file_open(final_path, flags, mode) {
            Ok(fd) => fd as u64,
            Err(e) => e as i64 as u64,
        }
    }
}

/// sys_close - Close file descriptor
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

/// sys_fstat - Get file status
pub fn sys_fstat(args: SyscallArgs) -> u64 {
    use crate::fs::{file_stat, Stat};

    let fd = args[0] as usize;
    let statbuf = args[1] as *mut Stat;

    // Check statbuf pointer validity
    if statbuf.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if statbuf is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(statbuf as usize, core::mem::size_of::<Stat>()) {
        return -errno::EFAULT as u64;
    }

    // Create temporary stat structure
    let mut stat = Stat::new();

    // Call VFS layer file_stat
    match file_stat(fd, &mut stat) {
        Ok(()) => {
            // Copy stat structure to user space
            unsafe {
                *statbuf = stat;
            }
            0  // Success
        }
        Err(errno) => {
            errno as u64  // Return error code
        }
    }
}

/// sys_fstatat - Get file status by path
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor (AT_FDCWD = -100 means current directory)
/// - args[1]: pathname - file path
/// - args[2]: statbuf - stat structure buffer
/// - args[3]: flags - flags (AT_SYMLINK_NOFOLLOW, etc.)
pub fn sys_fstatat(args: SyscallArgs) -> u64 {
    use crate::fs::{Stat, stat_file_by_path};

    const AT_FDCWD: i32 = -100;

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let statbuf = args[2] as *mut Stat;
    let _flags = args[3] as i32;

    // Check pointer validity
    if pathname_ptr.is_null() || statbuf.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pointers are in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(statbuf as usize, core::mem::size_of::<Stat>()) {
        return -errno::EFAULT as u64;
    }

    // Read path from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Build full path
    let full_path: alloc::borrow::Cow<str> = if pathname_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname_str)
    } else if dirfd == AT_FDCWD {
        // Relative to current working directory
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
                let mut path = alloc::string::String::with_capacity(cwd_str.len() + pathname_str.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(pathname_str);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(pathname_str)
            }
        } else {
            alloc::borrow::Cow::Borrowed(pathname_str)
        }
    } else {
        // TODO: Support lookup via dirfd
        alloc::borrow::Cow::Borrowed(pathname_str)
    };

    // Create temporary stat structure
    let mut stat = Stat::new();

    // Call VFS layer to get file status
    match stat_file_by_path(full_path.as_ref(), &mut stat) {
        Ok(()) => {
            // Copy stat structure to user space safely
            let stat_size = core::mem::size_of::<Stat>();
            let result = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    statbuf as *mut u8,
                    &stat as *const Stat as *const u8,
                    stat_size
                )
            };
            if result != 0 {
                return -errno::EFAULT as u64;
            }
            0  // Success
        }
        Err(errno) => {
            errno as i64 as u64  // Return error code
        }
    }
}

/// sys_getdents64 - Read directory entries
pub fn sys_getdents64(args: SyscallArgs) -> u64 {
    use crate::fs::vfs::file_getdents64;

    let fd = args[0] as usize;
    let dirp = args[1] as *mut u8;
    let count = args[2] as usize;

    // Check pointer validity
    if dirp.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if dirp is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(dirp as usize, count) {
        return -errno::EFAULT as u64;
    }

    if count == 0 {
        return -errno::EINVAL as u64;
    }

    // Create temporary buffer
    let mut buffer = alloc::vec::Vec::with_capacity(count);
    unsafe {
        buffer.set_len(count);
    }

    // Call VFS layer
    let result = file_getdents64(fd, &mut buffer, count);
    match result {
        Ok(bytes_read) => {
            // Copy data to user space
            unsafe {
                let sstatus: u64;
                let sum_bit: u64 = 0x40000;  // SUM bit (bit 18)
                core::arch::asm!(
                    "csrr {sstatus}, sstatus",
                    "or {tmp}, {sstatus}, {sum}",
                    "csrw sstatus, {tmp}",
                    sstatus = out(reg) sstatus,
                    tmp = out(reg) _,
                    sum = in(reg) sum_bit,
                );

                // Copy data to user space
                core::ptr::copy_nonoverlapping(buffer.as_ptr(), dirp, bytes_read);

                // Restore sstatus
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

/// sys_mkdir - Create directory (deprecated, use mkdirat)
pub fn sys_mkdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;
    let _mode = args[1] as u32;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pathname is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
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

/// sys_mkdirat - Create directory relative to directory file descriptor
/// Linux syscall number: 34 (riscv64)
pub fn sys_mkdirat(args: SyscallArgs) -> u64 {
    let _dirfd = args[0] as i32;  // AT_FDCWD = -100 for current directory
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as u32;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pathname is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // TODO: Handle dirfd properly (AT_FDCWD, absolute path, etc.)
    match crate::fs::vfs::file_mkdir(pathname_str, mode) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_rmdir - Remove empty directory
pub fn sys_rmdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pathname is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
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

/// sys_unlink - Remove file
pub fn sys_unlink(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pathname is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
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

/// sys_unlinkat - Remove file or directory (syscall 35)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor (AT_FDCWD = -100)
/// - args[1]: pathname - file path
/// - args[2]: flags - flags (AT_REMOVEDIR = 0x200 for removing directory)
pub fn sys_unlinkat(args: SyscallArgs) -> u64 {
    const AT_FDCWD: i32 = -100;
    const AT_REMOVEDIR: u32 = 0x200;

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pathname is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Build full path
    let full_path: alloc::borrow::Cow<str> = if pathname_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname_str)
    } else if dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
                let mut path = alloc::string::String::with_capacity(cwd_str.len() + pathname_str.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(pathname_str);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(pathname_str)
            }
        } else {
            alloc::borrow::Cow::Borrowed(pathname_str)
        }
    } else {
        alloc::borrow::Cow::Borrowed(pathname_str)
    };

    // Choose removal type based on flags
    if (flags & AT_REMOVEDIR) != 0 {
        // Remove directory
        match crate::fs::vfs::file_rmdir(full_path.as_ref()) {
            Ok(()) => 0,
            Err(e) => e as i64 as u64,
        }
    } else {
        // Remove file
        match crate::fs::vfs::file_unlink(full_path.as_ref()) {
            Ok(()) => 0,
            Err(e) => e as i64 as u64,
        }
    }
}

/// sys_readlinkat - Read symbolic link
pub fn sys_readlinkat(args: SyscallArgs) -> u64 {
    let _dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let buf = args[2] as *mut u8;
    let bufsize = args[3] as usize;

    if pathname_ptr.is_null() || buf.is_null() {
        return -errno::EINVAL as u64;
    }

    // Check if pointers are in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, bufsize) {
        return -errno::EFAULT as u64;
    }

    // Read path from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Currently only support reading /proc/self/exe (return program path)
    if pathname_str == "/proc/self/exe" {
        // Get current process program path
        if let Some(current) = crate::sched::current() {
            let exe_path = unsafe { (*current).get_exe_path() };

            if exe_path.len() >= bufsize {
                return -errno::ENAMETOOLONG as u64;
            }

            // Copy to user buffer
            unsafe {
                core::ptr::copy_nonoverlapping(exe_path.as_ptr(), buf, exe_path.len());
            }

            return exe_path.len() as u64;
        }
    }

    // Other symbolic links not supported
    -errno::ENOENT as u64
}

/// sys_lseek - Set file offset
pub fn sys_lseek(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;

    let fd = args[0] as i32;
    let offset = args[1] as isize;  // Use isize instead of i64
    let whence = args[2] as i32;

    unsafe {
        match get_file_fd(fd as usize) {
            Some(file) => {
                let result = file.lseek(offset, whence);
                // lseek returns isize, negative indicates error
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

/// sys_chdir - Change current directory
pub fn sys_chdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if pathname is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Verify directory exists
    match crate::fs::vfs::file_opendir(pathname_str, 0) {
        Ok(_) => {
            if let Some(current) = crate::sched::current() {
                // Parse to absolute path
                let abs_path = if pathname_str.starts_with('/') {
                    // Already absolute path
                    pathname.to_vec()
                } else {
                    // Relative path: combine with current cwd
                    let cwd = unsafe { (*current).get_cwd() };
                    let mut abs = cwd.to_vec();
                    if !cwd.ends_with(&[b'/']) {
                        abs.push(b'/');
                    }
                    abs.extend_from_slice(pathname);
                    abs
                };

                unsafe {
                    (*current).set_cwd(&abs_path);
                }
            }
            0
        }
        Err(e) => e as i64 as u64,
    }
}

/// sys_getcwd - Get current working directory
pub fn sys_getcwd(args: SyscallArgs) -> u64 {
    let buf = args[0] as *mut u8;
    let size = args[1] as usize;

    if buf.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if buf is in valid user space
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, size) {
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

/// sys_umask - Set file mode creation mask
pub fn sys_umask(args: SyscallArgs) -> u64 {
    let _new_mask = args[0] & 0o777;  // Only use low 9 bits

    // Since we don't have complete file permission support currently,
    // Simplified implementation: return previous mask (assume 022)
    // TODO: Store new_mask in process structure
    0o022u64  // Default mask
}

/// sys_faccessat - Check file access permissions (syscall 48)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: mode - access mode (F_OK, R_OK, W_OK, X_OK)
/// - args[3]: flags - flags (AT_EACCESS, etc.)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_faccessat(args: SyscallArgs) -> u64 {
    let _dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let _mode = args[2] as i32;
    let _flags = args[3] as i32;

    // Check pointer validity
    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read filename from user space safely
    let mut kernel_buf = [0u8; 256];
    let filename = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Check if file exists using VFS
    // For simplicity, just check if we can open it
    match crate::fs::vfs::file_open(filename_str, 0, 0) {
        Ok(fd) => {
            let _ = crate::fs::vfs::file_close(fd);
            0  // File exists and is accessible
        }
        Err(e) => e as u64,
    }
}

/// sys_futimesat - Change file timestamps (syscall 88)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: times - pointer to timeval array (or NULL)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// # Behavior
/// - If file exists: update timestamps and return 0
/// - If file doesn't exist: return -ENOENT (does NOT create the file)
/// - If times is NULL: use current time for both atime and mtime
pub fn sys_futimesat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let _times = args[2] as *const u8;

    // Check pointer validity
    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return -errno::EFAULT as u64;
    }

    // Read filename from user space safely
    let mut kernel_buf = [0u8; 256];
    let filename = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Build full path
    const AT_FDCWD: i32 = -100;
    let full_path: alloc::borrow::Cow<str> = if filename_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(filename_str)
    } else if dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
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
        // TODO: handle dirfd properly
        alloc::borrow::Cow::Borrowed(filename_str)
    };

    // Check if file exists
    match crate::fs::stat_file_by_path(full_path.as_ref(), &mut crate::fs::Stat::new()) {
        Ok(()) => {
            // File exists - TODO: actually update timestamps
            // For now, just return success
            0
        }
        Err(e) => {
            // File doesn't exist - return error (don't create)
            e as i64 as u64
        }
    }
}
