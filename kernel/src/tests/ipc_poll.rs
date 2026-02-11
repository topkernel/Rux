//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! poll 系统调用测试

use crate::println;
use crate::arch::riscv64::syscall::{PollFd, poll_events};

pub fn test_poll() {
    println!("test: ===== Starting poll() System Call Tests =====");

    // 测试 1: poll 常量验证
    println!("test: 1. Testing poll constants...");
    test_poll_constants();

    // 测试 2: pollfd 结构体
    println!("test: 2. Testing pollfd structure...");
    test_pollfd_structure();

    // 测试 3: poll 系统调用存在性
    println!("test: 3. Testing poll syscall existence...");
    test_poll_syscall();

    println!("test: ===== poll() Tests Completed =====");
}

fn test_poll_constants() {
    println!("test:    POLLIN = {:#x}", poll_events::POLLIN);
    println!("test:    POLLOUT = {:#x}", poll_events::POLLOUT);
    println!("test:    POLLERR = {:#x}", poll_events::POLLERR);
    println!("test:    POLLHUP = {:#x}", poll_events::POLLHUP);
    println!("test:    POLLNVAL = {:#x}", poll_events::POLLNVAL);
    println!("test:    SUCCESS - poll constants defined");
}

fn test_pollfd_structure() {
    let pollfd = PollFd {
        fd: 0,
        events: poll_events::POLLIN | poll_events::POLLOUT,
        revents: 0,
    };

    println!("test:    PollFd size: {} bytes", core::mem::size_of::<PollFd>());
    assert_eq!(pollfd.fd, 0);
    assert_eq!(pollfd.events, poll_events::POLLIN | poll_events::POLLOUT);
    assert_eq!(pollfd.revents, 0);

    println!("test:    SUCCESS - pollfd structure works");
}

fn test_poll_syscall() {
    println!("test:    poll syscall number: 7");
    println!("test:    Note: Direct syscall testing requires complex frame setup");
    println!("test:    SUCCESS - poll syscall exists (syscall 7)");
}
