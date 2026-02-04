//! ARMv8 系统调用处理
//!
//! 实现基于 SVC (Supervisor Call) 指令的系统调用接口

use core::arch::asm;
use crate::println;
use crate::debug_println;
use crate::fs::{File, FileFlags, FileOps, Pipe, get_file_fd, get_file_fd_install, close_file_fd, CharDev};
use crate::signal::{SigAction, SigFlags, Signal};
use crate::collection::SimpleArc;
use alloc::sync::Arc;  // 保留 Arc 用于 Pipe，后续也需要修复

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
        19 => sys_readv(args),
        20 => sys_writev(args),
        2 => sys_openat(args),
        3 => sys_close(args),
        9 => sys_mmap(args),
        11 => sys_munmap(args),
        10 => sys_mprotect(args),
        12 => sys_brk(args),
        14 => sys_rt_sigprocmask(args),
        15 => sys_rt_sigreturn(args),
        27 => sys_mincore(args),
        28 => sys_madvise(args),
        131 => sys_sigaltstack(args),
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
        96 => sys_gettimeofday(args),
        217 => sys_clock_gettime(args),
        72 => sys_fcntl(args),
        74 => sys_fsync(args),
        75 => sys_fdatasync(args),
        97 => sys_getrlimit(args),
        160 => sys_setrlimit(args),
        157 => sys_adjtimex(args),
        258 => sys_pselect6(args),
        259 => sys_ppoll(args),
        245 => sys_openat(args),
        82 => sys_unlink(args),
        83 => sys_mkdir(args),
        84 => sys_rmdir(args),
        61 => sys_getdents64(args),
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
    let read_file = SimpleArc::new(File::new(FileFlags::new(FileFlags::O_RDONLY))).expect("Failed to create read file");
    read_file.set_ops(&PIPE_READ_OPS);
    read_file.set_private_data(pipe_arc.as_ref() as *const Pipe as *mut u8);

    // 创建写端文件
    let write_file = SimpleArc::new(File::new(FileFlags::new(FileFlags::O_WRONLY))).expect("Failed to create write file");
    write_file.set_ops(&PIPE_WRITE_OPS);
    write_file.set_private_data(pipe_arc.as_ref() as *const Pipe as *mut u8);

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
        let current = match sched::current() {
            Some(c) => c,
            None => return -3_i64 as u64,  // ESRCH
        };

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
/// 对应 Linux 的 rt_sigreturn 系统调用 (syscall 15)
/// 用于从信号处理函数返回，恢复进程上下文
///
/// # Arguments
///
/// 系统调用不需要参数，信号帧地址从用户栈获取
///
/// # Returns
///
/// 这个函数不应该返回到系统调用路径，而是直接恢复到信号中断的位置
///
/// # Safety
///
/// 此函数必须从信号处理函数返回时调用
fn sys_rt_sigreturn(_args: [u64; 6]) -> u64 {
    use crate::process::sched;
    use crate::signal::restore_sigcontext;
    use crate::console::putchar;

    const MSG1: &[u8] = b"sys_rt_sigreturn: entering\n";
    for &b in MSG1 {
        putchar(b);
    }

    unsafe {
        let current = match sched::current() {
            Some(c) => c,
            None => {
                const MSG2: &[u8] = b"sys_rt_sigreturn: no current task\n";
                for &b in MSG2 {
                    putchar(b);
                }
                return -1_i64 as u64;  // 失败
            }
        };

        // 获取用户栈指针（从 CPU 上下文）
        let ctx = (*current).context_mut();
        let frame_addr = ctx.user_sp;

        const MSG3: &[u8] = b"sys_rt_sigreturn: calling restore_sigcontext\n";
        for &b in MSG3 {
            putchar(b);
        }

        // 调用恢复函数
        if restore_sigcontext(current, frame_addr) {
            const MSG4: &[u8] = b"sys_rt_sigreturn: restore successful\n";
            for &b in MSG4 {
                putchar(b);
            }
        } else {
            const MSG5: &[u8] = b"sys_rt_sigreturn: restore failed\n";
            for &b in MSG5 {
                putchar(b);
            }
            return -22_i64 as u64;  // EINVAL
        }

        // 注意：真正的 rt_sigreturn 不应该返回到系统调用路径
        // 而是通过修改 CPU 上下文，直接跳转到被中断的位置
        // 这需要在 trap 处理层面特殊处理

        0  // 当前简化实现：返回 0
    }
}

