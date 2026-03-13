//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IO-related system calls
//!
//! Includes: read, write,writev, dup, dup2, fcntl, ioctl, flock, pipe2

use super::*;
use core::sync::atomic::{AtomicU32, Ordering};

/// iovec structure (for writev/readv)
#[repr(C)]
struct Iovec{
    iov_base: *const u8,
    iov_len: usize,
}

// ============================================================================
// Terminal (TTY) state
// ============================================================================

/// Termios local flags ( c_lflag)
const L_ISIG: u32   = 0x0001;   // Signal handling enabled
const L_ICANON: u32 = 0x0100;   // Canonical mode
const L_ECHO: u32   = 0x0008;   // Echo enabled
const L_ECHOE: u32  = 0x0010;   // Echo erase
const L_ECHOK: u32 = 0x0020;   // Echo kill

/// Global terminal settings ( simplified - single console)
/// c_lflag stores the local mode flags
static TTY_LFLAG: AtomicU32 = AtomicU32::new(L_ISIG | L_ICANON | L_ECHO | L_ECHOE | L_ECHOK);

/// Check if terminal echo is enabled
pub fn tty_echo_enabled() -> bool {
    (TTY_LFLAG.load(Ordering::Relaxed) & L_ECHO) != 0
}

/// Get terminal c_lflag
pub fn tty_get_lflag() -> u32 {
    TTY_LFLAG.load(Ordering::Relaxed)
}

/// Set terminal c_lflag
pub fn tty_set_lflag(lflag: u32) {
    TTY_LFLAG.store(lflag, Ordering::Release);
}

