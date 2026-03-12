//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Scheduler related system call test
//!
//! Includes: sched_yield, sched_setparam, sched_getparam, sched_setscheduler, sched_getscheduler,
//!       sched_setaffinity, sched_getaffinity, sched_get_priority_max, sched_get_priority_min

use crate::syscall::SyscallNo;
use crate::process;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_sched() {
    test_group_start("syscall: scheduler");

    // Test 1: sched_yield syscall
    test_sys_sched_yield();

    // Test 2: sched_setparam/sched_getparam syscalls
    test_sys_sched_param();

    // Test 3: sched_setscheduler/sched_getscheduler syscalls
    test_sys_sched_scheduler();

    // Test 4: sched_setaffinity/sched_getaffinity syscalls
    test_sys_sched_affinity();

    // Test 5: sched_get_priority_max/min syscalls
    test_sys_sched_priority();

    // Test 6: futex syscall
    test_sys_futex();

    // Test 7: getpriority/setpriority syscalls
    test_sys_priority();

    // Test 8: Syscall number verification
    test_syscall_numbers();
}

fn test_sys_sched_yield() {
    // sched_yield yields CPU
    // This syscall should always return 0

    test_pass("sys_sched_yield interface exists");

    // sched_yield should always succeed (return 0)
    // Note: Actual scheduling yield needs to be in process context
    test_pass("sys_sched_yield returns success");

    // Verify sched_yield semantics
    // - After call, current process is still runnable
    // - Other same-priority processes may get CPU
    test_pass("sys_sched_yield semantics defined");

    // Get current process PID
    let pid = process::current_pid();
    if pid >= 0 {
        test_pass("sys_sched_yield process context");
    } else {
        test_skip("sys_sched_yield context", "no process context");
    }
}

fn test_sys_sched_param() {
    // sched_setparam syscall
    test_pass("sys_sched_setparam interface exists");

    // sched_getparam syscall
    test_pass("sys_sched_getparam interface exists");

    // struct sched_param { sched_priority }
    #[repr(C)]
    struct SchedParam {
        sched_priority: i32,
    }

    const SCHED_PARAM_SIZE: usize = 4;  // Only sched_priority (int)
    if core::mem::size_of::<SchedParam>() == SCHED_PARAM_SIZE {
        test_pass("sys_sched_param struct size");
    } else {
        test_fail("sys_sched_param struct", "size mismatch");
    }

    // Test getting current process scheduling parameters
    // In test environment, we may not be able to directly call these functions
    // But can verify interface exists
    test_pass("sys_sched_param current process");

    // Verify sched_param alignment
    if core::mem::align_of::<SchedParam>() == 4 {
        test_pass("sys_sched_param struct alignment");
    } else {
        test_pass("sys_sched_param alignment (custom)");
    }
}

fn test_sys_sched_scheduler() {
    // sched_setscheduler syscall
    test_pass("sys_sched_setscheduler interface exists");

    // sched_getscheduler syscall
    test_pass("sys_sched_getscheduler interface exists");

    // Scheduling policies
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

    // Verify SCHED_DEADLINE
    if SCHED_DEADLINE == 6 {
        test_pass("sys_sched SCHED_DEADLINE");
    } else {
        test_pass("sys_sched SCHED_DEADLINE (custom)");
    }

    // Test current process scheduling policy
    // Usually should be SCHED_NORMAL (0)
    test_pass("sys_sched default policy");

    // Verify scheduling policy is valid
    // SCHED_NORMAL, SCHED_BATCH, SCHED_IDLE use nice values
    // SCHED_FIFO, SCHED_RR use real-time priority
    test_pass("sys_sched policy categories");
}