/// rt_sigprocmask - 信号掩码操作
///
/// 对应 Linux 的 rt_sigprocmask 系统调用
/// 参数：x0=操作方式, x1=new_set, x2=old_set, x3=sigsetsize
/// 返回：0 表示成功，-1 表示失败
fn sys_rt_sigprocmask(args: [u64; 6]) -> u64 {
    let how = args[0] as i32;
    let new_set_ptr = args[1] as *const u64;
    let old_set_ptr = args[2] as *mut u64;
    let sigsetsize = args[3] as usize;

    use crate::signal::sigprocmask_how;
    use crate::process::sched;

    // aarch64 使用 8 字节的 sigset_t
    const SIGSET_SIZE: usize = 8;

    // 验证信号集大小
    if sigsetsize != SIGSET_SIZE {
        println!("sys_rt_sigprocmask: invalid sigsetsize {}", sigsetsize);
        return -22_i64 as u64;  // EINVAL
    }

    unsafe {
        let current = match sched::current() {
            Some(c) => c,
            None => {
                println!("sys_rt_sigprocmask: no current task");
                return -3_i64 as u64;  // ESRCH
            }
        };

        // 获取新的信号集
        let mut new_set: u64 = 0;
        if !new_set_ptr.is_null() {
            new_set = *new_set_ptr;
        }

        // 获取当前信号掩码
        let current_sigmask = (*current).sigmask;

        // 保存旧的信号掩码
        if !old_set_ptr.is_null() {
            *old_set_ptr = current_sigmask;
        }

        // 根据 how 参数执行操作
        let new_sigmask = match how {
            sigprocmask_how::SIG_BLOCK => {
                // SIG_BLOCK: 添加信号到阻塞掩码
                current_sigmask | new_set
            }
            sigprocmask_how::SIG_UNBLOCK => {
                // SIG_UNBLOCK: 从阻塞掩码删除信号
                current_sigmask & !new_set
            }
            sigprocmask_how::SIG_SETMASK => {
                // SIG_SETMASK: 设置新的阻塞掩码
                new_set
            }
            _ => {
                println!("sys_rt_sigprocmask: invalid how {}", how);
                return -22_i64 as u64;  // EINVAL
            }
        };

        // 更新任务的信号掩码
        (*current).sigmask = new_sigmask;

        0  // 成功
    }
}

/// mprotect - 改变内存保护属性
///
/// 对应 Linux 的 mprotect 系统调用 (syscall 10)
/// 参数：x0=地址, x1=长度, x2=保护标志
/// 返回：0 表示成功，-1 表示失败
///
/// 保护标志 (prot):
/// - PROT_READ (1): 可读
/// - PROT_WRITE (2): 可写
/// - PROT_EXEC (4): 可执行
fn sys_mprotect(args: [u64; 6]) -> u64 {
    let addr = args[0];
    let len = args[1];
    let prot = args[2];

    println!("sys_mprotect: addr={:#x}, len={}, prot={:#x}", addr, len, prot);

    // 验证参数
    if len == 0 {
        return 0;  // 空范围，成功
    }

    // 检查地址对齐（必须页对齐）
    const PAGE_SIZE: u64 = 4096;
    if addr & (PAGE_SIZE - 1) != 0 {
        println!("sys_mprotect: addr not page aligned");
        return -22_i64 as u64;  // EINVAL
    }

    // 检查保护标志
    let valid_prot = 0x1 | 0x2 | 0x4;  // PROT_READ | PROT_WRITE | PROT_EXEC
    if prot & !valid_prot != 0 {
        println!("sys_mprotect: invalid prot flags");
        return -22_i64 as u64;  // EINVAL
    }

    // 验证用户空间地址
    unsafe {
        if !verify_user_ptr_array(addr, len as usize) {
            println!("sys_mprotect: invalid user address");
            return -14_i64 as u64;  // EFAULT
        }
    }

    // 简化实现：返回 ENOSYS（功能未实现）
    // TODO: 实现真正的 mprotect 功能
    // mprotect 需要：
    // 1. 查找覆盖 [addr, addr+len) 的所有 VMA
    // 2. 拆分 VMA（如果需要）
    // 3. 更新页表项的保护位
    // 4. 更新 VMA 的保护标志

    -38_i64 as u64  // ENOSYS
}

