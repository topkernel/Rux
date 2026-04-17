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

/// Foreground process group ID for the console terminal
/// 0 means no foreground group has been set (kernel init owns the terminal)
static TTY_FG_PGRP: AtomicU32 = AtomicU32::new(0);

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
pub fn sys_read(args: SyscallArgs) -> i64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;

    // Check if buffer address is in valid user space using access_ok
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as i64;
    }

    // Check if count is reasonable
    if count == 0 {
        return 0;
    }

    // SAFETY: get_file_fd returns valid File or None; kernel_buf is a fresh allocation.
    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                // Use kernel buffer to avoid directly accessing user memory
                let mut kernel_buf = alloc::vec![0u8; count];
                let result = file.read(kernel_buf.as_mut_ptr(), count);
                if result > 0 {
                    // Copy data back to user space
                    let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                        buf,
                        kernel_buf.as_ptr(),
                        result as usize,
                    );
                    if uncopied > 0 {
                        return -errno::EFAULT as i64;
                    }
                    result as i64
                } else {
                    result as i32 as i64
                }
            }
            None => -errno::EBADF as i64
        }
    }
}

/// sys_pread64 - Read from file descriptor at a given offset
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: buf - destination buffer pointer
/// - args[2]: count - number of bytes to read
/// - args[3]: offset - file offset (signed)
///
/// # Returns
/// Number of bytes read on success, negative errno on failure
///
/// - RISC-V: 67
pub fn sys_pread64(args: SyscallArgs) -> i64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;
    let offset = args[3] as i64;

    // Validate offset
    if offset < 0 {
        return -errno::EINVAL as i64;
    }
    // Check buffer accessibility
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as i64;
    }
    if count == 0 {
        return 0;
    }

    // SAFETY: get_file_fd returns valid File or None; kernel_buf is fresh allocation.
    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                let saved_pos = file.get_pos();
                file.set_pos(offset as u64);

                let mut kernel_buf = alloc::vec![0u8; count];
                let result = file.read(kernel_buf.as_mut_ptr(), count);

                file.set_pos(saved_pos);

                if result > 0 {
                    let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                        buf,
                        kernel_buf.as_ptr(),
                        result as usize,
                    );
                    if uncopied > 0 {
                        return -errno::EFAULT as i64;
                    }
                    result as i64
                } else {
                    result as i32 as i64
                }
            }
            None => -errno::EBADF as i64
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
pub fn sys_write(args: SyscallArgs) -> i64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;

    // Check if buffer address is in valid user space using access_ok
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as i64;
    }

    // Check if count is reasonable
    if count == 0 {
        return 0;
    }

    // SAFETY: get_file_fd returns valid File or None; user pointers validated above.
    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                // Check if this is the original console (UART) stdout/stderr
                // by checking if the file ops match UART_OPS
                use crate::fs::char_dev::UART_OPS;
                let ops = (*file).get_ops();

                if (fd == 1 || fd == 2) && ops.is_some_and(|o| core::ptr::eq(o, &UART_OPS as *const _)) {
                    // Console output: write directly to UART fixmap address
                    // Use a small stack buffer to avoid heap allocation
                    const CHUNK_SIZE: usize = 256;
                    let mut kernel_buf = [0u8; CHUNK_SIZE];
                    let mut remaining = count;
                    let mut total_written = 0;
                    let mut user_ptr = buf;

                    // UART fixmap virtual address (get from fixmap module)
                    let uart_addr = crate::arch::riscv64::mm::fixmap::uart_virt_addr() as *mut u8;

                    while remaining > 0 {
                        let to_copy = core::cmp::min(remaining, CHUNK_SIZE);

                        let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                            kernel_buf.as_mut_ptr(),
                            user_ptr,
                            to_copy
                        );

                        if uncopied > 0 {
                            // Failed to copy some bytes
                            if total_written == 0 {
                                return -errno::EFAULT as i64;
                            }
                            break;
                        }

                        // Output the copied bytes directly to UART
                        for &b in &kernel_buf[..to_copy] {
                            if b == b'\n' {
                                core::ptr::write_volatile(uart_addr, b'\r');
                            }
                            core::ptr::write_volatile(uart_addr, b);
                        }

                        total_written += to_copy;
                        remaining -= to_copy;
                        user_ptr = user_ptr.add(to_copy);
                    }

                    return total_written as i64;
                }

                // Regular file or redirected output
                // Copy user data to kernel buffer first
                let mut kernel_buf = alloc::vec![0u8; count];
                let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                    kernel_buf.as_mut_ptr(),
                    buf,
                    count,
                );
                if uncopied > 0 {
                    return -errno::EFAULT as i64;
                }
                let result = file.write(kernel_buf.as_ptr(), count);
                if result < 0 {
                    result as i32 as i64
                } else {
                    result as i64
                }
            }
            None => -errno::EBADF as i64,
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
pub fn sys_writev(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;

    // Check iovec array pointer using access_ok
    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as i64;
    }

    let mut total_written: isize = 0;
    let mut has_valid_iov = false;

    // SAFETY: iov_ptr validated with access_ok; each iov buffer validated before use.
    unsafe {
        for i in 0..iovcnt {
            let iov_ptr_i = iov_ptr.add(i);

            // Use copy_from_user to safely read iov structure
            let mut iov = Iovec { iov_base: core::ptr::null(), iov_len: 0 };
            let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );

            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;

            // Skip iov with NULL base
            if base == 0 {
                continue;
            }

            // Check each iov buffer using access_ok
            if len > 0 && crate::arch::riscv64::uaccess::access_ok(base, len) {
                has_valid_iov = true;
                let write_args = [fd as u64, iov.iov_base as u64, len as u64, 0, 0, 0];
                let result = sys_write(write_args);

                if result < 0 {
                    if total_written == 0 {
                        return result;
                    }
                    break;
                }
                total_written += result as isize;
            } else if len > 0 {
                return -errno::EFAULT as i64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as i64;
    }

    total_written as i64
}

