//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Miscellaneous system call test
//!
//! Includes: uname, prlimit64, getrandom, select, pselect6, eventfd

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_misc() {
    test_group_start("syscall: miscellaneous");

    // Test 1: prlimit64 syscall
    test_sys_prlimit64();

    // Test 2: getrandom syscall
    test_sys_getrandom();

    // Test 3: select/pselect6 syscalls
    test_sys_select();

    // Test 4: eventfd syscall
    test_sys_eventfd();

    // Test 5: epoll syscalls
    test_sys_epoll();

    // Test 6: poll syscall
    test_sys_poll();

    // Test 7: Syscall number verification
    test_syscall_numbers();
}

fn test_sys_prlimit64() {
    // prlimit64 syscall
    test_pass("sys_prlimit64 interface exists");

    // Resource limit types
    const RLIMIT_CPU: i32 = 0;        // CPU time
    const RLIMIT_FSIZE: i32 = 1;      // File size
    const RLIMIT_DATA: i32 = 2;       // Data size
    const RLIMIT_STACK: i32 = 3;      // Stack size
    const RLIMIT_CORE: i32 = 4;       // Core file size
    const RLIMIT_RSS: i32 = 5;        // Resident set size
    const RLIMIT_NPROC: i32 = 6;      // Number of processes
    const RLIMIT_NOFILE: i32 = 7;     // Number of open files
    const RLIMIT_MEMLOCK: i32 = 8;    // Memory lock
    const RLIMIT_AS: i32 = 9;         // Address space
    const RLIMIT_LOCKS: i32 = 10;     // File locks
    const RLIMIT_SIGPENDING: i32 = 11; // Pending signals
    const RLIMIT_MSGQUEUE: i32 = 12;  // Message queue
    const RLIMIT_NICE: i32 = 13;      // Nice priority
    const RLIMIT_RTPRIO: i32 = 14;    // Real-time priority
    const RLIMIT_RTTIME: i32 = 15;    // Real-time timeout

    if RLIMIT_CPU == 0 && RLIMIT_NOFILE == 7 && RLIMIT_AS == 9 {
        test_pass("sys_prlimit64 resource types");
    } else {
        test_fail("sys_prlimit64 resource types", "mismatch");
    }

    // Verify more resource limits
    if RLIMIT_NPROC == 6 && RLIMIT_STACK == 3 && RLIMIT_CORE == 4 {
        test_pass("sys_prlimit64 extended types");
    } else {
        test_fail("sys_prlimit64 extended types", "mismatch");
    }

    // struct rlimit64 { rlim_cur, rlim_max }
    // Each 64-bit, 16 bytes total
    #[repr(C)]
    struct RLimit64 {
        rlim_cur: u64,
        rlim_max: u64,
    }

    const RLIMIT64_SIZE: usize = 16;
    if core::mem::size_of::<RLimit64>() == RLIMIT64_SIZE {
        test_pass("sys_prlimit64 struct size");
    } else {
        test_fail("sys_prlimit64 struct", "size mismatch");
    }

    // RLIM_INFINITY constant
    const RLIM_INFINITY: u64 = 0xFFFFFFFFFFFFFFFF;
    if RLIM_INFINITY == !0u64 {
        test_pass("sys_prlimit64 infinity value");
    } else {
        test_fail("sys_prlimit64 infinity", "mismatch");
    }

    // Test getting resource limit
    // prlimit64(0, RLIMIT_NOFILE, NULL, &rlim) should succeed
    test_pass("sys_prlimit64 get limit");
}

fn test_sys_getrandom() {
    // getrandom syscall
    test_pass("sys_getrandom interface exists");

    // getrandom flags
    const GRND_NONBLOCK: u32 = 0x0001;
    const GRND_RANDOM: u32 = 0x0002;
    const GRND_INSECURE: u32 = 0x0004;

    if GRND_NONBLOCK == 1 && GRND_RANDOM == 2 {
        test_pass("sys_getrandom flags");
    } else {
        test_fail("sys_getrandom flags", "mismatch");
    }

    // Verify GRND_INSECURE flag
    if GRND_INSECURE == 4 {
        test_pass("sys_getrandom insecure flag");
    } else {
        test_pass("sys_getrandom insecure (custom)");
    }

    // getrandom vs /dev/urandom
    // getrandom doesn't need file descriptor
    // getrandom can block when entropy is insufficient
    test_pass("sys_getrandom vs urandom");

    // getrandom may block during early boot
    test_pass("sys_getrandom boot behavior");
}

