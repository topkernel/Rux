//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit 系统调用处理
//!
//! 实现基于 ecall (Environment Call) 指令的系统调用接口
//!
//! RISC-V 系统调用约定:
//! - a7: 系统调用号
//! - a0-a5: 参数 (最多6个)
//! - 返回值: a0
//! - 错误码: a0 设置为负数

use core::arch::asm;
use crate::println;
use crate::debug_println;
use crate::config::{USER_STACK_SIZE, USER_STACK_TOP};

/// 时间值结构体 (struct timeval)
///
/// 对应 Linux 的 timeval (include/uapi/linux/time.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct TimeVal {
    pub tv_sec: i64,   // 秒
    pub tv_usec: i64,  // 微秒
}

/// 文件描述符集 (fd_set)
///
/// 对应 Linux 的 fd_set (include/uapi/linux/types.h)
/// 简化实现：使用 u64 位图，最多支持 64 个文件描述符
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FdSet {
    pub fds_bits: [u64; 1],  // 简化：只支持 64 个 fd
}

impl FdSet {
    pub const fn new() -> Self {
        Self { fds_bits: [0] }
    }

    pub fn set(&mut self, fd: i32) {
        if fd >= 0 && fd < 64 {
            self.fds_bits[0] |= 1 << fd;
        }
    }

    pub fn clear(&mut self, fd: i32) {
        if fd >= 0 && fd < 64 {
            self.fds_bits[0] &= !(1 << fd);
        }
    }

    pub fn is_set(&self, fd: i32) -> bool {
        if fd >= 0 && fd < 64 {
            (self.fds_bits[0] & (1 << fd)) != 0
        } else {
            false
        }
    }

    pub fn zero(&mut self) {
        self.fds_bits[0] = 0;
    }
}

/// select 系统调用的文件描述符数量限制
const FD_SETSIZE: i32 = 64;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SyscallNo {
    /// IO 操作
    Read = 63,
    Write = 64,
    Open = 1024,
    Close = 57,
    Stat = 1038,
    Fstat = 80,
    Lstat = 1039,
    Poll = 168,
    Lseek = 62,
    Mmap = 222,
    Mprotect = 226,
    Munmap = 215,
    Brk = 214,
    Ioctl = 29,

    /// 进程操作
    Clone = 220,
    Execve = 221,
    Exit = 93,
    ExitGroup = 94,
    Wait4 = 260,
    Kill = 129,
    Getpid = 172,
    Getppid = 110,

    /// 文件操作
    Openat = 56,
    Unlink = 74,
    Mkdir = 77,
    Rmdir = 79,

    /// 信号操作
    RtSigaction = 134,
    RtSigprocmask = 135,
    RtSigreturn = 139,
    Sigaltstack = 132,

    /// 时间操作
    Nanosleep = 101,
    Gettimeofday = 169,
    ClockGettime = 113,
    ClockGetres = 114,

    /// 网络操作
    Socket = 198,
    Bind = 200,
    Listen = 201,
    Accept = 202,
    Connect = 203,
    SendTo = 206,
    RecvFrom = 207,

    /// 其他
    Pipe = 59,
    Dup = 23,
    Dup2 = 24,
    Getuid = 174,
    Getgid = 176,
    Geteuid = 175,
    Getegid = 177,
    Uname = 160,
    Fcntl = 25,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SyscallFrame {
    pub a0: u64,   // offset 0   - 返回值 / 第1个参数
    pub a1: u64,   // offset 8   - 第2个参数
    pub a2: u64,   // offset 16  - 第3个参数
    pub a3: u64,   // offset 24  - 第4个参数
    pub a4: u64,   // offset 32  - 第5个参数
    pub a5: u64,   // offset 40  - 第6个参数
    pub a6: u64,   // offset 48
    pub a7: u64,   // offset 56  - 系统调用号
    pub t0: u64,   // offset 64
    pub t1: u64,   // offset 72
    pub t2: u64,   // offset 80
    pub t3: u64,   // offset 88
    pub t4: u64,   // offset 96
    pub t5: u64,   // offset 104
    pub t6: u64,   // offset 112
    pub s0: u64,   // offset 120
    pub s1: u64,   // offset 128
    pub s2: u64,   // offset 136
    pub s3: u64,   // offset 144
    pub s4: u64,   // offset 152
    pub s5: u64,   // offset 160
    pub s6: u64,   // offset 168
    pub s7: u64,   // offset 176
    pub s8: u64,   // offset 184
    pub s9: u64,   // offset 192
    pub s10: u64,  // offset 200
    pub s11: u64,  // offset 208
    pub ra: u64,   // offset 216 - 返回地址
    pub sp: u64,   // offset 224 - 栈指针
    pub gp: u64,   // offset 232
    pub tp: u64,   // offset 240
    pub pc: u64,   // offset 248 - 程序计数器
    pub status: u64, // offset 256 - 程序状态 (mstatus)
}

impl Default for SyscallFrame {
    fn default() -> Self {
        Self {
            a0: 0,
            a1: 0,
            a2: 0,
            a3: 0,
            a4: 0,
            a5: 0,
            a6: 0,
            a7: 0,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
            t4: 0,
            t5: 0,
            t6: 0,
            s0: 0,
            s1: 0,
            s2: 0,
            s3: 0,
            s4: 0,
            s5: 0,
            s6: 0,
            s7: 0,
            s8: 0,
            s9: 0,
            s10: 0,
            s11: 0,
            ra: 0,
            sp: 0,
            gp: 0,
            tp: 0,
            pc: 0,
            status: 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) {
    // 调试输出：打印系统调用号
    use crate::console::putchar;
    const MSG: &[u8] = b"[ECALL:";
    for &b in MSG {
        putchar(b);
    }
    // 打印 a7 的十六进制值
    let hex_chars = b"0123456789ABCDEF";
    let val = frame.a7;
    putchar(hex_chars[((val >> 4) & 0xF) as usize]);
    putchar(hex_chars[(val & 0xF) as usize]);
    const MSG2: &[u8] = b"]\n";
    for &b in MSG2 {
        putchar(b);
    }

    let syscall_no = frame.a7;
    let args = [frame.a0, frame.a1, frame.a2, frame.a3, frame.a4, frame.a5];

    // 根据系统调用号分发
    frame.a0 = match syscall_no as u32 {
        63 => sys_read(args),
        64 => sys_write(args),
        56 => sys_openat(args),
        57 => sys_close(args),
        93 => sys_exit(args),
        172 => sys_getpid(args),
        110 => sys_getppid(args),
        129 => sys_kill(args),
        134 => { debug_println!("sys_rt_sigaction: not implemented"); -38_i64 as u64 },  // ENOSYS
        135 => sys_rt_sigprocmask(args),  // RISC-V rt_sigprocmask
        280 => sys_select(args),          // RISC-V select
        281 => sys_pselect6(args),        // RISC-V pselect6
        7 => sys_poll(args),              // RISC-V poll
        20 => sys_epoll_create(args),     // RISC-V epoll_create (可能需要确认)
        251 => sys_epoll_create1(args),   // RISC-V epoll_create1
        21 => sys_epoll_ctl(args),        // RISC-V epoll_ctl (可能需要确认)
        22 => sys_epoll_wait(args),       // RISC-V epoll_wait (可能需要确认)
        252 => sys_epoll_pwait(args),     // RISC-V epoll_pwait
        290 => sys_eventfd(args),         // RISC-V eventfd (可能需要确认)
        291 => sys_eventfd2(args),        // RISC-V eventfd2
        59 => sys_pipe2(args),            // RISC-V pipe2 (supports flags)
        220 => sys_fork(args),
        220 => sys_fork(args),
        221 => sys_execve(args),
        260 => sys_wait4(args),
        160 => sys_uname(args),
        174 => sys_getuid(args),
        176 => sys_getgid(args),
        175 => sys_geteuid(args),
        177 => sys_getegid(args),
        169 => sys_gettimeofday(args),
        113 => sys_clock_gettime(args),
        101 => sys_nanosleep(args),  // 纳秒级睡眠
        23 => sys_dup(args),
        24 => sys_dup2(args),
        25 => sys_fcntl(args),
        80 => sys_fstat(args),
        77 => sys_mkdir(args),
        79 => sys_rmdir(args),
        74 => sys_unlink(args),
        78 => sys_link(args),
        214 => sys_brk(args),
        222 => sys_mmap(args),
        215 => sys_munmap(args),
        198 => sys_socket(args),
        200 => sys_bind(args),
        201 => sys_listen(args),
        202 => sys_accept(args),
        203 => sys_connect(args),
        206 => sys_sendto(args),
        207 => sys_recvfrom(args),
        _ => {
            debug_println!("Unknown syscall: {}", syscall_no);
            -38_i64 as u64  // ENOSYS - 函数未实现
        }
    };
}

// ============================================================================
// 系统调用实现
// ============================================================================

fn sys_read(args: [u64; 6]) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;

    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                let result = file.read(buf, count);
                if result < 0 {
                    result as u32 as u64  // 返回错误码
                } else {
                    result as u64  // 返回读取的字节数
                }
            }
            None => {
                debug_println!("sys_read: invalid fd");
                -9_i64 as u64  // EBADF
            }
        }
    }
}