/// mincore - 查询页面驻留状态
///
/// 对应 Linux 的 mincore 系统调用 (syscall 27)
/// 参数：x0=地址, x1=长度, x2=状态向量指针
/// 返回：0 表示成功，-1 表示失败
///
/// 状态向量：每个字节对应一个页，最低位表示页面是否在内存中
fn sys_mincore(args: [u64; 6]) -> u64 {
    let addr = args[0];
    let len = args[1];
    let vec_ptr = args[2] as *mut u8;

    println!("sys_mincore: addr={:#x}, len={}, vec_ptr={:#x}",
             addr, len, args[2]);

    // 验证参数
    if len == 0 {
        return 0;  // 空范围，成功
    }

    // 检查地址对齐（必须页对齐）
    const PAGE_SIZE: u64 = 4096;
    if addr & (PAGE_SIZE - 1) != 0 {
        println!("sys_mincore: addr not page aligned");
        return -22_i64 as u64;  // EINVAL
    }

    // 验证用户空间指针
    unsafe {
        if !verify_user_ptr_array(addr, len as usize) {
            println!("sys_mincore: invalid user address");
            return -14_i64 as u64;  // EFAULT
        }

        if vec_ptr.is_null() {
            println!("sys_mincore: null vec pointer");
            return -14_i64 as u64;  // EFAULT
        }

        // 计算需要的字节数（每页一个字节）
        let page_count = ((len + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
        if !verify_user_ptr_array(args[2], page_count) {
            println!("sys_mincore: vec buffer too small");
            return -22_i64 as u64;  // EINVAL
        }
    }

    // 简化实现：所有页面标记为驻留
    // TODO: 实现真正的 mincore 功能
    // mincore 需要：
    // 1. 遍历 [addr, addr+len) 范围内的所有页
    // 2. 检查页表项，判断页面是否在内存中
    // 3. 设置状态向量中的对应位

    // 当前简化实现：将所有页面标记为驻留（设置最低位）
    unsafe {
        let page_count = ((len + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
        for i in 0..page_count {
            *vec_ptr.add(i) = 1;  // 标记为驻留
        }
    }

    0  // 成功
}

/// madvise - 给内核提供内存使用建议
///
/// 对应 Linux 的 madvise 系统调用 (syscall 28)
/// 参数：x0=地址, x1=长度, x2=建议
/// 返回：0 表示成功，-1 表示失败
///
/// 常见建议：
/// - MADV_NORMAL (0): 无特殊建议
/// - MADV_RANDOM (1): 随机访问
/// - MADV_SEQUENTIAL (2): 顺序访问
/// - MADV_WILLNEED (3): 将会需要（预读）
/// - MADV_DONTNEED (4): 不需要（释放）
/// - MADV_REMOVE (9): 释放页（如 shmem）
/// - MADV_DONTFORK (10): fork时不复制
/// - MADV_DOFORK (11): fork时复制
/// - MADV_MERGEABLE (12): 可合并（KSM）
/// - MADV_UNMERGEABLE (13): 不可合并
/// - MADV_HUGEPAGE (14): 使用大页
/// - MADV_NOHUGEPAGE (15): 不使用大页
/// - MADV_DONTDUMP (16): core dump时不包含
/// - MADV_DODUMP (17): core dump时包含
fn sys_madvise(args: [u64; 6]) -> u64 {
    let addr = args[0];
    let len = args[1];
    let advice = args[2] as i32;

    println!("sys_madvise: addr={:#x}, len={}, advice={}", addr, len, advice);

    // 验证参数
    if len == 0 {
        return 0;  // 空范围，成功
    }

    // 检查地址对齐（必须页对齐）
    const PAGE_SIZE: u64 = 4096;
    if addr & (PAGE_SIZE - 1) != 0 {
        println!("sys_madvise: addr not page aligned");
        return -22_i64 as u64;  // EINVAL
    }

    // 验证用户空间地址
    unsafe {
        if !verify_user_ptr_array(addr, len as usize) {
            println!("sys_madvise: invalid user address");
            return -14_i64 as u64;  // EFAULT
        }
    }

    // 简化实现：忽略所有建议，总是返回成功
    // TODO: 实现真正的 madvise 功能
    // madvise 需要：
    // 1. 查找覆盖 [addr, addr+len) 的所有 VMA
    // 2. 根据 advice 类型采取不同操作：
    //    - MADV_DONTNEED: 释放页面，清空 PTE
    //    - MADV_WILLNEED: 触发页面预读
    //    - MADV_SEQUENTIAL: 设置访问位，预读优化
    //    - MADV_RANDOM: 禁用预读
    //    - MADV_REMOVE: 分离页面（如 tmpfs/shmem）
    //    - MADV_HUGEPAGE/NOHUGEPAGE: 透明大页控制
    // 3. 更新 VMA 的标志

    // 当前简化实现：总是返回 0（忽略建议）
    0  // 成功
}

/// sigaltstack - 设置或获取信号栈
///
/// 对应 Linux 的 sigaltstack 系统调用 (syscall 131)
/// 参数：x0=new_ss, x1=old_ss
/// 返回：0 表示成功，-1 表示失败
///
/// 信号栈用于信号处理函数，当正常栈可能损坏时使用
fn sys_sigaltstack(args: [u64; 6]) -> u64 {
    use crate::process::sched;
    use crate::signal::SignalStack;
    use crate::signal::ss_flags;

    let new_ss_ptr = args[0] as *const SignalStack;
    let old_ss_ptr = args[1] as *mut SignalStack;

    println!("sys_sigaltstack: new_ss_ptr={:#x}, old_ss_ptr={:#x}",
             args[0], args[1]);

    unsafe {
        let current = match sched::current() {
            Some(c) => c,
            None => {
                println!("sys_sigaltstack: no current task");
                return -3_i64 as u64;  // ESRCH
            }
        };

        // 如果 old_ss_ptr 不为空，返回当前信号栈
        if !old_ss_ptr.is_null() {
            let current_sigstack = &(*current).sigstack;
            (*old_ss_ptr).ss_sp = current_sigstack.ss_sp;
            (*old_ss_ptr).ss_size = current_sigstack.ss_size;
            (*old_ss_ptr).ss_flags = current_sigstack.ss_flags;

            println!("sys_sigaltstack: returning current sigstack");
        }

        // 如果 new_ss_ptr 不为空，设置新的信号栈
        if !new_ss_ptr.is_null() {
            let new_ss = &*new_ss_ptr;

            // 验证栈大小
            const MINSIGSTKSZ: usize = 2048;
            if new_ss.ss_size < MINSIGSTKSZ as u64 {
                println!("sys_sigaltstack: stack too small");
                return -22_i64 as u64;  // EINVAL
            }

            // 检查标志
            if (new_ss.ss_flags & !(ss_flags::SS_DISABLE | ss_flags::SS_ONSTACK | ss_flags::SS_AUTODISABLE)) != 0 {
                println!("sys_sigaltstack: invalid flags");
                return -22_i64 as u64;  // EINVAL
            }

            // 检查是否已经在信号栈上
            if (*current).sigstack.is_on_stack() {
                println!("sys_sigaltstack: already on signal stack");
                return -16_i64 as u64;  // EPERM
            }

            // 设置新的信号栈
            (*current).sigstack.ss_sp = new_ss.ss_sp;
            (*current).sigstack.ss_size = new_ss.ss_size;
            (*current).sigstack.ss_flags = new_ss.ss_flags;

            println!("sys_sigaltstack: set new sigstack sp={:#x}, size={}",
                     new_ss.ss_sp, new_ss.ss_size);
        }

        0  // 成功
    }
}

// ============================================================================
// 时间相关结构体
// ============================================================================

/// timeval 结构体 - 用于 gettimeofday
///
/// 对应 Linux 的 struct timeval (include/uapi/linux/time.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeVal {
    pub tv_sec: i64,   // 秒
    pub tv_usec: i64,  // 微秒
}

/// timezone 结构体 - 用于 gettimeofday
///
/// 对应 Linux 的 struct timezone (include/uapi/linux/time.h)
/// 注意：Linux 已废弃此参数，tz 参数通常应设为 NULL
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeZone {
    pub tz_minuteswest: i32,  // UTC 以西的分钟数
    pub tz_dsttime: i32,      // DST 修正类型
}

/// timespec 结构体 - 用于 clock_gettime
///
/// 对应 Linux 的 struct timespec (include/uapi/linux/time.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct TimeSpec {
    pub tv_sec: i64,   // 秒
    pub tv_nsec: i64,  // 纳秒
}

/// clockid_t - 时钟 ID 类型
///
/// 对应 Linux 的 clockid_t (include/uapi/linux/time.h)
pub type ClockId = i32;

/// 时钟 ID 常量
pub mod clockid {
    use super::ClockId;

    pub const CLOCK_REALTIME: ClockId = 0;
    pub const CLOCK_MONOTONIC: ClockId = 1;
    pub const CLOCK_PROCESS_CPUTIME_ID: ClockId = 2;
    pub const CLOCK_THREAD_CPUTIME_ID: ClockId = 3;
    pub const CLOCK_MONOTONIC_RAW: ClockId = 4;
    pub const CLOCK_REALTIME_COARSE: ClockId = 5;
    pub const CLOCK_MONOTONIC_COARSE: ClockId = 6;
    pub const CLOCK_BOOTTIME: ClockId = 7;
    pub const CLOCK_REALTIME_ALARM: ClockId = 8;
    pub const CLOCK_BOOTTIME_ALARM: ClockId = 9;
}

/// gettimeofday - 获取系统时间
///
/// 对应 Linux 的 gettimeofday 系统调用 (syscall 96)
/// 参数：x0=tv (timeval指针), x1=tz (timezone指针，已废弃)
/// 返回：0 表示成功，负数表示错误码
///
/// 注意：timezone 参数在 Linux 中已废弃，应设为 NULL
fn sys_gettimeofday(args: [u64; 6]) -> u64 {
    let tv_ptr = args[0] as *mut TimeVal;
    let _tz_ptr = args[1] as *mut TimeZone;

    // 简化实现：返回一个固定的模拟时间
    // TODO: 实际应从硬件定时器或系统时钟读取
    // 当前返回：从内核启动后的模拟时间（秒数 + 微秒数）

    // 这里使用一个简单的计数器模拟时间
    // 在真实系统中，应该从 ARMv8 架构定时器 (CNTVCT) 读取
    use core::arch::asm;

    let mut cnt: u64 = 0;
    unsafe {
        // 读取 ARMv8 虚拟计数器值
        asm!("mrs {}, cntvct_el0", out(reg) cnt);
    }

    // 假设计数器频率为 1MHz（每微秒递增 1）
    // 实际频率需要从 CNTFRQ_EL0 读取
    let tv_sec = (cnt / 1_000_000) as i64;
    let tv_usec = (cnt % 1_000_000) as i64;

    // 将时间写入用户空间
    if !tv_ptr.is_null() {
        unsafe {
            // TODO: 应该使用 copy_to_user 验证用户指针
            (*tv_ptr).tv_sec = tv_sec;
            (*tv_ptr).tv_usec = tv_usec;
        }
    }

    // timezone 参数已废弃，Linux 内核忽略此参数
    // 如果 tz_ptr 不为 NULL，可以将其清零

    0  // 成功
}

/// clock_gettime - 获取指定时钟的时间
///
/// 对应 Linux 的 clock_gettime 系统调用 (syscall 217)
/// 参数：x0=clockid (时钟ID), x1=tp (timespec指针)
/// 返回：0 表示成功，负数表示错误码
///
/// 支持的时钟：
/// - CLOCK_REALTIME: 系统范围内的实时时钟
/// - CLOCK_MONOTONIC: 单调递增的时钟（不受系统时间调整影响）
/// - CLOCK_BOOTTIME: 从启动开始的单调时钟（包含睡眠时间）
fn sys_clock_gettime(args: [u64; 6]) -> u64 {
    let clockid = args[0] as ClockId;
    let tp_ptr = args[1] as *mut TimeSpec;

    // 简化实现：所有时钟返回相同的模拟时间
    // TODO: 实际应根据不同的 clockid 返回不同的时间源
    match clockid {
        clockid::CLOCK_REALTIME | clockid::CLOCK_MONOTONIC | clockid::CLOCK_BOOTTIME => {
            // 读取 ARMv8 虚拟计数器值
            use core::arch::asm;
            let mut cnt: u64 = 0;
            unsafe {
                asm!("mrs {}, cntvct_el0", out(reg) cnt);
            }

            // 假设计数器频率为 1MHz
            let tv_sec = (cnt / 1_000_000) as i64;
            let tv_nsec = ((cnt % 1_000_000) * 1000) as i64;  // 微秒转纳秒

            // 将时间写入用户空间
            if !tp_ptr.is_null() {
                unsafe {
                    // TODO: 应该使用 copy_to_user 验证用户指针
                    (*tp_ptr).tv_sec = tv_sec;
                    (*tp_ptr).tv_nsec = tv_nsec;
                }
            }

            0  // 成功
        }
        _ => {
            // 不支持的时钟类型
            -22_i64 as u64  // EINVAL
        }
    }
}

// ============================================================================
// 向量 I/O 相关结构体
// ============================================================================

/// iovec 结构体 - 用于 readv/writev
///
/// 对应 Linux 的 struct iovec (include/uapi/linux/uio.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct IOVec {
    pub iov_base: u64,  // 缓冲区地址
    pub iov_len: u64,   // 缓冲区长度
}

/// readv - 从文件描述符读取数据到多个缓冲区
///
/// 对应 Linux 的 readv 系统调用 (syscall 19)
/// 参数：x0=fd, x1=iov (iovec数组指针), x2=iovcnt (iovec数量)
/// 返回：读取的总字节数，负数表示错误码
///
/// readv 是 read 的向量版本，允许单次调用读取到多个缓冲区
fn sys_readv(args: [u64; 6]) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const IOVec;
    let iovcnt = args[2] as usize;

    // 限制 iovec 数量，防止过度分配
    const UIO_MAXIOV: usize = 1024;
    if iovcnt == 0 || iovcnt > UIO_MAXIOV {
        return -22_i64 as u64;  // EINVAL
    }

    unsafe {
        match get_file_fd(fd) {
            Some(file) => {
                let mut total_read: usize = 0;

                // 遍历所有 iovec，逐个读取
                for i in 0..iovcnt {
                    let iov = &*iov_ptr.add(i);
                    let buf = iov.iov_base as *mut u8;
                    let len = iov.iov_len as usize;

                    if len == 0 {
                        continue;
                    }

                    // 调用底层 read 函数
                    let result = file.read(buf, len);
                    if result < 0 {
                        return result as u32 as u64;  // 返回错误码
                    }

                    let bytes_read = result as usize;
                    total_read += bytes_read;

                    // 如果读取字节数小于请求，说明到达 EOF 或出错
                    if bytes_read < len {
                        break;
                    }
                }

                total_read as u64
            }
            None => {
                debug_println!("sys_readv: invalid fd");
                -9_i64 as u64  // EBADF
            }
        }
    }
}

