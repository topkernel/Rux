//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Quick test - for debugging timeout issues

use super::{test_pass, test_group_start};

pub fn test_quick() {
    test_group_start("quick");

    // Test 1
    test_pass("test 1");

    // Test 2
    test_pass("test 2");
}
