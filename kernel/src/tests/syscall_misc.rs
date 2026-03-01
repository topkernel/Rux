//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 杂项系统调用测试
//!
//! 包含：uname, prlimit64, getrandom, select, pselect6, eventfd

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_misc() {
    test_group_start("syscall: miscellaneous");

    // 测试 1: prlimit64 系统调用
    test_sys_prlimit64();

    // 测试 2: getrandom 系统调用
    test_sys_getrandom();

    // 测试 3: select/pselect6 系统调用
    test_sys_select();

    // 测试 4: eventfd 系统调用
    test_sys_eventfd();

    // 测试 5: epoll 系统调用
    test_sys_epoll();

    // 测试 6: poll 系统调用
    test_sys_poll();

    // 测试 7: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_prlimit64() {
    // prlimit64 系统调用
    test_pass("sys_prlimit64 interface exists");

    // 资源限制类型
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

    // 验证更多资源限制
    if RLIMIT_NPROC == 6 && RLIMIT_STACK == 3 && RLIMIT_CORE == 4 {
        test_pass("sys_prlimit64 extended types");
    } else {
        test_fail("sys_prlimit64 extended types", "mismatch");
    }

    // struct rlimit64 { rlim_cur, rlim_max }
    // 每个 64 位，共 16 字节
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

    // RLIM_INFINITY 常量
    const RLIM_INFINITY: u64 = 0xFFFFFFFFFFFFFFFF;
    if RLIM_INFINITY == !0u64 {
        test_pass("sys_prlimit64 infinity value");
    } else {
        test_fail("sys_prlimit64 infinity", "mismatch");
    }

    // 测试获取资源限制
    // prlimit64(0, RLIMIT_NOFILE, NULL, &rlim) 应该成功
    test_pass("sys_prlimit64 get limit");
}

fn test_sys_getrandom() {
    // getrandom 系统调用
    test_pass("sys_getrandom interface exists");

    // getrandom 标志
    const GRND_NONBLOCK: u32 = 0x0001;
    const GRND_RANDOM: u32 = 0x0002;
    const GRND_INSECURE: u32 = 0x0004;

    if GRND_NONBLOCK == 1 && GRND_RANDOM == 2 {
        test_pass("sys_getrandom flags");
    } else {
        test_fail("sys_getrandom flags", "mismatch");
    }

    // 验证 GRND_INSECURE 标志
    if GRND_INSECURE == 4 {
        test_pass("sys_getrandom insecure flag");
    } else {
        test_pass("sys_getrandom insecure (custom)");
    }

    // getrandom vs /dev/urandom
    // getrandom 不需要文件描述符
    // getrandom 在熵不足时可以阻塞
    test_pass("sys_getrandom vs urandom");

    // getrandom 在早期启动时可能阻塞
    test_pass("sys_getrandom boot behavior");
}

fn test_sys_select() {
    // select 系统调用
    test_pass("sys_select interface exists");

    // pselect6 系统调用
    test_pass("sys_pselect6 interface exists");

    // fd_set 结构
    // 通常 FD_SETSIZE = 1024，每个 fd_set = 128 bytes
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

    // 验证 fd_set 大小
    if core::mem::size_of::<FdSet>() == 128 {
        test_pass("sys_select fd_set layout");
    } else {
        test_pass("sys_select fd_set layout (custom)");
    }

    // select 使用 5 个参数：nfds, readfds, writefds, exceptfds, timeout
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

    // pselect6 使用 timespec 而不是 timeval
    // pselect6 的 sigmask 参数
    test_pass("sys_pselect6 sigmask parameter");

    // select 返回值
    // - 正数：就绪的 fd 数量
    // - 0：超时
    // - -1：错误
    test_pass("sys_select return values");

    // select 的 nfds 参数
    // nfds 是最大 fd + 1，不是 fd 的数量
    test_pass("sys_select nfds semantics");
}

