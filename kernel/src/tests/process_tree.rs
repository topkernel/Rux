//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：进程树管理功能
use crate::println;
use crate::process::Task;
use crate::process::task::SchedPolicy;
use alloc::boxed::Box;

pub fn test_process_tree() {
    println!("test: Testing process tree management...");

    // 创建父进程（使用堆分配避免栈溢出）
    println!("test: 1. Creating parent task (PID 1)...");
    let mut parent_task_box = Box::new(Task::new(1, SchedPolicy::Normal));
    // 重新初始化 children 和 sibling（因为 Box::new 后地址改变了）
    parent_task_box.children.init();
    parent_task_box.sibling.init();
    let parent_task = Box::leak(parent_task_box) as *mut Task;

    // 创建子进程 1（使用堆分配）
    println!("test: 2. Creating child task 1 (PID 2)...");
    let mut child1_box = Box::new(Task::new(2, SchedPolicy::Normal));
    child1_box.children.init();
    child1_box.sibling.init();
    let child1 = Box::leak(child1_box) as *mut Task;

    // 创建子进程 2（使用堆分配）
    println!("test: 3. Creating child task 2 (PID 3)...");
    let mut child2_box = Box::new(Task::new(3, SchedPolicy::Normal));
    child2_box.children.init();
    child2_box.sibling.init();
    let child2 = Box::leak(child2_box) as *mut Task;

    unsafe {
        // 测试添加子进程
        println!("test: 4. Adding child1 (PID 2) to parent...");
        (*parent_task).add_child(child1);
        println!("test:    Child1 added");

        println!("test: 5. Adding child2 (PID 3) to parent...");
        (*parent_task).add_child(child2);
        println!("test:    Child2 added");

        // 测试 has_children
        println!("test: 6. Checking if parent has children...");
        if (*parent_task).has_children() {
            println!("test:    YES - parent has children");
        } else {
            println!("test:    FAILED - parent should have children");
        }

        // 测试 first_child
        println!("test: 7. Getting first child...");
        match (*parent_task).first_child() {
            Some(_) => {
                println!("test:    SUCCESS - first child found");
            }
            None => {
                println!("test:    FAILED - no first child");
            }
        }

        // 测试 next_sibling
        println!("test: 8. Getting next sibling of first child...");
        if let Some(child1_ptr) = (*parent_task).first_child() {
            match (*child1_ptr).next_sibling() {
                Some(_) => {
                    println!("test:    SUCCESS - next sibling found");
                }
                None => {
                    println!("test:    No next sibling (unexpected)");
                }
            }
        }

        // 测试 count_children
        println!("test: 9. Counting children...");
        let count = (*parent_task).count_children();
        println!("test:    Parent has {} children", count);
        if count == 2 {
            println!("test:    SUCCESS - count is correct");
        } else {
            println!("test:    FAILED - expected 2 children, got {}", count);
        }

        // 测试 find_child_by_pid
        println!("test: 10. Finding child by PID 2...");
        match (*parent_task).find_child_by_pid(2) {
            Some(_) => {
                println!("test:    SUCCESS - found child with PID 2");
            }
            None => {
                println!("test:    FAILED - child not found");
            }
        }

        // 测试 for_each_child
        println!("test: 11. Iterating over all children...");
        let mut iteration_count = 0;
        (*parent_task).for_each_child(|_child| {
            iteration_count += 1;
            println!("test:    Child #{}", iteration_count);
        });
        if iteration_count == 2 {
            println!("test:    SUCCESS - iterated over all children");
        } else {
            println!("test:    FAILED - expected 2 iterations, got {}", iteration_count);
        }

        // 测试 remove_child
        println!("test: 12. Removing first child...");
        if let Some(child1_ptr) = (*parent_task).first_child() {
            (*parent_task).remove_child(child1_ptr);
            println!("test:    Child removed");

            // 验证删除后的计数
            let new_count = (*parent_task).count_children();
            println!("test:    Parent now has {} children", new_count);
            if new_count == 1 {
                println!("test:    SUCCESS - count is correct after removal");
            } else {
                println!("test:    FAILED - expected 1 child, got {}", new_count);
            }
        }

        // 测试 next_sibling after removal
        println!("test: 13. Testing sibling after removal...");
        if let Some(first_child) = (*parent_task).first_child() {
            match (*first_child).next_sibling() {
                Some(_) => {
                    println!("test:    UNEXPECTED - should have no more siblings");
                }
                None => {
                    println!("test:    SUCCESS - no more siblings (correct)");
                }
            }
        }

        // 测试链表完整性
        println!("test: 14. Testing list integrity...");
        let final_count = (*parent_task).count_children();
        println!("test:    Final child count: {}", final_count);
        if final_count == 1 {
            println!("test:    SUCCESS - list integrity maintained");
        } else {
            println!("test:    FAILED - expected 1 child, got {}", final_count);
        }
    }

    println!("test: process tree testing completed.");
}
