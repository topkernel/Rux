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
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_file_open() {
    test_group_start("file_open() functionality");

    // 先获取 RootFS 超级块
    let sb_ptr = rootfs::get_rootfs();
    if sb_ptr.is_null() {
        test_fail("RootFS initialization", "superblock is null");
        return;
    }

    // 初始化当前任务的 fdtable（用于测试）
    unsafe {
        if sched::get_current_fdtable().is_none() {
            test_skip("fdtable tests", "no fdtable available");

            let sb = &*sb_ptr;

            // 测试 1: 文件查找
            let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
            match sb.lookup("/test_existing.txt") {
                Some(_) => test_pass("RootFS lookup existing file"),
                None => test_fail("RootFS lookup existing file", "not found"),
            }

            // 测试 2: 文件不存在
            match sb.lookup("/nonexistent") {
                Some(_) => test_fail("RootFS lookup nonexistent", "should not find"),
                None => test_pass("RootFS lookup nonexistent"),
            }

            // 测试 3: O_CREAT 创建文件
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => test_pass("RootFS create_file"),
                Err(e) => test_fail("RootFS create_file", "error"),
            }

            // 测试 4: 验证文件已创建
            match sb.lookup("/test_new_file") {
                Some(_) => test_pass("RootFS verify created file"),
                None => test_fail("RootFS verify created file", "not found"),
            }

            // 测试 5: 创建已存在的文件（应该失败）
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => test_fail("RootFS create existing", "should fail"),
                Err(_) => test_pass("RootFS create existing"),
            }

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
    match vfs::file_open("/test_existing.txt", FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            test_pass("open existing file");
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(_) => {
            test_fail("open existing file", "open failed");
        }
    }

    // 测试 2: 打开不存在的文件（应该失败）
    match vfs::file_open("/nonexistent", FileFlags::O_RDONLY, 0) {
        Ok(_) => {
            test_fail("open nonexistent file", "should fail");
        }
        Err(_) => {
            test_pass("open nonexistent file");
        }
    }

    // 测试 3: O_CREAT - 创建新文件
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            test_pass("O_CREAT new file");
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(_) => {
            test_fail("O_CREAT new file", "create failed");
        }
    }

    // 测试 4: O_EXCL - 独占创建已存在的文件（应该失败）
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(_) => {
            test_fail("O_EXCL existing file", "should fail with EEXIST");
        }
        Err(_) => {
            test_pass("O_EXCL existing file");
        }
    }

    // 测试 5: O_EXCL - 独占创建新文件（应该成功）
    match vfs::file_open("/test_excl_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            test_pass("O_EXCL new file");
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(_) => {
            test_fail("O_EXCL new file", "create failed");
        }
    }

    println!("test: file_open() testing completed.");
}
