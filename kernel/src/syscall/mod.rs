//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System call module
//!
//! This module implements the RISC-V 64-bit system call interface, organized by functionality

pub mod dispatch;
pub mod io;
pub mod process;
pub mod memory;
pub mod file;
pub mod signal;
pub mod time;
pub mod network;
pub mod sched;
pub mod misc;

// Re-export common types and functions
pub use dispatch::{syscall_handler, SyscallArgs};
pub use io::*;
pub use process::*;
pub use memory::*;
pub use file::*;
pub use signal::*;
pub use time::*;
pub use network::*;
pub use sched::*;
pub use misc::*;

/// System call number definitions
#[allow(dead_code)]
#[repr(u32)]
pub enum SyscallNo {
    // IO operations
    IoSetup = 0,
    IoDestroy = 1,
    IoSubmit = 2,
    IoCancel = 3,
    IoGetevents = 4,
    Setxattr = 5,
    Lsetxattr = 6,
    Fsetxattr = 7,
    Getxattr = 8,
    Lgetxattr = 9,
    Fgetxattr = 10,
    Listxattr = 11,
    Llistxattr = 12,
    Flistxattr = 13,
    Removexattr = 14,
    Lremovexattr = 15,
    Fremovexattr = 16,
    Getcwd = 17,
    LookupDcookie = 18,
    Eventfd2 = 19,
    EpollCreate1 = 20,
    EpollCtl = 21,
    EpollPwait = 22,
    Dup = 23,
    Dup2 = 24,
    Fcntl = 25,
    InotifyInit1 = 26,
    InotifyAddWatch = 27,
    InotifyRmWatch = 28,
    Ioctl = 29,
    IoprioSet = 30,
    IoprioGet = 31,
    Flock = 32,
    Mknodat = 33,
    Mkdirat = 34,
    Unlinkat = 35,
    Symlinkat = 36,
    Linkat = 37,
    Renameat = 38,
    Umount = 39,
    Mount = 40,
    PivotRoot = 41,
    NFSServCtl = 42,
    Statfs = 43,
    Fstatfs = 44,
    Truncate = 45,
    Ftruncate = 46,
    Fallocate = 47,
    Faccessat = 48,
    Chdir = 49,
    Fchdir = 50,
    Chroot = 51,
    Fchmod = 52,
    Fchmodat = 53,
    Fchownat = 54,
    Fchown = 55,
    Openat = 56,
    Close = 57,
    Vhangup = 58,
    Pipe2 = 59,
    Quotactl = 60,
    Getdents64 = 61,
    Lseek = 62,
    Read = 63,
    Write = 64,
    Readv = 65,
    Writev = 66,
    Pread64 = 67,
    Pwrite64 = 68,
    Preadv = 69,
    Pwritev = 70,
    Sendfile64 = 71,
    Signalfd4 = 74,
    TimerfdCreate = 85,
    TimerfdSettime = 86,
    TimerfdGettime = 87,
    Utimensat = 88,
    Acct = 89,
    Personality = 92,
    Exit = 93,
    ExitGroup = 94,
    Waitid = 95,
    SetTidAddress = 96,
    Unshare = 97,
    Futex = 98,
    SetRobustList = 99,
    GetRobustList = 100,
    Nanosleep = 101,
    Getitimer = 102,
    Setitimer = 103,
    KexecLoad = 104,
    InitModule = 105,
    DeleteModule = 106,
    TimerCreate = 107,
    TimerGettime = 108,
    TimerGetoverrun = 109,
    TimerSettime = 110,
    TimerDelete = 111,
    ClockSettime = 112,
    ClockGettime = 113,
    ClockGetres = 114,
    ClockNanosleep = 115,
    Syslog = 116,
    Ptraces = 117,
    SchedSetparam = 118,
    SchedSetscheduler = 119,
    SchedGetscheduler = 120,
    SchedGetparam = 121,
    SchedSetaffinity = 122,
    SchedGetaffinity = 123,
    SchedYield = 124,
    SchedGetPriorityMax = 125,
    SchedGetPriorityMin = 126,
    SchedRrGetInterval = 127,
    RestartSyscall = 128,
    Kill = 129,
    Tkill = 130,
    Tgkill = 131,
    Sigaltstack = 132,
    RtSigsuspend = 133,
    RtSigaction = 134,
    RtSigprocmask = 135,
    RtSigpending = 136,
    RtSigtimedwait = 137,
    RtSigqueueinfo = 138,
    RtSigreturn = 139,
    Setpriority = 140,
    Getpriority = 141,
    Reboot = 142,
    Setregid = 143,
    Setgid = 144,
    Setreuid = 145,
    Setuid = 146,
    Setresuid = 147,
    Getresuid = 148,
    Setresgid = 149,
    Getresgid = 150,
    Setfsuid = 151,
    Setfsgid = 152,
    Times = 153,
    Sethostname = 154,
    Setdomainname = 155,
    Getrlimit = 156,
    Setrlimit = 157,
    CreateModule = 158,
    Getdents = 159,
    Uname = 160,
    Gettid = 178,
    Prlimit64 = 261,
    Getrandom = 278,
    Umask = 166,
    Getuid = 174,
    Geteuid = 175,
    Getgid = 176,
    Getegid = 177,

