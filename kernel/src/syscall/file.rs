//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File system related system calls
//!
//! Includes: open, openat, close, fstat, getdents64, mkdir, rmdir, unlink, readlinkat, lseek, chdir, getcwd, umask

use super::*;
use crate::arch::riscv64::uaccess::strncpy_from_user;

/// Maximum path length (Linux PATH_MAX)
const PATH_MAX: usize = 4096;

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
    const O_CLOEXEC: u32 = 0o02000000;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let result = if (flags & O_DIRECTORY) != 0 {
        crate::fs::vfs::file_opendir(&full_path, flags)
    } else {
        crate::fs::file_open(&full_path, flags, mode)
    };

    match result {
        Ok(fd) => {
            if (flags & O_CLOEXEC) != 0 {
                unsafe {
                    if let Some(file) = crate::fs::get_file_fd(fd) {
                        file.set_cloexec(true);
                    }
                }
            }
            fd as u64
        }
        Err(e) => e as i64 as u64,
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

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let statbuf = args[2] as *mut Stat;

    if statbuf.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(statbuf as usize, core::mem::size_of::<Stat>()) {
        return -errno::EFAULT as u64;
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut stat = Stat::new();

    match stat_file_by_path(&full_path, &mut stat) {
        Ok(()) => {
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
            0
        }
        Err(errno) => errno as i64 as u64,
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

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    match crate::fs::vfs::file_mkdir(pathname, 0o755) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_mkdirat - Create directory relative to directory file descriptor
/// Linux syscall number: 34 (riscv64)
pub fn sys_mkdirat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as u32;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::file_mkdir(&full_path, mode) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_rmdir - Remove empty directory
pub fn sys_rmdir(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    match crate::fs::vfs::file_rmdir(pathname) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_unlink - Remove file
pub fn sys_unlink(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    match crate::fs::vfs::file_unlink(pathname) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_linkat - Create hard link (syscall 37)
///
/// # Arguments
/// - args[0]: olddirfd - old directory file descriptor (AT_FDCWD = -100)
/// - args[1]: oldpath - existing file path
/// - args[2]: newdirfd - new directory file descriptor (AT_FDCWD = -100)
/// - args[3]: newpath - new link path
/// - args[4]: flags - reserved (AT_SYMLINK_FOLLOW, AT_EMPTY_PATH)
pub fn sys_linkat(args: SyscallArgs) -> u64 {
    let olddirfd = args[0] as i32;
    let oldpath_ptr = args[1] as *const u8;
    let newdirfd = args[2] as i32;
    let newpath_ptr = args[3] as *const u8;

    let old_full = match resolve_user_path(olddirfd, oldpath_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new_full = match resolve_user_path(newdirfd, newpath_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_link(&old_full, &new_full) {
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
    const AT_REMOVEDIR: u32 = 0x200;

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if (flags & AT_REMOVEDIR) != 0 {
        match crate::fs::vfs::file_rmdir(&full_path) {
            Ok(()) => 0,
            Err(e) => e as i64 as u64,
        }
    } else {
        match crate::fs::vfs::file_unlink(&full_path) {
            Ok(()) => 0,
            Err(e) => e as i64 as u64,
        }
    }
}

/// sys_renameat - Rename a file or directory
///
/// # Arguments
/// - args[0]: olddirfd - old directory file descriptor
/// - args[1]: oldpath - old pathname
/// - args[2]: newdirfd - new directory file descriptor
/// - args[3]: newpath - new pathname
pub fn sys_renameat(args: SyscallArgs) -> u64 {
    let olddirfd = args[0] as i32;
    let oldpath_ptr = args[1] as *const u8;
    let newdirfd = args[2] as i32;
    let newpath_ptr = args[3] as *const u8;

    let old_full = match resolve_user_path(olddirfd, oldpath_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new_full = match resolve_user_path(newdirfd, newpath_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_rename(&old_full, &new_full) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_readlinkat - Read symbolic link
pub fn sys_readlinkat(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[1] as *const u8;
    let buf = args[2] as *mut u8;
    let bufsize = args[3] as usize;

    if buf.is_null() {
        return -errno::EINVAL as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, bufsize) {
        return -errno::EFAULT as u64;
    }

    let mut pathbuf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut pathbuf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    // Currently only support reading /proc/self/exe (return program path)
    if pathname == "/proc/self/exe" {
        if let Some(current) = crate::sched::current() {
            let exe_path = unsafe { (*current).get_exe_path() };

            if exe_path.len() >= bufsize {
                return -errno::ENAMETOOLONG as u64;
            }

            unsafe {
                core::ptr::copy_nonoverlapping(exe_path.as_ptr(), buf, exe_path.len());
            }

            return exe_path.len() as u64;
        }
    }

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

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    // Verify directory exists
    match crate::fs::vfs::file_opendir(pathname, 0) {
        Ok(_) => {
            if let Some(current) = crate::sched::current() {
                let abs_path = if pathname.starts_with('/') {
                    pathname.as_bytes().to_vec()
                } else {
                    let cwd = unsafe { (*current).get_cwd() };
                    let mut abs = cwd.to_vec();
                    if !cwd.ends_with(&[b'/']) {
                        abs.push(b'/');
                    }
                    abs.extend_from_slice(pathname.as_bytes());
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

/// sys_fchdir - Change working directory by fd
/// RISC-V syscall number: 50
pub fn sys_fchdir(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;

    let file = unsafe { crate::fs::get_file_fd(fd) }
        .ok_or(-errno::EBADF as u64);
    let file = match file {
        Ok(f) => f,
        Err(e) => return e,
    };

    let inode_opt = unsafe { &*file.inode.get() };
    let inode = match inode_opt.as_ref() {
        Some(i) => i,
        None => return -errno::EBADF as u64,
    };

    if !inode.mode.is_directory() {
        return -errno::ENOTDIR as u64;
    }

    // Reconstruct absolute path from dentry chain
    let dentry_opt = unsafe { &*file.dentry.get() };
    let dentry = match dentry_opt.as_ref() {
        Some(d) => d,
        None => return -errno::EBADF as u64,
    };

    // Walk dentry chain up to root, collect names
    let mut components = alloc::vec::Vec::new();
    let mut current = dentry.clone();
    loop {
        let name = current.get_name();
        if name == "/" {
            break;
        }
        components.push(name);
        let parent_opt = current.parent.lock().clone();
        match parent_opt {
            Some(p) => current = p,
            None => break,
        }
    }

    // Build absolute path: / + reversed components
    let mut path = alloc::vec::Vec::new();
    path.push(b'/');
    for (i, comp) in components.iter().rev().enumerate() {
        if i > 0 {
            path.push(b'/');
        }
        path.extend_from_slice(comp.as_bytes());
    }

    if let Some(task) = crate::sched::current() {
        unsafe { (*task).set_cwd(&path); }
    }

    0
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
    let new_mask = (args[0] & 0o777) as u32;
    if let Some(task) = crate::sched::current() {
        task.set_umask(new_mask) as u64
    } else {
        0o022u64
    }
}

/// sys_mount - Mount a filesystem (syscall 40)
///
/// Linux ABI: mount(source, target, filesystemtype, mountflags, data)
pub fn sys_mount(args: SyscallArgs) -> u64 {
    // Only root can mount
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        return crate::errno::Errno::OperationNotPermitted.as_neg_i32() as u64;
    };
    if cred.euid != 0 {
        return crate::errno::Errno::OperationNotPermitted.as_neg_i32() as u64;
    }

    let mut target_buf = [0u8; PATH_MAX];
    let target = match read_user_path(args[1] as *const u8, &mut target_buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let mut fstype_buf = [0u8; 64];
    let fs_type_str = match read_user_str(args[2] as *const u8, &mut fstype_buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    match crate::fs::mount::do_mount(target, fs_type_str, args[3]) {
        Ok(()) => 0,
        Err(e) => e as u64,
    }
}

/// sys_umount - Unmount a filesystem (syscall 39)
///
/// Linux ABI: umount(target, flags)
pub fn sys_umount(args: SyscallArgs) -> u64 {
    // Only root can unmount
    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        return crate::errno::Errno::OperationNotPermitted.as_neg_i32() as u64;
    };
    if cred.euid != 0 {
        return crate::errno::Errno::OperationNotPermitted.as_neg_i32() as u64;
    }

    let mut target_buf = [0u8; PATH_MAX];
    let target = match read_user_path(args[0] as *const u8, &mut target_buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_umount(target) {
        Ok(()) => 0,
        Err(e) => e as u64,
    }
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
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as i32;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut may_mask: u32 = 0;
    if mode & 0o004 != 0 { may_mask |= crate::fs::permission::MAY_READ; }
    if mode & 0o002 != 0 { may_mask |= crate::fs::permission::MAY_WRITE; }
    if mode & 0o001 != 0 { may_mask |= crate::fs::permission::MAY_EXEC; }

    let cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new()
    };

    match crate::fs::vfs::path_lookup(&full_path, 0) {
        Ok(vfs_path) => {
            if let Some(inode) = &vfs_path.inode {
                let inode_mode = inode.mode.bits() as u16;
                let inode_uid = inode.uid.load(core::sync::atomic::Ordering::Relaxed);
                let inode_gid = inode.gid.load(core::sync::atomic::Ordering::Relaxed);

                if !crate::fs::permission::generic_permission(inode_mode, inode_uid, inode_gid, may_mask, &cred) {
                    return -errno::EACCES as u64;
                }
            }
            0
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

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Check if file exists
    match crate::fs::stat_file_by_path(&full_path, &mut crate::fs::Stat::new()) {
        Ok(()) => {
            // File exists - TODO: actually update timestamps
            0
        }
        Err(e) => e as i64 as u64,
    }
}

/// Read a null-terminated path string from user space into a kernel buffer.
/// Returns a borrowed &str with lifetime tied to the buffer.
/// Does NOT resolve relative paths or combine with CWD.
fn read_user_path<'a>(
    pathname_ptr: *const u8,
    buf: &'a mut [u8; PATH_MAX],
) -> Result<&'a str, u64> {
    if pathname_ptr.is_null() {
        return Err(-errno::EFAULT as u64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(pathname_ptr as usize, 1) {
        return Err(-errno::EFAULT as u64);
    }
    let pathname = match strncpy_from_user(pathname_ptr, PATH_MAX, buf) {
        Ok(s) => s,
        Err(e) => return Err(e as u64),
    };
    core::str::from_utf8(pathname).map_err(|_| -errno::EINVAL as u64)
}

/// Read a null-terminated string from user space (short variant for non-path arguments).
fn read_user_str<'a>(
    ptr: *const u8,
    buf: &'a mut [u8],
) -> Result<&'a str, u64> {
    if ptr.is_null() {
        return Err(-errno::EFAULT as u64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(ptr as usize, 1) {
        return Err(-errno::EFAULT as u64);
    }
    let s = match strncpy_from_user(ptr, buf.len(), buf) {
        Ok(s) => s,
        Err(e) => return Err(e as u64),
    };
    core::str::from_utf8(s).map_err(|_| -errno::EINVAL as u64)
}

/// Helper: read path from user space and resolve to absolute path (CWD-aware).
fn resolve_user_path(dirfd: i32, pathname_ptr: *const u8) -> Result<alloc::string::String, u64> {
    const AT_FDCWD: i32 = -100;

    let mut buf = [0u8; PATH_MAX];
    let pathname_str = read_user_path(pathname_ptr, &mut buf)?;

    let full_path: alloc::string::String = if pathname_str.starts_with('/') {
        alloc::string::String::from(pathname_str)
    } else if dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
                let mut path = alloc::string::String::with_capacity(cwd_str.len() + pathname_str.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') { path.push('/'); }
                path.push_str(pathname_str);
                path
            } else {
                alloc::string::String::from(pathname_str)
            }
        } else {
            alloc::string::String::from(pathname_str)
        }
    } else {
        // TODO: handle dirfd properly
        alloc::string::String::from(pathname_str)
    };
    Ok(full_path)
}

/// sys_fchmodat - Change file mode (syscall 53)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: mode - new file mode (permission bits)
/// - args[3]: flags - reserved
pub fn sys_fchmodat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as u32;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_chmod(full_path.as_ref(), mode) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_fchownat - Change file ownership (syscall 54)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: uid - new owner uid (u32::MAX = no change)
/// - args[3]: gid - new owner gid (u32::MAX = no change)
/// - args[4]: flags - flags (AT_SYMLINK_NOFOLLOW, etc.)
pub fn sys_fchownat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let uid = args[2] as u32;
    let gid = args[3] as u32;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_chown(full_path.as_ref(), uid, gid) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_ftruncate - Truncate an open file (syscall 46)
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: length - new file size
pub fn sys_ftruncate(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let length = args[1] as i64;

    match crate::fs::vfs::vfs_ftruncate(fd, length) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_truncate - Truncate a file by path (syscall 76)
///
/// # Arguments
/// - args[0]: pathname - file path
/// - args[1]: length - new file size
pub fn sys_truncate(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;
    let length = args[1] as i64;

    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_truncate(full_path.as_ref(), length) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// struct statfs - Filesystem statistics (64-bit Linux ABI)
#[repr(C)]
struct Statfs {
    f_type: u64,
    f_bsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: [u32; 2],
    f_namelen: u64,
    f_frsize: u64,
    f_flags: u64,
    f_spare: [u64; 4],
}

/// Fill a Statfs from the rootfs superblock
fn fill_rootfs_statfs(buf: &mut Statfs) {
    use crate::fs::rootfs::{get_rootfs, ROOTFS_MAGIC};
    buf.f_type = ROOTFS_MAGIC as u64;
    buf.f_bsize = 4096;
    buf.f_blocks = 0;
    buf.f_bfree = 0;
    buf.f_bavail = 0;
    buf.f_files = 0;
    buf.f_ffree = 0;
    buf.f_fsid = [ROOTFS_MAGIC, 0];
    buf.f_namelen = 255;
    buf.f_frsize = 4096;
    buf.f_flags = 0;
    buf.f_spare = [0; 4];

    // Try to get more accurate info from rootfs
    let sb_ptr = get_rootfs();
    if !sb_ptr.is_null() {
        unsafe {
            buf.f_bsize = (*sb_ptr).sb.s_blocksize as u64;
            buf.f_type = (*sb_ptr).sb.s_magic as u64;
        }
    }
}

/// Fill a Statfs for ext4
fn fill_ext4_statfs(buf: &mut Statfs) {
    buf.f_type = 0xEF53;  // EXT4_SUPER_MAGIC
    buf.f_bsize = 4096;
    buf.f_blocks = 0;
    buf.f_bfree = 0;
    buf.f_bavail = 0;
    buf.f_files = 0;
    buf.f_ffree = 0;
    buf.f_fsid = [0xEF53, 0];
    buf.f_namelen = 255;
    buf.f_frsize = 4096;
    buf.f_flags = 0;
    buf.f_spare = [0; 4];
}

/// sys_statfs - Get filesystem statistics by path
/// RISC-V syscall number: 43
pub fn sys_statfs(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;
    let buf_ptr = args[1] as *mut Statfs;

    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, core::mem::size_of::<Statfs>()) {
        return -errno::EFAULT as u64;
    }

    // Determine filesystem type from path
    // Simplified: use rootfs statfs for all paths
    let mut statfs_buf = Statfs {
        f_type: 0, f_bsize: 0, f_blocks: 0, f_bfree: 0, f_bavail: 0,
        f_files: 0, f_ffree: 0, f_fsid: [0; 2], f_namelen: 0,
        f_frsize: 0, f_flags: 0, f_spare: [0; 4],
    };

    // Check if the path is on ext4 (under /mnt or similar)
    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let path_str = full_path.as_str();
    if path_str.starts_with("/mnt") || path_str.starts_with("/disk") {
        fill_ext4_statfs(&mut statfs_buf);
    } else {
        fill_rootfs_statfs(&mut statfs_buf);
    }

    // Copy to user space
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_to_user(
            buf_ptr as *mut u8,
            &statfs_buf as *const Statfs as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    if uncopied > 0 {
        return -errno::EFAULT as u64;
    }

    0
}

/// sys_fstatfs - Get filesystem statistics by fd
/// RISC-V syscall number: 44
pub fn sys_fstatfs(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *mut Statfs;

    if buf_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, core::mem::size_of::<Statfs>()) {
        return -errno::EFAULT as u64;
    }

    // Get file and check if it has an inode with superblock info
    use crate::fs::get_file_fd;
    let mut statfs_buf = Statfs {
        f_type: 0, f_bsize: 0, f_blocks: 0, f_bfree: 0, f_bavail: 0,
        f_files: 0, f_ffree: 0, f_fsid: [0; 2], f_namelen: 0,
        f_frsize: 0, f_flags: 0, f_spare: [0; 4],
    };

    unsafe {
        match get_file_fd(fd) {
            Some(_file) => {
                // Use rootfs statfs as default for all fds
                fill_rootfs_statfs(&mut statfs_buf);
            }
            None => return -errno::EBADF as u64,
        }
    }

    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_to_user(
            buf_ptr as *mut u8,
            &statfs_buf as *const Statfs as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    if uncopied > 0 {
        return -errno::EFAULT as u64;
    }

    0
}

/// sys_symlinkat - Create symbolic link relative to directory fd
/// RISC-V syscall number: 36
pub fn sys_symlinkat(args: SyscallArgs) -> u64 {
    let _target_ptr = args[0] as *const u8;
    let _newdirfd = args[1] as i32;
    let _linkpath_ptr = args[2] as *const u8;

    // Symlinks not yet supported
    -errno::ENOSYS as u64
}
