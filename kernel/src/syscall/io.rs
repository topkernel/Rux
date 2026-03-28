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
        return -errno::EFAULT as u64;
    }

    // Check if count is reasonable
    if count == 0 {
        return 0;
    }

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
                        return -errno::EFAULT as u64;
                    }
                    result as u64
                } else {
                    result as u32 as u64
                }
            }
            None => -errno::EBADF as u64
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
pub fn sys_pread64(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;
    let offset = args[3] as i64;

    // Validate offset
    if offset < 0 {
        return -errno::EINVAL as u64;
    }
    // Check buffer accessibility
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as u64;
    }
    if count == 0 {
        return 0;
    }

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
                        return -errno::EFAULT as u64;
                    }
                    result as u64
                } else {
                    result as u32 as u64
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
                                return -errno::EFAULT as u64;
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

                    return total_written as u64;
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
                    return -errno::EFAULT as u64;
                }
                let result = file.write(kernel_buf.as_ptr(), count);
                if result < 0 {
                    result as u32 as u64
                } else {
                    result as u64
                }
            }
            None => -errno::EBADF as u64,
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
            let iov_ptr_i = iov_ptr.add(i);

            // Use copy_from_user to safely read iov structure
            let mut iov = Iovec { iov_base: core::ptr::null(), iov_len: 0 };
            let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );

            if uncopied > 0 {
                return -errno::EFAULT as u64;
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

                let result_i64 = result as i64;
                if result_i64 < 0 {
                    if total_written == 0 {
                        return result;
                    }
                    break;
                }
                total_written += result as isize;
            } else if len > 0 {
                return -errno::EFAULT as u64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as u64;
    }

    total_written as u64
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
pub fn sys_readv(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;

    // Check iovec array pointer using access_ok
    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as u64;
    }

    let mut total_read: isize = 0;
    let mut has_valid_iov = false;

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
                return -errno::EFAULT as u64;
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

                let result_i64 = result as i64;
                if result_i64 < 0 {
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
                return -errno::EFAULT as u64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as u64;
    }

    total_read as u64
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

/// sys_dup3 - Duplicate file descriptor to specified number with flags
/// Linux syscall number: 24 (riscv64)
pub fn sys_dup3(args: SyscallArgs) -> u64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;
    let flags = args[2] as u32;

    // dup3 returns EINVAL if oldfd == newfd (unlike dup2)
    if oldfd == newfd {
        return -errno::EINVAL as u64;
    }

    // Only O_CLOEXEC is valid for dup3
    if flags & !(crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
        return -errno::EINVAL as u64;
    }

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
                        fd as u64
                    }
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

            // Build termios structure in kernel buffer first
            let mut termios_buf = [0u8; 60];
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
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    termios_buf.as_ptr(),
                    60
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as u64;
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
            // Read termios structure from user space using copy_from_user
            let mut termios_buf = [0u8; 60];
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    termios_buf.as_mut_ptr(),
                    arg as *const u8,
                    60
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as u64;
            }
            // Read c_lflag from buffer and update global state
            unsafe {
                let ptr = termios_buf.as_ptr() as *const u32;
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

            // Build winsize structure in kernel buffer first
            let winsize_buf: [u8; 8] = [
                25, 0,   // ws_row = 25 (little-endian)
                80, 0,   // ws_col = 80 (little-endian)
                0, 0,    // ws_xpixel
                0, 0,    // ws_ypixel
            ];

            // Copy to user space with SUM bit properly set
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    winsize_buf.as_ptr(),
                    8
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as u64;
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
            // Build result in kernel buffer and copy to user space
            let result_buf: [u8; 4] = [0, 0, 0, 0];  // Return 0 bytes available
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    result_buf.as_ptr(),
                    4
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as u64;
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
pub fn sys_pwrite64(args: SyscallArgs) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;
    let offset = args[3] as i64;

    // Validate offset
    if offset < 0 {
        return -errno::EINVAL as u64;
    }
    // Check buffer accessibility
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as u64;
    }
    if count == 0 {
        return 0;
    }

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
                    return -errno::EFAULT as u64;
                }
                let result = file.write(kernel_buf.as_ptr(), count);

                file.set_pos(saved_pos);

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

