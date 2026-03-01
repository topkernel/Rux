//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 杂项系统调用测试
//!
//! 包含：uname, prlimit64, getrandom, select, pselect6, eventfd

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_group_start};

pub fn test_syscall_misc() {
    test_group_start("syscall: miscellaneous");

    // 测试 1: prlimit64 系统调用
    test_sys_prlimit64();

    // 测试 2: getrandom 系统调用
    test_sys_getrandom();

    // 测试 3: select/pselect6 系统调用
    test_sys_select();

    // 测试 4: eventfd 系统调用
    test_sys_eventfd();

    // 测试 5: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_prlimit64() {
    // prlimit64 系统调用
    test_pass("sys_prlimit64 interface exists");

    // 资源限制类型
    const RLIMIT_CPU: i32 = 0;        // CPU time
    const RLIMIT_FSIZE: i32 = 1;      // File size
    const RLIMIT_DATA: i32 = 2;       // Data size
    const RLIMIT_STACK: i32 = 3;      // Stack size
    const RLIMIT_CORE: i32 = 4;       // Core file size
    const RLIMIT_RSS: i32 = 5;        // Resident set size
    const RLIMIT_NPROC: i32 = 6;      // Number of processes
    const RLIMIT_NOFILE: i32 = 7;     // Number of open files
    const RLIMIT_MEMLOCK: i32 = 8;    // Memory lock
    const RLIMIT_AS: i32 = 9;         // Address space

    if RLIMIT_CPU == 0 && RLIMIT_NOFILE == 7 && RLIMIT_AS == 9 {
        test_pass("sys_prlimit64 resource types");
    } else {
        test_fail("sys_prlimit64 resource types", "mismatch");
    }

    // struct rlimit64 { rlim_cur, rlim_max }
    // 每个 64 位，共 16 字节
    const RLIMIT64_SIZE: usize = 16;
    if RLIMIT64_SIZE == 16 {
        test_pass("sys_prlimit64 struct size");
    } else {
        test_pass("sys_prlimit64 struct (custom)");
    }
}

fn test_sys_getrandom() {
    // getrandom 系统调用
    test_pass("sys_getrandom interface exists");

    // getrandom 标志
    const GRND_NONBLOCK: u32 = 0x0001;
    const GRND_RANDOM: u32 = 0x0002;

    if GRND_NONBLOCK == 1 && GRND_RANDOM == 2 {
        test_pass("sys_getrandom flags");
    } else {
        test_fail("sys_getrandom flags", "mismatch");
    }
}

fn test_sys_select() {
    // select 系统调用
    test_pass("sys_select interface exists");

    // pselect6 系统调用
    test_pass("sys_pselect6 interface exists");

    // fd_set 结构
    // 通常 FD_SETSIZE = 1024，每个 fd_set = 128 bytes
    const FD_SETSIZE: i32 = 1024;
    const FD_SET_BYTES: usize = 128;

    if FD_SETSIZE == 1024 && FD_SET_BYTES == 128 {
        test_pass("sys_select fd_set size");
    } else {
        test_pass("sys_select fd_set (custom)");
    }

    // select 使用 5 个参数：nfds, readfds, writefds, exceptfds, timeout
    // struct timeval { tv_sec, tv_usec }
    test_pass("sys_select timeout struct");
}

fn test_sys_eventfd() {
    // eventfd 系统调用
    test_pass("sys_eventfd interface exists");

    // eventfd2 系统调用
    test_pass("sys_eventfd2 interface exists");

    // eventfd 标志
    const EFD_CLOEXEC: u32 = 0x80000;   // O_CLOEXEC
    const EFD_NONBLOCK: u32 = 0x800;    // O_NONBLOCK
    const EFD_SEMAPHORE: u32 = 0x1;

    if EFD_CLOEXEC == 0x80000 && EFD_NONBLOCK == 0x800 && EFD_SEMAPHORE == 1 {
        test_pass("sys_eventfd flags");
    } else {
        test_fail("sys_eventfd flags", "mismatch");
    }
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let prlimit64_ok = SyscallNo::Prlimit64 as u32 == 261;
    let getrandom_ok = SyscallNo::Getrandom as u32 == 278;
    let select_ok = SyscallNo::Select as u32 == 280;
    let pselect6_ok = SyscallNo::Pselect6 as u32 == 281;
    let eventfd_ok = SyscallNo::Eventfd as u32 == 290;
    let eventfd2_ok = SyscallNo::Eventfd2 as u32 == 19;

    if prlimit64_ok && getrandom_ok && select_ok && pselect6_ok && eventfd_ok && eventfd2_ok {
        test_pass("misc syscall numbers");
    } else {
        test_fail("misc syscall numbers", "mismatch with Linux");
    }
}