/// writev - 将多个缓冲区的数据写入文件描述符
///
/// 对应 Linux 的 writev 系统调用 (syscall 20)
/// 参数：x0=fd, x1=iov (iovec数组指针), x2=iovcnt (iovec数量)
/// 返回：写入的总字节数，负数表示错误码
///
/// writev 是 write 的向量版本，允许单次调用写入多个缓冲区
fn sys_writev(args: [u64; 6]) -> u64 {
    let fd = args[0] as usize;
    let iov_ptr = args[1] as *const IOVec;
    let iovcnt = args[2] as usize;

    // 限制 iovec 数量，防止过度分配
    const UIO_MAXIOV: usize = 1024;
    if iovcnt == 0 || iovcnt > UIO_MAXIOV {
        return -22_i64 as u64;  // EINVAL
    }

    unsafe {
        // 特殊处理 stdout (1) 和 stderr (2) - 直接写入 UART
        if fd == 1 || fd == 2 {
            use crate::console::putchar;
            let mut total_written: usize = 0;

            for i in 0..iovcnt {
                let iov = &*iov_ptr.add(i);
                let buf = iov.iov_base as *const u8;
                let len = iov.iov_len as usize;

                let slice = core::slice::from_raw_parts(buf, len);
                for &b in slice {
                    putchar(b);
                }

                total_written += len;
            }

            return total_written as u64;
        }

        match get_file_fd(fd) {
            Some(file) => {
                let mut total_written: usize = 0;

                // 遍历所有 iovec，逐个写入
                for i in 0..iovcnt {
                    let iov = &*iov_ptr.add(i);
                    let buf = iov.iov_base as *const u8;
                    let len = iov.iov_len as usize;

                    if len == 0 {
                        continue;
                    }

                    // 调用底层 write 函数
                    let result = file.write(buf, len);
                    if result < 0 {
                        return result as u32 as u64;  // 返回错误码
                    }

                    let bytes_written = result as usize;
                    total_written += bytes_written;

                    // 如果写入字节数小于请求，可能是磁盘已满等
                    if bytes_written < len {
                        break;
                    }
                }

                total_written as u64
            }
            None => {
                debug_println!("sys_writev: invalid fd");
                -9_i64 as u64  // EBADF
            }
        }
    }
}

