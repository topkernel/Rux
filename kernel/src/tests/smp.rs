//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: SMP multi-core startup
use crate::println;
use crate::arch::riscv64::smp;
use crate::config::MAX_CPUS;
use alloc::format;
use super::{test_pass, test_group_start};

pub fn test_smp() {
    // Get current hart info
    let hart_id = smp::cpu_id();
    let is_boot = smp::is_boot_hart();
    let max_cpus = MAX_CPUS;

    // Only run full tests on boot hart
    if is_boot {
        test_group_start("SMP multi-core startup");

        test_pass(&format!("boot hart detected (hart {})", hart_id));
        test_pass(&format!("hart ID = {}", hart_id));
        test_pass(&format!("MAX_CPUS = {}", max_cpus));

        if max_cpus > 1 {
            test_pass("multi-core system supported");
        } else {
            test_pass("single-core system");
        }

        test_println!("test: SMP testing completed on boot hart {}.", hart_id);
    } else {
        // Secondary harts only print basic info
        test_println!("test: [Hart {}] Secondary hart running", hart_id);
    }
}