fn sys_write(args: [u64; 6]) -> u64 {
    use crate::fs::get_file_fd;
    let fd = args[0] as usize;
    let buf = args[1] as *const u8;
    let count = args[2] as usize;

    unsafe {
        // Special handling for stdout (1) and stderr (2) - write directly to UART
        if fd == 1 || fd == 2 {
            use crate::console::putchar;
            let slice = core::slice::from_raw_parts(buf, count);
            for &b in slice {
                putchar(b);
            }
            return count as u64;
        }

        match get_file_fd(fd) {
            Some(file) => {
                let result = file.write(buf, count);
                if result < 0 {
                    result as u32 as u64  // 返回错误码
                } else {
                    result as u64  // 返回写入的字节数
                }
            }
            None => {
                debug_println!("sys_write: invalid fd");
                -9_i64 as u64  // EBADF
            }
        }
    }
}

fn sys_openat(args: [u64; 6]) -> u64 {
    let _dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;
    let mode = args[3] as u32;

    println!("sys_openat: pathname_ptr={:#x}, flags={:#x}, mode={:#x}",
             pathname_ptr as usize, flags, mode);

    // 检查路径指针
    if pathname_ptr.is_null() {
        println!("sys_openat: null pathname");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取文件名（简化：假设以 null 结尾）
    let filename = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    // 转换为字符串
    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_openat: invalid utf-8 filename");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_openat: opening '{}'", filename_str);

    // 调用 VFS 打开文件
    match crate::fs::file_open(filename_str, flags, mode) {
        Ok(fd) => fd as u64,
        Err(e) => e as u32 as u64,
    }
}

fn sys_close(args: [u64; 6]) -> u64 {
    use crate::fs::close_file_fd;
    let fd = args[0] as usize;

    println!("sys_close: fd={}", fd);

    unsafe {
        match close_file_fd(fd) {
            Ok(()) => 0,
            Err(e) => e as u32 as u64,
        }
    }
}

fn sys_pipe(args: [u64; 6]) -> u64 {
    sys_pipe2_impl(args, 0)
}

/// sys_pipe2 - 创建带有标志的管道
///
/// # 参数
/// - args[0]: pipefd - 指向存储两个文件描述符的数组
/// - args[1]: flags - 标志位 (O_CLOEXEC, O_NONBLOCK, 等)
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # 标志位
/// - O_CLOEXEC (0x80000): exec() 时关闭文件描述符
/// - O_NONBLOCK (0x800): 非阻塞模式
fn sys_pipe2(args: [u64; 6]) -> u64 {
    let pipefd_ptr = args[0] as *mut i32;
    let flags = args[1] as u32;

    sys_pipe2_impl([pipefd_ptr as u64, flags as u64, args[2], args[3], args[4], args[5]], flags as u64)
}

/// 管道系统调用的实现
fn sys_pipe2_impl(args: [u64; 6], flags: u64) -> u64 {
    let pipefd_ptr = args[0] as *mut i32;

    // 检查指针有效性（简化检查，只检查是否为 null）
    if pipefd_ptr.is_null() {
        println!("sys_pipe2: pipefd is null");
        return -14_i64 as u64;  // EFAULT
    }

    // 解析标志位
    const O_CLOEXEC: u32 = 0x80000;
    const O_NONBLOCK: u32 = 0x800;

    let has_cloexec = (flags as u32) & O_CLOEXEC != 0;
    let has_nonblock = (flags as u32) & O_NONBLOCK != 0;

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => {
            println!("sys_pipe2: no fdtable");
            return -9_i64 as u64;  // EBADF
        }
    };

    // 创建管道
    let (read_file, write_file) = crate::fs::create_pipe();

    // 分配文件描述符
    let read_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("sys_pipe2: failed to alloc read fd");
            return -24_i64 as u64;  // EMFILE - 进程打开文件数过多
        }
    };

    let write_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("sys_pipe2: failed to alloc write fd");
            // 释放已分配的读端（直接关闭文件描述符）
            let _ = fdtable.close_fd(read_fd);
            return -24_i64 as u64;  // EMFILE
        }
    };

    // 设置文件描述符标志
    if has_cloexec {
        // TODO: 实现 close-on-exec 标志
        println!("sys_pipe2: O_CLOEXEC flag not yet supported");
        // 继续执行，不返回错误
    }

    // TODO: 实现 O_NONBLOCK 标志
    if has_nonblock {
        println!("sys_pipe2: O_NONBLOCK flag not yet supported");
        // 继续执行，不返回错误
    }

    // 安装文件到 fdtable
    if fdtable.install_fd(read_fd, read_file).is_err() {
        println!("sys_pipe2: failed to install read fd");
        let _ = fdtable.close_fd(read_fd);
        let _ = fdtable.close_fd(write_fd);
        return -9_i64 as u64;  // EBADF
    }

    if fdtable.install_fd(write_fd, write_file).is_err() {
        println!("sys_pipe2: failed to install write fd");
        let _ = fdtable.close_fd(read_fd);
        let _ = fdtable.close_fd(write_fd);
        return -9_i64 as u64;  // EBADF
    }

    // 将文件描述符写入用户空间
    unsafe {
        *pipefd_ptr.add(0) = read_fd as i32;
        *pipefd_ptr.add(1) = write_fd as i32;
    }

    println!("sys_pipe2: created pipe, read_fd={}, write_fd={}, flags={:#x}", read_fd, write_fd, flags);

    0  // 成功
}

/// sys_pselect6 - I/O 多路复用 (使用 sigmask)
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
///
/// # 说明
/// 这是一个简化实现，主要支持:
/// - 检查文件描述符是否就绪
/// - 超时机制
/// - 返回修改后的 fd_sets
fn sys_pselect6(args: [u64; 6]) -> u64 {
    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
    let timeout_ptr = args[4] as *const TimeVal;
    let _sigmask_ptr = args[5] as *const u64;  // sigmask 暂未使用

    println!("sys_pselect6: nfds={}, readfds={:#x}, writefds={:#x}, exceptfds={:#x}, timeout={:#x}",
             nfds, readfds_ptr as u64, writefds_ptr as u64, exceptfds_ptr as u64, timeout_ptr as u64);

    // 验证 nfds 范围
    if nfds < 0 || nfds > FD_SETSIZE {
        println!("sys_pselect6: invalid nfds {}", nfds);
        return -22_i64 as u64;  // EINVAL
    }

    // 检查指针有效性
    if readfds_ptr.is_null() && writefds_ptr.is_null() && exceptfds_ptr.is_null() {
        println!("sys_pselect6: all fd_sets are null");
        return -14_i64 as u64;  // EFAULT
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
        None => {
            println!("sys_pselect6: no fdtable");
            return -9_i64 as u64;  // EBADF
        }
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

    println!("sys_pselect6: {} file descriptors ready", ready_count);

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
///
/// # 说明
/// select 是 pselect6 的简化版本，不使用信号掩码
/// 实际调用 sys_pselect6 完成
fn sys_select(args: [u64; 6]) -> u64 {
    // select 是 pselect6 的特殊情况，sigmask 为 null
    sys_pselect6([args[0], args[1], args[2], args[3], args[4], 0])
}

/// sys_rt_sigprocmask - 检查和更改阻塞的信号
///
/// # 参数
/// - args[0]: how - 操作方式
///   - SIG_BLOCK (0): 将 set 中的信号添加到阻塞掩码
///   - SIG_UNBLOCK (1): 从阻塞掩码中删除 set 中的信号
///   - SIG_SETMASK (2): 设置阻塞掩码为 set
/// - args[1]: set - 新信号掩码指针
/// - args[2]: oldset - 用于返回旧信号掩码的指针
/// - args[3]: sigsetsize - 信号集大小 (必须为 8)
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # 说明
/// 参考实现: Linux kernel/kernel/signal.c::sys_rt_sigprocmask()
fn sys_rt_sigprocmask(args: [u64; 6]) -> u64 {
    let how = args[0] as i32;
    let set_ptr = args[1] as *const u64;  // SigSet is u64
    let oldset_ptr = args[2] as *mut u64;
    let sigsetsize = args[3] as usize;

    println!("sys_rt_sigprocmask: how={}, set={:#x}, oldset={:#x}, sigsetsize={}",
             how, set_ptr as u64, oldset_ptr as u64, sigsetsize);

    // 验证 sigsetsize
    if sigsetsize != 8 {
        println!("sys_rt_sigprocmask: invalid sigsetsize {}", sigsetsize);
        return -22_i64 as u64;  // EINVAL
    }

    // 验证 how 参数
    use crate::signal::sigprocmask_how;
    if how != sigprocmask_how::SIG_BLOCK
        && how != sigprocmask_how::SIG_UNBLOCK
        && how != sigprocmask_how::SIG_SETMASK
    {
        println!("sys_rt_sigprocmask: invalid how {}", how);
        return -22_i64 as u64;  // EINVAL
    }

    // 读取新的信号掩码
    let new_mask = if !set_ptr.is_null() {
        unsafe { *set_ptr }
    } else {
        0
    };

    // 获取当前进程的 runqueue
    let rq = match crate::sched::this_cpu_rq() {
        Some(r) => r,
        None => {
            println!("sys_rt_sigprocmask: no runqueue");
            return -1_i64 as u64;  // EPERM
        }
    };

    let current = rq.lock().current;
    if current.is_null() {
        println!("sys_rt_sigprocmask: no current task");
        return -1_i64 as u64;  // EPERM
    }

    // 获取当前信号掩码
    let old_mask = unsafe { (*current).sigmask };

    // 设置新的信号掩码
    let result_mask = match how {
        sigprocmask_how::SIG_BLOCK => {
            // 添加信号到阻塞掩码
            old_mask | new_mask
        }
        sigprocmask_how::SIG_UNBLOCK => {
            // 从阻塞掩码删除信号
            old_mask & !new_mask
        }
        sigprocmask_how::SIG_SETMASK => {
            // 设置新的阻塞掩码
            new_mask
        }
        _ => old_mask, // 不应该到达这里
    };

    // 更新当前进程的信号掩码
    unsafe {
        (*current).sigmask = result_mask;
    }

    // 返回旧的信号掩码
    if !oldset_ptr.is_null() {
        unsafe {
            *oldset_ptr = old_mask;
        }
    }

    println!("sys_rt_sigprocmask: old_mask={:#x}, new_mask={:#x}", old_mask, result_mask);

    0  // 成功
}

/// pollfd 结构体 (struct pollfd)
///
/// 对应 Linux 的 pollfd (include/uapi/linux/poll.h)
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

/// sys_poll - I/O 多路复用 (poll 方式)
///
/// # 参数
/// - args[0]: fds - pollfd 数组指针
/// - args[1]: nfds - pollfd 数组长度
/// - args[2]: timeout - 超时时间（毫秒）
///
/// # 返回
/// 成功返回就绪的文件描述符数量，超时返回 0，失败返回负错误码
///
/// # 说明
/// poll 比 select 更灵活，没有文件描述符数量限制
/// 参考实现: Linux kernel/fs/select.c::sys_poll()
fn sys_poll(args: [u64; 6]) -> u64 {
    use poll_events::*;

    let fds_ptr = args[0] as *mut PollFd;
    let nfds = args[1] as usize;
    let timeout_ms = args[2] as i32;

    println!("sys_poll: fds={:#x}, nfds={}, timeout={}ms", fds_ptr as u64, nfds, timeout_ms);

    // 检查指针有效性
    if fds_ptr.is_null() {
        println!("sys_poll: fds is null");
        return -14_i64 as u64;  // EFAULT
    }

    // 检查 nfds 范围
    if nfds == 0 || nfds > 1024 {  // 简化：最多支持 1024 个 fd
        println!("sys_poll: invalid nfds {}", nfds);
        return -22_i64 as u64;  // EINVAL
    }

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => {
            println!("sys_poll: no fdtable");
            return -9_i64 as u64;  // EBADF
        }
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

    println!("sys_poll: {} file descriptors ready", ready_count);

    ready_count as u64
}

/// epoll_event 结构体
///
/// 对应 Linux 的 epoll_event (include/uapi/linux/eventpoll.h)
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
use core::sync::atomic::{AtomicU32, Ordering};

static EPOLL_INSTANCE_COUNTER: AtomicU32 = AtomicU32::new(1);

/// sys_epoll_create - 创建 epoll 实例
///
/// # 参数
/// - args[0]: size - 建议的大小（忽略，Linux 2.6.27+ 已忽略此参数）
///
/// # 返回
/// 成功返回 epoll 文件描述符，失败返回负错误码
///
/// # 说明
/// 创建一个 epoll 实例，返回用于后续 epoll_ctl/epoll_wait 的文件描述符
/// 简化实现：返回一个伪文件描述符
/// 参考实现: Linux kernel/fs/eventpoll.c::sys_epoll_create()
fn sys_epoll_create(args: [u64; 6]) -> u64 {
    let _size = args[0] as i32;

    println!("sys_epoll_create: size={}", _size);

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => {
            println!("sys_epoll_create: no fdtable");
            return -9_i64 as u64;  // EBADF
        }
    };

    // 分配文件描述符
    let epoll_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("sys_epoll_create: failed to alloc fd");
            return -24_i64 as u64;  // EMFILE
        }
    };

    // 简化实现：
    // 在真实实现中，应该创建一个 EpollFile 并安装到 fdtable
    // 这里我们只是分配一个 fd，实际功能由 epoll_ctl/epoll_wait 实现
    // TODO: 创建 EpollFile 结构

    println!("sys_epoll_create: created epoll fd {}", epoll_fd);

    epoll_fd as u64
}