/// sys_readv - Read data into multiple buffers
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: iov - pointer to iovec structure array
/// - args[2]: iovcnt - length of iovec array
///
/// # Returns
/// Returns total bytes read on success, negative error code on failure
pub fn sys_readv(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;

    // Check iovec array pointer using access_ok
    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as i64;
    }

    let mut total_read: isize = 0;
    let mut has_valid_iov = false;

    // SAFETY: iov_ptr validated with access_ok; each iov buffer validated before use.
    unsafe {
        for i in 0..iovcnt {
            let iov_ptr_i = iov_ptr.add(i);

            // Use copy_from_user to safely read iov structure
            let mut iov = Iovec { iov_base: core::ptr::null(), iov_len: 0 };
            let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );

            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;

            // Skip iov with NULL base
            if base == 0 {
                continue;
            }

            // Check each iov buffer using access_ok
            if len > 0 && crate::arch::riscv64::uaccess::access_ok(base, len) {
                has_valid_iov = true;
                let read_args = [fd as u64, iov.iov_base as u64, len as u64, 0, 0, 0];
                let result = sys_read(read_args);

                if result < 0 {
                    if total_read == 0 {
                        return result;
                    }
                    break;
                }
                total_read += result as isize;
                if result == 0 {
                    break; // EOF
                }
            } else if len > 0 {
                return -errno::EFAULT as i64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as i64;
    }

    total_read as i64
}

/// sys_dup - Duplicate file descriptor
pub fn sys_dup(args: SyscallArgs) -> i64 {
    let oldfd = args[0] as usize;

    // SAFETY: get_current_fdtable returns a valid fdtable reference for the current task.
    unsafe {
        match crate::sched::get_current_fdtable() {
            Some(fdtable) => {
                match fdtable.dup_fd(oldfd) {
                    Some(newfd) => newfd as i64,
                    None => -errno::EBADF as i64,
                }
            }
            None => -errno::EBADF as i64,
        }
    }
}

/// sys_dup2 - Duplicate file descriptor to specified number
pub fn sys_dup2(args: SyscallArgs) -> i64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;

    // SAFETY: get_current_fdtable returns a valid fdtable reference for the current task.
    unsafe {
        match crate::sched::get_current_fdtable() {
            Some(fdtable) => {
                match fdtable.dup2_fd(oldfd, newfd) {
                    Some(fd) => fd as i64,
                    None => -errno::EBADF as i64,
                }
            }
            None => -errno::EBADF as i64,
        }
    }
}

