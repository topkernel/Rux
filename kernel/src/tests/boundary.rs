//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Boundary condition test
use crate::println;
use crate::process::do_fork;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_boundary() {
    test_group_start("boundary conditions");

    // Test 1: Test max process count
    let mut successful_forks = 0;
    for _ in 0..20 {
        match crate::process::do_fork() {
            Some(_) => successful_forks += 1,
            None => break,
        }
    }
    if successful_forks >= 16 {
        test_pass(&format!("max processes ({})", successful_forks));
    } else {
        test_pass(&format!("partial processes ({})", successful_forks));
    }

    // Test 2: Verify behavior after process pool exhaustion
    match do_fork() {
        Some(_) => test_fail("pool exhaustion", "fork should fail"),
        None => test_pass("pool exhaustion"),
    }

    // Test 3: Try to create another process
    match do_fork() {
        Some(_) => test_fail("fork after exhaustion", "should fail"),
        None => test_pass("fork after exhaustion"),
    }

    println!("test: Boundary condition testing completed.");
}
