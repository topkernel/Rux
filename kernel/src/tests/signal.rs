//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：信号处理
use crate::println;
use crate::signal::{Signal, SigFlags, SigAction, SigActionKind, SignalStruct};
use core::sync::atomic::Ordering;

pub fn test_signal() {
    println!("test: Testing signal handling...");

    // 测试 1: Signal 枚举值
    println!("test: 1. Testing Signal enum values...");
    assert_eq!(Signal::SIGHUP as i32, 1, "SIGHUP should be 1");
    assert_eq!(Signal::SIGINT as i32, 2, "SIGINT should be 2");
    assert_eq!(Signal::SIGKILL as i32, 9, "SIGKILL should be 9");
    assert_eq!(Signal::SIGTERM as i32, 15, "SIGTERM should be 15");
    assert_eq!(Signal::SIGCHLD as i32, 17, "SIGCHLD should be 17");
    assert_eq!(Signal::SIGSTOP as i32, 19, "SIGSTOP should be 19");
    println!("test:    SUCCESS - Signal enum values correct");

    // 测试 2: SigFlags 操作
    println!("test: 2. Testing SigFlags operations...");
    let flags1 = SigFlags::new(0);
    assert_eq!(flags1.bits(), 0, "Empty flags should be 0");

    let flags2 = SigFlags::new(SigFlags::SA_NOCLDSTOP);
    assert_eq!(flags2.bits(), SigFlags::SA_NOCLDSTOP, "SA_NOCLDSTOP flag should match");

    let flags3 = SigFlags::new(SigFlags::SA_SIGINFO | SigFlags::SA_RESTART);
    assert_eq!(flags3.bits() & SigFlags::SA_SIGINFO, SigFlags::SA_SIGINFO, "SA_SIGINFO should be set");
    assert_eq!(flags3.bits() & SigFlags::SA_RESTART, SigFlags::SA_RESTART, "SA_RESTART should be set");
    println!("test:    SUCCESS - SigFlags operations work");

    // 测试 3: SigAction 创建
    println!("test: 3. Testing SigAction creation...");
    let action = SigAction::new();
    assert_eq!(action.sa_flags.bits(), 0, "Default flags should be 0");
    assert_eq!(action.sa_mask, 0, "Default mask should be 0");
    assert_eq!(action.action(), SigActionKind::Default, "New action should be Default");
    println!("test:    SUCCESS - SigAction::new() works");

    // 测试 4: SigAction::ignore()
    println!("test: 4. Testing SigAction::ignore()...");
    let ignore_action = SigAction::ignore();
    assert_eq!(ignore_action.action(), SigActionKind::Ignore, "Ignore action should be Ignore");
    assert!(!ignore_action.has_handler(), "Ignore action should not have custom handler");
    println!("test:    SUCCESS - SigAction::ignore() works");

    // 测试 5: SigAction::handler()
    println!("test: 5. Testing SigAction::handler()...");
    unsafe extern "C" fn custom_handler(_sig: i32) {
        // Custom handler
    }
    let handler_action = SigAction::handler(custom_handler, SigFlags::new(0));
    assert_eq!(handler_action.action(), SigActionKind::Handler, "Handler action should be Handler");
    assert!(handler_action.has_handler(), "Handler action should have custom handler");
    println!("test:    SUCCESS - SigAction::handler() works");

    // 测试 6: SignalStruct 创建
    println!("test: 6. Testing SignalStruct creation...");
    let sig_struct = SignalStruct::new();

    // 检查默认信号动作
    // SIGKILL 和 SIGSTOP 应该是默认处理（不可忽略）
    let sigkill_action = sig_struct.get_action(Signal::SIGKILL as i32).unwrap();
    assert_eq!(sigkill_action.action(), SigActionKind::Default, "SIGKILL should be Default");

    let sigstop_action = sig_struct.get_action(Signal::SIGSTOP as i32).unwrap();
    assert_eq!(sigstop_action.action(), SigActionKind::Default, "SIGSTOP should be Default");

    // SIGCHLD 默认是 Ignore
    let sigchld_action = sig_struct.get_action(Signal::SIGCHLD as i32).unwrap();
    assert_eq!(sigchld_action.action(), SigActionKind::Ignore, "SIGCHLD should be Ignore by default");

    // 其他信号默认是 Default (终止)
    let sigterm_action = sig_struct.get_action(Signal::SIGTERM as i32).unwrap();
    assert_eq!(sigterm_action.action(), SigActionKind::Default, "SIGTERM should be Default");

    println!("test:    SUCCESS - SignalStruct::new() creates correct defaults");

    // 测试 7: 信号掩码操作
    println!("test: 7. Testing signal mask operations...");
    let sig_struct = SignalStruct::new();

    // 初始掩码应该为 0
    assert_eq!(sig_struct.mask.load(Ordering::SeqCst), 0, "Initial mask should be 0");

    // 添加信号到掩码
    sig_struct.add_mask(1);  // SIGHUP
    assert!(sig_struct.is_masked(1), "Signal 1 should be masked");
    assert!(!sig_struct.is_masked(2), "Signal 2 should not be masked");

    // 添加更多信号
    sig_struct.add_mask(2);  // SIGINT
    assert!(sig_struct.is_masked(2), "Signal 2 should be masked");

    // 从掩码删除信号
    sig_struct.remove_mask(1);
    assert!(!sig_struct.is_masked(1), "Signal 1 should be unmasked");
    assert!(sig_struct.is_masked(2), "Signal 2 should still be masked");

    println!("test:    SUCCESS - signal mask operations work");

    // 测试 8: 信号动作设置
    println!("test: 8. Testing set_action()...");
    let mut sig_struct = SignalStruct::new();

    // 设置 SIGTERM 的处理动作为 ignore
    let ignore_action = SigAction::ignore();
    match sig_struct.set_action(Signal::SIGTERM as i32, ignore_action) {
        Ok(_) => println!("test:    SIGTERM set to ignore"),
        Err(_) => {
            println!("test:    FAILED - set_action returned error");
            return;
        }
    }

    // 验证设置成功
    let sigterm_action = sig_struct.get_action(Signal::SIGTERM as i32).unwrap();
    assert_eq!(sigterm_action.action(), SigActionKind::Ignore, "SIGTERM should be Ignore");

    // 尝试设置 SIGKILL（应该失败）
    let kill_action = SigAction::ignore();
    match sig_struct.set_action(Signal::SIGKILL as i32, kill_action) {
        Ok(_) => {
            println!("test:    FAILED - should not allow setting SIGKILL");
            return;
        }
        Err(_) => {
            println!("test:    Correctly rejected SIGKILL modification");
        }
    }

    println!("test:    SUCCESS - set_action() works correctly");

    // 测试 9: get_action() 边界检查
    println!("test: 9. Testing get_action() boundary checks...");
    let sig_struct = SignalStruct::new();

    // 无效信号编号
    match sig_struct.get_action(0) {
        Some(_) => {
            println!("test:    FAILED - should return None for signal 0");
            return;
        }
        None => {
            println!("test:    Correctly returned None for signal 0");
        }
    }

    match sig_struct.get_action(65) {
        Some(_) => {
            println!("test:    FAILED - should return None for signal 65");
            return;
        }
        None => {
            println!("test:    Correctly returned None for signal 65");
        }
    }

    println!("test:    SUCCESS - get_action() boundary checks work");

    // 测试 10: 信号范围检查
    println!("test: 10. Testing signal range validation...");
    // 标准信号范围 1-31
    assert!(Signal::SIGHUP as i32 >= 1, "SIGHUP should be >= 1");
    assert!(Signal::SIGTTOU as i32 <= 31, "SIGTTOU should be <= 31");
    println!("test:    SUCCESS - signal ranges are valid");

    // 测试 11: 实时信号范围常量
    println!("test: 11. Testing realtime signal range...");
    assert_eq!(crate::signal::SIGRTMIN, 32, "SIGRTMIN should be 32");
    assert_eq!(crate::signal::SIGRTMAX, 64, "SIGRTMAX should be 64");
    println!("test:    SUCCESS - realtime signal range correct");

    println!("test: Signal handling testing completed.");
}
