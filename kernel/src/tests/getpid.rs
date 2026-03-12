//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! getpid/getppid system call test
use crate::println;
use crate::sched;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_getpid() {
    test_group_start("getpid/getppid");

    // Test 1: Get current process PID
    let current_pid = sched::get_current_pid();
    test_pass(&format!("getpid = {}", current_pid));

    // Test 2: Get parent process PID
    let parent_pid = sched::get_current_ppid();
    test_pass(&format!("getppid = {}", parent_pid));

    // Test 3: Verify function consistency
    let pid1 = sched::get_current_pid();
    let pid2 = sched::get_current_pid();
    if pid1 == pid2 {
        test_pass("getpid consistency");
    } else {
        test_fail("getpid consistency", "inconsistent values");
    }

    // Test 4: Verify getppid consistency
    let ppid1 = sched::get_current_ppid();
    let ppid2 = sched::get_current_ppid();
    if ppid1 == ppid2 {
        test_pass("getppid consistency");
    } else {
        test_fail("getppid consistency", "inconsistent values");
    }

    // Test 5: Test process module wrapper functions
    let wrapper_pid = crate::process::current_pid();
    let wrapper_ppid = crate::process::current_ppid();
    if wrapper_pid == current_pid && wrapper_ppid == parent_pid {
        test_pass("wrapper functions");
    } else {
        test_fail("wrapper functions", "mismatch");
    }

    println!("test: getpid/getppid testing completed.");
}
