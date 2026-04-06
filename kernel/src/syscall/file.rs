//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File system related system calls
//!
//! Includes: open, openat, close, fstat, getdents64, mkdir, rmdir, unlink, readlinkat, lseek, chdir, getcwd, umask

use super::*;
use crate::arch::riscv64::uaccess::strncpy_from_user;

/// Maximum path length (PATH_MAX)
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

/// Parse /proc/[pid]/fd path for O_DIRECTORY open.
/// Returns the PID if path matches /proc/{pid}/fd (exact, no trailing content).
fn parse_proc_fd_dir_path(path: &str) -> Option<u64> {
    let path = path.trim_end_matches('/');
    // Match /proc/{pid}/fd
    let path = path.trim_start_matches('/');
    let parts: alloc::vec::Vec<&str> = path.split('/').collect();
    if parts.len() == 3 && parts[0] == "proc" && parts[2] == "fd" {
        parts[1].parse().ok()
    } else {
        None
    }
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

    // Shortcut: /proc/[pid]/fd with O_DIRECTORY
    if (flags & O_CREAT) == 0 && (flags & O_DIRECTORY) != 0 {
        if let Some(pid) = parse_proc_fd_dir_path(&full_path) {
            return match crate::fs::vfs::open_procfs_dir(pid, flags) {
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
            };
        }
    }

    // Shortcut: /proc/[pid]/xxx paths go through procfs read_file
    // because VFS inode lookup doesn't support PID subdirectories
    if (flags & O_CREAT) == 0 && (flags & O_DIRECTORY) == 0 {
        if let Some(content) = crate::fs::procfs::read_file(&full_path) {
            return match crate::fs::vfs::open_mem_file(content, flags) {
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
            };
        }
    }

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
    let fd = args[0] as usize;

    // Handle POSIX MQ fds (range 512+)
    if fd >= 512 {
        return crate::ipc::posix_mq::close_mq_fd(fd as i32) as u64;
    }

    use crate::fs::close_file_fd;
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

    let ret = match stat_file_by_path(&full_path, &mut stat) {
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
                -errno::EFAULT as u64
            } else {
                0
            }
        }
        Err(errno) => errno as i64 as u64,
    };
    ret
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
/// Syscall number: 34
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
    let dirfd = args[0] as i32;
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

    // /proc/self/exe - return program path
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

    // /proc/[pid]/fd/N - return fd symlink target
    // Supports both absolute and relative paths
    let full_path = resolve_proc_readlink_path(dirfd, pathname);
    if let Some(target) = handle_proc_fd_readlink(&full_path) {
        if target.len() >= bufsize {
            return -errno::ENAMETOOLONG as u64;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(target.as_ptr(), buf, target.len());
        }
        return target.len() as u64;
    }

    -errno::ENOENT as u64
}

/// Resolve path for procfs readlink (handles /proc/self/ -> /proc/{pid}/)
fn resolve_proc_readlink_path(dirfd: i32, pathname: &str) -> alloc::string::String {
    const AT_FDCWD: i32 = -100;

    if pathname.starts_with("/proc/") {
        // Already absolute - just resolve "self" to PID
        let mut path = alloc::string::String::from(pathname);
        if path.contains("/self/") {
            let pid = unsafe { crate::process::current_pid() };
            path = path.replace("/self/", &alloc::format!("/{}/", pid));
        }
        return path;
    }

    // Relative path - try to resolve using dirfd
    if dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            let cwd_str = core::str::from_utf8(&cwd).unwrap_or("?");
            let mut full = alloc::format!("{}{}", cwd_str, pathname);
            if full.contains("/self/") {
                let pid = unsafe { crate::process::current_pid() };
                full = full.replace("/self/", &alloc::format!("/{}/", pid));
            }
            return full;
        }
    }

    alloc::string::String::from(pathname)
}