/// sys_preadv - Read from file descriptor at a given offset into multiple buffers
///
/// - RISC-V: 69
pub fn sys_preadv(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;
    let offset_l = args[3] as u64;
    let offset_h = args[4] as u64;
    let offset = ((offset_h as u128) << 64) | (offset_l as u128);

    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as u64;
    }

    if offset > i64::MAX as u128 {
        return -errno::EINVAL as u64;
    }

    let mut total_read: isize = 0;
    let mut has_valid_iov = false;

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
                return -errno::EFAULT as u64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;
            if base == 0 { continue; }
            if len > 0 && crate::arch::riscv64::uaccess::access_ok(base, len) {
                has_valid_iov = true;
                let pread_args = [fd as u64, iov.iov_base as u64, len as u64, offset as u64, 0, 0];
                let result = sys_pread64(pread_args);
                let result_i64 = result as i64;
                if result_i64 < 0 {
                    if total_read == 0 { return result; }
                    break;
                }
                total_read += result as isize;
            } else if len > 0 {
                return -errno::EFAULT as u64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as u64;
    }

    total_read as u64
}

/// sys_pwritev - Write to file descriptor at a given offset from multiple buffers
///
/// - RISC-V: 70
pub fn sys_pwritev(args: SyscallArgs) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;
    let offset_l = args[3] as u64;
    let offset_h = args[4] as u64;
    let offset = ((offset_h as u128) << 64) | (offset_l as u128);

    let iov_size = core::mem::size_of::<Iovec>() * iovcnt;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return -errno::EFAULT as u64;
    }

    if offset > i64::MAX as u128 {
        return -errno::EINVAL as u64;
    }

    let mut total_written: isize = 0;
    let mut has_valid_iov = false;

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
                return -errno::EFAULT as u64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;
            if base == 0 { continue; }
            if len > 0 && crate::arch::riscv64::uaccess::access_ok(base, len) {
                has_valid_iov = true;
                let pwrite_args = [fd as u64, iov.iov_base as u64, len as u64, offset as u64, 0, 0];
                let result = sys_pwrite64(pwrite_args);
                let result_i64 = result as i64;
                if result_i64 < 0 {
                    if total_written == 0 { return result; }
                    break;
                }
                total_written += result as isize;
            } else if len > 0 {
                return -errno::EFAULT as u64;
            }
        }
    }

    if !has_valid_iov && iovcnt > 0 {
        return -errno::EFAULT as u64;
    }

    total_written as u64
}

/// sys_pipe2 - Create pipe with flags
pub fn sys_pipe2(args: SyscallArgs) -> u64 {
    let pipefd = args[0] as *mut i32;
    let flags = args[1] as u32;

    // Check pointer using access_ok
    if pipefd.is_null() {
        return -errno::EFAULT as u64;
    }

    if !crate::arch::riscv64::uaccess::access_ok(pipefd as usize, 8) {  // 2 * sizeof(int)
        return -errno::EFAULT as u64;
    }

    // Only O_CLOEXEC and O_NONBLOCK are valid for pipe2
    const VALID_FLAGS: u32 = crate::fs::file::FileFlags::O_CLOEXEC
        | crate::fs::file::FileFlags::O_NONBLOCK;
    if flags & !VALID_FLAGS != 0 {
        return -errno::EINVAL as u64;
    }

    // Create pipe
    let (read_file, write_file) = crate::fs::pipe::create_pipe();

    // Set O_NONBLOCK on both ends if requested
    if (flags & crate::fs::file::FileFlags::O_NONBLOCK) != 0 {
        unsafe {
            let flags_ptr = &read_file.flags as *const _ as *mut crate::fs::file::FileFlags;
            (*flags_ptr).add_flags(crate::fs::file::FileFlags::O_NONBLOCK);
            let flags_ptr = &write_file.flags as *const _ as *mut crate::fs::file::FileFlags;
            (*flags_ptr).add_flags(crate::fs::file::FileFlags::O_NONBLOCK);
        }
    }

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
    if fdtable.install_fd(read_fd, read_file.clone()).is_err() {
        return -errno::EMFILE as u64;
    }
    if fdtable.install_fd(write_fd, write_file.clone()).is_err() {
        return -errno::EMFILE as u64;
    }

    // Set close-on-exec if O_CLOEXEC is set
    if (flags & crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
        read_file.set_cloexec(true);
        write_file.set_cloexec(true);
    }

    unsafe {
        *pipefd = read_fd as i32;
        *pipefd.offset(1) = write_fd as i32;
    }
    0
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
pub fn sys_sendfile(args: SyscallArgs) -> u64 {
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
            return -errno::EFAULT as u64;
        }
    }

    unsafe {
        // Get file objects
        let in_file = match get_file_fd(in_fd) {
            Some(f) => f,
            None => return -errno::EBADF as u64,
        };
        let out_file = match get_file_fd(out_fd) {
            Some(f) => f,
            None => return -errno::EBADF as u64,
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
                    return total_transferred as u64;
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

        total_transferred as u64
    }
}
