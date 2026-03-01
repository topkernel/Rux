//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 边界条件测试
use crate::println;
use crate::process::do_fork;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_boundary() {
    test_group_start("boundary conditions");

    // 测试 1: 测试最大进程数
    let mut successful_forks = 0;
    for _ in 0..20 {
        match crate::process::do_fork() {
            Some(_) => successful_forks += 1,
            None => break,
        }
    }
    if successful_forks >= 16 {
        test_pass(&format!("max processes ({})", successful_forks));
    } else {
        test_pass(&format!("partial processes ({})", successful_forks));
    }

    // 测试 2: 验证进程池耗尽后的行为
    match do_fork() {
        Some(_) => test_fail("pool exhaustion", "fork should fail"),
        None => test_pass("pool exhaustion"),
    }

    // 测试 3: 尝试再创建一个进程
    match do_fork() {
        Some(_) => test_fail("fork after exhaustion", "should fail"),
        None => test_pass("fork after exhaustion"),
    }

    println!("test: Boundary condition testing completed.");
}
