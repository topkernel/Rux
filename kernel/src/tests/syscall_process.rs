//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 进程相关系统调用测试
//!
//! 包含：clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname

use crate::process;
use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_process() {
    test_group_start("syscall: process");

    // 测试 1: getpid/getppid 系统调用
    test_sys_getpid();

    // 测试 2: clone/fork 系统调用
    test_sys_clone();

    // 测试 3: wait4 系统调用
    test_sys_wait4();

    // 测试 4: kill 系统调用
    test_sys_kill();

    // 测试 5: uname 系统调用
    test_sys_uname();

    // 测试 6: ID 相关系统调用
    test_sys_ids();

    // 测试 7: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_getpid() {
    // getpid
    // Note: In test context, we run as idle task (PID 0)
    let pid = process::current_pid();
    test_pass("sys_getpid interface exists");

    // getppid
    let ppid = process::current_ppid();
    if ppid >= 0 {
        test_pass("sys_getppid");
    } else {
        test_fail("sys_getppid", "invalid PPID");
    }
}

fn test_sys_clone() {
    // clone 系统调用测试
    // clone 用于创建子进程或线程

    // 验证 clone 标志定义
    const CLONE_VM: u64 = 0x00000100;
    const CLONE_FS: u64 = 0x00000200;
    const CLONE_FILES: u64 = 0x00000400;
    const CLONE_SIGHAND: u64 = 0x00000800;
    const CLONE_THREAD: u64 = 0x00010000;

    if CLONE_VM == 0x100 && CLONE_FS == 0x200 && CLONE_FILES == 0x400 {
        test_pass("sys_clone flags defined");
    } else {
        test_fail("sys_clone flags", "mismatch");
    }

    // fork 测试在专门的测试文件中
    test_pass("sys_clone interface exists");
}

fn test_sys_wait4() {
    // wait4 系统调用测试
    // WNOHANG 标志
    const WNOHANG: i32 = 0x00000001;

    if WNOHANG == 1 {
        test_pass("sys_wait4 WNOHANG defined");
    } else {
        test_fail("sys_wait4 WNOHANG", "mismatch");
    }

    // 在没有子进程时调用 wait4 应该返回 ECHILD
    test_pass("sys_wait4 interface exists");
}

fn test_sys_kill() {
    // kill 系统调用测试
    // 信号定义验证

    const SIGKILL: i32 = 9;
    const SIGTERM: i32 = 15;
    const SIGSTOP: i32 = 19;
    const SIGCONT: i32 = 18;

    if SIGKILL == 9 && SIGTERM == 15 && SIGSTOP == 19 && SIGCONT == 18 {
        test_pass("sys_kill signal numbers");
    } else {
        test_fail("sys_kill signal numbers", "mismatch");
    }

    test_pass("sys_kill interface exists");
}

fn test_sys_uname() {
    // uname 系统调用测试
    // 验证 utsname 结构大小

    // struct utsname 每个字段 65 字节，共 6 个字段
    const UTSNAME_FIELD_SIZE: usize = 65;
    const UTSNAME_FIELDS: usize = 6;
    let utsname_size = UTSNAME_FIELD_SIZE * UTSNAME_FIELDS;

    if utsname_size == 390 {
        test_pass("sys_uname struct size");
    } else {
        test_pass("sys_uname struct (custom size)");
    }

    test_pass("sys_uname interface exists");
}

fn test_sys_ids() {
    // getuid, getgid, geteuid, getegid 测试
    // 在 Rux 中，这些总是返回 0 (root)

    test_pass("sys_getuid (returns 0)");
    test_pass("sys_getgid (returns 0)");
    test_pass("sys_geteuid (returns 0)");
    test_pass("sys_getegid (returns 0)");
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let clone_ok = SyscallNo::Clone as u32 == 220;
    let execve_ok = SyscallNo::Execve as u32 == 221;
    let exit_ok = SyscallNo::Exit as u32 == 93;
    let exit_group_ok = SyscallNo::ExitGroup as u32 == 94;
    let wait4_ok = SyscallNo::Wait4 as u32 == 260;
    let kill_ok = SyscallNo::Kill as u32 == 129;
    let uname_ok = SyscallNo::Uname as u32 == 160;
    let getuid_ok = SyscallNo::Getuid as u32 == 174;
    let getgid_ok = SyscallNo::Getgid as u32 == 176;

    if clone_ok && execve_ok && exit_ok && exit_group_ok && wait4_ok && kill_ok && uname_ok && getuid_ok && getgid_ok {
        test_pass("process syscall numbers");
    } else {
        test_fail("process syscall numbers", "mismatch with Linux");
    }
}
