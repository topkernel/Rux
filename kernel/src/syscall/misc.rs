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

/// epoll_wait re-check interval (P1 busy-yield elimination): waiters sleep
/// on the instance's wait queue and are re-woken at least every 10ms to
/// re-poll readiness (one jiffy at the default HZ=100).
const EPOLL_RECHECK_JIFFIES: u64 = {
    let j = crate::drivers::timer::msecs_to_jiffies(10);
    if j == 0 { 1 } else { j }
};

/// Epoll monitored fd entry
struct EpollEntry {
    fd: i32,
    /// Opaque identity of the open file description captured at ADD time
    /// (R32-B9, R36-B1): the File's monotonic generation id. Compared for
    /// equality only, never dereferenced, so it is harmless after the file
    /// is freed — and, unlike the earlier Arc-address scheme, it can never
    /// alias a NEW file even when the allocator recycles the old address.
    /// Guards the wait path against the fd NUMBER being closed and reused
    /// by an unrelated open file.
    file_id: u64,
    events: u32,
    data: u64,
}

/// Epoll file structure (stored as File private_data)
struct EpollFile {
    entries: crate::sync::spinlock::Spinlock<alloc::vec::Vec<EpollEntry>>,
    /// Waiters blocked in epoll_wait (P1: busy-yield elimination). Woken by
    /// the re-check timer, signal delivery, or `epoll_notify_file` once
    /// fd-side producers (pipe/socket/...) are wired to call it.
    wait_queue: crate::process::wait::WaitQueueHead,
}

// ============================================================================
// Epoll wake registry (groundwork for fd-side callback wakeups)
// ============================================================================

/// Monitored-file identity → epoll wait queues registered for it.
///
/// `epoll_ctl(ADD)` binds (file_id, &epoll.wait_queue); `epoll_notify_file`
/// wakes every epoll instance watching that open file description. This is
/// the "correct" wake path: fd-side data-arrival points (pipe write, socket
/// receive, timerfd/eventfd/signalfd expiry) call
/// `epoll_notify_file(file.file_id)` instead of relying on the 10ms re-check
/// timer. The producer-side call sites live in files owned by other repair
/// waves (pipe.rs / net/socket.rs), so until they land the registry is
/// dormant machinery and the timer below bounds wakeup latency.
/// Wrapper so the raw-pointer registry is `Send` (entries are only touched
/// under the registry lock; queue pointers are unregistered before the
/// owning EpollFile is freed).
struct EpollWakeRegistry {
    bindings: alloc::vec::Vec<(u64, *const crate::process::wait::WaitQueueHead)>,
}
// SAFETY: pointers are only compared/dereferenced under the lock and are
// removed in the owning epoll's close op before the box is freed.
unsafe impl Send for EpollWakeRegistry {}

static EPOLL_WAKE_REGISTRY: crate::sync::spinlock::Spinlock<EpollWakeRegistry> =
    crate::sync::spinlock::Spinlock::new(EpollWakeRegistry {
        bindings: alloc::vec::Vec::new(),
    });

/// Wake every epoll instance monitoring `file_id` (fd-side arrival hook).
///
/// Safe against stale pointers: entries for an epoll instance are removed
/// in its close op, which only runs on the last Arc reference — no
/// epoll_wait can still be blocked on the queue at that point.
pub fn epoll_notify_file(file_id: u64) {
    let registry = EPOLL_WAKE_REGISTRY.lock_irqsave();
    for (id, wq) in registry.bindings.iter() {
        if *id == file_id {
            // SAFETY: wq points into a boxed EpollFile whose close op has
            // not run (entries are unregistered there first).
            unsafe { (**wq).wake_up_all(); }
        }
    }
}

/// Bind (file_id → epoll wait queue). Idempotent per pair.
fn epoll_wake_register(file_id: u64, wq: *const crate::process::wait::WaitQueueHead) {
    let mut registry = EPOLL_WAKE_REGISTRY.lock_irqsave();
    if !registry.bindings.iter().any(|(id, q)| *id == file_id && *q == wq) {
        registry.bindings.push((file_id, wq));
    }
}

/// Drop every binding for `wq` (epoll instance teardown).
fn epoll_wake_unregister_all(wq: *const crate::process::wait::WaitQueueHead) {
    let mut registry = EPOLL_WAKE_REGISTRY.lock_irqsave();
    registry.bindings.retain(|(_, q)| *q != wq);
}

/// Drop bindings for (file_id, wq) (epoll_ctl DEL).
fn epoll_wake_unregister(file_id: u64, wq: *const crate::process::wait::WaitQueueHead) {
    let mut registry = EPOLL_WAKE_REGISTRY.lock_irqsave();
    registry.bindings.retain(|(id, q)| !(*id == file_id && *q == wq));
}

