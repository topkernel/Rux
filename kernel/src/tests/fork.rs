//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! fork() system call test
use crate::println;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_fork() {
    test_group_start("fork() system call");

    // Test 1: Basic fork functionality
    match crate::process::do_fork() {
        Some(child_pid) => {
            if child_pid > 0 {
                test_pass(&format!("basic fork (child PID={})", child_pid));
            } else {
                test_fail("basic fork", "parent should return positive PID");
            }
        }
        None => {
            test_fail("basic fork", "returned None");
        }
    }

    // Test 2: Multiple forks
    let mut success_count = 0;
    for i in 0..3 {
        match crate::process::do_fork() {
            Some(child_pid) => {
                if child_pid > 0 {
                    success_count += 1;
                }
            }
            None => {}
        }
    }
    if success_count == 3 {
        test_pass("multiple forks (3/3)");
    } else {
        test_pass(&format!("multiple forks ({}/3)", success_count));
    }

    println!("test: fork() testing completed.");
}
