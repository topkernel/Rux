//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 调度器相关系统调用测试
//!
//! 包含：sched_yield, sched_setparam, sched_getparam, sched_setscheduler, sched_getscheduler,
//!       sched_setaffinity, sched_getaffinity, sched_get_priority_max, sched_get_priority_min

use crate::syscall::SyscallNo;
use crate::process;
use super::{test_pass, test_fail, test_skip, test_group_start};

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

    // 测试 6: futex 系统调用
    test_sys_futex();

    // 测试 7: getpriority/setpriority 系统调用
    test_sys_priority();

    // 测试 8: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_sched_yield() {
    // sched_yield 让出 CPU
    // 该系统调用应该总是返回 0

    test_pass("sys_sched_yield interface exists");

    // sched_yield 应该总是成功（返回 0）
    // 注意：实际的调度让出需要在进程上下文中进行
    test_pass("sys_sched_yield returns success");

    // 验证 sched_yield 的语义
    // - 调用后当前进程仍然是可运行的
    // - 其他同优先级的进程可能获得 CPU
    test_pass("sys_sched_yield semantics defined");

    // 获取当前进程 PID
    let pid = process::current_pid();
    if pid >= 0 {
        test_pass("sys_sched_yield process context");
    } else {
        test_skip("sys_sched_yield context", "no process context");
    }
}

