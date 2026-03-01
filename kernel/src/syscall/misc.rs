//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 杂项系统调用
//!
//! 包含：poll, select, pselect6, epoll_create, epoll_create1, epoll_ctl, epoll_wait,
//! epoll_pwait, eventfd, eventfd2, getrandom, read_input_event

use super::*;
use core::sync::atomic::{AtomicU32, Ordering};

/// pollfd 结构体 (struct pollfd)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PollFd {
    pub fd: i32,           // 文件描述符
    pub events: u16,       // 请求的事件
    pub revents: u16,      // 返回的事件
}

/// poll 事件类型
pub mod poll_events {
    pub const POLLIN: u16 = 0x0001;      // 可读
    pub const POLLPRI: u16 = 0x0002;     // 紧急可读
    pub const POLLOUT: u16 = 0x0004;     // 可写
    pub const POLLERR: u16 = 0x0008;     // 错误
    pub const POLLHUP: u16 = 0x0010;     // 挂断
    pub const POLLNVAL: u16 = 0x0020;    // 无效请求
    pub const POLLRDNORM: u16 = 0x0040;  // 等同于 POLLIN
    pub const POLLRDBAND: u16 = 0x0080;  // 优先带数据可读
    pub const POLLWRNORM: u16 = 0x0100;  // 等同于 POLLOUT
    pub const POLLWRBAND: u16 = 0x0200;  // 优先带数据可写
}

/// epoll_event 结构体
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EPollEvent {
    pub events: u32,       // 事件类型
    pub data: u64,         // 用户数据
}

/// epoll 事件类型
pub mod epoll_events {
    pub const EPOLLIN: u32 = 0x00000001;     // 可读
    pub const EPOLLPRI: u32 = 0x00000002;    // 紧急可读
    pub const EPOLLOUT: u32 = 0x00000004;    // 可写
    pub const EPOLLERR: u32 = 0x00000008;    // 错误
    pub const EPOLLHUP: u32 = 0x00000010;    // 挂断
    pub const EPOLLRDHUP: u32 = 0x00002000;  // 对端关闭连接
    pub const EPOLLONESHOT: u32 = 0x40000000; // 只监听一次
    pub const EPOLLET: u32 = 1 << 31;       // 边缘触发
}

/// epoll 操作类型
pub mod epoll_ctl_ops {
    pub const EPOLL_CTL_ADD: i32 = 1;   // 添加 fd
    pub const EPOLL_CTL_DEL: i32 = 2;   // 删除 fd
    pub const EPOLL_CTL_MOD: i32 = 3;   // 修改 fd
}

// 全局 epoll 实例计数器（简化实现）
static EPOLL_INSTANCE_COUNTER: AtomicU32 = AtomicU32::new(1);

/// sys_poll - I/O 多路复用 (poll 方式)
///
/// # 参数
/// - args[0]: fds - pollfd 数组指针
/// - args[1]: nfds - pollfd 数组长度
/// - args[2]: timeout - 超时时间（毫秒）
///
/// # 返回
/// 成功返回就绪的文件描述符数量，超时返回 0，失败返回负错误码
pub fn sys_poll(args: SyscallArgs) -> u64 {
    use poll_events::*;

    let fds_ptr = args[0] as *mut PollFd;
    let nfds = args[1] as usize;
    let timeout_ms = args[2] as i32;

    // 检查指针有效性
    if fds_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 检查 nfds 范围
    if nfds == 0 || nfds > 1024 {  // 简化：最多支持 1024 个 fd
        return -errno::EINVAL as u64;
    }

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    let mut ready_count = 0;

    // 检查所有文件描述符
    for i in 0..nfds {
        unsafe {
            let pollfd = &mut *fds_ptr.add(i);
            pollfd.revents = 0;  // 清空返回事件

            // 检查文件描述符是否存在
            let file_exists = fdtable.get_file(pollfd.fd as usize).is_some();

            if !file_exists {
                // 文件描述符不存在
                pollfd.revents |= POLLNVAL;
                ready_count += 1;
                continue;
            }

            // 简化实现：
            // 1. 对于 POLLIN: 所有有效的 fd 都认为是可读的
            if pollfd.events & POLLIN != 0 {
                pollfd.revents |= POLLIN | POLLRDNORM;
                ready_count += 1;
            }

            // 2. 对于 POLLOUT: 所有有效的 fd 都认为是可写的
            if pollfd.events & POLLOUT != 0 {
                pollfd.revents |= POLLOUT | POLLWRNORM;
                ready_count += 1;
            }

            // 3. 对于 POLLPRI: 暂不支持
            // 4. 暂不设置 POLLERR/POLLHUP
        }
    }

    // TODO: 实现超时机制
    // 当前简化实现：立即返回
    let _ = timeout_ms;

    ready_count as u64
}

