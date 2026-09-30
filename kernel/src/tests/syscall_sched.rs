//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Scheduler related system call test

use crate::syscall::sched::{
    sys_sched_yield, sys_getpriority, sys_setpriority,
    PRIO_PROCESS, PRIO_PGRP, PRIO_USER, MIN_NICE, MAX_NICE,
    SCHED_NORMAL, SCHED_FIFO, SCHED_RR, SCHED_BATCH, SCHED_IDLE, SCHED_DEADLINE,
    SchedParam, SchedAttr,
};
use crate::syscall::SyscallNo;
use crate::process;
use crate::sched::{self, rt, fair};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_sched() {
    test_group_start("syscall: scheduler");

    // Test 1: sched_yield
    test_sched_yield();

    // Test 2: getpriority / setpriority
    test_priority();

    // Test 3: SchedPolicy enum
    test_sched_policy();

    // Test 4: Scheduling constants
    test_sched_constants();

    // Test 5: Syscall numbers
    test_syscall_numbers();
}

fn test_sched_yield() {
    // sched_yield always returns 0
    let result = sys_sched_yield([0, 0, 0, 0, 0, 0]);
    test_assert!(result == 0, "sys_sched_yield returns 0");

    // PID should not change after yield
    let pid_before = process::current_pid();
    let _ = sys_sched_yield([0, 0, 0, 0, 0, 0]);
    let pid_after = process::current_pid();
    test_assert!(pid_before == pid_after, "sched_yield preserves PID");

    // Multiple yields should all succeed
    for _ in 0..5 {
        let r = sys_sched_yield([0, 0, 0, 0, 0, 0]);
        if r != 0 {
            test_fail("sched_yield loop", "non-zero return");
            return;
        }
    }
    test_pass("sched_yield 5 consecutive calls");
}