/// sys_read - Read data from file descriptor
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: buf - destination buffer pointer
/// - args[2]: count - number of bytes to read
///
/// # Returns
/// Returns number of bytes read on success, negative error code on failure
pub fn sys_read(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;

    // Check if buffer address is in valid user space using access_ok
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        crate::println!("sys_read: access_ok failed for buf={:#x}, count={}", buf as usize, count);
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

/// sys_write - Write data to file descriptor
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: buf - source buffer pointer
/// - args[2]: count - number of bytes to write
///
/// # Returns
/// Returns number of bytes written on success, negative error code on failure
pub fn sys_write(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;

    // Check if buffer address is in valid user space using access_ok
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as u64;
    }

    // Check if count is reasonable
    if count == 0 {
        return 0;
    }

    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                // Check if this is the original console (UART) stdout/stderr
                // by checking if the file ops match UART_OPS
                use crate::fs::char_dev::UART_OPS;
                let ops = (*file).get_ops();
                if (fd == 1 || fd == 2) && ops.is_some_and(|o| core::ptr::eq(o, &UART_OPS as *const _)) {
                    // Console output: use putchar for proper handling
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

                // Regular file or redirected output
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

/// sys_writev - Write multiple buffers to file descriptor
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: iov - pointer to iovec structure array
/// - args[2]: iovcnt - length of iovec array
///
/// # Returns
/// Returns total bytes written on success, negative error code on failure
pub fn sys_writev(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;

    // Check iovec array pointer using access_ok
    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as u64;
    }

    let mut total_written: isize = 0;
    let mut has_valid_iov = false;

    unsafe {
        for i in 0..iovcnt {
            let iov = &*iov_ptr.add(i);

            let base = iov.iov_base as usize;
            // Check each iov buffer using access_ok
            if iov.iov_len > 0 && crate::arch::riscv64::uaccess::access_ok(base, iov.iov_len) {
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

/// sys_dup - Duplicate file descriptor
pub fn sys_dup(args: SyscallArgs) -> u64 {
    let oldfd = args[0] as usize;

    unsafe {
        match crate::sched::get_current_fdtable() {
            Some(fdtable) => {
                match fdtable.dup_fd(oldfd) {
                    Some(newfd) => newfd as u64,
                    None => -errno::EBADF as i64 as u64,
                }
            }
            None => -errno::EBADF as i64 as u64,
        }
    }
}

/// sys_dup2 - Duplicate file descriptor to specified number
pub fn sys_dup2(args: SyscallArgs) -> u64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;

    unsafe {
        match crate::sched::get_current_fdtable() {
            Some(fdtable) => {
                match fdtable.dup2_fd(oldfd, newfd) {
                    Some(fd) => fd as u64,
                    None => -errno::EBADF as i64 as u64,
                }
            }
            None => -errno::EBADF as i64 as u64,
        }
    }
}

/// sys_fcntl - File control
pub fn sys_fcntl(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let cmd = args[1] as usize;
    let arg = args[2] as usize;

    match crate::fs::vfs::file_fcntl(fd, cmd, arg) {
        Ok(result) => result as u64,
        Err(errno) => errno as u64,
    }
}

/// sys_ioctl - IO control
pub fn sys_ioctl(args: SyscallArgs) -> u64 {
    let fd = args[0] as i32;
    let request = args[1] as u32;
    let arg = args[2] as usize;

    // Special handling for framebuffer device (fd >= 1000 is device file)
    if fd >= 1000 {
        let result = crate::drivers::gpu::fbdev_ioctl(request, arg) as i64;
        return result as u64;
    }

    // TTY ioctl commands
    match request {
        // TCGETS - Get terminal attributes (0x5401)
        0x5401 => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            // Check address validity (termios struct ~60 bytes)
            if !crate::arch::riscv64::uaccess::access_ok(arg, 60) {
                return -errno::EFAULT as u64;
            }
            // Fill termios structure with current settings
            let lflag = tty_get_lflag();
            unsafe {
                let ptr = arg as *mut u32;
                // c_iflag: ICRNL | IXON
                *ptr.offset(0) = 0x0100 | 0x0400;
                // c_oflag: OPOST | ONLCR
                *ptr.offset(1) = 0x0001 | 0x0004;
                // c_cflag: B38400 | CS8 | CREAD | HUPCL
                *ptr.offset(2) = 0x000F | 0x0030 | 0x0080 | 0x0400;
                // c_lflag: use current settings
                *ptr.offset(3) = lflag;
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
        // TCSETS, TCSETSW, TCSETSF - Set terminal attributes
        0x5402 | 0x5403 | 0x5404 => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            // Check address validity
            if !crate::arch::riscv64::uaccess::access_ok(arg, 60) {
                return -errno::EFAULT as u64;
            }
            // Read c_lflag from user space and update global state
            unsafe {
                let ptr = arg as *const u32;
                let lflag = *ptr.offset(3);
                tty_set_lflag(lflag);
            }
            0
        }
        // TIOCGWINSZ - Get window size (0x5413)
        0x5413 => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            // Check address validity (winsize struct 8 bytes)
            if !crate::arch::riscv64::uaccess::access_ok(arg, 8) {
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
        // TIOCSWINSZ - Set window size (0x5414)
        0x5414 => {
            0  // Ignore setting
        }
        // FIONREAD - Get readable byte count (0x541B)
        0x541B => {
            if arg == 0 {
                return -errno::EFAULT as u64;
            }
            // Check address validity
            if !crate::arch::riscv64::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as u64;
            }
            unsafe {
                let ptr = arg as *mut i32;
                *ptr = 0;  // Simplified: return 0
            }
            0
        }
        // Other TTY commands
        _ if (request & 0xFF00) == 0x5400 => {
            0  // Simplified: return success
        }
        // Other commands
        _ => {
            // For stdin/stdout/stderr, return success
            if fd >= 0 && fd <= 2 {
                0
            } else {
                -errno::ENOTTY as u64
            }
        }
    }
}

/// sys_flock - File lock (simplified implementation)
pub fn sys_flock(_args: SyscallArgs) -> u64 {
    // Simplified implementation: always return success
    0
}

/// sys_pipe2 - Create pipe with flags
pub fn sys_pipe2(args: SyscallArgs) -> u64 {
    let pipefd = args[0] as *mut i32;
    let _flags = args[1] as u32;

    // Check pointer using access_ok
    if pipefd.is_null() {
        return -errno::EFAULT as u64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(pipefd as usize, 8) {  // 2 * sizeof(int)
        return -errno::EFAULT as u64;
    }

    // Create pipe
    let (read_file, write_file) = crate::fs::pipe::create_pipe();

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EMFILE as u64,
    };

    // Allocate file descriptors
    let read_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    let write_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    // Install files to fdtable
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