/// sys_pselect6 - I/O 多路复用 (pselect6 方式)
///
/// # 参数
/// - args[0]: nfds - 需要检查的最高文件描述符 + 1
/// - args[1]: readfds - 可读文件描述符集合指针
/// - args[2]: writefds - 可写文件描述符集合指针
/// - args[3]: exceptfds - 异常文件描述符集合指针
/// - args[4]: timeout - 超时时间 (TimeVal 指针)
/// - args[5]: sigmask - 信号掩码指针
///
/// # 返回
/// 成功返回就绪的文件描述符数量，超时返回 0，失败返回负错误码
pub fn sys_pselect6(args: SyscallArgs) -> u64 {
    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
    let timeout_ptr = args[4] as *const TimeVal;
    let _sigmask_ptr = args[5] as *const u64;  // sigmask 暂未使用

    // 验证 nfds 范围
    if nfds < 0 || nfds > FD_SETSIZE {
        return -errno::EINVAL as u64;
    }

    // 检查指针有效性
    if readfds_ptr.is_null() && writefds_ptr.is_null() && exceptfds_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 读取原始 fd_sets
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

    // 创建返回的 fd_sets
    let mut result_readfds = FdSet::new();
    let mut result_writefds = FdSet::new();
    let mut result_exceptfds = FdSet::new();

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    let mut ready_count = 0;

    // 检查所有文件描述符
    for fd in 0..nfds {
        let mut is_readable = false;
        let mut is_writable = false;
        let mut has_exception = false;

        // 检查文件描述符是否存在
        let file_exists = fdtable.get_file(fd as usize).is_some();

        if !file_exists {
            // 文件描述符不存在，跳过
            continue;
        }

        // 简化实现：
        // 1. 对于 readfds: 所有有效的 fd 都认为是可读的
        if original_readfds.is_set(fd) {
            is_readable = true;
        }

        // 2. 对于 writefds: 所有有效的 fd 都认为是可写的
        if original_writefds.is_set(fd) {
            is_writable = true;
        }

        // 3. 对于 exceptfds: 暂不实现异常检查
        if original_exceptfds.is_set(fd) {
            has_exception = false;  // 暂不支持异常
        }

        // 设置返回的 fd_sets
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

    // 将结果写回用户空间
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

    // TODO: 实现超时机制
    let _ = timeout_ptr;

    ready_count as u64
}

/// sys_select - I/O 多路复用 (BSD 风格)
///
/// # 参数
/// - args[0]: nfds - 需要检查的最高文件描述符 + 1
/// - args[1]: readfds - 可读文件描述符集合指针
/// - args[2]: writefds - 可写文件描述符集合指针
/// - args[3]: exceptfds - 异常文件描述符集合指针
/// - args[4]: timeout - 超时时间 (TimeVal 指针)
///
/// # 返回
/// 成功返回就绪的文件描述符数量，超时返回 0，失败返回负错误码
pub fn sys_select(args: SyscallArgs) -> u64 {
    // select 是 pselect6 的特殊情况，sigmask 为 null
    sys_pselect6([args[0], args[1], args[2], args[3], args[4], 0])
}

/// sys_epoll_create - 创建 epoll 实例
///
/// # 参数
/// - args[0]: size - 提示内核需要分配的事件数量（已废弃）
///
/// # 返回
/// 成功返回 epoll 文件描述符，失败返回负错误码
pub fn sys_epoll_create(args: SyscallArgs) -> u64 {
    let _size = args[0] as i32;

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    // 分配文件描述符
    let epoll_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    // 简化实现：
    // 在真实实现中，应该创建一个 EpollFile 并安装到 fdtable
    // 这里我们只是分配一个 fd，实际功能由 epoll_ctl/epoll_wait 实现
    // TODO: 创建 EpollFile 结构

    epoll_fd as u64
}

/// sys_epoll_create1 - 创建 epoll 实例（带标志）
///
/// # 参数
/// - args[0]: flags - 标志位
///
/// # 返回
/// 成功返回 epoll 文件描述符，失败返回负错误码
pub fn sys_epoll_create1(args: SyscallArgs) -> u64 {
    // 简化实现：忽略标志
    // O_CLOEXEC (0x80000) 等标志暂不支持
    sys_epoll_create(args)
}

/// sys_epoll_ctl - 控制 epoll 实例
///
/// # 参数
/// - args[0]: epfd - epoll 文件描述符
/// - args[1]: op - 操作类型 (ADD/DEL/MOD)
/// - args[2]: fd - 目标文件描述符
/// - args[3]: event - 事件指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_epoll_ctl(args: SyscallArgs) -> u64 {
    use epoll_ctl_ops::*;

    let epfd = args[0] as i32;
    let op = args[1] as i32;
    let fd = args[2] as i32;
    let event_ptr = args[3] as *const EPollEvent;

    // 验证 epfd
    if epfd < 0 {
        return -errno::EBADF as u64;
    }

    // 验证 op
    if op != EPOLL_CTL_ADD && op != EPOLL_CTL_DEL && op != EPOLL_CTL_MOD {
        return -errno::EINVAL as u64;
    }

    // 验证 fd
    if fd < 0 {
        return -errno::EBADF as u64;
    }

    // 验证 event_ptr（ADD 和 MOD 需要 event）
    if (op == EPOLL_CTL_ADD || op == EPOLL_CTL_MOD) && event_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 简化实现：
    // 在真实实现中，应该：
    // 1. 查找 epfd 对应的 EpollFile
    // 2. 根据 op 添加/删除/修改 fd 到 epoll 集合
    // TODO: 实现 EpollFile 和红黑树

    0  // 成功
}

