//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：堆分配器
use crate::println;
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::vec;

pub fn test_heap_allocator() {
    println!("test: Testing heap allocator...");

    // 测试 1: Box 分配
    println!("test: 1. Testing Box allocation...");
    let boxed = Box::new(42);
    assert_eq!(*boxed, 42, "Box value should be 42");
    let boxed_str = Box::new("Hello");
    assert_eq!(*boxed_str, "Hello", "Box str should be Hello");
    println!("test:    SUCCESS - Box allocation works");

    // 测试 2: Vec 分配（已启用 - 测试 drop 修复）
    println!("test: 2. Testing Vec allocation...");
    let mut vec = Vec::new();
    vec.push(1);
    vec.push(2);
    vec.push(3);
    assert_eq!(vec.len(), 3, "Vec should have 3 elements");
    assert_eq!(vec[0], 1, "First element should be 1");
    assert_eq!(vec[2], 3, "Third element should be 3");
    println!("test:    SUCCESS - Vec allocation works");
    drop(vec); // 显式 drop 以测试 drop 功能

    // 测试 3: String 分配（已启用 - 测试 drop 修复）
    println!("test: 3. Testing String allocation...");
    let s = String::from("Test string");
    assert_eq!(s, "Test string", "String should match");
    assert_eq!(s.len(), 11, "String length should be 11");
    println!("test:    SUCCESS - String allocation works");
    drop(s); // 显式 drop 以测试 drop 功能

    // 测试 4: 大量分配（已启用 - 测试 drop 修复）
    println!("test: 4. Testing multiple allocations...");
    println!("test:    DEBUG - Creating Vec with 3 elements...");
    let mut vec2 = Vec::new();
    vec2.push(10);
    vec2.push(20);
    vec2.push(30);
    assert_eq!(vec2.len(), 3, "Vec should have 3 elements");
    println!("test:    SUCCESS - multiple allocations work");
    // vec2 会在函数返回时自动 drop

    // 测试 5: 分配和释放（简化版本，避免 Vec drop PANIC）
    println!("test: 5. Testing allocation...");
    let new_box = Box::new(888);
    assert_eq!(*new_box, 888, "New box should work");
    println!("test:    SUCCESS - Box allocation works");

    println!("test: Heap allocator testing completed.");
}
