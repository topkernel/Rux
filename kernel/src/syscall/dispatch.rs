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

    // Dispatch based on system call number
    let result: u64 = match syscall_no as u32 {
        // ==================== IO Operations ====================
        63 => io::sys_read(args),
        64 => io::sys_write(args),
        66 => io::sys_writev(args),
        23 => io::sys_dup(args),
        24 => io::sys_dup2(args),
        25 => io::sys_fcntl(args),
        29 => io::sys_ioctl(args),
        73 => io::sys_flock(args),
        59 => io::sys_pipe2(args),

        // ==================== File Operations ====================
        2 => file::sys_open(args),     // open (wrapped to openat)
        56 => file::sys_openat(args),
        57 => file::sys_close(args),
        80 => file::sys_fstat(args),
        79 => file::sys_fstatat(args), // fstatat (was incorrectly mapped to rmdir)
        61 => file::sys_getdents64(args),
        77 => file::sys_mkdir(args),
        35 => file::sys_unlinkat(args), // unlinkat (for unlink and rmdir)
        74 => file::sys_unlink(args),
        78 => file::sys_readlinkat(args),
        62 => file::sys_lseek(args),
        49 => file::sys_chdir(args),
        17 => file::sys_getcwd(args),
        166 => file::sys_umask(args),

        // ==================== Process Operations ====================
        220 => process::sys_clone(args),
        221 => process::sys_execve(args),
        93 => process::sys_exit(args),
        94 => process::sys_exit(args), // exit_group
        260 => process::sys_wait4(args),
        172 => process::sys_getpid(args),
        110 => process::sys_getppid(args),
        129 => process::sys_kill(args),
        96 => process::sys_set_tid_address(args, regs.tp),
        99 => process::sys_set_robust_list(args),
        160 => process::sys_uname(args),
        174 => process::sys_getuid(args),
        176 => process::sys_getgid(args),
        175 => process::sys_geteuid(args),
        177 => process::sys_getegid(args),
        261 => process::sys_prlimit64(args),

        // ==================== Memory Operations ====================
        214 => memory::sys_brk(args),
        222 => memory::sys_mmap(args),
        215 => memory::sys_munmap(args),
        226 => memory::sys_mprotect(args),
        227 => memory::sys_msync(args),
        216 => memory::sys_mremap(args),
        233 => memory::sys_madvise(args),
        232 => memory::sys_mincore(args),
        228 => memory::sys_mlock(args),
        229 => memory::sys_munlock(args),

        // ==================== Signal Operations ====================
        134 => signal::sys_rt_sigaction(args),
        135 => signal::sys_rt_sigprocmask(args),
        139 => signal::sys_rt_sigreturn(regs),  // rt_sigreturn needs PtRegs
        132 => signal::sys_sigaltstack(args),
        133 => signal::sys_sigpending(args),

        // ==================== Time Operations ====================
        169 => time::sys_gettimeofday(args),
        113 => time::sys_clock_gettime(args),
        101 => time::sys_nanosleep(args),
        114 => time::sys_clock_getres(args),
        115 => time::sys_clock_nanosleep(args),

        // ==================== Network Operations ====================
        198 => network::sys_socket(args),
        200 => network::sys_bind(args),
        201 => network::sys_listen(args),
        202 => network::sys_accept(args),
        203 => network::sys_connect(args),
        206 => network::sys_sendto(args),
        207 => network::sys_recvfrom(args),

        // ==================== Scheduling Operations ====================
        98 => sched::sys_futex(args),
        124 => sched::sys_sched_yield(args),
        140 => sched::sys_getpriority(args),
        141 => sched::sys_setpriority(args),

        // ==================== Select/Poll ====================
        7 => misc::sys_poll(args),
        280 => misc::sys_select(args),
        281 => misc::sys_pselect6(args),
        20 => misc::sys_epoll_create(args),
        251 => misc::sys_epoll_create1(args),
        21 => misc::sys_epoll_ctl(args),
        22 => misc::sys_epoll_wait(args),
        252 => misc::sys_epoll_pwait(args),
        290 => misc::sys_eventfd(args),
        291 => misc::sys_eventfd2(args),

        // ==================== Others ====================
        278 => misc::sys_getrandom(args),

        // ==================== Unimplemented System Calls ====================
        _ => {
            crate::println!("syscall: unknown syscall {} (args: {:#x}, {:#x}, {:#x})",
                syscall_no, args[0], args[1], args[2]);
            (-errno::ENOSYS) as u64
        }
    };

    syscall_set_return_value(regs, result);
}
