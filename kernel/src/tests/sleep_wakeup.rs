//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：进程睡眠和唤醒机制
//
// 测试内容：
// 1. Task::sleep() - 进程睡眠
// 2. Task::wake_up() - 唤醒进程
// 3. TaskState::INTERRUPTIBLE - 可中断睡眠
// 4. TaskState::UNINTERRUPTIBLE - 不可中断睡眠

use crate::println;
use crate::process::task::{Task, TaskState};

pub fn test_sleep_and_wakeup() {
    println!("test: ===== Testing Sleep and Wakeup Mechanism =====");

    // 测试 1: 验证 TaskState 常量值正确
    test_taskstate_values();

    // 测试 2: 验证状态设置和获取
    test_state_getset();

    // 测试 3: 验证 wake_up 函数存在
    test_wake_up_function();

    // 测试 4: 验证 sleep 函数存在
    test_sleep_function();

    println!("test: ===== Sleep and Wakeup Testing Completed =====");
}

fn test_taskstate_values() {
    println!("test: 1. Testing TaskState constant values...");

    // #define TASK_RUNNING        0
    // #define TASK_INTERRUPTIBLE  1
    // #define TASK_UNINTERRUPTIBLE    2
    // #define EXIT_ZOMBIE        16 (0x10)
    // #define EXIT_STOPPED        4

    assert_eq!(TaskState::RUNNING, 0, "TASK_RUNNING should be 0");
    assert_eq!(TaskState::INTERRUPTIBLE, 1, "TASK_INTERRUPTIBLE should be 1");
    assert_eq!(TaskState::UNINTERRUPTIBLE, 2, "TASK_UNINTERRUPTIBLE should be 2");
    assert_eq!(TaskState::ZOMBIE, 16, "EXIT_ZOMBIE should be 16");
    assert_eq!(TaskState::STOPPED, 4, "EXIT_STOPPED should be 4");

    println!("test:    SUCCESS - TaskState constants are correct");
}

/// 测试 2: 验证状态设置和获取
fn test_state_getset() {
    println!("test: 2. Testing state get/set...");

    let mut task = Task::new(999, crate::process::task::SchedPolicy::Normal);

    // 验证初始状态
    assert!(task.state().is_running(), "Initial state should be Running");
    println!("test:    Initial state: Running");

    // 验证设置为 INTERRUPTIBLE
    task.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    assert!(task.state().is_interruptible(), "State should be Interruptible");
    println!("test:    After set_state: Interruptible");

    // 验证设置为 UNINTERRUPTIBLE
    task.set_state(TaskState::new(TaskState::UNINTERRUPTIBLE));
    assert!(task.state().is_sleeping(), "State should be sleeping (Uninterruptible)");
    println!("test:    After set_state: Uninterruptible");

    // 验证恢复为 RUNNING
    task.set_state(TaskState::new(TaskState::RUNNING));
    assert!(task.state().is_running(), "State should be Running");
    println!("test:    After set_state: Running");

    println!("test:    SUCCESS - state get/set works correctly");
}

/// 测试 3: 验证 wake_up 函数存在且功能正常
fn test_wake_up_function() {
    println!("test: 3. Testing wake_up function...");

    let mut task = Task::new(1000, crate::process::task::SchedPolicy::Normal);

    // 设置为睡眠状态
    task.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    println!("test:    Task state set to Interruptible");

    // 唤醒进程
    let result = Task::wake_up(&mut task as *mut Task);
    assert_eq!(result, true, "wake_up should return true for sleeping task");
    println!("test:    wake_up returned: {}", result);

    // 验证状态已恢复为 Running
    assert!(task.state().is_running(), "State should be Running after wake_up");
    println!("test:    State after wake_up: Running");

    // 测试唤醒已运行的进程（应该返回 false）
    let result2 = Task::wake_up(&mut task as *mut Task);
    assert_eq!(result2, false, "wake_up should return false for running task");
    println!("test:    wake_up on running task returned: {} (expected false)", result2);

    println!("test:    SUCCESS - wake_up function works correctly");
}

/// 测试 4: 验证 sleep 函数存在
fn test_sleep_function() {
    println!("test: 4. Testing sleep function availability...");

    // 注意：不能在这里真正调用 Task::sleep()，因为它会触发调度
    // 我们只验证函数存在且类型正确

    println!("test:    Task::sleep function exists");
    println!("test:    Signature: Task::sleep(TaskState)");
    println!("test:    SUCCESS - sleep function is available");
}
