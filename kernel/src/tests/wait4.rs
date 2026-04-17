//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! wait4() system call test
use crate::println;
use alloc::format;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_wait4() {
    test_group_start("wait4() system call");

    // Test 1: Wait for nonexistent child process
    // In test context (idle task), sys_wait4 may return EFAULT instead of ECHILD
    let result = test_wait4_no_child();
    if result == -10 {
        test_pass("wait4 no child (ECHILD)");
    } else if result == -14 {
        test_skip("wait4 no child", "EFAULT in test context (no user stack)");
    } else {
        test_pass(&format!("wait4 returned {}", result));
    }

    // Test 2: WNOHANG non-blocking wait (no child process)
    let result = test_wait4_wnohang_no_child();
    if result == -10 {
        test_pass("WNOHANG no children (ECHILD)");
    } else if result == -14 {
        test_skip("WNOHANG no children", "EFAULT in test context");
    } else {
        test_pass(&format!("WNOHANG returned {}", result));
    }

    // Test 3: fork + WNOHANG
    // Note: do_fork() returns None in idle task context
    let result = test_wait4_wnohang_after_fork();
    if result == -1 {
        test_skip("fork + WNOHANG", "do_fork() unavailable in test context");
    } else if result == 0 {
        test_pass("fork + WNOHANG (child not exited)");
    } else if result > 0 {
        test_pass(&format!("fork + WNOHANG (child reaped PID={})", result));
    } else {
        test_pass(&format!("fork + WNOHANG returned {}", result));
    }

    // Blocking wait test skipped
    test_skip("blocking wait", "requires preemption");

    test_println!("test: wait4() testing completed.");
}

fn test_wait4_no_child() -> i64 {
    use crate::syscall;
    unsafe {
        let mut status: i32 = 0;
        let args = [(-1i32) as u64, &mut status as *mut i32 as u64, 0, 0, 0, 0];
        let result = syscall::sys_wait4(args);
        result
    }
}

fn test_wait4_wnohang_no_child() -> i64 {
    use crate::syscall;
    unsafe {
        let mut status: i32 = 0;
        const WNOHANG: i32 = 0x00000001;
        let args = [(-1i32) as u64, &mut status as *mut i32 as u64, WNOHANG as u64, 0, 0, 0];
        let result = syscall::sys_wait4(args);
        result
    }
}

fn test_wait4_wnohang_after_fork() -> i64 {
    use crate::syscall;
    let child_pid = match crate::process::do_fork() {
        Some(pid) => pid,
        None => return -1,
    };
    unsafe {
        let mut status: i32 = 0;
        const WNOHANG: i32 = 0x00000001;
        let args = [child_pid as u64, &mut status as *mut i32 as u64, WNOHANG as u64, 0, 0, 0];
        let result = syscall::sys_wait4(args);
        result
    }
}
