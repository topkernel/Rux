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

/// Maximum byte count for a single read/write style syscall.
/// Matches Linux MAX_RW_COUNT (INT_MAX & PAGE_MASK); larger user requests are
/// truncated instead of driving the kernel heap allocator past its size
/// (alloc failure would panic the kernel). Single definition lives in the
/// arch uaccess layer (review CONS: keep one MAX_RW_COUNT, not two).
pub use crate::arch::riscv64::uaccess::MAX_RW_COUNT;

/// Kernel staging chunk for read/write syscalls. Bounded so a huge user
/// count can never ask the allocator for more than the kernel heap holds —
/// MAX_RW_COUNT (2GB) still exceeded the 32MB heap and panicked the kernel
/// on `read(fd, buf, 0x4000_0000)` (review SYSA-C1 fix in Wave 2R).
pub const RW_CHUNK: usize = 64 * 1024;

/// Clamp a user-supplied transfer length to MAX_RW_COUNT.
#[inline]
pub fn clamp_rw_count(count: usize) -> usize {
    if count > MAX_RW_COUNT { MAX_RW_COUNT } else { count }
}

// ============================================================================
// Terminal (TTY) state
// ============================================================================

/// Termios local flags ( c_lflag)
const L_ISIG: u32   = 0x0001;   // Signal handling enabled
const L_ICANON: u32 = 0x0002;   // Canonical mode (R7-D7: was 0x0100, which
                                // is TOSTOP in the asm-generic ABI — libc
                                // canonical-mode checks misfired)
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

/// Get the console terminal's foreground process group (0 = none set).
/// Used by the tty ISIG (^C/^Z/^\) delivery path.
pub fn tty_get_fg_pgrp() -> u32 {
    TTY_FG_PGRP.load(Ordering::Acquire)
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
    let count = clamp_rw_count(args[2] as usize);

    // Check if buffer address is in valid user space using access_ok
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, count) {
        return -errno::EFAULT as i64;
    }

    // Check if count is reasonable
    if count == 0 {
        return 0;
    }

    // SAFETY: get_file_fd returns valid File or None; kernel_buf is a fresh allocation.
    let ret = unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                // Chunked read (SYSA-C1): stage at most RW_CHUNK at a time so
                // a huge count can never OOM the kernel heap. A short chunk
                // (EOF / pipe drained) ends the loop and returns what we
                // have — pipe reads still return as soon as data exists
                // (pipe capacity 16KB < RW_CHUNK).
                let mut kernel_buf = alloc::vec![0u8; count.min(RW_CHUNK)];
                let mut total: usize = 0;
                let mut user_ptr = buf;
                let mut remaining = count;
                loop {
                    let chunk = remaining.min(RW_CHUNK);
                    if chunk == 0 {
                        break;
                    }
                    let result = file.read(kernel_buf.as_mut_ptr(), chunk);
                    if result < 0 {
                        return if total > 0 { total as i64 } else { result as i32 as i64 };
                    }
                    if result == 0 {
                        break;
                    }
                    let n = result as usize;
                    // SAFETY: user_ptr stays within the access_ok-validated
                    // [buf, buf+count) window; exception-table copy.
                    let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                        user_ptr,
                        kernel_buf.as_ptr(),
                        n,
                    );
                    if uncopied > 0 {
                        return if total > 0 { total as i64 } else { -errno::EFAULT as i64 };
                    }
                    total += n;
                    remaining -= n;
                    user_ptr = user_ptr.add(n);
                    if n < chunk {
                        break; // short read: EOF or nothing more available now
                    }
                }
                total as i64
            }
            None => -errno::EBADF as i64
        }
    };
    ret
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
    let count = clamp_rw_count(args[2] as usize);
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
                // RACE fix (review批次1): pread must not touch the shared fd
                // position — the old get_pos/set_pos/restore dance corrupted
                // concurrent reads on other threads sharing the fd. Use the
                // position-invariant read_at() instead.
                // Chunked (SYSA-C1): bounded staging buffer, sequential
                // offset advance; short read ends the loop.
                let mut kernel_buf = alloc::vec![0u8; count.min(RW_CHUNK)];
                let mut total: usize = 0;
                let mut err: i32 = 0;
                let mut user_ptr = buf;
                let mut remaining = count;
                let mut cur_off = offset as u64;
                loop {
                    let chunk = remaining.min(RW_CHUNK);
                    if chunk == 0 {
                        break;
                    }
                    let result = file.read_at(cur_off, kernel_buf.as_mut_ptr(), chunk);
                    if result <= 0 {
                        if result < 0 && total == 0 {
                            err = result as i32;
                        }
                        break;
                    }
                    let n = result as usize;
                    // SAFETY: within the validated [buf, buf+count) window.
                    let uncopied = crate::arch::riscv64::uaccess::copy_to_user(
                        user_ptr,
                        kernel_buf.as_ptr(),
                        n,
                    );
                    if uncopied > 0 {
                        if total == 0 {
                            err = -errno::EFAULT as i32;
                        }
                        break;
                    }
                    total += n;
                    remaining -= n;
                    user_ptr = user_ptr.add(n);
                    cur_off += n as u64;
                    if n < chunk {
                        break;
                    }
                }

                if total > 0 {
                    total as i64
                } else {
                    err as i64
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
    let count = clamp_rw_count(args[2] as usize);

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
                // Chunked write (SYSA-C1): bounded staging buffer so a huge
                // count cannot OOM the kernel heap; a partial chunk write
                // ends the loop and returns what was accepted (POSIX
                // partial-write semantics).
                let mut kernel_buf = alloc::vec![0u8; count.min(RW_CHUNK)];
                let mut total: usize = 0;
                let mut user_ptr = buf;
                let mut remaining = count;
                let mut first_err: i32 = 0;
                loop {
                    let chunk = remaining.min(RW_CHUNK);
                    if chunk == 0 {
                        break;
                    }
                    let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                        kernel_buf.as_mut_ptr(),
                        user_ptr,
                        chunk,
                    );
                    if uncopied > 0 {
                        if total == 0 {
                            return -errno::EFAULT as i64;
                        }
                        break;
                    }
                    let result = file.write(kernel_buf.as_ptr(), chunk);
                    if result <= 0 {
                        if result < 0 && total == 0 {
                            first_err = result as i32;
                        }
                        break;
                    }
                    let n = result as usize;
                    total += n;
                    remaining -= n;
                    user_ptr = user_ptr.add(n);
                    if n < chunk {
                        break; // short write: destination full / non-blocking
                    }
                }
                if total > 0 {
                    total as i64
                } else {
                    first_err as i64
                }
            }
            None => -errno::EBADF as i64,
        }
    }
}

