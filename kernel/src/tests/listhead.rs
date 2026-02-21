//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：ListHead 双向链表功能
use crate::println;
use crate::list::ListHead;

pub fn test_listhead() {
    println!("test: Testing ListHead doubly-linked list...");

    // 测试 1: 初始化和空链表检查
    println!("test: 1. Testing init and is_empty...");
    let mut head = ListHead::new();
    head.init();
    assert!(head.is_empty(), "Empty list should return true for is_empty()");
    println!("test:    SUCCESS - is_empty works");

    // 测试 2: add_tail 添加单个节点
    println!("test: 2. Testing add_tail with single node...");
    let mut node1 = ListHead::new();
    node1.init();
    unsafe {
        node1.add_tail(&head as *const _ as *mut ListHead);
    }
    assert!(!head.is_empty(), "List with one node should not be empty");
    println!("test:    SUCCESS - single node added");

    // 测试 3: add_tail 添加多个节点
    println!("test: 3. Testing add_tail with multiple nodes...");
    let mut node2 = ListHead::new();
    node2.init();
    let mut node3 = ListHead::new();
    node3.init();
    unsafe {
        node2.add_tail(&head as *const _ as *mut ListHead);
        node3.add_tail(&head as *const _ as *mut ListHead);
    }
    println!("test:    Multiple nodes added");

    // 测试 4: for_each 遍历
    println!("test: 4. Testing for_each iteration...");
    let mut count = 0;
    unsafe {
        ListHead::for_each(&head as *const _ as *mut ListHead, |_| {
            count += 1;
        });
    }
    assert_eq!(count, 3, "Should have 3 nodes");
    println!("test:    SUCCESS - iterated over 3 nodes");

    // 测试 5: del 删除节点
    println!("test: 5. Testing del (remove node)...");
    unsafe {
        node2.del();
    }
    count = 0;
    unsafe {
        ListHead::for_each(&head as *const _ as *mut ListHead, |_| {
            count += 1;
        });
    }
    assert_eq!(count, 2, "Should have 2 nodes after removal");
    println!("test:    SUCCESS - node removed");

    // 测试 6: is_empty after删除所有节点
    println!("test: 6. Testing is_empty after removing all nodes...");
    unsafe {
        node1.del();
        node3.del();
    }
    assert!(head.is_empty(), "List should be empty after removing all nodes");
    println!("test:    SUCCESS - all nodes removed");

    println!("test: ListHead testing completed.");
}
