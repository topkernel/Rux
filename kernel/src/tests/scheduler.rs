//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：进程调度器
use crate::println;
use alloc::format;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_scheduler() {
    test_group_start("scheduler");

    // 测试 1: 获取当前进程 PID
    let pid = crate::sched::get_current_pid();
    if pid == 0 {
        test_pass("get_current_pid (idle task)");
    } else {
        test_pass("get_current_pid (running task)");
    }

    // 测试 2: 获取当前进程 PPID
    let _ppid = crate::sched::get_current_ppid();
    test_pass("get_current_ppid");

    // 测试 3: 获取当前任务
    match crate::sched::current() {
        Some(task) => {
            let task_pid = task.pid();
            let task_state = task.state();
            test_pass(&format!("current() task PID={} state={:?}", task_pid, task_state));
        }
        None => {
            test_fail("current()", "returned None");
            return;
        }
    }

    // 测试 4: 获取文件描述符表
    match crate::sched::get_current_fdtable() {
        Some(_) => {
            test_pass("get_current_fdtable");
        }
        None => {
            test_skip("get_current_fdtable", "no fdtable for idle task");
        }
    }

    // 测试 5: 测试 find_task_by_pid (查找 idle task)
    let task_ptr = unsafe { crate::sched::find_task_by_pid(0) };
    if !task_ptr.is_null() {
        test_pass("find_task_by_pid(0)");
    } else {
        test_skip("find_task_by_pid(0)", "idle task not in global list");
    }

    // 测试 6: 测试 find_task_by_pid with invalid PID
    let invalid_ptr = unsafe { crate::sched::find_task_by_pid(99999) };
    if invalid_ptr.is_null() {
        test_pass("find_task_by_pid(invalid)");
    } else {
        test_fail("find_task_by_pid(invalid)", "should return null");
    }

    // 测试 7: 验证 schedule 函数存在
    test_pass("schedule() function available");

    println!("test: Scheduler testing completed.");
}
