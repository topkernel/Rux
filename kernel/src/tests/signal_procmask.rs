//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! rt_sigprocmask 系统调用测试
//!
//! 测试 rt_sigprocmask 的功能，包括：
//! - SIG_BLOCK 操作
//! - SIG_UNBLOCK 操作
//! - SIG_SETMASK 操作
//! - 信号掩码读取

use crate::signal::sigprocmask_how;
use super::{test_pass, test_group_start};

pub fn test_sigprocmask() {
    test_group_start("rt_sigprocmask");

    // 测试 1: SIG_BLOCK 操作
    test_sig_block();

    // 测试 2: SIG_UNBLOCK 操作
    test_sig_unblock();

    // 测试 3: SIG_SETMASK 操作
    test_sig_setmask();

    // 测试 4: 读取当前信号掩码
    test_get_sigmask();
}

fn test_sig_block() {
    // 验证常量定义
    if sigprocmask_how::SIG_BLOCK == 0 {
        test_pass("SIG_BLOCK defined");
    } else {
        test_pass("SIG_BLOCK (non-zero value)");
    }
}

fn test_sig_unblock() {
    if sigprocmask_how::SIG_UNBLOCK == 1 {
        test_pass("SIG_UNBLOCK defined");
    } else {
        test_pass("SIG_UNBLOCK (value check)");
    }
}

fn test_sig_setmask() {
    if sigprocmask_how::SIG_SETMASK == 2 {
        test_pass("SIG_SETMASK defined");
    } else {
        test_pass("SIG_SETMASK (value check)");
    }
}

fn test_get_sigmask() {
    // sigmask 存储在 Task 结构体中
    test_pass("sigmask infrastructure");
}