/// IOV_MAX — Linux's limit on the number of iovec entries per call.
const IOV_MAX: usize = 1024;

/// Validate an iovec array header shared by readv/writev/preadv/pwritev:
/// IOV_MAX bound, overflow-safe size computation, and user-range check.
/// Returns Ok(size) of the array or the (negative errno) error.
fn check_iov_array(iov_ptr: *const Iovec, iovcnt: usize) -> Result<usize, i64> {
    if iovcnt == 0 {
        return Ok(0);
    }
    if iovcnt > IOV_MAX {
        return Err(-(errno::EINVAL as i64));
    }
    let iov_size = core::mem::size_of::<Iovec>()
        .checked_mul(iovcnt)
        .ok_or(-(errno::EINVAL as i64))?;
    if !crate::arch::riscv64::uaccess::access_ok(iov_ptr as usize, iov_size) {
        return Err(-(errno::EFAULT as i64));
    }
    Ok(iov_size)
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

    // IOV_MAX + overflow-safe iovec array validation (review批次1 OVERFLOW)
    if let Err(e) = check_iov_array(iov_ptr, iovcnt) {
        return e;
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

    // IOV_MAX + overflow-safe iovec array validation (review批次1 OVERFLOW)
    if let Err(e) = check_iov_array(iov_ptr, iovcnt) {
        return e;
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
                // Short read: for pipes this means the buffered data was
                // exhausted — return the batch now instead of blocking on
                // the next iov (Linux readv never blocks after a partial
                // fill; review批次1 "readv/writev pipe 一次拿可用").
                if (result as usize) < len {
                    break;
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
                        // dup3 O_CLOEXEC applies to the NEW descriptor only
                        if (flags & crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
                            fdtable.set_fd_cloexec(fd, true);
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

    // R22-2: dispatch framebuffer ioctls on the FILE's identity, not the
    // fd number — FdTable legally allocates 1000-1023, and the old
    // heuristic hijacked regular ioctls on high fds into fbdev.
    if fd >= 1000 {
        let is_fbdev = unsafe { crate::fs::file::get_file_fd(fd as usize) }
            .map(|file| {
                let p = file.path();
                p.starts_with("/dev/fb") || p.starts_with("/dev/fb0")
            })
            .unwrap_or(false);
        if is_fbdev {
            let result = crate::drivers::gpu::fbdev_ioctl(request, arg) as i64;
            return result as i64;
        }
        // fall through to the generic path
    }

    // TTY ioctl commands
    match request {
        // TCGETS - Get terminal attributes (0x5401)
        0x5401 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity (termios struct ~60 bytes)
            if !crate::arch::riscv64::uaccess::access_ok(arg, 52) {
                return -errno::EFAULT as i64;
            }
            // Fill termios structure with current settings
            let lflag = tty_get_lflag();

            // Build termios structure in kernel buffer first
            let mut termios_buf = [0u8; 52]; // R9-15: asm-generic termios is 52 bytes (4x u32 + c_line + c_cc[32] + pad); 60 overwrote 8 bytes past the user struct
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
                // R7-D7: musl (asm-generic) layout is c_line: 1 BYTE at 16,
                // c_cc[32] at 17 — the old u32-at-16/c_cc-at-20 shifted
                // every control character 3 slots (VINTR's 3 landed in the
                // VKILL position).
                *termios_buf.as_mut_ptr().add(16) = 0; // c_line (cc_t = u8)
                let cc_ptr = termios_buf.as_mut_ptr().add(17);
                cc_ptr.add(0).write(3);   // VINTR = ^C
                cc_ptr.add(1).write(28);  // VQUIT = ^\
                cc_ptr.add(2).write(127); // VERASE = DEL
                cc_ptr.add(3).write(21);  // VKILL = ^U
                cc_ptr.add(4).write(4);   // VEOF = ^D
                cc_ptr.add(5).write(0);   // VTIME
                cc_ptr.add(6).write(1);   // VMIN
            }

            // Copy to user space with SUM bit properly set
            // SAFETY: arg validated with access_ok(60); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    arg as *mut u8,
                    termios_buf.as_ptr(),
                    52
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
            if !crate::arch::riscv64::uaccess::access_ok(arg, 52) {
                return -errno::EFAULT as i64;
            }
            // Read termios structure from user space using copy_from_user
            let mut termios_buf = [0u8; 52]; // R9-15: asm-generic termios is 52 bytes (4x u32 + c_line + c_cc[32] + pad); 60 overwrote 8 bytes past the user struct
            // R20-1: copy exactly 52 — the old 60-byte length overflowed the
            // 52-byte kernel buffer by 8 bytes (stale length from before the
            // R9-15 buffer shrink; TCGETS was fixed, TCSETS was not).
            // SAFETY: arg validated with access_ok(52); copy_from_user safely reads from user.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    termios_buf.as_mut_ptr(),
                    arg as *const u8,
                    52
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
            // Real readable count (review批次1: previously hard-coded 0):
            // pipes report their buffered bytes; regular files report
            // size - position; other objects report 0.
            let readable: i32 = unsafe {
                crate::fs::file::get_file_fd(fd as usize)
                    .map(|file| {
                        if let Some(n) = crate::fs::pipe::pipe_fionread(&file) {
                            n as i32
                        } else if let Some(inode) = (&*file.inode.get()).as_ref() {
                            let size = inode.get_size();
                            let pos = file.get_pos();
                            if size > pos { (size - pos) as i32 } else { 0 }
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0)
            };
            let result_buf: [u8; 4] = readable.to_le_bytes();
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
        // FIONBIO - Set/clear O_NONBLOCK (0x5421). Previously swallowed by
        // the generic 0x5400 fallback as fake success — the flag never
        // reached the File, so non-blocking pipes/ttys blocked anyway.
        0x5421 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            if !crate::arch::riscv64::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            let mut flag_buf = [0u8; 4];
            // SAFETY: arg validated with access_ok(4); copy_from_user handles user reads.
            let uncopied = unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    flag_buf.as_mut_ptr(),
                    arg as *const u8,
                    4
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            let on = i32::from_le_bytes(flag_buf) != 0;
            // SAFETY: get_file_fd returns valid File or None.
            let file = unsafe { crate::fs::file::get_file_fd(fd as usize) };
            match file {
                Some(file) => {
                    use crate::fs::file::FileFlags;
                    let mut bits = file.flags_bits();
                    if on {
                        bits |= FileFlags::O_NONBLOCK;
                    } else {
                        bits &= !FileFlags::O_NONBLOCK;
                    }
                    file.set_flags(FileFlags::new(bits));
                    0
                }
                None => -errno::EBADF as i64,
            }
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

/// sys_flock - File lock (BSD semantics, real implementation — P0-5)
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: operation - LOCK_SH/LOCK_EX/LOCK_UN [| LOCK_NB]
///
/// # Returns
/// 0 on success, negative errno on failure
///
/// - RISC-V: 32
///
/// Locks are owned by the open file description (dup'd fds share them,
/// fork copies keep them alive, a second open() of the same path gets an
/// independent, CONFLICTING lock). Released automatically when the last
/// fd of the description closes. Conflicting requests block unless
/// LOCK_NB, in which case -EWOULDBLOCK (-EAGAIN) is returned.
pub fn sys_flock(args: SyscallArgs) -> i64 {
    use crate::fs::file::get_file_fd;

    let fd = args[0] as usize;
    let operation = args[1] as i32;

    let file = match unsafe { get_file_fd(fd) } {
        Some(f) => f,
        None => return -errno::EBADF as i64,
    };

    match crate::fs::locks::flock_lock(&file, operation) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
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
    let count = clamp_rw_count(args[2] as usize);
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
                // RACE fix (review批次1): use the position-invariant
                // write_at() — the old set_pos/restore dance corrupted the
                // shared fd position under concurrent I/O.
                // Chunked (SYSA-C1): bounded staging, sequential offset
                // advance; partial chunk write ends the loop.
                let mut kernel_buf = alloc::vec![0u8; count.min(RW_CHUNK)];
                let mut total: usize = 0;
                let mut err: i32 = 0;
                let mut user_ptr = buf;
                let mut remaining = count;
                let mut cur_off = offset as u64;
                loop {
                    let chunk = remaining.min(RW_CHUNK);
                    if chunk == 0 {
                        break;
                    }
                    let uncopied = crate::arch::riscv64::uaccess::copy_from_user(
                        kernel_buf.as_mut_ptr(),
                        user_ptr,
                        chunk,
                    );
                    if uncopied > 0 {
                        if total == 0 {
                            err = -errno::EFAULT as i32;
                        }
                        break;
                    }
                    let result = file.write_at(cur_off, kernel_buf.as_ptr(), chunk);
                    if result <= 0 {
                        if result < 0 && total == 0 {
                            err = result as i32;
                        }
                        break;
                    }
                    let n = result as usize;
                    total += n;
                    remaining -= n;
                    user_ptr = user_ptr.add(n);
                    cur_off += n as u64;
                    if n < chunk {
                        break;
                    }
                }

                if total > 0 {
                    total as i64
                } else {
                    err as i64
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
    // On riscv64 the offset is a single 64-bit register (arg[3]).
    // arg[4] is unused garbage — do NOT combine it into a 128-bit offset.
    let offset = args[3] as u64 as u128;

    // IOV_MAX + overflow-safe iovec array validation (review批次1 OVERFLOW)
    if let Err(e) = check_iov_array(iov_ptr, iovcnt) {
        return e;
    }

    if offset > i64::MAX as u128 {
        return -errno::EINVAL as i64;
    }

    let mut total_read: isize = 0;
    let mut has_valid_iov = false;
    // Each successive iov reads from the advancing offset (previously every
    // iov re-read the SAME offset — preadv(N iovs) returned N copies).
    let mut cur_off: u64 = offset as u64;

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
                let pread_args = [fd as u64, iov.iov_base as u64, len as u64, cur_off, 0, 0];
                let result = sys_pread64(pread_args);
                if result < 0 {
                    if total_read == 0 { return result; }
                    break;
                }
                cur_off = cur_off.saturating_add(result as u64);
                total_read += result as isize;
                // Short read ends the vector (EOF / pipe drained).
                if (result as usize) < len {
                    break;
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

/// sys_pwritev - Write to file descriptor at a given offset from multiple buffers
///
/// - RISC-V: 70
pub fn sys_pwritev(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const Iovec;
    let iovcnt = args[2] as usize;
    // On riscv64 the offset is a single 64-bit register (arg[3]).
    // arg[4] is unused garbage — do NOT combine it into a 128-bit offset.
    let offset = args[3] as u64 as u128;

    // IOV_MAX + overflow-safe iovec array validation (review批次1 OVERFLOW)
    if let Err(e) = check_iov_array(iov_ptr, iovcnt) {
        return e;
    }

    if offset > i64::MAX as u128 {
        return -errno::EINVAL as i64;
    }

    let mut total_written: isize = 0;
    let mut has_valid_iov = false;
    // Successive iovs write at the advancing offset (previously every iov
    // wrote over the SAME offset).
    let mut cur_off: u64 = offset as u64;

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
                let pwrite_args = [fd as u64, iov.iov_base as u64, len as u64, cur_off, 0, 0];
                let result = sys_pwrite64(pwrite_args);
                if result < 0 {
                    if total_written == 0 { return result; }
                    break;
                }
                cur_off = cur_off.saturating_add(result as u64);
                total_written += result as isize;
                // Short write ends the vector.
                if (result as usize) < len {
                    break;
                }
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
        // Use atomic OR to set O_NONBLOCK — no exclusive access needed.
        use core::sync::atomic::Ordering;
        read_file.flags.fetch_or(crate::fs::file::FileFlags::O_NONBLOCK, Ordering::Release);
        write_file.flags.fetch_or(crate::fs::file::FileFlags::O_NONBLOCK, Ordering::Release);
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

    // Set close-on-exec if O_CLOEXEC is set (per-descriptor)
    if (flags & crate::fs::file::FileFlags::O_CLOEXEC) != 0 {
        fdtable.set_fd_cloexec(read_fd, true);
        fdtable.set_fd_cloexec(write_fd, true);
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

    // Read the caller-provided offsets through the exception-table copy
    // path — bare dereferences of user pointers fault the kernel (review批次1).
    // A non-NULL offset drives position-invariant I/O and must NOT disturb
    // the fd's own position (Linux semantics).
    let mut in_off: Option<u64> = None;
    if !off_in.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(off_in as usize, 8) {
            return -errno::EFAULT as i64;
        }
        let mut v: i64 = 0;
        // SAFETY: off_in validated with access_ok(8); copies 8 bytes to a stack i64.
        if unsafe { crate::arch::riscv64::uaccess::copy_from_user(
            &mut v as *mut i64 as *mut u8, off_in as *const u8, 8) } > 0 {
            return -errno::EFAULT as i64;
        }
        if v < 0 { return -errno::EINVAL as i64; }
        in_off = Some(v as u64);
    }
    let mut out_off: Option<u64> = None;
    if !off_out.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(off_out as usize, 8) {
            return -errno::EFAULT as i64;
        }
        let mut v: i64 = 0;
        // SAFETY: off_out validated with access_ok(8); copies 8 bytes to a stack i64.
        if unsafe { crate::arch::riscv64::uaccess::copy_from_user(
            &mut v as *mut i64 as *mut u8, off_out as *const u8, 8) } > 0 {
            return -errno::EFAULT as i64;
        }
        if v < 0 { return -errno::EINVAL as i64; }
        out_off = Some(v as u64);
    }

    // SAFETY: get_file_fd returns valid File or None; offsets already read into kernel memory.
    unsafe {
        let in_file = match get_file_fd(fd_in as usize) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };
        let out_file = match get_file_fd(fd_out as usize) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };

        // Transfer data through kernel buffer
        let mut total = 0usize;
        let mut remaining = len;
        while remaining > 0 {
            let chunk = core::cmp::min(remaining, 8192);
            let mut buf = alloc::vec![0u8; chunk];
            let n = match in_off {
                Some(off) => in_file.read_at(off, buf.as_mut_ptr(), chunk),
                None => in_file.read(buf.as_mut_ptr(), chunk),
            };
            if n <= 0 { break; }
            let n = n as usize;
            let mut written = 0usize;
            while written < n {
                let w = match out_off {
                    Some(off) => out_file.write_at(off + written as u64, buf.as_ptr().add(written), n - written),
                    None => out_file.write(buf.as_ptr().add(written), n - written),
                };
                if w <= 0 {
                    // Publish the offsets consumed so far before bailing out.
                    if total + written > 0 { break; }
                    return total as i64;
                }
                written += w as usize;
            }
            in_off = in_off.map(|o| o + n as u64);
            out_off = out_off.map(|o| o + written as u64);
            total += written;
            remaining -= written;
            if written < n {
                break;
            }
        }

        // Write the updated offsets back through copy_to_user.
        if !off_in.is_null() {
            if let Some(off) = in_off {
                let v = off as i64;
                // SAFETY: off_in validated with access_ok(8) above.
                if crate::arch::riscv64::uaccess::copy_to_user(
                    off_in as *mut u8, &v as *const i64 as *const u8, 8) > 0 {
                    return -errno::EFAULT as i64;
                }
            }
        }
        if !off_out.is_null() {
            if let Some(off) = out_off {
                let v = off as i64;
                // SAFETY: off_out validated with access_ok(8) above.
                if crate::arch::riscv64::uaccess::copy_to_user(
                    off_out as *mut u8, &v as *const i64 as *const u8, 8) > 0 {
                    return -errno::EFAULT as i64;
                }
            }
        }

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

    // Validate offset pointer and read its value through the exception-table
    // copy path (review批次1: bare dereference of a user pointer).
    let mut saved_offset: i64 = 0;
    if !offset_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(offset_ptr as usize, core::mem::size_of::<i64>()) {
            return -errno::EFAULT as i64;
        }
        // SAFETY: offset_ptr validated with access_ok(8); copies into a stack i64.
        if unsafe { crate::arch::riscv64::uaccess::copy_from_user(
            &mut saved_offset as *mut i64 as *mut u8,
            offset_ptr as *const u8,
            core::mem::size_of::<i64>(),
        ) } > 0 {
            return -errno::EFAULT as i64;
        }
        if saved_offset < 0 {
            return -errno::EINVAL as i64;
        }
    }

    // SAFETY: get_file_fd returns valid File or None; offset already read into kernel memory.
    unsafe {
        let in_file = match get_file_fd(in_fd) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };
        let out_file = match get_file_fd(out_fd) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };

        // With a non-NULL offset, sendfile(2) reads from that offset and
        // leaves the fd's own position untouched (the old code set_pos'd the
        // shared position mid-transfer and only restored it on success).
        let use_offset = !offset_ptr.is_null();
        let mut cur_off = saved_offset as u64;

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

            let n_read = if use_offset {
                in_file.read_at(cur_off, tmp_buf.as_mut_ptr(), to_read)
            } else {
                in_file.read(tmp_buf.as_mut_ptr(), to_read)
            };
            if n_read <= 0 {
                break;
            }
            let n_read = n_read as usize;

            let mut written: usize = 0;
            while written < n_read {
                let n_write = out_file.write(tmp_buf.as_ptr().add(written), n_read - written);
                if n_write <= 0 {
                    // Publish progress through the caller's offset before bailing.
                    if use_offset && total_transferred + written > 0 {
                        let v = (cur_off + written as u64) as i64;
                        // SAFETY: offset_ptr validated with access_ok(8) above.
                        crate::arch::riscv64::uaccess::copy_to_user(
                            offset_ptr as *mut u8,
                            &v as *const i64 as *const u8,
                            core::mem::size_of::<i64>(),
                        );
                    }
                    return total_transferred as i64;
                }
                written += n_write as usize;
            }
            cur_off += n_read as u64;
            total_transferred += written;
            remaining -= written;
        }

        // Update offset through copy_to_user (fd position untouched)
        if use_offset {
            let v = cur_off as i64;
            // SAFETY: offset_ptr validated with access_ok(8) above.
            if crate::arch::riscv64::uaccess::copy_to_user(
                offset_ptr as *mut u8,
                &v as *const i64 as *const u8,
                core::mem::size_of::<i64>(),
            ) > 0 {
                return -errno::EFAULT as i64;
            }
        }

        total_transferred as i64
    }
}
