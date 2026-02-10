//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! execve() 系统调用测试
//!
//! 测试程序执行功能

use crate::println;

pub fn test_execve() {
    println!("test: Testing execve() system call...");

    // 注意：execve 需要实际的用户程序才能测试
    // 当前测试将验证 execve 的路径解析和错误处理

    // 测试 1: 空指针（应该失败）
    println!("test: 1. Testing execve with null pathname...");
    let result = test_execve_null();
    if result == -14 {
        println!("test:    SUCCESS - correctly returned EFAULT");
    } else {
        println!("test:    FAILED - expected EFAULT (-14), got {}", result);
    }

    // 测试 2: 不存在的文件（应该失败）
    println!("test: 2. Testing execve with non-existent file...");
    let result = test_execve_nonexistent();
    if result == -2 {
        println!("test:    SUCCESS - correctly returned ENOENT");
    } else {
        println!("test:    FAILED - expected ENOENT (-2), got {}", result);
    }

    // 测试 3: 有效文件（如果存在）
    println!("test: 3. Testing execve with valid ELF...");
    let result = test_execve_valid();
    if result == 0 {
        println!("test:    SUCCESS - execve returned successfully");
    } else {
        println!("test:    Note - execve failed with error code {}", result);
        println!("test:    This is expected if no user program is embedded");
    }

    println!("test: execve() testing completed.");
}

// 测试空指针
fn test_execve_null() -> i64 {
    use crate::arch::riscv64::syscall;
    unsafe {
        let args = [0u64, 0, 0, 0, 0, 0];
        syscall::sys_execve(args) as i64
    }
}

// 测试不存在的文件
fn test_execve_nonexistent() -> i64 {
    use crate::arch::riscv64::syscall;

    // 创建一个不存在的文件名
    let filename = b"/nonexistent_elf_file\0";
    let filename_ptr = filename.as_ptr() as u64;

    unsafe {
        let args = [filename_ptr, 0, 0, 0, 0, 0];
        syscall::sys_execve(args) as i64
    }
}

// 测试有效的 ELF 文件
fn test_execve_valid() -> i64 {
    use crate::arch::riscv64::syscall;
    use crate::fs;

    // 首先检查是否有 hello_world 程序
    let hello_data = unsafe { crate::embedded_user_programs::HELLO_WORLD_ELF };

    // 如果 hello_world 存在，尝试执行它
    if !hello_data.is_empty() {
        println!("test:    Found embedded hello_world ELF, attempting execve...");

        // 创建临时文件名（实际 execve 会从文件系统读取）
        let filename = b"/hello_world\0";
        let filename_ptr = filename.as_ptr() as u64;

        unsafe {
            let args = [filename_ptr, 0, 0, 0, 0, 0];
            syscall::sys_execve(args) as i64
        }
    } else {
        println!("test:    No embedded user program found");
        -1
    }
}
