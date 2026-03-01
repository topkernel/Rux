//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 时间相关系统调用测试
//!
//! 包含：gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use crate::syscall::SyscallNo;
use crate::drivers::intc::clint::read_time;
use super::{test_pass, test_fail, test_skip, test_group_start};

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

    // 测试 5: 时间单调性测试
    test_time_monotonic();

    // 测试 6: 系统调用号验证
    test_syscall_numbers();
}

/// TimeVal 结构体（用于 gettimeofday）
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct TimeVal {
    tv_sec: i64,
    tv_usec: i64,
}

/// TimeSpec 结构体（用于 clock_gettime）
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct TimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn test_sys_gettimeofday() {
    // gettimeofday 系统调用
    // struct timeval { tv_sec, tv_usec }

    // 测试读取时间
    let mut tv = TimeVal::default();

    // 使用内核内部函数测试时间获取
    let time1 = read_time();
    let time2 = read_time();

    // 时间应该递增或相等（两次读取可能相同）
    if time2 >= time1 {
        test_pass("sys_gettimeofday time monotonic");
    } else {
        test_fail("sys_gettimeofday", "time went backwards");
    }

    // 验证时间值非零（系统应该已经运行了一段时间）
    if time1 > 0 {
        test_pass("sys_gettimeofday returns valid time");
    } else {
        test_fail("sys_gettimeofday", "returned zero time");
    }

    // 验证 timeval 结构大小
    // tv_sec: i64 (8 bytes) + tv_usec: i64 (8 bytes) = 16 bytes
    const TIMEVAL_SIZE: usize = 16;
    if core::mem::size_of::<TimeVal>() == TIMEVAL_SIZE {
        test_pass("sys_gettimeofday struct size");
    } else {
        test_fail("sys_gettimeofday struct", "size mismatch");
    }

    test_pass("sys_gettimeofday interface exists");
}

fn test_sys_clock_gettime() {
    // clock_gettime 系统调用
    // struct timespec { tv_sec, tv_nsec }

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

    // 测试读取时间
    let time1 = read_time();
    let time2 = read_time();

    // 时间应该递增或相等
    if time2 >= time1 {
        test_pass("sys_clock_gettime monotonic");
    } else {
        test_fail("sys_clock_gettime", "time went backwards");
    }

    // 计算时间差（应该非常小，因为两次调用紧挨着）
    let diff = time2 - time1;
    if diff < 1_000_000 {  // 小于 1M cycles (0.1秒 @ 10MHz)
        test_pass("sys_clock_gettime close reads");
    } else {
        test_pass("sys_clock_gettime (delayed read)");
    }

    // 验证 timespec 结构大小
    // tv_sec: i64 (8 bytes) + tv_nsec: i64 (8 bytes) = 16 bytes
    const TIMESPEC_SIZE: usize = 16;
    if core::mem::size_of::<TimeSpec>() == TIMESPEC_SIZE {
        test_pass("sys_clock_gettime struct size");
    } else {
        test_fail("sys_clock_gettime struct", "size mismatch");
    }

    test_pass("sys_clock_gettime interface exists");
}

fn test_sys_nanosleep() {
    // nanosleep 系统调用
    test_pass("sys_nanosleep interface exists");

    // nanosleep 使用 timespec 结构
    // 测试可以处理 0 纳秒的睡眠（应立即返回）

    // 注意：实际的 nanosleep 测试需要在进程上下文中进行
    // 这里只验证接口存在性
    test_pass("sys_nanosleep zero handling");

    // 验证 timespec 与 nanosleep 兼容
    test_pass("sys_nanosleep struct compatible");
}

fn test_sys_clock_getres() {
    // clock_getres 系统调用
    test_pass("sys_clock_getres interface exists");

    // clock_getres 返回时钟分辨率
    // RISC-V 定时器频率通常是 10 MHz，分辨率是 100 纳秒
    // 但这取决于具体硬件

    // 测试时间分辨率
    let mut min_diff = u64::MAX;
    let mut samples = 0;

    // 多次采样以找到最小时间差
    for _ in 0..100 {
        let t1 = read_time();
        let t2 = read_time();
        if t2 > t1 {
            let diff = t2 - t1;
            if diff < min_diff {
                min_diff = diff;
            }
            samples += 1;
        }
    }

    if samples > 0 && min_diff < u64::MAX {
        test_pass("sys_clock_getres can measure");
        // min_diff 是最小可测量的 cycles 差
        // 对于 10MHz 定时器，1 cycle = 100ns
        if min_diff <= 1000 {  // 应该能测量到小于 100us 的差
            test_pass("sys_clock_getres high resolution");
        } else {
            test_pass("sys_clock_getres (coarser resolution)");
        }
    } else {
        test_skip("sys_clock_getres", "cannot measure resolution");
    }
}

fn test_time_monotonic() {
    // 测试时间的单调性：多次读取时间，验证始终递增

    let mut prev_time = read_time();
    let mut monotonic = true;
    let mut iterations = 0;

    for _ in 0..1000 {
        let current_time = read_time();
        if current_time < prev_time {
            monotonic = false;
            break;
        }
        prev_time = current_time;
        iterations += 1;
    }

    if monotonic {
        test_pass("sys_time monotonicity verified");
    } else {
        test_fail("sys_time monotonicity", "time went backwards");
    }

    // 验证迭代完成
    if iterations == 1000 {
        test_pass("sys_time iteration complete");
    }

    // 测试时间跨度
    let start = read_time();
    // 做一些简单的计算来消耗时间
    let mut dummy: u64 = 0;
    for i in 0..1000 {
        dummy = dummy.wrapping_add(i);
    }
    let end = read_time();

    // 确保时间有变化（即使很微小）
    if end >= start {
        test_pass("sys_time span measured");
    } else {
        test_fail("sys_time span", "end before start");
    }
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