/// sys_epoll_create1 - 创建 epoll 实例（带标志）
///
/// # 参数
/// - args[0]: flags - 标志位
///
/// # 返回
/// 成功返回 epoll 文件描述符，失败返回负错误码
///
/// # 说明
/// epoll_create1 是 epoll_create 的扩展版本，支持标志位
/// 简化实现：忽略标志，调用 epoll_create
fn sys_epoll_create1(args: [u64; 6]) -> u64 {
    let flags = args[0] as i32;

    println!("sys_epoll_create1: flags={:#x}", flags);

    // 简化实现：忽略标志
    // O_CLOEXEC (0x80000) 等标志暂不支持
    sys_epoll_create([0, args[1], args[2], args[3], args[4], args[5]])
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
///
/// # 说明
/// 向 epoll 实例添加、删除或修改文件描述符
/// 简化实现：只验证参数，不实际维护 epoll 集合
/// 参考实现: Linux kernel/fs/eventpoll.c::sys_epoll_ctl()
fn sys_epoll_ctl(args: [u64; 6]) -> u64 {
    use epoll_ctl_ops::*;

    let epfd = args[0] as i32;
    let op = args[1] as i32;
    let fd = args[2] as i32;
    let event_ptr = args[3] as *const EPollEvent;

    println!("sys_epoll_ctl: epfd={}, op={}, fd={}, event={:#x}",
             epfd, op, fd, event_ptr as u64);

    // 验证 epfd
    if epfd < 0 {
        println!("sys_epoll_ctl: invalid epfd");
        return -9_i64 as u64;  // EBADF
    }

    // 验证 op
    if op != EPOLL_CTL_ADD && op != EPOLL_CTL_DEL && op != EPOLL_CTL_MOD {
        println!("sys_epoll_ctl: invalid op {}", op);
        return -22_i64 as u64;  // EINVAL
    }

    // 验证 fd
    if fd < 0 {
        println!("sys_epoll_ctl: invalid fd");
        return -9_i64 as u64;  // EBADF
    }

    // 验证 event_ptr（ADD 和 MOD 需要 event）
    if (op == EPOLL_CTL_ADD || op == EPOLL_CTL_MOD) && event_ptr.is_null() {
        println!("sys_epoll_ctl: event is null for ADD/MOD");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取事件
    let event = if !event_ptr.is_null() {
        unsafe { *event_ptr }
    } else {
        EPollEvent { events: 0, data: 0 }
    };

    // 简化实现：
    // 在真实实现中，应该：
    // 1. 查找 epfd 对应的 EpollFile
    // 2. 根据 op 添加/删除/修改 fd 到 epoll 集合
    // 3. 维护红黑树（Linux 使用红黑树存储监听的 fd）
    // TODO: 实现 EpollFile 和红黑树

    println!("sys_epoll_ctl: op={}, fd={}, events={:#x}, data={:#x}",
             match op {
                 EPOLL_CTL_ADD => "ADD",
                 EPOLL_CTL_DEL => "DEL",
                 EPOLL_CTL_MOD => "MOD",
                 _ => "UNKNOWN",
             },
             fd, event.events, event.data);

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
///
/// # 说明
/// 等待 epoll 实例上的事件
/// 简化实现：返回 0（超时）
/// 参考实现: Linux kernel/fs/eventpoll.c::sys_epoll_wait()
fn sys_epoll_wait(args: [u64; 6]) -> u64 {
    let epfd = args[0] as i32;
    let events_ptr = args[1] as *mut EPollEvent;
    let maxevents = args[2] as i32;
    let timeout_ms = args[3] as i32;

    println!("sys_epoll_wait: epfd={}, events={:#x}, maxevents={}, timeout={}ms",
             epfd, events_ptr as u64, maxevents, timeout_ms);

    // 验证 epfd
    if epfd < 0 {
        println!("sys_epoll_wait: invalid epfd");
        return -9_i64 as u64;  // EBADF
    }

    // 验证 events_ptr
    if events_ptr.is_null() {
        println!("sys_epoll_wait: events is null");
        return -14_i64 as u64;  // EFAULT
    }

    // 验证 maxevents
    if maxevents <= 0 || maxevents > 1024 {
        println!("sys_epoll_wait: invalid maxevents {}", maxevents);
        return -22_i64 as u64;  // EINVAL
    }

    // 简化实现：
    // 在真实实现中，应该：
    // 1. 查找 epfd 对应的 EpollFile
    // 2. 检查就绪队列（Linux 使用链表存储就绪事件）
    // 3. 等待事件或超时
    // 4. 将就绪事件复制到用户空间
    // TODO: 实现真实的等待逻辑

    // 当前简化：立即返回 0（超时）
    let _ = (epfd, events_ptr, maxevents, timeout_ms);

    println!("sys_epoll_wait: timeout (no events)");

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
///
/// # 说明
/// epoll_pwait 是 epoll_wait 的扩展版本，支持信号掩码
/// 简化实现：忽略信号掩码，调用 epoll_wait
fn sys_epoll_pwait(args: [u64; 6]) -> u64 {
    let epfd = args[0] as i32;
    let events_ptr = args[1] as *mut EPollEvent;
    let maxevents = args[2] as i32;
    let timeout_ms = args[3] as i32;
    let _sigmask_ptr = args[4] as *const u64;

    println!("sys_epoll_pwait: epfd={}, events={:#x}, maxevents={}, timeout={}ms",
             epfd, events_ptr as u64, maxevents, timeout_ms);

    // 简化实现：忽略信号掩码
    sys_epoll_wait([epfd as u64, events_ptr as u64, maxevents as u64, timeout_ms as u64, args[5], 0])
}

/// sys_eventfd - 创建 eventfd 对象
///
/// # 参数
/// - args[0]: initval - 初始值
///
/// # 返回
/// 成功返回 eventfd 文件描述符，失败返回负错误码
///
/// # 说明
/// eventfd 是一种进程间通信机制，用于事件通知
/// 简化实现：返回一个伪文件描述符
/// 参考实现: Linux kernel/fs/eventfd.c::sys_eventfd()
fn sys_eventfd(args: [u64; 6]) -> u64 {
    let initval = args[0] as u32;

    println!("sys_eventfd: initval={}", initval);

    // 获取当前进程的 fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => {
            println!("sys_eventfd: no fdtable");
            return -9_i64 as u64;  // EBADF
        }
    };

    // 分配文件描述符
    let eventfd_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            println!("sys_eventfd: failed to alloc fd");
            return -24_i64 as u64;  // EMFILE
        }
    };

    // 简化实现：
    // 在真实实现中，应该创建一个 EventFdFile 并安装到 fdtable
    // eventfd 本质上是一个 64 位计数器
    // TODO: 创建 EventFdFile 结构

    println!("sys_eventfd: created eventfd fd {}", eventfd_fd);

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
///
/// # 说明
/// eventfd2 是 eventfd 的扩展版本，支持标志位
/// 简化实现：忽略标志，调用 eventfd
fn sys_eventfd2(args: [u64; 6]) -> u64 {
    let initval = args[0] as u32;
    let flags = args[1] as i32;

    println!("sys_eventfd2: initval={}, flags={:#x}", initval, flags);

    // 简化实现：忽略标志
    // EFD_CLOEXEC (0x80000), EFD_NONBLOCK (0x800), EFD_SEMAPHORE (0x1) 等标志暂不支持
    sys_eventfd([initval as u64, args[2], args[3], args[4], args[5], 0])
}

fn sys_getpid(_args: [u64; 6]) -> u64 {
    use crate::process;
    process::current_pid() as u64
}

fn sys_getppid(_args: [u64; 6]) -> u64 {
    use crate::process;
    process::current_ppid() as u64
}

fn sys_getuid(_args: [u64; 6]) -> u64 {
    0  // root 用户
}

fn sys_getgid(_args: [u64; 6]) -> u64 {
    0  // root 组
}

fn sys_geteuid(_args: [u64; 6]) -> u64 {
    0
}

fn sys_getegid(_args: [u64; 6]) -> u64 {
    0
}

pub fn sys_exit(args: [u64; 6]) -> u64 {
    let exit_code = args[0] as i32;
    println!("sys_exit: exiting with code {}", exit_code);
    crate::sched::do_exit(exit_code);
}

fn sys_kill(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    println!("sys_kill: pid={}, sig={}", pid, sig);

    match crate::sched::send_signal(pid as u32, sig) {
        Ok(()) => 0,
        Err(e) => e as u32 as u64,
    }
}

fn sys_fork(_args: [u64; 6]) -> u64 {
    println!("sys_fork: creating new process");

    match crate::sched::do_fork() {
        Some(pid) => {
            println!("sys_fork: created process with PID {}", pid);
            pid as u64
        }
        None => {
            println!("sys_fork: failed to create process");
            (-1_i64) as u64  // 返回 -1 表示失败
        }
    }
}

pub fn sys_execve(args: [u64; 6]) -> u64 {
    use crate::fs::elf::ElfLoader;
    use crate::fs;

    let pathname_ptr = args[0] as *const u8;
    let _argv = args[1] as *const *const u8;
    let _envp = args[2] as *const *const u8;

    println!("sys_execve: called");

    // ===== 1. 读取文件名 =====
    if pathname_ptr.is_null() {
        println!("sys_execve: null pathname");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取文件名（简化：假设以 null 结尾，最大长度 256）
    let filename = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let filename_str = match core::str::from_utf8(filename) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_execve: invalid utf-8 filename");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_execve: pathname='{}'", filename_str);

    // ===== 2. 从文件系统读取文件 =====
    let file_data = fs::read_file_from_rootfs(filename_str);
    let file_data = match file_data {
        Some(data) => data,
        None => {
            println!("sys_execve: file not found: {}", filename_str);
            return -2_i64 as u64;  // ENOENT
        }
    };

    println!("sys_execve: file size = {} bytes", file_data.len());

    // ===== 3. 验证 ELF 格式 =====
    let validation_result = ElfLoader::validate(&file_data);
    if let Err(e) = validation_result {
        println!("sys_execve: invalid ELF: {:?}", e);
        return -8_i64 as u64;  // ENOEXEC
    }

    // ===== 4. 获取 ELF 头信息 =====
    let ehdr = ElfLoader::get_entry(&file_data);
    let entry = match ehdr {
        Ok(addr) => addr,
        Err(e) => {
            println!("sys_execve: failed to get entry: {:?}", e);
            return -8_i64 as u64;
        }
    };

    println!("sys_execve: ELF entry point = {:#x}", entry);

    // ===== 5. 获取程序头数量 =====
    let phdr_count = match ElfLoader::get_program_headers(&file_data) {
        Ok(count) => count,
        Err(e) => {
            println!("sys_execve: failed to get program headers: {:?}", e);
            return -8_i64 as u64;
        }
    };

    println!("sys_execve: {} program headers", phdr_count);

    // 获取 ELF 头
    let ehdr = match unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(&file_data) } {
        Some(e) => e,
        None => {
            println!("sys_execve: failed to get ELF header");
            return -8_i64 as u64;
        }
    };

    // ===== 6. 分析 PT_LOAD 段 =====
    for i in 0..phdr_count {
        if let Some(phdr) = unsafe { ehdr.get_program_header(&file_data, i) } {
            if phdr.is_load() {
                println!("  PT_LOAD[{}]: vaddr={:#x}, filesz={}, memsz={}, flags={:#x}",
                         i, phdr.p_vaddr, phdr.p_filesz, phdr.p_memsz, phdr.p_flags);
            }
        }
    }

    // ===== 7. 检查 PT_INTERP（动态链接器） =====
    if let Some(interp) = ElfLoader::get_interpreter(&file_data) {
        let interp_str = core::str::from_utf8(interp).unwrap_or("<invalid>");
        println!("sys_execve: interpreter: {}", interp_str);
    }

    // ===== 8. 创建用户地址空间 =====
    use crate::arch::riscv64::mm::{
        create_user_address_space, alloc_and_map_user_memory,
        PageTableEntry, PAGE_SIZE
    };

    let user_root_ppn = match create_user_address_space() {
        Some(ppn) => {
            println!("sys_execve: created user address space (root_ppn={:#x})", ppn);
            ppn
        }
        None => {
            println!("sys_execve: failed to create user address space");
            return -12_i64 as u64;  // ENOMEM
        }
    };

    // ===== 9. 加载 PT_LOAD 段 =====
    for i in 0..phdr_count {
        if let Some(phdr) = unsafe { ehdr.get_program_header(&file_data, i) } {
            if phdr.is_load() {
                let vaddr = phdr.p_vaddr;
                let memsz = phdr.p_memsz as usize;
                let filesz = phdr.p_filesz as usize;
                let offset = phdr.p_offset as usize;

                // 页对齐
                let aligned_vaddr = vaddr & !(PAGE_SIZE as u64 - 1);
                let aligned_size = ((memsz as u64 + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1)) as usize;

                // 计算页标志
                let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
                if phdr.p_flags & crate::fs::elf::PF_R != 0 {
                    flags |= PageTableEntry::R;
                }
                if phdr.p_flags & crate::fs::elf::PF_W != 0 {
                    flags |= PageTableEntry::W;
                }
                if phdr.p_flags & crate::fs::elf::PF_X != 0 {
                    flags |= PageTableEntry::X;
                }
                // 用户可访问
                flags |= PageTableEntry::U;

                // 分配并映射内存
                let phys_addr = unsafe {
                    match alloc_and_map_user_memory(user_root_ppn, aligned_vaddr, aligned_size as u64, flags) {
                        Some(addr) => addr,
                        None => {
                            println!("sys_execve: failed to allocate memory for segment at {:#x}", vaddr);
                            return -12_i64 as u64;  // ENOMEM
                        }
                    }
                };

                // 复制 ELF 数据到物理内存
                unsafe {
                    let offset_in_segment = vaddr - aligned_vaddr;
                    let dst = (phys_addr + offset_in_segment) as *mut u8;
                    let src = file_data.as_ptr().add(offset);

                    if filesz > 0 {
                        core::ptr::copy_nonoverlapping(src, dst, filesz);
                    }

                    // BSS 段清零
                    if memsz > filesz {
                        let bss_start = dst.add(filesz);
                        let bss_size = memsz - filesz;
                        core::ptr::write_bytes(bss_start, 0, bss_size);
                    }
                }

                println!("sys_execve: loaded segment: vaddr={:#x}, memsz={}, phys={:#x}",
                         vaddr, memsz, phys_addr);
            }
        }
    }

    // ===== 10. 分配用户栈 =====
    let user_stack_bottom = USER_STACK_TOP - (USER_STACK_SIZE as u64);

    let stack_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W
        | PageTableEntry::A | PageTableEntry::D | PageTableEntry::U;

    let user_stack_phys = unsafe {
        match alloc_and_map_user_memory(user_root_ppn, user_stack_bottom, USER_STACK_SIZE as u64, stack_flags) {
            Some(addr) => addr,
            None => {
                println!("sys_execve: failed to allocate user stack");
                return -12_i64 as u64;  // ENOMEM
            }
        }
    };

    println!("sys_execve: user stack: virt={:#x}, phys={:#x}", USER_STACK_TOP, user_stack_phys);

    // ===== 11. 设置 argv/envp 到用户栈 =====
    // Linux 栈布局（从高地址到低地址）：
    // | envp[n]     |
    // | ...         |
    // | envp[0]     |
    // | NULL        |  <- envp 数组结束
    // | argv[argc]  |  <- NULL
    // | argv[argc-1]|
    // | ...         |
    // | argv[0]     |
    // | argc        |  <- 栈指针指向这里

    let user_stack_with_args = match setup_user_stack(user_root_ppn, user_stack_phys, USER_STACK_TOP, args[1], args[2]) {
        Ok(sp) => sp,
        Err(e) => {
            println!("sys_execve: failed to setup user stack: {}", e);
            return -12_i64 as u64;  // ENOMEM
        }
    };

    println!("sys_execve: user stack with args: sp={:#x}", user_stack_with_args);

    // ===== 12. 切换到用户模式并执行 =====
    unsafe {
        switch_to_user(user_root_ppn, entry, user_stack_with_args);
    }

    // 不应该返回
    #[allow(unreachable_code)]
    {
        println!("sys_execve: unexpectedly returned from user mode");
        -1_i64 as u64
    }
}