/// Handle /proc/[pid]/fd/N readlink, returns symlink target or None
fn handle_proc_fd_readlink(path: &str) -> Option<alloc::vec::Vec<u8>> {
    // Match pattern: /proc/{pid}/fd/{fd_num}
    let path = path.trim_start_matches('/');

    let parts: alloc::vec::Vec<&str> = path.split('/').collect();
    if parts.len() < 3 {
        return None;
    }

    // parts[0] = "proc", parts[1] = pid or "self" (already resolved), parts[2] = "fd"
    if parts[0] != "proc" || parts[2] != "fd" {
        return None;
    }

    let pid: u64 = match parts[1].parse() {
        Ok(p) => p,
        Err(_) => return None,
    };

    let fd: u32 = match parts[3].parse() {
        Ok(f) => f,
        Err(_) => return None,
    };

    let target = crate::fs::procfs::pid::generate_fd_link(pid, fd);
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
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
/// mount(source, target, filesystemtype, mountflags, data)
pub fn sys_mount(args: SyscallArgs) -> u64 {
    // CAP_SYS_ADMIN required to mount
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
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
/// umount(target, flags)
pub fn sys_umount(args: SyscallArgs) -> u64 {
    // CAP_SYS_ADMIN required to unmount
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
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
        crate::process::task::Cred::new_init()
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
pub fn resolve_user_path(dirfd: i32, pathname_ptr: *const u8) -> Result<alloc::string::String, u64> {
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

/// struct statfs - Filesystem statistics (64-bit)
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
    let target_ptr = args[0] as *const u8;
    let newdirfd = args[1] as i32;
    let linkpath_ptr = args[2] as *const u8;

    // Read target path (symlink content — NOT resolved against CWD)
    let mut target_buf = [0u8; PATH_MAX];
    let target = match read_user_path(target_ptr, &mut target_buf) {
        Ok(s) => s,
        Err(e) => return e,
    };

    // Resolve link path (where the symlink will be created)
    let full_path = match resolve_user_path(newdirfd, linkpath_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_symlink(&full_path, target) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_statx - Extended file status (syscall 291)
///
/// statx(dirfd, pathname, flags, mask, statxbuf)
pub fn sys_statx(args: SyscallArgs) -> u64 {
    use crate::fs::stat::{Stat, Statx};

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let _flags = args[2] as u32;
    let mask = args[3] as u32;
    let statxbuf = args[4] as *mut Statx;

    if statxbuf.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(statxbuf as usize, core::mem::size_of::<Statx>()) {
        return -errno::EFAULT as u64;
    }

    let mut stat = Stat::new();

    // TODO: Support AT_SYMLINK_NOFOLLOW (0x100) — requires no-follow path lookup
    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if let Err(e) = crate::fs::vfs::stat_file_by_path(&full_path, &mut stat) {
        return e as i64 as u64;
    }

    // STATX mask constants
    const STATX_TYPE: u32 = 0x0001;
    const STATX_MODE: u32 = 0x0002;
    const STATX_NLINK: u32 = 0x0004;
    const STATX_UID: u32 = 0x0008;
    const STATX_GID: u32 = 0x0010;
    const STATX_ATIME: u32 = 0x0020;
    const STATX_MTIME: u32 = 0x0040;
    const STATX_CTIME: u32 = 0x0080;
    const STATX_INO: u32 = 0x0100;
    const STATX_SIZE: u32 = 0x0200;
    const STATX_BLOCKS: u32 = 0x0400;

    let mut stx = Statx::new();
    let mut requested = 0u32;

    if mask & STATX_TYPE != 0 { requested |= STATX_TYPE; }
    if mask & STATX_MODE != 0 { requested |= STATX_MODE; }
    if mask & STATX_NLINK != 0 { requested |= STATX_NLINK; }
    if mask & STATX_UID != 0 { requested |= STATX_UID; }
    if mask & STATX_GID != 0 { requested |= STATX_GID; }
    if mask & STATX_ATIME != 0 { requested |= STATX_ATIME; }
    if mask & STATX_MTIME != 0 { requested |= STATX_MTIME; }
    if mask & STATX_CTIME != 0 { requested |= STATX_CTIME; }
    if mask & STATX_INO != 0 { requested |= STATX_INO; }
    if mask & STATX_SIZE != 0 { requested |= STATX_SIZE; }
    if mask & STATX_BLOCKS != 0 { requested |= STATX_BLOCKS; }

    stx.stx_mask = requested;
    stx.stx_blksize = stat.st_blksize as u32;

    if requested & STATX_TYPE != 0 || requested & STATX_MODE != 0 {
        stx.stx_mode = stat.st_mode as u16;
    }
    if requested & STATX_NLINK != 0 { stx.stx_nlink = stat.st_nlink; }
    if requested & STATX_UID != 0 { stx.stx_uid = stat.st_uid; }
    if requested & STATX_GID != 0 { stx.stx_gid = stat.st_gid; }
    if requested & STATX_ATIME != 0 {
        stx.stx_atime.tv_sec = stat.st_atime;
        stx.stx_atime.tv_nsec = stat.st_atime_nsec as u32;
    }
    if requested & STATX_MTIME != 0 {
        stx.stx_mtime.tv_sec = stat.st_mtime;
        stx.stx_mtime.tv_nsec = stat.st_mtime_nsec as u32;
    }
    if requested & STATX_CTIME != 0 {
        stx.stx_ctime.tv_sec = stat.st_ctime;
        stx.stx_ctime.tv_nsec = stat.st_ctime_nsec as u32;
    }
    if requested & STATX_INO != 0 { stx.stx_ino = stat.st_ino; }
    if requested & STATX_SIZE != 0 { stx.stx_size = stat.st_size as u64; }
    if requested & STATX_BLOCKS != 0 { stx.stx_blocks = stat.st_blocks as u64; }

    // Copy to user space
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_to_user(
            statxbuf as *mut u8,
            &stx as *const Statx as *const u8,
            core::mem::size_of::<Statx>(),
        )
    };
    if uncopied != 0 {
        return -errno::EFAULT as u64;
    }

    0
}

/// sys_openat2 - Open file extended (syscall 437)
///
/// openat2(dirfd, pathname, how, size)
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

pub fn sys_openat2(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let how_ptr = args[2] as *const OpenHow;
    let size = args[3] as usize;

    // Validate: size must cover at least flags + mode + resolve (24 bytes)
    const OPEN_HOW_VER0_SIZE: usize = 24;
    const OPEN_HOW_MAX_SIZE: usize = 24;

    if size < OPEN_HOW_VER0_SIZE || size > OPEN_HOW_MAX_SIZE {
        return -errno::EINVAL as u64;
    }

    if how_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(how_ptr as usize, size) {
        return -errno::EFAULT as u64;
    }

    // Read struct open_how from user space
    let mut buf = [0u8; OPEN_HOW_MAX_SIZE];
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(
            buf.as_mut_ptr(),
            how_ptr as *const u8,
            size,
        )
    };
    if uncopied > 0 {
        return -errno::EFAULT as u64;
    }
    let how: OpenHow = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const OpenHow) };

    // Validate flags: reject O_CREAT/O_EXCL/O_TRUNC for now (simplification)
    // Actually, just pass through — sys_openat handles these
    // Validate resolve flags: only accept known bits
    const RESOLVE_NO_XDEV: u64 = 0x01;
    const RESOLVE_NO_SYMLINKS: u64 = 0x02;
    const RESOLVE_NO_MAGICLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;
    const RESOLVE_IN_ROOT: u64 = 0x10;
    const RESOLVE_CACHED: u64 = 0x20;

    let supported = RESOLVE_NO_XDEV | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS
        | RESOLVE_BENEATH | RESOLVE_IN_ROOT | RESOLVE_CACHED;
    if (how.resolve & !supported) != 0 {
        return -errno::EINVAL as u64;
    }

    // TODO: Implement RESOLVE_* semantics (currently just delegate to openat)
    let openat_args = [
        dirfd as u64,
        pathname_ptr as u64,
        how.flags,
        how.mode,
        0, 0,
    ];
    sys_openat(openat_args)
}

/// sys_mknodat - create a filesystem node
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - path pointer
/// - args[2]: mode - file mode (type + permissions)
/// - args[3]: dev - device number (for block/char special files)
pub fn sys_mknodat(args: SyscallArgs) -> u64 {
    let _dirfd = args[0] as i32;
    let pathname = args[1] as *const u8;
    let mode = args[2] as u32;
    let _dev = args[3] as u32;

    let path = match resolve_user_path(_dirfd, pathname) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Only support regular files and directories for now
    let ftype = mode & 0o170000;
    if ftype != 0o100000 && ftype != 0o040000 {
        // TODO: support FIFO (S_IFIFO), block/char devices
        return -errno::EINVAL as u64;
    }

    if ftype == 0o040000 {
        // Directory
        match crate::fs::vfs::vfs_mkdir(&path, mode & 0o7777) {
            Ok(_) => 0,
            Err(e) => -e as u64,
        }
    } else {
        // Regular file - create empty file via open with O_CREAT|O_TRUNC
        match crate::fs::file_open(&path, 0o100 | 0o200, mode & 0o777) {
            Ok(fd) => {
                // Close the fd immediately
                unsafe { crate::fs::close_file_fd(fd); }
                0
            }
            Err(e) => e as i64 as u64,
        }
    }
}

/// sys_fchmod - change file permissions
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: mode - file mode
pub fn sys_fchmod(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let mode = args[1] as u32;

    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(file) => {
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt.as_ref() {
                Some(inode) => {
                    // Permission check: require CAP_FOWNER or be the file owner
                    let cred = match crate::sched::current() {
                        Some(t) => t.cred().clone(),
                        None => return -errno::EPERM as u64,
                    };
                    let inode_uid = inode.uid.load(core::sync::atomic::Ordering::Relaxed);
                    if !crate::security::has_capability(&cred, crate::security::CAP_FOWNER)
                        && cred.euid != inode_uid {
                        return -errno::EPERM as u64;
                    }
                    // Use op_setattr to update mode
                    let result = inode.op_setattr(0x0001, mode as u64, 0);
                    if result == 0 { 0 } else { -result as u64 }
                }
                None => -errno::EBADF as u64,
            }
        }
        None => -errno::EBADF as u64,
    }
}