// ============================================================================
// I/O 多路复用相关结构体
// ============================================================================

/// fd_set 结构体 - 用于 select
///
/// 对应 Linux 的 fd_set (include/uapi/linux/posix_types.h)
/// 使用位图表示文件描述符集合
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct FdSet {
    pub fds_bits: [u64; 16],  // 支持 1024 个文件描述符 (16 * 64)
}

impl FdSet {
    /// 创建空的 fd_set
    pub fn new() -> Self {
        Self { fds_bits: [0; 16] }
    }

    /// 设置文件描述符
    pub fn set(&mut self, fd: usize) {
        if fd < 1024 {
            let idx = fd / 64;
            let bit = fd % 64;
            self.fds_bits[idx] |= 1u64 << bit;
        }
    }

    /// 清除文件描述符
    pub fn clear(&mut self, fd: usize) {
        if fd < 1024 {
            let idx = fd / 64;
            let bit = fd % 64;
            self.fds_bits[idx] &= !(1u64 << bit);
        }
    }

    /// 检查文件描述符是否被设置
    pub fn is_set(&self, fd: usize) -> bool {
        if fd < 1024 {
            let idx = fd / 64;
            let bit = fd % 64;
            (self.fds_bits[idx] & (1u64 << bit)) != 0
        } else {
            false
        }
    }

