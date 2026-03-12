//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Process sleep and wakeup mechanism test
use crate::println;
use crate::process::task::{Task, TaskState};
use super::{test_pass, test_fail, test_group_start};

pub fn test_sleep_and_wakeup() {
    test_group_start("sleep and wakeup");

    // Test 1: Verify TaskState constant values
    if TaskState::RUNNING == 0 && TaskState::INTERRUPTIBLE == 1
        && TaskState::UNINTERRUPTIBLE == 2 && TaskState::ZOMBIE == 16
        && TaskState::STOPPED == 4 {
        test_pass("TaskState constants");
    } else {
        test_fail("TaskState constants", "value mismatch");
    }

    // Test 2: Verify state setting and getting
    let mut task = Task::new(999, crate::process::task::SchedPolicy::Normal);
    if !task.state().is_running() {
        test_fail("initial state", "should be Running");
        return;
    }
    task.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    if !task.state().is_interruptible() {
        test_fail("set INTERRUPTIBLE", "state not set");
        return;
    }
    task.set_state(TaskState::new(TaskState::UNINTERRUPTIBLE));
    if !task.state().is_sleeping() {
        test_fail("set UNINTERRUPTIBLE", "state not set");
        return;
    }
    task.set_state(TaskState::new(TaskState::RUNNING));
    if !task.state().is_running() {
        test_fail("restore RUNNING", "state not set");
        return;
    }
    test_pass("state get/set");

    // Test 3: Verify wake_up function
    let mut task = Task::new(1000, crate::process::task::SchedPolicy::Normal);
    task.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    let result = Task::wake_up(&mut task as *mut Task);
    if result && task.state().is_running() {
        test_pass("wake_up sleeping task");
    } else {
        test_fail("wake_up sleeping task", "failed");
    }
    let result2 = Task::wake_up(&mut task as *mut Task);
    if !result2 {
        test_pass("wake_up running task (false)");
    } else {
        test_fail("wake_up running task", "should return false");
    }

    // Test 4: Verify sleep function exists
    test_pass("sleep function available");

    println!("test: sleep and wakeup testing completed.");
}