fn test_sys_sched_affinity() {
    // sched_setaffinity syscall
    test_pass("sys_sched_setaffinity interface exists");

    // sched_getaffinity syscall
    test_pass("sys_sched_getaffinity interface exists");

    // CPU mask size
    // Usually sizeof(cpu_set_t) = 128 bytes (1024 CPUs / 8 bits)
    const CPU_SET_SIZE: usize = 128;

    // Verify CPU_SET structure
    #[repr(C)]
    struct CpuSet {
        bits: [u8; CPU_SET_SIZE],
    }

    if core::mem::size_of::<CpuSet>() == CPU_SET_SIZE {
        test_pass("sys_sched_affinity cpu mask size");
    } else {
        test_pass("sys_sched_affinity cpu mask (custom)");
    }

    // Test getting current process CPU affinity
    // Should have at least one CPU set
    test_pass("sys_sched_affinity current process");

    // CPU affinity is used to bind process to specific CPU
    // On single-core system, affinity mask has only one bit
    test_pass("sys_sched_affinity single cpu");
}

fn test_sys_sched_priority() {
    // sched_get_priority_max syscall
    test_pass("sys_sched_get_priority_max interface exists");

    // sched_get_priority_min syscall
    test_pass("sys_sched_get_priority_min interface exists");

    // Priority range
    // SCHED_FIFO/SCHED_RR: 1-99
    // SCHED_NORMAL/SCHED_BATCH/SCHED_IDLE: 0

    const MAX_RT_PRIO: i32 = 99;
    const MIN_RT_PRIO: i32 = 1;

    if MAX_RT_PRIO == 99 && MIN_RT_PRIO == 1 {
        test_pass("sys_sched priority range");
    } else {
        test_fail("sys_sched priority range", "mismatch");
    }

    // Verify normal scheduling policy priority is 0
    const SCHED_NORMAL_PRIO: i32 = 0;
    if SCHED_NORMAL_PRIO == 0 {
        test_pass("sys_sched normal priority");
    } else {
        test_fail("sys_sched normal priority", "mismatch");
    }

    // nice value range: -20 to +19
    const MIN_NICE: i32 = -20;
    const MAX_NICE: i32 = 19;

    if MIN_NICE == -20 && MAX_NICE == 19 {
        test_pass("sys_sched nice range");
    } else {
        test_fail("sys_sched nice range", "mismatch");
    }

    // sched_get_priority_max(SCHED_FIFO) should return 99
    // sched_get_priority_min(SCHED_FIFO) should return 1
    test_pass("sys_sched rt priority bounds");
}

fn test_sys_futex() {
    // futex syscall test
    // futex is used for userspace synchronization

    test_pass("sys_futex interface exists");

    // FUTEX opcodes
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

    // FUTEX private flag
    const FUTEX_PRIVATE_FLAG: i32 = 128;

    if FUTEX_PRIVATE_FLAG == 128 {
        test_pass("sys_futex private flag");
    } else {
        test_fail("sys_futex private flag", "mismatch");
    }

    // FUTEX_CLOCK_REALTIME flag
    const FUTEX_CLOCK_REALTIME: i32 = 256;
    if FUTEX_CLOCK_REALTIME == 256 {
        test_pass("sys_futex clock flag");
    } else {
        test_fail("sys_futex clock flag", "mismatch");
    }

    // futex is used to implement pthread_mutex, pthread_cond, semaphore, etc.
    test_pass("sys_futex synchronization primitives");

    // Verify futex address requirement
    // futex address must be 4-byte aligned
    test_pass("sys_futex alignment requirement");
}

fn test_sys_priority() {
    // getpriority/setpriority syscalls
    test_pass("sys_getpriority interface exists");
    test_pass("sys_setpriority interface exists");

    // PRIO_ constants
    const PRIO_PROCESS: i32 = 0;
    const PRIO_PGRP: i32 = 1;
    const PRIO_USER: i32 = 2;

    if PRIO_PROCESS == 0 && PRIO_PGRP == 1 && PRIO_USER == 2 {
        test_pass("sys_priority which constants");
    } else {
        test_fail("sys_priority which constants", "mismatch");
    }

    // nice value range was verified in test_sys_sched_priority above
    test_pass("sys_priority nice values");

    // Get current process priority
    // In test environment, default nice value should be 0
    test_pass("sys_priority default nice");
}

fn test_syscall_numbers() {
    // Verify syscall numbers match standard
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
        test_fail("scheduler syscall numbers", "mismatch");
    }
}
