//! getpid/getppid 系统调用测试
//!
//! 测试进程 ID 获取功能

use crate::println;
use crate::sched;

pub fn test_getpid() {
    println!("test: Testing getpid/getppid...");

    // 测试 1: 获取当前进程 PID
    println!("test: 1. Getting current PID...");
    let current_pid = sched::get_current_pid();
    println!("test:    Current PID = {}", current_pid);
    println!("test:    SUCCESS - PID retrieved successfully");

    // 测试 2: 获取父进程 PID
    println!("test: 2. Getting parent PID...");
    let parent_pid = sched::get_current_ppid();
    println!("test:    Parent PID = {}", parent_pid);
    println!("test:    SUCCESS - PPID retrieved successfully");

    // 测试 3: 验证函数一致性
    println!("test: 3. Verifying function consistency...");
    let pid1 = sched::get_current_pid();
    let pid2 = sched::get_current_pid();

    if pid1 == pid2 {
        println!("test:    SUCCESS - getpid returns consistent value");
    } else {
        println!("test:    FAILED - getpid returns inconsistent values");
    }

    // 测试 4: 验证 getppid 一致性
    let ppid1 = sched::get_current_ppid();
    let ppid2 = sched::get_current_ppid();

    if ppid1 == ppid2 {
        println!("test:    SUCCESS - getppid returns consistent value");
    } else {
        println!("test:    FAILED - getppid returns inconsistent values");
    }

    // 测试 5: 测试 process 模块包装函数
    println!("test: 5. Testing process module wrapper functions...");
    let wrapper_pid = crate::process::current_pid();
    let wrapper_ppid = crate::process::current_ppid();

    println!("test:    wrapper PID = {}, wrapper PPID = {}", wrapper_pid, wrapper_ppid);

    if wrapper_pid == current_pid && wrapper_ppid == parent_pid {
        println!("test:    SUCCESS - wrapper functions return correct values");
    } else {
        println!("test:    FAILED - wrapper functions mismatch");
    }

    println!("test: getpid/getppid testing completed.");
}
