//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::process::pid::{
    PID_SWAPPER, PID_INIT, PID_MAX_LIMIT, PID_MAX_DEFAULT, RESERVED_PIDS,
    alloc_pid, free_pid,
};
use super::{test_pass, test_fail, test_group_start};

pub fn test_pid() {
    test_group_start("pid");

    // Test 1: PID constants
    test_assert_eq!(PID_SWAPPER, 0, "PID_SWAPPER == 0");
    test_assert_eq!(PID_INIT, 1, "PID_INIT == 1");
    test_assert_eq!(PID_MAX_LIMIT, 4194304, "PID_MAX_LIMIT == 4194304");
    test_assert_eq!(PID_MAX_DEFAULT, 32768, "PID_MAX_DEFAULT == 32768");
    test_assert_eq!(RESERVED_PIDS, 300, "RESERVED_PIDS == 300");

    // Test 2: alloc_pid returns Some
    let pid = alloc_pid();
    test_assert!(pid.is_some(), "alloc_pid() returns Some");

    // Test 3: alloc_pid returns value >= RESERVED_PIDS
    if let Some(p) = pid {
        test_assert!(p >= RESERVED_PIDS, "alloc_pid() >= RESERVED_PIDS");
    } else {
        test_fail("alloc_pid() >= RESERVED_PIDS", "got None");
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

    // Test 6: free_pid on reserved PIDs is a safe no-op
    free_pid(0);
    free_pid(1);
    free_pid(299);
    test_pass("free_pid reserved PIDs no-op");

    // Test 7: free + realloc does not panic
    let p = alloc_pid().expect("alloc for free test");
    free_pid(p);
    test_pass("free_pid does not panic");

    // Test 8: free_pid out-of-range is safe
    free_pid(PID_MAX_DEFAULT);
    free_pid(PID_MAX_DEFAULT + 1);
    free_pid(u32::MAX);
    test_pass("free_pid out-of-range no-op");

    // Test 9: Double free is safe (defensive check)
    let p = alloc_pid().expect("alloc for double-free test");
    free_pid(p);
    free_pid(p);
    test_pass("double free_pid is safe");
}