    /// 清空所有位
    pub fn zero(&mut self) {
        self.fds_bits = [0; 16];
    }

    /// 获取设置的最高文件描述符号
    pub fn max_fd(&self) -> i32 {
        for i in (0..16).rev() {
            if self.fds_bits[i] != 0 {
                let base = (i * 64) as i32;
                let bits = self.fds_bits[i];
                // 找到最高设置位
                let leading_zeros = bits.leading_zeros() as i32;
                return base + (63 - leading_zeros);
            }
        }
        -1
    }
}

/// pollfd 结构体 - 用于 poll
///
/// 对应 Linux 的 struct pollfd (include/uapi/linux/poll.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct PollFd {
    pub fd: i32,           // 文件描述符
    pub events: i16,       // 监控的事件
    pub revents: i16,      // 返回的事件
}

/// poll 事件类型
pub mod poll_events {
    /// 可读
    pub const POLLIN: i16 = 0x0001;
    /// 普通数据可读
    pub const POLLRDNORM: i16 = 0x0040;
    /// 优先级带数据可读
    pub const POLLRDBAND: i16 = 0x0080;
    /// 高优先级数据可读
    pub const POLLPRI: i16 = 0x0002;

    /// 可写
    pub const POLLOUT: i16 = 0x0004;
    /// 普通数据可写
    pub const POLLWRNORM: i16 = 0x0100;
    /// 优先级带数据可写
    pub const POLLWRBAND: i16 = 0x0200;

    /// 错误
    pub const POLLERR: i16 = 0x0008;
    /// 挂起
    pub const POLLHUP: i16 = 0x0010;
    /// 无效请求
    pub const POLLNVAL: i16 = 0x0020;
}

/// sigmask_t - 信号掩码类型
pub type SigMask = u64;

/// pselect6 - 同步 I/O 多路复用（带信号掩码）
///
/// 对应 Linux 的 pselect6 系统调用 (syscall 258)
/// 参数：x0=nfds, x1=readfds, x2=writefds, x3=exceptfds, x4=timeout, x5=sigmask
/// 返回：就绪的文件描述符数量，负数表示错误码
///
/// pselect6 是 select 的增强版本，允许设置信号掩码
fn sys_pselect6(args: [u64; 6]) -> u64 {
    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
    let _timeout_ptr = args[4] as *const TimeSpec;
    let _sigmask_ptr = args[5] as *const SigMask;

    // 简化实现：总是返回 0（超时）
    // TODO: 实现真正的 pselect6 功能
    // pselect6 需要：
    // 1. 遍历所有文件描述符（0 到 nfds-1）
    // 2. 检查每个 fd 是否可读/可写/异常
    // 3. 更新对应的 fd_set
    // 4. 返回就绪的 fd 数量
    // 5. 支持 timeout（如果非 NULL）
    // 6. 支持信号掩码（如果非 NULL）

    // 当前框架实现：验证 fd 并清空结果集
    unsafe {
        if !readfds_ptr.is_null() {
            (*readfds_ptr).zero();
        }
        if !writefds_ptr.is_null() {
            (*writefds_ptr).zero();
        }
        if !exceptfds_ptr.is_null() {
            (*exceptfds_ptr).zero();
        }
    }

    // 简化实现：返回 0 表示超时
    0  // 超时，没有 fd 就绪
}

/// ppoll - I/O 多路复用（带信号掩码）
///
/// 对应 Linux 的 ppoll 系统调用 (syscall 259)
/// 参数：x0=fds, x1=nfds, x2=timeout, x3=sigmask, x4=sigsetsize
/// 返回：就绪的文件描述符数量，负数表示错误码
///
/// ppoll 是 poll 的增强版本，允许设置信号掩码
fn sys_ppoll(args: [u64; 6]) -> u64 {
    use poll_events::*;

    let fds_ptr = args[0] as *mut PollFd;
    let nfds = args[1] as usize;
    let _timeout_ptr = args[2] as *const TimeSpec;
    let _sigmask_ptr = args[3] as *const SigMask;
    let _sigsetsize = args[4] as usize;

    // 限制 nfds 防止过度分配
    const RLIMIT_NOFILE: usize = 1024;
    if nfds > RLIMIT_NOFILE {
        return -22_i64 as u64;  // EINVAL
    }

    // 简化实现：总是返回 0（超时）
    // TODO: 实现真正的 ppoll 功能
    // ppoll 需要：
    // 1. 遍历所有 pollfd 结构体
    // 2. 检查每个 fd 是否满足请求的事件
    // 3. 设置 revents 字段
    // 4. 返回就绪的 fd 数量
    // 5. 支持 timeout（如果非 NULL）
    // 6. 支持信号掩码（如果非 NULL）

    // 当前框架实现：清空所有 revents
    unsafe {
        for i in 0..nfds {
            let pollfd = &mut *fds_ptr.add(i);
            pollfd.revents = 0;

            // 检查 fd 有效性
            if pollfd.fd < 0 {
                pollfd.revents = POLLNVAL;
            }
        }
    }

    // 简化实现：返回 0 表示超时
    0  // 超时，没有 fd 就绪
}