/// sys_epoll_wait - 等待 epoll 事件
///
/// # 参数
/// - args[0]: epfd - epoll 文件描述符
/// - args[1]: events - 事件数组指针
/// - args[2]: maxevents - 最大事件数
/// - args[3]: timeout - 超时时间（毫秒）
///
/// # 返回
/// 成功返回就绪的事件数量，超时返回 0，失败返回负错误码
pub fn sys_epoll_wait(args: SyscallArgs) -> u64 {
    let epfd = args[0] as i32;
    let events_ptr = args[1] as *mut EPollEvent;
    let maxevents = args[2] as i32;
    let timeout_ms = args[3] as i32;

    // 验证 epfd
    if epfd < 0 {
        return -errno::EBADF as u64;
    }

    // 验证 events_ptr
    if events_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 验证 maxevents
    if maxevents <= 0 || maxevents > 1024 {
        return -errno::EINVAL as u64;
    }

    // 简化实现：
    // 在真实实现中，应该：
    // 1. 查找 epfd 对应的 EpollFile
    // 3. 等待事件或超时
    // 4. 将就绪事件复制到用户空间
    // TODO: 实现真实的等待逻辑

    // 当前简化：立即返回 0（超时）
    let _ = (epfd, events_ptr, maxevents, timeout_ms);

    0  // 超时
}

