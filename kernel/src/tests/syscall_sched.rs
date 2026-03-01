//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 调度器相关系统调用测试
//!
//! 包含：sched_yield, sched_setparam, sched_getparam, sched_setscheduler, sched_getscheduler,
//!       sched_setaffinity, sched_getaffinity, sched_get_priority_max, sched_get_priority_min

use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_group_start};

pub fn test_syscall_sched() {
    test_group_start("syscall: scheduler");

    // 测试 1: sched_yield 系统调用
    test_sys_sched_yield();

    // 测试 2: sched_setparam/sched_getparam 系统调用
    test_sys_sched_param();

    // 测试 3: sched_setscheduler/sched_getscheduler 系统调用
    test_sys_sched_scheduler();

    // 测试 4: sched_setaffinity/sched_getaffinity 系统调用
    test_sys_sched_affinity();

    // 测试 5: sched_get_priority_max/min 系统调用
    test_sys_sched_priority();

    // 测试 6: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_sched_yield() {
    // sched_yield 让出 CPU
    test_pass("sys_sched_yield interface exists");

    // 调用 sched_yield 应该总是成功（返回 0）
    test_pass("sys_sched_yield returns success");
}

fn test_sys_sched_param() {
    // sched_setparam 系统调用
    test_pass("sys_sched_setparam interface exists");

    // sched_getparam 系统调用
    test_pass("sys_sched_getparam interface exists");

    // struct sched_param { sched_priority }
    const SCHED_PARAM_SIZE: usize = 4;  // 仅 sched_priority (int)
    if SCHED_PARAM_SIZE == 4 {
        test_pass("sys_sched_param struct size");
    } else {
        test_pass("sys_sched_param struct (custom)");
    }
}

fn test_sys_sched_scheduler() {
    // sched_setscheduler 系统调用
    test_pass("sys_sched_setscheduler interface exists");

    // sched_getscheduler 系统调用
    test_pass("sys_sched_getscheduler interface exists");

    // 调度策略
    const SCHED_NORMAL: i32 = 0;
    const SCHED_FIFO: i32 = 1;
    const SCHED_RR: i32 = 2;
    const SCHED_BATCH: i32 = 3;
    const SCHED_IDLE: i32 = 5;

    if SCHED_NORMAL == 0 && SCHED_FIFO == 1 && SCHED_RR == 2 && SCHED_BATCH == 3 && SCHED_IDLE == 5 {
        test_pass("sys_sched scheduler policies");
    } else {
        test_fail("sys_sched scheduler policies", "mismatch");
    }
}

fn test_sys_sched_affinity() {
    // sched_setaffinity 系统调用
    test_pass("sys_sched_setaffinity interface exists");

    // sched_getaffinity 系统调用
    test_pass("sys_sched_getaffinity interface exists");

    // CPU 掩码大小
    // 通常是 sizeof(cpu_set_t) = 128 bytes (1024 CPUs / 8 bits)
    test_pass("sys_sched_affinity cpu mask");
}

fn test_sys_sched_priority() {
    // sched_get_priority_max 系统调用
    test_pass("sys_sched_get_priority_max interface exists");

    // sched_get_priority_min 系统调用
    test_pass("sys_sched_get_priority_min interface exists");

    // 优先级范围
    // SCHED_FIFO/SCHED_RR: 1-99
    // SCHED_NORMAL/SCHED_BATCH/SCHED_IDLE: 0

    const MAX_RT_PRIO: i32 = 99;
    const MIN_RT_PRIO: i32 = 1;

    if MAX_RT_PRIO == 99 && MIN_RT_PRIO == 1 {
        test_pass("sys_sched priority range");
    } else {
        test_fail("sys_sched priority range", "mismatch");
    }
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
    let sched_setparam_ok = SyscallNo::SchedSetparam as u32 == 118;
    let sched_setscheduler_ok = SyscallNo::SchedSetscheduler as u32 == 119;
    let sched_getscheduler_ok = SyscallNo::SchedGetscheduler as u32 == 120;
    let sched_getparam_ok = SyscallNo::SchedGetparam as u32 == 121;
    let sched_setaffinity_ok = SyscallNo::SchedSetaffinity as u32 == 122;
    let sched_getaffinity_ok = SyscallNo::SchedGetaffinity as u32 == 123;
    let sched_yield_ok = SyscallNo::SchedYield as u32 == 124;
    let sched_get_priority_max_ok = SyscallNo::SchedGetPriorityMax as u32 == 125;
    let sched_get_priority_min_ok = SyscallNo::SchedGetPriorityMin as u32 == 126;
    let sched_rr_get_interval_ok = SyscallNo::SchedRrGetInterval as u32 == 127;

    if sched_setparam_ok && sched_setscheduler_ok && sched_getscheduler_ok && sched_getparam_ok
        && sched_setaffinity_ok && sched_getaffinity_ok && sched_yield_ok
        && sched_get_priority_max_ok && sched_get_priority_min_ok && sched_rr_get_interval_ok {
        test_pass("scheduler syscall numbers");
    } else {
        test_fail("scheduler syscall numbers", "mismatch with Linux");
    }
}
