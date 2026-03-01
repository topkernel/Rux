//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 测试标准 alloc crate 类型是否可用
//!
//! 用于验证 Rust nightly 是否解决了 `__rust_no_alloc_shim_is_unstable_v2` 问题

use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::string::String;
use core::sync::atomic::{AtomicUsize, Ordering};
use super::{test_pass, test_fail, test_group_start};

pub fn test_standard_alloc() {
    test_group_start("standard alloc");

    // 测试 1: alloc::vec::Vec
    test_vec();

    // 测试 2: alloc::boxed::Box
    test_box();

    // 测试 3: alloc::sync::Arc
    test_arc();

    // 测试 4: alloc::string::String
    test_string();
}

fn test_vec() {
    let mut vec = Vec::new();
    vec.push(1);
    vec.push(2);
    vec.push(3);

    if vec.len() == 3 && vec[0] == 1 && vec[1] == 2 && vec[2] == 3 {
        test_pass("Vec");
    } else {
        test_fail("Vec", "unexpected behavior");
    }
}

fn test_box() {
    let boxed = Box::new(42);

    if *boxed == 42 {
        test_pass("Box");
    } else {
        test_fail("Box", "unexpected value");
    }
}

fn test_arc() {
    struct TestArc {
        value: AtomicUsize,
    }

    let arc1 = Arc::new(TestArc {
        value: AtomicUsize::new(10),
    });

    let arc2 = Arc::clone(&arc1);

    arc1.value.store(20, Ordering::SeqCst);

    let value = arc2.value.load(Ordering::SeqCst);

    if value == 20 {
        test_pass("Arc");
    } else {
        test_fail("Arc", "unexpected value");
    }
}

fn test_string() {
    let mut s = String::from("Hello");
    s.push_str(" from alloc!");

    if s == "Hello from alloc!" {
        test_pass("String");
    } else {
        test_fail("String", "unexpected value");
    }
}