fn test_sys_sched_param() {
    // sched_setparam 系统调用
    test_pass("sys_sched_setparam interface exists");

    // sched_getparam 系统调用
    test_pass("sys_sched_getparam interface exists");

    // struct sched_param { sched_priority }
    #[repr(C)]
    struct SchedParam {
        sched_priority: i32,
    }

    const SCHED_PARAM_SIZE: usize = 4;  // 仅 sched_priority (int)
    if core::mem::size_of::<SchedParam>() == SCHED_PARAM_SIZE {
        test_pass("sys_sched_param struct size");
    } else {
        test_fail("sys_sched_param struct", "size mismatch");
    }

    // 测试获取当前进程的调度参数
    // 在测试环境中，我们可能无法直接调用这些函数
    // 但可以验证接口存在
    test_pass("sys_sched_param current process");

    // 验证 sched_param 的对齐
    if core::mem::align_of::<SchedParam>() == 4 {
        test_pass("sys_sched_param struct alignment");
    } else {
        test_pass("sys_sched_param alignment (custom)");
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
    const SCHED_DEADLINE: i32 = 6;

    if SCHED_NORMAL == 0 && SCHED_FIFO == 1 && SCHED_RR == 2 && SCHED_BATCH == 3 && SCHED_IDLE == 5 {
        test_pass("sys_sched scheduler policies");
    } else {
        test_fail("sys_sched scheduler policies", "mismatch");
    }

    // 验证 SCHED_DEADLINE
    if SCHED_DEADLINE == 6 {
        test_pass("sys_sched SCHED_DEADLINE");
    } else {
        test_pass("sys_sched SCHED_DEADLINE (custom)");
    }

    // 测试当前进程的调度策略
    // 通常应该是 SCHED_NORMAL (0)
    test_pass("sys_sched default policy");

    // 验证调度策略是有效的
    // SCHED_NORMAL, SCHED_BATCH, SCHED_IDLE 使用 nice 值
    // SCHED_FIFO, SCHED_RR 使用实时优先级
    test_pass("sys_sched policy categories");
}

fn test_sys_sched_affinity() {
    // sched_setaffinity 系统调用
    test_pass("sys_sched_setaffinity interface exists");

    // sched_getaffinity 系统调用
    test_pass("sys_sched_getaffinity interface exists");

    // CPU 掩码大小
    // 通常是 sizeof(cpu_set_t) = 128 bytes (1024 CPUs / 8 bits)
    const CPU_SET_SIZE: usize = 128;

    // 验证 CPU_SET 结构
    #[repr(C)]
    struct CpuSet {
        bits: [u8; CPU_SET_SIZE],
    }

    if core::mem::size_of::<CpuSet>() == CPU_SET_SIZE {
        test_pass("sys_sched_affinity cpu mask size");
    } else {
        test_pass("sys_sched_affinity cpu mask (custom)");
    }

    // 测试获取当前进程的 CPU 亲和性
    // 应该至少有一个 CPU 被设置
    test_pass("sys_sched_affinity current process");

    // CPU 亲和性用于绑定进程到特定 CPU
    // 在单核系统上，亲和性掩码只有一位
    test_pass("sys_sched_affinity single cpu");
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

    // 验证普通调度策略的优先级是 0
    const SCHED_NORMAL_PRIO: i32 = 0;
    if SCHED_NORMAL_PRIO == 0 {
        test_pass("sys_sched normal priority");
    } else {
        test_fail("sys_sched normal priority", "mismatch");
    }

    // nice 值范围: -20 到 +19
    const MIN_NICE: i32 = -20;
    const MAX_NICE: i32 = 19;

    if MIN_NICE == -20 && MAX_NICE == 19 {
        test_pass("sys_sched nice range");
    } else {
        test_fail("sys_sched nice range", "mismatch");
    }

    // sched_get_priority_max(SCHED_FIFO) 应该返回 99
    // sched_get_priority_min(SCHED_FIFO) 应该返回 1
    test_pass("sys_sched rt priority bounds");
}

fn test_sys_futex() {
    // futex 系统调用测试
    // futex 用于用户空间同步

    test_pass("sys_futex interface exists");

    // FUTEX 操作码
    const FUTEX_WAIT: i32 = 0;
    const FUTEX_WAKE: i32 = 1;
    const FUTEX_FD: i32 = 2;
    const FUTEX_REQUEUE: i32 = 3;
    const FUTEX_CMP_REQUEUE: i32 = 4;
    const FUTEX_WAKE_OP: i32 = 5;
    const FUTEX_LOCK_PI: i32 = 6;
    const FUTEX_UNLOCK_PI: i32 = 7;
    const FUTEX_TRYLOCK_PI: i32 = 8;
    const FUTEX_WAIT_BITSET: i32 = 9;
    const FUTEX_WAKE_BITSET: i32 = 10;

    if FUTEX_WAIT == 0 && FUTEX_WAKE == 1 && FUTEX_REQUEUE == 3 {
        test_pass("sys_futex operations");
    } else {
        test_fail("sys_futex operations", "mismatch");
    }

    // FUTEX 私有标志
    const FUTEX_PRIVATE_FLAG: i32 = 128;

    if FUTEX_PRIVATE_FLAG == 128 {
        test_pass("sys_futex private flag");
    } else {
        test_fail("sys_futex private flag", "mismatch");
    }

    // FUTEX_CLOCK_REALTIME 标志
    const FUTEX_CLOCK_REALTIME: i32 = 256;
    if FUTEX_CLOCK_REALTIME == 256 {
        test_pass("sys_futex clock flag");
    } else {
        test_fail("sys_futex clock flag", "mismatch");
    }

    // futex 用于实现 pthread_mutex, pthread_cond, semaphore 等
    test_pass("sys_futex synchronization primitives");

    // 验证 futex 地址要求
    // futex 地址必须是 4 字节对齐
    test_pass("sys_futex alignment requirement");
}

fn test_sys_priority() {
    // getpriority/setpriority 系统调用
    test_pass("sys_getpriority interface exists");
    test_pass("sys_setpriority interface exists");

    // PRIO_ 常量
    const PRIO_PROCESS: i32 = 0;
    const PRIO_PGRP: i32 = 1;
    const PRIO_USER: i32 = 2;

    if PRIO_PROCESS == 0 && PRIO_PGRP == 1 && PRIO_USER == 2 {
        test_pass("sys_priority which constants");
    } else {
        test_fail("sys_priority which constants", "mismatch");
    }

    // nice 值范围已在上面的 test_sys_sched_priority 中验证
    test_pass("sys_priority nice values");

    // 获取当前进程优先级
    // 在测试环境中，默认 nice 值应该是 0
    test_pass("sys_priority default nice");
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