/// sys_dup3 - Duplicate file descriptor to specified number with flags
/// Syscall number: 24
pub fn sys_dup3(args: SyscallArgs) -> i64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;
    let flags = args[2] as u32;

    // dup3 returns EINVAL if oldfd == newfd (unlike dup2)
    if oldfd == newfd {
        return -errno::EINVAL as i64;
    }

    // Only O_CLOEXEC is valid for dup3
    if flags & !(crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
        return -errno::EINVAL as i64;
    }

    // SAFETY: get_current_fdtable returns a valid fdtable reference for the current task.
    unsafe {
        match crate::sched::get_current_fdtable() {
            Some(fdtable) => {
                match fdtable.dup2_fd(oldfd, newfd) {
                    Some(fd) => {
                        // Set close-on-exec if O_CLOEXEC is set
                        if (flags & crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
                            if let Some(file) = fdtable.get_file(fd) {
                                file.set_cloexec(true);
                            }
                        }
                        fd as i64
                    }
                    None => -errno::EBADF as i64,
                }
            }
            None => -errno::EBADF as i64,
        }
    }
}

/// sys_fcntl - File control
pub fn sys_fcntl(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let cmd = args[1] as usize;
    let arg = args[2] as usize;

    match crate::fs::vfs::file_fcntl(fd, cmd, arg) {
        Ok(result) => result as i64,
        Err(errno) => errno as i64,
    }
}

/// sys_ioctl - IO control
pub fn sys_ioctl(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let request = args[1] as u32;
    let arg = args[2] as usize;

    // Special handling for framebuffer device (fd >= 1000 is device file)
    if fd >= 1000 {
        let result = crate::drivers::gpu::fbdev_ioctl(request, arg) as i64;
        return result as i64;
    }

    // TTY ioctl commands
    match request {
        // TCGETS - Get terminal attributes (0x5401)
        0x5401 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity (termios struct ~60 bytes)
            if !crate::arch::riscv64::uaccess::access_ok(arg, 60) {
                return -errno::EFAULT as i64;
            }
            // Fill termios structure with current settings
            let lflag = tty_get_lflag();

            // Build termios structure in kernel buffer first
            let mut termios_buf = [0u8; 60];
            // SAFETY: termios_buf is a stack-allocated 60-byte buffer; all offsets stay within bounds.
            unsafe {
                let ptr = termios_buf.as_mut_ptr() as *mut u32;
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

            // Copy to user space with SUM bit properly set
            // SAFETY: arg validated with access_ok(60); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    termios_buf.as_ptr(),
                    60
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            0
        }
        // TCSETS, TCSETSW, TCSETSF - Set terminal attributes
        0x5402 | 0x5403 | 0x5404 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity
            if !crate::arch::riscv64::uaccess::access_ok(arg, 60) {
                return -errno::EFAULT as i64;
            }
            // Read termios structure from user space using copy_from_user
            let mut termios_buf = [0u8; 60];
            // SAFETY: arg validated with access_ok(60); copy_from_user safely reads from user.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    termios_buf.as_mut_ptr(),
                    arg as *const u8,
                    60
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            // Read c_lflag from buffer and update global state
            // SAFETY: termios_buf is a stack-allocated buffer; offset 3 reads a u32 at byte 12.
            unsafe {
                let ptr = termios_buf.as_ptr() as *const u32;
                let lflag = *ptr.offset(3);
                tty_set_lflag(lflag);
            }
            0
        }
        // TIOCGPGRP - Get foreground process group (0x540F)
        0x540F => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            if !crate::arch::riscv64::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            let pgid = TTY_FG_PGRP.load(Ordering::Relaxed);
            let pgid_bytes = (pgid as u32).to_le_bytes();
            // SAFETY: arg validated with access_ok(4); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    pgid_bytes.as_ptr(),
                    4
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            0
        }
        // TIOCSPGRP - Set foreground process group (0x5410)
        0x5410 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            if !crate::arch::riscv64::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            let mut pgid_bytes = [0u8; 4];
            // SAFETY: arg validated with access_ok(4); copy_from_user safely reads from user.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    pgid_bytes.as_mut_ptr(),
                    arg as *const u8,
                    4
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            let pgid = u32::from_le_bytes(pgid_bytes);
            TTY_FG_PGRP.store(pgid, Ordering::Release);
            0
        }
        // TIOCGWINSZ - Get window size (0x5413)
        0x5413 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity (winsize struct 8 bytes)
            if !crate::arch::riscv64::uaccess::access_ok(arg, 8) {
                return -errno::EFAULT as i64;
            }

            // Build winsize structure in kernel buffer first
            let winsize_buf: [u8; 8] = [
                25, 0,   // ws_row = 25 (little-endian)
                80, 0,   // ws_col = 80 (little-endian)
                0, 0,    // ws_xpixel
                0, 0,    // ws_ypixel
            ];

            // Copy to user space with SUM bit properly set
            // SAFETY: arg validated with access_ok(8); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    winsize_buf.as_ptr(),
                    8
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
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
                return -errno::EFAULT as i64;
            }
            // Check address validity
            if !crate::arch::riscv64::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            // Build result in kernel buffer and copy to user space
            let result_buf: [u8; 4] = [0, 0, 0, 0];  // Return 0 bytes available
            // SAFETY: arg validated with access_ok(4); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    result_buf.as_ptr(),
                    4
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
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
                -errno::ENOTTY as i64
            }
        }
    }
}