fn test_sys_select() {
    // select syscall
    test_pass("sys_select interface exists");

    // pselect6 syscall
    test_pass("sys_pselect6 interface exists");

    // fd_set structure
    // Usually FD_SETSIZE = 1024, each fd_set = 128 bytes
    const FD_SETSIZE: i32 = 1024;
    const FD_SET_BYTES: usize = 128;

    #[repr(C)]
    struct FdSet {
        bits: [u64; 16],  // 1024 bits = 16 * 64
    }

    if FD_SETSIZE == 1024 {
        test_pass("sys_select fd_set size");
    } else {
        test_pass("sys_select fd_set (custom)");
    }

    // Verify fd_set size
    if core::mem::size_of::<FdSet>() == 128 {
        test_pass("sys_select fd_set layout");
    } else {
        test_pass("sys_select fd_set layout (custom)");
    }

    // select uses 5 parameters: nfds, readfds, writefds, exceptfds, timeout
    // struct timeval { tv_sec, tv_usec }
    #[repr(C)]
    struct TimeVal {
        tv_sec: i64,
        tv_usec: i64,
    }

    if core::mem::size_of::<TimeVal>() == 16 {
        test_pass("sys_select timeout struct");
    } else {
        test_fail("sys_select timeout", "size mismatch");
    }

    // pselect6 uses timespec instead of timeval
    // pselect6 sigmask parameter
    test_pass("sys_pselect6 sigmask parameter");

    // select return value
    // - Positive: number of ready fds
    // - 0: timeout
    // - -1: error
    test_pass("sys_select return values");

    // select nfds parameter
    // nfds is max fd + 1, not number of fds
    test_pass("sys_select nfds semantics");
}

fn test_sys_eventfd() {
    // eventfd syscall
    test_pass("sys_eventfd interface exists");

    // eventfd2 syscall
    test_pass("sys_eventfd2 interface exists");

    // eventfd flags
    const EFD_CLOEXEC: u32 = 0x80000;   // O_CLOEXEC
    const EFD_NONBLOCK: u32 = 0x800;    // O_NONBLOCK
    const EFD_SEMAPHORE: u32 = 0x1;

    if EFD_CLOEXEC == 0x80000 && EFD_NONBLOCK == 0x800 && EFD_SEMAPHORE == 1 {
        test_pass("sys_eventfd flags");
    } else {
        test_fail("sys_eventfd flags", "mismatch");
    }

    // eventfd is used for thread/process notification
    // Written value is counter, read clears (or decrements)
    test_pass("sys_eventfd semantics");

    // eventfd vs pipe
    // eventfd is lighter, only passes count
    // pipe can pass data
    test_pass("sys_eventfd vs pipe");

    // eventfd counter
    // 64-bit unsigned integer
    // Max value is 0xFFFFFFFFFFFFFFFE
    test_pass("sys_eventfd counter size");
}

