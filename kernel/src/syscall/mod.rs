//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System call module
//!
//! This module implements the RISC-V 64-bit system call interface, organized by functionality

// TREE-LEVEL FORBID (Layer-1 acceleration, 2026-11-21): this inner
// attribute applies to the WHOLE syscall module tree — the compiler
// now enforces the service boundary structurally (any new unsafe in
// any syscall file fails the build, not the per-file lint). The ONE
// escape is the audited edges module, which lives OUTSIDE this tree
// (declared at the crate root via #[path] — see main.rs/lib root).
#![forbid(unsafe_code)]

pub mod seccomp;
pub mod support;

/// The seam registration hook (flip milestone shape): called by
/// rust_main before the impl; the LIB-side default is a no-op — the
/// service-crate composition (the bin) overrides it via the table's
/// OnceSlot when syscall lands. Exists so the call site is stable
/// across the flip commits.
pub fn seam_register_default() {}

// The audited-edge escape: declared at the crate root (outside this
// forbid tree) and re-exported under the historical path.
pub use crate::syscall_edges as edges;
pub mod dispatch;
// pub mod io; — MOVED to the syscall service crate (State B final)
// pub mod process; — MOVED to the syscall service crate (State B final)
// pub mod memory; — MOVED to the syscall service crate (State B final)
// pub mod file; — MOVED to the syscall service crate (State B final)
// pub mod signal; — MOVED to the syscall service crate (State B final)
// pub mod time; — MOVED to the syscall service crate (State B final)
// pub mod network; — MOVED to the syscall service crate (State B final)
// pub mod sched; — MOVED to the syscall service crate (State B final)
// pub mod misc; — MOVED to the syscall service crate (State B final)
// pub mod x86_compat; — MOVED to the syscall service crate (State B final) (x86_64-only file)

// Re-export common types and functions
pub use dispatch::{syscall_handler, SyscallArgs};

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
    // asm-generic 24 is dup3 (the dispatch arm); dup2 has NO generic
    // number — x86_64 native 33 is handled in dispatch's legacy block.
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
    Sethostname = 161,
    Setdomainname = 162,
    Getrlimit = 163,
    Setrlimit = 164,
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

    // Key retention (asm-generic; x86_64 natives are 248/249/250,
    // translated in syscall/x86_compat.rs)
    AddKey = 217,
    RequestKey = 218,
    Keyctl = 219,
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

    // Multiplexing (asm-generic 64-bit has no select/eventfd(2)-less
    // legacy entries — pselect6=72, eventfd2=19; 280=bpf, 281=execveat,
    // 290=pkey_free are NOT select/pselect/eventfd).
    Pselect6 = 72,
    Ppoll = 73,
}

/// Error code definitions — full asm-generic errno table (errno-base.h +
/// errno.h). Values verified against Linux include/uapi/asm-generic/errno.h.
#[allow(dead_code)]
pub mod errno {
    // The full asm-generic table now lives in crate::errno::constants
    // (the framework home; unified 2026-11-21 — the Layer-1 pilot's
    // first severance: kernel-side users reference the framework table
    // directly, syscall re-exports for its own files).
    #[allow(unused_imports)]
    pub use crate::errno::constants::*;
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
    pub fds_bits: [u64; 16],
}

impl FdSet {
    pub const fn new() -> Self {
        Self { fds_bits: [0; 16] }
    }

    pub fn set(&mut self, fd: i32) {
        if fd >= 0 && (fd as usize) < 1024 {
            let fd = fd as usize;
            self.fds_bits[fd / 64] |= 1 << (fd % 64);
        }
    }

    pub fn clear(&mut self, fd: i32) {
        if fd >= 0 && (fd as usize) < 1024 {
            let fd = fd as usize;
            self.fds_bits[fd / 64] &= !(1 << (fd % 64));
        }
    }

    pub fn is_set(&self, fd: i32) -> bool {
        if fd >= 0 && (fd as usize) < 1024 {
            let fd = fd as usize;
            (self.fds_bits[fd / 64] & (1 << (fd % 64))) != 0
        } else {
            false
        }
    }

    pub fn zero(&mut self) {
        self.fds_bits = [0; 16];
    }
}

/// File descriptor count limit for select-style syscalls.
///
/// FD_SETSIZE is fixed at 1024 to match the FdSet layout above (16 × u64 =
/// 1024 bits) and the asm-generic ABI. Taking it from config would silently
/// break the bit layout whenever the config value differs (review 批次1).
pub const FD_SETSIZE: i32 = 1024;