// ============================================================================
// 文件控制相关常量
// ============================================================================

/// fcntl 命令常量
pub mod fcntl_cmd {
    /// 复制文件描述符
    pub const F_DUPFD: i32 = 0;
    /// 复制文件描述符并设置 close-on-exec
    pub const F_DUPFD_CLOEXEC: i32 = 1024;
    /// 获取文件描述符标志
    pub const F_GETFD: i32 = 1;
    /// 设置文件描述符标志
    pub const F_SETFD: i32 = 2;
    /// 获取文件状态标志
    pub const F_GETFL: i32 = 3;
    /// 设置文件状态标志
    pub const F_SETFL: i32 = 4;
    /// 获取文件锁
    pub const F_GETLK: i32 = 5;
    /// 设置文件锁
    pub const F_SETLK: i32 = 6;
    /// 设置文件锁（等待）
    pub const F_SETLKW: i32 = 7;
    /// 获取文件读写位置
    pub const F_GETOWN: i32 = 9;
    /// 设置文件读写位置
    pub const F_SETOWN: i32 = 8;
}

/// fcntl - 文件控制操作
///
/// 对应 Linux 的 fcntl 系统调用 (syscall 72)
/// 参数：x0=fd, x1=cmd, x2=arg
/// 返回：根据命令不同返回不同值，负数表示错误码
fn sys_fcntl(args: [u64; 6]) -> u64 {
    use fcntl_cmd::*;

    let fd = args[0] as i32;
    let cmd = args[1] as i32;
    let arg = args[2] as i64;

    match cmd {
        F_DUPFD => {
            let min_fd = arg as i32;
            println!("sys_fcntl: F_DUPFD fd={}, min_fd={}", fd, min_fd);
            unsafe {
                match get_file_fd(fd as usize) {
                    Some(_) => min_fd as u64,
                    None => -9_i64 as u64,  // EBADF
                }
            }
        }
        F_DUPFD_CLOEXEC => {
            let min_fd = arg as i32;
            println!("sys_fcntl: F_DUPFD_CLOEXEC fd={}, min_fd={}", fd, min_fd);
            unsafe {
                match get_file_fd(fd as usize) {
                    Some(_) => min_fd as u64,
                    None => -9_i64 as u64,  // EBADF
                }
            }
        }
        F_GETFD => {
            println!("sys_fcntl: F_GETFD fd={}", fd);
            0
        }
        F_SETFD => {
            println!("sys_fcntl: F_SETFD fd={}, flags={}", fd, arg);
            0
        }
        F_GETFL => {
            println!("sys_fcntl: F_GETFL fd={}", fd);
            2  // O_RDWR
        }
        F_SETFL => {
            println!("sys_fcntl: F_SETFL fd={}, flags={}", fd, arg);
            0
        }
        F_GETLK | F_SETLK | F_SETLKW => {
            println!("sys_fcntl: file locking cmd={}", cmd);
            -38_i64 as u64  // ENOSYS
        }
        _ => {
            println!("sys_fcntl: unknown cmd {}", cmd);
            -22_i64 as u64  // EINVAL
        }
    }
}

/// fsync - 同步文件到磁盘
///
/// 对应 Linux 的 fsync 系统调用 (syscall 74)
/// 参数：x0=fd
/// 返回：0 表示成功，负数表示错误码
fn sys_fsync(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    println!("sys_fsync: fd={}", fd);
    0  // 成功
}

/// fdatasync - 同步文件数据到磁盘（不同步元数据）
///
/// 对应 Linux 的 fdatasync 系统调用 (syscall 75)
/// 参数：x0=fd
/// 返回：0 表示成功，负数表示错误码
fn sys_fdatasync(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    println!("sys_fdatasync: fd={}", fd);
    0  // 成功
}

// ============================================================================
// 资源限制相关结构体
// ============================================================================

/// rlimit 结构体 - 资源限制
///
/// 对应 Linux 的 struct rlimit (include/uapi/linux/resource.h)
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct RLimit {
    pub rlim_cur: u64,  // 软限制（当前限制）
    pub rlim_max: u64,  // 硬限制（最大限制）
}

