//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 测试标准 alloc crate 类型是否可用
//!
//! 用于验证 Rust nightly 是否解决了 `__rust_no_alloc_shim_is_unstable_v2` 问题

use crate::println;

pub fn test_standard_alloc() {
    println!("test: Testing standard alloc crate types...");

    // 测试 1: alloc::vec::Vec
    println!("test: 1. Testing alloc::vec::Vec...");
    {
        use alloc::vec::Vec;

        let mut vec = Vec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);

        println!("test:    Vec created with {} elements", vec.len());
        println!("test:    vec[0] = {}, vec[1] = {}, vec[2] = {}", vec[0], vec[1], vec[2]);

        if vec.len() == 3 && vec[0] == 1 && vec[1] == 2 && vec[2] == 3 {
            println!("test:    SUCCESS - Vec works correctly");
        } else {
            println!("test:    FAILED - Vec unexpected behavior");
            return;
        }
    }

    // 测试 2: alloc::boxed::Box
    println!("test: 2. Testing alloc::boxed::Box...");
    {
        use alloc::boxed::Box;

        let boxed = Box::new(42);
        println!("test:    Box created with value {}", *boxed);

        if *boxed == 42 {
            println!("test:    SUCCESS - Box works correctly");
        } else {
            println!("test:    FAILED - Box unexpected value");
            return;
        }
    }

    // 测试 3: alloc::sync::Arc
    println!("test: 3. Testing alloc::sync::Arc...");
    {
        use alloc::sync::Arc;
        use core::sync::atomic::{AtomicUsize, Ordering};

        struct TestArc {
            value: AtomicUsize,
        }

        let arc1 = Arc::new(TestArc {
            value: AtomicUsize::new(10),
        });
        println!("test:    Arc created");

        let arc2 = Arc::clone(&arc1);
        println!("test:    Arc cloned");

        arc1.value.store(20, Ordering::SeqCst);
        println!("test:    Set value to 20");

        let value = arc2.value.load(Ordering::SeqCst);
        println!("test:    Read value from cloned Arc: {}", value);

        if value == 20 {
            println!("test:    SUCCESS - Arc works correctly");
        } else {
            println!("test:    FAILED - Arc unexpected value");
            return;
        }
    }

    // 测试 4: alloc::string::String
    println!("test: 4. Testing alloc::string::String...");
    {
        use alloc::string::String;

        let mut s = String::from("Hello");
        s.push_str(" from alloc!");
        println!("test:    String created: {}", s);

        if s == "Hello from alloc!" {
            println!("test:    SUCCESS - String works correctly");
        } else {
            println!("test:    FAILED - String unexpected value");
            return;
        }
    }

    println!("test: All standard alloc crate types work correctly!");
    println!("test: This means the __rust_no_alloc_shim_is_unstable_v2 issue is resolved.");
}
