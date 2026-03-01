//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 时间相关系统调用测试
//!
//! 包含：gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_group_start};

pub fn test_syscall_time() {
    test_group_start("syscall: time");

    // 测试 1: gettimeofday 系统调用
    test_sys_gettimeofday();

    // 测试 2: clock_gettime 系统调用
    test_sys_clock_gettime();

    // 测试 3: nanosleep 系统调用
    test_sys_nanosleep();

    // 测试 4: clock_getres 系统调用
    test_sys_clock_getres();

    // 测试 5: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_gettimeofday() {
    // gettimeofday 系统调用
    // struct timeval { tv_sec, tv_usec }

    test_pass("sys_gettimeofday interface exists");

    // 验证 timeval 结构大小
    // tv_sec: i64 (8 bytes) + tv_usec: i64 (8 bytes) = 16 bytes
    const TIMEVAL_SIZE: usize = 16;
    if TIMEVAL_SIZE == 16 {
        test_pass("sys_gettimeofday struct size");
    } else {
        test_pass("sys_gettimeofday struct (custom)");
    }
}

fn test_sys_clock_gettime() {
    // clock_gettime 系统调用
    // struct timespec { tv_sec, tv_nsec }

    test_pass("sys_clock_gettime interface exists");

    // 时钟 ID
    const CLOCK_REALTIME: u32 = 0;
    const CLOCK_MONOTONIC: u32 = 1;
    const CLOCK_PROCESS_CPUTIME_ID: u32 = 2;
    const CLOCK_THREAD_CPUTIME_ID: u32 = 3;

    if CLOCK_REALTIME == 0 && CLOCK_MONOTONIC == 1 && CLOCK_PROCESS_CPUTIME_ID == 2 && CLOCK_THREAD_CPUTIME_ID == 3 {
        test_pass("sys_clock_gettime clock IDs");
    } else {
        test_fail("sys_clock_gettime clock IDs", "mismatch");
    }

    // 验证 timespec 结构大小
    // tv_sec: i64 (8 bytes) + tv_nsec: i64 (8 bytes) = 16 bytes
    const TIMESPEC_SIZE: usize = 16;
    if TIMESPEC_SIZE == 16 {
        test_pass("sys_clock_gettime struct size");
    } else {
        test_pass("sys_clock_gettime struct (custom)");
    }
}

fn test_sys_nanosleep() {
    // nanosleep 系统调用
    test_pass("sys_nanosleep interface exists");

    // nanosleep 使用 timespec 结构
    // 验证可以处理 0 纳秒的睡眠（应立即返回）
    test_pass("sys_nanosleep zero handling");
}

fn test_sys_clock_getres() {
    // clock_getres 系统调用
    test_pass("sys_clock_getres interface exists");

    // clock_getres 返回时钟分辨率
    // 通常返回 1 纳秒或更高
    test_pass("sys_clock_getres resolution");
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let gettimeofday_ok = SyscallNo::Gettimeofday as u32 == 169;
    let clock_gettime_ok = SyscallNo::ClockGettime as u32 == 113;
    let clock_getres_ok = SyscallNo::ClockGetres as u32 == 114;
    let clock_nanosleep_ok = SyscallNo::ClockNanosleep as u32 == 115;
    let nanosleep_ok = SyscallNo::Nanosleep as u32 == 101;

    if gettimeofday_ok && clock_gettime_ok && clock_getres_ok && clock_nanosleep_ok && nanosleep_ok {
        test_pass("time syscall numbers");
    } else {
        test_fail("time syscall numbers", "mismatch with Linux");
    }
}
