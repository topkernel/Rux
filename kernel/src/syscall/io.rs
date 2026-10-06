//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IO-related system calls
//!
//! Includes: read, write,writev, dup, dup2, fcntl, ioctl, flock, pipe2

use super::*;
use core::sync::atomic::Ordering;

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
pub use crate::arch::uaccess::MAX_RW_COUNT;

/// Kernel staging chunk for read/write syscalls. Bounded so a huge user
/// count can never ask the allocator for more than the kernel heap holds —
/// MAX_RW_COUNT (2GB) still exceeded the 32MB heap and panicked the kernel
/// on `read(fd, buf, 0x4000_0000)` (review SYSA-C1 fix in Wave 2R).
pub const RW_CHUNK: usize = 64 * 1024;

/// Allocate an UNINITIALIZED staging buffer for read/write chunking.
///
/// The buffer is always fully overwritten before it is read (file layers
/// fill exactly the bytes they return; copy_from_user fills before write
/// dispatch), so the previous `vec![0u8; n]` paid a useless memset per
/// syscall — 4KB+ of zeroing on every read(2)/write(2) hot path (file,
/// pipe, tty) that showed up directly in throughput under TCG.
#[inline]
fn alloc_uninit_buf(n: usize) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(n);
    // SAFETY: capacity is exactly n, so set_len is in bounds. The buffer is
    // filled by the caller before any read of its contents (see doc).
    unsafe { v.set_len(n); }
    v
}

/// Clamp a user-supplied transfer length to MAX_RW_COUNT.
#[inline]
pub fn clamp_rw_count(count: usize) -> usize {
    if count > MAX_RW_COUNT { MAX_RW_COUNT } else { count }
}

// ============================================================================
// Terminal (TTY) state
// ============================================================================
//
// The console terminal's termios/winsize/fg-pgrp state lives in
// fs::tty::console() (a TtyDevice shared with the pty layer). The helpers
// below are thin delegates kept for the console.rs RX-IRQ path (ISIG/echo
// decisions) and legacy callers; pty fds carry their own per-pair TtyDevice.

/// Check if the console terminal echo is enabled
pub fn tty_echo_enabled() -> bool {
    crate::fs::tty::console().echo_enabled()
}

/// Get the console terminal c_lflag
pub fn tty_get_lflag() -> u32 {
    crate::fs::tty::console().lflag()
}

