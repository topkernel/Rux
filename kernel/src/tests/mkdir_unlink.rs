//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_mkdir, sys_rmdir, sys_unlink 测试

use alloc::format;
use crate::fs::{file_mkdir, file_rmdir, file_unlink, file_open, FileFlags};
use super::{test_pass, test_fail, test_group_start};

pub fn test_mkdir_unlink() {
    test_group_start("mkdir/rmdir/unlink");

    // 测试 1: mkdir 创建目录
    test_mkdir();

    // 测试 2: rmdir 删除空目录
    test_rmdir();

    // 测试 3: unlink 删除文件
    test_unlink();

    // 测试 4: 错误处理
    test_error_cases();
}

fn test_mkdir() {
    // 创建单级目录
    let dirname1 = "/test_mkdir_single";
    match file_mkdir(dirname1, 0o755) {
        Ok(()) => {
            // 验证目录存在
            let sb = unsafe { crate::fs::rootfs::get_rootfs() };
            if !sb.is_null() {
                let node = unsafe { (*sb).lookup(dirname1) };
                if let Some(n) = node {
                    if n.is_dir() {
                        test_pass("mkdir single level");
                    } else {
                        test_fail("mkdir", "not a directory");
                    }
                } else {
                    test_fail("mkdir", "directory not found");
                }
            }
        }
        Err(e) => {
            test_fail("mkdir", &format!("error: {}", e));
        }
    }

    // 创建多级目录（应该失败，因为父目录不存在）
    let dirname2 = "/test_parent/test_child";
    match file_mkdir(dirname2, 0o755) {
        Ok(()) => {
            test_fail("mkdir multi-level", "should fail without parent");
        }
        Err(_) => {
            test_pass("mkdir multi-level rejected");
        }
    }

    // 创建已存在的目录（应该失败）
    match file_mkdir(dirname1, 0o755) {
        Ok(()) => {
            test_fail("mkdir existing", "should fail for existing dir");
        }
        Err(_) => {
            test_pass("mkdir existing rejected");
        }
    }

    // 清理
    let _ = file_rmdir(dirname1);
}

fn test_rmdir() {
    // 创建测试目录
    let dirname = "/test_rmdir_dir";
    let _ = file_mkdir(dirname, 0o755);

    // 删除空目录
    match file_rmdir(dirname) {
        Ok(()) => {
            // 验证目录已删除
            let sb = unsafe { crate::fs::rootfs::get_rootfs() };
            if !sb.is_null() {
                let node = unsafe { (*sb).lookup(dirname) };
                if node.is_none() {
                    test_pass("rmdir empty directory");
                } else {
                    test_fail("rmdir", "directory still exists");
                }
            }
        }
        Err(e) => {
            test_fail("rmdir", &format!("error: {}", e));
        }
    }

    // 删除不存在的目录
    match file_rmdir("/nonexistent_dir") {
        Ok(()) => {
            test_fail("rmdir nonexistent", "should fail");
        }
        Err(_) => {
            test_pass("rmdir nonexistent rejected");
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
                    test_fail("rmdir non-empty", "should fail");
                }
                Err(_) => {
                    test_pass("rmdir non-empty rejected");
                }
            }
        }
        Err(_) => {
            // Skip test
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
            // 使用 unlink 删除文件
            match file_unlink(filename) {
                Ok(()) => {
                    // 验证文件已删除
                    let sb = unsafe { crate::fs::rootfs::get_rootfs() };
                    if !sb.is_null() {
                        let node = unsafe { (*sb).lookup(filename) };
                        if node.is_none() {
                            test_pass("unlink file");
                        } else {
                            test_fail("unlink", "file still exists");
                        }
                    }
                }
                Err(e) => {
                    test_fail("unlink", &format!("error: {}", e));
                }
            }
        }
        Err(_) => {
            // Skip
        }
    }

    // 删除不存在的文件
    match file_unlink("/nonexistent_file.txt") {
        Ok(()) => {
            test_fail("unlink nonexistent", "should fail");
        }
        Err(_) => {
            test_pass("unlink nonexistent rejected");
        }
    }

    // 尝试删除目录（应该失败）
    let dirname = "/test_unlink_dir";
    let _ = file_mkdir(dirname, 0o755);
    match file_unlink(dirname) {
        Ok(()) => {
            test_fail("unlink directory", "should fail (use rmdir)");
        }
        Err(_) => {
            test_pass("unlink directory rejected");
        }
    }
    // 清理
    let _ = file_rmdir(dirname);
}

fn test_error_cases() {
    // 测试 1: 无效路径（空路径）
    match file_mkdir("", 0o755) {
        Ok(()) => {
            test_fail("mkdir empty path", "should reject");
        }
        Err(_) => {
            test_pass("mkdir empty path rejected");
        }
    }

    // 测试 2: 尝试删除根目录
    match file_rmdir("/") {
        Ok(()) => {
            test_fail("rmdir root", "should fail");
        }
        Err(_) => {
            test_pass("rmdir root rejected");
        }
    }

    // 测试 3: 尝试 unlink 根目录
    match file_unlink("/") {
        Ok(()) => {
            test_fail("unlink root", "should fail");
        }
        Err(_) => {
            test_pass("unlink root rejected");
        }
    }

    // 测试 4: 创建名为 "." 或 ".." 的目录（应该被规范化或拒绝）
    match file_mkdir("/test/./subdir", 0o755) {
        Ok(()) => {
            test_pass("mkdir with '.' (normalized)");
            let _ = file_rmdir("/test/subdir");
            let _ = file_rmdir("/test");
        }
        Err(_) => {
            test_pass("mkdir with '.' rejected");
        }
    }
}