/// sys_epoll_pwait - 等待 epoll 事件（带信号掩码）
///
/// # 参数
/// - args[0]: epfd - epoll 文件描述符
/// - args[1]: events - 事件数组指针
/// - args[2]: maxevents - 最大事件数
/// - args[3]: timeout - 超时时间（毫秒）
/// - args[4]: sigmask - 信号掩码指针
///
/// # 返回
/// 成功返回就绪的事件数量，超时返回 0，失败返回负错误码
pub fn sys_epoll_pwait(args: SyscallArgs) -> u64 {
    // 简化实现：忽略信号掩码
    sys_epoll_wait([args[0], args[1], args[2], args[3], 0, 0])
}

/// sys_eventfd - 创建 eventfd 对象
///
/// # 参数
/// - args[0]: initval - 初始值
///
/// # 返回
/// 成功返回 eventfd 文件描述符，失败返回负错误码
pub fn sys_eventfd(args: SyscallArgs) -> u64 {
    let _initval = args[0] as u32;

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    // 分配文件描述符
    let eventfd_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => return -errno::EMFILE as u64,
    };

    // 简化实现：
    // 在真实实现中，应该创建一个 EventFdFile 并安装到 fdtable
    // eventfd 本质上是一个 64 位计数器
    // TODO: 创建 EventFdFile 结构

    eventfd_fd as u64
}

/// sys_eventfd2 - 创建 eventfd 对象（带标志）
///
/// # 参数
/// - args[0]: initval - 初始值
/// - args[1]: flags - 标志位
///
/// # 返回
/// 成功返回 eventfd 文件描述符，失败返回负错误码
pub fn sys_eventfd2(args: SyscallArgs) -> u64 {
    // 简化实现：忽略标志
    // EFD_CLOEXEC (0x80000), EFD_NONBLOCK (0x800), EFD_SEMAPHORE (0x1) 等标志暂不支持
    sys_eventfd(args)
}

/// sys_getrandom - 获取随机字节
///
/// # 参数
/// - args[0]: buf - 存储随机字节的缓冲区
/// - args[1]: buflen - 请求的字节数
/// - args[2]: flags - 标志 (GRND_NONBLOCK, GRND_RANDOM, etc.)
///
/// # 返回
/// 成功返回写入的字节数，失败返回负错误码
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

    // 验证用户空间指针
    let buf_addr = buf_ptr as usize;
    if buf_addr < 0x10000 || buf_addr >= 0x8000_0000 {
        return -errno::EFAULT as u64;
    }

    // 使用简单的伪随机数生成器
    // 在实际系统中应该使用硬件随机数或更安全的 RNG
    unsafe {
        // 使用时间戳作为种子
        let seed = crate::drivers::intc::clint::read_time();

        // 简单的线性同余生成器
        let mut state = seed;
        for i in 0..buflen {
            // LCG: state = state * 1103515245 + 12345
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *buf_ptr.add(i) = ((state >> 16) & 0xff) as u8;
        }
    }

    buflen as u64
}

/// sys_read_input_event - 读取用户输入事件（自定义系统调用）
///
/// # 参数
/// - args[0]: buf - 存储输入事件的缓冲区
/// - args[1]: count - 缓冲区大小 (字节数)
/// - args[2]: device_type - 设备类型 (0 = 键盘, 1 = 指针)
///
/// # 返回
/// 成功返回读取的字节数，无事件返回 0，失败返回负错误码
pub fn sys_read_input_event(args: SyscallArgs) -> u64 {
    use crate::drivers::input::{InputEvent, poll_events, get_keyboard_event, get_pointer_event};

    let buf = args[0] as *mut u8;
    let _count = args[1] as usize;
    let device_type = args[2] as usize;  // 0 = 键盘, 1 = 指针

    // 先轮询新事件
    poll_events();

    // 获取输入事件
    let event = if device_type == 1 {
        get_pointer_event()
    } else {
        get_keyboard_event()
    };

    match event {
        Some(event) => {
            unsafe {
                // 将事件复制到用户空间
                let dest = buf as *mut InputEvent;
                core::ptr::write_volatile(dest, event);
            }
            core::mem::size_of::<InputEvent>() as u64
        }
        None => 0,  // 无事件
    }
}
