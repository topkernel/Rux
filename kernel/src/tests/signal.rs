//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Signal handling
use crate::println;
use crate::signal::{Signal, SigFlags, SigAction, SigActionKind, SignalStruct};
use core::sync::atomic::Ordering;
use super::{test_pass, test_fail, test_group_start};

pub fn test_signal() {
    test_group_start("signal handling");

    // Test 1: Signal enum values
    if Signal::SIGHUP as i32 == 1 && Signal::SIGINT as i32 == 2
        && Signal::SIGKILL as i32 == 9 && Signal::SIGTERM as i32 == 15
        && Signal::SIGCHLD as i32 == 17 && Signal::SIGSTOP as i32 == 19 {
        test_pass("Signal enum values");
    } else {
        test_fail("Signal enum values", "value mismatch");
    }

    // Test 2: SigFlags operations
    let flags1 = SigFlags::new(0);
    let flags2 = SigFlags::new(SigFlags::SA_NOCLDSTOP);
    let flags3 = SigFlags::new(SigFlags::SA_SIGINFO | SigFlags::SA_RESTART);
    if flags1.bits() == 0 && flags2.bits() == SigFlags::SA_NOCLDSTOP
        && (flags3.bits() & SigFlags::SA_SIGINFO) == SigFlags::SA_SIGINFO
        && (flags3.bits() & SigFlags::SA_RESTART) == SigFlags::SA_RESTART {
        test_pass("SigFlags operations");
    } else {
        test_fail("SigFlags operations", "flag mismatch");
    }

    // Test 3: SigAction creation
    let action = SigAction::new();
    if action.sa_flags.bits() == 0 && action.sa_mask == 0
        && action.action() == SigActionKind::Default {
        test_pass("SigAction::new()");
    } else {
        test_fail("SigAction::new()", "default mismatch");
    }

    // Test 4: SigAction::ignore()
    let ignore_action = SigAction::ignore();
    if ignore_action.action() == SigActionKind::Ignore && !ignore_action.has_handler() {
        test_pass("SigAction::ignore()");
    } else {
        test_fail("SigAction::ignore()", "ignore mismatch");
    }

    // Test 5: SigAction::handler()
    unsafe extern "C" fn custom_handler(_sig: i32) {}
    let handler_action = SigAction::handler(custom_handler, SigFlags::new(0));
    if handler_action.action() == SigActionKind::Handler && handler_action.has_handler() {
        test_pass("SigAction::handler()");
    } else {
        test_fail("SigAction::handler()", "handler mismatch");
    }

    // Test 6: SignalStruct creation
    let sig_struct = SignalStruct::new();
    let sigkill_action = sig_struct.get_action(Signal::SIGKILL as i32).unwrap();
    let sigstop_action = sig_struct.get_action(Signal::SIGSTOP as i32).unwrap();
    let sigchld_action = sig_struct.get_action(Signal::SIGCHLD as i32).unwrap();
    let sigterm_action = sig_struct.get_action(Signal::SIGTERM as i32).unwrap();
    if sigkill_action.action() == SigActionKind::Default
        && sigstop_action.action() == SigActionKind::Default
        && sigchld_action.action() == SigActionKind::Ignore
        && sigterm_action.action() == SigActionKind::Default {
        test_pass("SignalStruct defaults");
    } else {
        test_fail("SignalStruct defaults", "default action mismatch");
    }

    // Test 7: Signal mask operations
    let sig_struct = SignalStruct::new();
    if sig_struct.mask.load(Ordering::SeqCst) != 0 {
        test_fail("signal mask init", "should be 0");
        return;
    }
    sig_struct.add_mask(1);
    if !sig_struct.is_masked(1) || sig_struct.is_masked(2) {
        test_fail("signal mask add", "mask state wrong");
        return;
    }
    sig_struct.add_mask(2);
    sig_struct.remove_mask(1);
    if sig_struct.is_masked(1) || !sig_struct.is_masked(2) {
        test_fail("signal mask remove", "mask state wrong");
        return;
    }
    test_pass("signal mask operations");

    // Test 8: Signal action setting
    let mut sig_struct = SignalStruct::new();
    let ignore_action = SigAction::ignore();
    if sig_struct.set_action(Signal::SIGTERM as i32, ignore_action).is_err() {
        test_fail("set_action SIGTERM", "failed");
        return;
    }
    let sigterm_action = sig_struct.get_action(Signal::SIGTERM as i32).unwrap();
    if sigterm_action.action() != SigActionKind::Ignore {
        test_fail("set_action SIGTERM", "not ignored");
        return;
    }
    let kill_action = SigAction::ignore();
    if sig_struct.set_action(Signal::SIGKILL as i32, kill_action).is_ok() {
        test_fail("set_action SIGKILL", "should reject");
        return;
    }
    test_pass("set_action");

    // Test 9: get_action() boundary check
    let sig_struct = SignalStruct::new();
    if sig_struct.get_action(0).is_some() || sig_struct.get_action(65).is_some() {
        test_fail("get_action boundary", "should return None");
    } else {
        test_pass("get_action boundary");
    }

    // Test 10: Signal range check
    if Signal::SIGHUP as i32 >= 1 && Signal::SIGTTOU as i32 <= 31 {
        test_pass("signal range");
    } else {
        test_fail("signal range", "range invalid");
    }

    // Test 11: Realtime signal range constants
    if crate::signal::SIGRTMIN == 32 && crate::signal::SIGRTMAX == 64 {
        test_pass("realtime signal range");
    } else {
        test_fail("realtime signal range", "range mismatch");
    }

    println!("test: Signal handling testing completed.");
}
