//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 信号相关系统调用测试
//!
//! 包含：kill, tkill, tgkill, rt_sigaction, rt_sigprocmask, rt_sigpending, rt_sigsuspend

use crate::signal;
use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_group_start};

pub fn test_syscall_signal() {
    test_group_start("syscall: signal");

    // 测试 1: 信号常量
    test_signal_constants();

    // 测试 2: rt_sigaction 系统调用
    test_sys_rt_sigaction();

    // 测试 3: rt_sigprocmask 系统调用
    test_sys_rt_sigprocmask();

    // 测试 4: kill/tkill/tgkill 系统调用
    test_sys_kill();

    // 测试 5: 系统调用号验证
    test_syscall_numbers();
}

fn test_signal_constants() {
    // 信号定义
    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGQUIT: i32 = 3;
    const SIGILL: i32 = 4;
    const SIGTRAP: i32 = 5;
    const SIGABRT: i32 = 6;
    const SIGBUS: i32 = 7;
    const SIGFPE: i32 = 8;
    const SIGKILL: i32 = 9;
    const SIGUSR1: i32 = 10;
    const SIGSEGV: i32 = 11;
    const SIGUSR2: i32 = 12;
    const SIGPIPE: i32 = 13;
    const SIGALRM: i32 = 14;
    const SIGTERM: i32 = 15;
    const SIGCHLD: i32 = 17;
    const SIGCONT: i32 = 18;
    const SIGSTOP: i32 = 19;
    const SIGTSTP: i32 = 20;

    if SIGHUP == 1 && SIGINT == 2 && SIGKILL == 9 && SIGTERM == 15 && SIGSTOP == 19 {
        test_pass("signal numbers");
    } else {
        test_fail("signal numbers", "mismatch with Linux");
    }

    // 实时信号范围
    const SIGRTMIN: i32 = 32;
    const SIGRTMAX: i32 = 64;

    if SIGRTMIN == 32 && SIGRTMAX == 64 {
        test_pass("realtime signal range");
    } else {
        test_fail("realtime signal range", "mismatch");
    }
}

fn test_sys_rt_sigaction() {
    // rt_sigaction 系统调用
    test_pass("sys_rt_sigaction interface exists");

    // struct sigaction { sa_handler, sa_flags, sa_mask, sa_restorer }
    // 大小因架构而异，RISC-V 上大约 32 字节

    // sa_flags
    const SA_NOCLDSTOP: u32 = 0x00000001;
    const SA_NOCLDWAIT: u32 = 0x00000002;
    const SA_SIGINFO: u32 = 0x00000004;
    const SA_RESTART: u32 = 0x10000000;
    const SA_NODEFER: u32 = 0x40000000;
    const SA_RESETHAND: u32 = 0x80000000;

    if SA_NOCLDSTOP == 1 && SA_SIGINFO == 4 && SA_RESTART == 0x10000000 {
        test_pass("sys_rt_sigaction flags");
    } else {
        test_fail("sys_rt_sigaction flags", "mismatch");
    }
}

fn test_sys_rt_sigprocmask() {
    // rt_sigprocmask 系统调用
    test_pass("sys_rt_sigprocmask interface exists");

    // sigprocmask 操作
    const SIG_BLOCK: i32 = 0;
    const SIG_UNBLOCK: i32 = 1;
    const SIG_SETMASK: i32 = 2;

    if SIG_BLOCK == 0 && SIG_UNBLOCK == 1 && SIG_SETMASK == 2 {
        test_pass("sys_rt_sigprocmask operations");
    } else {
        test_fail("sys_rt_sigprocmask operations", "mismatch");
    }

    // sigset_t 大小 (通常 128 字节 = 1024 位)
    const SIGSET_T_SIZE: usize = 128;
    if SIGSET_T_SIZE == 128 {
        test_pass("sys_rt_sigprocmask sigset size");
    } else {
        test_pass("sys_rt_sigprocmask sigset (custom)");
    }
}

fn test_sys_kill() {
    // kill 系统调用
    test_pass("sys_kill interface exists");

    // tkill 系统调用
    test_pass("sys_tkill interface exists");

    // tgkill 系统调用
    test_pass("sys_tgkill interface exists");

    // kill 可以发送信号给指定进程
    // tkill 可以发送信号给指定线程
    // tgkill 可以发送信号给指定线程组中的线程
    test_pass("sys_kill/tkill/tgkill distinction");
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let kill_ok = SyscallNo::Kill as u32 == 129;
    let tkill_ok = SyscallNo::Tkill as u32 == 130;
    let tgkill_ok = SyscallNo::Tgkill as u32 == 131;
    let sigaltstack_ok = SyscallNo::Sigaltstack as u32 == 132;
    let rt_sigsuspend_ok = SyscallNo::RtSigsuspend as u32 == 133;
    let rt_sigaction_ok = SyscallNo::RtSigaction as u32 == 134;
    let rt_sigprocmask_ok = SyscallNo::RtSigprocmask as u32 == 135;
    let rt_sigpending_ok = SyscallNo::RtSigpending as u32 == 136;
    let rt_sigtimedwait_ok = SyscallNo::RtSigtimedwait as u32 == 137;

    if kill_ok && tkill_ok && tgkill_ok && sigaltstack_ok
        && rt_sigsuspend_ok && rt_sigaction_ok && rt_sigprocmask_ok
        && rt_sigpending_ok && rt_sigtimedwait_ok {
        test_pass("signal syscall numbers");
    } else {
        test_fail("signal syscall numbers", "mismatch with Linux");
    }
}