/// Get the console terminal's foreground process group (0 = none set).
/// Used by the tty ISIG (^C/^Z/^\) delivery path.
pub fn tty_get_fg_pgrp() -> u32 {
    crate::fs::tty::console().fg_pgrp.load(Ordering::Acquire)
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
    if !crate::arch::uaccess::access_ok(buf as usize, count) {
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
                // Linux: reading from an O_WRONLY-only descriptor fails with
                // EBADF (the fd is "not open for reading") — LTP read03.
                let mode = file.flags.load(core::sync::atomic::Ordering::Relaxed)
                    & crate::fs::file::FileFlags::O_ACCMODE;
                if mode == crate::fs::file::FileFlags::O_WRONLY {
                    return -errno::EBADF as i64;
                }
                // Pipe fast path: transfer directly user↔ring. The generic
                // staging below costs an alloc + 2 copies per op; on pipes
                // that was ~half the per-syscall time under TCG.
                if crate::fs::pipe::is_pipe_file(&file) {
                    return crate::fs::pipe::pipe_read_user(&file, buf, count) as i64;
                }
                // ext4 fast path: page cache → user memory without the
                // staging buffer (O_DIRECT keeps the generic staged path —
                // it serves straight from the block layer).
                if file.flags_bits() & crate::fs::file::FileFlags::O_DIRECT == 0
                    && crate::fs::ext4::file::is_ext4_file(&file)
                {
                    return crate::fs::ext4::file::ext4_file_read_user_vfs(&file, buf, count) as i64;
                }
                // Chunked read (SYSA-C1): stage at most RW_CHUNK at a time so
                // a huge count can never OOM the kernel heap. A short chunk
                // (EOF / pipe drained) ends the loop and returns what we
                // have — pipe reads still return as soon as data exists
                // (pipe capacity 16KB < RW_CHUNK).
                //
                // UNINIT staging: the file layer fills exactly the `n`
                // bytes it returns before copy_to_user reads them, so
                // zero-filling the buffer was a 4KB memset per read(2) —
                // measurable on every hot path (file, pipe, tty).
                let mut kernel_buf = alloc_uninit_buf(count.min(RW_CHUNK));
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
                    let uncopied = crate::arch::uaccess::copy_to_user(
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
    if !crate::arch::uaccess::access_ok(buf as usize, count) {
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
                let mut kernel_buf = alloc_uninit_buf(count.min(RW_CHUNK));
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
                    let uncopied = crate::arch::uaccess::copy_to_user(
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
    if !crate::arch::uaccess::access_ok(buf as usize, count) {
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
                // Linux: writing to a read-only (O_RDONLY) descriptor fails
                // with EBADF (the fd is "not open for writing") — LTP
                // write03/write04.
                let mode = file.flags.load(core::sync::atomic::Ordering::Relaxed)
                    & crate::fs::file::FileFlags::O_ACCMODE;
                if mode == crate::fs::file::FileFlags::O_RDONLY {
                    return -errno::EBADF as i64;
                }
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
                    let uart_addr = crate::arch::mm::fixmap::uart_virt_addr() as *mut u8;

                    while remaining > 0 {
                        let to_copy = core::cmp::min(remaining, CHUNK_SIZE);

                        let uncopied = crate::arch::uaccess::copy_from_user(
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
                // Pipe fast path: transfer directly user↔ring (see the
                // matching branch in sys_read).
                if crate::fs::pipe::is_pipe_file(&file) {
                    return crate::fs::pipe::pipe_write_user(&file, buf, count) as i64;
                }
                // Chunked write (SYSA-C1): bounded staging buffer so a huge
                // count cannot OOM the kernel heap; a partial chunk write
                // ends the loop and returns what was accepted (POSIX
                // partial-write semantics).
                let mut kernel_buf = alloc_uninit_buf(count.min(RW_CHUNK));
                let mut total: usize = 0;
                let mut user_ptr = buf;
                let mut remaining = count;
                let mut first_err: i32 = 0;
                loop {
                    let chunk = remaining.min(RW_CHUNK);
                    if chunk == 0 {
                        break;
                    }
                    let uncopied = crate::arch::uaccess::copy_from_user(
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

/// Per-entry iovec sanity shared by the vector syscalls: a length whose
/// signed view is negative is EINVAL (Linux rw_verify_area/import_iovec)
/// — checking access_ok on it first misclassified the case as EFAULT
/// (LTP readv02/writev01 feed iov_len = -1 and expect EINVAL).
#[inline]
fn iov_len_sanity(len: usize) -> Result<(), i64> {
    if (len as isize) < 0 {
        return Err(-(errno::EINVAL as i64));
    }
    Ok(())
}

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
    if !crate::arch::uaccess::access_ok(iov_ptr as usize, iov_size) {
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
            let uncopied = crate::arch::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );

            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;

            if let Err(e) = iov_len_sanity(len) {
                return e;
            }

            // Skip iov with NULL base
            if base == 0 {
                continue;
            }

            // Check each iov buffer using access_ok
            if len > 0 && crate::arch::uaccess::access_ok(base, len) {
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
            let uncopied = crate::arch::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );

            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;

            if let Err(e) = iov_len_sanity(len) {
                return e;
            }

            // Skip iov with NULL base
            if base == 0 {
                continue;
            }

            // Check each iov buffer using access_ok
            if len > 0 && crate::arch::uaccess::access_ok(base, len) {
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
            Some(fdtable) => match fdtable.dup_fd_strict(oldfd) {
                Ok(newfd) => newfd as i64,
                Err(e) => e as i64,
            },
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

    // Linux fdget() gate: an ioctl on an fd that is not open fails with
    // EBADF before the request is even looked at (LTP sockioctl01 uses
    // fd=1025 with SIOCATMARK and expects EBADF, not ENOTTY).
    if fd < 0 || unsafe { crate::fs::file::get_file_fd(fd as usize) }.is_none() {
        return -errno::EBADF as i64;
    }

    // Framebuffer ioctls dispatch on the FILE's ops identity (R22-2
    // spirit, minus the fd>=1000 heuristic that only worked for the
    // side-namespace fd range).
    if fd >= 0 {
        if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            if crate::drivers::gpu::fbdev::is_fb_file(&file) {
                return crate::drivers::gpu::fbdev_ioctl(request, arg) as i64;
            }
        }
    }

    // Per-fd evdev ioctls (EVIOCG*/EVIOCGBIT/EVIOCGRAB on
    // /dev/input/eventX): dispatch on the File's ops identity. Without
    // this, input-capability queries (EVIOCGBIT above all) failed with
    // ENOTTY and every input stack (xf86-input-evdev/libinput/evtest)
    // aborted its device probe.
    // SAFETY: get_file_fd returns a valid Arc<File> or None.
    if fd >= 0 {
        if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            if let Some(ret) = crate::drivers::input::evdev::evdev_file_ioctl(&file, request, arg)
            {
                return ret;
            }
        }
    }

    // Per-fd terminal ioctls (pty master/slave): dispatch on the File's ops
    // identity. Non-pty files (or pty-unhandled requests like FIONBIO)
    // return None here and fall through to the console-global handling
    // below — the terminal state must follow the fd, not the system.
    // SAFETY: get_file_fd returns a valid Arc<File> or None.
    if fd >= 0 {
        if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            if let Some(ret) = crate::fs::pty::pty_ioctl(&file, request, arg) {
                return ret;
            }
        }
    }

    // Loop-family ioctls (LOOP_SET_FD/LOOP_CLR_FD/LOOP_GET_STATUS64/
    // LOOP_CTL_GET_FREE on /dev/loopN and /dev/loop-control): dispatch on
    // the File's ops identity (losetup / LTP tst_acquire_device).
    // SAFETY: get_file_fd returns a valid Arc<File> or None.
    if fd >= 0 {
        if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            if let Some(ret) = crate::drivers::loop_dev::loop_file_ioctl(&file, request, arg) {
                return ret;
            }
        }
    }

    // FS_IOC_GETFLAGS / FS_IOC_SETFLAGS (chattr flags on regular files
    // and directories). LTP unlink09 marks files IMMUTABLE/APPEND-only
    // and expects unlink(2) to fail EPERM; before this the ioctls
    // answered ENOTTY and the test TBROK'd in setup. Non-regular,
    // non-directory fds keep ENOTTY (Linux file_ioctl gate).
    if request == 0x8008_6601 || request == 0x4008_6602 {
        const FS_IOC_GETFLAGS: u32 = 0x8008_6601;
        const FS_IOC_SETFLAGS: u32 = 0x4008_6602;
        use crate::fs::inode::{
            FS_APPEND_FL, FS_FL_USER_MODIFIABLE, FS_FL_USER_VISIBLE, FS_IMMUTABLE_FL,
        };
        // SAFETY: get_file_fd returns a valid Arc<File> or None; the fd
        // existence was already validated above.
        let Some(file) = (unsafe { crate::fs::file::get_file_fd(fd as usize) }) else {
            return -errno::EBADF as i64;
        };
        // SAFETY: inode cell written once at open time; read-only here.
        let inode_opt = unsafe { &*file.inode.get() };
        let Some(inode) = inode_opt.as_ref() else {
            return -errno::ENOTTY as i64;
        };
        if !inode.mode.is_regular_file() && !inode.mode.is_directory() {
            return -errno::ENOTTY as i64;
        }
        if arg == 0 || !crate::arch::uaccess::access_ok(arg, 4) {
            return -errno::EFAULT as i64;
        }
        if request == FS_IOC_GETFLAGS {
            let flags =
                inode.ioc_flags.load(core::sync::atomic::Ordering::Acquire) & FS_FL_USER_VISIBLE;
            // SAFETY: arg validated non-null, 4-byte writable.
            if !unsafe {
                crate::arch::uaccess::put_user(
                    arg as *mut u32,
                    flags,
                )
            } {
                return -errno::EFAULT as i64;
            }
            return 0;
        }
        // FS_IOC_SETFLAGS
        // SAFETY: arg validated non-null, 4-byte readable.
        let Some(new_flags) = (unsafe {
            crate::arch::uaccess::get_user(arg as *const u32)
        }) else {
            return -errno::EFAULT as i64;
        };
        if new_flags & !FS_FL_USER_MODIFIABLE != 0 {
            return -errno::EOPNOTSUPP as i64;
        }
        // Owner or CAP_FOWNER (Linux ioctl_setflags).
        let owner_ok = crate::sched::current().map(|t| {
            t.cred().fsuid == inode.uid.load(core::sync::atomic::Ordering::Acquire)
                || crate::security::capable(crate::security::CAP_FOWNER)
        }).unwrap_or(false);
        if !owner_ok {
            return -errno::EACCES as i64;
        }
        let old = inode.ioc_flags.load(core::sync::atomic::Ordering::Acquire);
        if (new_flags ^ old) & (FS_IMMUTABLE_FL | FS_APPEND_FL) != 0
            && !crate::security::capable(crate::security::CAP_LINUX_IMMUTABLE)
        {
            return -errno::EPERM as i64;
        }
        let merged = (old & !FS_FL_USER_MODIFIABLE) | (new_flags & FS_FL_USER_MODIFIABLE);
        inode.ioc_flags.store(merged, core::sync::atomic::Ordering::Release);
        // ext4: persist into the on-disk i_flags so the flags survive an
        // icache eviction (the VFS copy is reseeded from disk at iget).
        if core::ptr::eq(
            inode.ops.unwrap_or(&crate::fs::ext4::EXT4_INODE_OPS) as *const _,
            &crate::fs::ext4::EXT4_INODE_OPS as *const _,
        ) {
            let _guard = crate::fs::ext4::EXT4_BIG_LOCK.lock_fair();
            if let Some(fs_ptr) = inode.private_data {
                let fs = fs_ptr as *const crate::fs::ext4::Ext4FileSystem;
                // SAFETY: private_data holds the Ext4FileSystem for
                // EXT4_INODE_OPS inodes.
                unsafe {
                    if let Ok(mut on_disk) =
                        crate::fs::ext4::inode::read_inode(&*fs, inode.ino as u32)
                    {
                        on_disk.i_flags = (on_disk.i_flags & !FS_FL_USER_MODIFIABLE)
                            | (merged & FS_FL_USER_MODIFIABLE);
                        if crate::fs::ext4::inode::write_inode_disk(
                            &*fs,
                            inode.ino as u32,
                            &on_disk,
                        )
                        .is_ok()
                        {
                            // Keep the VFS-cached Ext4Inode copy in sync.
                            if let Some(sb) = inode.sb {
                                let cached = &mut *(sb as *mut crate::fs::ext4::inode::Ext4Inode);
                                cached.flags = on_disk.i_flags;
                            }
                        }
                    }
                }
            }
        }
        return 0;
    }

    // P0-2: interface-management ioctls (SIOCGIFCONF / SIOCGIFADDR /
    // SIOCSIFADDR / SIOCGIFFLAGS / SIOCSIFFLAGS / SIOCGIFHWADDR / ...) —
    // forwarded to the network layer when the fd is a socket (any family).
    if crate::net::netlink::is_if_ioctl(request) {
        // Linux checks the fd BEFORE the request: an ioctl on a nonexistent
        // fd is EBADF, not ENOTTY (LTP sockioctl01 "bad file descriptor").
        let file_opt = if fd >= 0 {
            unsafe { crate::fs::file::get_file_fd(fd as usize) }
        } else {
            None
        };
        if file_opt.is_none() {
            return -errno::EBADF as i64;
        }
        let is_socket = file_opt
            .map(|file| {
                let ops = file.get_ops();
                match ops {
                    Some(ops) => {
                        core::ptr::eq(ops as *const _, &crate::net::socket::SOCKET_OPS as *const _)
                            || core::ptr::eq(
                                ops as *const _,
                                &crate::net::unix::UNIX_SOCKET_OPS as *const _,
                            )
                            || core::ptr::eq(
                                ops as *const _,
                                &crate::net::netlink::NETLINK_OPS as *const _,
                            )
                            || core::ptr::eq(
                                ops as *const _,
                                &crate::net::raw::RAW_OPS as *const _,
                            )
                    }
                    None => false,
                }
            })
            .unwrap_or(false);
        if is_socket {
            return crate::net::netlink::net_if_ioctl(request, arg);
        }
        return -errno::ENOTTY as i64;
    }

    // SIOCATMARK (0x8905) — socket-only, and Linux only wires it for
    // stream sockets: a UDP socket gets ENOTTY (LTP sockioctl01 "ATMARK on
    // UDP"), a non-socket fd gets ENOTTY ("not a socket"), and a bad
    // result pointer is EFAULT ("invalid option buffer" — the arg is an
    // int* written with the at-OOB-mark flag; we never generate urgent
    // data, so the answer is always 0).
    if request == 0x8905 {
        let file_opt = if fd >= 0 {
            unsafe { crate::fs::file::get_file_fd(fd as usize) }
        } else {
            None
        };
        let is_socket = file_opt
            .as_ref()
            .map(|file| {
                let ops = file.get_ops();
                match ops {
                    Some(ops) => {
                        core::ptr::eq(ops as *const _, &crate::net::socket::SOCKET_OPS as *const _)
                            || core::ptr::eq(
                                ops as *const _,
                                &crate::net::unix::UNIX_SOCKET_OPS as *const _,
                            )
                    }
                    None => false,
                }
            })
            .unwrap_or(false);
        if !is_socket {
            return -errno::ENOTTY as i64;
        }
        // TCP socket: report "not at mark". The unix-socket case falls
        // through with 0 too (Linux treats SIOCATMARK as a generic sock
        // op; AF_UNIX has no urgent data either).
        if let Some(file) = file_opt.as_ref() {
            let is_stream = file
                .get_ops()
                .map(|ops| {
                    core::ptr::eq(ops as *const _, &crate::net::socket::SOCKET_OPS as *const _)
                })
                .and_then(|is_inet| {
                    if !is_inet {
                        return Some(true); // AF_UNIX: no datagram distinction
                    }
                    // SAFETY: private_data holds an Arc<Socket> for
                    // SOCKET_OPS files, installed at socket creation.
                    let ptr = unsafe { *file.private_data.get() }? as *const crate::net::socket::Socket;
                    Some(unsafe { (*ptr).sock_type == crate::net::socket::SocketType::Tcp })
                })
                .unwrap_or(true);
            if !is_stream {
                return -errno::ENOTTY as i64;
            }
        }
        if arg == 0 || !crate::arch::uaccess::access_ok(arg, 4) {
            return -errno::EFAULT as i64;
        }
        // SAFETY: arg validated non-null with access_ok(4).
        if !unsafe { crate::arch::uaccess::put_user(arg as *mut i32, 0i32) } {
            return -errno::EFAULT as i64;
        }
        return 0;
    }

    // TTY ioctl commands — but only on fds that ARE terminals.
    //
    // This whole block used to run the CONSOLE-GLOBAL tty state for any fd,
    // with no per-fd type check: a PIPE fd answered TCGETS (isatty!) and
    // TIOCGPGRP with success. An interactive shell wired to pipes then took
    // the job-control path forever — observed with dash under fbterm
    // (stdin/stdout/stderr = pipes): isatty(0)=true -> setjobctl ->
    // TIOCGPGRP returned the console's foreign fg_pgrp, kill(-pgrp, SIGTTIN)
    // returned ESRCH, and dash spun in the tcsetpgrp retry loop at 100%
    // CPU, freezing the screen on the banner and starving everything else
    // (syscall trace: kill/ioctl(0x540F)/getpid x N forever).
    // FIONREAD (0x541B) and FIONBIO (0x5421) work on any fd on Linux and
    // stay ungated.
    if matches!(request,
        0x5401..=0x540F | 0x5410 | 0x5413 | 0x5414)
        || (request & 0xFF00) == 0x5400 && !matches!(request, 0x541B | 0x5421)
    {
        match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            None => return -errno::EBADF as i64,
            Some(file) => {
                if !crate::fs::tty::file_is_tty(&file) {
                    return -errno::ENOTTY as i64;
                }
            }
        }
    }

    match request {
        // TCGETS - Get terminal attributes (0x5401)
        0x5401 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity (kernel-ABI termios = 36 bytes)
            if !crate::arch::uaccess::access_ok(arg, crate::fs::tty::TERMIOS_KERNEL_SIZE) {
                return -errno::EFAULT as i64;
            }
            // Fill termios structure from the shared console tty state
            // (fs::tty — full termios persistence; the c_lflag half is what
            // console.rs consults for ISIG/echo).
            let tio = crate::fs::tty::console().get_termios();
            // Kernel-ABI termios = 36 bytes (c_cc[19]); glibc's tcgetattr
            // stack buffer is exactly that size — the old 52-byte copy
            // smashed its canary (the ls/grep/dpkg abort family).
            let mut termios_buf = [0u8; crate::fs::tty::TERMIOS_KERNEL_SIZE];
            crate::fs::tty::termios_to_kernel_bytes(&tio, &mut termios_buf);

            // Copy to user space with SUM bit properly set
            // SAFETY: arg validated with access_ok(36); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::uaccess::copy_to_user(
                    arg as *mut u8,
                    termios_buf.as_ptr(),
                    crate::fs::tty::TERMIOS_KERNEL_SIZE
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
            // Check address validity (kernel-ABI termios = 36 bytes)
            if !crate::arch::uaccess::access_ok(arg, crate::fs::tty::TERMIOS_KERNEL_SIZE) {
                return -errno::EFAULT as i64;
            }
            // Read termios structure from user space using copy_from_user
            // (kernel-ABI 36-byte layout, c_cc[19]).
            // SAFETY: arg validated with access_ok(36); copy_from_user safely reads from user.
            let mut termios_buf = [0u8; crate::fs::tty::TERMIOS_KERNEL_SIZE];
            let uncopied = unsafe {
                crate::arch::uaccess::copy_from_user(
                    termios_buf.as_mut_ptr(),
                    arg as *const u8,
                    crate::fs::tty::TERMIOS_KERNEL_SIZE
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            // Store the full termios into the shared console tty state
            // (console.rs reads the c_lflag half for ISIG/echo decisions).
            let tio = crate::fs::tty::termios_from_kernel_bytes(&termios_buf);
            if request == 0x5404 {
                crate::fs::tty::console().flush_input();
            }
            crate::fs::tty::console().set_termios(tio);
            0
        }
        // TCGETA / TCSETA / TCSETAW / TCSETAF - termio (BSD-style) variants
        0x5405 | 0x5406 | 0x5407 | 0x5408 => {
            let set = request != 0x5405;
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            if !crate::arch::uaccess::access_ok(arg, 17) {
                return -errno::EFAULT as i64;
            }
            let console_tty = crate::fs::tty::console();
            let mut kbuf = [0u8; 17]; // struct termio: 4x u16 + c_line + c_cc[8]
            if set {
                // SAFETY: arg validated with access_ok(17); copy_from_user safely reads from user.
                if unsafe {
                    crate::arch::uaccess::copy_from_user(
                        kbuf.as_mut_ptr(),
                        arg as *const u8,
                        17
                    )
                } > 0 {
                    return -errno::EFAULT as i64;
                }
                let base = console_tty.get_termios();
                let tio = crate::fs::tty::termios_from_termio_bytes(&kbuf, &base);
                if request == 0x5408 {
                    console_tty.flush_input();
                }
                console_tty.set_termios(tio);
            } else {
                let tio = console_tty.get_termios();
                crate::fs::tty::termios_to_termio_bytes(&tio, &mut kbuf);
                // SAFETY: arg validated with access_ok(17); copy_to_user handles user writes.
                if unsafe {
                    crate::arch::uaccess::copy_to_user(
                        arg as *mut u8,
                        kbuf.as_ptr(),
                        17
                    )
                } > 0 {
                    return -errno::EFAULT as i64;
                }
            }
            0
        }
        // TIOCGPGRP - Get foreground process group (0x540F)
        0x540F => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            if !crate::arch::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            let pgid = crate::fs::tty::console().fg_pgrp.load(Ordering::Acquire);
            // No foreground pgrp set on the console: report the caller's
            // own process group (Linux sets fg_pgrp when the session
            // leader acquires the ctty on open; without a ctty model this
            // is the same answer for the first shell). Returning a
            // foreign/zero pgid or ENOTTY makes shell job-control loops
            // spin — mrsh 4c81598 (init shell) burned 100% CPU forever
            // comparing tcgetpgrp to its own pgrp.
            let pgid = if pgid == 0 {
                crate::process::current_pgid()
            } else {
                pgid
            };
            let pgid_bytes = (pgid as u32).to_le_bytes();
            // SAFETY: arg validated with access_ok(4); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::uaccess::copy_to_user(
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
            if !crate::arch::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            let mut pgid_bytes = [0u8; 4];
            // SAFETY: arg validated with access_ok(4); copy_from_user safely reads from user.
            let uncopied = unsafe {
                crate::arch::uaccess::copy_from_user(
                    pgid_bytes.as_mut_ptr(),
                    arg as *const u8,
                    4
                )
            };
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }
            let pgid = u32::from_le_bytes(pgid_bytes);
            crate::fs::tty::console()
                .fg_pgrp
                .store(pgid, Ordering::Release);
            0
        }
        // TIOCGWINSZ - Get window size (0x5413)
        0x5413 => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity (winsize struct 8 bytes)
            if !crate::arch::uaccess::access_ok(arg, 8) {
                return -errno::EFAULT as i64;
            }

            // Read the shared console tty window size (default 25x80)
            let winsize_buf = crate::fs::tty::console().get_winsize().to_le_bytes();

            // Copy to user space with SUM bit properly set
            // SAFETY: arg validated with access_ok(8); copy_to_user handles user writes.
            let uncopied = unsafe {
                crate::arch::uaccess::copy_to_user(
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
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            if !crate::arch::uaccess::access_ok(arg, 8) {
                return -errno::EFAULT as i64;
            }
            let mut kbuf = [0u8; 8];
            // SAFETY: arg validated with access_ok(8); copy_from_user safely reads from user.
            if unsafe {
                crate::arch::uaccess::copy_from_user(
                    kbuf.as_mut_ptr(),
                    arg as *const u8,
                    8
                )
            } > 0 {
                return -errno::EFAULT as i64;
            }
            let ws = crate::fs::tty::WinSize::from_le_bytes(&kbuf);
            let console_tty = crate::fs::tty::console();
            if console_tty.set_winsize(ws) {
                console_tty.send_sigwinch();
            }
            0
        }
        // FIONREAD - Get readable byte count (0x541B)
        0x541B => {
            if arg == 0 {
                return -errno::EFAULT as i64;
            }
            // Check address validity
            if !crate::arch::uaccess::access_ok(arg, 4) {
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
                crate::arch::uaccess::copy_to_user(
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
            if !crate::arch::uaccess::access_ok(arg, 4) {
                return -errno::EFAULT as i64;
            }
            let mut flag_buf = [0u8; 4];
            // SAFETY: arg validated with access_ok(4); copy_from_user handles user reads.
            let uncopied = unsafe {
                crate::arch::uaccess::copy_from_user(
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
        // Unhandled 'T' (tty) commands: ENOTTY, exactly like Linux for an
        // ioctl the driver does not implement. The old blanket success
        // here was load-bearing in the worst way: glibc's openpty() takes
        // the TIOCGPTPEER result as an fd NUMBER, so "0" handed it a
        // phantom slave at fd 0 (really the console), and every shell
        // spawned via openpty wired stdio to the wrong file — bash hung
        // silently, dash spun in its tcsetpgrp retry loop. Callers probe
        // ioctls and expect ENOTTY for unsupported ones; fake success
        // corrupts anything that interprets the return value.
        _ if (request & 0xFF00) == 0x5400 => -errno::ENOTTY as i64,
        // Other commands: ENOTTY for every fd — including stdio. Same
        // reasoning: a probing caller must see "not supported", never a
        // fabricated success for an ioctl that did nothing.
        _ => -errno::ENOTTY as i64,
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
    if !crate::arch::uaccess::access_ok(buf as usize, count) {
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
                let mut kernel_buf = alloc_uninit_buf(count.min(RW_CHUNK));
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
                    let uncopied = crate::arch::uaccess::copy_from_user(
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
            let uncopied = crate::arch::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;
            if let Err(e) = iov_len_sanity(len) { return e; }
            if base == 0 { continue; }
            if len > 0 && crate::arch::uaccess::access_ok(base, len) {
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
            let uncopied = crate::arch::uaccess::copy_from_user(
                &mut iov as *mut Iovec as *mut u8,
                iov_ptr_i as *const u8,
                core::mem::size_of::<Iovec>()
            );
            if uncopied > 0 {
                return -errno::EFAULT as i64;
            }

            let base = iov.iov_base as usize;
            let len = iov.iov_len;
            if let Err(e) = iov_len_sanity(len) { return e; }
            if base == 0 { continue; }
            if len > 0 && crate::arch::uaccess::access_ok(base, len) {
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

    if !crate::arch::uaccess::access_ok(pipefd as usize, 8) {  // 2 * sizeof(int)
        return -errno::EFAULT as i64;
    }

    // O_CLOEXEC, O_NONBLOCK and O_DIRECT are valid for pipe2. O_DIRECT
    // (kernel 3.4+) applies to the WRITE end only — it marks packets for
    // future buffer-flag use; Linux rejects anything else with EINVAL.
    const VALID_FLAGS: u32 = crate::fs::file::FileFlags::O_CLOEXEC
        | crate::fs::file::FileFlags::O_NONBLOCK
        | crate::fs::file::FileFlags::O_DIRECT;
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
    // O_DIRECT on the write end (LTP pipe2_01 checks F_GETFL shows it).
    if (flags & crate::fs::file::FileFlags::O_DIRECT) != 0 {
        use core::sync::atomic::Ordering;
        write_file.flags.fetch_or(crate::fs::file::FileFlags::O_DIRECT, Ordering::Release);
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
        crate::arch::uaccess::copy_to_user(
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
        if !crate::arch::uaccess::access_ok(off_in as usize, 8) {
            return -errno::EFAULT as i64;
        }
        let mut v: i64 = 0;
        // SAFETY: off_in validated with access_ok(8); copies 8 bytes to a stack i64.
        if unsafe { crate::arch::uaccess::copy_from_user(
            &mut v as *mut i64 as *mut u8, off_in as *const u8, 8) } > 0 {
            return -errno::EFAULT as i64;
        }
        if v < 0 { return -errno::EINVAL as i64; }
        in_off = Some(v as u64);
    }
    let mut out_off: Option<u64> = None;
    if !off_out.is_null() {
        if !crate::arch::uaccess::access_ok(off_out as usize, 8) {
            return -errno::EFAULT as i64;
        }
        let mut v: i64 = 0;
        // SAFETY: off_out validated with access_ok(8); copies 8 bytes to a stack i64.
        if unsafe { crate::arch::uaccess::copy_from_user(
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

        let ipipe = crate::fs::pipe::pipe_of_file(&in_file);
        let opipe = crate::fs::pipe::pipe_of_file(&out_file);

        // Linux do_splice(): one of the two ends must be a pipe.
        if ipipe.is_some() && opipe.is_some() {
            // pipe → pipe
            if !off_in.is_null() || !off_out.is_null() {
                return -errno::ESPIPE as i64;
            }
            if core::ptr::eq(ipipe.unwrap() as *const _, opipe.unwrap() as *const _) {
                return -errno::EINVAL as i64; // splicing to self
            }
            let mut total = 0usize;
            while total < len {
                let chunk = core::cmp::min(len - total, 4096);
                let mut buf = alloc::vec![0u8; chunk];
                // Consume from the input pipe (File::read carries the
                // blocking/EOF semantics of pipe_file_read).
                let n = in_file.read(buf.as_mut_ptr(), chunk);
                if n <= 0 {
                    if n < 0 && total == 0 { return n as i64; }
                    break;
                }
                // Publish ALL of it into the output pipe (pipe_file_write
                // blocks until written; EPIPE propagates).
                let mut w = 0usize;
                while w < n as usize {
                    let r = out_file.write(buf.as_ptr().add(w), n as usize - w);
                    if r <= 0 {
                        if r < 0 && total == 0 && w == 0 { return r as i64; }
                        // Reader died mid-transfer: report what moved.
                        return writeback_and_return(
                            off_in, in_off, off_out, out_off, total as i64,
                        );
                    }
                    w += r as usize;
                }
                total += n as usize;
            }
            return writeback_and_return(off_in, in_off, off_out, out_off, total as i64);
        }

        if let Some(_pipe) = ipipe {
            // pipe → file (Linux ipipe branch).
            if !off_in.is_null() {
                return -errno::ESPIPE as i64;
            }
            if in_file.flags().is_writeonly() {
                return -errno::EBADF as i64; // wrong-direction pipe end
            }
            if out_file.flags().is_readonly() {
                // O_RDONLY (and O_PATH-style no-access) targets cannot take
                // spliced output (Linux FMODE_WRITE gate → EBADF).
                return -errno::EBADF as i64;
            }
            if out_file.flags().bits() & crate::fs::file::FileFlags::O_APPEND != 0 {
                return -errno::EINVAL as i64; // O_APPEND out (Linux)
            }
            let mut total = 0usize;
            while total < len {
                let chunk = core::cmp::min(len - total, 8192);
                let mut buf = alloc::vec![0u8; chunk];
                let n = in_file.read(buf.as_mut_ptr(), chunk);
                if n <= 0 {
                    if n < 0 && total == 0 { return n as i64; }
                    break; // EOF
                }
                let n = n as usize;
                let mut written = 0usize;
                while written < n {
                    let w = match out_off {
                        Some(off) => out_file.write_at(off + total as u64 + written as u64,
                                                       buf.as_ptr().add(written), n - written),
                        None => out_file.write(buf.as_ptr().add(written), n - written),
                    };
                    if w <= 0 {
                        if total + written > 0 {
                            return writeback_and_return(
                                off_in, in_off, off_out,
                                out_off.map(|o| o + total as u64 + written as u64),
                                (total + written) as i64);
                        }
                        return w as i64;
                    }
                    written += w as usize;
                }
                total += written;
            }
            return writeback_and_return(
                off_in, in_off, off_out,
                out_off.map(|o| o + total as u64),
                total as i64,
            );
        }

        if let Some(_pipe) = opipe {
            // file → pipe (Linux opipe branch).
            if !off_out.is_null() {
                return -errno::ESPIPE as i64;
            }
            if in_file.flags().is_writeonly() {
                return -errno::EBADF as i64; // fd_in not readable
            }
            if out_file.flags().is_readonly() {
                return -errno::EBADF as i64; // writing to the read end
            }
            // Linux rejects splice sources that cannot feed an iter read:
            // - O_PATH descriptors carry no read mode (EBADF)
            // - directories (EBADF — LTP splice07/08 expect {EBADF,EINVAL})
            // - sockets: af_unix has sock_no_splice_read (EINVAL); blocking
            //   on an empty socket would hang the fd-matrix sweep instead
            const O_PATH_RISCV: u32 = 0x200000;
            if in_file.flags().bits() & O_PATH_RISCV != 0 {
                return -errno::EBADF as i64;
            }
            if let Some(inode) = (*in_file.inode.get()).as_ref() {
                if inode.mode.is_directory() {
                    return -errno::EBADF as i64;
                }
            }
            if let Some(ops) = in_file.get_ops() {
                if core::ptr::eq(ops as *const _, &crate::net::socket::SOCKET_OPS as *const _)
                    || core::ptr::eq(ops as *const _, &crate::net::unix::UNIX_SOCKET_OPS as *const _)
                    || core::ptr::eq(ops as *const _, &crate::net::netlink::NETLINK_OPS as *const _)
                    || core::ptr::eq(ops as *const _, &crate::net::raw::RAW_OPS as *const _)
                {
                    return -errno::EINVAL as i64; // sock_no_splice_read
                }
            }
            let mut total = 0usize;
            while total < len {
                let chunk = core::cmp::min(len - total, 8192);
                let mut buf = alloc::vec![0u8; chunk];
                let n = match in_off {
                    Some(off) => in_file.read_at(off, buf.as_mut_ptr(), chunk),
                    None => in_file.read(buf.as_mut_ptr(), chunk),
                };
                if n <= 0 {
                    if n < 0 && total == 0 { return n as i64; }
                    break; // EOF on source
                }
                let n = n as usize;
                let mut written = 0usize;
                while written < n {
                    let w = out_file.write(buf.as_ptr().add(written), n - written);
                    if w <= 0 {
                        if total + written > 0 {
                            return writeback_and_return(
                                off_in, in_off, off_out, out_off,
                                (total + written) as i64);
                        }
                        return w as i64;
                    }
                    written += w as usize;
                }
                in_off = in_off.map(|o| o + n as u64);
                total += written;
            }
            return writeback_and_return(off_in, in_off, off_out, out_off, total as i64);
        }

        // Neither end is a pipe.
        -errno::EINVAL as i64
    }
}

/// Publish the consumed offsets (splice off_in/off_out contract) and return
/// the byte count. SAFETY: both pointers were access_ok(8)-validated by the
/// caller before any copy.
unsafe fn writeback_and_return(
    off_in: *mut i64,
    in_off: Option<u64>,
    off_out: *mut i64,
    out_off: Option<u64>,
    ret: i64,
) -> i64 {
    if !off_in.is_null() {
        if let Some(off) = in_off {
            let v = off as i64;
            if crate::arch::uaccess::copy_to_user(
                off_in as *mut u8, &v as *const i64 as *const u8, 8) > 0 {
                return -errno::EFAULT as i64;
            }
        }
    }
    if !off_out.is_null() {
        if let Some(off) = out_off {
            let v = off as i64;
            if crate::arch::uaccess::copy_to_user(
                off_out as *mut u8, &v as *const i64 as *const u8, 8) > 0 {
                return -errno::EFAULT as i64;
            }
        }
    }
    ret
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
/// Linux semantics (LTP vmsplice01..04):
/// - EBADF: fd not open, or does not refer to a pipe
/// - EINVAL: unknown flags, or nr_segs > IOV_MAX (1024)
/// - write end: fill available pipe space with user bytes, return the
///   (possibly short) count; block while the pipe is FULL (EAGAIN with
///   SPLICE_F_NONBLOCK)
/// - read end: drain pipe bytes into the user iovs
///
/// # Arguments
/// - args[0]: fd - pipe file descriptor
/// - args[1]: iov - pointer to iovec array
/// - args[2]: nr_segs - number of iovec entries
/// - args[3]: flags - SPLICE_F_MOVE/NONBLOCK/MORE/GIFT
pub fn sys_vmsplice(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let iov_ptr = args[1] as *const u8;
    let nr_segs = args[2] as usize;
    let flags = args[3] as u32;

    const SPLICE_F_MOVE: u32 = 0x1;
    const SPLICE_F_NONBLOCK: u32 = 0x2;
    const SPLICE_F_MORE: u32 = 0x4;
    const SPLICE_F_GIFT: u32 = 0x8;
    if flags & !(SPLICE_F_MOVE | SPLICE_F_NONBLOCK | SPLICE_F_MORE | SPLICE_F_GIFT) != 0 {
        return -errno::EINVAL as i64;
    }
    if nr_segs == 0 {
        return 0;
    }
    const IOV_MAX: usize = 1024;
    if nr_segs > IOV_MAX {
        return -errno::EINVAL as i64;
    }
    if iov_ptr.is_null() {
        return -errno::EFAULT as i64;
    }
    if !crate::arch::uaccess::access_ok(iov_ptr as usize, nr_segs * 16) {
        return -errno::EFAULT as i64;
    }

    // Copy the iovec array in (16 bytes per entry on LP64).
    let mut raw = alloc::vec![0u8; nr_segs * 16];
    // SAFETY: iov_ptr validated with access_ok above; exception-table copy.
    if unsafe {
        crate::arch::uaccess::copy_from_user(raw.as_mut_ptr(), iov_ptr, nr_segs * 16)
    } != 0
    {
        return -errno::EFAULT as i64;
    }
    let mut iovs: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::with_capacity(nr_segs);
    for i in 0..nr_segs {
        let base = u64::from_le_bytes(raw[i * 16..i * 16 + 8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(raw[i * 16 + 8..i * 16 + 16].try_into().unwrap()) as usize;
        if len == 0 {
            continue;
        }
        if !crate::arch::uaccess::access_ok(base, len.min(4096)) {
            return -errno::EFAULT as i64;
        }
        iovs.push((base, len));
    }
    if iovs.is_empty() {
        return 0;
    }

    // fd must be a pipe (Linux get_pipe_info failure → EBADF).
    // SAFETY: get_file_fd returns a valid Arc<File> or None.
    let file = match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -errno::EBADF as i64,
    };
    if crate::fs::pipe::pipe_of_file(&file).is_none() {
        return -errno::EBADF as i64;
    }

    let nonblock = flags & SPLICE_F_NONBLOCK != 0;
    // Direction from the fd's access mode (Linux: FMODE_WRITE → to pipe).
    let f = file.flags();
    let ret = if !f.is_readonly() {
        crate::fs::pipe::pipe_vmsplice_to(&file, &iovs, nonblock)
    } else if !f.is_writeonly() {
        crate::fs::pipe::pipe_vmsplice_from(&file, &iovs, nonblock)
    } else {
        return -errno::EBADF as i64;
    };
    ret as i64
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
        if !crate::arch::uaccess::access_ok(offset_ptr as usize, core::mem::size_of::<i64>()) {
            return -errno::EFAULT as i64;
        }
        // SAFETY: offset_ptr validated with access_ok(8); copies into a stack i64.
        if unsafe { crate::arch::uaccess::copy_from_user(
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
                        crate::arch::uaccess::copy_to_user(
                            offset_ptr as *mut u8,
                            &v as *const i64 as *const u8,
                            core::mem::size_of::<i64>(),
                        );
                    }
                    // Linux do_sendfile: an error on the FIRST write attempt
                    // surfaces as the sendfile errno — a full nonblocking
                    // out_fd must report EAGAIN, not a silent 0-byte
                    // "success" (LTP sendfile07).
                    if n_write < 0 && total_transferred + written == 0 {
                        return n_write as i64;
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
            if crate::arch::uaccess::copy_to_user(
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
