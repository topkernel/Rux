//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: Process tree management functionality
use crate::println;
use crate::process::Task;
use crate::process::task::SchedPolicy;
use alloc::boxed::Box;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_process_tree() {
    test_group_start("process tree management");

    // Create parent process
    let mut parent_task_box = Box::new(Task::new(1, SchedPolicy::Normal));
    parent_task_box.children.init();
    parent_task_box.sibling.init();
    let parent_task = Box::leak(parent_task_box) as *mut Task;
    test_pass("create parent task (PID 1)");

    // Create child process 1
    let mut child1_box = Box::new(Task::new(2, SchedPolicy::Normal));
    child1_box.children.init();
    child1_box.sibling.init();
    let child1 = Box::leak(child1_box) as *mut Task;
    test_pass("create child1 (PID 2)");

    // Create child process 2
    let mut child2_box = Box::new(Task::new(3, SchedPolicy::Normal));
    child2_box.children.init();
    child2_box.sibling.init();
    let child2 = Box::leak(child2_box) as *mut Task;
    test_pass("create child2 (PID 3)");

    unsafe {
        // Test adding child processes
        (*parent_task).add_child(child1);
        (*parent_task).add_child(child2);
        test_pass("add children to parent");

        // Test has_children
        if (*parent_task).has_children() {
            test_pass("has_children");
        } else {
            test_fail("has_children", "should have children");
        }

        // Test first_child
        if (*parent_task).first_child().is_some() {
            test_pass("first_child");
        } else {
            test_fail("first_child", "no first child");
        }

        // Test next_sibling
        if let Some(child1_ptr) = (*parent_task).first_child() {
            if (*child1_ptr).next_sibling().is_some() {
                test_pass("next_sibling");
            } else {
                test_fail("next_sibling", "no sibling");
            }
        }

        // Test count_children
        let count = (*parent_task).count_children();
        if count == 2 {
            test_pass("count_children == 2");
        } else {
            test_fail("count_children", &format!("expected 2, got {}", count));
        }

        // Test find_child_by_pid
        if (*parent_task).find_child_by_pid(2).is_some() {
            test_pass("find_child_by_pid(2)");
        } else {
            test_fail("find_child_by_pid(2)", "not found");
        }

        // Test for_each_child
        let mut iteration_count = 0;
        (*parent_task).for_each_child(|_child| {
            iteration_count += 1;
        });
        if iteration_count == 2 {
            test_pass("for_each_child");
        } else {
            test_fail("for_each_child", &format!("expected 2, got {}", iteration_count));
        }

        // Test remove_child
        if let Some(child1_ptr) = (*parent_task).first_child() {
            (*parent_task).remove_child(child1_ptr);
            let new_count = (*parent_task).count_children();
            if new_count == 1 {
                test_pass("remove_child");
            } else {
                test_fail("remove_child", &format!("expected 1, got {}", new_count));
            }
        }

        // Test sibling after removal
        if let Some(first_child) = (*parent_task).first_child() {
            if (*first_child).next_sibling().is_none() {
                test_pass("no more siblings after removal");
            } else {
                test_fail("no more siblings", "should have no sibling");
            }
        }

        // Test list integrity
        let final_count = (*parent_task).count_children();
        if final_count == 1 {
            test_pass("list integrity");
        } else {
            test_fail("list integrity", &format!("expected 1, got {}", final_count));
        }
    }

    println!("test: process tree testing completed.");
}
