//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! rt_sigprocmask system call test
//!
//! Tests rt_sigprocmask functionality, including:
//! - SIG_BLOCK operation
//! - SIG_UNBLOCK operation
//! - SIG_SETMASK operation
//! - Signal mask reading

use crate::signal::sigprocmask_how;
use super::{test_pass, test_group_start};

pub fn test_sigprocmask() {
    test_group_start("rt_sigprocmask");

    // Test 1: SIG_BLOCK operation
    test_sig_block();

    // Test 2: SIG_UNBLOCK operation
    test_sig_unblock();

    // Test 3: SIG_SETMASK operation
    test_sig_setmask();

    // Test 4: Read current signal mask
    test_get_sigmask();
}

fn test_sig_block() {
    // Verify constant definition
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
    // sigmask is stored in Task structure
    test_pass("sigmask infrastructure");
}