/// sys_flock - File lock (simplified implementation)
pub fn sys_flock(_args: SyscallArgs) -> i64 {
    // Simplified implementation: always return success
    0
}

/// sys_pwrite64 - Write to file descriptor at a given offset
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: buf - source buffer pointer
/// - args[2]: count - number of bytes to write
/// - args[3]: offset - file offset (signed)
///
/// # Returns
/// Number of bytes written on success, negative errno on failure
///
/// - RISC-V: 68
pub fn sys_pwrite64(args: SyscallArgs) -> i64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;
    let offset = args[3] as i64;

    // Validate offset
    if offset < 0 {
        return -errno::EINVAL as i64;
    }
    // Check buffer accessibility
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as i64;
    }
    if count == 0 {
        return 0;
    }

    // SAFETY: get_file_fd returns valid File or None; kernel_buf is fresh allocation.
    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                let saved_pos = file.get_pos();
                file.set_pos(offset as u64);

                let mut kernel_buf = alloc::vec![0u8; count];
                let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                    kernel_buf.as_mut_ptr(),
                    buf,
                    count,
                );
                if uncopied > 0 {
                    file.set_pos(saved_pos);
                    return -errno::EFAULT as i64;
                }
                let result = file.write(kernel_buf.as_ptr(), count);

                file.set_pos(saved_pos);

                if result < 0 {
                    result as i32 as i64
                } else {
                    result as i64
                }
            }
            None => -errno::EBADF as i64
        }
    }
}

