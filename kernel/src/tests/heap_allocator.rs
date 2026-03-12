//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Heap allocator
use crate::println;
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use super::{test_pass, test_fail, test_group_start};

pub fn test_heap_allocator() {
    test_group_start("heap allocator");

    // Test 1: Box allocation
    let boxed = Box::new(42);
    if *boxed == 42 {
        test_pass("Box allocation");
    } else {
        test_fail("Box allocation", "value mismatch");
    }

    let boxed_str = Box::new("Hello");
    if *boxed_str == "Hello" {
        test_pass("Box str allocation");
    } else {
        test_fail("Box str allocation", "value mismatch");
    }

    // Test 2: Vec allocation
    let mut vec = Vec::new();
    vec.push(1);
    vec.push(2);
    vec.push(3);
    if vec.len() == 3 && vec[0] == 1 && vec[2] == 3 {
        test_pass("Vec allocation");
    } else {
        test_fail("Vec allocation", "vec content mismatch");
    }
    drop(vec);

    // Test 3: String allocation
    let s = String::from("Test string");
    if s == "Test string" && s.len() == 11 {
        test_pass("String allocation");
    } else {
        test_fail("String allocation", "string content mismatch");
    }
    drop(s);

    // Test 4: Multiple allocations
    let mut vec2 = Vec::new();
    vec2.push(10);
    vec2.push(20);
    vec2.push(30);
    if vec2.len() == 3 {
        test_pass("multiple allocations");
    } else {
        test_fail("multiple allocations", "len mismatch");
    }

    // Test 5: Allocation and deallocation
    let new_box = Box::new(888);
    if *new_box == 888 {
        test_pass("Box allocation 2");
    } else {
        test_fail("Box allocation 2", "value mismatch");
    }

    println!("test: Heap allocator testing completed.");
}
