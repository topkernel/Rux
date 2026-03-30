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
        23 => io::sys_dup(args),               // dup
        24 => io::sys_dup3(args),              // dup3
        25 => io::sys_fcntl(args),             // fcntl
        29 => io::sys_ioctl(args),             // ioctl
        32 => io::sys_flock(args),             // flock
        34 => file::sys_mkdirat(args),         // mkdirat
        35 => file::sys_unlinkat(args),        // unlinkat
        36 => file::sys_symlinkat(args),       // symlinkat
        37 => file::sys_linkat(args),          // linkat
        38 => file::sys_renameat(args),        // renameat
        43 => file::sys_statfs(args),          // statfs
        44 => file::sys_fstatfs(args),         // fstatfs
        45 => file::sys_truncate(args),        // truncate
        46 => file::sys_ftruncate(args),       // ftruncate
        48 => file::sys_faccessat(args),       // faccessat
        49 => file::sys_chdir(args),           // chdir
        50 => file::sys_fchdir(args),          // fchdir
        53 => file::sys_fchmodat(args),        // fchmodat
        54 => file::sys_fchownat(args),        // fchownat
        56 => file::sys_openat(args),          // openat
        57 => file::sys_close(args),           // close
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
        39 => file::sys_umount(args),          // umount
        40 => file::sys_mount(args),           // mount
        78 => file::sys_readlinkat(args),      // readlinkat
        79 => file::sys_fstatat(args),         // fstatat
        80 => file::sys_fstat(args),           // fstat
        88 => file::sys_futimesat(args),       // utimensat

        // ==================== Process Operations ====================
        96 => process::sys_set_tid_address(args, regs.tp),
        98 => sched::sys_futex(args),          // futex
        99 => process::sys_set_robust_list(args),
        101 => time::sys_nanosleep(args),      // nanosleep
        173 => process::sys_getppid(args),     // getppid
        113 => time::sys_clock_gettime(args),  // clock_gettime
        114 => time::sys_clock_getres(args),   // clock_getres
        115 => time::sys_clock_nanosleep(args),// clock_nanosleep
        116 => crate::printk::sys_syslog(args),  // syslog
        124 => sched::sys_sched_yield(args),   // sched_yield
        129 => process::sys_kill(args),        // kill
        132 => signal::sys_sigaltstack(args),  // sigaltstack
        133 => signal::sys_sigpending(args),   // rt_sigpending
        134 => signal::sys_rt_sigaction(args), // rt_sigaction
        135 => signal::sys_rt_sigprocmask(args),// rt_sigprocmask
        130 => signal::sys_tkill(args),        // tkill
        139 => signal::sys_rt_sigreturn(regs), // rt_sigreturn
        140 => sched::sys_getpriority(args),   // getpriority
        141 => sched::sys_setpriority(args),   // setpriority
        160 => process::sys_uname(args),       // newuname
        166 => file::sys_umask(args),          // umask
        169 => time::sys_gettimeofday(args),   // gettimeofday
        172 => process::sys_getpid(args),      // getpid
        178 => process::sys_gettid(args),      // gettid
        174 => process::sys_getuid(args),      // getuid
        175 => process::sys_geteuid(args),     // geteuid
        176 => process::sys_getgid(args),      // getgid
        177 => process::sys_getegid(args),     // getegid
        144 => process::sys_setgid(args),      // setgid
        145 => process::sys_setreuid(args),    // setreuid
        146 => process::sys_setuid(args),      // setuid
        147 => process::sys_setregid(args),    // setregid
        158 => process::sys_getgroups(args),   // getgroups
        159 => process::sys_setgroups(args),   // setgroups
        154 => process::sys_setpgid(args),     // setpgid
        155 => process::sys_getpgid(args),     // getpgid
        156 => process::sys_getsid(args),      // getsid
        157 => process::sys_setsid(args),      // setsid

        // ==================== Network Operations ====================
        198 => network::sys_socket(args),      // socket
        200 => network::sys_bind(args),        // bind
        201 => network::sys_listen(args),      // listen
        202 => network::sys_accept(args),      // accept
        203 => network::sys_connect(args),     // connect
        206 => network::sys_sendto(args),      // sendto
        207 => network::sys_recvfrom(args),    // recvfrom

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

        // ==================== Process Lifecycle ====================
        93 => process::sys_exit(args),         // exit
        94 => process::sys_exit(args),         // exit_group
        260 => process::sys_wait4(args),       // wait4
        261 => process::sys_prlimit64(args),   // prlimit64

        // ==================== Select/Poll/Epoll ====================
        251 => misc::sys_epoll_create1(args),  // epoll_create1
        252 => misc::sys_epoll_pwait(args),    // epoll_pwait
        276 => file::sys_renameat(args),       // renameat2 (flags ignored)

        // ==================== Others ====================
        278 => misc::sys_getrandom(args),      // getrandom
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
