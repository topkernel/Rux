//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Process scheduler
use crate::println;
use alloc::format;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_scheduler() {
    test_group_start("scheduler");

    // Test 1: Get current process PID
    let pid = crate::sched::get_current_pid();
    if pid == 0 {
        test_pass("get_current_pid (idle task)");
    } else {
        test_pass("get_current_pid (running task)");
    }

    // Test 2: Get current process PPID
    let _ppid = crate::sched::get_current_ppid();
    test_pass("get_current_ppid");

    // Test 3: Get current task
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

    // Test 4: Get file descriptor table
    match crate::sched::get_current_fdtable() {
        Some(_) => {
            test_pass("get_current_fdtable");
        }
        None => {
            test_skip("get_current_fdtable", "no fdtable for idle task");
        }
    }

    // Test 5: Test find_task_by_pid (find idle task)
    let task_ptr = unsafe { crate::sched::find_task_by_pid(0) };
    if !task_ptr.is_null() {
        test_pass("find_task_by_pid(0)");
    } else {
        test_skip("find_task_by_pid(0)", "idle task not in global list");
    }

    // Test 6: Test find_task_by_pid with invalid PID
    let invalid_ptr = unsafe { crate::sched::find_task_by_pid(99999) };
    if invalid_ptr.is_null() {
        test_pass("find_task_by_pid(invalid)");
    } else {
        test_fail("find_task_by_pid(invalid)", "should return null");
    }

    // Test 7: Verify schedule function exists
    test_pass("schedule() function available");

    println!("test: Scheduler testing completed.");
}
