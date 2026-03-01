//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! getpid/getppid 系统调用测试
use crate::println;
use crate::sched;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_getpid() {
    test_group_start("getpid/getppid");

    // 测试 1: 获取当前进程 PID
    let current_pid = sched::get_current_pid();
    test_pass(&format!("getpid = {}", current_pid));

    // 测试 2: 获取父进程 PID
    let parent_pid = sched::get_current_ppid();
    test_pass(&format!("getppid = {}", parent_pid));

    // 测试 3: 验证函数一致性
    let pid1 = sched::get_current_pid();
    let pid2 = sched::get_current_pid();
    if pid1 == pid2 {
        test_pass("getpid consistency");
    } else {
        test_fail("getpid consistency", "inconsistent values");
    }

    // 测试 4: 验证 getppid 一致性
    let ppid1 = sched::get_current_ppid();
    let ppid2 = sched::get_current_ppid();
    if ppid1 == ppid2 {
        test_pass("getppid consistency");
    } else {
        test_fail("getppid consistency", "inconsistent values");
    }

    // 测试 5: 测试 process 模块包装函数
    let wrapper_pid = crate::process::current_pid();
    let wrapper_ppid = crate::process::current_ppid();
    if wrapper_pid == current_pid && wrapper_ppid == parent_pid {
        test_pass("wrapper functions");
    } else {
        test_fail("wrapper functions", "mismatch");
    }

    println!("test: getpid/getppid testing completed.");
}
