//! ARMv8 系统调用处理
//!
//! 实现基于 SVC (Supervisor Call) 指令的系统调用接口

use core::arch::asm;
use crate::println;
use crate::debug_println;
use crate::fs::{File, FileFlags, FileOps, Pipe, get_file_fd, get_file_fd_install, close_file_fd, CharDev};
use crate::signal::{SigAction, SigFlags, Signal};
use alloc::sync::Arc;

/// 系统调用编号
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SyscallNo {
    /// 读写操作
    Read = 0,
    Write = 1,

    /// 文件操作
    Open = 2,
    Close = 3,
    Stat = 4,
    Fstat = 5,
    Lstat = 6,
    Poll = 7,
    Lseek = 8,
    Mmap = 9,
    Mprotect = 10,
    Munmap = 11,
    Brk = 12,
    RtSigaction = 13,
    RtSigprocmask = 14,
    RtSigreturn = 15,
    Ioctl = 16,
    Pread64 = 17,
    Pwrite64 = 18,
    Readv = 19,
    Writev = 20,
    Access = 21,
    Pipe = 22,
    Select = 23,
    SchedYield = 24,
    Mremap = 25,
    Msync = 26,
    Mincore = 27,
    Madvise = 28,
    Shmget = 29,
    Shmat = 30,
    Shmctl = 31,
    Dup = 32,
    Dup2 = 33,
    Pause = 34,
    Nanosleep = 35,
    Getitimer = 36,
    Alarm = 37,
    Setitimer = 38,
    Getpid = 39,
    Sendfile = 40,
    Socket = 41,
    Connect = 42,
    Accept = 43,
    Sendto = 44,
    Recvfrom = 45,
    Sendmsg = 46,
    Recvmsg = 47,
    Shutdown = 48,
    Bind = 49,
    Listen = 50,
    Getsockname = 51,
    Getpeername = 52,
    Socketpair = 53,
    Setsockopt = 54,
    Getsockopt = 55,
    Clone = 56,
    Fork = 57,
    Vfork = 58,
    Execve = 59,
    Exit = 60,
    Wait4 = 61,
    Kill = 62,
    Uname = 63,
    Semget = 64,
    Semop = 65,
    Semctl = 66,
    Shmdt = 67,
    Msgget = 68,
    Msgsnd = 69,
    Msgrcv = 70,
    Msgctl = 71,
    Fcntl = 72,
    Flock = 73,
    Fsync = 74,
    Fdatasync = 75,
    Truncate = 76,
    Ftruncate = 77,
    Getdents = 78,
    Getcwd = 79,
    Chdir = 80,
    Fchdir = 81,
    Rename = 82,
    Mkdir = 83,
    Rmdir = 84,
    Creat = 85,
    Link = 86,
    Unlink = 87,
    Symlink = 88,
    Readlink = 89,
    Chmod = 90,
    Fchmod = 91,
    Chown = 92,
    Fchown = 93,
    Lchown = 94,
    Umask = 95,
    Gettimeofday = 96,
    Getrlimit = 97,
    Getrusage = 98,
    Sysinfo = 99,
    Times = 100,
    Getuid = 102,
    Getgid = 104,
    Setuid = 105,
    Setgid = 106,
    Geteuid = 107,
    Getegid = 108,
    Setpgid = 109,
    Getppid = 110,
    Getpgrp = 111,
    Setsid = 112,
    Setreuid = 113,
    Setregid = 114,
    Getgroups = 115,
    Setgroups = 116,
    Setresuid = 117,
    Getresuid = 118,
    Setresgid = 119,
    Getresgid = 120,
    Getpgid = 121,
    Setfsuid = 122,
    Setfsgid = 123,
    Getsid = 124,
    Capget = 125,
    Capset = 126,
    RtSigpending = 127,
    RtSigtimedwait = 128,
    RtSigqueueinfo = 129,
    RtSigsuspend = 130,
    Sigaltstack = 131,
    Utime = 132,
    Mknod = 133,
    Uselib = 134,
    Personality = 135,
    Ustat = 136,
    Statfs = 137,
    Fstatfs = 138,
    Sysfs = 139,
    Getpriority = 140,
    Setpriority = 141,
    SchedSetparam = 142,
    SchedGetparam = 143,
    SchedSetscheduler = 144,
    SchedGetscheduler = 145,
    SchedGetPriorityMax = 146,
    SchedGetPriorityMin = 147,
    SchedRrGetInterval = 148,
    Mlock = 149,
    Munlock = 150,
    Mlockall = 151,
    Munlockall = 152,
    Vhangup = 153,
    PivotRoot = 154,
    Prctl = 155,
    ArchPrctl = 156,
    Adjtimex = 157,
    Setrlimit = 158,
    Chroot = 159,
    Sync = 160,
    Acct = 161,
    Settimeofday = 162,
    Mount = 163,
    Umount2 = 164,
    Swapon = 165,
    Swapoff = 166,
    Reboot = 167,
    Sethostname = 168,
    Setdomainname = 169,
    Iopl = 170,
    Ioperm = 171,
    InitModule = 172,
    DeleteModule = 173,
    Quotactl = 174,
    Gettid = 175,
    Readahead = 176,
    Setxattr = 177,
    Lsetxattr = 178,
    Fsetxattr = 179,
    Getxattr = 180,
    Lgetxattr = 181,
    Fgetxattr = 182,
    Listxattr = 183,
    Llistxattr = 184,
    Flistxattr = 185,
    Removexattr = 186,
    Lremovexattr = 187,
    Fremovexattr = 188,
    Tkill = 189,
    Time = 190,
    Futex = 191,
    SchedSetaffinity = 192,
    SchedGetaffinity = 193,
    SetThreadArea = 194,
    IoSetup = 195,
    IoDestroy = 196,
    IoGetevents = 197,
    IoSubmit = 198,
    IoCancel = 199,
    GetThreadArea = 200,
    LookupDcookie = 201,
    EpollCreate = 202,
    EpollCtlOld = 203,
    EpollWaitOld = 204,
    RemapFilePages = 205,
    Getdents64 = 206,
    SetTidAddress = 207,
    RestartSyscall = 208,
    Semtimedop = 209,
    Fadvise64 = 210,
    TimerCreate = 211,
    TimerSettime = 212,
    TimerGettime = 213,
    TimerGetoverrun = 214,
    TimerDelete = 215,
    ClockSettime = 216,
    ClockGettime = 217,
    ClockGetres = 218,
    ClockNanosleep = 219,
    ExitGroup = 220,
    EpollWait = 221,
    EpollCtl = 222,
    Tgkill = 223,
    Utimes = 224,
    Mbind = 225,
    SetMempolicy = 226,
    GetMempolicy = 227,
    MqOpen = 228,
    MqUnlink = 229,
    MqTimedsend = 230,
    MqTimedreceive = 231,
    MqNotify = 232,
    MqGetsetattr = 233,
    KexecLoad = 234,
    Waitid = 235,
    AddKey = 236,
    RequestKey = 237,
    Keyctl = 238,
    IoprioSet = 239,
    IoprioGet = 240,
    InotifyInit = 241,
    InotifyAddWatch = 242,
    InotifyRmWatch = 243,
    MigratePages = 244,
    Openat = 245,
    Mkdirat = 246,
    Mknodat = 247,
    Fchownat = 248,
    Futimesat = 249,
    Newfstatat = 250,
    Unlinkat = 251,
    Renameat = 252,
    Linkat = 253,
    Symlinkat = 254,
    Readlinkat = 255,
    Fchmodat = 256,
    Faccessat = 257,
    Pselect6 = 258,
    Ppoll = 259,
    Unshare = 260,
    SetRobustList = 261,
    GetRobustList = 262,
    Splice = 263,
    Tee = 264,
    SyncFileRange = 265,
    Vmsplice = 266,
    MovePages = 267,
    Utimensat = 268,
    EpollPwait = 269,
    Signalfd = 270,
    TimerfdCreate = 271,
    Eventfd = 272,
    Fallocate = 273,
    TimerfdSettime = 274,
    TimerfdGettime = 275,
    Accept4 = 276,
    Signalfd4 = 277,
    Eventfd2 = 278,
    EpollCreate1 = 279,
    Dup3 = 280,
    Pipe2 = 281,
    InotifyInit1 = 282,
    Preadv = 283,
    Pwritev = 284,
    RtTgsigqueueinfo = 285,
    PerfEventOpen = 286,
    Recvmmsg = 287,
    Setns = 288,
    Getcpu = 289,
    ProcessVmReadv = 290,
    ProcessVmWritev = 291,
    Kcmp = 292,
    FinitModule = 293,
    SchedSetattr = 294,
    SchedGetattr = 295,
    Renameat2 = 296,
    Seccomp = 297,
    Getrandom = 298,
    MemfdCreate = 299,
    KexecFileLoad = 300,
    Bpf = 301,
    Execveat = 302,
    Userfaultfd = 303,
    Membarrier = 304,
    Mlock2 = 305,
    CopyFileRange = 306,
    Preadv2 = 307,
    Pwritev2 = 308,
    PkeyMprotect = 309,
    PkeyAlloc = 310,
    PkeyFree = 311,
    Statx = 312,

    /// ARM64 特定系统调用 (note: Mlock2 was already defined as 305)
    RiscvInsnEmulate = 318,
}