/// sys_fallocate - preallocate or deallocate space to a file
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: mode - operation mode (FALLOC_FL_KEEP_SIZE, etc.)
/// - args[2]: offset - offset within file
/// - args[3]: length - length to allocate
pub fn sys_fallocate(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let mode = args[1] as i32;
    let offset = args[2] as i64;
    let length = args[3] as i64;

    const FALLOC_FL_KEEP_SIZE: i32 = 1;
    const FALLOC_FL_PUNCH_HOLE: i32 = 2;
    const FALLOC_FL_COLLAPSE_RANGE: i32 = 8;
    const FALLOC_FL_ZERO_RANGE: i32 = 16;
    const FALLOC_FL_INSERT_RANGE: i32 = 32;
    const FALLOC_FL_UNSHARE_RANGE: i32 = 64;

    if offset < 0 || length <= 0 {
        return -errno::EINVAL as u64;
    }

    // Validate fd is open
    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(_) => {}
        None => return -errno::EBADF as u64,
    }

    match mode {
        FALLOC_FL_KEEP_SIZE | 0 => {
            // Simple preallocation — succeed silently
            // FALLOC_FL_KEEP_SIZE: allocate space but don't change file size
            0
        }
        FALLOC_FL_PUNCH_HOLE | (FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE) => {
            // Punch hole — succeed silently (file system doesn't support hole punching)
            0
        }
        FALLOC_FL_ZERO_RANGE | (FALLOC_FL_ZERO_RANGE | FALLOC_FL_KEEP_SIZE) => {
            // Zero range — succeed silently
            0
        }
        FALLOC_FL_COLLAPSE_RANGE | FALLOC_FL_INSERT_RANGE | FALLOC_FL_UNSHARE_RANGE => {
            // These require filesystem support
            -errno::EOPNOTSUPP as u64
        }
        _ => -errno::EINVAL as u64,
    }
}

