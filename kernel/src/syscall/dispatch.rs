//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System call dispatch module
//!
//! This module handles system call dispatch and common processing

use crate::arch::pt_regs::PtRegs;
use super::*;

/// System call argument array type
pub type SyscallArgs = [u64; 6];

/// Get system call number from PtRegs
#[inline]
fn syscall_get_nr(regs: &PtRegs) -> u64 {
    regs.syscall_nr() as u64
}

/// Get system call arguments from PtRegs
#[inline]
fn syscall_get_arguments(regs: &PtRegs) -> SyscallArgs {
    regs.syscall_args()
}

// FORENSIC: global syscall ring — replayed at NOVMA crashes to reconstruct
// the victim's final syscall sequence.
pub struct SyscallEntry {
    pub pid: core::sync::atomic::AtomicU32,
    pub nr: core::sync::atomic::AtomicU32,
    pub a0: core::sync::atomic::AtomicU64,
    pub a1: core::sync::atomic::AtomicU64,
    pub ret: core::sync::atomic::AtomicU64,
}
impl SyscallEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            nr: core::sync::atomic::AtomicU32::new(0),
            a0: core::sync::atomic::AtomicU64::new(0),
            a1: core::sync::atomic::AtomicU64::new(0),
            ret: core::sync::atomic::AtomicU64::new(0),
        }
    }
    fn record(&self, pid: u32, nr: u32, a0: u64, a1: u64, ret: u64) {
        use core::sync::atomic::Ordering::Relaxed;
        self.pid.store(pid, Relaxed);
        self.nr.store(nr, Relaxed);
        self.a0.store(a0, Relaxed);
        self.a1.store(a1, Relaxed);
        self.ret.store(ret, Relaxed);
    }
}
const SYSCALL_NEW: SyscallEntry = SyscallEntry::new();
pub static SYSCALL_RING: [SyscallEntry; 2048] = [SYSCALL_NEW; 2048];
pub static SYSCALL_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Set system call return value
#[inline]
fn syscall_set_return_value(regs: &mut PtRegs, value: i64) {
    regs.set_return_value(value);
}

/// dfx=strace1: one console line per loader-relevant syscall of PID 1.
///
/// Numbers are the GENERIC (asm-generic) dispatch numbers — the x86_64
/// native numbers were translated before the dispatch match.
fn strace1_line(nr: u32, args: &SyscallArgs, result: i64) {
    // (name, pathname argument index or None)
    let entry: Option<(&'static str, Option<usize>)> = match nr {
        48 => Some(("faccessat", Some(1))),   // (dirfd, path, mode)
        56 => Some(("openat", Some(1))),      // (dirfd, path, flags, mode)
        57 => Some(("close", None)),
        61 => Some(("getdents64", None)),
        62 => Some(("lseek", None)),
        63 => Some(("read", None)),
        64 => Some(("write", None)),
        67 => Some(("pread64", None)),
        78 => Some(("readlinkat", Some(1))),
        79 => Some(("fstatat", Some(1))),
        80 => Some(("fstat", None)),
        214 => Some(("brk", None)),
        215 => Some(("munmap", None)),
        221 => Some(("execve", Some(0))),
        222 => Some(("mmap", None)),          // fd/off shown numerically
        226 => Some(("mprotect", None)),
        _ => None,
    };
    let (name, path_arg) = match entry {
        Some(e) => e,
        None => return,
    };

    let mut line: alloc::string::String = alloc::format!("S1 {}(", name);
    match path_arg {
        Some(i) => {
            let mut buf = [0u8; 128];
            let shown = match crate::arch::uaccess::strncpy_from_user(
                args[i] as *const u8, buf.len(), &mut buf,
            ) {
                Ok(s) => core::str::from_utf8(s).unwrap_or("<non-utf8>"),
                Err(_) => "<efault>",
            };
            line.push_str(&alloc::format!("\"{}\"", shown));
            if i > 0 {
                line.push_str(&alloc::format!(", dirfd={}", args[0]));
            }
            if name == "openat" {
                line.push_str(&alloc::format!(", flags={:#x}", args[2]));
            }
        }
        None => match name {
            "mmap" => line.push_str(&alloc::format!(
                "addr={:#x} len={:#x} prot={:#x} flags={:#x} fd={} off={:#x}",
                args[0], args[1], args[2], args[3], args[4] as i64, args[5]
            )),
            "read" | "pread64" => line.push_str(&alloc::format!(
                "fd={} count={:#x}{}", args[0] as i64, args[2],
                if name == "pread64" { alloc::format!(", off={:#x}", args[3]) } else { alloc::string::String::new() }
            )),
            _ => line.push_str(&alloc::format!(
                "{}{}", args[0] as i64,
                if name == "write" || name == "lseek" || name == "fstat" || name == "close" {
                    alloc::format!(", {:#x}", args[1])
                } else if name == "lseek" || name == "mprotect" {
                    alloc::format!(", {:#x}, {:#x}", args[1], args[2])
                } else {
                    alloc::string::String::new()
                }
            )),
        },
    }
    line.push_str(&alloc::format!(") = {}", result));
    crate::pr_info!("{}", line);
}

