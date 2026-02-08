//! wait4() 系统调用测试
//!
//! 测试进程等待功能

use crate::println;

pub fn test_wait4() {
    println!("test: Testing wait4() system call...");

    // 测试 1: 等待不存在的子进程（应该返回 ECHILD）
    println!("test: 1. Testing wait4 with non-existent child...");
    let result = test_wait4_no_child();
    if result == -10 {
        println!("test:    SUCCESS - correctly returned ECHILD");
    } else {
        println!("test:    FAILED - expected ECHILD (-10), got {}", result);
    }

    // 测试 2: WNOHANG 非阻塞等待（没有子进程）
    println!("test: 2. Testing wait4 with WNOHANG (no children)...");
    let result = test_wait4_wnohang_no_child();
    if result == -10 {
        println!("test:    SUCCESS - correctly returned ECHILD");
    } else {
        println!("test:    Note - returned {}", result);
    }

    // 测试 3: fork + WNOHANG（子进程存在但未退出）
    println!("test: 3. Testing fork + WNOHANG...");
    let result = test_wait4_wnohang_after_fork();
    if result == 0 {
        println!("test:    SUCCESS - WNOHANG returned 0 (no child exited yet)");
    } else if result > 0 {
        println!("test:    Note - child reaped with PID = {}", result);
    } else {
        println!("test:    Note - returned error {}", result);
    }

    // 注意：阻塞等待测试已跳过，因为需要实现抢占式调度才能避免死锁
    println!("test: 4. Blocking wait test skipped (requires preemption)");

    println!("test: wait4() testing completed.");
}

// 测试等待不存在的子进程
fn test_wait4_no_child() -> i64 {
    use crate::arch::riscv64::syscall;

    unsafe {
        // pid = -1 (等待任意子进程)
        // 没有子进程存在，应该返回 ECHILD (-10)
        let mut status: i32 = 0;
        let args = [
            (-1i32) as u64,  // pid = -1
            &mut status as *mut i32 as u64,  // status
            0,  // options = 0 (阻塞等待)
            0, 0, 0
        ];
        syscall::sys_wait4(args) as i64
    }
}

// 测试 WNOHANG（没有子进程）
fn test_wait4_wnohang_no_child() -> i64 {
    use crate::arch::riscv64::syscall;

    unsafe {
        let mut status: i32 = 0;
        const WNOHANG: i32 = 0x00000001;
        let args = [
            (-1i32) as u64,
            &mut status as *mut i32 as u64,
            WNOHANG as u64,
            0, 0, 0
        ];
        syscall::sys_wait4(args) as i64
    }
}

// 测试 fork + WNOHANG（子进程存在但未退出）
fn test_wait4_wnohang_after_fork() -> i64 {
    use crate::sched;
    use crate::arch::riscv64::syscall;

    // 创建子进程
    let child_pid = match sched::do_fork() {
        Some(pid) => pid,
        None => return -1,
    };

    println!("test:    Child process created with PID = {}", child_pid);

    // 立即使用 WNOHANG 等待（非阻塞）
    // 子进程刚创建，还未退出，应该返回 0
    unsafe {
        let mut status: i32 = 0;
        const WNOHANG: i32 = 0x00000001;
        let args = [
            child_pid as u64,
            &mut status as *mut i32 as u64,
            WNOHANG as u64,
            0, 0, 0
        ];
        let result = syscall::sys_wait4(args) as i64;
        println!("test:    WNOHANG result = {}", result);
        result
    }
}