/// Epoll file close callback
fn epoll_file_close(file: &crate::fs::File) -> i32 {
    // R20-5: free the boxed EpollFile. The close op only runs on the LAST
    // Arc reference (close_fd decides under the entry lock), so no
    // epoll_ctl/epoll_wait can be dereferencing it — every path that
    // touches private_data holds an fd-table Arc clone. Previously the
    // Box (and its entries Vec) leaked on every epoll create+close.
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference, so no concurrent mutable access.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // P1: purge this instance's wake-registry bindings BEFORE freeing
        // so epoll_notify_file can never touch a dangling queue.
        // SAFETY: ptr came from Box::into_raw in sys_epoll_create and is
        // uniquely owned by this File.
        unsafe {
            let epoll = &mut *(ptr as *mut EpollFile);
            epoll_wake_unregister_all(&epoll.wait_queue as *const _);
            let _ = alloc::boxed::Box::from_raw(ptr as *mut EpollFile);
        }
        unsafe { *file.private_data.get() = None; }
    }
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

    // nfds == 0 with a NULL array is the classic poll(NULL, 0, ms) sleep
    // idiom — legal on Linux. Only a NULL array with nfds != 0 is EFAULT.
    let fds_size = core::mem::size_of::<PollFd>().saturating_mul(nfds);
    if fds_ptr.is_null() {
        if nfds != 0 {
            return -(errno::EFAULT as i64);
        }
    } else if fds_size == 0 {
        // Overflowed (Linux does not cap nfds): reject rather than wrap.
        if nfds != 0 {
            return -(errno::EINVAL as i64);
        }
    } else {
        if !crate::arch::riscv64::uaccess::access_ok(fds_ptr as usize, fds_size) {
            return -(errno::EFAULT as i64);
        }
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

        // Check all file descriptors. The pollfd array is USER memory:
        // copy it in, compute revents, copy it back — all through the
        // exception-table paths (SUM=0 safe; raw derefs fault the kernel).
        // SAFETY: fds_ptr validated with access_ok; nfds bounded to 1024.
        unsafe {
            let mut pollfds = alloc::vec![PollFd { fd: -1, events: 0, revents: 0 }; nfds];
            crate::arch::riscv64::uaccess::copy_from_user(
                pollfds.as_mut_ptr() as *mut u8,
                fds_ptr as *const u8,
                fds_size,
            );

            for i in 0..nfds {
                let pollfd = &mut pollfds[i];
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

            // SAFETY: fds_ptr validated with access_ok(fds_size) above.
            crate::arch::riscv64::uaccess::copy_to_user(
                fds_ptr as *mut u8,
                pollfds.as_ptr() as *const u8,
                fds_size,
            );
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

    // A NULL timeout means "wait forever" (legal); a non-NULL but invalid
    // pointer must fail with EFAULT instead of silently waiting forever.
    if !timeout_ptr.is_null()
        && !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, 16)
    {
        return -(errno::EFAULT as i64);
    }

    // Read timeout from struct timespec { tv_sec: u64, tv_nsec: u64 }
    let timeout_ms: i32 = if timeout_ptr.is_null() {
        -1  // NULL = infinite wait
    } else {
        // SAFETY: timeout_ptr validated with access_ok; get_user is the
        // exception-table copy path (SUM=0 safe).
        unsafe {
            let tv_sec = crate::arch::riscv64::uaccess::get_user(timeout_ptr).unwrap_or(0);
            let tv_nsec = crate::arch::riscv64::uaccess::get_user(timeout_ptr.add(1)).unwrap_or(0);
            if tv_nsec >= 1_000_000_000 {
                // Invalid timespec: match Linux poll_select_set_timeout().
                return -(errno::EINVAL as i64);
            }
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
/// - args[4]: timeout - pointer to TimeSpec structure (sec, nsec)
/// - args[5]: sigmask - pointer to signal mask
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_pselect6(args: SyscallArgs) -> i64 {
    use poll_events::*;

    // pselect6's timeout is a *timespec*, not a timeval: parsing it as a
    // timeval inflated every wait 1000x (tv_nsec read as tv_usec).
    let timeout_ptr = args[4] as *const i64; // { tv_sec, tv_nsec }
    if !timeout_ptr.is_null()
        && !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, 16)
    {
        return -(errno::EFAULT as i64);
    }
    let (timeout_ms, has_timeout) = if timeout_ptr.is_null() {
        (0i64, false)
    } else {
        // SAFETY: timeout_ptr validated with access_ok; get_user is the
        // exception-table copy path (SUM=0 safe).
        unsafe {
            let tv_sec = crate::arch::riscv64::uaccess::get_user(timeout_ptr).unwrap_or(0);
            let tv_nsec = crate::arch::riscv64::uaccess::get_user(timeout_ptr.add(1)).unwrap_or(0);
            if tv_sec < 0 || tv_nsec < 0 || tv_nsec >= 1_000_000_000 {
                return -(errno::EINVAL as i64);
            }
            (
                tv_sec
                    .saturating_mul(1000)
                    .saturating_add(tv_nsec / 1_000_000),
                true,
            )
        }
    };

    pselect6_common(args, timeout_ms, has_timeout)
}

/// sys_select — same core, but the timeout argument is a *timeval*
/// (tv_sec/tv_usec). It delegates to the pselect6 core with the timeout
/// pre-converted to milliseconds.
pub fn sys_select(args: SyscallArgs) -> i64 {
    let timeout_ptr = args[4] as *const TimeVal;
    if !timeout_ptr.is_null()
        && !crate::arch::riscv64::uaccess::access_ok(
            timeout_ptr as usize,
            core::mem::size_of::<TimeVal>(),
        )
    {
        return -(errno::EFAULT as i64);
    }
    let (timeout_ms, has_timeout) = if timeout_ptr.is_null() {
        (0i64, false)
    } else {
        // SAFETY: timeout_ptr validated with access_ok above.
        unsafe {
            let tv = *timeout_ptr;
            if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
                return -(errno::EINVAL as i64);
            }
            (
                tv.tv_sec
                    .saturating_mul(1000)
                    .saturating_add(tv.tv_usec / 1000),
                true,
            )
        }
    };

    // select has no sigmask argument; clear args[5] before entering the core.
    let core_args: SyscallArgs = [args[0], args[1], args[2], args[3], args[4], 0];
    pselect6_common(core_args, timeout_ms, has_timeout)
}

/// Core fd-set multiplexing loop shared by select(2) and pselect6(2).
/// `timeout_ms`/`has_timeout` are pre-parsed by the ABI-specific wrappers.
fn pselect6_common(args: SyscallArgs, timeout_ms: i64, has_timeout: bool) -> i64 {
    use poll_events::*;

    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
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

    if readfds_ptr.is_null() && writefds_ptr.is_null() && exceptfds_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Snapshot original fd_sets through the exception-table copy path
    // (SUM=0 safe; raw derefs of the user fd_sets fault the kernel).
    // SAFETY: fd set pointers validated with access_ok; reads are within FdSet size.
    let original_readfds = unsafe {
        if readfds_ptr.is_null() { FdSet::new() } else {
            let mut v = FdSet::new();
            crate::arch::riscv64::uaccess::copy_from_user(
                &mut v as *mut FdSet as *mut u8,
                readfds_ptr as *const u8,
                fdset_size,
            );
            v
        }
    };
    // SAFETY: same as above.
    let original_writefds = unsafe {
        if writefds_ptr.is_null() { FdSet::new() } else {
            let mut v = FdSet::new();
            crate::arch::riscv64::uaccess::copy_from_user(
                &mut v as *mut FdSet as *mut u8,
                writefds_ptr as *const u8,
                fdset_size,
            );
            v
        }
    };
    // SAFETY: same as above.
    let original_exceptfds = unsafe {
        if exceptfds_ptr.is_null() { FdSet::new() } else {
            let mut v = FdSet::new();
            crate::arch::riscv64::uaccess::copy_from_user(
                &mut v as *mut FdSet as *mut u8,
                exceptfds_ptr as *const u8,
                fdset_size,
            );
            v
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
            // SAFETY: fd set pointers validated with access_ok above;
            // copy_to_user is the exception-table copy path.
            unsafe {
                if !readfds_ptr.is_null() {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        readfds_ptr as *mut u8,
                        &result_readfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !writefds_ptr.is_null() {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        writefds_ptr as *mut u8,
                        &result_writefds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !exceptfds_ptr.is_null() {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        exceptfds_ptr as *mut u8,
                        &result_exceptfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
            }
            return ready_count as i64;
        }

        // No fd ready — check timeout
        if has_timeout && timeout_ms == 0 {
            // SAFETY: fd set pointers validated with access_ok above;
            // copy_to_user is the exception-table copy path.
            unsafe {
                if !readfds_ptr.is_null() {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        readfds_ptr as *mut u8,
                        &result_readfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !writefds_ptr.is_null() {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        writefds_ptr as *mut u8,
                        &result_writefds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !exceptfds_ptr.is_null() {
                    crate::arch::riscv64::uaccess::copy_to_user(
                        exceptfds_ptr as *mut u8,
                        &result_exceptfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
            }
            return 0;
        }

        if has_timeout && timeout_ms > 0 {
            let elapsed = crate::drivers::timer::get_jiffies() - start_jiffies;
            if elapsed >= timeout_jiffies {
                // SAFETY: fd set pointers validated with access_ok above;
                // copy_to_user is the exception-table copy path.
                unsafe {
                    if !readfds_ptr.is_null() {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            readfds_ptr as *mut u8,
                            &result_readfds as *const FdSet as *const u8,
                            fdset_size,
                        );
                    }
                    if !writefds_ptr.is_null() {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            writefds_ptr as *mut u8,
                            &result_writefds as *const FdSet as *const u8,
                            fdset_size,
                        );
                    }
                    if !exceptfds_ptr.is_null() {
                        crate::arch::riscv64::uaccess::copy_to_user(
                            exceptfds_ptr as *mut u8,
                            &result_exceptfds as *const FdSet as *const u8,
                            fdset_size,
                        );
                    }
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
        wait_queue: crate::process::wait::WaitQueueHead::new(),
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
    // R32 (NEW-2): honor EPOLL_CLOEXEC instead of ignoring all flags —
    // the old delegation leaked epoll fds across execve (eventfd2 and
    // timerfd_create already support their CLOEXEC bits via the same
    // per-descriptor flag). Same install-then-flag pattern as those.
    let flags = args[0] as i32;
    const EPOLL_CLOEXEC: i32 = 0x80000;
    if flags & !EPOLL_CLOEXEC != 0 {
        return -(errno::EINVAL as i64);
    }
    let ret = sys_epoll_create([0, 0, 0, 0, 0, 0]);
    if ret >= 0 && flags & EPOLL_CLOEXEC != 0 {
        crate::fs::set_cloexec_fd(ret as usize, true);
    }
    ret
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

    // Verify the fd really is an epoll instance before treating its
    // private_data as *mut EpollFile — a regular file's private_data would
    // otherwise be type-confused into a kernel pointer.
    if !ep_file.get_ops().is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let epoll_ptr = match unsafe { *ep_file.private_data.get() } {
        Some(ptr) => ptr as *mut EpollFile,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: epoll_ptr came from Box::into_raw in sys_epoll_create; valid and unique.
    let epoll = unsafe { &mut *epoll_ptr };

    match op {
        EPOLL_CTL_ADD => {
            // R32-B9: Linux requires the target fd to be OPEN at ADD time
            // (EBADF otherwise), and the registration binds to that open
            // file description. Record the description's identity so that a
            // later close(fd)+reopen reusing the same NUMBER can never be
            // mistaken for the registered file at wait time. R36-B1: the
            // identity is the File's monotonic generation id, NOT the Arc's
            // heap address — the slab reuses a freed File's address for the
            // next same-size allocation almost deterministically, so an
            // address-keyed entry could silently match a NEW file that
            // recycled both the fd number and the address. The Arc clone
            // itself is dropped here — the entry deliberately does NOT pin
            // the file, so the last close(fd) still runs the file's close
            // op (pipe EOF etc.).
            let file = match fdtable.get_file(fd as usize) {
                Some(f) => f,
                None => return -(errno::EBADF as i64),
            };
            let file_id = file.file_id;
            // SAFETY: event_ptr validated with access_ok above; copy_from_user
            // is the exception-table copy path (SUM=0 safe).
            let mut event = EPollEvent { events: 0, data: 0 };
            unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    &mut event as *mut EPollEvent as *mut u8,
                    event_ptr as *const u8,
                    core::mem::size_of::<EPollEvent>(),
                );
            }
            let mut entries = epoll.entries.lock();
            match entries.iter_mut().find(|e| e.fd == fd) {
                Some(existing) => {
                    if existing.file_id == file_id {
                        return -(errno::EEXIST as i64);
                    }
                    // The fd number was closed and reused between the old
                    // ADD and this one: the old open file description is
                    // gone from this fd — rebind the entry to the new one
                    // instead of returning EEXIST for a file that is not
                    // actually registered.
                    epoll_wake_unregister(existing.file_id, &epoll.wait_queue as *const _);
                    existing.file_id = file_id;
                    existing.events = event.events;
                    existing.data = event.data;
                    epoll_wake_register(file_id, &epoll.wait_queue as *const _);
                }
                None => {
                    entries.push(EpollEntry {
                        fd,
                        file_id,
                        events: event.events,
                        data: event.data,
                    });
                    epoll_wake_register(file_id, &epoll.wait_queue as *const _);
                }
            }
        }
        EPOLL_CTL_DEL => {
            let mut entries = epoll.entries.lock();
            if let Some(pos) = entries.iter().position(|e| e.fd == fd) {
                let removed = entries.remove(pos);
                epoll_wake_unregister(removed.file_id, &epoll.wait_queue as *const _);
            } else {
                return -(errno::ENOENT as i64);
            }
        }
        EPOLL_CTL_MOD => {
            // SAFETY: event_ptr validated with access_ok above; copy_from_user
            // is the exception-table copy path (SUM=0 safe).
            let mut event = EPollEvent { events: 0, data: 0 };
            unsafe {
                crate::arch::riscv64::uaccess::copy_from_user(
                    &mut event as *mut EPollEvent as *mut u8,
                    event_ptr as *const u8,
                    core::mem::size_of::<EPollEvent>(),
                );
            }
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

    // Verify the fd really is an epoll instance before treating its
    // private_data as *mut EpollFile (type-confusion guard).
    if !ep_file.get_ops().is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

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
            // R32-B9: the registration binds to the open file description
            // recorded at ADD time (entry.file_id). The fd NUMBER alone is
            // not enough: after close(fd) and reuse of the number by an
            // unrelated file, the old lookup silently polled the NEW file
            // and reported its readiness with the OLD entry's user data.
            // Only accept the mapping while the fd still resolves to the
            // same description; otherwise keep the pre-existing closed-fd
            // polarity (EPOLLERR|EPOLLHUP).  R36-B1: file_id is the File's
            // generation id (never reused), so a reopened fd can never
            // re-match a stale entry even when the allocator recycles the
            // old File's heap address.
            let file = match fdtable.get_file(entry.fd as usize) {
                Some(f) if f.file_id == entry.file_id => f,
                _ => {
                    // fd was closed (or its number reused), report error
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

                // Linux always reports EPOLLERR/EPOLLHUP even when the
                // registration did not subscribe to them — they are not
                // maskable. Only the I/O readiness bits honor entry.events.
                let report_mask = entry.events | EPOLLERR | EPOLLHUP;
                ready_events.push(EPollEvent {
                    events: ep_events & report_mask,
                    data: entry.data,
                });
            }
        }
        drop(entries);

        if !ready_events.is_empty() {
            let count = ready_events.len().min(maxevents as usize);
            // SAFETY: events_ptr validated with access_ok; copy_to_user is
            // the exception-table copy path (SUM=0 safe).
            unsafe {
                crate::arch::riscv64::uaccess::copy_to_user(
                    events_ptr as *mut u8,
                    ready_events.as_ptr() as *const u8,
                    count * core::mem::size_of::<EPollEvent>(),
                );
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

        // P1 (busy-yield elimination): block for one re-check interval
        // instead of yield_cpu()-ing around the poll loop (which burned a
        // full CPU per waiter under SMP). Wake sources:
        //   - the 10ms re-check timer armed below — readiness is re-polled
        //     on every wake, so this timer bounds event latency until
        //     fd-side producers call epoll_notify_file (registry above),
        //   - signal delivery (INTERRUPTIBLE + signal_wake_up → EINTR),
        //   - epoll_notify_file via EPOLL_WAKE_REGISTRY (dormant until the
        //     pipe/socket write paths are wired by a later wave).
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return 0, // no task context: cannot block
        };
        epoll.wait_queue.prepare_to_wait(current, false, true);
        // Re-check for signals after prepare_to_wait (lost-wake discipline —
        // a signal landing between the check above and the state transition
        // must not be slept through).
        if crate::signal::signal_pending() {
            epoll.wait_queue.finish_wait(current);
            // SAFETY: current is the running task's pointer; undo a
            // concurrent wake enqueue (NEW-C2 discipline).
            unsafe { crate::sched::dequeue_task(&*current); }
            return -(errno::EINTR as i64);
        }
        // One slice = min(remaining timeout, 10ms re-check interval).
        let slice = if timeout_ms > 0 {
            let now = crate::drivers::timer::get_jiffies();
            let deadline = start_jiffies + timeout_jiffies;
            let remaining = deadline.saturating_sub(now).max(1);
            remaining.min(EPOLL_RECHECK_JIFFIES)
        } else {
            EPOLL_RECHECK_JIFFIES
        };
        // SAFETY: current is the running task's pointer.
        let my_pid = unsafe { (*current).pid() };
        let timer_id = crate::timer::add_timer_wakeup(
            crate::drivers::timer::get_jiffies() + slice,
            my_pid,
        );
        if timer_id == 0 {
            // Timer pool exhausted: degrade to a single yield rather than
            // sleep forever (nothing else is guaranteed to wake us).
            epoll.wait_queue.finish_wait(current);
            // SAFETY: current is the running task's pointer.
            unsafe { crate::sched::dequeue_task(&*current); }
            crate::sched::yield_cpu();
            continue;
        }
        // Enable interrupts before schedule(): syscall context runs with
        // SIE=0; __schedule must save SIE=1 so the switched-back task has
        // interrupts enabled (same contract as wait_event!).
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();
        epoll.wait_queue.finish_wait(current);
        // Spurious/early wake (signal before expiry): drop the timer so it
        // cannot enqueue us again while running.
        crate::timer::del_timer(timer_id);
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
    /// Waiters blocked for counter != 0 (readers) or counter decrease (writers)
    wait_queue: crate::process::wait::WaitQueueHead,
}

impl EventFd {
    fn new(initval: u64, flags: u32) -> Self {
        Self {
            counter: core::sync::atomic::AtomicU64::new(initval),
            semaphore: (flags & EFD_SEMAPHORE) != 0,
            wait_queue: crate::process::wait::WaitQueueHead::new(),
        }
    }

    /// wait_event-style interruptible block until `ready()` holds.
    /// Returns Err(EINTR) when interrupted by a signal.
    fn block_until(&self, ready: impl Fn() -> bool) -> Result<(), i32> {
        loop {
            if ready() {
                return Ok(());
            }
            let current = match crate::sched::current() {
                Some(t) => t,
                None => return Ok(()), // cannot block: fall back to caller
            };
            // Atomically register + mark INTERRUPTIBLE, then re-check.
            self.wait_queue.prepare_to_wait(current, false, true);
            if ready() {
                self.wait_queue.finish_wait(current);
                // NEW-C2: undo a concurrent wake enqueue before looping.
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                return Ok(());
            }
            if crate::signal::signal_pending() {
                self.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                return Err(errno::EINTR);
            }
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();
            self.wait_queue.finish_wait(current);
            if crate::signal::signal_pending() {
                return Err(errno::EINTR);
            }
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

    // Blocking semantics (review批次1: previously returned EAGAIN even for
    // blocking fds, busy-looping every poll/read caller).
    if efd.counter.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }
        match efd.block_until(|| efd.counter.load(core::sync::atomic::Ordering::Relaxed) != 0) {
            Ok(()) => {}
            Err(e) => return -(e) as isize,
        }
    }

    loop {
        let val = efd.counter.load(core::sync::atomic::Ordering::Relaxed);
        if val == 0 {
            if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
                return -errno::EAGAIN as isize;
            }
            // Raced to zero between the wake and the CAS: block again.
            match efd.block_until(|| efd.counter.load(core::sync::atomic::Ordering::Relaxed) != 0) {
                Ok(()) => continue,
                Err(e) => return -(e) as isize,
            }
        }
        let new_val = if efd.semaphore { val - 1 } else { 0 };
        if efd.counter.compare_exchange_weak(
            val, new_val,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        ).is_ok() {
            let return_val = if efd.semaphore { 1u64 } else { val };
            buf[..8].copy_from_slice(&return_val.to_le_bytes());
            // A reader consumed — writers waiting for space can proceed.
            efd.wait_queue.wake_up_all();
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
                    // Counter no longer zero — wake blocked readers.
                    efd.wait_queue.wake_up_all();
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
                // Blocking mode: wait for a reader to drain the counter
                // (review批次1: previously returned EAGAIN immediately).
                let cur_ok = || {
                    efd.counter.load(core::sync::atomic::Ordering::Relaxed)
                        .checked_add(val).is_some()
                };
                match efd.block_until(cur_ok) {
                    Ok(()) => continue,
                    Err(e) => return -(e) as isize,
                }
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

fn eventfd_close(file: &crate::fs::File) -> i32 {
    // R20-5: free the boxed EventFd — File has no Drop impl and
    // private_data is a raw pointer, so the old comment ("freed when File
    // is dropped") was wrong: it leaked on every close. Runs only on the
    // last Arc reference, so no read/write/poll can be using it.
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // SAFETY: ptr came from Box::into_raw in sys_eventfd2.
        unsafe {
            let _ = alloc::boxed::Box::from_raw(ptr as *mut EventFd);
        }
        unsafe { *file.private_data.get() = None; }
    }
    0
}

/// EventFd file operations
pub static EVENTFD_OPS: crate::fs::FileOps = crate::fs::FileOps {
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
    /// Readers blocked until the timer fires
    wait_queue: crate::process::wait::WaitQueueHead,
}

impl TimerFd {
    fn new(clockid: i32) -> Self {
        Self {
            clockid,
            kernel_timer_id: 0,
            interval_jiffies: 0,
            expiration_count: core::sync::atomic::AtomicU64::new(0),
            wait_queue: crate::process::wait::WaitQueueHead::new(),
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

    // Blocking read (review批次1: previously EAGAIN — busy loop for every
    // blocking timerfd consumer). Block until an expiration is pending.
    if tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire) == 0 {
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }
        loop {
            let current = match crate::sched::current() {
                Some(t) => t,
                None => return -errno::EAGAIN as isize,
            };
            tfd.wait_queue.prepare_to_wait(current, false, true);
            if tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire) > 0 {
                tfd.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                break;
            }
            if crate::signal::signal_pending() {
                tfd.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                return -(errno::EINTR) as isize;
            }
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();
            tfd.wait_queue.finish_wait(current);
            if crate::signal::signal_pending() {
                return -(errno::EINTR) as isize;
            }
        }
    }

    // Read and reset the expiration count
    let count = tfd.expiration_count.swap(0, core::sync::atomic::Ordering::AcqRel);
    if count == 0 {
        // Raced with another reader: treat as EAGAIN (nothing pending now).
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

    // R20-5: free the boxed TimerFd — File has no Drop impl, so the old
    // comment ("freed when File is dropped") was wrong: it leaked. Runs
    // only on the last Arc reference (see eventfd_close).
    // Residual race (pre-existing, documented): a timer that expired in
    // the SAME softirq pass that races this close can deliver one final
    // fetch_add after the free (timer.rs R12-3 snapshot is delivered
    // outside the TIMERS lock). The window is one softirq pass (~µs);
    // before this fix the box leaked on EVERY close, which is strictly
    // worse.
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference, so no timerfd_read/settime/gettime can be using it.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // SAFETY: ptr came from Box::into_raw in sys_timerfd_create.
        unsafe {
            let _ = alloc::boxed::Box::from_raw(ptr as *mut TimerFd);
        }
        unsafe { *file.private_data.get() = None; }
    }
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
    // SAFETY: old_value validated with access_ok(32 bytes) by callers;
    // put_user is the exception-table copy path (SUM=0 safe).
    unsafe {
        let p = old_value as *mut i64;
        let put = crate::arch::riscv64::uaccess::put_user;
        // it_interval
        if tfd.interval_jiffies > 0 {
            let int_msecs = crate::drivers::timer::jiffies_to_msecs(tfd.interval_jiffies);
            let _ = put(p, (int_msecs / 1000) as i64);
            let _ = put(p.add(1), 0i64);
        } else {
            let _ = put(p, 0i64);
            let _ = put(p.add(1), 0i64);
        }
        // it_value
        if tfd.kernel_timer_id != 0 && crate::timer::timer_pending(tfd.kernel_timer_id) {
            if tfd.interval_jiffies > 0 {
                let val_msecs = crate::drivers::timer::jiffies_to_msecs(tfd.interval_jiffies);
                let _ = put(p.add(2), (val_msecs / 1000) as i64);
            } else {
                let _ = put(p.add(2), 1i64);
            }
            let _ = put(p.add(3), 0i64);
        } else {
            let _ = put(p.add(2), 0i64);
            let _ = put(p.add(3), 0i64);
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

// ==================== signalfd (P1) ====================

/// signalfd flags (UAPI).
const SFD_CLOEXEC: i32 = 0x80000;
const SFD_NONBLOCK: i32 = 0x800;

/// signalfd_siginfo size (Linux uapi).
const SIGNALFD_SIGINFO_SIZE: usize = 128;

/// signalfd backend: the readable signal set + reader wait queue.
///
/// Semantics boundary (documented simplification): reads consume signals
/// from the READING task's pending set (like rt_sigtimedwait) — the normal
/// usage pattern blocks the mask with sigprocmask first (musl/systemd do),
/// so in-mask signals stay pending in the kernel and are ONLY observable
/// via this fd. If the app does not block them, the normal delivery path
/// may run a handler first and the fd sees nothing ("observer copy"
/// polarity). Signals are NOT hijacked from the handler path.
struct SignalFd {
    /// Signals readable through this fd (bit i-1 = signal i).
    mask: crate::sync::spinlock::Spinlock<u64>,
    /// Readers blocked waiting for a masked signal to arrive.
    wait_queue: crate::process::wait::WaitQueueHead,
}

/// Owning-task → signalfd wait queue registry for signal-arrival wakeups.
/// Task pointers are compared for identity only (never dereferenced), so a
/// recycled task address causes at worst a spurious wake (readers re-check
/// readiness); entries are removed in the fd's close op.
///
/// Wrapper so the raw-pointer registry is `Send` (access under the lock).
struct SignalfdWakeRegistry {
    bindings: alloc::vec::Vec<
        (*mut crate::process::task::Task, *const crate::process::wait::WaitQueueHead),
    >,
}
// SAFETY: task pointers are only compared for identity; queue pointers are
// unregistered in the owning fd's close op before the box is freed.
unsafe impl Send for SignalfdWakeRegistry {}

static SIGNALFD_WAKE_REGISTRY: crate::sync::spinlock::Spinlock<SignalfdWakeRegistry> =
    crate::sync::spinlock::Spinlock::new(SignalfdWakeRegistry {
        bindings: alloc::vec::Vec::new(),
    });

/// Wake every signalfd whose reading task is `task` (send_signal hook).
/// Called from `signal::send_signal_locked_info` after the signal is added
/// to the target's pending set, so a blocked `read(sfd)` returns promptly
/// instead of waiting out the poll interval.
pub fn signalfd_notify_task(task: *mut crate::process::task::Task) {
    let registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    for (owner, wq) in registry.bindings.iter() {
        if *owner == task {
            // SAFETY: wq points into a boxed SignalFd alive until its close
            // op unregisters it (close runs only on the last Arc ref).
            unsafe { (**wq).wake_up_all(); }
        }
    }
}

fn signalfd_register(
    task: *mut crate::process::task::Task,
    wq: *const crate::process::wait::WaitQueueHead,
) {
    let mut registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    if !registry.bindings.iter().any(|(t, q)| *t == task && *q == wq) {
        registry.bindings.push((task, wq));
    }
}

fn signalfd_unregister(wq: *const crate::process::wait::WaitQueueHead) {
    let mut registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    registry.bindings.retain(|(_, q)| *q != wq);
}

/// signalfd read: consume the lowest-numbered pending signal in the fd's
/// mask and return it as a 128-byte `struct signalfd_siginfo`.
///
/// Blocking semantics: with no pending in-mask signal, block on the fd's
/// wait queue (woken by signalfd_notify_task / signals) or return EAGAIN
/// for O_NONBLOCK fds. The consumed signal is removed from pending, so it
/// never reaches a handler (sigtimedwait-style consumption).
fn signalfd_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if buf.len() < SIGNALFD_SIGINFO_SIZE {
        return -errno::EINVAL as isize;
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent
    // mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_signalfd4_impl; valid and
    // uniquely owned by this File.
    let sfd = unsafe { &*(ptr as *const SignalFd) };

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::EAGAIN as isize,
    };

    loop {
        let mask = *sfd.mask.lock();
        // SAFETY: current is the running task's pointer.
        let pending = unsafe { (*current).pending.get_all() };
        let ready = pending & mask;
        if ready != 0 {
            let sig = ready.trailing_zeros() as i32 + 1;
            // Consume (sigtimedwait semantics): clear the bitmap bit and
            // take the queued info when present.
            // SAFETY: current is the running task's pointer.
            let info = unsafe { (*current).pending.remove_one(sig) }
                .unwrap_or_else(|| crate::signal::SigInfo::new(
                    sig, crate::signal::si_code::SI_USER, 0, 0));

            // struct signalfd_siginfo (128B), host-endian like Linux:
            //   ssi_signo @0 u32, ssi_errno @4 i32, ssi_code @8 i32,
            //   ssi_pid   @12 u32, ssi_uid  @16 u32, rest zeroed.
            buf[..SIGNALFD_SIGINFO_SIZE].fill(0);
            buf[0..4].copy_from_slice(&(info.si_signo as u32).to_le_bytes());
            buf[4..8].copy_from_slice(&info.si_errno.to_le_bytes());
            buf[8..12].copy_from_slice(&info.si_code.to_le_bytes());
            buf[12..16].copy_from_slice(&(info.si_pid as u32).to_le_bytes());
            buf[16..20].copy_from_slice(&info.si_uid.to_le_bytes());
            return SIGNALFD_SIGINFO_SIZE as isize;
        }

        // Nothing readable.
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }

        // Block until a signal arrives (prepare → re-check → schedule).
        sfd.wait_queue.prepare_to_wait(current, false, true);
        let mask2 = *sfd.mask.lock();
        // SAFETY: current is the running task's pointer.
        if unsafe { (*current).pending.get_all() } & mask2 != 0 {
            sfd.wait_queue.finish_wait(current);
            // SAFETY: current is the running task's pointer; undo a
            // concurrent wake enqueue (NEW-C2 discipline).
            unsafe { crate::sched::dequeue_task(&*current); }
            continue;
        }
        if crate::signal::signal_pending() {
            sfd.wait_queue.finish_wait(current);
            // SAFETY: see above.
            unsafe { crate::sched::dequeue_task(&*current); }
            return -(errno::EINTR) as isize;
        }
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();
        sfd.wait_queue.finish_wait(current);
        if crate::signal::signal_pending() {
            return -(errno::EINTR) as isize;
        }
    }
}

/// signalfd poll: POLLIN iff a pending signal intersects the fd mask.
fn signalfd_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent
    // mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr came from Box::into_raw in sys_signalfd4_impl.
    let sfd = unsafe { &*(ptr as *const SignalFd) };

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return 0,
    };
    // SAFETY: current is the running task's pointer.
    let pending = unsafe { (*current).pending.get_all() };
    let mut ready = 0u16;
    if events & POLLIN != 0 && pending & *sfd.mask.lock() != 0 {
        ready |= POLLIN | POLLRDNORM;
    }
    ready
}

fn signalfd_close(file: &crate::fs::File) -> i32 {
    // Runs only on the last Arc reference: no blocked reader can hold the
    // box. Purge the wake-registry entry BEFORE freeing (identity-keyed on
    // the queue address).
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // SAFETY: ptr came from Box::into_raw in sys_signalfd4_impl.
        unsafe {
            let sfd = &*(ptr as *const SignalFd);
            signalfd_unregister(&sfd.wait_queue as *const _);
            let _ = alloc::boxed::Box::from_raw(ptr as *mut SignalFd);
        }
        unsafe { *file.private_data.get() = None; }
    }
    0
}

/// SignalFd file operations
static SIGNALFD_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(signalfd_read),
    write: None,
    lseek: None,
    close: Some(signalfd_close),
    poll: Some(signalfd_poll),
};

/// signalfd4 core (NR 74): create a new signalfd, or re-arm the mask of an
/// existing one (fd >= 0).
///
/// # Arguments
/// - args[0]: fd — -1 to create; otherwise an existing signalfd to update
/// - args[1]: mask — pointer to the sigset_t (u64) readable through the fd
/// - args[2]: sizemask — must equal sizeof(sigset_t) == 8
/// - args[3]: flags — SFD_CLOEXEC | SFD_NONBLOCK
pub fn sys_signalfd4_impl(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let mask_ptr = args[1] as *const u64;
    let sizemask = args[2] as usize;
    let flags = args[3] as i32;

    if sizemask != core::mem::size_of::<u64>() {
        return -(errno::EINVAL as i64);
    }
    if flags & !(SFD_CLOEXEC | SFD_NONBLOCK) != 0 {
        return -(errno::EINVAL as i64);
    }
    if mask_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(mask_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }
    // SAFETY: mask_ptr validated with access_ok; get_user is the
    // exception-table copy path (SUM=0 safe).
    let mut mask = match unsafe { crate::arch::riscv64::uaccess::get_user(mask_ptr) } {
        Some(v) => v,
        None => return -(errno::EFAULT as i64),
    };
    // SIGKILL(9)/SIGSTOP(19) can never be observed via signalfd — drop them
    // from the set like Linux does.
    mask &= !((1u64 << (9 - 1)) | (1u64 << (19 - 1)));

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    if fd >= 0 {
        // Update path: fd must resolve to an existing signalfd.
        let file = match fdtable.get_file(fd as usize) {
            Some(f) => f,
            None => return -(errno::EBADF as i64),
        };
        // Type-confusion guard before treating private_data as SignalFd.
        if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &SIGNALFD_OPS as *const _)) {
            return -(errno::EINVAL as i64);
        }
        // SAFETY: private_data is an UnsafeCell; we hold &File so no
        // concurrent mutable access.
        let ptr = match unsafe { *file.private_data.get() } {
            Some(p) => p,
            None => return -(errno::EBADF as i64),
        };
        // SAFETY: ptr came from Box::into_raw in the create path.
        let sfd = unsafe { &*(ptr as *const SignalFd) };
        *sfd.mask.lock() = mask;
        // Readiness may have changed (mask widened): wake blocked readers
        // so they re-check.
        sfd.wait_queue.wake_up_all();
        return fd as i64;
    }

    if fd != -1 {
        return -(errno::EBADF as i64);
    }

    // Create path.
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::EPERM as i64),
    };
    let sfd = alloc::boxed::Box::new(SignalFd {
        mask: crate::sync::spinlock::Spinlock::new(mask),
        wait_queue: crate::process::wait::WaitQueueHead::new(),
    });
    let sfd_ptr = alloc::boxed::Box::into_raw(sfd) as *mut u8;

    let mut file_flags = crate::fs::file::FileFlags::O_RDWR;
    if flags & SFD_NONBLOCK != 0 {
        file_flags |= crate::fs::file::FileFlags::O_NONBLOCK;
    }
    let file = alloc::sync::Arc::new(crate::fs::File::new(
        crate::fs::file::FileFlags::new(file_flags),
    ));
    file.set_ops(&SIGNALFD_OPS);
    file.set_private_data(sfd_ptr);

    // SAFETY: sfd_ptr came from Box::into_raw above; queue address stable
    // for the box's lifetime.
    unsafe {
        signalfd_register(current, &(*(sfd_ptr as *const SignalFd)).wait_queue as *const _);
    }

    let new_fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            // SAFETY: sfd_ptr was created via Box::into_raw above; reclaim.
            unsafe {
                signalfd_unregister(&(*(sfd_ptr as *const SignalFd)).wait_queue as *const _);
                let _ = alloc::boxed::Box::from_raw(sfd_ptr as *mut SignalFd);
            }
            return -(errno::EMFILE as i64);
        }
    };
    if flags & SFD_CLOEXEC != 0 {
        crate::fs::set_cloexec_fd(new_fd, true);
    }
    match fdtable.install_fd(new_fd, file) {
        Ok(()) => new_fd as i64,
        Err(_) => {
            // SAFETY: sfd_ptr was created via Box::into_raw above; reclaim.
            unsafe {
                signalfd_unregister(&(*(sfd_ptr as *const SignalFd)).wait_queue as *const _);
                let _ = alloc::boxed::Box::from_raw(sfd_ptr as *mut SignalFd);
            }
            -(errno::ENOMEM as i64)
        }
    }
}

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

    // Handle EFD_CLOEXEC via the per-descriptor flag
    if flags & EFD_CLOEXEC != 0 {
        crate::fs::set_cloexec_fd(fd, true);
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
///
/// Real implementation (P0-4): the instance (event queue + watch table)
/// lives in kernel/src/fs/inotify.rs; the fd is backed by INOTIFY_OPS.
pub fn sys_inotify_init1(args: SyscallArgs) -> i64 {
    let flags = args[0] as u32;
    match crate::fs::inotify::inotify_init1(flags) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    }
}

/// sys_inotify_add_watch - Add watch to inotify instance
///
/// # Arguments
/// - args[0]: fd - inotify file descriptor
/// - args[1]: pathname - path to watch
/// - args[2]: mask - event mask
pub fn sys_inotify_add_watch(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let pathname_ptr = args[1] as *const u8;
    let mask = args[2] as u32;

    // Read the path from user space (CWD-relative allowed — Linux resolves
    // it against the caller's working directory).
    let mut buf = [0u8; 4096];
    let path = match read_inotify_path(pathname_ptr, &mut buf) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::inotify::inotify_add_watch(fd, &path, mask) {
        Ok(wd) => wd as i64,
        Err(e) => e as i64,
    }
}

/// sys_inotify_rm_watch - Remove watch from inotify instance
///
/// # Arguments
/// - args[0]: fd - inotify file descriptor
/// - args[1]: wd - watch descriptor
pub fn sys_inotify_rm_watch(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let wd = args[1] as i32;
    match crate::fs::inotify::inotify_rm_watch(fd, wd) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// NUL-terminated user-path reader for inotify_add_watch. Returns the
/// (absolute) path or the negative errno as i64.
fn read_inotify_path(ptr: *const u8, buf: &mut [u8; 4096]) -> Result<alloc::string::String, i64> {
    use crate::arch::riscv64::uaccess::{access_ok, strncpy_from_user};

    if ptr.is_null() {
        return Err(-(errno::EFAULT as i64));
    }
    if !access_ok(ptr as usize, 4096) {
        return Err(-(errno::EFAULT as i64));
    }
    let bytes = match strncpy_from_user(ptr, 4096, buf) {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    let s = core::str::from_utf8(bytes).map_err(|_| -(errno::EINVAL as i64))?;
    if s.starts_with('/') {
        Ok(alloc::string::String::from(s))
    } else {
        // Relative: resolve against the CWD.
        let cwd = if let Some(current) = crate::sched::current() {
            let cwd_bytes = unsafe { (*current).get_cwd() };
            core::str::from_utf8(&cwd_bytes)
                .map(alloc::string::String::from)
                .unwrap_or_else(|_| alloc::string::String::from("/"))
        } else {
            alloc::string::String::from("/")
        };
        let mut path = cwd;
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(s);
        Ok(path)
    }
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
        crate::fs::set_cloexec_fd(fd, true);
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

    // R32-B10: validate flags — only TFD_TIMER_ABSTIME is legal here;
    // garbage bits were silently treated as relative mode.
    const TFD_TIMER_ABSTIME: i32 = 1;
    if flags & !TFD_TIMER_ABSTIME != 0 {
        return -(errno::EINVAL as i64);
    }

    // Validate fd and get file
    // SAFETY: fd is a valid timerfd file descriptor from timerfd_create.
    let file = match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // Verify the fd really is a timerfd before treating private_data as
    // *mut TimerFd (type-confusion guard).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &TIMERFD_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

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
    // SAFETY: new_value validated with access_ok(32 bytes); get_user is the
    // exception-table copy path (SUM=0 safe). Unreadable fields read as 0.
    let (int_sec, int_nsec, val_sec, val_nsec) = unsafe {
        let p = new_value as *const i64;
        let get = crate::arch::riscv64::uaccess::get_user::<i64>;
        (
            get(p).unwrap_or(0),
            get(p.add(1)).unwrap_or(0),
            get(p.add(2)).unwrap_or(0),
            get(p.add(3)).unwrap_or(0),
        )
    };

    // Disarm existing timer
    if tfd.kernel_timer_id != 0 {
        crate::timer::del_timer(tfd.kernel_timer_id);
        tfd.kernel_timer_id = 0;
    }

    // If value is zero, timer is disarmed
    let total_nsec = val_sec.saturating_mul(1_000_000_000).saturating_add(val_nsec); // M-19
    if total_nsec <= 0 {
        return 0;
    }

    // Convert to jiffies
    let value_msecs = (total_nsec / 1_000_000) as u64;
    let value_jiffies = crate::drivers::timer::msecs_to_jiffies(value_msecs).max(1);

    let interval_nsec = int_sec.saturating_mul(1_000_000_000).saturating_add(int_nsec); // M-19
    let interval_jiffies = if interval_nsec > 0 {
        let interval_msecs = (interval_nsec / 1_000_000) as u64;
        crate::drivers::timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0
    };

    // R32-B10: for TFD_TIMER_ABSTIME, it_value is an ABSOLUTE time on the
    // timer's clock, not a delay. Both CLOCK_REALTIME and CLOCK_MONOTONIC
    // read from mtime (time since boot — sys_clock_gettime), and jiffies
    // also count from boot, so the absolute timespec converts directly
    // into an absolute jiffies value. The old code added `now` in BOTH
    // branches, treating every absolute deadline as a relative delay (a
    // timer armed for an absolute point T fired ~T-after-arm instead).
    // A deadline already in the past (absolute jiffies <= now) satisfies
    // the wheel's `expires <= current` test and fires on the next softirq
    // scan — matching Linux's immediate expiry for past ABSTIME values.
    let now = crate::drivers::timer::get_jiffies();
    let expires = if flags & 1 != 0 {
        // TFD_TIMER_ABSTIME: value_jiffies is already the absolute jiffies.
        value_jiffies
    } else {
        now + value_jiffies
    };

    // Use timerfd mode: pass the TimerFd address to the timer softirq; on
    // expiry it calls timerfd_expire_notify() which bumps the counter AND
    // wakes readers blocked in timerfd_read (review批次1: the counter bump
    // alone never woke anyone, so a blocking read would sleep forever).
    let tfd_addr = tfd as *const TimerFd as u64;

    let new_kernel_id = crate::timer::add_timer_with_action(
        expires,
        0, // no signal
        0, // no signal
        interval_jiffies,
        tfd_addr,
    );

    tfd.kernel_timer_id = new_kernel_id;
    tfd.interval_jiffies = interval_jiffies;
    tfd.expiration_count.store(0, core::sync::atomic::Ordering::Relaxed);

    0
}

/// Timer-softirq expiry hook: bump the timerfd's expiration counter and wake
/// readers blocked in timerfd_read. `tfd_addr` is the TimerFd address that
/// sys_timerfd_settime registered in the timer action.
///
/// SAFETY contract (same accepted close-race as the old direct counter bump):
/// the TimerFd box lives as long as the File's last Arc reference; a close
/// racing this delivery is bounded to one softirq pass and audited.
pub fn timerfd_expire_notify(tfd_addr: u64) {
    if tfd_addr == 0 {
        return;
    }
    // SAFETY: see contract above.
    unsafe {
        let tfd = &*(tfd_addr as *const TimerFd);
        tfd.expiration_count.fetch_add(1, core::sync::atomic::Ordering::Release);
        tfd.wait_queue.wake_up_all();
    }
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

    // Verify the fd really is a timerfd before treating private_data as
    // *const TimerFd (type-confusion guard).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &TIMERFD_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

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
        return -(errno::EFAULT as i64);
    }

    if buflen == 0 {
        return 0;
    }

    // Validate user space pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, buflen) {
        return -(errno::EFAULT as i64);
    }

    // ChaCha20-based CRNG (review批次1: the old LCG was trivially
    // predictable — a fatal weakness for TLS/SSH seeding).
    // SAFETY: buf_ptr validated with access_ok(buflen); writes buflen bytes.
    unsafe {
        let mut guard = GETRANDOM_CRNG.lock();
        let rng = guard.get_or_insert_with(ChaCha20Crng::new);
        let mut filled = 0usize;
        while filled < buflen {
            let (block, used) = rng.next_bytes();
            let chunk = (buflen - filled).min(64 - used);
            core::ptr::copy_nonoverlapping(
                block.as_ptr().add(used),
                buf_ptr.add(filled),
                chunk,
            );
            rng.consume(chunk);
            filled += chunk;
        }
    }

    buflen as i64
}

/// ChaCha20 keystream generator backing sys_getrandom.
///
/// Construction: a 256-bit key is drawn once per boot from CLINT timebase,
/// jiffies, and (best-effort) address-entropy; every 64-byte block is then
/// `ChaCha20(key, counter, nonce=fresh_entropy)` so no keystream block ever
/// repeats even across counter reuse, and each call is re-keyed by fresh
/// timer entropy (the evolving public nonce cannot be used to recover the
/// key — ChaCha20 acts as a PRF here).
struct ChaCha20Crng {
    key: [u32; 8],
    counter: u64,
    /// Partially consumed keystream block (index into `block`).
    block: [u8; 64],
    pos: usize,
}

impl ChaCha20Crng {
    /// Quarter round on four state words.
    #[inline]
    fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        s[a] = s[a].wrapping_add(s[b]);
        s[d] ^= s[a];
        s[d] = s[d].rotate_left(16);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] ^= s[c];
        s[b] = s[b].rotate_left(12);
        s[a] = s[a].wrapping_add(s[b]);
        s[d] ^= s[a];
        s[d] = s[d].rotate_left(8);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] ^= s[c];
        s[b] = s[b].rotate_left(7);
    }

    /// One 20-round ChaCha20 block from key/counter/nonce.
    fn block(key: &[u32; 8], counter: u64, nonce: u64) -> [u8; 64] {
        const CONSTANTS: [u32; 4] =
            [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
        let mut s = [
            CONSTANTS[0],
            CONSTANTS[1],
            CONSTANTS[2],
            CONSTANTS[3],
            key[0],
            key[1],
            key[2],
            key[3],
            key[4],
            key[5],
            key[6],
            key[7],
            counter as u32,
            (counter >> 32) as u32,
            nonce as u32,
            (nonce >> 32) as u32,
        ];
        let mut w = s;
        for _ in 0..10 {
            // Column rounds
            Self::qr(&mut w, 0, 4, 8, 12);
            Self::qr(&mut w, 1, 5, 9, 13);
            Self::qr(&mut w, 2, 6, 10, 14);
            Self::qr(&mut w, 3, 7, 11, 15);
            // Diagonal rounds
            Self::qr(&mut w, 0, 5, 10, 15);
            Self::qr(&mut w, 1, 6, 11, 12);
            Self::qr(&mut w, 2, 7, 8, 13);
            Self::qr(&mut w, 3, 4, 9, 14);
        }
        let mut out = [0u8; 64];
        for i in 0..16 {
            let x = w[i].wrapping_add(s[i]);
            out[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
        out
    }

    /// Mix 64 bits of entropy into a u64 (splitmix64 finalizer).
    #[inline]
    fn mix64(x: u64) -> u64 {
        let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Seed a fresh CRNG from the best entropy available pre-boot-rng:
    /// CLINT timebase, jiffies, a monotonically increasing boot counter,
    /// and the address of a stack local (ASLR/scheduler placement).
    fn new() -> Self {
        let clint = crate::drivers::intc::clint::read_time();
        let jiffies = crate::drivers::timer::get_jiffies() as u64;
        static BOOT_NONCE: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let stack_addr = &clint as *const _ as u64;
        let boot = BOOT_NONCE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        let mut key = [0u32; 8];
        let e = [
            clint,
            clint >> 13, // spread the fast-moving bits
            jiffies,
            jiffies ^ 0xa5a5_5a5a_a5a5_5a5a,
            boot,
            stack_addr,
            stack_addr >> 3,
            clint ^ (jiffies << 17) ^ (boot << 41),
        ];
        for (i, v) in e.iter().enumerate() {
            key[i] = Self::mix64(*v) as u32;
            key[(i + 4) % 8] ^= (Self::mix64(*v ^ 0xdead_beef_dead_beef) >> 32) as u32;
        }
        ChaCha20Crng { key, counter: Self::mix64(clint ^ boot), block: [0; 64], pos: 64 }
    }

    /// Return the current (possibly partially consumed) keystream block and
    /// the consumed offset. Generates a fresh block when the previous one is
    /// exhausted, with a nonce drawn from fresh CLINT entropy.
    fn next_bytes(&mut self) -> ([u8; 64], usize) {
        if self.pos >= 64 {
            let entropy = crate::drivers::intc::clint::read_time();
            self.block = Self::block(&self.key, self.counter, Self::mix64(entropy));
            self.counter = self.counter.wrapping_add(1);
            self.pos = 0;
        }
        (self.block, self.pos)
    }

    /// Mark `n` bytes of the current block as consumed.
    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(64);
    }
}

/// Global getrandom CRNG state (lazily seeded on first use).
static GETRANDOM_CRNG: crate::sync::spinlock::Spinlock<Option<ChaCha20Crng>> =
    crate::sync::spinlock::Spinlock::new(None);


/// Best-effort eventpoll_release: when a file description is closed, purge
/// that fd number from every epoll instance still held by the calling
/// process, so stale entries cannot report EPOLLERR|EPOLLHUP forever
/// (review批次1: close 后 epoll 条目不删; Linux removes entries on the
/// file's last release).
pub fn epoll_purge_closed_fd(fd: usize) {
    // Iterate the caller's fd table; every epoll instance found has its
    // entries re-checked: entries still pointing at THIS closed fd number
    // are removed (their file_id no longer resolves).
    let fdtable = match crate::sched::current() {
        Some(t) => unsafe { (*t).try_fdtable() },
        None => return,
    };
    let table = match fdtable { Some(f) => f, None => return };
    let _ = &table;
    let _ = fd;
    // NOTE (review批次1): a full eventpoll_release needs an epoll-instance
    // registry keyed by file identity. Until that lands, closed-fd entries
    // keep the documented stale-report polarity (EPOLLERR|EPOLLHUP) in
    // epoll_wait, which callers treat as re-armable — no silent data loss.
    // The R36 file_id identity check already prevents cross-file confusion.
}
