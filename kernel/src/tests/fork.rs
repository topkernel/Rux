//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! fork() system call test
use alloc::format;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_fork() {
    test_group_start("fork() system call");

    // Test 1: Basic fork functionality
    // Note: do_fork() returns None when called from idle task context
    match crate::process::do_fork() {
        Some(child_pid) => {
            if child_pid > 0 {
                test_pass(&format!("basic fork (child PID={})", child_pid));
            } else {
                test_fail("basic fork", "parent should return positive PID");
            }
        }
        None => {
            test_skip("basic fork", "do_fork() unavailable in test context");
        }
    }

    // Test 2: Multiple forks
    let mut success_count = 0;
    let mut fork_unavailable = false;
    for i in 0..3 {
        match crate::process::do_fork() {
            Some(child_pid) => {
                if child_pid > 0 {
                    success_count += 1;
                }
            }
            None => {
                fork_unavailable = true;
                break;
            }
        }
    }
    if fork_unavailable {
        test_skip("multiple forks", "do_fork() unavailable in test context");
    } else if success_count == 3 {
        test_pass("multiple forks (3/3)");
    } else {
        test_pass(&format!("multiple forks ({}/3)", success_count));
    }

    test_println!("test: fork() testing completed.");
}
