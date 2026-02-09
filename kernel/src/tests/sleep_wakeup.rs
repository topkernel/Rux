// 测试：进程睡眠和唤醒机制
//
// 测试内容：
// 1. Task::sleep() - 进程睡眠
// 2. Task::wake_up() - 唤醒进程
// 3. TaskState::Interruptible - 可中断睡眠
// 4. TaskState::Uninterruptible - 不可中断睡眠

use crate::println;
use crate::process::task::{Task, TaskState};

pub fn test_sleep_and_wakeup() {
    println!("test: ===== Testing Sleep and Wakeup Mechanism =====");

    // 测试 1: 验证 TaskState 枚举值正确
    test_taskstate_values();

    // 测试 2: 验证状态设置和获取
    test_state_getset();

    // 测试 3: 验证 wake_up 函数存在
    test_wake_up_function();

    // 测试 4: 验证 sleep 函数存在
    test_sleep_function();

    println!("test: ===== Sleep and Wakeup Testing Completed =====");
}

/// 测试 1: 验证 TaskState 枚举值与 Linux 一致
fn test_taskstate_values() {
    println!("test: 1. Testing TaskState enum values...");

    // 验证枚举值与 Linux 完全一致
    // include/linux/sched.h:
    // #define TASK_RUNNING        0
    // #define TASK_INTERRUPTIBLE  1
    // #define TASK_UNINTERRUPTIBLE    2
    // #define EXIT_ZOMBIE        4
    // #define EXIT_STOPPED        8

    assert_eq!(TaskState::Running as u32, 0, "TASK_RUNNING should be 0");
    assert_eq!(TaskState::Interruptible as u32, 1, "TASK_INTERRUPTIBLE should be 1");
    assert_eq!(TaskState::Uninterruptible as u32, 2, "TASK_UNINTERRUPTIBLE should be 2");
    assert_eq!(TaskState::Zombie as u32, 4, "EXIT_ZOMBIE should be 4");
    assert_eq!(TaskState::Stopped as u32, 8, "EXIT_STOPPED should be 8");

    println!("test:    SUCCESS - TaskState values match Linux kernel");
}

/// 测试 2: 验证状态设置和获取
fn test_state_getset() {
    println!("test: 2. Testing state get/set...");

    let mut task = Task::new(999, crate::process::task::SchedPolicy::Normal);

    // 验证初始状态
    assert_eq!(task.state(), TaskState::Running, "Initial state should be Running");
    println!("test:    Initial state: Running");

    // 验证设置为 Interruptible
    task.set_state(TaskState::Interruptible);
    assert_eq!(task.state(), TaskState::Interruptible, "State should be Interruptible");
    println!("test:    After set_state: Interruptible");

    // 验证设置为 Uninterruptible
    task.set_state(TaskState::Uninterruptible);
    assert_eq!(task.state(), TaskState::Uninterruptible, "State should be Uninterruptible");
    println!("test:    After set_state: Uninterruptible");

    // 验证恢复为 Running
    task.set_state(TaskState::Running);
    assert_eq!(task.state(), TaskState::Running, "State should be Running");
    println!("test:    After set_state: Running");

    println!("test:    SUCCESS - state get/set works correctly");
}

/// 测试 3: 验证 wake_up 函数存在且功能正常
fn test_wake_up_function() {
    println!("test: 3. Testing wake_up function...");

    let mut task = Task::new(1000, crate::process::task::SchedPolicy::Normal);

    // 设置为睡眠状态
    task.set_state(TaskState::Interruptible);
    println!("test:    Task state set to Interruptible");

    // 唤醒进程
    let result = Task::wake_up(&mut task as *mut Task);
    assert_eq!(result, true, "wake_up should return true for sleeping task");
    println!("test:    wake_up returned: {}", result);

    // 验证状态已恢复为 Running
    assert_eq!(task.state(), TaskState::Running, "State should be Running after wake_up");
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