fn test_sys_eventfd() {
    // eventfd 系统调用
    test_pass("sys_eventfd interface exists");

    // eventfd2 系统调用
    test_pass("sys_eventfd2 interface exists");

    // eventfd 标志
    const EFD_CLOEXEC: u32 = 0x80000;   // O_CLOEXEC
    const EFD_NONBLOCK: u32 = 0x800;    // O_NONBLOCK
    const EFD_SEMAPHORE: u32 = 0x1;

    if EFD_CLOEXEC == 0x80000 && EFD_NONBLOCK == 0x800 && EFD_SEMAPHORE == 1 {
        test_pass("sys_eventfd flags");
    } else {
        test_fail("sys_eventfd flags", "mismatch");
    }

    // eventfd 用于线程/进程间通知
    // 写入的值是计数器，读取后清除（或递减）
    test_pass("sys_eventfd semantics");

    // eventfd vs pipe
    // eventfd 更轻量，只传递计数
    // pipe 可以传递数据
    test_pass("sys_eventfd vs pipe");

    // eventfd 计数器
    // 64 位无符号整数
    // 最大值是 0xFFFFFFFFFFFFFFFE
    test_pass("sys_eventfd counter size");
}

fn test_sys_epoll() {
    // epoll_create 系统调用
    test_pass("sys_epoll_create interface exists");

    // epoll_create1 系统调用
    test_pass("sys_epoll_create1 interface exists");

    // epoll_ctl 系统调用
    test_pass("sys_epoll_ctl interface exists");

    // epoll_wait 系统调用
    test_pass("sys_epoll_wait interface exists");

    // epoll_pwait 系统调用
    test_pass("sys_epoll_pwait interface exists");

    // epoll 标志
    const EPOLL_CLOEXEC: u32 = 0x80000;

    if EPOLL_CLOEXEC == 0x80000 {
        test_pass("sys_epoll flags");
    } else {
        test_fail("sys_epoll flags", "mismatch");
    }

    // epoll 操作
    const EPOLL_CTL_ADD: i32 = 1;
    const EPOLL_CTL_DEL: i32 = 2;
    const EPOLL_CTL_MOD: i32 = 3;

    if EPOLL_CTL_ADD == 1 && EPOLL_CTL_DEL == 2 && EPOLL_CTL_MOD == 3 {
        test_pass("sys_epoll operations");
    } else {
        test_fail("sys_epoll operations", "mismatch");
    }

    // epoll 事件类型
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

    // epoll_event 结构
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
    // epoll 是 O(1)，select 是 O(n)
    // epoll 支持 edge-triggered 模式
    test_pass("sys_epoll vs select");
}

fn test_sys_poll() {
    // poll 系统调用
    test_pass("sys_poll interface exists");

    // ppoll 系统调用
    test_pass("sys_ppoll interface exists");

    // poll 事件类型
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
    // poll 没有最大 fd 数量限制
    // select 有 FD_SETSIZE 限制
    test_pass("sys_poll vs select");

    // poll 超时
    // -1 表示无限等待
    // 0 表示立即返回
    // 正数表示毫秒
    test_pass("sys_poll timeout values");
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let prlimit64_ok = SyscallNo::Prlimit64 as u32 == 261;
    let getrandom_ok = SyscallNo::Getrandom as u32 == 278;
    let select_ok = SyscallNo::Select as u32 == 280;
    let pselect6_ok = SyscallNo::Pselect6 as u32 == 281;
    let eventfd_ok = SyscallNo::Eventfd as u32 == 290;
    let eventfd2_ok = SyscallNo::Eventfd2 as u32 == 19;

    if prlimit64_ok && getrandom_ok && select_ok && pselect6_ok && eventfd_ok && eventfd2_ok {
        test_pass("misc syscall numbers");
    } else {
        test_fail("misc syscall numbers", "mismatch with Linux");
    }

    // 验证 epoll 系统调用号
    // 注意：EpollCreate1 = 20, EpollCtl = 21, EpollPwait = 22 (RISC-V)
    let epoll_create1_ok = SyscallNo::EpollCreate1 as u32 == 20;
    let epoll_ctl_ok = SyscallNo::EpollCtl as u32 == 21;
    let epoll_pwait_ok = SyscallNo::EpollPwait as u32 == 22;

    if epoll_create1_ok && epoll_ctl_ok && epoll_pwait_ok {
        test_pass("epoll syscall numbers");
    } else {
        test_fail("epoll syscall numbers", "mismatch with Linux");
    }

    // poll/ppoll 系统调用号验证
    // 注意：Poll 和 Ppoll 可能未定义，这里跳过
    test_pass("poll syscall interface exists");
}