/// sys_futimesat - change file timestamps (NR 88 = utimensat)
pub fn sys_fsync(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(_file) => {
            // TODO: implement fsync for individual files
            0 // Success stub
        }
        None => -errno::EBADF as u64,
    }
}

/// sys_fdatasync - synchronize a file's in-core data with storage device
pub fn sys_fdatasync(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(_) => 0,
        None => -errno::EBADF as u64,
    }
}

/// sys_sync - synchronize filesystem caches
///
/// # Arguments: none
pub fn sys_sync(_args: SyscallArgs) -> u64 {
    // Flush all buffer cache
    let _ = crate::fs::bio::sync_buffers();
    0
}

/// Extended attribute syscalls (NR 5-16) - all stubs returning -ENOSYS
pub fn sys_setxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_lsetxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_fsetxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_getxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_lgetxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_fgetxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_listxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_llistxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_flistxattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_removexattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_lremovexattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }
pub fn sys_fremovexattr(_args: SyscallArgs) -> u64 { -errno::ENOSYS as u64 }

/// sys_chroot - Change root directory (NR 51)
pub fn sys_chroot(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;
    // CAP_SYS_CHROOT required to chroot
    if !crate::security::capable(crate::security::CAP_SYS_CHROOT) {
        return -errno::EPERM as u64;
    }

    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Verify path is a directory
    let mut stat = crate::fs::Stat::new();
    if let Err(e) = crate::fs::vfs::stat_file_by_path(&full_path, &mut stat) {
        return e as i64 as u64;
    }
    if stat.st_mode & 0o170000 != 0o040000 {
        return -errno::ENOTDIR as u64;
    }

    // TODO: implement actual root switching
    0
}