    // File operations
    Fstat = 80,
    Statx = 291,

    // Memory operations
    Brk = 214,
    Mmap = 222,
    Munmap = 215,
    Mremap = 216,
    Mprotect = 226,
    Msync = 227,
    Mlock = 228,
    Munlock = 229,
    Mlockall = 230,
    Munlockall = 231,
    Mincore = 232,
    Madvise = 233,

    // Process operations
    Clone = 220,
    Execve = 221,
    Wait4 = 260,

    // Network
    Socket = 198,
    Socketpair = 199,
    Bind = 200,
    Listen = 201,
    Accept = 202,
    Connect = 203,
    Getsockname = 204,
    Getpeername = 205,
    Sendto = 206,
    Recvfrom = 207,
    Setsockopt = 208,
    Getsockopt = 209,
    Shutdown = 210,
    Sendmsg = 211,
    Recvmsg = 212,

    // System information
    Gettimeofday = 169,
    Settimeofday = 170,

    // Others
    Select = 280,
    Pselect6 = 281,
    Eventfd = 290,
}

/// Error code definitions
#[allow(dead_code)]
pub mod errno {
    pub const EPERM: i32 = 1;       // Operation not permitted
    pub const ENOENT: i32 = 2;      // No such file or directory
    pub const ESRCH: i32 = 3;       // No such process
    pub const EINTR: i32 = 4;       // Interrupted system call
    pub const EIO: i32 = 5;         // I/O error
    pub const ENXIO: i32 = 6;       // No such device or address
    pub const E2BIG: i32 = 7;       // Argument list too long
    pub const ENOEXEC: i32 = 8;     // Exec format error
    pub const EBADF: i32 = 9;       // Bad file number
    pub const ECHILD: i32 = 10;     // No child processes
    pub const EAGAIN: i32 = 11;     // Try again
    pub const ENOMEM: i32 = 12;     // Out of memory
    pub const EACCES: i32 = 13;     // Permission denied
    pub const EFAULT: i32 = 14;     // Bad address
    pub const ENOTBLK: i32 = 15;    // Block device required
    pub const EBUSY: i32 = 16;      // Device or resource busy
    pub const EEXIST: i32 = 17;     // File exists
    pub const EXDEV: i32 = 18;      // Cross-device link
    pub const ENODEV: i32 = 19;     // No such device
    pub const ENOTDIR: i32 = 20;    // Not a directory
    pub const EISDIR: i32 = 21;     // Is a directory
    pub const EINVAL: i32 = 22;     // Invalid argument
    pub const ENFILE: i32 = 23;     // File table overflow
    pub const EMFILE: i32 = 24;     // Too many open files
    pub const ENOTTY: i32 = 25;     // Not a typewriter
    pub const ETXTBSY: i32 = 26;    // Text file busy
    pub const EFBIG: i32 = 27;      // File too large
    pub const ENOSPC: i32 = 28;     // No space left on device
    pub const ESPIPE: i32 = 29;     // Illegal seek
    pub const EROFS: i32 = 30;      // Read-only file system
    pub const EMLINK: i32 = 31;     // Too many links
    pub const EPIPE: i32 = 32;      // Broken pipe
    pub const EDOM: i32 = 33;       // Math argument out of domain
    pub const ERANGE: i32 = 34;     // Math result not representable
    pub const EDEADLK: i32 = 35;    // Resource deadlock avoided
    pub const ENAMETOOLONG: i32 = 36; // File name too long
    pub const ENOSYS: i32 = 38;     // Invalid system call number
    pub const ENOTEMPTY: i32 = 39;  // Directory not empty
    pub const ELOOP: i32 = 40;      // Too many symbolic links encountered
    pub const ENOPROTOOPT: i32 = 92; // Protocol not available
    pub const EOPNOTSUPP: i32 = 95; // Operation not supported
    pub const EAFNOSUPPORT: i32 = 97; // Address family not supported
    pub const EADDRINUSE: i32 = 98; // Address already in use
    pub const EADDRNOTAVAIL: i32 = 99; // Cannot assign requested address
    pub const ENETDOWN: i32 = 100;  // Network is down
    pub const ENETUNREACH: i32 = 101; // Network is unreachable
    pub const ECONNRESET: i32 = 104; // Connection reset by peer
    pub const ENOTCONN: i32 = 107;  // Transport endpoint not connected
    pub const ETIMEDOUT: i32 = 110; // Connection timed out
    pub const ECONNREFUSED: i32 = 111; // Connection refused
    pub const EINPROGRESS: i32 = 115; // Operation now in progress
    pub const ENOTSOCK: i32 = 88;   // Socket operation on non-socket
    pub const ESOCKTNOSUPPORT: i32 = 124; // Socket type not supported
}

/// Time value structure (struct timeval)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// File descriptor set (fd_set)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FdSet {
    pub fds_bits: [u64; 1],
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

/// File descriptor count limit for select system call - from config
pub const FD_SETSIZE: i32 = crate::config::FD_SETSIZE as i32;