/// 系统调用寄存器上下文
/// 必须与 trap.S 中的栈帧布局匹配
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SyscallFrame {
    pub x0: u64,   // offset 0   - 返回值 / 第1个参数
    pub x1: u64,   // offset 8   - 第2个参数
    pub x2: u64,   // offset 16  - 第3个参数
    pub x3: u64,   // offset 24  - 第4个参数
    pub x4: u64,   // offset 32  - 第5个参数
    pub x5: u64,   // offset 40  - 第6个参数
    pub x6: u64,   // offset 48
    pub x7: u64,   // offset 56
    pub x8: u64,   // offset 64  - 系统调用号 (in x8)
    pub x9: u64,   // offset 72
    pub x10: u64,  // offset 80
    pub x11: u64,  // offset 88
    pub x12: u64,  // offset 96
    pub x13: u64,  // offset 104
    pub x14: u64,  // offset 112
    pub x15: u64,  // offset 120
    pub x16: u64,  // offset 128
    pub x17: u64,  // offset 136
    pub x18: u64,  // offset 144
    pub x19: u64,  // offset 152
    pub x20: u64,  // offset 160
    pub x21: u64,  // offset 168
    pub x22: u64,  // offset 176
    pub x23: u64,  // offset 184
    pub x24: u64,  // offset 192
    pub x25: u64,  // offset 200
    pub x26: u64,  // offset 208
    pub x27: u64,  // offset 216
    pub x28: u64,  // offset 224
    pub x29: u64,  // offset 232
    pub x30: u64,  // offset 240 - 链接寄存器
    pub elr: u64,  // offset 248 - 返回地址
    pub esr: u64,  // offset 256 - 异常 syndrome
    pub spsr: u64, // offset 264 - 程序状态
}

