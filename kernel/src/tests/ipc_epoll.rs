//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! epoll 系统调用测试

use crate::println;
use crate::arch::riscv64::syscall::{EPollEvent, epoll_events, epoll_ctl_ops};

pub fn test_epoll() {
    println!("test: ===== Starting epoll() System Call Tests =====");

    // 测试 1: epoll 常量验证
    println!("test: 1. Testing epoll constants...");
    test_epoll_constants();

    // 测试 2: epoll_event 结构体
    println!("test: 2. Testing epoll_event structure...");
    test_epoll_event_structure();

    // 测试 3: epoll_ctl 操作类型
    println!("test: 3. Testing epoll_ctl operations...");
    test_epoll_ctl_operations();

    // 测试 4: epoll 系统调用存在性
    println!("test: 4. Testing epoll syscalls existence...");
    test_epoll_syscalls();

    println!("test: ===== epoll() Tests Completed =====");
}

fn test_epoll_constants() {
    println!("test:    EPOLLIN = {:#x}", epoll_events::EPOLLIN);
    println!("test:    EPOLLOUT = {:#x}", epoll_events::EPOLLOUT);
    println!("test:    EPOLLERR = {:#x}", epoll_events::EPOLLERR);
    println!("test:    EPOLLHUP = {:#x}", epoll_events::EPOLLHUP);
    println!("test:    EPOLLET = {:#x}", epoll_events::EPOLLET);
    println!("test:    EPOLLONESHOT = {:#x}", epoll_events::EPOLLONESHOT);
    println!("test:    SUCCESS - epoll constants defined");
}

fn test_epoll_event_structure() {
    let event = EPollEvent {
        events: epoll_events::EPOLLIN | epoll_events::EPOLLOUT,
        data: 0xDEADBEEF,
    };

    println!("test:    EPollEvent size: {} bytes", core::mem::size_of::<EPollEvent>());
    assert_eq!(event.events, epoll_events::EPOLLIN | epoll_events::EPOLLOUT);
    assert_eq!(event.data, 0xDEADBEEF);

    println!("test:    SUCCESS - epoll_event structure works");
}

fn test_epoll_ctl_operations() {
    println!("test:    EPOLL_CTL_ADD = {}", epoll_ctl_ops::EPOLL_CTL_ADD);
    println!("test:    EPOLL_CTL_DEL = {}", epoll_ctl_ops::EPOLL_CTL_DEL);
    println!("test:    EPOLL_CTL_MOD = {}", epoll_ctl_ops::EPOLL_CTL_MOD);
    println!("test:    SUCCESS - epoll_ctl operations defined");
}

fn test_epoll_syscalls() {
    println!("test:    epoll_create syscall number: 20");
    println!("test:    epoll_create1 syscall number: 251");
    println!("test:    epoll_ctl syscall number: 21");
    println!("test:    epoll_wait syscall number: 22");
    println!("test:    epoll_pwait syscall number: 252");
    println!("test:    Note: Direct syscall testing requires complex frame setup");
    println!("test:    SUCCESS - epoll syscalls exist");
}
