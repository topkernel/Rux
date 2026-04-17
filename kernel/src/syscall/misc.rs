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

/// Epoll monitored fd entry
struct EpollEntry {
    fd: i32,
    events: u32,
    data: u64,
}

/// Epoll file structure (stored as File private_data)
struct EpollFile {
    entries: crate::sync::spinlock::Spinlock<alloc::vec::Vec<EpollEntry>>,
}

/// Epoll file close callback
fn epoll_file_close(_file: &crate::fs::File) -> i32 {
    0
}

/// Epoll file operations
static EPOLL_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: None,
    write: None,
    lseek: None,
    close: Some(epoll_file_close),
    poll: None,
};

/// sys_poll - I/O multiplexing (poll style)
///
/// # Arguments
/// - args[0]: fds - pointer to pollfd array
/// - args[1]: nfds - length of pollfd array
/// - args[2]: timeout - timeout in milliseconds
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_poll(args: SyscallArgs) -> i64 {
    use poll_events::*;

    let fds_ptr = args[0] as *mut PollFd;
    let nfds = args[1] as usize;
    let timeout_ms = args[2] as i32;

    // Check pointer validity
    if fds_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Check if fds_ptr is in valid user space
    let fds_size = core::mem::size_of::<PollFd>() * nfds;
    if !crate::arch::riscv64::uaccess::access_ok(fds_ptr as usize, fds_size) {
        return -(errno::EFAULT as i64);
    }

    // Check nfds range
    if nfds == 0 || nfds > 1024 {
        return -(errno::EINVAL as i64);
    }

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
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
            // SAFETY: fds_ptr validated with access_ok; nfds bounded to 1024.
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
            return ready_count as i64;
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
            return -(errno::EINTR as i64);
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
pub fn sys_ppoll(args: SyscallArgs) -> i64 {
    // ppoll has same pollfd checking logic as poll, but reads timeout from timespec
    let timeout_ptr = args[2] as *const u64;

    // Read timeout from struct timespec { tv_sec: u64, tv_nsec: u64 }
    let timeout_ms: i32 = if timeout_ptr.is_null() || !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, 16) {
        -1  // NULL or invalid pointer = infinite wait
    } else {
        // SAFETY: timeout_ptr validated with access_ok; reads two u64 fields.
        unsafe {
            let tv_sec = core::ptr::read_volatile(timeout_ptr);
            let tv_nsec = core::ptr::read_volatile(timeout_ptr.add(1));
            if tv_sec == 0 && tv_nsec == 0 {
                0  // Immediate return
            } else {
                // Convert to milliseconds, cap at i32 max
                let total_ms = tv_sec.saturating_mul(1000).saturating_add(tv_nsec / 1_000_000);
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
pub fn sys_pselect6(args: SyscallArgs) -> i64 {
    use poll_events::*;

    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
    let timeout_ptr = args[4] as *const TimeVal;
    let _sigmask_ptr = args[5] as *const u64;

    // Validate nfds range
    if nfds < 0 || nfds > FD_SETSIZE {
        return -(errno::EINVAL as i64);
    }

    // Check pointer validity
    let fdset_size = core::mem::size_of::<FdSet>();
    if !readfds_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(readfds_ptr as usize, fdset_size) {
        return -(errno::EFAULT as i64);
    }
    if !writefds_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(writefds_ptr as usize, fdset_size) {
        return -(errno::EFAULT as i64);
    }
    if !exceptfds_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(exceptfds_ptr as usize, fdset_size) {
        return -(errno::EFAULT as i64);
    }
    if !timeout_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, core::mem::size_of::<TimeVal>()) {
        return -(errno::EFAULT as i64);
    }

    if readfds_ptr.is_null() && writefds_ptr.is_null() && exceptfds_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Snapshot original fd_sets
    // SAFETY: fd set pointers validated with access_ok; reads are within FdSet size.
    let original_readfds = unsafe {
        if readfds_ptr.is_null() { FdSet::new() } else { *readfds_ptr }
    };
    // SAFETY: same as above.
    let original_writefds = unsafe {
        if writefds_ptr.is_null() { FdSet::new() } else { *writefds_ptr }
    };
    // SAFETY: same as above.
    let original_exceptfds = unsafe {
        if exceptfds_ptr.is_null() { FdSet::new() } else { *exceptfds_ptr }
    };

    // Parse timeout
    let (timeout_ms, has_timeout) = if timeout_ptr.is_null() {
        (0i64, false)
    } else {
        // SAFETY: timeout_ptr validated with access_ok; reads two i64 fields.
        unsafe {
            let tv = *timeout_ptr;
            (tv.tv_sec * 1000 + tv.tv_usec / 1000, true)
        }
    };

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    // Busy-wait loop with timeout (matching sys_poll pattern)
    let start_jiffies = crate::drivers::timer::get_jiffies();
    let timeout_jiffies = if has_timeout && timeout_ms > 0 {
        crate::drivers::timer::msecs_to_jiffies(timeout_ms as u64)
    } else {
        0
    };

    loop {
        let mut result_readfds = FdSet::new();
        let mut result_writefds = FdSet::new();
        let mut result_exceptfds = FdSet::new();
        let mut ready_count = 0usize;

        for fd in 0..nfds {
            let file = match fdtable.get_file(fd as usize) {
                Some(f) => f,
                None => continue,
            };

            // Map select events to poll events
            let mut poll_events: u16 = 0;
            if original_readfds.is_set(fd) {
                poll_events |= POLLIN;
            }
            if original_writefds.is_set(fd) {
                poll_events |= POLLOUT;
            }
            if original_exceptfds.is_set(fd) {
                poll_events |= POLLERR;
            }

            if poll_events == 0 {
                continue;
            }

            // Call file's poll callback
            let revents = match file.get_ops() {
                Some(ops) => {
                    match ops.poll {
                        Some(poll_fn) => poll_fn(&file, poll_events),
                        None => {
                            // No poll handler: regular files are always ready
                            let mut r = 0u16;
                            if poll_events & POLLIN != 0 { r |= POLLIN | POLLRDNORM; }
                            if poll_events & POLLOUT != 0 { r |= POLLOUT | POLLWRNORM; }
                            r
                        }
                    }
                }
                None => {
                    let mut r = 0u16;
                    if poll_events & POLLIN != 0 { r |= POLLIN | POLLRDNORM; }
                    if poll_events & POLLOUT != 0 { r |= POLLOUT | POLLWRNORM; }
                    r
                }
            };

            // Map poll revents back to select fd_sets
            if revents != 0 {
                if (revents & (POLLIN | POLLRDNORM | POLLHUP | POLLERR)) != 0
                    && original_readfds.is_set(fd)
                {
                    result_readfds.set(fd);
                    ready_count += 1;
                }
                if (revents & (POLLOUT | POLLWRNORM | POLLERR)) != 0
                    && original_writefds.is_set(fd)
                {
                    result_writefds.set(fd);
                    ready_count += 1;
                }
                if (revents & (POLLERR | POLLHUP)) != 0
                    && original_exceptfds.is_set(fd)
                {
                    result_exceptfds.set(fd);
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 {
            // SAFETY: fd set pointers validated with access_ok above; writes FdSet-sized results.
            unsafe {
                if !readfds_ptr.is_null() { *readfds_ptr = result_readfds; }
                if !writefds_ptr.is_null() { *writefds_ptr = result_writefds; }
                if !exceptfds_ptr.is_null() { *exceptfds_ptr = result_exceptfds; }
            }
            return ready_count as i64;
        }

        // No fd ready — check timeout
        if has_timeout && timeout_ms == 0 {
            // SAFETY: fd set pointers validated with access_ok above; writes FdSet-sized results.
            unsafe {
                if !readfds_ptr.is_null() { *readfds_ptr = result_readfds; }
                if !writefds_ptr.is_null() { *writefds_ptr = result_writefds; }
                if !exceptfds_ptr.is_null() { *exceptfds_ptr = result_exceptfds; }
            }
            return 0;
        }

        if has_timeout && timeout_ms > 0 {
            let elapsed = crate::drivers::timer::get_jiffies() - start_jiffies;
            if elapsed >= timeout_jiffies {
                // SAFETY: fd set pointers validated with access_ok above; writes FdSet-sized results.
                unsafe {
                    if !readfds_ptr.is_null() { *readfds_ptr = result_readfds; }
                    if !writefds_ptr.is_null() { *writefds_ptr = result_writefds; }
                    if !exceptfds_ptr.is_null() { *exceptfds_ptr = result_exceptfds; }
                }
                return 0;
            }
        }

        // Check for pending signals
        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        crate::sched::yield_cpu();
    }
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
pub fn sys_select(args: SyscallArgs) -> i64 {
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
pub fn sys_epoll_create(args: SyscallArgs) -> i64 {
    let _size = args[0] as i32;

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let epoll = alloc::boxed::Box::new(EpollFile {
        entries: crate::sync::spinlock::Spinlock::new(alloc::vec::Vec::new()),
    });
    let epoll_ptr = alloc::boxed::Box::into_raw(epoll) as *mut u8;

    let file = alloc::sync::Arc::new(crate::fs::File::new(
        crate::fs::FileFlags::new(crate::fs::FileFlags::O_RDWR)
    ));
    file.set_ops(&EPOLL_OPS);
    file.set_private_data(epoll_ptr);

    let epoll_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // SAFETY: epoll_ptr was created via Box::into_raw above; reclaim to free.
            unsafe {
                let _ = alloc::boxed::Box::from_raw(epoll_ptr as *mut EpollFile);
            }
            return -(errno::EMFILE as i64);
        }
    };

    match fdtable.install_fd(epoll_fd, file) {
        Ok(()) => epoll_fd as i64,
        Err(()) => {
            // SAFETY: epoll_ptr was created via Box::into_raw above; reclaim to free.
            unsafe {
                let _ = alloc::boxed::Box::from_raw(epoll_ptr as *mut EpollFile);
            }
            -(errno::ENOMEM as i64)
        }
    }
}

/// sys_epoll_create1 - Create epoll instance (with flags)
///
/// # Arguments
/// - args[0]: flags - flag bits
///
/// # Returns
/// Returns epoll file descriptor on success, negative error code on failure
pub fn sys_epoll_create1(args: SyscallArgs) -> i64 {
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
pub fn sys_epoll_ctl(args: SyscallArgs) -> i64 {
    use epoll_ctl_ops::*;

    let epfd = args[0] as i32;
    let op = args[1] as i32;
    let fd = args[2] as i32;
    let event_ptr = args[3] as *const EPollEvent;

    if epfd < 0 || fd < 0 {
        return -(errno::EBADF as i64);
    }
    if op != EPOLL_CTL_ADD && op != EPOLL_CTL_DEL && op != EPOLL_CTL_MOD {
        return -(errno::EINVAL as i64);
    }
    if (op == EPOLL_CTL_ADD || op == EPOLL_CTL_MOD) && event_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !event_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(event_ptr as usize, core::mem::size_of::<EPollEvent>()) {
        return -(errno::EFAULT as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let ep_file = match fdtable.get_file(epfd as usize) {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let epoll_ptr = match unsafe { *ep_file.private_data.get() } {
        Some(ptr) => ptr as *mut EpollFile,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: epoll_ptr came from Box::into_raw in sys_epoll_create; valid and unique.
    let epoll = unsafe { &mut *epoll_ptr };

    match op {
        EPOLL_CTL_ADD => {
            // SAFETY: event_ptr validated with access_ok above; reads EPollEvent.
            let event = unsafe { *event_ptr };
            let mut entries = epoll.entries.lock();
            if entries.iter().any(|e| e.fd == fd) {
                return -(errno::EEXIST as i64);
            }
            entries.push(EpollEntry {
                fd,
                events: event.events,
                data: event.data,
            });
        }
        EPOLL_CTL_DEL => {
            let mut entries = epoll.entries.lock();
            if let Some(pos) = entries.iter().position(|e| e.fd == fd) {
                entries.remove(pos);
            } else {
                return -(errno::ENOENT as i64);
            }
        }
        EPOLL_CTL_MOD => {
            // SAFETY: event_ptr validated with access_ok above; reads EPollEvent.
            let event = unsafe { *event_ptr };
            let mut entries = epoll.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.fd == fd) {
                entry.events = event.events;
                entry.data = event.data;
            } else {
                return -(errno::ENOENT as i64);
            }
        }
        _ => return -(errno::EINVAL as i64),
    }

    0
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
pub fn sys_epoll_wait(args: SyscallArgs) -> i64 {
    use epoll_events::*;
    use poll_events::*;

    let epfd = args[0] as i32;
    let events_ptr = args[1] as *mut EPollEvent;
    let maxevents = args[2] as i32;
    let timeout_ms = args[3] as i32;

    if epfd < 0 || events_ptr.is_null() || maxevents <= 0 || maxevents > 1024 {
        return -(errno::EINVAL as i64);
    }

    let events_size = core::mem::size_of::<EPollEvent>() * (maxevents as usize);
    if !crate::arch::riscv64::uaccess::access_ok(events_ptr as usize, events_size) {
        return -(errno::EFAULT as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let ep_file = match fdtable.get_file(epfd as usize) {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let epoll_ptr = match unsafe { *ep_file.private_data.get() } {
        Some(ptr) => ptr as *mut EpollFile,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: epoll_ptr came from Box::into_raw in sys_epoll_create; valid and unique.
    let epoll = unsafe { &mut *epoll_ptr };

    let start_jiffies = crate::drivers::timer::get_jiffies();
    let timeout_jiffies = if timeout_ms > 0 {
        crate::drivers::timer::msecs_to_jiffies(timeout_ms as u64)
    } else {
        0
    };

    loop {
        let entries = epoll.entries.lock();
        let mut ready_events: alloc::vec::Vec<EPollEvent> = alloc::vec::Vec::new();

        for entry in entries.iter() {
            let file = match fdtable.get_file(entry.fd as usize) {
                Some(f) => f,
                None => {
                    // fd was closed, report error
                    ready_events.push(EPollEvent {
                        events: EPOLLERR | EPOLLHUP,
                        data: entry.data,
                    });
                    continue;
                }
            };

            // Map epoll events to poll events
            let mut poll_mask: u16 = 0;
            if entry.events & EPOLLIN != 0 { poll_mask |= POLLIN; }
            if entry.events & EPOLLOUT != 0 { poll_mask |= POLLOUT; }

            let revents = match file.get_ops() {
                Some(ops) => match ops.poll {
                    Some(poll_fn) => poll_fn(&file, poll_mask),
                    None => poll_mask,
                },
                None => poll_mask,
            };

            if revents != 0 {
                let mut ep_events: u32 = 0;
                if revents & (POLLIN | POLLRDNORM | POLLHUP) != 0 { ep_events |= EPOLLIN; }
                if revents & (POLLOUT | POLLWRNORM) != 0 { ep_events |= EPOLLOUT; }
                if revents & POLLERR != 0 { ep_events |= EPOLLERR; }
                if revents & POLLHUP != 0 { ep_events |= EPOLLHUP; }

                ready_events.push(EPollEvent {
                    events: ep_events & entry.events,
                    data: entry.data,
                });
            }
        }
        drop(entries);

        if !ready_events.is_empty() {
            let count = ready_events.len().min(maxevents as usize);
            // SAFETY: events_ptr validated with access_ok; count bounded by maxevents <= 1024.
            unsafe {
                for i in 0..count {
                    *events_ptr.add(i) = ready_events[i];
                }
            }
            return count as i64;
        }

        // Check timeout
        if timeout_ms == 0 {
            return 0;
        }
        if timeout_ms > 0 {
            let elapsed = crate::drivers::timer::get_jiffies() - start_jiffies;
            if elapsed >= timeout_jiffies {
                return 0;
            }
        }

        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        crate::sched::yield_cpu();
    }
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
pub fn sys_epoll_pwait(args: SyscallArgs) -> i64 {
    // Simplified implementation: ignore signal mask
    sys_epoll_wait([args[0], args[1], args[2], args[3], 0, 0])
}

// ============================================================================
// eventfd
// ============================================================================

/// EFD_SEMAPHORE: read returns 1 and decrements counter by 1 (instead of returning full counter)
const EFD_SEMAPHORE: u32 = 0x1;

/// eventfd backend: 64-bit counter
struct EventFd {
    counter: core::sync::atomic::AtomicU64,
    /// EFD_SEMAPHORE flag
    semaphore: bool,
}

impl EventFd {
    fn new(initval: u64, flags: u32) -> Self {
        Self {
            counter: core::sync::atomic::AtomicU64::new(initval),
            semaphore: (flags & EFD_SEMAPHORE) != 0,
        }
    }
}

fn eventfd_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if buf.len() < 8 {
        return -errno::EINVAL as isize;
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_eventfd2; valid and properly aligned.
    let efd = unsafe { &*(ptr as *const EventFd) };

    loop {
        let val = efd.counter.load(core::sync::atomic::Ordering::Relaxed);
        if val == 0 {
            if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
                return -errno::EAGAIN as isize;
            }
            // TODO: block until woken
            return -errno::EAGAIN as isize;
        }
        let new_val = if efd.semaphore { val - 1 } else { 0 };
        if efd.counter.compare_exchange_weak(
            val, new_val,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        ).is_ok() {
            let return_val = if efd.semaphore { 1u64 } else { val };
            buf[..8].copy_from_slice(&return_val.to_le_bytes());
            return 8;
        }
        // CAS failed, retry
    }
}

fn eventfd_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    if buf.len() < 8 {
        return -errno::EINVAL as isize;
    }
    let val = u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
    if val == u64::MAX {
        return -errno::EINVAL as isize;
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_eventfd2; valid and properly aligned.
    let efd = unsafe { &*(ptr as *const EventFd) };

    loop {
        let cur = efd.counter.load(core::sync::atomic::Ordering::Relaxed);
        let new = cur.checked_add(val);
        match new {
            Some(n) => {
                if efd.counter.compare_exchange_weak(
                    cur, n,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Relaxed,
                ).is_ok() {
                    return 8;
                }
                // CAS failed, retry
            }
            None => {
                // Overflow: counter + val > u64::MAX
                let flags = file.flags().bits();
                if flags & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
                    return -errno::EAGAIN as isize;
                }
                // TODO: block until counter decreases
                return -errno::EAGAIN as isize;
            }
        }
    }
}

fn eventfd_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr came from Box::into_raw in sys_eventfd2; valid and properly aligned.
    let efd = unsafe { &*(ptr as *const EventFd) };

    let counter = efd.counter.load(core::sync::atomic::Ordering::Relaxed);

    if events & POLLIN != 0 && counter > 0 {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 && counter < u64::MAX - 1 {
        ready |= POLLOUT | POLLWRNORM;
    }

    ready
}

fn eventfd_close(_file: &crate::fs::File) -> i32 {
    // EventFd is freed when File is dropped (Box in private_data)
    0
}

/// EventFd file operations
static EVENTFD_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(eventfd_read),
    write: Some(eventfd_write),
    lseek: None,
    close: Some(eventfd_close),
    poll: Some(eventfd_poll),
};

// ==================== timerfd ====================

/// timerfd backend: timer + expiration counter
struct TimerFd {
    /// Clock ID (CLOCK_REALTIME=0, CLOCK_MONOTONIC=1)
    clockid: i32,
    /// Kernel timer ID (0 = disarmed)
    kernel_timer_id: u64,
    /// Interval in jiffies (0 = one-shot)
    interval_jiffies: u64,
    /// Number of timer expirations since last read()
    expiration_count: core::sync::atomic::AtomicU64,
}

impl TimerFd {
    fn new(clockid: i32) -> Self {
        Self {
            clockid,
            kernel_timer_id: 0,
            interval_jiffies: 0,
            expiration_count: core::sync::atomic::AtomicU64::new(0),
        }
    }
}

fn timerfd_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if buf.len() < 8 {
        return -errno::EINVAL as isize;
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    // Read and reset the expiration count
    let count = tfd.expiration_count.swap(0, core::sync::atomic::Ordering::AcqRel);
    if count == 0 {
        // Non-blocking check
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }
        // TODO: block until timer fires
        return -errno::EAGAIN as isize;
    }

    buf[..8].copy_from_slice(&count.to_le_bytes());
    8
}

fn timerfd_close(file: &crate::fs::File) -> i32 {
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    // Disarm kernel timer
    if tfd.kernel_timer_id != 0 {
        crate::timer::del_timer(tfd.kernel_timer_id);
    }

    // TimerFd is freed when File is dropped (Box in private_data)
    0
}

fn timerfd_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    let count = tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire);

    if events & POLLIN != 0 && count > 0 {
        ready |= POLLIN | POLLRDNORM;
    }

    ready
}

/// Write old timer settings (for timerfd_gettime / timerfd_settime old_value)
fn timerfd_write_olds(tfd: &TimerFd, old_value: *mut u64) {
    // SAFETY: old_value validated with access_ok(32 bytes) by callers; writes 4 i64 values.
    unsafe {
        let p = old_value as *mut i64;
        // it_interval
        if tfd.interval_jiffies > 0 {
            let int_msecs = crate::drivers::timer::jiffies_to_msecs(tfd.interval_jiffies);
            core::ptr::write(p, (int_msecs / 1000) as i64);
            core::ptr::write(p.add(1), 0i64);
        } else {
            core::ptr::write(p, 0i64);
            core::ptr::write(p.add(1), 0i64);
        }
        // it_value
        if tfd.kernel_timer_id != 0 && crate::timer::timer_pending(tfd.kernel_timer_id) {
            if tfd.interval_jiffies > 0 {
                let val_msecs = crate::drivers::timer::jiffies_to_msecs(tfd.interval_jiffies);
                core::ptr::write(p.add(2), (val_msecs / 1000) as i64);
            } else {
                core::ptr::write(p.add(2), 1i64);
            }
            core::ptr::write(p.add(3), 0i64);
        } else {
            core::ptr::write(p.add(2), 0i64);
            core::ptr::write(p.add(3), 0i64);
        }
    }
}

/// TimerFd file operations
static TIMERFD_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(timerfd_read),
    write: None,
    lseek: None,
    close: Some(timerfd_close),
    poll: Some(timerfd_poll),
};

/// sys_eventfd - Create eventfd object (legacy, no flags)
pub fn sys_eventfd(args: SyscallArgs) -> i64 {
    sys_eventfd2([args[0], 0, 0, 0, 0, 0])
}

/// sys_eventfd2 - Create eventfd object (with flags)
///
/// # Arguments
/// - args[0]: initval - initial value of the counter
/// - args[1]: flags - EFD_CLOEXEC (0x80000), EFD_NONBLOCK (0x800), EFD_SEMAPHORE (0x1)
///
/// # Returns
/// Returns eventfd file descriptor on success, negative error code on failure
pub fn sys_eventfd2(args: SyscallArgs) -> i64 {
    let initval = args[0] as u64;
    let flags = args[1] as u32;

    // Validate flags: only accept EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE
    const EFD_CLOEXEC: u32 = 0x80000;
    const EFD_NONBLOCK: u32 = 0x800;
    const VALID_FLAGS: u32 = EFD_CLOEXEC | EFD_NONBLOCK | EFD_SEMAPHORE;
    if flags & !VALID_FLAGS != 0 {
        return -(errno::EINVAL as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    // Create EventFd
    let efd = alloc::boxed::Box::new(EventFd::new(initval, flags & EFD_SEMAPHORE));
    let efd_ptr = alloc::boxed::Box::into_raw(efd) as *mut u8;

    // Build file flags
    let mut file_flags = crate::fs::file::FileFlags::O_RDWR;
    if flags & EFD_NONBLOCK != 0 {
        file_flags |= crate::fs::file::FileFlags::O_NONBLOCK;
    }

    let file = alloc::sync::Arc::new(crate::fs::File::new(crate::fs::file::FileFlags::new(file_flags)));
    file.set_ops(&EVENTFD_OPS);
    file.set_private_data(efd_ptr);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // Reclaim EventFd
            // SAFETY: efd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(efd_ptr as *mut EventFd); }
            return -(errno::EMFILE as i64);
        }
    };

    // Handle EFD_CLOEXEC via fcntl
    if flags & EFD_CLOEXEC != 0 {
        file.set_cloexec(true);
    }

    match fdtable.install_fd(fd, file) {
        Ok(()) => fd as i64,
        Err(_) => {
            // SAFETY: efd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(efd_ptr as *mut EventFd); }
            -(errno::EMFILE as i64)
        }
    }
}

/// sys_inotify_init1 - Create inotify instance
///
/// # Arguments
/// - args[0]: flags - IN_CLOEXEC, IN_NONBLOCK
pub fn sys_inotify_init1(args: SyscallArgs) -> i64 {
    let _flags = args[0] as i32;
    // inotify requires full filesystem monitoring infrastructure
    // Return -EMFILE to indicate resource limit rather than ENOSYS
    -(errno::EMFILE as i64)
}

/// sys_inotify_add_watch - Add watch to inotify instance
///
/// # Arguments
/// - args[0]: fd - inotify file descriptor
/// - args[1]: pathname - path to watch
/// - args[2]: mask - event mask
pub fn sys_inotify_add_watch(_args: SyscallArgs) -> i64 {
    -(errno::EBADF as i64)
}

/// sys_inotify_rm_watch - Remove watch from inotify instance
///
/// # Arguments
/// - args[0]: fd - inotify file descriptor
/// - args[1]: wd - watch descriptor
pub fn sys_inotify_rm_watch(_args: SyscallArgs) -> i64 {
    -(errno::EBADF as i64)
}

/// sys_timerfd_create - Create timer file descriptor
///
/// # Arguments
/// - args[0]: clockid - clock ID (CLOCK_REALTIME=0, CLOCK_MONOTONIC=1)
/// - args[1]: flags - TFD_CLOEXEC, TFD_NONBLOCK
pub fn sys_timerfd_create(args: SyscallArgs) -> i64 {
    let clockid = args[0] as i32;
    let flags = args[1] as i32;

    // Only CLOCK_REALTIME and CLOCK_MONOTONIC supported
    if clockid != 0 && clockid != 1 {
        return -(errno::EINVAL as i64);
    }

    // Validate flags
    const TFD_CLOEXEC: i32 = 0x80000;
    const TFD_NONBLOCK: i32 = 0x800;
    if flags & !(TFD_CLOEXEC | TFD_NONBLOCK) != 0 {
        return -(errno::EINVAL as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EMFILE as i64),
    };

    // Create TimerFd
    let tfd = alloc::boxed::Box::new(TimerFd::new(clockid));
    let tfd_ptr = alloc::boxed::Box::into_raw(tfd) as *mut u8;

    // Build file flags
    let mut file_flags = crate::fs::file::FileFlags::O_RDONLY;
    if flags & TFD_NONBLOCK != 0 {
        file_flags |= crate::fs::file::FileFlags::O_NONBLOCK;
    }

    let file = alloc::sync::Arc::new(crate::fs::File::new(crate::fs::file::FileFlags::new(file_flags)));
    file.set_ops(&TIMERFD_OPS);
    file.set_private_data(tfd_ptr);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // SAFETY: tfd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(tfd_ptr as *mut TimerFd); }
            return -(errno::EMFILE as i64);
        }
    };

    if flags & TFD_CLOEXEC != 0 {
        file.set_cloexec(true);
    }

    match fdtable.install_fd(fd, file) {
        Ok(()) => fd as i64,
        Err(_) => {
            // SAFETY: tfd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(tfd_ptr as *mut TimerFd); }
            -(errno::EMFILE as i64)
        }
    }
}

/// sys_timerfd_settime - Set timer settings
///
/// # Arguments
/// - args[0]: fd - timerfd file descriptor
/// - args[1]: flags - TFD_TIMER_ABSTIME
/// - args[2]: new_value - new timer settings (struct itimerspec, 32 bytes)
/// - args[3]: old_value - old timer settings (output)
pub fn sys_timerfd_settime(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let flags = args[1] as i32;
    let new_value = args[2] as *const u64;
    let old_value = args[3] as *mut u64;

    if new_value.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(new_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    // Validate fd and get file
    // SAFETY: fd is a valid timerfd file descriptor from timerfd_create.
    let file = match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and unique.
    let tfd = unsafe { &mut *(ptr as *mut TimerFd) };

    // Write old_value (current settings)
    if !old_value.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(old_value as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        timerfd_write_olds(tfd, old_value);
    }

    // Read struct itimerspec { struct timespec it_interval, struct timespec it_value }
    // SAFETY: new_value validated with access_ok(32 bytes); reads 4 i64 fields.
    let (int_sec, int_nsec, val_sec, val_nsec) = unsafe {
        let p = new_value as *const i64;
        (
            core::ptr::read(p),
            core::ptr::read(p.add(1)),
            core::ptr::read(p.add(2)),
            core::ptr::read(p.add(3)),
        )
    };

    // Disarm existing timer
    if tfd.kernel_timer_id != 0 {
        crate::timer::del_timer(tfd.kernel_timer_id);
        tfd.kernel_timer_id = 0;
    }

    // If value is zero, timer is disarmed
    let total_nsec = val_sec * 1_000_000_000 + val_nsec;
    if total_nsec <= 0 {
        return 0;
    }

    // Convert to jiffies
    let value_msecs = (total_nsec / 1_000_000) as u64;
    let value_jiffies = crate::drivers::timer::msecs_to_jiffies(value_msecs).max(1);

    let interval_nsec = int_sec * 1_000_000_000 + int_nsec;
    let interval_jiffies = if interval_nsec > 0 {
        let interval_msecs = (interval_nsec / 1_000_000) as u64;
        crate::drivers::timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0
    };

    let expires = if flags & 1 != 0 {
        // TFD_TIMER_ABSTIME
        crate::drivers::timer::get_jiffies() + value_jiffies
    } else {
        crate::drivers::timer::get_jiffies() + value_jiffies
    };

    // Use timerfd mode: pass the expiration_count address as tfd_addr
    // The timer softirq handler will increment it on expiry.
    let counter_addr = &tfd.expiration_count as *const core::sync::atomic::AtomicU64 as u64;

    let new_kernel_id = crate::timer::add_timer_with_action(
        expires,
        0, // no signal
        0, // no signal
        interval_jiffies,
        counter_addr,
    );

    tfd.kernel_timer_id = new_kernel_id;
    tfd.interval_jiffies = interval_jiffies;
    tfd.expiration_count.store(0, core::sync::atomic::Ordering::Relaxed);

    0
}

/// sys_timerfd_gettime - Get timer settings
///
/// # Arguments
/// - args[0]: fd - timerfd file descriptor
/// - args[1]: curr_value - current timer settings (output, 32 bytes)
pub fn sys_timerfd_gettime(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let curr_value = args[1] as *mut u64;

    if curr_value.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(curr_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: fd is a valid timerfd file descriptor from timerfd_create.
    let file = match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    timerfd_write_olds(tfd, curr_value);
    0
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
pub fn sys_getrandom(args: SyscallArgs) -> i64 {
    let buf_ptr = args[0] as *mut u8;
    let buflen = args[1] as usize;
    let _flags = args[2] as u32;

    if buf_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }

    if buflen == 0 {
        return 0;
    }

    // Validate user space pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, buflen) {
        return -(errno::EFAULT as i64);
    }

    // Use simple pseudo-random number generator
    // In a real system should use hardware random or more secure RNG
    // SAFETY: buf_ptr validated with access_ok(buflen); writes buflen bytes.
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

    buflen as i64
}