/// 处理系统调用
///
/// ARMv8 系统调用约定:
/// - x8: 系统调用号
/// - x0-x5: 参数 (最多6个)
/// - 返回值: x0
/// - 错误码: x0 设置为负数
#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) {
    // 调试输出：打印系统调用号
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"[SVC:";
        for &b in MSG {
            putchar(b);
        }
        // 打印 x8 的十六进制值
        let hex_chars = b"0123456789ABCDEF";
        let val = frame.x8;
        putchar(hex_chars[((val >> 4) & 0xF) as usize]);
        putchar(hex_chars[(val & 0xF) as usize]);
        const MSG2: &[u8] = b"]\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    let syscall_no = frame.x8;
    let args = [frame.x0, frame.x1, frame.x2, frame.x3, frame.x4, frame.x5];

    // 根据系统调用号分发
    frame.x0 = match syscall_no as u32 {
        0 => sys_read(args),
        1 => sys_write(args),
        2 => sys_openat(args),
        3 => sys_close(args),
        9 => sys_mmap(args),
        11 => sys_munmap(args),
        12 => sys_brk(args),
        15 => sys_rt_sigreturn(args),
        16 => sys_ioctl(args),
        22 => sys_pipe(args),
        32 => sys_dup(args),
        33 => sys_dup2(args),
        39 => sys_getpid(args),
        48 => sys_sigaction(args),
        57 => sys_fork(args),
        58 => sys_vfork(args),
        59 => sys_execve(args),
        60 => sys_exit(args),
        61 => sys_wait4(args),
        62 => sys_kill(args),
        63 => sys_uname(args),
        102 => sys_getuid(args),
        104 => sys_getgid(args),
        107 => sys_geteuid(args),
        108 => sys_getegid(args),
        110 => sys_getppid(args),
        157 => sys_adjtimex(args),
        245 => sys_openat(args),
        _ => {
            debug_println!("Unknown syscall");
            -38_i64 as u64  // ENOSYS - 函数未实现
        }
    };
}

// ============================================================================
// 系统调用实现
// ============================================================================

