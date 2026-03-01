//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 抢占式调度器测试
use crate::println;
use crate::drivers::timer;
use alloc::format;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_preemptive_scheduler() {
    test_group_start("preemptive scheduler");

    // 测试 1: jiffies 计数器
    let jiffies1 = timer::get_jiffies();
    let jiffies2 = timer::get_jiffies();
    if jiffies2 >= jiffies1 {
        test_pass(&format!("jiffies ({} -> {})", jiffies1, jiffies2));
    } else {
        test_fail("jiffies", "counter went backwards");
    }

    // 测试 2: need_resched 标志
    let _initial_resched = crate::sched::need_resched();
    crate::sched::set_need_resched();
    let after_set = crate::sched::need_resched();
    if after_set {
        test_pass("need_resched flag");
    } else {
        test_fail("need_resched flag", "not set");
    }

    // 测试 3: 时间片管理
    match crate::sched::current() {
        Some(task) => {
            let task_ref = unsafe { &mut *(task as *mut crate::process::Task) };
            let initial_slice = task_ref.get_time_slice();
            task_ref.reset_time_slice();
            let reset_slice = task_ref.get_time_slice();
            if reset_slice > 0 {
                test_pass(&format!("time slice ({} -> {})", initial_slice, reset_slice));
            } else {
                test_fail("time slice", "zero after reset");
            }
        }
        None => {
            test_skip("time slice", "no current task");
        }
    }

    // 测试 4: jiffies 转换函数
    let msecs = timer::jiffies_to_msecs(100);
    let jiffies = timer::msecs_to_jiffies(500);
    if msecs == 1000 && jiffies == 50 {
        test_pass("jiffies conversion");
    } else {
        test_fail("jiffies conversion", &format!("msecs={}, jiffies={}", msecs, jiffies));
    }

    println!("test: preemptive scheduler testing completed.");
}