fn setup_user_stack(
    _user_root_ppn: u64,
    user_stack_phys: u64,
    user_stack_top: u64,
    argv: u64,
    envp: u64,
) -> Result<u64, &'static str> {
    use alloc::vec::Vec;
    use core::slice;

    // ===== 1. 读取 argv 数组 =====
    let argv_ptr = argv as *const *const u8;
    let mut argv_strings: Vec<Vec<u8>> = Vec::new();

    if !argv_ptr.is_null() {
        unsafe {
            let mut i = 0;
            loop {
                let ptr = *argv_ptr.add(i);
                if ptr.is_null() {
                    break;
                }

                // 读取字符串
                let mut len = 0;
                let mut str_ptr = ptr;
                while len < 4096 {  // 最大长度限制
                    let byte = *str_ptr;
                    if byte == 0 {
                        break;
                    }
                    len += 1;
                    str_ptr = str_ptr.add(1);
                }

                let string_vec = slice::from_raw_parts(ptr, len).to_vec();
                argv_strings.push(string_vec);
                i += 1;

                if i >= 256 {  // 最多 256 个参数
                    break;
                }
            }
        }
    }

    let argc = argv_strings.len();

    // ===== 2. 读取 envp 数组 =====
    let envp_ptr = envp as *const *const u8;
    let mut envp_strings: Vec<Vec<u8>> = Vec::new();

    if !envp_ptr.is_null() {
        unsafe {
            let mut i = 0;
            loop {
                let ptr = *envp_ptr.add(i);
                if ptr.is_null() {
                    break;
                }

                // 读取字符串
                let mut len = 0;
                let mut str_ptr = ptr;
                while len < 4096 {
                    let byte = *str_ptr;
                    if byte == 0 {
                        break;
                    }
                    len += 1;
                    str_ptr = str_ptr.add(1);
                }

                let string_vec = slice::from_raw_parts(ptr, len).to_vec();
                envp_strings.push(string_vec);
                i += 1;

                if i >= 256 {  // 最多 256 个环境变量
                    break;
                }
            }
        }
    }

    println!("setup_user_stack: argc={}, envc={}", argc, envp_strings.len());

    // ===== 3. 计算需要的栈空间 =====
    // 栈布局（从高地址到低地址）：
    // | envp strings     |
    // | argv strings     |
    // | envp pointers    |
    // | NULL (envp 结束)  |
    // | NULL (argv[argc]) |
    // | argv pointers    |
    // | argc             |  <- SP

    let mut total_size = 0usize;

    // 环境变量字符串
    for s in &envp_strings {
        total_size += s.len() + 1;  // +1 for null terminator
    }
    // argv 字符串
    for s in &argv_strings {
        total_size += s.len() + 1;
    }

    // 指针对齐到 8 字节
    let ptr_size = 8;

    // envp 指针数组
    total_size += (envp_strings.len() + 1) * ptr_size;  // +1 for NULL

    // argv 指针数组
    total_size += (argc + 1) * ptr_size;  // +1 for NULL

    // argc
    total_size += 8;

    // 栈对齐到 16 字节
    total_size = (total_size + 15) & !15;

    println!("setup_user_stack: total stack size = {} bytes", total_size);

    // ===== 4. 在用户栈上布置数据 =====
    let mut current_vaddr = user_stack_top;
    let mut current_paddr = user_stack_phys;

    // 减去总大小
    current_vaddr -= total_size as u64;
    current_paddr -= total_size as u64;

    // 对齐栈指针
    current_vaddr &= !15;
    current_paddr &= !15;

    let _final_sp = current_vaddr;
    let mut offset = 0usize;

    // ===== 5. 写入字符串数据 =====
    // 首先写入所有环境变量字符串（在高地址）
    let mut envp_addrs: Vec<u64> = Vec::new();
    for s in &envp_strings {
        let str_vaddr = current_vaddr + offset as u64;
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u8;
            for (i, &byte) in s.iter().enumerate() {
                *dst.add(i) = byte;
            }
            *dst.add(s.len()) = 0;  // null terminator
        }
        envp_addrs.push(str_vaddr);
        offset += s.len() + 1;
    }

    // 然后 argv 字符串
    let mut argv_addrs: Vec<u64> = Vec::new();
    for s in &argv_strings {
        let str_vaddr = current_vaddr + offset as u64;
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u8;
            for (i, &byte) in s.iter().enumerate() {
                *dst.add(i) = byte;
            }
            *dst.add(s.len()) = 0;  // null terminator
        }
        argv_addrs.push(str_vaddr);
        offset += s.len() + 1;
    }

    // ===== 6. 写入指针数组 =====
    // 对齐到指针大小
    while offset % ptr_size != 0 {
        offset += 1;
    }

    // envp 指针数组
    for &addr in &envp_addrs {
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u64;
            *dst = addr;
        }
        offset += ptr_size;
    }
    // envp NULL 终止符
    unsafe {
        let dst = (current_paddr + offset as u64) as *mut u64;
        *dst = 0;
    }
    offset += ptr_size;

    // argv 指针数组（注意：需要倒序写入，因为栈从高地址向低地址增长）
    // 实际上我们不需要倒序，因为我们是从低地址向高地址构建的
    // 但是按照 Linux 的布局，argv[0] 应该在最低地址

    // 先写 argv NULL 终止符
    unsafe {
        let dst = (current_paddr + offset as u64) as *mut u64;
        *dst = 0;
    }
    offset += ptr_size;

    // 然后写 argv 指针（从后往前）
    for i in (0..argc).rev() {
        unsafe {
            let dst = (current_paddr + offset as u64) as *mut u64;
            *dst = argv_addrs[i];
        }
        offset += ptr_size;
    }

    // ===== 7. 写入 argc =====
    unsafe {
        let dst = (current_paddr + offset as u64) as *mut u64;
        *dst = argc as u64;
    }
    offset += 8;

    // 最终的栈指针应该在 argc 的位置
    let final_sp = current_vaddr + offset as u64 - 8;

    println!("setup_user_stack: final sp={:#x}, argc={}, argv={:#x}", final_sp, argc,
             if argc > 0 { argv_addrs[0] } else { 0 });

    Ok(final_sp)
}

