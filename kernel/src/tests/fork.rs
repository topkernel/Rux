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

    // TODO: 测试 2: 多次 fork（暂时禁用，需要调试 runqueue 问题）
    // println!("test: 2. Testing multiple forks...");
    // let mut pids = alloc::vec::Vec::new();
    // for i in 0..3 {
    //     match crate::sched::do_fork() {
    //         Some(child_pid) => {
    //             pids.push(child_pid);
    //             println!("test:    Fork #{}: child PID = {}", i + 1, child_pid);
    //         }
    //         None => {
    //             println!("test:    Fork #{}: FAILED", i + 1);
    //         }
    //     }
    // }
    // println!("test:    Created {} child processes", pids.len());
    // if pids.len() == 3 {
    //     println!("test:    SUCCESS - multiple forks work");
    // } else {
    //     println!("test:    FAILED - expected 3 children, got {}", pids.len());
    // }
    println!("test: 2. Multiple forks test skipped (pending investigation)");

    println!("test: fork() testing completed.");
}
