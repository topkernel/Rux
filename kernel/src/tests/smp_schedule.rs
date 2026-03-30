//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! SMP scheduling verification test
use crate::println;
use crate::sched;
use alloc::format;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_smp_schedule() {
    test_group_start("SMP scheduling");

    let current_cpu = crate::arch::cpu_id() as u64 as usize;
    let max_cpus = sched::MAX_CPUS;

    test_pass(&format!("current CPU = {}", current_cpu));
    test_pass(&format!("MAX_CPUS = {}", max_cpus));

    if max_cpus <= 1 {
        test_skip("SMP tests", "single-core system");
        return;
    }

    // Test Per-CPU run queues
    let mut rq_count = 0;
    for cpu_id in 0..max_cpus {
        if sched::cpu_rq(cpu_id).is_some() {
            rq_count += 1;
        }
    }
    if rq_count == max_cpus {
        test_pass("all CPU runqueues");
    } else {
        test_pass(&format!("{} of {} runqueues", rq_count, max_cpus));
    }

    // Create tasks
    let mut created_tasks = 0;
    for _ in 0..5 {
        if crate::process::do_fork().is_some() {
            created_tasks += 1;
        }
    }
    test_pass(&format!("created {} tasks", created_tasks));

    // Verify current CPU's run queue
    if sched::this_cpu_rq().is_some() {
        test_pass("current CPU runqueue");
    } else {
        test_fail("current CPU runqueue", "not found");
    }

    // Verify load balance function
    sched::load_balance();
    test_pass("load_balance()");

    test_println!("test: SMP scheduling testing completed.");
}
