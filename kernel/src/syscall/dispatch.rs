//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System call dispatch module
//!
//! This module handles system call dispatch and common processing

use crate::arch::riscv64::pt_regs::PtRegs;
use super::*;

/// System call argument array type
pub type SyscallArgs = [u64; 6];

/// Get system call number from PtRegs
#[inline]
fn syscall_get_nr(regs: &PtRegs) -> u64 {
    regs.a7
}

/// Get system call arguments from PtRegs
#[inline]
fn syscall_get_arguments(regs: &PtRegs) -> SyscallArgs {
    [regs.orig_a0, regs.a1, regs.a2, regs.a3, regs.a4, regs.a5]
}

/// Set system call return value
#[inline]
fn syscall_set_return_value(regs: &mut PtRegs, value: u64) {
    regs.a0 = value;
}

/// System call entry function
///
/// Called by trap.rs, dispatches to specific system call handlers
pub extern "C" fn syscall_handler(regs: &mut PtRegs) {
    let syscall_no = syscall_get_nr(regs);
    let args = syscall_get_arguments(regs);

    crate::pr_debug!("syscall: pid={}, nr={}, args=[{:#x}, {:#x}, {:#x}]",
        crate::process::current_pid(), syscall_no, args[0], args[1], args[2]);

    // Dispatch based on system call number (sorted by number)
    let result: u64 = match syscall_no as u32 {
        // ==================== File Operations ====================
        17 => file::sys_getcwd(args),
        19 => misc::sys_eventfd2(args),        // eventfd2
        20 => misc::sys_epoll_create(args),    // epoll_create1
        21 => misc::sys_epoll_ctl(args),       // epoll_ctl
        22 => misc::sys_epoll_pwait(args),     // epoll_pwait
        26 => misc::sys_inotify_init1(args), // inotify_init1
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
        43 => file::sys_statfs(args),          // statfs
        44 => file::sys_fstatfs(args),         // fstatfs
        45 => file::sys_truncate(args),        // truncate
        46 => file::sys_ftruncate(args),       // ftruncate
        47 => file::sys_fallocate(args),       // fallocate
        48 => file::sys_faccessat(args),       // faccessat
        49 => file::sys_chdir(args),           // chdir
        50 => file::sys_fchdir(args),          // fchdir
        52 => file::sys_fchmod(args),          // fchmod
        53 => file::sys_fchmodat(args),        // fchmodat
        54 => file::sys_fchownat(args),        // fchownat
        56 => file::sys_openat(args),          // openat
        57 => file::sys_close(args),           // close
        60 => process::sys_quotactl(args),     // quotactl
        59 => io::sys_pipe2(args),             // pipe2
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
        75 => io::sys_vmsplice(args),         // vmsplice
        76 => io::sys_splice(args),            // splice
        77 => io::sys_tee(args),               // tee
        74 => signal::sys_signalfd4(args),     // signalfd4
        78 => file::sys_readlinkat(args),      // readlinkat
        79 => file::sys_fstatat(args),         // fstatat
        80 => file::sys_fstat(args),           // fstat
        81 => file::sys_sync(args),            // sync
        82 => file::sys_fsync(args),           // fsync
        83 => file::sys_fdatasync(args),       // fdatasync
        85 => misc::sys_timerfd_create(args),  // timerfd_create
        86 => misc::sys_timerfd_settime(args), // timerfd_settime
        87 => misc::sys_timerfd_gettime(args), // timerfd_gettime
        88 => file::sys_futimesat(args),       // utimensat

        // ==================== Process Operations ====================
        97 => process::sys_unshare(args),      // unshare
        93 => process::sys_exit(args),         // exit
        94 => process::sys_exit(args),         // exit_group
        95 => process::sys_waitid(args),       // waitid
        96 => process::sys_set_tid_address(args, regs.tp),
        99 => process::sys_set_robust_list(args),
        102 => time::sys_getitimer(args),      // getitimer
        103 => time::sys_setitimer(args),      // setitimer
        112 => time::sys_clock_settime(args),  // clock_settime
        113 => time::sys_clock_gettime(args),  // clock_gettime
        114 => time::sys_clock_getres(args),   // clock_getres
        115 => time::sys_clock_nanosleep(args),// clock_nanosleep
        116 => crate::printk::sys_syslog(args),  // syslog
        117 => process::sys_ptrace(args),      // ptrace
        118 => sched::sys_sched_setparam(args),// sched_setparam
        119 => sched::sys_sched_setscheduler(args), // sched_setscheduler
        120 => sched::sys_sched_getscheduler(args), // sched_getscheduler
        122 => sched::sys_sched_getparam(args),// sched_getparam
        123 => sched::sys_sched_setaffinity(args), // sched_setaffinity
        124 => sched::sys_sched_yield(args),   // sched_yield
        125 => sched::sys_sched_get_priority_max(args), // sched_get_priority_max
        126 => sched::sys_sched_get_priority_min(args), // sched_get_priority_min
        127 => sched::sys_sched_rr_get_interval(args), // sched_rr_get_interval
        128 => signal::sys_restart_syscall(args), // restart_syscall
        129 => process::sys_kill(args),        // kill
        130 => signal::sys_tkill(args),        // tkill
        131 => process::sys_tgkill(args),      // tgkill
        132 => signal::sys_sigaltstack(args),  // sigaltstack
        133 => signal::sys_sigpending(args),   // rt_sigpending
        134 => signal::sys_rt_sigaction(args), // rt_sigaction
        135 => signal::sys_rt_sigprocmask(args),// rt_sigprocmask
        134 => signal::sys_rt_sigsuspend(args), // rt_sigsuspend
        137 => process::sys_rt_sigtimedwait(args), // rt_sigtimedwait
        138 => process::sys_rt_sigqueueinfo(args), // rt_sigqueueinfo
        139 => signal::sys_rt_sigreturn(regs), // rt_sigreturn
        140 => sched::sys_getpriority(args),   // getpriority
        141 => sched::sys_setpriority(args),   // setpriority
        142 => process::sys_reboot(args),      // reboot
        143 => process::sys_setregid(args),    // setregid
        144 => process::sys_setgid(args),      // setgid
        145 => process::sys_setreuid(args),    // setreuid
        146 => process::sys_setuid(args),      // setuid
        147 => process::sys_setresuid(args),   // setresuid
        148 => process::sys_getresuid(args),   // getresuid
        149 => process::sys_setresgid(args),   // setresgid
        150 => process::sys_getresgid(args),   // getresgid
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
        172 => process::sys_getpid(args),      // getpid
        173 => process::sys_getppid(args),     // getppid
        174 => process::sys_getuid(args),      // getuid
        175 => process::sys_geteuid(args),     // geteuid
        176 => process::sys_getgid(args),      // getgid
        177 => process::sys_getegid(args),     // getegid
        178 => process::sys_gettid(args),      // gettid

        // ==================== IPC Operations ====================
        194 => process::sys_shmget(args),      // shmget
        195 => process::sys_shmctl(args),      // shmctl
        196 => process::sys_shmat(args),       // shmat
        197 => process::sys_shmdt(args),       // shmdt

        // ==================== Network Operations ====================
        198 => network::sys_socket(args),      // socket
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
        242 => network::sys_accept4(args),     // accept4

        // ==================== Memory Operations ====================
        214 => memory::sys_brk(args),          // brk
        215 => memory::sys_munmap(args),       // munmap
        216 => memory::sys_mremap(args),       // mremap
        222 => memory::sys_mmap(args),         // mmap
        226 => memory::sys_mprotect(args),     // mprotect
        227 => memory::sys_msync(args),        // msync
        228 => memory::sys_mlock(args),        // mlock
        229 => memory::sys_munlock(args),      // munlock
        232 => memory::sys_mincore(args),      // mincore
        233 => memory::sys_madvise(args),      // madvise

        // ==================== Process Operations (cont.) ====================
        220 => process::sys_clone(args),       // clone
        221 => process::sys_execve(args),      // execve
        268 => process::sys_setns(args),       // setns

        // ==================== RISC-V Specific ====================
        258 => process::sys_riscv_hwprobe(args), // riscv_hwprobe
        259 => process::sys_riscv_flush_icache(args), // riscv_flush_icache
        267 => process::sys_syncfs(args),      // syncfs

        // ==================== Select/Poll/Epoll ====================

        // ==================== Process Lifecycle ====================
        260 => process::sys_wait4(args),       // wait4
        261 => process::sys_prlimit64(args),   // prlimit64

        // ==================== Select/Poll/Epoll ====================
        276 => file::sys_renameat2(args),      // renameat2
        281 => process::sys_execveat(args),    // execveat

        // ==================== Scheduler Extended ====================
        351 => sched::sys_sched_getattr(args), // sched_getattr
        352 => sched::sys_sched_setattr(args), // sched_setattr

        // ==================== Others ====================
        279 => process::sys_memfd_create(args), // memfd_create
        278 => misc::sys_getrandom(args),      // getrandom
        290 => misc::sys_eventfd(args),        // eventfd
        291 => file::sys_statx(args),          // statx
        437 => file::sys_openat2(args),        // openat2

        // ==================== Unimplemented System Calls ====================
        _ => {
            crate::println!("syscall: unknown syscall {} (args: {:#x}, {:#x}, {:#x})",
                syscall_no, args[0], args[1], args[2]);
            (-errno::ENOSYS) as u64
        }
    };

    syscall_set_return_value(regs, result);

    crate::pr_debug!("syscall: pid={}, nr={}, ret={:#x} ({})",
        crate::process::current_pid(), syscall_no, result,
        if (result as i64) < 0 { "error" } else { "ok" });
}
