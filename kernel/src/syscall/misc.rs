//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Miscellaneous system calls
//!
//! Includes: poll, select, pselect6, epoll_create, epoll_create1, epoll_ctl, epoll_wait,
//! epoll_pwait, eventfd, eventfd2, getrandom, read_input_event

use super::*;
use core::sync::atomic::{AtomicU32, Ordering};

/// pollfd structure (struct pollfd)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PollFd {
    pub fd: i32,
    pub events: u16,
    pub revents: u16,
}

/// poll event types
pub mod poll_events {
    pub const POLLIN: u16 = 0x0001;
    pub const POLLPRI: u16 = 0x0002;
    pub const POLLOUT: u16 = 0x0004;
    pub const POLLERR: u16 = 0x0008;
    pub const POLLHUP: u16 = 0x0010;
    pub const POLLNVAL: u16 = 0x0020;
    pub const POLLRDNORM: u16 = 0x0040;
    pub const POLLRDBAND: u16 = 0x0080;
    pub const POLLWRNORM: u16 = 0x0100;
    pub const POLLWRBAND: u16 = 0x0200;
}

/// epoll_event structure
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EPollEvent {
    pub events: u32,
    pub data: u64,
}

/// epoll event types
pub mod epoll_events {
    pub const EPOLLIN: u32 = 0x00000001;
    pub const EPOLLPRI: u32 = 0x00000002;
    pub const EPOLLOUT: u32 = 0x00000004;
    pub const EPOLLERR: u32 = 0x00000008;
    pub const EPOLLHUP: u32 = 0x00000010;
    pub const EPOLLRDHUP: u32 = 0x00002000;
    pub const EPOLLONESHOT: u32 = 0x40000000;
    pub const EPOLLET: u32 = 1 << 31;
}

/// epoll operation types
pub mod epoll_ctl_ops {
    pub const EPOLL_CTL_ADD: i32 = 1;
    pub const EPOLL_CTL_DEL: i32 = 2;
    pub const EPOLL_CTL_MOD: i32 = 3;
}

// Global epoll instance counter (simplified implementation)
static EPOLL_INSTANCE_COUNTER: AtomicU32 = AtomicU32::new(1);