/// read - 从文件描述符读取
fn sys_read(args: [u64; 6]) -> u64 {
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

/// write - 写入到文件描述符
fn sys_write(args: [u64; 6]) -> u64 {
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

/// pipe - 创建管道
///
/// 对应 Linux 的 pipe 系统调用
/// 参数：x0=指向两个 int 的数组 [readfd, writefd]
fn sys_pipe(args: [u64; 6]) -> u64 {
    let pipe_fds = args[0] as *mut i32;

    if pipe_fds.is_null() {
        debug_println!("sys_pipe: null pointer");
        return -14_i64 as u64;  // EFAULT
    }

    // 创建管道
    let pipe = Pipe::new();
    let pipe_arc = Arc::new(pipe);

    // 管道读端文件操作
    static PIPE_READ_OPS: FileOps = FileOps {
        read: Some(pipe_file_read),
        write: None,
        lseek: None,
        close: Some(pipe_file_close),
    };

    // 管道写端文件操作
    static PIPE_WRITE_OPS: FileOps = FileOps {
        read: None,
        write: Some(pipe_file_write),
        lseek: None,
        close: Some(pipe_file_close),
    };

    // 创建读端文件
    let read_file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    read_file.set_ops(&PIPE_READ_OPS);
    read_file.set_private_data(Arc::as_ptr(&pipe_arc) as *mut u8);

    // 创建写端文件
    let write_file = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    write_file.set_ops(&PIPE_WRITE_OPS);
    write_file.set_private_data(Arc::as_ptr(&pipe_arc) as *mut u8);

    // 分配文件描述符
    unsafe {
        match (get_file_fd_install(read_file), get_file_fd_install(write_file)) {
            (Some(read_fd), Some(write_fd)) => {
                // 写入文件描述符到用户空间
                *pipe_fds.add(0) = read_fd as i32;
                *pipe_fds.add(1) = write_fd as i32;
                0  // 成功
            }
            _ => {
                debug_println!("sys_pipe: failed to allocate file descriptors");
                -24_i64 as u64  // EMFILE
            }
        }
    }
}

/// 管道文件读取操作
unsafe fn pipe_file_read(file: *mut File, buf: *mut u8, count: usize) -> isize {
    use crate::fs::pipe_read;
    if let Some(priv_data) = *(*file).private_data.get() {
        let pipe = &*(priv_data as *const Pipe);
        let slice = core::slice::from_raw_parts_mut(buf, count);
        return pipe_read(pipe, slice);
    }
    -9  // EBADF
}

/// 管道文件写入操作
unsafe fn pipe_file_write(file: *mut File, buf: *const u8, count: usize) -> isize {
    use crate::fs::pipe_write;
    if let Some(priv_data) = *(*file).private_data.get() {
        let pipe = &*(priv_data as *const Pipe);
        let slice = core::slice::from_raw_parts(buf, count);
        return pipe_write(pipe, slice);
    }
    -9  // EBADF
}

/// 管道文件关闭操作
unsafe fn pipe_file_close(_file: *mut File) -> i32 {
    // TODO: 实现引用计数管理
    0
}

/// getpid - 获取进程 ID
fn sys_getpid(_args: [u64; 6]) -> u64 {
    use crate::process;
    process::current_pid() as u64
}

/// getppid - 获取父进程 ID
fn sys_getppid(_args: [u64; 6]) -> u64 {
    use crate::process;
    process::current_ppid() as u64
}

/// getuid - 获取用户 ID
fn sys_getuid(_args: [u64; 6]) -> u64 {
    0  // root 用户
}

/// getgid - 获取组 ID
fn sys_getgid(_args: [u64; 6]) -> u64 {
    0  // root 组
}

/// geteuid - 获取有效用户 ID
fn sys_geteuid(_args: [u64; 6]) -> u64 {
    0
}

/// getegid - 获取有效组 ID
fn sys_getegid(_args: [u64; 6]) -> u64 {
    0
}

/// sigaction - 设置信号处理动作
///
/// 对应 Linux 的 sigaction 系统调用
/// 参数：x0=信号编号, x1=new act, x2=old act, x3=sigsetsize
fn sys_sigaction(args: [u64; 6]) -> u64 {
    let sig = args[0] as i32;
    let act_ptr = args[1] as *const SigAction;
    let _old_act_ptr = args[2] as *mut SigAction;

    println!("sys_sigaction: sig={}", sig);

    // 检查信号编号是否有效
    if sig < 1 || sig > 64 {
        return -22_i64 as u64;  // EINVAL
    }

    // SIGKILL 和 SIGSTOP 不能被捕获或忽略
    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
        return -1_i64 as u64;  // EPERM
    }

    unsafe {
        use crate::process::sched;

        // 获取当前进程
        let current = sched::RQ.current;
        if current.is_null() {
            return -3_i64 as u64;  // ESRCH
        }

        // 如果 act_ptr 不为空，设置新的信号处理动作
        if !act_ptr.is_null() {
            let new_act = &*act_ptr;

            // TODO: 保存旧动作到 old_act_ptr
            // TODO: 处理 sa_mask

            match (*current).signal.as_mut().unwrap().set_action(sig, *new_act) {
                Ok(()) => {
                    println!("sys_sigaction: set action for signal {}", sig);
                    0
                }
                Err(_) => -1_i64 as u64,  // EPERM
            }
        } else {
            0  // 查询模式，暂时返回成功
        }
    }
}

/// exit - 退出当前进程
///
/// 对应 Linux 的 exit 系统调用
fn sys_exit(args: [u64; 6]) -> u64 {
    let exit_code = args[0] as i32;
    println!("sys_exit: exiting with code {}", exit_code);

    // 调用 do_exit 终止当前进程
    // 这个函数永远不会返回
    crate::process::sched::do_exit(exit_code);
}

/// kill - 向进程发送信号
///
/// 对应 Linux 的 kill 系统调用
/// 参数：x0=PID, x1=信号编号
fn sys_kill(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    println!("sys_kill: pid={}, sig={}", pid, sig);

    // 使用调度器的信号发送功能
    match crate::process::sched::send_signal(pid as u32, sig) {
        Ok(()) => 0,
        Err(e) => e as u32 as u64,  // 返回错误码
    }
}

/// waitpid - 等待进程状态改变
///
/// 对应 Linux 的 waitpid 系统调用
fn sys_waitpid(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let status_ptr = args[1] as *mut i32;
    let options = args[2] as i32;

    println!("sys_waitpid: pid={}, options={}", pid, options);

    // TODO: 处理 options (WNOHANG, WUNTRACED, WCONTINUED)
    if options != 0 {
        println!("sys_waitpid: options not fully supported yet");
    }

    // 调用 do_wait 等待子进程
    match crate::process::sched::do_wait(pid, status_ptr) {
        Ok(child_pid) => child_pid as u64,
        Err(e) => e as u32 as u64,
    }
}

/// wait4 - 等待进程状态改变（更通用的版本）
///
/// 对应 Linux 的 wait4 系统调用
fn sys_wait4(args: [u64; 6]) -> u64 {
    let pid = args[0] as i32;
    let _wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    println!("sys_wait4: pid={}, options={}", pid, options);

    // 调用waitpid的实现
    sys_waitpid(args)
}

/// adjtimex - 调整时钟
fn sys_adjtimex(_args: [u64; 6]) -> u64 {
    // TODO: 实现时钟调整
    debug_println!("sys_adjtimex: not implemented");
    -38_i64 as u64  // ENOSYS
}

/// execve - 执行新程序
///
/// 对应 Linux 的 execve 系统调用
/// 参数：x0=程序路径指针, x1=argv指针, x2=envp指针
fn sys_execve(args: [u64; 6]) -> u64 {
    use crate::fs::ElfLoader;
    use core::slice;

    let pathname_ptr = args[0] as *const u8;
    let _argv_ptr = args[1] as *const *const u8;
    let _envp_ptr = args[2] as *const *const u8;

    println!("sys_execve: attempting to execute new program");

    // 安全检查：确保路径指针不为空
    if pathname_ptr.is_null() {
        println!("sys_execve: null pathname");
        return -14_i64 as u64;  // EFAULT
    }

    // TODO: 完整实现需要：
    // 1. 从文件系统读取 ELF 文件
    // 2. 验证 ELF 格式 (ElfLoader::validate)
    // 3. 创建新地址空间
    // 4. 加载程序段 (PT_LOAD segments)
    // 5. 设置用户栈和参数 (argv, envp)
    // 6. 设置返回地址到入口点
    // 7. 切换到用户模式

    // 当前：暂时返回 ENOENT (没有该文件或目录)
    // 因为还没有实现真正的文件系统来读取文件
    println!("sys_execve: filesystem not yet implemented");
    -2_i64 as u64  // ENOENT
}

/// openat - 打开文件（相对于目录文件描述符）
///
/// 对应 Linux 的 openat 系统调用
/// 参数：x0=目录文件描述符(dirfd), x1=文件名指针, x2=标志(flags), x3=模式(mode)
fn sys_openat(args: [u64; 6]) -> u64 {
    let _dirfd = args[0] as i32;  // 目录文件描述符（暂时忽略，使用 AT_FDCWD）
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

/// fork - 创建子进程
///
/// 对应 Linux 的 fork 系统调用
/// 返回：父进程中返回子进程 PID，子进程中返回 0，失败返回 -1
fn sys_fork(_args: [u64; 6]) -> u64 {
    println!("sys_fork: creating new process");

    // 调用调度器的 do_fork 函数
    match crate::process::sched::do_fork() {
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

/// vfork - 创建子进程（共享父进程地址空间）
///
/// 对应 Linux 的 vfork 系统调用
/// 与 fork 的区别：子进程共享父进程的内存空间，父进程被阻塞
/// 返回：父进程中返回子进程 PID，子进程中返回 0，失败返回 -1
fn sys_vfork(_args: [u64; 6]) -> u64 {
    // 当前 vfork 实现与 fork 相同
    // TODO: 实现真正的 vfork 语义（阻塞父进程）
    println!("sys_vfork: creating new process (shared address space)");
    sys_fork(_args)
}

/// close - 关闭文件描述符
///
/// 对应 Linux 的 close 系统调用
/// 参数：x0=文件描述符
/// 返回：0 表示成功，-1 表示失败
fn sys_close(args: [u64; 6]) -> u64 {
    let fd = args[0] as usize;

    println!("sys_close: fd={}", fd);

    unsafe {
        match close_file_fd(fd) {
            Ok(()) => 0,
            Err(e) => e as u32 as u64,
        }
    }
}

/// lseek - 重定位文件的读写位置
///
/// 对应 Linux 的 lseek 系统调用
/// 参数：x0=文件描述符, x1=偏移量, x2=定位方式(SEEK_SET=0, SEEK_CUR=1, SEEK_END=2)
/// 返回：新的文件位置，-1 表示失败
fn sys_lseek(args: [u64; 6]) -> u64 {
    let fd = args[0] as usize;
    let offset = args[1] as i64;
    let whence = args[2] as i32;

    println!("sys_lseek: fd={}, offset={}, whence={}", fd, offset, whence);

    // 暂时返回 ESPIPE (Illegal seek) - 表示文件不支持 seek
    // TODO: 实现真正的 lseek 功能
    -29_i64 as u64  // ESPIPE
}

/// dup - 复制文件描述符
///
/// 对应 Linux 的 dup 系统调用
/// 参数：x0=旧的文件描述符
/// 返回：新的文件描述符，-1 表示失败
fn sys_dup(args: [u64; 6]) -> u64 {
    let oldfd = args[0] as usize;

    println!("sys_dup: oldfd={}", oldfd);

    // TODO: 实现真正的 dup 功能
    // 暂时返回 EMFILE (进程打开的文件过多)
    -24_i64 as u64  // EMFILE
}

/// dup2 - 复制文件描述符到指定位置
///
/// 对应 Linux 的 dup2 系统调用
/// 参数：x0=旧的文件描述符, x1=新的文件描述符
/// 返回：新的文件描述符，-1 表示失败
fn sys_dup2(args: [u64; 6]) -> u64 {
    let oldfd = args[0] as usize;
    let newfd = args[1] as usize;

    println!("sys_dup2: oldfd={}, newfd={}", oldfd, newfd);

    // TODO: 实现真正的 dup2 功能
    // 暂时返回 EMFILE (进程打开的文件过多)
    -24_i64 as u64  // EMFILE
}

/// 获取当前系统调用号 (从 x8 寄存器)
#[inline]
pub fn get_syscall_no() -> u64 {
    let no: u64;
    unsafe {
        asm!("mrs {}, x8", out(reg) no, options(nomem, nostack, pure));
    }
    no
}

/// 设置系统调用返回值
#[inline]
pub unsafe fn set_syscall_ret(val: u64) {
    asm!("mov x0, {}", in(reg) val, options(nomem, nostack));
}

// ============================================================================
// 用户/内核空间隔离
// ============================================================================

/// 用户空间地址范围
///
/// ARMv8 用户空间地址范围（标准配置）
/// 用户空间：0x0000_0000_0000_0000 ~ 0x0000_ffff_ffff_ffff
/// 内核空间：0xffff_0000_0000_0000 ~ 0xffff_ffff_ffff_ffff
const USER_SPACE_END: u64 = 0x0000_ffff_ffff_ffff;

/// 验证用户空间指针
///
/// 检查指针是否在用户空间范围内
///
/// # Safety
///
/// 必须在系统调用上下文中调用
#[inline]
pub unsafe fn verify_user_ptr(ptr: u64) -> bool {
    ptr <= USER_SPACE_END
}

/// 验证用户空间指针数组
///
/// 检查指针数组是否都在用户空间范围内
///
/// # Arguments
///
/// * `ptr` - 起始地址
/// * `size` - 大小（字节）
///
/// # Safety
///
/// 必须在系统调用上下文中调用
pub unsafe fn verify_user_ptr_array(ptr: u64, size: usize) -> bool {
    // 检查溢出
    if ptr > USER_SPACE_END {
        return false;
    }

    // 检查 ptr + size 是否溢出或超出用户空间
    match ptr.checked_add(size as u64) {
        Some(end) if end <= USER_SPACE_END => true,
        _ => false,
    }
}

/// 从用户空间复制字符串
///
/// 安全地从用户空间复制以 null 结尾的字符串
///
/// # Arguments
///
/// * `ptr` - 用户空间字符串指针
/// * `max_len` - 最大长度（防止恶意用户空间）
///
/// # Returns
///
/// * `Ok(Vec<u8>)` - 复制的字符串（UTF-8 字节）
/// * `Err(i32)` - 错误码（负数）
pub unsafe fn copy_user_string(ptr: u64, max_len: usize) -> Result<alloc::vec::Vec<u8>, i32> {
    // 验证指针
    if !verify_user_ptr(ptr) {
        return Err(-14_i32); // EFAULT
    }

    // 计算实际长度
    let mut len = 0;
    let mut user_ptr = ptr as *const u8;

    while len < max_len {
        let byte = *user_ptr;
        if byte == 0 {
            break;
        }
        len += 1;
        user_ptr = user_ptr.add(1);

        // 检查是否超出用户空间
        if (ptr + len as u64) > USER_SPACE_END {
            return Err(-14_i32); // EFAULT
        }
    }

    // 复制字符串
    let mut buf = alloc::vec![0u8; len];
    let src_ptr = ptr as *const u8;
    core::ptr::copy_nonoverlapping(src_ptr, buf.as_mut_ptr(), len);

    Ok(buf)
}

/// 从用户空间复制数据
///
/// 安全地从用户空间复制数据到内核空间
///
/// # Arguments
///
/// * `src` - 用户空间源地址
/// * `dst` - 内核空间目标地址
/// * `size` - 复制大小
///
/// # Returns
///
/// * `Ok(())` - 成功
/// * `Err(i32)` - 错误码
pub unsafe fn copy_from_user(src: u64, dst: *mut u8, size: usize) -> Result<(), i32> {
    // 验证源地址
    if !verify_user_ptr_array(src, size) {
        return Err(-14_i32); // EFAULT
    }

    // 执行复制
    let src_ptr = src as *const u8;
    core::ptr::copy_nonoverlapping(src_ptr, dst, size);

    Ok(())
}

/// 复制数据到用户空间
///
/// 安全地将内核空间数据复制到用户空间
///
/// # Arguments
///
/// * `src` - 内核空间源地址
/// * `dst` - 用户空间目标地址
/// * `size` - 复制大小
///
/// # Returns
///
/// * `Ok(())` - 成功
/// * `Err(i32)` - 错误码
pub unsafe fn copy_to_user(src: *const u8, dst: u64, size: usize) -> Result<(), i32> {
    // 验证目标地址
    if !verify_user_ptr_array(dst, size) {
        return Err(-14_i32); // EFAULT
    }

    // 执行复制
    let dst_ptr = dst as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst_ptr, size);

    Ok(())
}

// ============================================================================
// 新增系统调用实现（Phase 3）
// ============================================================================

/// brk - 改变数据段大小
///
/// 对应 Linux 的 brk 系统调用
/// 参数：x0=新的程序断点地址
/// 返回：新的程序断点，或 0 表示失败
fn sys_brk(args: [u64; 6]) -> u64 {
    let new_brk = args[0];

    // 简化实现：返回 0 表示失败（ENOMEM）
    // TODO: 实现真正的 brk 功能
    // brk 需要维护进程的堆空间
    println!("sys_brk: new_brk={:#x}", new_brk);

    // 暂时返回当前 brk（不做任何改变）
    // 实际实现需要：
    // 1. 维护进程的 brk 值
    // 2. 验证新地址是否合理
    // 3. 更新页表映射

    0  // 表示失败
}

/// mmap - 创建内存映射
///
/// 对应 Linux 的 mmap 系统调用
/// 参数：x0=地址, x1=长度, x2=保护标志, x3=映射标志, x4=文件描述符, x5=偏移量
/// 返回：映射地址，或 -1 表示失败
fn sys_mmap(args: [u64; 6]) -> u64 {
    let addr = args[0];
    let length = args[1];
    let prot = args[2];
    let flags = args[3];
    let fd = args[4] as i32;
    let offset = args[5];

    println!("sys_mmap: addr={:#x}, length={}, prot={:#x}, flags={:#x}, fd={}, offset={}",
             addr, length, prot, flags, fd, offset);

    // 简化实现：返回 ENOMEM（内存不足）
    // TODO: 实现真正的 mmap 功能
    // mmap 需要：
    // 1. 分配虚拟内存区域
    // 2. 设置页表映射
    // 3. 对于文件映射，关联文件

    (-12_i64) as u64  // ENOMEM
}

/// munmap - 取消内存映射
///
/// 对应 Linux 的 munmap 系统调用
/// 参数：x0=地址, x1=长度
/// 返回：0 表示成功，-1 表示失败
fn sys_munmap(args: [u64; 6]) -> u64 {
    let addr = args[0];
    let length = args[1];

    println!("sys_munmap: addr={:#x}, length={}", addr, length);

    // 简化实现：总是返回成功
    // TODO: 实现真正的 munmap 功能
    // munmap 需要：
    // 1. 查找对应的 VMA
    // 2. 取消页表映射
    // 3. 释放资源

    0  // 成功
}

/// ioctl - 设备控制操作
///
/// 对应 Linux 的 ioctl 系统调用
/// 参数：x0=文件描述符, x1=命令, x2=参数
/// 返回：0 表示成功，-1 表示失败
fn sys_ioctl(args: [u64; 6]) -> u64 {
    let fd = args[0] as usize;
    let cmd = args[1];
    let arg = args[2];

    println!("sys_ioctl: fd={}, cmd={:#x}, arg={:#x}", fd, cmd, arg);

    // 简化实现：返回 ENOTTY（不是终端）
    // TODO: 实现真正的 ioctl 功能
    // ioctl 需要：
    // 1. 根据文件描述符获取文件
    // 2. 调用文件的 ioctl 操作
    // 3. 支持常见命令（TCGETS, TCSETS, FIONBIO 等）

    -25_i64 as u64  // ENOTTY
}

/// utsname 结构（Linux 兼容）
#[repr(C)]
struct UtsName {
    sysname: [i8; 65],    // 操作系统名称
    nodename: [i8; 65],   // 网络节点名称
    release: [i8; 65],    // 操作系统版本
    version: [i8; 65],    // 版本详细信息
    machine: [i8; 65],    // 硬件架构
    domainname: [i8; 65], // NIS 域名
}

/// uname - 获取系统信息
///
/// 对应 Linux 的 uname 系统调用
/// 参数：x0=utsname 结构指针
/// 返回：0 表示成功，-1 表示失败
fn sys_uname(args: [u64; 6]) -> u64 {
    let buf_ptr = args[0] as *mut UtsName;

    println!("sys_uname: buf_ptr={:#x}", args[0]);

    // 验证用户空间指针
    unsafe {
        if !verify_user_ptr_array(args[0], core::mem::size_of::<UtsName>()) {
            return (-14_i64) as u64;  // EFAULT
        }

        if buf_ptr.is_null() {
            return (-14_i64) as u64;  // EFAULT
        }

        // 填充 utsname 结构
        let sysname = b"Rux\0";
        let nodename = b"rux\0";
        let release = b"0.1.0\0";
        let version = b"Rux Kernel v0.1.0\0";
        let machine = b"aarch64\0";
        let domainname = b"(none)\0";

        // 复制系统名称
        for (i, &b) in sysname.iter().enumerate() {
            if i < 65 {
                (*buf_ptr).sysname[i] = b as i8;
            }
        }

        // 复制节点名称
        for (i, &b) in nodename.iter().enumerate() {
            if i < 65 {
                (*buf_ptr).nodename[i] = b as i8;
            }
        }

        // 复制版本
        for (i, &b) in release.iter().enumerate() {
            if i < 65 {
                (*buf_ptr).release[i] = b as i8;
            }
        }

        // 复制详细版本
        for (i, &b) in version.iter().enumerate() {
            if i < 65 {
                (*buf_ptr).version[i] = b as i8;
            }
        }

        // 复制硬件架构
        for (i, &b) in machine.iter().enumerate() {
            if i < 65 {
                (*buf_ptr).machine[i] = b as i8;
            }
        }

        // 复制域名
        for (i, &b) in domainname.iter().enumerate() {
            if i < 65 {
                (*buf_ptr).domainname[i] = b as i8;
            }
        }
    }

    0  // 成功
}

/// rt_sigreturn - 从信号处理函数返回
///
/// 对应 Linux 的 rt_sigreturn 系统调用
/// 用于从信号处理函数返回，恢复进程上下文
///
/// # Safety
///
/// 此函数必须从信号处理函数返回时调用
fn sys_rt_sigreturn(_args: [u64; 6]) -> u64 {
    println!("sys_rt_sigreturn: returning from signal handler");

    // TODO: 实现完整的信号返回机制
    // sigreturn 需要：
    // 1. 从栈上恢复信号上下文
    // 2. 恢复寄存器状态
    // 3. 恢复信号掩码
    // 4. 返回到被中断的位置

    // 当前简化实现：直接返回 0
    // 注意：真正的实现需要操作栈和寄存器
    0
}
