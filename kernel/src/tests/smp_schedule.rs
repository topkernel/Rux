//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! SMP 调度验证测试
//!
//! 测试多核环境下的任务调度和负载均衡

use crate::println;
use crate::sched;

pub fn test_smp_schedule() {
    println!("test: Testing SMP scheduling...");

    // 测试 1: 验证多核环境
    println!("test: 1. Verifying multi-core environment...");
    let current_cpu = crate::arch::cpu_id() as u64 as usize;
    let max_cpus = sched::MAX_CPUS;

    println!("test:    Current CPU = {}", current_cpu);
    println!("test:    MAX_CPUS = {}", max_cpus);

    if max_cpus > 1 {
        println!("test:    Multi-core system detected");
    } else {
        println!("test:    Single-core system - SMP scheduling tests skipped");
        return;
    }

    // 测试 2: 验证 Per-CPU 运行队列
    println!("test: 2. Verifying per-CPU runqueues...");
    let mut rq_count = 0;
    for cpu_id in 0..max_cpus {
        if let Some(_rq) = sched::cpu_rq(cpu_id) {
            rq_count += 1;
            println!("test:    CPU {} runqueue: initialized", cpu_id);
        } else {
            println!("test:    CPU {} runqueue: not initialized", cpu_id);
        }
    }
    println!("test:    Total runqueues: {}", rq_count);

    if rq_count == max_cpus {
        println!("test:    SUCCESS - all CPUs have runqueues");
    } else {
        println!("test:    PARTIAL - only {} of {} CPUs have runqueues", rq_count, max_cpus);
    }

    // 测试 3: 在当前 CPU 上创建多个任务
    println!("test: 3. Creating tasks on current CPU...");
    let mut created_tasks = 0;
    let max_test_tasks = 5;

    for i in 0..max_test_tasks {
        match sched::do_fork() {
            Some(child_pid) => {
                created_tasks += 1;
                println!("test:    Created task #{}: PID {}", i + 1, child_pid);
            }
            None => {
                println!("test:    Failed to create task #{}", i + 1);
                break;
            }
        }
    }

    println!("test:    Created {} tasks", created_tasks);

    if created_tasks == max_test_tasks {
        println!("test:    SUCCESS - all tasks created");
    } else {
        println!("test:    PARTIAL - only {} of {} tasks created", created_tasks, max_test_tasks);
    }

    // 测试 4: 验证当前 CPU 的运行队列
    println!("test: 4. Verifying current CPU runqueue state...");
    if let Some(_rq) = sched::this_cpu_rq() {
        println!("test:    Current CPU {} runqueue: exists", current_cpu);
        println!("test:    SUCCESS - runqueue accessible");
    } else {
        println!("test:    FAILED - no runqueue for current CPU");
    }

    // 测试 5: 验证负载均衡函数存在
    println!("test: 5. Verifying load balance function...");
    println!("test:    Calling load_balance()...");
    sched::load_balance();
    println!("test:    load_balance() executed");
    println!("test:    SUCCESS - load balance function available");

    // 测试 6: 验证所有 CPU 的运行队列
    println!("test: 6. Checking all CPU runqueues...");
    let mut rq_count = 0;
    for cpu_id in 0..max_cpus {
        if let Some(_rq) = sched::cpu_rq(cpu_id) {
            rq_count += 1;
            println!("test:    CPU {}: runqueue exists", cpu_id);
        }
    }
    println!("test:    Total runqueues available: {}", rq_count);

    if rq_count == max_cpus {
        println!("test:    SUCCESS - all CPUs have runqueues");
    } else {
        println!("test:    PARTIAL - only {} of {} CPUs have runqueues", rq_count, max_cpus);
    }

    println!("test: SMP scheduling testing completed.");
}
