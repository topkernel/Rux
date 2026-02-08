//! 边界条件测试
//!
//! 测试进程管理的边界情况

use crate::println;
use crate::sched;

pub fn test_boundary() {
    println!("test: Testing boundary conditions...");

    // 测试 1: 测试最大进程数（TASK_POOL_SIZE = 16）
    println!("test: 1. Testing maximum process count...");
    println!("test:    TASK_POOL_SIZE = {}", 16);

    let mut successful_forks = 0;
    let mut failed_forks = 0;

    // 尝试创建 20 个进程（超过 TASK_POOL_SIZE）
    for i in 0..20 {
        match sched::do_fork() {
            Some(child_pid) => {
                successful_forks += 1;
                if i < 5 || i >= 15 {
                    // 只打印前 5 个和后 5 个
                    println!("test:    Fork #{}: child PID = {}", i + 1, child_pid);
                }
            }
            None => {
                failed_forks += 1;
                println!("test:    Fork #{}: FAILED - pool exhausted", i + 1);
                break; // 第一次失败后停止
            }
        }
    }

    println!("test:    Total successful forks: {}", successful_forks);
    println!("test:    Total failed forks: {}", failed_forks);

    if successful_forks == 16 {
        println!("test:    SUCCESS - all {} tasks created", 16);
    } else if successful_forks < 16 {
        println!("test:    PARTIAL - only {} tasks created (expected 16)", successful_forks);
    } else {
        println!("test:    UNEXPECTED - {} tasks created (expected max 16)", successful_forks);
    }

    // 测试 2: 验证进程池耗尽后的行为
    println!("test: 2. Testing behavior after pool exhaustion...");
    match sched::do_fork() {
        Some(_) => {
            println!("test:    UNEXPECTED - fork should fail after pool exhausted");
        }
        None => {
            println!("test:    SUCCESS - fork correctly fails after pool exhausted");
        }
    }

    // 测试 3: 验证当前进程数
    println!("test: 3. Checking current process count...");
    let current_pid = sched::get_current_pid();
    println!("test:    Current PID = {}", current_pid);

    // 测试 4: 尝试再创建一个进程（应该失败）
    println!("test: 4. Attempting one more fork (should fail)...");
    match sched::do_fork() {
        Some(_) => {
            println!("test:    FAILED - fork should fail");
        }
        None => {
            println!("test:    SUCCESS - fork correctly failed");
        }
    }

    println!("test: Boundary condition testing completed.");
}