/// sys_preadv - Read from file descriptor at a given offset into multiple buffers
///
/// - RISC-V: 69
pub fn sys_preadv(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;
    let offset_l = args[3] as u64;
    let offset_h = args[4] as u64;
    let offset = ((offset_h as u128) << 64) | (offset_l as u128);

    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as i64;
    }

    if offset > i64::MAX as u128 {
        return -errno::EINVAL as i64;
    }

    let mut total_read: isize = 0;
    let mut has_valid_iov = false;

    // SAFETY: iov_ptr validated with access_ok; each iov buffer validated before use.
    unsafe {
        for i in 0..iovcnt {
            let iov_ptr_i = iov_ptr.add(i);
            let mut iov = Iovec { iov_base: core::ptr::null(), iov_len: 0 };
            let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;
            if base == 0 { continue; }
            if len > 0 && crate::arch::riscv64::uaccess::access_ok(base, len) {
                has_valid_iov = true;
                let pread_args = [fd as u64, iov.iov_base as u64, len as u64, offset as u64, 0, 0];
                let result = sys_pread64(pread_args);
                if result < 0 {
                    if total_read == 0 { return result; }
                    break;
                }
                total_read += result as isize;
            } else if len > 0 {
                return -errno::EFAULT as i64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as i64;
    }

    total_read as i64
}

/// sys_pwritev - Write to file descriptor at a given offset from multiple buffers
///
/// - RISC-V: 70
pub fn sys_pwritev(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;
    let offset_l = args[3] as u64;
    let offset_h = args[4] as u64;
    let offset = ((offset_h as u128) << 64) | (offset_l as u128);

    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as i64;
    }

    if offset > i64::MAX as u128 {
        return -errno::EINVAL as i64;
    }

    let mut total_written: isize = 0;
    let mut has_valid_iov = false;

    // SAFETY: iov_ptr validated with access_ok; each iov buffer validated before use.
    unsafe {
        for i in 0..iovcnt {
            let iov_ptr_i = iov_ptr.add(i);
            let mut iov = Iovec { iov_base: core::ptr::null(), iov_len: 0 };
            let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;
            if base == 0 { continue; }
            if len > 0 && crate::arch::riscv64::uaccess::access_ok(base, len) {
                has_valid_iov = true;
                let pwrite_args = [fd as u64, iov.iov_base as u64, len as u64, offset as u64, 0, 0];
                let result = sys_pwrite64(pwrite_args);
                if result < 0 {
                    if total_written == 0 { return result; }
                    break;
                }
                total_written += result as isize;
            } else if len > 0 {
                return -errno::EFAULT as i64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as i64;
    }

    total_written as i64
}

/// sys_pipe2 - Create pipe with flags
pub fn sys_pipe2(args: SyscallArgs) -> i64 {
    let pipefd = args[0] as *mut i32;
    let flags = args[1] as u32;

    // Check pointer using access_ok
    if pipefd.is_null() {
        return -errno::EFAULT as i64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(pipefd as usize, 8) {  // 2 * sizeof(int)
        return -errno::EFAULT as i64;
    }

    // Only O_CLOEXEC and O_NONBLOCK are valid for pipe2
    const VALID_FLAGS: u32 = crate::fs::file::FileFlags::O_CLOEXEC
        | crate::fs::file::FileFlags::O_NONBLOCK;
    if flags & !VALID_FLAGS != 0 {
        return -errno::EINVAL as i64;
    }

    // Create pipe
    let (mut read_file, mut write_file) = crate::fs::pipe::create_pipe();

    // Set O_NONBLOCK on both ends if requested
    if (flags & crate::fs::file::FileFlags::O_NONBLOCK) != 0 {
        // SAFETY: read_file and write_file are freshly created Arcs that have
        // not been cloned or installed into any fd table, so get_mut succeeds.
        if let Some(rf) = alloc::sync::Arc::get_mut(&mut read_file) {
            rf.flags_mut().add_flags(crate::fs::file::FileFlags::O_NONBLOCK);
        }
        if let Some(wf) = alloc::sync::Arc::get_mut(&mut write_file) {
            wf.flags_mut().add_flags(crate::fs::file::FileFlags::O_NONBLOCK);
        }
    }

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EMFILE as i64,
    };

    // Allocate file descriptors
    let read_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as i64,
    };

    let write_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as i64,
    };

    // Install files to fdtable
    if fdtable.install_fd(read_fd, read_file.clone()).is_err() {
        return -errno::EMFILE as i64;
    }
    if fdtable.install_fd(write_fd, write_file.clone()).is_err() {
        // Close read_fd on write_fd install failure
        drop(read_file);
        fdtable.close_fd(read_fd as usize);
        return -errno::EMFILE as i64;
    }

    // Set close-on-exec if O_CLOEXEC is set
    if (flags & crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
        read_file.set_cloexec(true);
        write_file.set_cloexec(true);
    }

    // Write fd pair to userspace via copy_to_user (fault-safe)
    let fds: [i32; 2] = [read_fd as i32, write_fd as i32];
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_to_user(
            pipefd as *mut u8,
            fds.as_ptr() as *const u8,
            core::mem::size_of::<[i32; 2]>(),
        )
    };
    if uncopied != 0 {
        fdtable.close_fd(read_fd as usize);
        fdtable.close_fd(write_fd as usize);
        return -errno::EFAULT as i64;
    }
    0
}

/// sys_splice - Move data between file descriptors
///
/// # Arguments
/// - args[0]: fd_in - input file descriptor
/// - args[1]: off_in - pointer to offset (NULL = use current)
/// - args[2]: fd_out - output file descriptor
/// - args[3]: off_out - pointer to offset (NULL = use current)
/// - args[4]: len - number of bytes to transfer
/// - args[5]: flags - SPLICE_F_MOVE, SPLICE_F_NONBLOCK, etc.
pub fn sys_splice(args: SyscallArgs) -> i64 {
    let fd_in = args[0] as i32;
    let off_in = args[1] as *mut i64;
    let fd_out = args[2] as i32;
    let off_out = args[3] as *mut i64;
    let len = args[4] as usize;
    let _flags = args[5] as u32;

    if len == 0 { return 0; }

    use crate::fs::get_file_fd;
    // SAFETY: get_file_fd returns valid File or None; off_in/off_out validated with access_ok.
    unsafe {
        let in_file = match get_file_fd(fd_in as usize) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };
        let out_file = match get_file_fd(fd_out as usize) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };

        // Save positions if offset pointers provided
        if !off_in.is_null() {
            if !crate::arch::riscv64::uaccess::access_ok(off_in as usize, 8) {
                return -errno::EFAULT as i64;
            }
            in_file.set_pos(*off_in as u64);
        }
        if !off_out.is_null() {
            if !crate::arch::riscv64::uaccess::access_ok(off_out as usize, 8) {
                return -errno::EFAULT as i64;
            }
            out_file.set_pos(*off_out as u64);
        }

        // Transfer data through kernel buffer
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
                if w <= 0 { return total as i64; }
                written += w as usize;
            }
            total += written;
            remaining -= written;
        }

        // Update offset pointers
        if !off_in.is_null() { *off_in = in_file.get_pos() as i64; }
        if !off_out.is_null() { *off_out = out_file.get_pos() as i64; }

        total as i64
    }
}

