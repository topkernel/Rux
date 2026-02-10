//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! file_open() 功能测试
//!
//! 测试 VFS 层的 file_open 函数，包括文件查找、创建和标志处理

use crate::println;
use alloc::vec::Vec;
use crate::fs::vfs;
use crate::fs::file::{FileFlags, close_file_fd};
use crate::fs::rootfs;
use crate::sched;

pub fn test_file_open() {
    println!("test: Testing file_open() functionality...");

    // 先获取 RootFS 超级块
    let sb_ptr = rootfs::get_rootfs();
    if sb_ptr.is_null() {
        println!("test: RootFS not initialized!");
        return;
    }

    // 初始化当前任务的 fdtable（用于测试）
    println!("test: Initializing fdtable for testing...");
    unsafe {
        if sched::get_current_fdtable().is_none() {
            println!("test:    No fdtable - skipping fd-dependent tests");
            println!("test:    Testing file lookup and creation logic only...");

            let sb = &*sb_ptr;

            // 测试 1: 文件查找
            println!("test: 1. RootFS lookup /test_existing.txt...");
            let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
            match sb.lookup("/test_existing.txt") {
                Some(_) => println!("test:    SUCCESS - file found"),
                None => println!("test:    FAILED - file not found"),
            }

            // 测试 2: 文件不存在
            println!("test: 2. RootFS lookup /nonexistent...");
            match sb.lookup("/nonexistent") {
                Some(_) => println!("test:    UNEXPECTED SUCCESS"),
                None => println!("test:    EXPECTED FAILURE - not found"),
            }

            // 测试 3: O_CREAT 创建文件
            println!("test: 3. RootFS create_file /test_new_file...");
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => println!("test:    SUCCESS - file created"),
                Err(e) => println!("test:    FAILED - error={}", e),
            }

            // 测试 4: 验证文件已创建
            println!("test: 4. RootFS lookup /test_new_file after creation...");
            match sb.lookup("/test_new_file") {
                Some(_) => println!("test:    SUCCESS - file found"),
                None => println!("test:    FAILED - file not found"),
            }

            // 测试 5: 创建已存在的文件（应该失败）
            println!("test: 5. RootFS create_file /test_new_file (exists)...");
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => println!("test:    UNEXPECTED SUCCESS"),
                Err(e) => println!("test:    EXPECTED FAILURE - error={}", e),
            }

            println!("test: file_open() logic testing completed (no fdtable).");
            return;
        }
    }

    // 如果有 fdtable，执行完整测试
    unsafe {
        let sb = &*sb_ptr;
        // 创建 /test_existing.txt
        let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
    }

    // 测试 1: 打开已存在的文件（应该成功）
    println!("test: 1. Opening existing file /test_existing.txt...");
    match vfs::file_open("/test_existing.txt", FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            println!("test:    SUCCESS - fd={}", fd);
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(e) => {
            println!("test:    FAILED - error={}", e);
        }
    }

    // 测试 2: 打开不存在的文件（应该失败）
    println!("test: 2. Opening non-existent file /nonexistent...");
    match vfs::file_open("/nonexistent", FileFlags::O_RDONLY, 0) {
        Ok(_) => {
            println!("test:    UNEXPECTED SUCCESS");
        }
        Err(e) => {
            println!("test:    EXPECTED FAILURE - error={}", e);
        }
    }

    // 测试 3: O_CREAT - 创建新文件
    println!("test: 3. Creating new file /test_new_file...");
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            println!("test:    SUCCESS - fd={}", fd);
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(e) => {
            println!("test:    FAILED - error={}", e);
        }
    }

    // 测试 4: O_EXCL - 独占创建已存在的文件（应该失败）
    println!("test: 4. O_EXCL with existing file /test_new_file...");
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(_) => {
            println!("test:    UNEXPECTED SUCCESS (should fail with EEXIST)");
        }
        Err(e) => {
            println!("test:    EXPECTED FAILURE - error={}", e);
        }
    }

    // 测试 5: O_EXCL - 独占创建新文件（应该成功）
    println!("test: 5. O_EXCL with new file /test_excl_file...");
    match vfs::file_open("/test_excl_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            println!("test:    SUCCESS - fd={}", fd);
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(e) => {
            println!("test:    FAILED - error={}", e);
        }
    }

    println!("test: file_open() testing completed.");
}
