//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 快速测试 - 用于调试超时问题

use super::{test_pass, test_group_start};

pub fn test_quick() {
    test_group_start("quick");

    // 测试 1
    test_pass("test 1");

    // 测试 2
    test_pass("test 2");
}