/// sys_poll - I/O multiplexing (poll style)
///
/// # Arguments
/// - args[0]: fds - pointer to pollfd array
/// - args[1]: nfds - length of pollfd array
/// - args[2]: timeout - timeout in milliseconds
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_poll(args: SyscallArgs) -> u64 {
    use poll_events::*;

    let fds_ptr = args[0] as *mut PollFd;
    let nfds = args[1] as usize;
    let timeout_ms = args[2] as i32;

    // Check pointer validity
    if fds_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Check if fds_ptr is in valid user space
    let fds_size = core::mem::size_of::<PollFd>() * nfds;
    if !crate::arch::riscv64::uaccess::access_ok(fds_ptr as usize, fds_size) {
        return -errno::EFAULT as u64;
    }

    // Check nfds range
    if nfds == 0 || nfds > 1024 {
        return -errno::EINVAL as u64;
    }

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    // Poll loop with timeout support
    let start_jiffies = crate::drivers::timer::get_jiffies();
    let timeout_jiffies = if timeout_ms > 0 {
        crate::drivers::timer::msecs_to_jiffies(timeout_ms as u64)
    } else {
        0
    };

    loop {
        let mut ready_count = 0usize;

        // Check all file descriptors
        for i in 0..nfds {
            unsafe {
                let pollfd = &mut *fds_ptr.add(i);
                pollfd.revents = 0;

                let file = match fdtable.get_file(pollfd.fd as usize) {
                    Some(f) => f,
                    None => {
                        pollfd.revents |= POLLNVAL;
                        ready_count += 1;
                        continue;
                    }
                };

                // Use per-file-type poll callback if available
                let revents = match file.get_ops() {
                    Some(ops) => {
                        match ops.poll {
                            Some(poll_fn) => poll_fn(&file, pollfd.events),
                            None => {
                                // No poll handler: default to always ready
                                let mut r = 0u16;
                                if pollfd.events & POLLIN != 0 {
                                    r |= POLLIN | POLLRDNORM;
                                }
                                if pollfd.events & POLLOUT != 0 {
                                    r |= POLLOUT | POLLWRNORM;
                                }
                                r
                            }
                        }
                    }
                    None => {
                        // No ops: default to always ready
                        let mut r = 0u16;
                        if pollfd.events & POLLIN != 0 {
                            r |= POLLIN | POLLRDNORM;
                        }
                        if pollfd.events & POLLOUT != 0 {
                            r |= POLLOUT | POLLWRNORM;
                        }
                        r
                    }
                };

                if revents != 0 {
                    pollfd.revents = revents;
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 {
            return ready_count as u64;
        }

        // No fd ready - check timeout
        if timeout_ms == 0 {
            return 0;  // Return immediately
        }

        // timeout_ms < 0 means wait forever (only break on data or signal)
        if timeout_ms > 0 {
            // Check if timeout expired
            let elapsed = crate::drivers::timer::get_jiffies() - start_jiffies;
            if elapsed >= timeout_jiffies {
                return 0;
            }
        }

        // Check for pending signals
        if crate::signal::signal_pending() {
            return -errno::EINTR as u64;
        }

        // Yield CPU and retry
        crate::sched::yield_cpu();
    }
}

/// sys_ppoll - I/O multiplexing (ppoll style, syscall nr=73)
///
/// # Arguments
/// - args[0]: fds - pointer to pollfd array
/// - args[1]: nfds - length of pollfd array
/// - args[2]: timeout - pointer to struct timespec (sec, nsec), or NULL for infinite
/// - args[3]: sigmask - pointer to signal mask (ignored)
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_ppoll(args: SyscallArgs) -> u64 {
    // ppoll has same pollfd checking logic as poll, but reads timeout from timespec
    let timeout_ptr = args[2] as *const u64;

    // Read timeout from struct timespec { tv_sec: u64, tv_nsec: u64 }
    let timeout_ms: i32 = if timeout_ptr.is_null() || !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, 16) {
        -1  // NULL or invalid pointer = infinite wait
    } else {
        unsafe {
            let tv_sec = core::ptr::read_volatile(timeout_ptr);
            let tv_nsec = core::ptr::read_volatile(timeout_ptr.add(1));
            if tv_sec == 0 && tv_nsec == 0 {
                0  // Immediate return
            } else {
                // Convert to milliseconds, cap at i32 max
                let total_ms = tv_sec * 1000 + tv_nsec / 1_000_000;
                if total_ms > i32::MAX as u64 {
                    -1  // Very long timeout = infinite for our purposes
                } else {
                    total_ms as i32
                }
            }
        }
    };

    // Delegate to sys_poll with converted timeout
    let poll_args: super::SyscallArgs = [args[0], args[1], timeout_ms as u64, 0, 0, 0];
    sys_poll(poll_args)
}

/// sys_pselect6 - I/O multiplexing (pselect6 style)
///
/// # Arguments
/// - args[0]: nfds - highest file descriptor number to check + 1
/// - args[1]: readfds - pointer to readable file descriptor set
/// - args[2]: writefds - pointer to writable file descriptor set
/// - args[3]: exceptfds - pointer to exception file descriptor set
/// - args[4]: timeout - pointer to TimeVal structure
/// - args[5]: sigmask - pointer to signal mask
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_pselect6(args: SyscallArgs) -> u64 {
    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
    let timeout_ptr = args[4] as *const TimeVal;
    let _sigmask_ptr = args[5] as *const u64;  // sigmask not currently used

    // Validate nfds range
    if nfds < 0 || nfds > FD_SETSIZE {
        return -errno::EINVAL as u64;
    }

    // Check pointer validity using access_ok
    let fdset_size = core::mem::size_of::<FdSet>();
    if !readfds_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(readfds_ptr as usize, fdset_size) {
        return -errno::EFAULT as u64;
    }
    if !writefds_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(writefds_ptr as usize, fdset_size) {
        return -errno::EFAULT as u64;
    }
    if !exceptfds_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(exceptfds_ptr as usize, fdset_size) {
        return -errno::EFAULT as u64;
    }
    if !timeout_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, core::mem::size_of::<TimeVal>()) {
        return -errno::EFAULT as u64;
    }

    // Check if at least one fdset is provided
    if readfds_ptr.is_null() && writefds_ptr.is_null() && exceptfds_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Read original fd_sets
    let mut original_readfds = FdSet::new();
    let mut original_writefds = FdSet::new();
    let mut original_exceptfds = FdSet::new();

    unsafe {
        if !readfds_ptr.is_null() {
            original_readfds = *readfds_ptr;
        }
        if !writefds_ptr.is_null() {
            original_writefds = *writefds_ptr;
        }
        if !exceptfds_ptr.is_null() {
            original_exceptfds = *exceptfds_ptr;
        }
    }

    // Create result fd_sets
    let mut result_readfds = FdSet::new();
    let mut result_writefds = FdSet::new();
    let mut result_exceptfds = FdSet::new();

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    let mut ready_count = 0;

    // Check all file descriptors
    for fd in 0..nfds {
        let mut is_readable = false;
        let mut is_writable = false;
        let mut has_exception = false;

        // Check if file descriptor exists
        let file_exists = fdtable.get_file(fd as usize).is_some();

        if !file_exists {
            // File descriptor does not exist, skip
            continue;
        }

        // Simplified implementation:
        // 1. For readfds: all valid fds are considered readable
        if original_readfds.is_set(fd) {
            is_readable = true;
        }

        // 2. For writefds: all valid fds are considered writable
        if original_writefds.is_set(fd) {
            is_writable = true;
        }

        // 3. For exceptfds: exception checking not implemented
        if original_exceptfds.is_set(fd) {
            has_exception = false;  // Not currently supported
        }

        // Set result fd_sets
        if is_readable {
            result_readfds.set(fd);
            ready_count += 1;
        }
        if is_writable {
            result_writefds.set(fd);
            ready_count += 1;
        }
        if has_exception {
            result_exceptfds.set(fd);
            ready_count += 1;
        }
    }

    // Write results back to user space
    unsafe {
        if !readfds_ptr.is_null() {
            *readfds_ptr = result_readfds;
        }
        if !writefds_ptr.is_null() {
            *writefds_ptr = result_writefds;
        }
        if !exceptfds_ptr.is_null() {
            *exceptfds_ptr = result_exceptfds;
        }
    }

    // TODO: Implement timeout mechanism
    let _ = timeout_ptr;

    ready_count as u64
}

/// sys_select - I/O multiplexing (BSD style)
///
/// # Arguments
/// - args[0]: nfds - highest file descriptor number to check + 1
/// - args[1]: readfds - pointer to readable file descriptor set
/// - args[2]: writefds - pointer to writable file descriptor set
/// - args[3]: exceptfds - pointer to exception file descriptor set
/// - args[4]: timeout - pointer to TimeVal structure
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_select(args: SyscallArgs) -> u64 {
    // select is a special case of pselect6 with sigmask as null
    sys_pselect6([args[0], args[1], args[2], args[3], args[4], 0])
}

/// sys_epoll_create - Create epoll instance
///
/// # Arguments
/// - args[0]: size - hint for number of events to allocate (deprecated)
///
/// # Returns
/// Returns epoll file descriptor on success, negative error code on failure
pub fn sys_epoll_create(args: SyscallArgs) -> u64 {
    let _size = args[0] as i32;

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    // Allocate file descriptor
    let epoll_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    // Simplified implementation:
    // In a real implementation, should create an EpollFile and install it to fdtable
    // Here we just allocate an fd, actual functionality is implemented by epoll_ctl/epoll_wait
    // TODO: Create EpollFile structure

    epoll_fd as u64
}

/// sys_epoll_create1 - Create epoll instance (with flags)
///
/// # Arguments
/// - args[0]: flags - flag bits
///
/// # Returns
/// Returns epoll file descriptor on success, negative error code on failure
pub fn sys_epoll_create1(args: SyscallArgs) -> u64 {
    // Simplified implementation: ignore flags
    // O_CLOEXEC (0x80000) and other flags not currently supported
    sys_epoll_create(args)
}

/// sys_epoll_ctl - Control epoll instance
///
/// # Arguments
/// - args[0]: epfd - epoll file descriptor
/// - args[1]: op - operation type (ADD/DEL/MOD)
/// - args[2]: fd - target file descriptor
/// - args[3]: event - pointer to event structure
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_epoll_ctl(args: SyscallArgs) -> u64 {
    use epoll_ctl_ops::*;

    let epfd = args[0] as i32;
    let op = args[1] as i32;
    let fd = args[2] as i32;
    let event_ptr = args[3] as *const EPollEvent;

    // Validate epfd
    if epfd < 0 {
        return -errno::EBADF as u64;
    }

    // Validate op
    if op != EPOLL_CTL_ADD && op != EPOLL_CTL_DEL && op != EPOLL_CTL_MOD {
        return -errno::EINVAL as u64;
    }

    // Validate fd
    if fd < 0 {
        return -errno::EBADF as u64;
    }

    // Validate event_ptr (ADD and MOD require event)
    if (op == EPOLL_CTL_ADD || op == EPOLL_CTL_MOD) && event_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user pointer
    if !event_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(event_ptr as usize, core::mem::size_of::<EPollEvent>()) {
        return -errno::EFAULT as u64;
    }

    // Simplified implementation:
    // In a real implementation, should:
    // 1. Find the EpollFile corresponding to epfd
    // 2. Add/delete/modify fd to epoll set based on op
    // TODO: Implement EpollFile and red-black tree

    0  // Success
}

/// sys_epoll_wait - Wait for epoll events
///
/// # Arguments
/// - args[0]: epfd - epoll file descriptor
/// - args[1]: events - pointer to event array
/// - args[2]: maxevents - maximum number of events
/// - args[3]: timeout - timeout in milliseconds
///
/// # Returns
/// Returns number of ready events on success, 0 on timeout, negative error code on failure
pub fn sys_epoll_wait(args: SyscallArgs) -> u64 {
    let epfd = args[0] as i32;
    let events_ptr = args[1] as *mut EPollEvent;
    let maxevents = args[2] as i32;
    let timeout_ms = args[3] as i32;

    // Validate epfd
    if epfd < 0 {
        return -errno::EBADF as u64;
    }

    // Validate events_ptr
    if events_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user pointer
    let events_size = core::mem::size_of::<EPollEvent>() * (maxevents as usize);
    if !crate::arch::riscv64::uaccess::access_ok(events_ptr as usize, events_size) {
        return -errno::EFAULT as u64;
    }

    // Validate maxevents
    if maxevents <= 0 || maxevents > 1024 {
        return -errno::EINVAL as u64;
    }

    // Simplified implementation:
    // In a real implementation, should:
    // 1. Find the EpollFile corresponding to epfd
    // 3. Wait for events or timeout
    // 4. Copy ready events to user space
    // TODO: Implement real wait logic

    // Current simplified: return 0 immediately (timeout)
    let _ = (epfd, events_ptr, maxevents, timeout_ms);

    0  // Timeout
}

/// sys_epoll_pwait - Wait for epoll events (with signal mask)
///
/// # Arguments
/// - args[0]: epfd - epoll file descriptor
/// - args[1]: events - pointer to event array
/// - args[2]: maxevents - maximum number of events
/// - args[3]: timeout - timeout in milliseconds
/// - args[4]: sigmask - pointer to signal mask
///
/// # Returns
/// Returns number of ready events on success, 0 on timeout, negative error code on failure
pub fn sys_epoll_pwait(args: SyscallArgs) -> u64 {
    // Simplified implementation: ignore signal mask
    sys_epoll_wait([args[0], args[1], args[2], args[3], 0, 0])
}

/// sys_eventfd - Create eventfd object
///
/// # Arguments
/// - args[0]: initval - initial value
///
/// # Returns
/// Returns eventfd file descriptor on success, negative error code on failure
pub fn sys_eventfd(args: SyscallArgs) -> u64 {
    let _initval = args[0] as u32;

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    // Allocate file descriptor
    let eventfd_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    // Simplified implementation:
    // In a real implementation, should create an EventFdFile and install it to fdtable
    // eventfd is essentially a 64-bit counter
    // TODO: Create EventFdFile structure

    eventfd_fd as u64
}

/// sys_eventfd2 - Create eventfd object (with flags)
///
/// # Arguments
/// - args[0]: initval - initial value
/// - args[1]: flags - flag bits
///
/// # Returns
/// Returns eventfd file descriptor on success, negative error code on failure
pub fn sys_eventfd2(args: SyscallArgs) -> u64 {
    // Simplified implementation: ignore flags
    // EFD_CLOEXEC (0x80000), EFD_NONBLOCK (0x800), EFD_SEMAPHORE (0x1) not currently supported
    sys_eventfd(args)
}

/// sys_getrandom - Get random bytes
///
/// # Arguments
/// - args[0]: buf - buffer to store random bytes
/// - args[1]: buflen - number of bytes requested
/// - args[2]: flags - flags (GRND_NONBLOCK, GRND_RANDOM, etc.)
///
/// # Returns
/// Returns number of bytes written on success, negative error code on failure
pub fn sys_getrandom(args: SyscallArgs) -> u64 {
    let buf_ptr = args[0] as *mut u8;
    let buflen = args[1] as usize;
    let _flags = args[2] as u32;

    if buf_ptr.is_null() {
        return -errno::EINVAL as u64;
    }

    if buflen == 0 {
        return 0;
    }

    // Validate user space pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, buflen) {
        return -errno::EFAULT as u64;
    }

    // Use simple pseudo-random number generator
    // In a real system should use hardware random or more secure RNG
    unsafe {
        // Use timestamp as seed
        let seed = crate::drivers::intc::clint::read_time();

        // Simple linear congruential generator
        let mut state = seed;
        for i in 0..buflen {
            // LCG: state = state * 1103515245 + 12345
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *buf_ptr.add(i) = ((state >> 16) & 0xff) as u8;
        }
    }

    buflen as u64
}