/// System call entry function
///
/// Called by trap.rs, dispatches to specific system call handlers
pub extern "C" fn syscall_handler(regs: &mut PtRegs) {
    let syscall_no = syscall_get_nr(regs);
    let mut args = syscall_get_arguments(regs);

    // LASTSYS latch at the very top — BEFORE the x86_64 legacy early
    // returns (arch_prctl 158, poll 7, select 23, dup2 33, pause 34,
    // epoll_create 213, epoll_wait 232): glibc's epoll_wait() issues the
    // NATIVE 232, so a task parked in an event loop latched under the
    // later entry point showed its PREVIOUS syscall (a prctl!) instead
    // of the epoll_wait it is sleeping in. The raw native number is
    // stored; decode against the x86_64 table.
    {
        let cur = crate::sched::current();
        if let Some(cur) = cur {
            use core::sync::atomic::Ordering::Relaxed;
            // SAFETY: cur is the running task.
            unsafe {
                (*cur).last_syscall_nr.store(syscall_no as u32, Relaxed);
                (*cur).last_syscall_a0.store(args[0], Relaxed);
                (*cur).last_syscall_a1.store(args[1], Relaxed);
            }
        }
    }

    // x86_64: translate the native number/ABI onto the generic table
    // (identity for same-numbered entries, argument remaps for the
    // legacy no-`at` ABI). Untranslatable numbers report as unknown.
    // x86_64-native syscalls with no asm-generic equivalent.
    #[cfg(feature = "x86_64")]
    if syscall_no == 158 {
        // arch_prctl(2): TLS base (glibc/musl startup requirement).
        let result = crate::arch::process::sys_arch_prctl(args);
        syscall_set_return_value(regs, result);
        return;
    }

    // x86_64 legacy syscalls that have NO asm-generic number — handled
    // BEFORE translate() so they can never collide with the same-numbered
    // generic entry (poll(7) vs lsetxattr(7), select(23) vs dup(23),
    // pause(34) vs mkdirat(34), epoll_create(213) vs readahead(213),
    // epoll_wait(232) vs nothing → ENOSYS). asm-generic libc builds
    // emulate these over ppoll/pselect6/sigsuspend, but x86_64 musl/glibc
    // issue the native numbers (LTP x86-r2 N2 collaterals: poll01 EBADF,
    // pause01 EFAULT-from-mkdirat, epoll_create01, epoll_wait01).
    #[cfg(feature = "x86_64")]
    match syscall_no {
        7 => {
            // poll(fds, nfds, timeout_ms) — handler exists, no generic slot.
            syscall_set_return_value(regs, misc::sys_poll(args));
            return;
        }
        23 => {
            // select(nfds, r, w, e, timeval*) — timeval timeout variant.
            syscall_set_return_value(regs, misc::sys_select(args));
            return;
        }
        33 => {
            // dup2(oldfd, newfd). musl/glibc on x86_64 issue the native
            // NR 33 (on asm-generic archs their libc emulates the equal-fd
            // case over fcntl+dup3 before reaching the kernel — musl
            // dup2.c). Linux dup2 semantics: oldfd == newfd returns newfd
            // if it is open, else EBADF; dup3() itself would EINVAL.
            let result = if args[0] == args[1] {
                let probe = io::sys_fcntl([args[0], 1 /* F_GETFD */, 0, 0, 0, 0]);
                if probe < 0 {
                    probe // -EBADF from the lookup itself
                } else {
                    args[0] as i64
                }
            } else {
                io::sys_dup3([args[0], args[1], 0, 0, 0, 0])
            };
            syscall_set_return_value(regs, result);
            return;
        }
        34 => {
            // pause(): sleep until a signal; always -EINTR.
            syscall_set_return_value(regs, signal::sys_pause(args));
            return;
        }
        213 => {
            // epoll_create(size): Linux rejects size <= 0 with EINVAL.
            let result = if (args[0] as i64) <= 0 {
                -(errno::EINVAL as i64)
            } else {
                misc::sys_epoll_create(args)
            };
            syscall_set_return_value(regs, result);
            return;
        }
        232 => {
            // epoll_wait(epfd, events, maxevents, timeout_ms) — handler
            // exists, no generic slot (generic uses epoll_pwait).
            syscall_set_return_value(regs, misc::sys_epoll_wait(args));
            return;
        }
        _ => {}
    }

    #[cfg(feature = "x86_64")]
    let syscall_no = match crate::syscall::x86_compat::translate(syscall_no as u64, &args) {
        Some((generic_nr, generic_args)) => {
            args = generic_args;
            generic_nr as i64
        }
        None => syscall_no as i64,
    };

    // FORENSIC ring ENTRY record (ret sentinel = in-flight): a task
    // parked INSIDE a syscall (epoll_wait/binder ioctl/futex/read on a
    // slow fd) never reaches the exit-side record below, so the exit-only
    // ring showed its last COMPLETED call instead of the blocking one.
    // Also latched into the task itself (last_syscall_*) — the ring is
    // only 2048 entries, so a parked waiter's record is overwritten
    // within milliseconds under system-wide traffic; the taskdump's
    // lastsys= field reads the per-task latch.
    SYSCALL_RING[SYSCALL_CURSOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % SYSCALL_RING.len()]
        .record(crate::process::current_pid(), syscall_no as u32, args[0], args[1], u64::MAX);
    if let Some(cur) = crate::sched::current() {
        use core::sync::atomic::Ordering::Relaxed;
        // SAFETY: cur is the running task.
        unsafe {
            (*cur).last_syscall_nr.store(syscall_no as u32, Relaxed);
            (*cur).last_syscall_a0.store(args[0], Relaxed);
            (*cur).last_syscall_a1.store(args[1], Relaxed);
        }
    }

    // Dispatch based on system call number (sorted by number)
    let result: i64 = match syscall_no as u32 {
        // ==================== Linux AIO (NR 0-4) ====================
        0 => memory::sys_io_setup(args),       // io_setup
        1 => memory::sys_io_destroy(args),     // io_destroy
        2 => memory::sys_io_submit(args),      // io_submit
        3 => memory::sys_io_cancel(args),      // io_cancel
        4 => memory::sys_io_getevents(args),   // io_getevents

        // ==================== Extended Attributes (NR 5-16) ====================
        5 => file::sys_setxattr(args),         // setxattr
        6 => file::sys_lsetxattr(args),        // lsetxattr
        7 => file::sys_fsetxattr(args),        // fsetxattr
        8 => file::sys_getxattr(args),         // getxattr
        9 => file::sys_lgetxattr(args),        // lgetxattr
        10 => file::sys_fgetxattr(args),       // fgetxattr
        11 => file::sys_listxattr(args),       // listxattr
        12 => file::sys_llistxattr(args),      // llistxattr
        13 => file::sys_flistxattr(args),      // flistxattr
        14 => file::sys_removexattr(args),     // removexattr
        15 => file::sys_lremovexattr(args),    // lremovexattr
        16 => file::sys_fremovexattr(args),    // fremovexattr

        // ==================== File Operations ====================
        17 => file::sys_getcwd(args),
        18 => time::sys_lookup_dcookie(args),  // lookup_dcookie
        19 => misc::sys_eventfd2(args),        // eventfd2
        20 => misc::sys_epoll_create1(args),   // epoll_create1 (flags!
        // LTP round 3: NR 20 used to land on sys_epoll_create, silently
        // dropping EPOLL_CLOEXEC — epoll_create1_01's F_GETFD check failed)
        21 => misc::sys_epoll_ctl(args),       // epoll_ctl
        22 => misc::sys_epoll_pwait(args),     // epoll_pwait
        26 => misc::sys_inotify_init1(args),   // inotify_init1
        27 => misc::sys_inotify_add_watch(args), // inotify_add_watch
        28 => misc::sys_inotify_rm_watch(args),  // inotify_rm_watch
        23 => io::sys_dup(args),               // dup
        24 => io::sys_dup3(args),              // dup3
        25 => io::sys_fcntl(args),             // fcntl
        29 => io::sys_ioctl(args),             // ioctl
        30 => process::sys_ioprio_set(args),   // ioprio_set
        31 => process::sys_ioprio_get(args),   // ioprio_get
        32 => io::sys_flock(args),             // flock
        33 => file::sys_mknodat(args),         // mknodat
        34 => file::sys_mkdirat(args),         // mkdirat
        35 => file::sys_unlinkat(args),        // unlinkat
        36 => file::sys_symlinkat(args),       // symlinkat
        37 => file::sys_linkat(args),          // linkat
        38 => file::sys_renameat(args),        // renameat
        39 => file::sys_umount(args),          // umount2
        40 => file::sys_mount(args),           // mount
        41 => process::sys_pivot_root(args),   // pivot_root
        42 => time::sys_nfsservctl(args),      // nfsservctl
        43 => file::sys_statfs(args),          // statfs
        44 => file::sys_fstatfs(args),         // fstatfs
        45 => file::sys_truncate(args),        // truncate
        46 => file::sys_ftruncate(args),       // ftruncate
        47 => file::sys_fallocate(args),       // fallocate
        48 => file::sys_faccessat(args),       // faccessat
        49 => file::sys_chdir(args),           // chdir
        50 => file::sys_fchdir(args),          // fchdir
        51 => file::sys_chroot(args),          // chroot
        52 => file::sys_fchmod(args),          // fchmod
        53 => file::sys_fchmodat(args),        // fchmodat
        54 => file::sys_fchownat(args),        // fchownat
        55 => file::sys_fchown(args),          // fchown
        56 => file::sys_openat(args),          // openat
        57 => file::sys_close(args),           // close
        58 => file::sys_vhangup(args),         // vhangup
        59 => io::sys_pipe2(args),             // pipe2
        60 => process::sys_quotactl(args),     // quotactl
        61 => file::sys_getdents64(args),      // getdents64
        62 => file::sys_lseek(args),           // lseek
        63 => io::sys_read(args),              // read
        64 => io::sys_write(args),             // write
        65 => io::sys_readv(args),             // readv
        66 => io::sys_writev(args),            // writev
        67 => io::sys_pread64(args),           // pread64
        68 => io::sys_pwrite64(args),          // pwrite64
        69 => io::sys_preadv(args),            // preadv
        70 => io::sys_pwritev(args),           // pwritev
        71 => io::sys_sendfile(args),          // sendfile
        72 => misc::sys_pselect6(args),        // pselect6
        73 => misc::sys_ppoll(args),           // ppoll
        74 => signal::sys_signalfd4(args),     // signalfd4
        75 => io::sys_vmsplice(args),          // vmsplice
        76 => io::sys_splice(args),            // splice
        77 => io::sys_tee(args),               // tee
        78 => file::sys_readlinkat(args),      // readlinkat
        79 => file::sys_fstatat(args),         // fstatat
        80 => file::sys_fstat(args),           // fstat
        81 => file::sys_sync(args),            // sync
        82 => file::sys_fsync(args),           // fsync
        83 => file::sys_fdatasync(args),       // fdatasync
        84 => file::sys_sync_file_range(args), // sync_file_range
        85 => misc::sys_timerfd_create(args),  // timerfd_create
        86 => misc::sys_timerfd_settime(args), // timerfd_settime
        87 => misc::sys_timerfd_gettime(args), // timerfd_gettime
        88 => file::sys_futimesat(args),       // utimensat
        89 => file::sys_acct(args),            // acct

        // ==================== Process Operations ====================
        90 => process::sys_capget(args),       // capget
        91 => process::sys_capset(args),       // capset
        92 => process::sys_personality(args),  // personality
        93 => process::sys_exit(args),         // exit (calling thread only)
        94 => process::sys_exit_group(args),   // exit_group (whole thread group)
        95 => process::sys_waitid(args),       // waitid
        96 => process::sys_set_tid_address(args),
        97 => process::sys_unshare(args),      // unshare
        98 => sched::sys_futex(args),          // futex
        99 => process::sys_set_robust_list(args),
        100 => time::sys_get_robust_list(args), // get_robust_list
        101 => time::sys_nanosleep(args),      // nanosleep
        102 => time::sys_getitimer(args),      // getitimer
        103 => time::sys_setitimer(args),      // setitimer
        104 => process::sys_kexec_load(args),  // kexec_load
        105 => process::sys_init_module(args), // init_module
        106 => process::sys_delete_module(args), // delete_module
        107 => time::sys_timer_create(args),   // timer_create
        108 => time::sys_timer_gettime(args),  // timer_gettime
        109 => time::sys_timer_getoverrun(args), // timer_getoverrun
        110 => time::sys_timer_settime(args),  // timer_settime
        111 => time::sys_timer_delete(args),   // timer_delete
        112 => time::sys_clock_settime(args),  // clock_settime
        113 => time::sys_clock_gettime(args),  // clock_gettime
        114 => time::sys_clock_getres(args),   // clock_getres
        115 => time::sys_clock_nanosleep(args),// clock_nanosleep
        116 => crate::printk::sys_syslog(args),  // syslog
        117 => crate::process::ptrace::sys_ptrace(args), // ptrace
        118 => sched::sys_sched_setparam(args),// sched_setparam
        119 => sched::sys_sched_setscheduler(args), // sched_setscheduler
        120 => sched::sys_sched_getscheduler(args), // sched_getscheduler
        121 => sched::sys_sched_getparam(args),    // sched_getparam
        122 => sched::sys_sched_setaffinity(args), // sched_setaffinity
        123 => sched::sys_sched_getaffinity(args), // sched_getaffinity
        124 => sched::sys_sched_yield(args),   // sched_yield
        125 => sched::sys_sched_get_priority_max(args), // sched_get_priority_max
        126 => sched::sys_sched_get_priority_min(args), // sched_get_priority_min
        127 => sched::sys_sched_rr_get_interval(args), // sched_rr_get_interval
        128 => signal::sys_restart_syscall(args), // restart_syscall
        129 => process::sys_kill(args),        // kill
        130 => signal::sys_tkill(args),        // tkill
        131 => process::sys_tgkill(args),      // tgkill
        132 => signal::sys_sigaltstack(args),  // sigaltstack
        133 => signal::sys_rt_sigsuspend(args), // rt_sigsuspend (Linux NR 133)
        134 => signal::sys_rt_sigaction(args), // rt_sigaction (Linux NR 134)
        135 => signal::sys_rt_sigprocmask(args),// rt_sigprocmask
        136 => signal::sys_sigpending(args),   // rt_sigpending (Linux NR 136)
        137 => process::sys_rt_sigtimedwait(args), // rt_sigtimedwait
        138 => process::sys_rt_sigqueueinfo(args), // rt_sigqueueinfo
        139 => signal::sys_rt_sigreturn(regs), // rt_sigreturn
        140 => sched::sys_setpriority(args),   // setpriority (R7-4: was swapped)
        141 => sched::sys_getpriority(args),   // getpriority
        142 => process::sys_reboot(args),      // reboot
        143 => process::sys_setregid(args),    // setregid
        144 => process::sys_setgid(args),      // setgid
        145 => process::sys_setreuid(args),    // setreuid
        146 => process::sys_setuid(args),      // setuid
        147 => process::sys_setresuid(args),   // setresuid
        148 => process::sys_getresuid(args),   // getresuid
        149 => process::sys_setresgid(args),   // setresgid
        150 => process::sys_getresgid(args),   // getresgid
        151 => process::sys_setfsuid(args),    // setfsuid
        152 => process::sys_setfsgid(args),    // setfsgid
        153 => process::sys_times(args),       // times
        154 => process::sys_setpgid(args),     // setpgid
        155 => process::sys_getpgid(args),     // getpgid
        156 => process::sys_getsid(args),      // getsid
        157 => process::sys_setsid(args),      // setsid
        158 => process::sys_getgroups(args),   // getgroups
        159 => process::sys_setgroups(args),   // setgroups
        160 => process::sys_uname(args),       // newuname
        161 => process::sys_sethostname(args), // sethostname
        162 => process::sys_setdomainname(args), // setdomainname
        163 => process::sys_getrlimit(args),   // getrlimit
        164 => process::sys_setrlimit(args),   // setrlimit
        165 => process::sys_getrusage(args),   // getrusage
        166 => file::sys_umask(args),          // umask
        167 => process::sys_prctl(args),       // prctl
        168 => process::sys_getcpu(args),      // getcpu
        169 => time::sys_gettimeofday(args),   // gettimeofday
        170 => time::sys_settimeofday(args),   // settimeofday
        171 => time::sys_adjtimex(args),       // adjtimex
        172 => process::sys_getpid(args),      // getpid
        173 => process::sys_getppid(args),     // getppid
        174 => process::sys_getuid(args),      // getuid
        175 => process::sys_geteuid(args),     // geteuid
        176 => process::sys_getgid(args),      // getgid
        177 => process::sys_getegid(args),     // getegid
        178 => process::sys_gettid(args),      // gettid
        179 => process::sys_sysinfo(args),     // sysinfo

        // ==================== IPC Operations ====================
        // NR 180-185: POSIX MQ
        180 => crate::ipc::posix_mq::sys_mq_open(args),       // mq_open
        181 => crate::ipc::posix_mq::sys_mq_unlink(args),     // mq_unlink
        182 => crate::ipc::posix_mq::sys_mq_timedsend(args),  // mq_timedsend
        183 => crate::ipc::posix_mq::sys_mq_timedreceive(args), // mq_timedreceive
        184 => crate::ipc::posix_mq::sys_mq_notify(args),     // mq_notify
        185 => crate::ipc::posix_mq::sys_mq_getsetattr(args), // mq_getsetattr
        // NR 186-189: System V msg
        186 => crate::ipc::sysv_msg::sys_msgget(args),       // msgget
        187 => crate::ipc::sysv_msg::sys_msgctl(args),       // msgctl
        188 => crate::ipc::sysv_msg::sys_msgrcv(args),       // msgrcv
        189 => crate::ipc::sysv_msg::sys_msgsnd(args),       // msgsnd
        // NR 190-193: System V sem
        190 => crate::ipc::sysv_sem::sys_semget(args),       // semget
        191 => crate::ipc::sysv_sem::sys_semctl(args),       // semctl
        192 => crate::ipc::sysv_sem::sys_semtimedop(args),   // semtimedop
        193 => crate::ipc::sysv_sem::sys_semop(args),        // semop
        // NR 194-197: System V shm
        194 => crate::ipc::sysv_shm::sys_shmget(args),       // shmget
        195 => crate::ipc::sysv_shm::sys_shmctl(args),       // shmctl
        196 => crate::ipc::sysv_shm::sys_shmat(args),        // shmat
        197 => crate::ipc::sysv_shm::sys_shmdt(args),        // shmdt

        // ==================== Network Operations ====================
        198 => network::sys_socket(args),      // socket
        199 => network::sys_socketpair(args),  // socketpair
        200 => network::sys_bind(args),        // bind
        201 => network::sys_listen(args),      // listen
        202 => network::sys_accept(args),      // accept
        203 => network::sys_connect(args),     // connect
        204 => network::sys_getsockname(args), // getsockname
        205 => network::sys_getpeername(args), // getpeername
        206 => network::sys_sendto(args),      // sendto
        207 => network::sys_recvfrom(args),    // recvfrom
        208 => network::sys_setsockopt(args),  // setsockopt
        209 => network::sys_getsockopt(args),  // getsockopt
        210 => network::sys_shutdown(args),    // shutdown
        211 => network::sys_sendmsg(args),     // sendmsg
        212 => network::sys_recvmsg(args),     // recvmsg
        243 => network::sys_recvmmsg(args),    // recvmmsg
        242 => network::sys_accept4(args),     // accept4

        // ==================== Memory Operations ====================
        213 => file::sys_readahead(args),      // readahead
        214 => memory::sys_brk(args),          // brk
        215 => memory::sys_munmap(args),       // munmap
        216 => memory::sys_mremap(args),       // mremap
        222 => memory::sys_mmap(args),         // mmap
        223 => memory::sys_fadvise64(args),    // fadvise64
        224 => file::sys_swapon(args),         // swapon
        225 => file::sys_swapoff(args),        // swapoff
        226 => memory::sys_mprotect(args),     // mprotect
        227 => memory::sys_msync(args),        // msync
        228 => memory::sys_mlock(args),        // mlock
        229 => memory::sys_munlock(args),      // munlock
        230 => memory::sys_mlockall(args),     // mlockall
        231 => memory::sys_munlockall(args),   // munlockall
        232 => memory::sys_mincore(args),      // mincore
        233 => memory::sys_madvise(args),      // madvise
        234 => memory::sys_remap_file_pages(args), // remap_file_pages
        235 => memory::sys_mbind(args),        // mbind
        236 => memory::sys_get_mempolicy(args), // get_mempolicy
        237 => memory::sys_set_mempolicy(args), // set_mempolicy
        238 => memory::sys_migrate_pages(args), // migrate_pages
        239 => memory::sys_move_pages(args),   // move_pages
        240 => process::sys_rt_tgsigqueueinfo(args), // rt_tgsigqueueinfo (asm-generic 240)
        241 => process::sys_perf_event_open(args),   // perf_event_open (asm-generic 241)

        // ==================== Process Operations (cont.) ====================
        220 => process::sys_clone(args),       // clone
        221 => process::sys_execve(args),      // execve
        260 => process::sys_wait4(args),       // wait4
        261 => process::sys_prlimit64(args),   // prlimit64
        268 => process::sys_setns(args),       // setns

        // ==================== RISC-V Specific ====================
        258 => process::sys_riscv_hwprobe(args), // riscv_hwprobe
        259 => process::sys_riscv_flush_icache(args), // riscv_flush_icache
        267 => process::sys_syncfs(args),      // syncfs

        // ==================== Scheduler Extended ====================
        274 => sched::sys_sched_setattr(args), // sched_setattr (Linux NR 274)
        275 => sched::sys_sched_getattr(args), // sched_getattr (Linux NR 275)

        // ==================== File Handle / Copy ====================
        264 => file::sys_name_to_handle_at(args), // name_to_handle_at
        265 => file::sys_open_by_handle_at(args), // open_by_handle_at
        266 => time::sys_clock_adjtime(args),  // clock_adjtime
        269 => network::sys_sendmmsg(args),    // sendmmsg
        270 => process::sys_process_vm_readv(args), // process_vm_readv
        271 => process::sys_process_vm_writev(args), // process_vm_writev
        272 => process::sys_kcmp(args),        // kcmp
        273 => process::sys_finit_module(args), // finit_module

        // ==================== Select/Poll/Epoll ====================
        276 => file::sys_renameat2(args),      // renameat2
        277 => process::sys_seccomp(args),     // seccomp
        278 => misc::sys_getrandom(args),      // getrandom
        279 => process::sys_memfd_create(args), // memfd_create
        280 => process::sys_bpf(args),         // bpf
        281 => process::sys_execveat(args),    // execveat
        282 => process::sys_userfaultfd(args), // userfaultfd
        283 => process::sys_membarrier(args),  // membarrier
        284 => memory::sys_mlock2(args),       // mlock2
        285 => file::sys_copy_file_range(args), // copy_file_range
        286 => file::sys_preadv2(args),        // preadv2
        287 => file::sys_pwritev2(args),       // pwritev2
        288 => memory::sys_pkey_mprotect(args), // pkey_mprotect
        289 => memory::sys_pkey_alloc(args),   // pkey_alloc
        290 => memory::sys_pkey_free(args),    // pkey_free
        292 => memory::sys_io_pgetevents(args), // io_pgetevents
        293 => time::sys_rseq(args),           // rseq
        294 => process::sys_kexec_file_load(args), // kexec_file_load

        // ==================== Fanotify ====================
        262 => time::sys_fanotify_init(args),  // fanotify_init
        263 => time::sys_fanotify_mark(args),  // fanotify_mark

        // ==================== Others ====================
        291 => file::sys_statx(args),          // statx
        437 => file::sys_openat2(args),        // openat2

        // ==================== _time64 variants (NR 403-423) ====================
        403 => time::sys_clock_gettime64(args),    // clock_gettime64
        404 => time::sys_clock_settime64(args),    // clock_settime64
        405 => time::sys_clock_adjtime64(args),    // clock_adjtime64
        406 => time::sys_clock_getres_time64(args), // clock_getres_time64
        407 => time::sys_clock_nanosleep_time64(args), // clock_nanosleep_time64
        408 => time::sys_timer_gettime64(args),   // timer_gettime64
        409 => time::sys_timer_settime64(args),   // timer_settime64
        410 => time::sys_timerfd_gettime64(args), // timerfd_gettime64
        411 => time::sys_timerfd_settime64(args), // timerfd_settime64
        412 => time::sys_utimensat_time64(args),  // utimensat_time64
        413 => time::sys_pselect6_time64(args),  // pselect6_time64
        414 => time::sys_ppoll_time64(args),     // ppoll_time64
        416 => time::sys_io_pgetevents_time64(args), // io_pgetevents_time64
        417 => time::sys_recvmmsg_time64(args),  // recvmmsg_time64
        418 => crate::ipc::posix_mq::sys_mq_timedsend(args),  // mq_timedsend_time64
        419 => crate::ipc::posix_mq::sys_mq_timedreceive(args), // mq_timedreceive_time64
        420 => crate::ipc::sysv_sem::sys_semtimedop(args),  // semtimedop_time64
        421 => time::sys_rt_sigtimedwait_time64(args), // rt_sigtimedwait_time64
        422 => time::sys_futex_time64(args),    // futex_time64
        423 => time::sys_sched_rr_get_interval_time64(args), // sched_rr_get_interval_time64

        // ==================== Latest Kernel Features (NR 424-470) ====================
        424 => process::sys_pidfd_send_signal(args), // pidfd_send_signal
        425 => process::sys_io_uring_setup(args),   // io_uring_setup
        426 => process::sys_io_uring_enter(args),   // io_uring_enter
        427 => process::sys_io_uring_register(args), // io_uring_register
        428 => file::sys_open_tree(args),       // open_tree
        429 => file::sys_move_mount(args),      // move_mount
        430 => file::sys_fsopen(args),         // fsopen
        431 => file::sys_fsconfig(args),       // fsconfig
        432 => file::sys_fsmount(args),        // fsmount
        433 => file::sys_fspick(args),         // fspick
        434 => process::sys_pidfd_open(args),   // pidfd_open
        435 => process::sys_clone3(args),       // clone3
        436 => process::sys_close_range(args),  // close_range
        438 => process::sys_pidfd_getfd(args),  // pidfd_getfd
        439 => process::sys_faccessat2(args),  // faccessat2
        440 => process::sys_process_madvise(args), // process_madvise
        441 => file::sys_epoll_pwait2(args),   // epoll_pwait2
        442 => file::sys_mount_setattr(args),  // mount_setattr
        443 => file::sys_quotactl_fd(args),   // quotactl_fd
        444 => file::sys_landlock_create_ruleset(args), // landlock_create_ruleset
        445 => file::sys_landlock_add_rule(args), // landlock_add_rule
        446 => file::sys_landlock_restrict_self(args), // landlock_restrict_self
        447 => process::sys_memfd_secret(args), // memfd_secret
        448 => process::sys_process_mrelease(args), // process_mrelease
        449 => file::sys_futex_waitv(args),     // futex_waitv
        450 => memory::sys_set_mempolicy_home_node(args), // set_mempolicy_home_node
        451 => file::sys_cachestat(args),       // cachestat
        452 => file::sys_fchmodat2(args),       // fchmodat2
        453 => file::sys_map_shadow_stack(args), // map_shadow_stack
        454 => file::sys_futex_wake(args),      // futex_wake
        455 => file::sys_futex_wait(args),      // futex_wait
        456 => file::sys_futex_requeue(args),   // futex_requeue
        457 => file::sys_statmount(args),       // statmount
        458 => file::sys_listmount(args),       // listmount
        459 => file::sys_lsm_get_self_attr(args), // lsm_get_self_attr
        460 => file::sys_lsm_set_self_attr(args), // lsm_set_self_attr
        461 => file::sys_lsm_list_modules(args), // lsm_list_modules
        462 => file::sys_mseal(args),          // mseal
        463 => file::sys_setxattrat(args),      // setxattrat
        464 => file::sys_getxattrat(args),      // getxattrat
        465 => file::sys_listxattrat(args),     // listxattrat
        466 => file::sys_removexattrat(args),   // removexattrat
        467 => file::sys_open_tree_attr(args),  // open_tree_attr
        468 => file::sys_file_getattr(args),    // file_getattr
        469 => file::sys_file_setattr(args),    // file_setattr
        470 => file::sys_listns(args),         // listns

        // ==================== Unimplemented System Calls ====================
        _ => {
            crate::println!("syscall: unknown syscall {} (args: {:#x}, {:#x}, {:#x})",
                syscall_no, args[0], args[1], args[2]);
            -(errno::ENOSYS as i64)
        }
    };

    // EFAULTTRAC (sleep-EFAULT hunt): python's time.sleep surfaces EFAULT
    // from some syscall, but not from any of the instrumented sleep paths.
    // Name every syscall that returns exactly -EFAULT so the culprit is
    // identifiable from the console. Rate-limited.
    if result == -(errno::EFAULT as i64) {
        static EFT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if EFT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60 {
            crate::pr_info!(
                "EFAULTTRAC: pid={} nr={} a0={:#x} a1={:#x}",
                crate::process::current_pid(), syscall_no, args[0], args[1]
            );
        }
    }

    // STRACE1 (loader hunts, dfx=strace1): one line per file/memory
    // syscall of PID 1, with the pathname decoded for path-taking calls.
    // musl ld.so's "Error loading shared library" carries no errno — this
    // turns that into "which step failed" (search-open, fstat, mmap,
    // header read).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::Strace1)
        && crate::process::current_pid() == 1
    {
        strace1_line(syscall_no as u32, &args, result);
    }

    syscall_set_return_value(regs, result);

    // FORENSIC ring: last N syscalls per boot (pid, nr, a0, a1, ret).
    SYSCALL_RING[SYSCALL_CURSOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % SYSCALL_RING.len()]
        .record(crate::process::current_pid(), syscall_no as u32, args[0], args[1], result as u64);

    // sc128 (Session P): per-task nr-only ring of the last syscalls
    // (successes included) — read at APP-PARK to see the child's final
    // action sequence, not just its failures. sc135 (Session Q): ring
    // deepened 12 -> LAST_CALLS_LEN (96) — sc129's 12-deep ring saw only
    // the clock_gettime retry loop masking the real pre-park sequence.
    if let Some(cur) = crate::sched::current() {
        use core::sync::atomic::Ordering::Relaxed;
        let idx = unsafe { (*cur).last_calls_cur.fetch_add(1, Relaxed) }
            % crate::process::task::LAST_CALLS_LEN;
        unsafe { (*cur).last_calls[idx].store((syscall_no as u32) & 0xffff, Relaxed) };
    }

    // sc138 (Session Q): FILTERED init ring — skips the parked child's
    // retry-loop noise ({113 clock_gettime, 66 writev, 203 connect} and
    // main-stack-top futex waits) so APP-PARK / APP-THR dumps carry the
    // init sequence itself (loader/prctl/mprotect/clone cascade) instead
    // of a clock_gettime wall. This is the working-vs-parked byte-diff
    // substrate: park dumps show the aborted init; APP-THR dumps show a
    // full one that reached pthread_create.
    {
        let skip = match syscall_no {
            113 | 66 | 203 => true,
            // futex: sc138 showed the park loop also spins FUTEX WAITS on
            // the runtime-arena words (0x7ff00002aec0 family, short
            // timeouts) — those flooded the init ring as badly as the
            // clocks. sc139: skip ALL futex WAITs (op FUTEX_WAIT(0) /
            // FUTEX_WAIT_BITSET(9), with or without PRIVATE(0x80)) in the
            // init ring; WAKEs and requeues stay recorded — a wake is a
            // worker-handshake event, exactly what the diff needs. The
            // final park wait is named by the APP-PARK header itself.
            98 => {
                let op = (args[1] & 0x7f) as u32;
                op == 0 /* FUTEX_WAIT */ || op == 9 /* FUTEX_WAIT_BITSET */
                    || args[0] >= 0x7ffffffd0000 && args[0] < 0x7ffffffff000
            }
            _ => false,
        };
        if !skip {
            if let Some(cur) = crate::sched::current() {
                use core::sync::atomic::Ordering::Relaxed;
                let n = unsafe { (*cur).init_calls_cur.fetch_add(1, Relaxed) };
                let idx = n % crate::process::task::INIT_CALLS_LEN;
                unsafe {
                    (*cur).init_calls[idx].store((syscall_no as u32) & 0xffff, Relaxed);
                    (*cur).init_a0[idx].store(args[0], Relaxed);
                }
                // sc143: latch the birth window once (first 64 filtered
                // calls of this task's life) — the deep ring's tail gets
                // churned by retry loops, the birth window cannot.
                if n < crate::process::task::BIRTH_CALLS_LEN as usize {
                    unsafe {
                        (*cur).birth_calls[n].store((syscall_no as u32) & 0xffff, Relaxed);
                        (*cur).birth_a0[n].store(args[0], Relaxed);
                    }
                }
            }
        }
    }

    // sc125 (Session P): per-task ring of the last 8 FAILING syscalls —
    // the park decode needs the pre-park failure tail (musl pthread_create
    // mmaps the thread stack BEFORE clone; a failed mmap means no clone
    // ever reaches the kernel census and nothing else records it).
    // sc135 (Session Q): a0/a1 stored alongside — the nr167=-22 entries
    // need the prctl OPTION named.
    if result < 0 && result > -4096 {
        if let Some(cur) = crate::sched::current() {
            let packed = ((-(result) as u32) << 16) | ((syscall_no as u32) & 0xffff);
            let idx = unsafe { (*cur).last_fails_cur.fetch_add(1, core::sync::atomic::Ordering::Relaxed) } % 8;
            unsafe { (*cur).last_fails[idx].store(packed, core::sync::atomic::Ordering::Relaxed) };
            unsafe { (*cur).last_fails_a0[idx].store(args[0], core::sync::atomic::Ordering::Relaxed) };
            unsafe { (*cur).last_fails_a1[idx].store(args[1], core::sync::atomic::Ordering::Relaxed) };
        }
    }

    // sc140 (Session Q): the parked child's retry loop runs ioctl x3-5
    // between dlopen attempts — name the FD and COMMAND (binder
    // BINDER_WRITE_READ? /dev/xpm? a param-area ioctl?). The fd set at
    // park time has no xpm/dec fd, so the loop polls an open service fd.
    if syscall_no == 29 {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let family = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn";
        if family {
            static IO_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if IO_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 80 {
                crate::pr_warn!(
                    "appioctl: pid={} comm='{}' fd={} cmd={:#x} ret={}",
                    crate::process::current_pid(),
                    comm,
                    args[0],
                    args[1],
                    result
                );
            }
        }
    }

    // sc144-149: the parked children's OWN hilog lines never reach
    // hilogd (socket EAGAIN backpressure) — yet they are the narration
    // of exactly where the runtime init stalls. Dump the first 3 iovecs
    // of every writev to fd 3 from the app family (daemon excluded):
    // sc149 showed the child's sandbox-phase narration; the post-sandbox
    // runtime phase writes SUCCEED and need printing too (print every
    // result, budget 700).
    if syscall_no == 66 && args[0] == 3 {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let family = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn"
            || comm.contains("foundation")
            || comm.contains("sa_main");
        if family {
            // sc157: comm-based gating. The sc155/156 pid latch (first
            // family task to write fd3 = "the daemon") was fragile — in
            // sc156 an earlier appspawn-family writer latched a
            // non-daemon pid and the daemon's preload storm (ImplClass/
            // APPSPAWN/MMG/...) printed UNFILTERED, eating 1062 of the
            // 1200 budget before the launcher phase. Gate by comm
            // instead: 'appspawn' (daemon AND pre-rename children — the
            // sandbox-phase narration was decoded in Session Q already),
            // foundation and sa_main are keyword-gated; renamed children
            // print everything.
            let keyword_daemon = comm == "appspawn"
                || comm.contains("foundation")
                || comm.contains("sa_main");
            // sc152 (Session Q): for the DAEMON itself, print only the
            // SA-registration lines — the SA 501 (AppMgrService,
            // libappms loaded into the daemon) never registers with
            // samgr, and the daemon's own narration says whether the
            // registration was attempted and how it failed.
            // sc155 (Session R): BUDGET FIX — only lines that PASS the
            // print filter consume budget.
            static HI_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if HI_N.load(core::sync::atomic::Ordering::Relaxed) < 2400 {
                // iovec { void* base; size_t len }
                let base = args[1];
                if base != 0 {
                    // sc145: OH's hilog writev carries [header(bin), tag,
                    // text] as SEPARATE iovecs — walk the first 3.
                    let mut parts: alloc::vec::Vec<alloc::string::String> =
                        alloc::vec::Vec::new();
                    for vi in 0..3u64 {
                        let vp = base + vi * 16;
                        let iobase =
                            unsafe { crate::arch::uaccess::get_user(vp as *const u64) };
                        let iolen =
                            unsafe { crate::arch::uaccess::get_user((vp + 8) as *const u64) };
                        if let (Some(b), Some(l)) = (iobase, iolen) {
                            if l > 0 && l < 0x10000 {
                                let n = (l as usize).min(110);
                                let mut buf = [0u8; 110];
                                let ok = unsafe {
                                    crate::arch::uaccess::copy_from_user(
                                        buf.as_mut_ptr(),
                                        b as *const u8,
                                        n,
                                    ) == 0
                                };
                                if ok {
                                    let text: alloc::string::String = buf[..n]
                                        .iter()
                                        .map(|c| {
                                            if (0x20..=0x7f).contains(c) || *c == b'\n' {
                                                *c as char
                                            } else {
                                                '.'
                                            }
                                        })
                                        .collect();
                                    parts.push(text);
                                }
                            }
                        }
                    }
                    if !parts.is_empty() {
                        let joined = parts.join("|");
                        // sc156: the forked children's ArkCompiler GC
                        // chatter ("[gc] AddRegion/RemoveRegion/...")
                        // burned ~700 of the 1200 budget lines in sc155
                        // before the launcher phase — engine noise, drop.
                        let gc_noise = joined.contains("ArkCompiler.|[gc]");
                        // sc155: the print filter decides FIRST; only
                        // passing lines consume the budget (children
                        // always pass; daemon/foundation/sa_main need a
                        // registration keyword).
                        let daemon_regn = keyword_daemon
                            && (joined.contains("SystemAbility")
                                || joined.contains("501")
                                || joined.contains("amgr")
                                || joined.contains("SAM"));
                        if !gc_noise && (!keyword_daemon || daemon_regn) {
                            if HI_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 2400 {
                                crate::pr_warn!(
                                    "applog pid={} comm='{}' ret={} |{}",
                                    crate::process::current_pid(),
                                    comm,
                                    result,
                                    joined
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // sc136 (Session Q): the parked children's pre-park loop is
    // {clock_gettime xN, writev, connect, writev} — the connect TARGET
    // names the service the child talks to right before parking (hilog?
    // samgr? the appspawn engine?). sc136 run 1: the appspawn PARENT's
    // own connect storm ate the whole budget before the first child
    // forked, and fd 3's sockaddr is an ABSTRACT unix socket (sun_path
    // starts NUL — the empty-string parse printed nothing). Fix: skip
    // the boot's original appspawn pid (first comm='appspawn' task seen;
    // forked children carry the same comm but a different pid), and
    // hex-dump the first 16 sockaddr bytes for abstract sockets.
    if syscall_no == 203 {
        let cur_task = crate::sched::current();
        let comm = cur_task
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let is_app = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn";
        if is_app {
            static APPSPAWN_MAIN_PID: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let my_pid = crate::process::current_pid() as u32;
            let parent_pid = APPSPAWN_MAIN_PID.load(core::sync::atomic::Ordering::Relaxed);
            let skip = if comm == "appspawn" {
                if parent_pid == 0 {
                    APPSPAWN_MAIN_PID.store(my_pid, core::sync::atomic::Ordering::Relaxed);
                    true // this first one IS the parent — skip but remember it
                } else {
                    parent_pid == my_pid
                }
            } else {
                false
            };
            if !skip {
                static CONN_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if CONN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60 {
                    let mut sa = [0u8; 48];
                    use core::fmt::Write;
                    let mut desc = alloc::string::String::new();
                    if args[1] != 0 {
                        // sc141: RAW copy — strncpy_from_user stops at the
                        // first NUL, so an abstract sockaddr printed as
                        // "all zeros" when its real name started later.
                        let raw_ok = unsafe {
                            crate::arch::uaccess::copy_from_user(
                                sa.as_mut_ptr(),
                                args[1] as *const u8,
                                sa.len(),
                            ) == 0
                        };
                        if raw_ok {
                            let fam = u16::from_ne_bytes([sa[0], sa[1]]);
                            if fam == 1 {
                                let p = &sa[2..];
                                let end = p.iter().position(|c| *c == 0).unwrap_or(p.len());
                                if end == 0 {
                                    // abstract socket: the name is the
                                    // raw bytes after the leading NUL
                                    let _ = write!(desc, "abstract:");
                                    // sc140: 40 bytes + the caller's
                                    // addrlen — the name may be longer
                                    // than the old 18-byte window and
                                    // NUL bytes are legal in abstract
                                    // names (length IS the name).
                                    for b in sa.iter().take(40) {
                                        let _ = write!(desc, "{:02x}", b);
                                    }
                                } else {
                                    let _ = write!(
                                        desc,
                                        "{}",
                                        core::str::from_utf8(&p[..end]).unwrap_or("<utf8>")
                                    );
                                }
                            } else {
                                let _ = write!(desc, "fam{}", fam);
                            }
                        } else {
                            let _ = write!(desc, "<efault>");
                        }
                    } else {
                        let _ = write!(desc, "<null>");
                    }
                    crate::pr_warn!(
                        "appconn: pid {} comm='{}' fd={} -> {} addrlen={} ret={}",
                        my_pid,
                        comm,
                        args[0],
                        desc,
                        args[2],
                        result
                    );
                }
            }
        }
    }

    // sc135 (Session Q): name the prctl OPTION behind the parked app
    // children's nr167=-22 fail-tail entries (1-2 per child). The
    // unimplemented-option warning inside sys_prctl has a 32-line budget
    // that hdf_devhost retry loops eat long before the app window, and
    // EINVAL also leaves IMPLEMENTED arms (PR_SET_VMA sub-op, securebits
    // mask > 0xff, capbset > CAP_LAST_CAP, PR_CAP_AMBIENT sub-op) which
    // print nothing. Catch every prctl-EINVAL at dispatch exit with all
    // five args, scoped to the app family, budget 40.
    if syscall_no == 167 && result == -22 {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let is_app = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn";
        if is_app {
            static PRCTL_EI_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if PRCTL_EI_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                crate::pr_warn!(
                    "prctl-einval: pid {} comm='{}' option={} a1={:#x} a2={:#x} a3={:#x} a4={:#x}",
                    crate::process::current_pid(),
                    comm,
                    args[0],
                    args[1],
                    args[2],
                    args[3],
                    args[4]
                );
            }
        }
    }

    // OH bring-up: name the call behind any ENOTDIR — the hilogd
    // service-socket chmod fails with errno 20 and no in-handler trace
    // sees it, so decode it here (pathname arg for the *at family).
    // NOTE the label: this hook fires for BOTH shapes — real ENOTDIR
    // (result -20) and ENOENT (result -2) on stat-family numbers
    // [53,79,90] — and prints "missstat:" with the actual return value.
    // The old constant "ENOTDIR:" label made correct ENOENT returns on
    // missing musl-namespace paths ("/vendor/lib64/chipset-sdk-sp",
    // "/dev/watchdog" before the node existed) read as an errno-20 bug
    // across three independent investigations (b151/wdg1-wdg4; resolved
    // by shimming the nr-79 arm — zero hits while the hook printed).
    if (result == -20 || (result == -2
            && [53u64, 79, 90].contains(&(syscall_no as u64))))
        && crate::process::current_pid() >= 300
    {
        static ENOTDIR_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if ENOTDIR_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            let mut pb = [0u8; 96];
            let path = if [257u64, 48, 79, 263, 264, 53, 54].contains(&(syscall_no as u64)) && args[1] != 0 {
                crate::arch::uaccess::strncpy_from_user(
                    args[1] as *const u8, pb.len(), &mut pb)
                    .map(|s| core::str::from_utf8(s).unwrap_or("?"))
                    .unwrap_or("<efault>")
            } else { "-" };
            crate::pr_info!(
                "missstat: ret={} nr={} a0={:#x} path='{}' a2={:#x} pid={}",
                result,
                syscall_no,
                args[0],
                path,
                args[2],
                crate::process::current_pid()
            );
        }
    }

    // b26: open/openat ENOENT with its own budget — the missstat hook's
    // 40 lines are gone long before the appspawn children pivot, and the
    // sandbox-side "/dev/null" ENOENT (appspawn_common.c:350 'open
    // dev_null error: 2') needs the failing path + the task's root
    // context to diagnose the in-sandbox walk.
    // sc126 (Session P): THREE fixes — (1) the nr test checked the RAW
    // x86_64 257, but this exit-side hook sees the TRANSLATED generic
    // number (openat=56), so openat misses never printed; (2) pr_info
    // was filtered at boot loglevel=4 — now pr_warn; (3) scoped to
    // app/appspawn comms with a 120 budget — the children's pre-park
    // fail tail (APP-PARK fails:) is 6x openat ENOENT, and the paths
    // name exactly which sandbox files the runtime needs and misses.
    if result == -2
        && (syscall_no as u64 == 2 || syscall_no as u64 == 56 || syscall_no as u64 == 257)
    {
        let comm_ok = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| {
                let s = core::str::from_utf8(&c[..]).unwrap_or("").trim_end_matches('\0');
                s.contains("launcher")
                    || s.contains("settingsdata")
                    || s.contains("systemui")
                    || s.contains("dataability")
            })
            .unwrap_or(false);
        if comm_ok {
            static OPEN_ENOENT_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if OPEN_ENOENT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 200 {
                let mut pb = [0u8; 96];
                let parg = if syscall_no as u64 != 2 { args[1] } else { args[0] };
                let path = if parg != 0 {
                    crate::arch::uaccess::strncpy_from_user(
                        parg as *const u8, pb.len(), &mut pb)
                        .map(|s| core::str::from_utf8(s).unwrap_or("?"))
                        .unwrap_or("<efault>")
                } else { "-" };
                // Root context: a pivoted child resolves against its jail.
                let root = crate::sched::current()
                    .and_then(|t| unsafe { (*t).fs_arc() })
                    .map(|fs| fs.get_root())
                    .unwrap_or_default();
                let root_str = core::str::from_utf8(&root).unwrap_or("?");
                crate::pr_warn!(
                    "openmiss: path='{}' root='{}' pid={}",
                    path,
                    root_str,
                    crate::process::current_pid()
                );
            }
        }
    }

    // STEPTRACE (dfx=steptrace): one line per syscall return. Left
    // ungated this flooded the emulated UART (2.7GB per OH boot) and the
    // printing alone slowed services enough to lose races — see the
    // switch catalog note in dfx/switches.rs.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::Steptrace) {
        crate::pr_debug!("syscall: pid={}, nr={}, ret={:#x} ({})",
            crate::process::current_pid(), syscall_no, result,
            if result < 0 { "error" } else { "ok" });
    }
}
