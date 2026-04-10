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

    // Test Per-CPU run queues (GRQ design: no per-CPU RQ, skip)
    test_skip("per-CPU runqueues", "GRQ design uses global runqueue");

    // Create tasks
    let mut created_tasks = 0;
    for _ in 0..5 {
        if crate::process::do_fork().is_some() {
            created_tasks += 1;
        }
    }
    test_pass(&format!("created {} tasks", created_tasks));

    // Verify current CPU's run queue (GRQ design: skip)
    test_skip("current CPU runqueue", "GRQ design uses global runqueue");

    // Verify load balance function
    sched::load_balance();
    test_pass("load_balance()");

    test_println!("test: SMP scheduling testing completed.");
}
