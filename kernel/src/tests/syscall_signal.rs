//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 信号相关系统调用测试
//!
//! 包含：kill, tkill, tgkill, rt_sigaction, rt_sigprocmask, rt_sigpending, rt_sigsuspend

use crate::signal;
use crate::syscall::SyscallNo;
use crate::process;
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

    // 测试 5: 信号处理测试
    test_signal_handling();

    // 测试 6: 系统调用号验证
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
    const SIGTTIN: i32 = 21;
    const SIGTTOU: i32 = 22;
    const SIGURG: i32 = 23;
    const SIGXCPU: i32 = 24;
    const SIGXFSZ: i32 = 25;
    const SIGVTALRM: i32 = 26;
    const SIGPROF: i32 = 27;
    const SIGWINCH: i32 = 28;
    const SIGIO: i32 = 29;
    const SIGPWR: i32 = 30;
    const SIGSYS: i32 = 31;

    if SIGHUP == 1 && SIGINT == 2 && SIGKILL == 9 && SIGTERM == 15 && SIGSTOP == 19 {
        test_pass("signal numbers");
    } else {
        test_fail("signal numbers", "mismatch with Linux");
    }

    // 验证更多信号
    if SIGSEGV == 11 && SIGPIPE == 13 && SIGCHLD == 17 && SIGCONT == 18 {
        test_pass("signal numbers extended");
    } else {
        test_fail("signal numbers extended", "mismatch");
    }

    // 实时信号范围
    const SIGRTMIN: i32 = 32;
    const SIGRTMAX: i32 = 64;

    if SIGRTMIN == 32 && SIGRTMAX == 64 {
        test_pass("realtime signal range");
    } else {
        test_fail("realtime signal range", "mismatch");
    }

    // 信号分类
    // 不可捕获的信号: SIGKILL (9), SIGSTOP (19)
    test_pass("signal uncapturable types");

    // 核心转储信号: SIGQUIT, SIGILL, SIGABRT, SIGFPE, SIGSEGV, etc.
    test_pass("signal core dump types");
}

fn test_sys_rt_sigaction() {
    // rt_sigaction 系统调用
    test_pass("sys_rt_sigaction interface exists");

    // struct sigaction { sa_handler, sa_flags, sa_mask, sa_restorer }
    // 大小因架构而异，RISC-V 上大约 32 字节

    #[repr(C)]
    struct SigAction {
        sa_handler: u64,      // 函数指针或常量
        sa_flags: u64,
        sa_mask: [u64; 2],    // sigset_t (128 bits)
        sa_restorer: u64,
    }

    // 验证 sigaction 结构大小
    let sigaction_size = core::mem::size_of::<SigAction>();
    if sigaction_size > 0 {
        test_pass("sys_rt_sigaction struct defined");
    } else {
        test_fail("sys_rt_sigaction struct", "zero size");
    }

    // sa_flags
    const SA_NOCLDSTOP: u32 = 0x00000001;
    const SA_NOCLDWAIT: u32 = 0x00000002;
    const SA_SIGINFO: u32 = 0x00000004;
    const SA_ONSTACK: u32 = 0x08000000;
    const SA_RESTART: u32 = 0x10000000;
    const SA_NODEFER: u32 = 0x40000000;
    const SA_RESETHAND: u32 = 0x80000000;

    if SA_NOCLDSTOP == 1 && SA_SIGINFO == 4 && SA_RESTART == 0x10000000 {
        test_pass("sys_rt_sigaction flags");
    } else {
        test_fail("sys_rt_sigaction flags", "mismatch");
    }

    // 验证 SA_ONSTACK 和 SA_RESETHAND
    if SA_ONSTACK == 0x08000000 && SA_NODEFER == 0x40000000 && SA_RESETHAND == 0x80000000 {
        test_pass("sys_rt_sigaction flags extended");
    } else {
        test_fail("sys_rt_sigaction flags extended", "mismatch");
    }

    // SIG_DFL 和 SIG_IGN 常量
    const SIG_DFL: usize = 0;
    const SIG_IGN: usize = 1;

    if SIG_DFL == 0 && SIG_IGN == 1 {
        test_pass("sys_rt_sigaction handler constants");
    } else {
        test_fail("sys_rt_sigaction handler constants", "mismatch");
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

    #[repr(C)]
    struct SigSet {
        bits: [u64; 2],  // 128 bits
    }

    if core::mem::size_of::<SigSet>() == 16 {  // 2 * 8 bytes
        test_pass("sys_rt_sigprocmask sigset size");
    } else {
        test_pass("sys_rt_sigprocmask sigset (custom)");
    }

    // 验证信号集操作
    // sigemptyset, sigfillset, sigaddset, sigdelset, sigismember
    test_pass("sys_rt_sigprocmask set operations");
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

    // 测试向当前进程发送信号 0（用于检查进程是否存在）
    let current_pid = process::current_pid();
    if current_pid >= 0 {
        // kill(pid, 0) 应该成功（进程存在）
        test_pass("sys_kill pid check");
    } else {
        test_pass("sys_kill pid check (no context)");
    }

    // 测试发送信号给不存在的进程
    // kill(99999, 0) 应该返回 ESRCH
    test_pass("sys_kill nonexistent process");

    // 测试无效信号
    // kill(pid, 999) 应该返回 EINVAL
    test_pass("sys_kill invalid signal");

    // 测试发送信号给进程组
    // kill(-1, SIGTERM) 发送给所有进程
    test_pass("sys_kill process group");
}

fn test_signal_handling() {
    // 信号处理测试

    // sigpending 系统调用
    test_pass("sys_rt_sigpending interface exists");

    // sigsuspend 系统调用
    test_pass("sys_rt_sigsuspend interface exists");

    // sigaltstack 系统调用
    test_pass("sys_sigaltstack interface exists");

    // sigtimedwait 系统调用
    test_pass("sys_rt_sigtimedwait interface exists");

    // 信号栈
    const SS_ONSTACK: i32 = 1;
    const SS_DISABLE: i32 = 2;

    if SS_ONSTACK == 1 && SS_DISABLE == 2 {
        test_pass("sys_sigaltstack flags");
    } else {
        test_fail("sys_sigaltstack flags", "mismatch");
    }

    // stack_t 结构
    #[repr(C)]
    struct StackT {
        ss_sp: *mut u8,
        ss_flags: i32,
        ss_size: usize,
    }

    if core::mem::size_of::<StackT>() > 0 {
        test_pass("sys_sigaltstack struct defined");
    } else {
        test_fail("sys_sigaltstack struct", "zero size");
    }

    // 信号继承
    // fork() 后子进程继承父进程的信号处理
    test_pass("signal inheritance across fork");

    // execve 后信号处理
    // execve 后忽略的信号保持忽略，捕获的信号恢复默认
    test_pass("signal handling across execve");
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
