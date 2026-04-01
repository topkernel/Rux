//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::process::pid::{PID_SWAPPER, PID_INIT, PID_MAX_LIMIT, alloc_pid};
use super::{test_pass, test_fail, test_group_start};

pub fn test_pid() {
    test_group_start("pid");

    // Test 1: PID constants
    test_assert_eq!(PID_SWAPPER, 0, "PID_SWAPPER == 0");
    test_assert_eq!(PID_INIT, 1, "PID_INIT == 1");
    test_assert_eq!(PID_MAX_LIMIT, 4194304, "PID_MAX_LIMIT == 4194304");

    // Test 2: alloc_pid returns Some
    let pid = alloc_pid();
    test_assert!(pid.is_some(), "alloc_pid() returns Some");

    // Test 3: alloc_pid returns value > 1
    if let Some(p) = pid {
        test_assert!(p > 1, "alloc_pid() > 1 (after swapper and init)");
    } else {
        test_fail("alloc_pid() > 1", "got None");
    }

    // Test 4: Sequential alloc_pid returns increasing values
    let p1 = alloc_pid();
    let p2 = alloc_pid();
    if let (Some(a), Some(b)) = (p1, p2) {
        test_assert!(b > a, "sequential alloc_pid increases");
    } else {
        test_fail("sequential alloc_pid", "got None");
    }

    // Test 5: Multiple allocs all succeed
    let mut all_ok = true;
    for _ in 0..10 {
        if alloc_pid().is_none() {
            all_ok = false;
            break;
        }
    }
    test_assert!(all_ok, "10 consecutive alloc_pid() all succeed");
}