unsafe fn switch_to_user(user_root_ppn: u64, entry: u64, user_stack: u64) -> ! {
    use crate::arch::riscv64::mm::Satp;

    // 保存当前内核栈
    let _kernel_stack: u64;
    core::arch::asm!("mv {}, sp", out(reg) _kernel_stack);

    // 设置用户页表
    let satp = Satp::sv39(user_root_ppn, 0);
    println!("sys_execve: switching to user mode, satp={:#x}, entry={:#x}, sp={:#x}",
             satp.0, entry, user_stack);

    // 设置用户模式下的寄存器状态
    // RISC-V User 模式:
    // - mstatus.MPP = 00 (U-mode)
    // - mstatus.MPIE = 1 (启用中断返回)
    // - mepc = entry point
    // - sp = user_stack

    core::arch::asm!(
        // 1. 设置用户栈
        "mv sp, {2}",

        // 2. 设置 mstatus (进入用户模式)
        // MPP = 00 (U-mode), MPIE = 1
        "li t0, 0x1880",  // MPP=0 (bits 12:11), MPIE=1 (bit 7), MIE=1 (bit 3)
        "csrw mstatus, t0",

        // 3. 设置 mepc (用户程序入口点)
        "csrw mepc, {0}",

        // 4. 设置 mtvec (内核陷阱向量，用于系统调用)
        // 这应该已经在 trap::init() 中设置好了

        // 5. 设置 satp (用户页表)
        "csrw satp, {1}",

        // 6. 刷新 TLB
        "sfence.vma zero, zero",

        // 7. mret - 返回到用户模式
        "mret",

        // 参数
        in(reg) entry,
        in(reg) satp.0,
        in(reg) user_stack,

        options(nostack, noreturn)
    );
}

