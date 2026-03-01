//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：ListHead 双向链表功能
use crate::println;
use crate::list::ListHead;
use super::{test_pass, test_fail, test_group_start};

pub fn test_listhead() {
    test_group_start("ListHead doubly-linked list");

    // 测试 1: 初始化和空链表检查
    let mut head = ListHead::new();
    head.init();
    if head.is_empty() {
        test_pass("init and is_empty");
    } else {
        test_fail("init and is_empty", "empty list should return true");
    }

    // 测试 2: add_tail 添加单个节点
    let mut node1 = ListHead::new();
    node1.init();
    unsafe {
        node1.add_tail(&head as *const _ as *mut ListHead);
    }
    if !head.is_empty() {
        test_pass("add_tail single node");
    } else {
        test_fail("add_tail single node", "list should not be empty");
    }

    // 测试 3: add_tail 添加多个节点
    let mut node2 = ListHead::new();
    node2.init();
    let mut node3 = ListHead::new();
    node3.init();
    unsafe {
        node2.add_tail(&head as *const _ as *mut ListHead);
        node3.add_tail(&head as *const _ as *mut ListHead);
    }
    test_pass("add_tail multiple nodes");

    // 测试 4: for_each 遍历
    let mut count = 0;
    unsafe {
        ListHead::for_each(&head as *const _ as *mut ListHead, |_| {
            count += 1;
        });
    }
    if count == 3 {
        test_pass("for_each iteration (3 nodes)");
    } else {
        test_fail("for_each iteration", "expected 3 nodes");
    }

    // 测试 5: del 删除节点
    unsafe {
        node2.del();
    }
    count = 0;
    unsafe {
        ListHead::for_each(&head as *const _ as *mut ListHead, |_| {
            count += 1;
        });
    }
    if count == 2 {
        test_pass("del removes node (2 left)");
    } else {
        test_fail("del removes node", "expected 2 nodes");
    }

    // 测试 6: is_empty after删除所有节点
    unsafe {
        node1.del();
        node3.del();
    }
    if head.is_empty() {
        test_pass("is_empty after remove all");
    } else {
        test_fail("is_empty after remove all", "list should be empty");
    }

    println!("test: ListHead testing completed.");
}