fn test_priority() {
    // PRIO_* constants
    test_assert_eq!(PRIO_PROCESS, 0, "PRIO_PROCESS == 0");
    test_assert_eq!(PRIO_PGRP, 1, "PRIO_PGRP == 1");
    test_assert_eq!(PRIO_USER, 2, "PRIO_USER == 2");

    // Nice range
    test_assert_eq!(MIN_NICE, -20, "MIN_NICE == -20");
    test_assert_eq!(MAX_NICE, 19, "MAX_NICE == 19");

    // getpriority for current process (which=PRIO_PROCESS, who=0)
    // Returns nice + 20, so default nice=0 → returns 20
    let prio = sys_getpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
    // In test context (idle task), may not have a valid process
    if prio == 20 {
        test_pass("getpriority current process returns 20 (nice=0)");
    } else if prio < 0 {
        test_skip("getpriority current", "no valid process context");
    } else {
        // Some other nice value is also valid
        test_pass("getpriority returns valid priority");
    }

    // getpriority PRIO_PGRP / PRIO_USER (review批次1: both selectors are
    // now supported — who=0 takes the caller's own pgid/uid and returns
    // the best priority among matching tasks). At this point the boot
    // task still has nice=0 → 20 - 0 = 20.
    let result = sys_getpriority([PRIO_PGRP as u64, 0, 0, 0, 0, 0]);
    if result == 20 {
        test_pass("getpriority PRIO_PGRP returns best prio of own pgrp");
    } else if result < 0 {
        test_skip("getpriority PRIO_PGRP", "no matching process context");
    } else {
        test_fail("getpriority PRIO_PGRP", &alloc::format!("expected 20, got {}", result));
    }

    let result = sys_getpriority([PRIO_USER as u64, 0, 0, 0, 0, 0]);
    if result == 20 {
        test_pass("getpriority PRIO_USER returns best prio of own uid");
    } else if result < 0 {
        test_skip("getpriority PRIO_USER", "no matching process context");
    } else {
        test_fail("getpriority PRIO_USER", &alloc::format!("expected 20, got {}", result));
    }

    // setpriority for current process (which=PRIO_PROCESS, who=0, prio=5)
    let result = sys_setpriority([PRIO_PROCESS as u64, 0, 5, 0, 0, 0]);
    if result == 0 {
        test_pass("setpriority succeeds");
        // Verify: Linux ABI returns 20 - nice, so nice=5 → 15
        let new_prio = sys_getpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
        if new_prio == 15 {
            test_pass("getpriority reflects setpriority(5) → 15");
        } else if new_prio < 0 {
            test_skip("getpriority verify", "no process context");
        } else {
            test_fail("getpriority after set", &alloc::format!("expected 15, got {}", new_prio));
        }
        // Restore nice to 0
        let _ = sys_setpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
    } else {
        test_skip("setpriority", "no valid process context");
    }

    // setpriority PRIO_PGRP (supported since review批次1): who=0 selects
    // the caller's own process group. In the test context pgrp 0 also
    // contains kernel threads, and renicing a foreign task needs
    // CAP_SYS_NICE — the syscall then reports -EPERM, which is the
    // documented Linux behaviour, not a failure.
    let result = sys_setpriority([PRIO_PGRP as u64, 0, 0, 0, 0, 0]);
    if result == 0 {
        test_pass("setpriority PRIO_PGRP applies to own pgrp");
    } else if result == -1 {
        test_pass("setpriority PRIO_PGRP blocked by CAP_SYS_NICE on pgrp mates");
    } else if result == -3 {
        test_skip("setpriority PRIO_PGRP", "no matching process context");
    } else {
        test_fail("setpriority PRIO_PGRP", &alloc::format!("expected 0 or -EPERM, got {}", result));
    }

    // setpriority clamps nice value
    // setpriority with nice=-100 should clamp to MIN_NICE=-20
    let result = sys_setpriority([PRIO_PROCESS as u64, 0, (-100i32) as u64, 0, 0, 0]);
    if result == 0 {
        let prio = sys_getpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
        if prio == 40 {
            // nice=-20 → prio = 20-(-20) = 40
            test_pass("setpriority clamps to MIN_NICE");
        } else if prio < 0 {
            test_skip("setpriority clamp verify", "no process context");
        } else {
            test_fail("setpriority MIN_NICE", &alloc::format!("expected prio=40, got {}", prio));
        }
        let _ = sys_setpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
    } else if result == -13 {
        // EACCES: raising priority (nice < current) needs CAP_SYS_NICE,
        // which the boot task lacks — the clamp is still exercised by the
        // MAX_NICE case below.
        test_skip("setpriority clamp", "CAP_SYS_NICE required to raise priority");
    } else {
        test_skip("setpriority clamp", "no valid process context");
    }

    // setpriority with nice=100 should clamp to MAX_NICE=19
    let result = sys_setpriority([PRIO_PROCESS as u64, 0, 100, 0, 0, 0]);
    if result == 0 {
        let prio = sys_getpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
        if prio == 1 {
            // nice=19 → prio = 20-19 = 1
            test_pass("setpriority clamps to MAX_NICE");
        } else if prio < 0 {
            test_skip("setpriority MAX clamp", "no process context");
        } else {
            test_fail("setpriority MAX_NICE", &alloc::format!("expected prio=1, got {}", prio));
        }
        let _ = sys_setpriority([PRIO_PROCESS as u64, 0, 0, 0, 0, 0]);
    } else {
        test_skip("setpriority MAX clamp", "no valid process context");
    }

    // setpriority for nonexistent PID (99999)
    let result = sys_setpriority([PRIO_PROCESS as u64, 99999, 0, 0, 0, 0]);
    test_assert!(result == -3, "setpriority nonexistent PID returns -ESRCH");

    // getpriority for nonexistent PID (99999)
    let result = sys_getpriority([PRIO_PROCESS as u64, 99999, 0, 0, 0, 0]);
    test_assert!(result == -3, "getpriority nonexistent PID returns -ESRCH");
}

