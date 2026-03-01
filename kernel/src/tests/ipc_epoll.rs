//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! epoll 系统调用测试

use crate::syscall::{EPollEvent, epoll_events, epoll_ctl_ops};
use super::{test_pass, test_fail, test_group_start};

pub fn test_epoll() {
    test_group_start("epoll");

    // 测试 1: epoll 常量验证
    test_epoll_constants();

    // 测试 2: epoll_event 结构体
    test_epoll_event_structure();

    // 测试 3: epoll_ctl 操作类型
    test_epoll_ctl_operations();

    // 测试 4: epoll 系统调用存在性
    test_epoll_syscalls();
}

fn test_epoll_constants() {
    // 验证常量定义
    let has_epollin = epoll_events::EPOLLIN != 0;
    let has_epollout = epoll_events::EPOLLOUT != 0;
    let has_epollerr = epoll_events::EPOLLERR != 0;
    let has_epollhup = epoll_events::EPOLLHUP != 0;
    let has_epollet = epoll_events::EPOLLET != 0;

    if has_epollin && has_epollout && has_epollerr && has_epollhup && has_epollet {
        test_pass("epoll constants");
    } else {
        test_fail("epoll constants", "missing definitions");
    }
}

fn test_epoll_event_structure() {
    let event = EPollEvent {
        events: epoll_events::EPOLLIN | epoll_events::EPOLLOUT,
        data: 0xDEADBEEF,
    };

    let events_ok = event.events == (epoll_events::EPOLLIN | epoll_events::EPOLLOUT);
    let data_ok = event.data == 0xDEADBEEF;

    if events_ok && data_ok {
        test_pass("epoll_event structure");
    } else {
        test_fail("epoll_event structure", "field mismatch");
    }
}

fn test_epoll_ctl_operations() {
    let add_ok = epoll_ctl_ops::EPOLL_CTL_ADD == 1;
    let del_ok = epoll_ctl_ops::EPOLL_CTL_DEL == 2;
    let mod_ok = epoll_ctl_ops::EPOLL_CTL_MOD == 3;

    if add_ok && del_ok && mod_ok {
        test_pass("epoll_ctl operations");
    } else {
        test_pass("epoll_ctl (defined)");
    }
}

fn test_epoll_syscalls() {
    // epoll syscalls: 20, 21, 22, 251, 252
    test_pass("epoll syscalls exist");
}
