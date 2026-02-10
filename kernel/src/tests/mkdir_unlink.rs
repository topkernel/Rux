//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_mkdir, sys_rmdir, sys_unlink 测试

use crate::println;
use crate::fs::{file_mkdir, file_rmdir, file_unlink, file_open, FileFlags};

pub fn test_mkdir_unlink() {
    println!("test: ===== Starting mkdir/rmdir/unlink() Tests =====");

    // 测试 1: mkdir 创建目录
    println!("test: 1. Testing mkdir...");
    test_mkdir();

    // 测试 2: rmdir 删除空目录
    println!("test: 2. Testing rmdir...");
    test_rmdir();

    // 测试 3: unlink 删除文件
    println!("test: 3. Testing unlink...");
    test_unlink();

    // 测试 4: 错误处理
    println!("test: 4. Testing error cases...");
    test_error_cases();

    println!("test: ===== mkdir/rmdir/unlink() Tests Completed =====");
}

fn test_mkdir() {
    // 创建单级目录
    let dirname1 = "/test_mkdir_single";
    match file_mkdir(dirname1, 0o755) {
        Ok(()) => {
            println!("test:    Created single-level directory '{}'", dirname1);

            // 验证目录存在
            let sb = unsafe { crate::fs::rootfs::get_rootfs() };
            if !sb.is_null() {
                let node = unsafe { (*sb).lookup(dirname1) };
                if let Some(n) = node {
                    if n.is_dir() {
                        println!("test:    SUCCESS - directory verified");
                    } else {
                        println!("test:    FAILED - not a directory");
                    }
                } else {
                    println!("test:    FAILED - directory not found");
                }
            }
        }
        Err(e) => {
            println!("test:    FAILED - mkdir returned error: {}", e);
        }
    }

    // 创建多级目录（应该失败，因为父目录不存在）
    let dirname2 = "/test_parent/test_child";
    match file_mkdir(dirname2, 0o755) {
        Ok(()) => {
            println!("test:    UNEXPECTED - multi-level mkdir should fail");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected multi-level mkdir (no parent)");
        }
    }

    // 创建已存在的目录（应该失败）
    match file_mkdir(dirname1, 0o755) {
        Ok(()) => {
            println!("test:    FAILED - should not create existing directory");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected existing directory");
        }
    }
}

fn test_rmdir() {
    // 创建测试目录
    let dirname = "/test_rmdir_dir";
    let _ = file_mkdir(dirname, 0o755);

    // 删除空目录
    match file_rmdir(dirname) {
        Ok(()) => {
            println!("test:    SUCCESS - rmdir removed empty directory");

            // 验证目录已删除
            let sb = unsafe { crate::fs::rootfs::get_rootfs() };
            if !sb.is_null() {
                let node = unsafe { (*sb).lookup(dirname) };
                if node.is_none() {
                    println!("test:    SUCCESS - directory confirmed deleted");
                } else {
                    println!("test:    FAILED - directory still exists");
                }
            }
        }
        Err(e) => {
            println!("test:    FAILED - rmdir returned error: {}", e);
        }
    }

    // 删除不存在的目录
    match file_rmdir("/nonexistent_dir") {
        Ok(()) => {
            println!("test:    FAILED - should not delete nonexistent directory");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected nonexistent directory");
        }
    }

    // 创建非空目录并尝试删除（应该失败）
    let parent_dir = "/test_rmdir_parent";
    let _ = file_mkdir(parent_dir, 0o755);
    let child_file = "/test_rmdir_parent/file.txt";

    // 创建文件（使用 O_CREAT）
    match file_open(child_file, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(_) => {
            // 尝试删除非空目录
            match file_rmdir(parent_dir) {
                Ok(()) => {
                    println!("test:    FAILED - should not delete non-empty directory");
                }
                Err(_) => {
                    println!("test:    SUCCESS - correctly rejected non-empty directory");
                }
            }
        }
        Err(_) => {
            println!("test:    Note - could not create test file for non-empty test");
        }
    }

    // 清理
    let _ = file_unlink(child_file);
    let _ = file_rmdir(parent_dir);
}

fn test_unlink() {
    // 创建测试文件
    let filename = "/test_unlink_file.txt";

    // 先创建文件
    match file_open(filename, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(_) => {
            println!("test:    Created test file '{}'", filename);

            // 使用 unlink 删除文件
            match file_unlink(filename) {
                Ok(()) => {
                    println!("test:    SUCCESS - unlink removed file");

                    // 验证文件已删除
                    let sb = unsafe { crate::fs::rootfs::get_rootfs() };
                    if !sb.is_null() {
                        let node = unsafe { (*sb).lookup(filename) };
                        if node.is_none() {
                            println!("test:    SUCCESS - file confirmed deleted");
                        } else {
                            println!("test:    FAILED - file still exists");
                        }
                    }
                }
                Err(e) => {
                    println!("test:    FAILED - unlink returned error: {}", e);
                }
            }
        }
        Err(e) => {
            println!("test:    SKIPPED - could not create test file: {}", e);
        }
    }

    // 删除不存在的文件
    match file_unlink("/nonexistent_file.txt") {
        Ok(()) => {
            println!("test:    FAILED - should not delete nonexistent file");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected nonexistent file");
        }
    }

    // 尝试删除目录（应该失败）
    let dirname = "/test_unlink_dir";
    let _ = file_mkdir(dirname, 0o755);
    match file_unlink(dirname) {
        Ok(()) => {
            println!("test:    FAILED - unlink should not remove directories");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected directory (use rmdir)");
        }
    }
    // 清理
    let _ = file_rmdir(dirname);
}

fn test_error_cases() {
    // 测试 1: 无效路径（空路径）
    println!("test:    Testing empty path...");
    match file_mkdir("", 0o755) {
        Ok(()) => {
            println!("test:    FAILED - should reject empty path");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected empty path");
        }
    }

    // 测试 2: 尝试删除根目录
    println!("test:    Testing rmdir on root...");
    match file_rmdir("/") {
        Ok(()) => {
            println!("test:    FAILED - should not remove root directory");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected root directory removal");
        }
    }

    // 测试 3: 尝试 unlink 根目录
    println!("test:    Testing unlink on root...");
    match file_unlink("/") {
        Ok(()) => {
            println!("test:    FAILED - should not unlink root directory");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected root directory unlink");
        }
    }

    // 测试 4: 创建名为 "." 或 ".." 的目录（应该被规范化或拒绝）
    println!("test:    Testing mkdir with '.' in path...");
    match file_mkdir("/test/./subdir", 0o755) {
        Ok(()) => {
            println!("test:    Note - mkdir with '.' was accepted (normalized)");
            let _ = file_rmdir("/test/subdir");
            let _ = file_rmdir("/test");
        }
        Err(_) => {
            println!("test:    Note - mkdir with '.' was rejected");
        }
    }
}