/// sys_fchown - Change file ownership by fd (NR 55)
pub fn sys_fchown(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let uid = args[1] as u32;
    let gid = args[2] as u32;

    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(file) => {
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt.as_ref() {
                Some(inode) => {
                    // Permission check: require CAP_CHOWN
                    let cred = match crate::sched::current() {
                        Some(t) => t.cred().clone(),
                        None => return -errno::EPERM as u64,
                    };
                    if !crate::security::has_capability(&cred, crate::security::CAP_CHOWN) {
                        return -errno::EPERM as u64;
                    }
                    if uid != u32::MAX {
                        inode.uid.store(uid, core::sync::atomic::Ordering::Relaxed);
                    }
                    if gid != u32::MAX {
                        inode.gid.store(gid, core::sync::atomic::Ordering::Relaxed);
                    }
                    0
                }
                None => -errno::EBADF as u64,
            }
        }
        None => -errno::EBADF as u64,
    }
}

/// sys_vhangup - Simulate hangup on current tty (NR 58)
pub fn sys_vhangup(_args: SyscallArgs) -> u64 {
    // Permission check: require CAP_SYS_TTY_CONFIG
    if !crate::security::capable(crate::security::CAP_SYS_TTY_CONFIG) {
        return -errno::EPERM as u64;
    }
    0
}

/// sys_sync_file_range - Sync file range to disk (NR 84)
pub fn sys_sync_file_range(args: SyscallArgs) -> u64 {
    let _fd = args[0] as i32;
    let _offset = args[1] as i64;
    let _nbytes = args[2] as i64;
    let _flags = args[3] as u32;
    // Simplified: sync all buffers
    let _ = crate::fs::bio::sync_buffers();
    0
}

/// sys_acct - Enable/disable process accounting (NR 89)
pub fn sys_acct(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;

    if pathname_ptr.is_null() {
        // NULL means disable accounting — succeed
        return 0;
    }

    // Only root can enable accounting
    if let Some(task) = crate::sched::current() {
        if task.cred().euid != 0 {
            return -errno::EPERM as u64;
        }
    } else {
        return -errno::EPERM as u64;
    }

    // Process accounting not implemented — succeed silently
    0
}