/// 资源类型
pub mod rlimit_resource {
    pub const RLIMIT_CPU: i32 = 0;        // CPU 时间（秒）
    pub const RLIMIT_FSIZE: i32 = 1;      // 文件大小（字节）
    pub const RLIMIT_DATA: i32 = 2;       // 数据段大小（字节）
    pub const RLIMIT_STACK: i32 = 3;      // 栈大小（字节）
    pub const RLIMIT_CORE: i32 = 4;       // core 文件大小（字节）
    pub const RLIMIT_RSS: i32 = 5;        // 驻留集大小（字节）
    pub const RLIMIT_NPROC: i32 = 6;      // 进程数
    pub const RLIMIT_NOFILE: i32 = 7;     // 文件描述符数
    pub const RLIMIT_MEMLOCK: i32 = 8;    // 锁定内存（字节）
    pub const RLIMIT_AS: i32 = 9;         // 地址空间（字节）
    pub const RLIMIT_LOCKS: i32 = 10;     // 文件锁数
    pub const RLIMIT_SIGPENDING: i32 = 11; // 挂起信号数
    pub const RLIMIT_MSGQUEUE: i32 = 12;  // 消息队列字节数
    pub const RLIMIT_NICE: i32 = 13;      // 优先级
    pub const RLIMIT_RTPRIO: i32 = 14;    // 实时优先级
    pub const RLIMIT_RTTIME: i32 = 15;    // 实时 CPU 时间（微秒）
}

/// getrlimit - 获取资源限制
///
/// 对应 Linux 的 getrlimit 系统调用 (syscall 97)
/// 参数：x0=resource, x1=rlim
/// 返回：0 表示成功，负数表示错误码
fn sys_getrlimit(args: [u64; 6]) -> u64 {
    use rlimit_resource::*;

    let resource = args[0] as i32;
    let rlim_ptr = args[1] as *mut RLimit;

    if rlim_ptr.is_null() {
        return -14_i64 as u64;  // EFAULT
    }

    unsafe {
        let mut rlim = RLimit {
            rlim_cur: 0,
            rlim_max: 0,
        };

        match resource {
            RLIMIT_NOFILE => {
                rlim.rlim_cur = 1024;
                rlim.rlim_max = 4096;
            }
            RLIMIT_STACK => {
                rlim.rlim_cur = 8 * 1024 * 1024;
                rlim.rlim_max = 8 * 1024 * 1024;
            }
            RLIMIT_CPU | RLIMIT_AS => {
                rlim.rlim_cur = u64::MAX;
                rlim.rlim_max = u64::MAX;
            }
            _ => {
                rlim.rlim_cur = u64::MAX;
                rlim.rlim_max = u64::MAX;
            }
        }

        *rlim_ptr = rlim;
    }

    0  // 成功
}

/// setrlimit - 设置资源限制
///
/// 对应 Linux 的 setrlimit 系统调用 (syscall 160)
/// 参数：x0=resource, x1=rlim
/// 返回：0 表示成功，负数表示错误码
fn sys_setrlimit(args: [u64; 6]) -> u64 {
    let resource = args[0] as i32;
    let rlim_ptr = args[1] as *const RLimit;

    if rlim_ptr.is_null() {
        return -14_i64 as u64;  // EFAULT
    }

    println!("sys_setrlimit: resource={}, rlim_cur={}, rlim_max={}",
             resource, unsafe { (*rlim_ptr).rlim_cur }, unsafe { (*rlim_ptr).rlim_max });

    0  // 成功
}

// ============================================================================
// 目录操作相关系统调用
// ============================================================================

/// unlink - 删除文件链接
///
/// 对应 Linux 的 unlink 系统调用 (syscall 82 on aarch64)
/// 参数：x0=pathname (文件路径)
/// 返回：0 表示成功，负数表示错误码
fn sys_unlink(args: [u64; 6]) -> u64 {
    let pathname_ptr = args[0] as *const i8;

    // 简化实现：总是返回 ENOSYS
    // TODO: 实现真正的 unlink 功能
    // unlink 需要：
    // 1. 路径解析
    // 2. 检查文件是否存在
    // 3. 删除目录项
    // 4. 减少 inode 引用计数
    // 5. 如果引用计数为0，释放 inode

    println!("sys_unlink: pathname_ptr={:#x}", args[0]);

    -38_i64 as u64  // ENOSYS
}

/// mkdir - 创建目录
///
/// 对应 Linux 的 mkdir 系统调用 (syscall 83 on aarch64)
/// 参数：x0=pathname (目录路径), x1=mode (权限)
/// 返回：0 表示成功，负数表示错误码
fn sys_mkdir(_args: [u64; 6]) -> u64 {
    -38_i64 as u64  // ENOSYS - TEMPORARILY DISABLED
}

/// rmdir - 删除目录
///
/// 对应 Linux 的 rmdir 系统调用 (syscall 84 on aarch64)
/// 参数：x0=pathname (目录路径)
/// 返回：0 表示成功，负数表示错误码
fn sys_rmdir(args: [u64; 6]) -> u64 {
    let pathname_ptr = args[0] as u64;

    // TEMPORARILY DISABLED - VFS being debugged
    -38_i64 as u64  // ENOSYS
}

/// getdents64 - 读取目录项
///
/// 对应 Linux 的 getdents64 系统调用 (syscall 61 on aarch64)
/// 参数：x0=fd, x1=dirent, x2=count
/// 返回：读取的字节数，负数表示错误码
/// TEMPORARILY DISABLED - VFS being debugged
fn sys_getdents64(_args: [u64; 6]) -> u64 {
    -38_i64 as u64  // ENOSYS
}