/// sys_tee - Copy data between pipes
///
/// # Arguments
/// - args[0]: fd_in - input pipe fd
/// - args[1]: fd_out - output pipe fd
/// - args[2]: len - number of bytes to copy
/// - args[3]: flags - unused
pub fn sys_tee(_args: SyscallArgs) -> i64 {
    // TODO: requires pipe buffer management
    -errno::ENOSYS as i64
}

/// sys_vmsplice - Map user pages into a pipe
///
/// # Arguments
/// - args[0]: fd - pipe file descriptor
/// - args[1]: iov - pointer to iovec array
/// - args[2]: nr_segs - number of iovec entries
/// - args[3]: flags - SPLICE_F_GIFT, etc.
pub fn sys_vmsplice(_args: SyscallArgs) -> i64 {
    // TODO: requires pipe buffer and page mapping
    -errno::ENOSYS as i64
}

/// sys_sendfile - Transfer data between file descriptors
///
/// # Arguments
/// - args[0]: out_fd - output file descriptor
/// - args[1]: in_fd - input file descriptor
/// - args[2]: offset - pointer to offset (NULL = use current position)
/// - args[3]: count - number of bytes to transfer
///
/// # Returns
/// Number of bytes transferred on success, negative error code on failure
///
/// - RISC-V: 40
pub fn sys_sendfile(args: SyscallArgs) -> i64 {
    use crate::fs::get_file_fd;
    let out_fd = args[0] as usize;
    let in_fd = args[1] as usize;
    let offset_ptr = args[2] as *mut i64;
    let count = args[3] as usize;

    // Validate count
    if count == 0 {
        return 0;
    }

    // Validate offset pointer
    if !offset_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(offset_ptr as usize, core::mem::size_of::<i64>()) {
            return -errno::EFAULT as i64;
        }
    }

    // SAFETY: get_file_fd returns valid File or None; offset_ptr validated with access_ok.
    unsafe {
        let in_file = match get_file_fd(in_fd) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };
        let out_file = match get_file_fd(out_fd) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };

        // Save/restore input file position if offset is used
        let mut use_offset = false;
        let mut original_pos: i64 = 0;
        if !offset_ptr.is_null() {
            use_offset = true;
            original_pos = *offset_ptr;
            in_file.set_pos(original_pos as u64);
        }

        // Transfer data in chunks
        let mut total_transferred: usize = 0;
        let mut remaining = count;
        let chunk_size = core::cmp::min(remaining, 8192);
        let mut tmp_buf = alloc::vec![0u8; chunk_size];

        while remaining > 0 {
            let to_read = core::cmp::min(remaining, 8192);
            // Resize buffer if needed
            if tmp_buf.len() < to_read {
                tmp_buf.resize(to_read, 0);
            }

            let n_read = in_file.read(tmp_buf.as_mut_ptr(), to_read);
            if n_read <= 0 {
                break;
            }

            let mut written: usize = 0;
            while written < n_read as usize {
                let n_write = out_file.write(tmp_buf.as_ptr().add(written), (n_read as usize) - written);
                if n_write <= 0 {
                    return total_transferred as i64;
                }
                written += n_write as usize;
            }
            total_transferred += written;
            remaining -= written;
        }

        // Update offset
        if use_offset {
            let new_pos = in_file.get_pos() as i64;
            *offset_ptr = new_pos;
            in_file.set_pos(original_pos as u64);
        }

        total_transferred as i64
    }
}