pub fn sys_wait4(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    // WNOHANG: 如果没有子进程退出，立即返回 0
    // Linux 行为：如果有子进程但未退出，返回 0；如果没有子进程，返回 ECHILD
    const WNOHANG: i32 = 0x00000001;

    if options & WNOHANG != 0 {
        // WNOHANG 模式：非阻塞检查
        match crate::sched::do_wait_nonblock(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) if e == -11 => 0,  // EAGAIN -> 返回 0 表示没有子进程退出
            Err(e) => e as u32 as u64,
        }
    } else {
        // 阻塞等待子进程退出
        // 现在会真正阻塞，直到子进程退出
        match crate::sched::do_wait(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) => e as u32 as u64,
        }
    }
}

fn sys_uname(_args: [u64; 6]) -> u64 {
    println!("sys_uname: not fully implemented");
    -38_i64 as u64  // ENOSYS
}

fn sys_gettimeofday(_args: [u64; 6]) -> u64 {
    println!("sys_gettimeofday: not implemented");
    -38_i64 as u64  // ENOSYS
}

fn sys_clock_gettime(_args: [u64; 6]) -> u64 {
    println!("sys_clock_gettime: not implemented");
    -38_i64 as u64  // ENOSYS
}

// ============================================================================
// 睡眠和等待系统调用
// ============================================================================

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,   // 秒
    pub tv_nsec: i64,  // 纳秒
}

