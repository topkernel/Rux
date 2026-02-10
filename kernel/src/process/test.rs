//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 进程管理测试

use crate::println;
use crate::sched;

pub fn test_fork() {
    println!("test_fork: creating new process...");

    match sched::do_fork() {
        Some(pid) => {
            println!("test_fork: successfully created process with PID {}", pid);

            // 获取当前PID
            let current_pid = sched::get_current_pid();
            println!("test_fork: current PID = {}", current_pid);

            // 测试调度
            println!("test_fork: testing scheduler...");
            sched::schedule();
            println!("test_fork: schedule completed");

            // 再次获取PID（应该已经切换）
            let new_pid = sched::get_current_pid();
            println!("test_fork: after schedule, PID = {}", new_pid);
        }
        None => {
            println!("test_fork: failed to create process");
        }
    }

    println!("test_fork: completed");
}
