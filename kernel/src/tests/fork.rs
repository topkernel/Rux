//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! fork() 系统调用测试
//!
//! 测试进程创建功能

use crate::println;

pub fn test_fork() {
    println!("test: Testing fork() system call...");

    // 测试 1: 基本 fork 功能
    println!("test: 1. Testing basic fork...");
    match crate::sched::do_fork() {
        Some(child_pid) => {
            println!("test:    Fork successful, child PID = {}", child_pid);
            println!("test:    Parent process returned with PID = {}", child_pid);
            if child_pid > 0 {
                println!("test:    SUCCESS - parent process returns child PID");
            } else {
                println!("test:    FAILED - parent should return positive PID");
            }
        }
        None => {
            println!("test:    FAILED - fork returned None");
        }
    }

    // 测试 2: 多次 fork（启用调试）
    println!("test: 2. Testing multiple forks...");
    println!("test:    Attempting to create 3 child processes...");
    for i in 0..3 {
        println!("test:    Fork attempt #{}...", i + 1);
        match crate::sched::do_fork() {
            Some(child_pid) => {
                println!("test:    Fork #{}: child PID = {}", i + 1, child_pid);
            }
            None => {
                println!("test:    Fork #{}: FAILED - returned None", i + 1);
            }
        }
    }
    println!("test:    Multiple fork test completed");

    println!("test: fork() testing completed.");
}