fn sys_nanosleep(args: [u64; 6]) -> u64 {
    use crate::drivers::timer;
    use crate::process;

    let req_ptr = args[0] as *const Timespec;
    let rem_ptr = args[1] as *mut Timespec;

    // 检查请求指针有效性
    if req_ptr.is_null() {
        println!("sys_nanosleep: null req pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取请求的睡眠时间
    let req = unsafe { *req_ptr };
    let total_nanos = req.tv_sec * 1_000_000_000 + req.tv_nsec;

    println!("sys_nanosleep: sleeping for {}s {}ns (total {}ns)",
             req.tv_sec, req.tv_nsec, total_nanos);

    // 转换为毫秒
    let sleep_msecs = (total_nanos / 1_000_000) as u64;

    // 如果睡眠时间为 0，直接返回
    if sleep_msecs == 0 {
        return 0;
    }

    // 获取当前 jiffies
    let start_jiffies = timer::get_jiffies();

    // 计算目标 jiffies
    let sleep_jiffies = timer::msecs_to_jiffies(sleep_msecs);
    let target_jiffies = start_jiffies + sleep_jiffies;

    println!("sys_nanosleep: start_jiffies={}, sleep_jiffies={}, target={}",
             start_jiffies, sleep_jiffies, target_jiffies);

    // 循环睡眠，直到达到目标时间
    // 对应 Linux 内核的 schedule_timeout() (kernel/timer.c)
    loop {
        let current_jiffies = timer::get_jiffies();

        // 检查是否已经达到目标时间
        if current_jiffies >= target_jiffies {
            println!("sys_nanosleep: sleep completed, current_jiffies={}", current_jiffies);
            return 0;  // 成功
        }

        // 计算剩余时间
        let remaining_jiffies = target_jiffies - current_jiffies;
        let remaining_msecs = timer::jiffies_to_msecs(remaining_jiffies);

        println!("sys_nanosleep: sleeping, remaining {} msecs", remaining_msecs);

        // 检查是否有待处理信号
        // 对应 Linux 内核的 signal_pending() (include/linux/sched/signal.h)
        use crate::signal;
        if signal::signal_pending() {
            println!("sys_nanosleep: interrupted by signal");

            // 写入剩余时间到 rem（如果提供了 rem_ptr）
            if !rem_ptr.is_null() {
                unsafe {
                    // 将毫秒转换为 timespec
                    let rem_sec = (remaining_msecs / 1000) as i64;
                    let rem_nsec = ((remaining_msecs % 1000) * 1_000_000) as i64;
                    *rem_ptr = Timespec {
                        tv_sec: rem_sec,
                        tv_nsec: rem_nsec,
                    };
                }
            }

            return -4_i64 as u64;  // EINTR
        }

        // 使用 Task::sleep() 进入可中断睡眠
        // 注意：这里会触发调度，醒来后继续检查时间
        process::Task::sleep(crate::process::task::TaskState::Interruptible);
    }
}

fn sys_dup(args: [u64; 6]) -> u64 {
    let oldfd = args[0] as usize;
    println!("sys_dup: oldfd={}", oldfd);
    -24_i64 as u64  // EMFILE
}

fn sys_dup2(args: [u64; 6]) -> u64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;
    println!("sys_dup2: oldfd={}, newfd={}", oldfd, newfd);
    -24_i64 as u64  // EMFILE
}

/// sys_fstat - 获取文件状态信息
///
/// 对应 Linux 的 sys_fstat (fs/stat.c)
///
/// # 参数
/// - args[0] (fd): 文件描述符
/// - args[1] (statbuf): 指向 stat 结构的指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 80
fn sys_fstat(args: [u64; 6]) -> u64 {
    use crate::fs::{file_stat, Stat};

    let fd = args[0] as usize;
    let statbuf = args[1] as *mut Stat;

    // 检查 statbuf 指针有效性
    if statbuf.is_null() {
        println!("sys_fstat: null statbuf pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 创建临时 stat 结构
    let mut stat = Stat::new();

    // 调用 VFS 层的 file_stat
    match file_stat(fd, &mut stat) {
        Ok(()) => {
            // 将 stat 结构复制到用户空间
            unsafe {
                *statbuf = stat;
            }
            0  // 成功
        }
        Err(errno) => {
            println!("sys_fstat: file_stat failed for fd={}, error={}", fd, errno);
            errno as u64  // 返回错误码
        }
    }
}

fn sys_fcntl(args: [u64; 6]) -> u64 {
    use crate::fs::file_fcntl;

    let fd = args[0] as usize;
    let cmd = args[1] as usize;
    let arg = args[2] as usize;

    match file_fcntl(fd, cmd, arg) {
        Ok(result) => result as u64,
        Err(errno) => errno as u64,
    }
}

/// sys_mkdir - 创建目录
///
/// 对应 Linux 的 sys_mkdirat (fs/namei.c)
///
/// # 参数
/// - args[0] (pathname): 目录路径指针
/// - args[1] (mode): 目录权限
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 77 (mkdirat), 我们实现简化版 mkdir
fn sys_mkdir(args: [u64; 6]) -> u64 {
    use crate::fs::file_mkdir;

    let pathname_ptr = args[0] as *const u8;
    let mode = args[1] as u32;

    // 检查路径指针有效性
    if pathname_ptr.is_null() {
        println!("sys_mkdir: null pathname pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取目录名（假设以 null 结尾，最大长度 256）
    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    // 转换为字符串
    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_mkdir: invalid utf-8 pathname");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_mkdir: pathname='{}', mode={:#o}", pathname_str, mode);

    // 调用 VFS 层创建目录
    match file_mkdir(pathname_str, mode) {
        Ok(()) => 0,  // 成功
        Err(errno) => errno as u64,
    }
}

/// sys_rmdir - 删除目录
///
/// 对应 Linux 的 sys_rmdir (fs/namei.c)
///
/// # 参数
/// - args[0] (pathname): 目录路径指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 79
fn sys_rmdir(args: [u64; 6]) -> u64 {
    use crate::fs::file_rmdir;

    let pathname_ptr = args[0] as *const u8;

    // 检查路径指针有效性
    if pathname_ptr.is_null() {
        println!("sys_rmdir: null pathname pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取目录名（假设以 null 结尾，最大长度 256）
    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    // 转换为字符串
    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_rmdir: invalid utf-8 pathname");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_rmdir: pathname='{}'", pathname_str);

    // 调用 VFS 层删除目录
    match file_rmdir(pathname_str) {
        Ok(()) => 0,  // 成功
        Err(errno) => errno as u64,
    }
}

/// sys_unlink - 删除文件
///
/// 对应 Linux 的 sys_unlinkat (fs/namei.c)
///
/// # 参数
/// - args[0] (pathname): 文件路径指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 74 (unlinkat), 我们实现简化版 unlink
fn sys_unlink(args: [u64; 6]) -> u64 {
    use crate::fs::file_unlink;

    let pathname_ptr = args[0] as *const u8;

    // 检查路径指针有效性
    if pathname_ptr.is_null() {
        println!("sys_unlink: null pathname pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取文件名（假设以 null 结尾，最大长度 256）
    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    // 转换为字符串
    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_unlink: invalid utf-8 pathname");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_unlink: pathname='{}'", pathname_str);

    // 调用 VFS 层删除文件
    match file_unlink(pathname_str) {
        Ok(()) => 0,  // 成功
        Err(errno) => errno as u64,
    }
}

/// sys_link - 创建硬链接
///
/// 对应 Linux 的 sys_linkat (fs/namei.c)
///
/// # 参数
/// - args[0] (oldpath): 已存在的文件路径指针
/// - args[1] (newpath): 新链接路径指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 78 (linkat), 我们实现简化版 link
fn sys_link(args: [u64; 6]) -> u64 {
    use crate::fs::file_link;

    let oldpath_ptr = args[0] as *const u8;
    let newpath_ptr = args[1] as *const u8;

    // 检查路径指针有效性
    if oldpath_ptr.is_null() {
        println!("sys_link: null oldpath pointer");
        return -14_i64 as u64;  // EFAULT
    }
    if newpath_ptr.is_null() {
        println!("sys_link: null newpath pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取旧文件名
    let oldpath = unsafe {
        let mut len = 0;
        let mut ptr = oldpath_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(oldpath_ptr, len)
    };

    // 读取新文件名
    let newpath = unsafe {
        let mut len = 0;
        let mut ptr = newpath_ptr;
        while len < 256 {
            let byte = *ptr;
            if byte == 0 {
                break;
            }
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(newpath_ptr, len)
    };

    // 转换为字符串
    let oldpath_str = match core::str::from_utf8(oldpath) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_link: invalid utf-8 oldpath");
            return -22_i64 as u64;  // EINVAL
        }
    };

    let newpath_str = match core::str::from_utf8(newpath) {
        Ok(s) => s,
        Err(_) => {
            println!("sys_link: invalid utf-8 newpath");
            return -22_i64 as u64;  // EINVAL
        }
    };

    println!("sys_link: oldpath='{}', newpath='{}'", oldpath_str, newpath_str);

    // 调用 VFS 层创建硬链接
    match file_link(oldpath_str, newpath_str) {
        Ok(()) => 0,  // 成功
        Err(errno) => errno as u64,
    }
}

// ============================================================================
// 网络系统调用
// ============================================================================

/// sys_socket - 创建 socket
///
/// 对应 Linux 的 sys_socket (net/socket.c)
///
/// # 参数
/// - args[0] (domain): 协议族 (AF_INET=2)
/// - args[1] (type): socket 类型 (SOCK_STREAM=1, SOCK_DGRAM=2)
/// - args[2] (protocol): 协议类型 (IPPROTO_TCP=6, IPPROTO_UDP=17)
///
/// # 返回
/// 成功返回文件描述符，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 198
fn sys_socket(args: [u64; 6]) -> u64 {
    let domain = args[0] as i32;
    let type_ = args[1] as i32;
    let protocol = args[2] as i32;

    println!("sys_socket: domain={}, type={}, protocol={}", domain, type_, protocol);

    // 目前只支持 AF_INET (IPv4)
    if domain != 2 {
        println!("sys_socket: unsupported domain {}", domain);
        return -97_i64 as u64;  // EAFNOSUPPORT
    }

    match type_ {
        1 => {
            // SOCK_STREAM (TCP)
            if protocol != 0 && protocol != 6 {
                println!("sys_socket: invalid protocol {} for SOCK_STREAM", protocol);
                return -22_i64 as u64;  // EINVAL
            }

            use crate::net::tcp;
            match tcp::tcp_socket_alloc() {
                Ok(fd) => fd as u64,
                Err(e) => {
                    println!("sys_socket: tcp_socket_alloc failed: {}", e);
                    e as u64
                }
            }
        }
        2 => {
            // SOCK_DGRAM (UDP)
            if protocol != 0 && protocol != 17 {
                println!("sys_socket: invalid protocol {} for SOCK_DGRAM", protocol);
                return -22_i64 as u64;  // EINVAL
            }

            use crate::net::udp;
            match udp::udp_socket_alloc() {
                Ok(fd) => fd as u64,
                Err(e) => {
                    println!("sys_socket: udp_socket_alloc failed: {}", e);
                    e as u64
                }
            }
        }
        _ => {
            println!("sys_socket: unsupported socket type {}", type_);
            -94_i64 as u64  // ESOCKTNOSUPPORT
        }
    }
}

/// sys_bind - 绑定 socket 到地址
///
/// 对应 Linux 的 sys_bind (net/socket.c)
///
/// # 参数
/// - args[0] (fd): socket 文件描述符
/// - args[1] (addr): sockaddr 结构指针
/// - args[2] (addrlen): 地址长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 200
fn sys_bind(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    println!("sys_bind: fd={}, addr={:#x}", fd, addr_ptr as usize);

    // 检查地址指针有效性
    if addr_ptr.is_null() {
        println!("sys_bind: null addr pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取 sockaddr_in 结构（简化实现）
    // struct sockaddr_in {
    //     sa_family_t sin_family;  // 2 bytes
    //     in_port_t sin_port;      // 2 bytes (network byte order)
    //     struct in_addr sin_addr; // 4 bytes
    //     char sin_zero[8];        // 8 bytes
    // };

    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };
    let sin_addr = unsafe { u32::from_be_bytes(*((addr_ptr.add(4)) as *const [u8; 4])) };

    println!("sys_bind: family={}, port={}, addr={:#x}", sin_family, sin_port, sin_addr);

    // 目前只支持 AF_INET
    if sin_family != 2 {
        println!("sys_bind: unsupported family {}", sin_family);
        return -97_i64 as u64;  // EAFNOSUPPORT
    }

    // TODO: 需要一种方法确定 fd 是 TCP 还是 UDP socket
    // 简化实现：尝试两种协议
    use crate::net::{tcp, udp};

    // 先尝试 TCP
    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        println!("sys_bind: binding TCP socket {} to port {}", fd, sin_port);
        return tcp::tcp_bind(fd, sin_port) as u64;
    }

    // 再尝试 UDP
    if let Some(_socket) = udp::udp_socket_get(fd) {
        println!("sys_bind: binding UDP socket {} to port {}", fd, sin_port);
        return udp::udp_bind(fd, sin_port) as u64;
    }

    println!("sys_bind: invalid fd {}", fd);
    -9_i64 as u64  // EBADF
}

/// sys_listen - 监听 socket
///
/// 对应 Linux 的 sys_listen (net/socket.c)
///
/// # 参数
/// - args[0] (fd): socket 文件描述符
/// - args[1] (backlog): 等待连接队列长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 201
fn sys_listen(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let backlog = args[1] as i32;

    println!("sys_listen: fd={}, backlog={}", fd, backlog);

    use crate::net::tcp;

    if let Some(_socket) = tcp::tcp_socket_get(fd) {
        tcp::tcp_listen(fd, backlog as u32) as u64
    } else {
        println!("sys_listen: invalid fd {}", fd);
        -9_i64 as u64  // EBADF
    }
}

/// sys_accept - 接受连接
///
/// 对应 Linux 的 sys_accept (net/socket.c)
///
/// # 参数
/// - args[0] (fd): socket 文件描述符
/// - args[1] (addr): sockaddr 结构指针（输出）
/// - args[2] (addrlen): 地址长度指针（输入/输出）
///
/// # 返回
/// 成功返回新 socket 的文件描述符，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 202
fn sys_accept(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let _addr_ptr = args[1] as *mut u8;
    let _addrlen_ptr = args[2] as *mut u32;

    println!("sys_accept: fd={}", fd);

    use crate::net::tcp;

    // TODO: 实现完整的 accept 逻辑
    // 1. 检查 socket 是否处于 LISTEN 状态
    // 2. 从等待队列中取出连接请求
    // 3. 创建新的 socket
    // 4. 将新 socket 设置为 ESTABLISHED 状态
    // 5. 返回新 socket 的 fd

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_accept(fd) as u64,
        None => {
            println!("sys_accept: invalid fd {}", fd);
            -9_i64 as u64  // EBADF
        }
    }
}

/// sys_connect - 连接到远程地址
///
/// 对应 Linux 的 sys_connect (net/socket.c)
///
/// # 参数
/// - args[0] (fd): socket 文件描述符
/// - args[1] (addr): sockaddr 结构指针
/// - args[2] (addrlen): 地址长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 203
fn sys_connect(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let addr_ptr = args[1] as *const u8;
    let _addrlen = args[2] as u32;

    println!("sys_connect: fd={}, addr={:#x}", fd, addr_ptr as usize);

    // 检查地址指针有效性
    if addr_ptr.is_null() {
        println!("sys_connect: null addr pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 读取 sockaddr_in 结构
    let sin_family = unsafe { u16::from_le_bytes(*(addr_ptr as *const [u8; 2])) };
    let sin_port = unsafe { u16::from_be_bytes(*((addr_ptr.add(2)) as *const [u8; 2])) };
    let sin_addr = unsafe { u32::from_be_bytes(*((addr_ptr.add(4)) as *const [u8; 4])) };

    println!("sys_connect: family={}, port={}, addr={:#x}", sin_family, sin_port, sin_addr);

    // 目前只支持 AF_INET
    if sin_family != 2 {
        println!("sys_connect: unsupported family {}", sin_family);
        return -97_i64 as u64;  // EAFNOSUPPORT
    }

    use crate::net::tcp;

    match tcp::tcp_socket_get(fd) {
        Some(_socket) => tcp::tcp_connect(fd, sin_addr, sin_port) as u64,
        None => {
            println!("sys_connect: invalid fd {}", fd);
            -9_i64 as u64  // EBADF
        }
    }
}

/// sys_sendto - 发送数据（可能指定目标地址）
///
/// 对应 Linux 的 sys_sendto (net/socket.c)
///
/// # 参数
/// - args[0] (fd): socket 文件描述符
/// - args[1] (buf): 数据缓冲区指针
/// - args[2] (len): 数据长度
/// - args[3] (flags): 标志位
/// - args[4] (addr): 目标地址指针（可选）
/// - args[5] (addrlen): 地址长度（可选）
///
/// # 返回
/// 成功返回发送的字节数，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 206
fn sys_sendto(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let buf_ptr = args[1] as *const u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let _addr_ptr = args[4] as *const u8;
    let _addrlen = args[5] as u32;

    println!("sys_sendto: fd={}, buf={:#x}, len={}", fd, buf_ptr as usize, len);

    // 检查缓冲区指针有效性
    if buf_ptr.is_null() {
        println!("sys_sendto: null buf pointer");
        return -14_i64 as u64;  // EFAULT
    }

    if len == 0 {
        return 0;
    }

    // 读取数据
    let data = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    // TODO: 需要确定是 TCP 还是 UDP socket
    // 简化实现：暂时返回错误
    println!("sys_sendto: not fully implemented, data={}", data.len());

    -38_i64 as u64  // ENOSYS
}

/// sys_recvfrom - 接收数据（可能获取源地址）
///
/// 对应 Linux 的 sys_recvfrom (net/socket.c)
///
/// # 参数
/// - args[0] (fd): socket 文件描述符
/// - args[1] (buf): 数据缓冲区指针
/// - args[2] (len): 缓冲区长度
/// - args[3] (flags): 标志位
/// - args[4] (addr): 源地址指针（可选，输出）
/// - args[5] (addrlen): 地址长度指针（可选，输入/输出）
///
/// # 返回
/// 成功返回接收的字节数，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 207
fn sys_recvfrom(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let buf_ptr = args[1] as *mut u8;
    let len = args[2] as usize;
    let _flags = args[3] as i32;
    let _addr_ptr = args[4] as *mut u8;
    let _addrlen_ptr = args[5] as *mut u32;

    println!("sys_recvfrom: fd={}, buf={:#x}, len={}", fd, buf_ptr as usize, len);

    // 检查缓冲区指针有效性
    if buf_ptr.is_null() {
        println!("sys_recvfrom: null buf pointer");
        return -14_i64 as u64;  // EFAULT
    }

    if len == 0 {
        return 0;
    }

    // TODO: 需要确定是 TCP 还是 UDP socket
    // 简化实现：暂时返回错误
    println!("sys_recvfrom: not fully implemented");

    -38_i64 as u64  // ENOSYS
}

/// sys_brk - 改变数据段大小
///
/// 对应 Linux 的 sys_brk (mm/mmap.c)
///
/// # 参数
/// - args[0] (addr): 新的堆顶部地址
///
/// # 返回
/// 成功返回新的堆顶部地址，失败返回当前地址（无变化）
///
/// # 行为
/// - 如果 addr 为 0，返回当前 brk 值
/// - 如果 addr 小于当前 brk，缩小堆并返回新值
/// - 如果 addr 大于当前 brk，尝试扩展堆并返回新值
/// - 如果扩展失败，返回当前值（无变化）
///
/// # Linux 系统调用号
/// - RISC-V: 214
fn sys_brk(args: [u64; 6]) -> u64 {
    use crate::sched;

    let new_brk = args[0] as usize;

    // 获取当前进程
    match sched::current() {
        Some(current_task) => {
            // 检查是否有地址空间
            match current_task.address_space() {
                Some(_address_space) => {
                    // TODO: 实现 per-task brk 管理
                    // 当前简化实现：返回请求的地址
                    println!("sys_brk: new_brk={:#x}", new_brk);

                    // 暂时返回请求的地址（不验证）
                    new_brk as u64
                }
                None => {
                    println!("sys_brk: no address space");
                    -12_i64 as u64  // ENOMEM
                }
            }
        }
        None => {
            println!("sys_brk: no current task");
            -12_i64 as u64  // ENOMEM
        }
    }
}

/// sys_mmap - 创建内存映射
///
/// 对应 Linux 的 sys_mmap (mm/mmap.c)
///
/// # 参数
/// - args[0] (addr): 建议的起始地址
/// - args[1] (length): 映射长度
/// - args[2] (prot): 保护标志 (PROT_READ/WRITE/EXEC)
/// - args[3] (flags): 映射标志 (MAP_PRIVATE/SHARED/ANONYMOUS)
/// - args[4] (fd): 文件描述符
/// - args[5] (offset): 文件偏移
///
/// # 返回
/// 成功返回映射的起始地址，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 222
fn sys_mmap(args: [u64; 6]) -> u64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::{VmaFlags, VmaType};
    use crate::mm::pagemap::Perm;

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let prot = args[2] as u32;
    let flags = args[3] as u32;
    let _fd = args[4] as i32;
    let _offset = args[5] as u64;

    println!("sys_mmap: addr={:#x}, length={}, prot={:#x}, flags={:#x}",
             addr, length, prot, flags);

    // 获取当前进程
    match crate::sched::current() {
        Some(current_task) => {
            // 检查是否有地址空间
            match current_task.address_space_mut() {
                Some(address_space) => {
                    // 解析保护标志
                    let mut perm = Perm::None;
                    if prot & 0x1 != 0 {  // PROT_READ
                        perm = Perm::Read;
                    }
                    if prot & 0x2 != 0 {  // PROT_WRITE
                        perm = Perm::ReadWrite;
                    }
                    if prot & 0x4 != 0 {  // PROT_EXEC
                        match perm {
                            Perm::None => perm = Perm::ReadWriteExec,
                            Perm::Read => perm = Perm::ReadWriteExec, // 简化：假设读+执行
                            Perm::ReadWrite => perm = Perm::ReadWriteExec,
                            _ => {}
                        }
                    }

                    // 解析映射标志
                    let mut vma_flags = VmaFlags::new();
                    if flags & 0x01 != 0 {  // MAP_SHARED
                        vma_flags.insert(VmaFlags::SHARED);
                    }
                    if flags & 0x02 != 0 {  // MAP_PRIVATE
                        vma_flags.insert(VmaFlags::PRIVATE);
                    }
                    if flags & 0x20 != 0 {  // MAP_ANONYMOUS
                        // 匿名映射
                    }

                    // 设置 VMA 类型
                    let vma_type = if flags & 0x20 != 0 {
                        VmaType::Anonymous
                    } else {
                        VmaType::FileBacked
                    };

                    // 调用 AddressSpace::mmap
                    match address_space.mmap(
                        VirtAddr::new(addr),
                        length,
                        vma_flags,
                        vma_type,
                        perm,
                    ) {
                        Ok(mapped_addr) => {
                            println!("sys_mmap: mapped at {:#x}", mapped_addr.as_usize());
                            mapped_addr.as_usize() as u64
                        }
                        Err(e) => {
                            println!("sys_mmap: mmap failed: {:?}", e);
                            -12_i64 as u64  // ENOMEM
                        }
                    }
                }
                None => {
                    println!("sys_mmap: no address space");
                    -12_i64 as u64  // ENOMEM
                }
            }
        }
        None => {
            println!("sys_mmap: no current task");
            -12_i64 as u64  // ENOMEM
        }
    }
}

/// sys_munmap - 取消内存映射
///
/// 对应 Linux 的 sys_munmap (mm/mmap.c)
///
/// # 参数
/// - args[0] (addr): 起始地址
/// - args[1] (length): 长度
///
/// # 返回
/// 成功返回 0，失败返回负错误码
///
/// # Linux 系统调用号
/// - RISC-V: 215
fn sys_munmap(args: [u64; 6]) -> u64 {
    use crate::mm::page::VirtAddr;

    let addr = args[0] as usize;
    let length = args[1] as usize;

    println!("sys_munmap: addr={:#x}, length={}", addr, length);

    // 获取当前进程
    match crate::sched::current() {
        Some(current_task) => {
            // 检查是否有地址空间
            match current_task.address_space_mut() {
                Some(address_space) => {
                    // 调用 AddressSpace::munmap
                    match address_space.munmap(VirtAddr::new(addr), length) {
                        Ok(()) => {
                            println!("sys_munmap: unmapped successfully");
                            0
                        }
                        Err(e) => {
                            println!("sys_munmap: munmap failed: {:?}", e);
                            -12_i64 as u64  // ENOMEM
                        }
                    }
                }
                None => {
                    println!("sys_munmap: no address space");
                    -12_i64 as u64  // ENOMEM
                }
            }
        }
        None => {
            println!("sys_munmap: no current task");
            -12_i64 as u64  // ENOMEM
        }
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

#[inline]
pub fn get_syscall_no() -> u64 {
    let no: u64;
    unsafe {
        asm!("mv {}, a7", out(reg) no, options(nomem, nostack, pure));
    }
    no
}

#[inline]
pub unsafe fn set_syscall_ret(val: u64) {
    asm!("mv a0, {}", in(reg) val, options(nomem, nostack));
}

const USER_SPACE_END: u64 = 0x0000_ffff_ffff_ffff;

#[inline]
pub unsafe fn verify_user_ptr(ptr: u64) -> bool {
    ptr <= USER_SPACE_END
}

pub unsafe fn verify_user_ptr_array(ptr: u64, size: usize) -> bool {
    if ptr > USER_SPACE_END {
        return false;
    }

    match ptr.checked_add(size as u64) {
        Some(end) if end <= USER_SPACE_END => true,
        _ => false,
    }
}

pub fn sys_write_impl(fd: i32, buf: *const u8, count: usize) -> u64 {
    use crate::console::putchar;

    unsafe {
        // Special handling for stdout (1) and stderr (2) - write directly to UART
        if fd == 1 || fd == 2 {
            let slice = core::slice::from_raw_parts(buf, count);
            for &b in slice {
                putchar(b);
            }
            return count as u64;
        }

        // 其他文件描述符：使用 VFS
        use crate::fs::get_file_fd;
        match get_file_fd(fd as usize) {
            Some(_file) => {
                // TODO: 实现 VFS write
                crate::println!("sys_write: fd={}, count={} (VFS not implemented)", fd, count);
                -9_i32 as u64  // EBADF
            }
            None => {
                crate::println!("sys_write: invalid fd {}", fd);
                -9_i32 as u64  // EBADF
            }
        }
    }
}