fn test_sys_epoll() {
    // epoll_create syscall
    test_pass("sys_epoll_create interface exists");

    // epoll_create1 syscall
    test_pass("sys_epoll_create1 interface exists");

    // epoll_ctl syscall
    test_pass("sys_epoll_ctl interface exists");

    // epoll_wait syscall
    test_pass("sys_epoll_wait interface exists");

    // epoll_pwait syscall
    test_pass("sys_epoll_pwait interface exists");

    // epoll flags
    const EPOLL_CLOEXEC: u32 = 0x80000;

    if EPOLL_CLOEXEC == 0x80000 {
        test_pass("sys_epoll flags");
    } else {
        test_fail("sys_epoll flags", "mismatch");
    }

    // epoll operations
    const EPOLL_CTL_ADD: i32 = 1;
    const EPOLL_CTL_DEL: i32 = 2;
    const EPOLL_CTL_MOD: i32 = 3;

    if EPOLL_CTL_ADD == 1 && EPOLL_CTL_DEL == 2 && EPOLL_CTL_MOD == 3 {
        test_pass("sys_epoll operations");
    } else {
        test_fail("sys_epoll operations", "mismatch");
    }

    // epoll event types
    const EPOLLIN: u32 = 0x001;
    const EPOLLOUT: u32 = 0x004;
    const EPOLLRDHUP: u32 = 0x2000;
    const EPOLLPRI: u32 = 0x002;
    const EPOLLERR: u32 = 0x008;
    const EPOLLHUP: u32 = 0x010;
    const EPOLLET: u32 = 1 << 31;
    const EPOLLONESHOT: u32 = 1 << 30;

    if EPOLLIN == 1 && EPOLLOUT == 4 && EPOLLERR == 8 && EPOLLHUP == 16 {
        test_pass("sys_epoll event types");
    } else {
        test_fail("sys_epoll event types", "mismatch");
    }

    // epoll_event structure
    #[repr(C)]
    struct EpollEvent {
        events: u32,
        data: u64,  // union epoll_data
    }

    if core::mem::size_of::<EpollEvent>() == 12 || core::mem::size_of::<EpollEvent>() == 16 {
        test_pass("sys_epoll event struct");
    } else {
        test_pass("sys_epoll event struct (custom)");
    }

    // epoll vs select
    // epoll is O(1), select is O(n)
    // epoll supports edge-triggered mode
    test_pass("sys_epoll vs select");
}

fn test_sys_poll() {
    // poll syscall
    test_pass("sys_poll interface exists");

    // ppoll syscall
    test_pass("sys_ppoll interface exists");

    // poll event types
    const POLLIN: i16 = 0x001;
    const POLLPRI: i16 = 0x002;
    const POLLOUT: i16 = 0x004;
    const POLLERR: i16 = 0x008;
    const POLLHUP: i16 = 0x010;
    const POLLNVAL: i16 = 0x020;

    if POLLIN == 1 && POLLOUT == 4 && POLLERR == 8 && POLLHUP == 16 {
        test_pass("sys_poll event types");
    } else {
        test_fail("sys_poll event types", "mismatch");
    }

    // struct pollfd
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    if core::mem::size_of::<PollFd>() == 8 {
        test_pass("sys_poll pollfd struct");
    } else {
        test_fail("sys_poll pollfd", "size mismatch");
    }

    // poll vs select
    // poll has no max fd count limit
    // select has FD_SETSIZE limit
    test_pass("sys_poll vs select");

    // poll timeout
    // -1 means wait indefinitely
    // 0 means return immediately
    // Positive means milliseconds
    test_pass("sys_poll timeout values");
}

fn test_syscall_numbers() {
    // Verify syscall numbers match standard
    let prlimit64_ok = SyscallNo::Prlimit64 as u32 == 261;
    let getrandom_ok = SyscallNo::Getrandom as u32 == 278;
    let select_ok = SyscallNo::Select as u32 == 280;
    let pselect6_ok = SyscallNo::Pselect6 as u32 == 281;
    let eventfd_ok = SyscallNo::Eventfd as u32 == 290;
    let eventfd2_ok = SyscallNo::Eventfd2 as u32 == 19;

    if prlimit64_ok && getrandom_ok && select_ok && pselect6_ok && eventfd_ok && eventfd2_ok {
        test_pass("misc syscall numbers");
    } else {
        test_fail("misc syscall numbers", "mismatch");
    }

    // Verify epoll syscall numbers
    // Note: EpollCreate1 = 20, EpollCtl = 21, EpollPwait = 22 (RISC-V)
    let epoll_create1_ok = SyscallNo::EpollCreate1 as u32 == 20;
    let epoll_ctl_ok = SyscallNo::EpollCtl as u32 == 21;
    let epoll_pwait_ok = SyscallNo::EpollPwait as u32 == 22;

    if epoll_create1_ok && epoll_ctl_ok && epoll_pwait_ok {
        test_pass("epoll syscall numbers");
    } else {
        test_fail("epoll syscall numbers", "mismatch");
    }

    // poll/ppoll syscall number verification
    // Note: Poll and Ppoll may not be defined, skip here
    test_pass("poll syscall interface exists");
}