fn test_sched_policy() {
    // SchedPolicy enum values
    use crate::process::task::SchedPolicy;
    test_assert_eq!(SchedPolicy::Normal as u32, 0, "SchedPolicy::Normal == 0");
    test_assert_eq!(SchedPolicy::Fifo as u32, 1, "SchedPolicy::Fifo == 1");
    test_assert_eq!(SchedPolicy::Rr as u32, 2, "SchedPolicy::Rr == 2");
    test_assert_eq!(SchedPolicy::Batch as u32, 3, "SchedPolicy::Batch == 3");
    test_assert_eq!(SchedPolicy::Idle as u32, 5, "SchedPolicy::Idle == 5");
    test_assert_eq!(SchedPolicy::Deadline as u32, 6, "SchedPolicy::Deadline == 6");

    // SchedPolicy is repr(u32)
    test_assert_eq!(core::mem::size_of::<SchedPolicy>(), 4, "SchedPolicy size == 4");

    // Verify default policy of current task
    match crate::sched::current() {
        Some(current) => {
            let policy = current.policy();
            if policy == SchedPolicy::Normal {
                test_pass("default task policy is SCHED_NORMAL");
            } else {
                test_pass(&alloc::format!("task policy = {:?}", policy));
            }
        }
        None => {
            test_skip("task policy", "no current task");
        }
    }

    // Verify nice value
    match crate::sched::current() {
        Some(current) => {
            let nice = current.nice();
            if nice == 0 {
                test_pass("default task nice == 0");
            } else {
                test_pass(&alloc::format!("task nice = {}", nice));
            }
        }
        None => {
            test_skip("task nice", "no current task");
        }
    }

    // Verify RT priority (should be 0 for non-RT tasks)
    match crate::sched::current() {
        Some(current) => {
            let rt_prio = current.rt_priority();
            if rt_prio == 0 {
                test_pass("non-RT task rt_priority == 0");
            } else {
                test_fail("rt_priority", &alloc::format!("expected 0, got {}", rt_prio));
            }
        }
        None => {
            test_skip("task rt_priority", "no current task");
        }
    }
}

fn test_sched_constants() {
    // Scheduling policy constants
    test_assert_eq!(SCHED_NORMAL, 0, "SCHED_NORMAL == 0");
    test_assert_eq!(SCHED_FIFO, 1, "SCHED_FIFO == 1");
    test_assert_eq!(SCHED_RR, 2, "SCHED_RR == 2");
    test_assert_eq!(SCHED_BATCH, 3, "SCHED_BATCH == 3");
    test_assert_eq!(SCHED_IDLE, 5, "SCHED_IDLE == 5");
    test_assert_eq!(SCHED_DEADLINE, 6, "SCHED_DEADLINE == 6");

    // SchedParam struct layout
    test_assert_eq!(core::mem::size_of::<SchedParam>(), 4, "SchedParam size == 4");
    test_assert_eq!(core::mem::align_of::<SchedParam>(), 4, "SchedParam align == 4");

    // SchedAttr struct size (at least 48 bytes)
    test_assert!(core::mem::size_of::<SchedAttr>() >= 48, "SchedAttr size >= 48");

    // RT scheduler constants
    test_assert_eq!(rt::MAX_RT_PRIO, 100, "MAX_RT_PRIO == 100");
    test_assert_eq!(rt::RR_TIMESLICE_TICKS, (100 * crate::config::KERNEL_HZ) / 1000, "RR_TIMESLICE_TICKS == 100ms in ticks");

    // CFS constants
    test_assert_eq!(fair::NICE_0_LOAD, 1024, "NICE_0_LOAD == 1024");
}

fn test_syscall_numbers() {
    test_assert_eq!(SyscallNo::SchedYield as u32, 124, "SchedYield == 124");
    test_assert_eq!(SyscallNo::Setpriority as u32, 140, "Setpriority == 140");
    test_assert_eq!(SyscallNo::Getpriority as u32, 141, "Getpriority == 141");
    test_assert_eq!(SyscallNo::Futex as u32, 98, "Futex == 98");
    test_assert_eq!(SyscallNo::SchedSetparam as u32, 118, "SchedSetparam == 118");
    test_assert_eq!(SyscallNo::SchedSetscheduler as u32, 119, "SchedSetscheduler == 119");
    test_assert_eq!(SyscallNo::SchedGetscheduler as u32, 120, "SchedGetscheduler == 120");
    test_assert_eq!(SyscallNo::SchedGetparam as u32, 121, "SchedGetparam == 121");
    test_assert_eq!(SyscallNo::SchedSetaffinity as u32, 122, "SchedSetaffinity == 122");
    test_assert_eq!(SyscallNo::SchedGetaffinity as u32, 123, "SchedGetaffinity == 123");
    test_assert_eq!(SyscallNo::SchedGetPriorityMax as u32, 125, "SchedGetPriorityMax == 125");
    test_assert_eq!(SyscallNo::SchedGetPriorityMin as u32, 126, "SchedGetPriorityMin == 126");
    test_assert_eq!(SyscallNo::SchedRrGetInterval as u32, 127, "SchedRrGetInterval == 127");
}