/// sys_readahead - Readahead pages into cache (NR 213)
pub fn sys_readahead(_args: SyscallArgs) -> u64 {
    0 // Simplified: success (no-op)
}

/// sys_swapon - Start swap (NR 224)
pub fn sys_swapon(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_swapoff - Stop swap (NR 225)
pub fn sys_swapoff(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_remap_file_pages - Remap file pages (NR 234, deprecated)
pub fn sys_remap_file_pages(_args: SyscallArgs) -> u64 {
    0 // Deprecated, return success
}

/// sys_name_to_handle_at - Get file handle (NR 264)
pub fn sys_name_to_handle_at(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_open_by_handle_at - Open by file handle (NR 265)
pub fn sys_open_by_handle_at(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_copy_file_range - Copy data between files (NR 285)
pub fn sys_copy_file_range(args: SyscallArgs) -> u64 {
    let fd_in = args[0] as i32;
    let _off_in = args[1] as *mut i64;
    let fd_out = args[2] as i32;
    let _off_out = args[3] as *mut i64;
    let len = args[4] as usize;
    let _flags = args[5] as u32;

    if len == 0 { return 0; }

    use crate::fs::get_file_fd;
    unsafe {
        let in_file = match get_file_fd(fd_in as usize) {
            Some(f) => f,
            None => return -errno::EBADF as u64,
        };
        let out_file = match get_file_fd(fd_out as usize) {
            Some(f) => f,
            None => return -errno::EBADF as u64,
        };

        let mut total = 0usize;
        let mut remaining = len;
        while remaining > 0 {
            let chunk = core::cmp::min(remaining, 8192);
            let mut buf = alloc::vec![0u8; chunk];
            let n = in_file.read(buf.as_mut_ptr(), chunk);
            if n <= 0 { break; }
            let mut written = 0usize;
            while written < n as usize {
                let w = out_file.write(buf.as_ptr().add(written), (n as usize) - written);
                if w <= 0 { return total as u64; }
                written += w as usize;
            }
            total += written;
            remaining -= written;
        }
        total as u64
    }
}

/// sys_preadv2 - Read from fd into vectors v2 (NR 286)
pub fn sys_preadv2(args: SyscallArgs) -> u64 {
    // Same as preadv, ignoring flags
    crate::syscall::io::sys_preadv(args)
}

/// sys_pwritev2 - Write vectors to fd v2 (NR 287)
pub fn sys_pwritev2(args: SyscallArgs) -> u64 {
    // Same as pwritev, ignoring flags
    crate::syscall::io::sys_pwritev(args)
}

// ============================================================================
// NR 428-433: Filesystem mounting/operations
// ============================================================================

/// sys_open_tree - Open a directory tree (NR 428)
pub fn sys_open_tree(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_move_mount - Move a mount (NR 429)
pub fn sys_move_mount(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_fsopen - Open a filesystem (NR 430)
pub fn sys_fsopen(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_fsconfig - Configure a filesystem context (NR 431)
pub fn sys_fsconfig(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_fsmount - Create a mount (NR 432)
pub fn sys_fsmount(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_fspick - Pick a filesystem (NR 433)
pub fn sys_fspick(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

// ============================================================================
// NR 441-443: epoll_pwait2, mount_setattr, quotactl_fd
// ============================================================================

/// sys_epoll_pwait2 - epoll_wait with sigmask (NR 441)
pub fn sys_epoll_pwait2(args: SyscallArgs) -> u64 {
    // Delegate to epoll_pwait, ignoring extra timeout pointer
    crate::syscall::misc::sys_epoll_pwait(args)
}

/// sys_mount_setattr - Change mount attributes (NR 442)
pub fn sys_mount_setattr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_quotactl_fd - Disk quota operations via fd (NR 443)
pub fn sys_quotactl_fd(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

// ============================================================================
// NR 444-446: Landlock
// ============================================================================

/// sys_landlock_create_ruleset - Create landlock ruleset (NR 444)
pub fn sys_landlock_create_ruleset(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_landlock_add_rule - Add landlock rule (NR 445)
pub fn sys_landlock_add_rule(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_landlock_restrict_self - Restrict self with landlock (NR 446)
pub fn sys_landlock_restrict_self(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

// ============================================================================
// NR 449-466: futex_waitv, set_mempolicy_home_node, etc.
// ============================================================================

/// sys_futex_waitv - Wait on multiple futexes (NR 449)
pub fn sys_futex_waitv(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_set_mempolicy_home_node - Set home node for memory policy (NR 450)
pub fn sys_set_mempolicy_home_node(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_cachestat - Get cache stats (NR 451)
pub fn sys_cachestat(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_fchmodat2 - Change file mode (extended) (NR 452)
pub fn sys_fchmodat2(args: SyscallArgs) -> u64 {
    // Delegate to fchmodat, ignoring extra flags
    sys_fchmodat(args)
}

/// sys_map_shadow_stack - Map shadow stack (NR 453)
pub fn sys_map_shadow_stack(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_futex_wake - Wake futex (NR 454)
pub fn sys_futex_wake(args: SyscallArgs) -> u64 {
    // Delegate to futex with FUTEX_WAKE operation
    crate::syscall::sched::sys_futex(args)
}

/// sys_futex_wait - Wait on futex (NR 455)
pub fn sys_futex_wait(args: SyscallArgs) -> u64 {
    crate::syscall::sched::sys_futex(args)
}

/// sys_futex_requeue - Requeue futex (NR 456)
pub fn sys_futex_requeue(args: SyscallArgs) -> u64 {
    crate::syscall::sched::sys_futex(args)
}

/// sys_statmount - Get mount info (NR 457)
pub fn sys_statmount(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_listmount - List mounts (NR 458)
pub fn sys_listmount(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

// ============================================================================
// NR 459-470: LSM, mseal, xattrat, file_getattr/setattr, listns
// ============================================================================

/// sys_lsm_get_self_attr - Get LSM self attribute (NR 459)
pub fn sys_lsm_get_self_attr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_lsm_set_self_attr - Set LSM self attribute (NR 460)
pub fn sys_lsm_set_self_attr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_lsm_list_modules - List LSM modules (NR 461)
pub fn sys_lsm_list_modules(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mseal - Memory sealing (NR 462)
pub fn sys_mseal(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_setxattrat - Set extended attribute at path (NR 463)
pub fn sys_setxattrat(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_getxattrat - Get extended attribute at path (NR 464)
pub fn sys_getxattrat(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_listxattrat - List extended attributes at path (NR 465)
pub fn sys_listxattrat(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_removexattrat - Remove extended attribute at path (NR 466)
pub fn sys_removexattrat(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_open_tree_attr - Open directory tree with attributes (NR 467)
pub fn sys_open_tree_attr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_file_getattr - Get file attributes (NR 468)
pub fn sys_file_getattr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_file_setattr - Set file attributes (NR 469)
pub fn sys_file_setattr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_listns - List namespaces (NR 470)
pub fn sys_listns(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_renameat2 - rename a file (with flags support)
///
/// # Arguments
/// - args[0]: olddirfd - old directory fd
/// - args[1]: oldpath - old path pointer
/// - args[2]: newdirfd - new directory fd
/// - args[3]: newpath - new path pointer
/// - args[4]: flags - rename flags (RENAME_NOREPLACE, etc.)
pub fn sys_renameat2(args: SyscallArgs) -> u64 {
    let olddirfd = args[0] as i32;
    let oldpath = args[1] as *const u8;
    let newdirfd = args[2] as i32;
    let newpath = args[3] as *const u8;
    let _flags = args[4] as u32;

    let old = match resolve_user_path(olddirfd, oldpath) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new = match resolve_user_path(newdirfd, newpath) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::vfs::vfs_rename(&old, &new) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}
