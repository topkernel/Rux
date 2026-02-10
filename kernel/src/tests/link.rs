//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_link 测试

use crate::println;
use crate::fs::{file_link, file_unlink, file_open, file_close, file_mkdir, file_rmdir, FileFlags};

pub fn test_link() {
    println!("test: ===== Starting link() Tests =====");

    // 测试 1: link 创建硬链接
    println!("test: 1. Testing basic link...");
    test_basic_link();

    // 测试 2: link 删除任一名称不影响文件
    println!("test: 2. Testing link persistence...");
    test_link_persistence();

    // 测试 3: link 错误处理
    println!("test: 3. Testing link error cases...");
    test_link_errors();

    println!("test: ===== link() Tests Completed =====");
}

fn test_basic_link() {
    // 创建原始文件
    let oldpath = "/test_link_original.txt";

    // 先创建文件（通过打开方式）
    match file_open(oldpath, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(_) => {
            println!("test:    Created original file '{}'", oldpath);
        }
        Err(e) => {
            println!("test:    SKIPPED - could not create original file: {}", e);
            return;
        }
    }

    // 创建硬链接
    let newpath = "/test_link_hardlink.txt";
    match file_link(oldpath, newpath) {
        Ok(()) => {
            println!("test:    SUCCESS - created hard link '{}' -> '{}'", newpath, oldpath);

            // 验证两个路径都指向同一个文件
            let sb = unsafe { crate::fs::rootfs::get_rootfs() };
            if !sb.is_null() {
                let old_node = unsafe { (*sb).lookup(oldpath) };
                let new_node = unsafe { (*sb).lookup(newpath) };

                match (old_node, new_node) {
                    (Some(o), Some(n)) => {
                        // 检查 inode 号是否相同
                        if o.ino == n.ino {
                            println!("test:    SUCCESS - both paths point to same inode ({})", o.ino);
                        } else {
                            println!("test:    Note - different inodes: {} vs {}", o.ino, n.ino);
                        }
                    }
                    (None, None) => {
                        println!("test:    FAILED - neither path found");
                    }
                    (None, _) => {
                        println!("test:    FAILED - original path not found");
                    }
                    (_, None) => {
                        println!("test:    FAILED - new link not found");
                    }
                }
            }
        }
        Err(e) => {
            println!("test:    FAILED - link returned error: {}", e);
        }
    }

    // 清理
    let _ = file_unlink(oldpath);
    let _ = file_unlink(newpath);
}

fn test_link_persistence() {
    // 创建原始文件
    let oldpath = "/test_persist_original.txt";
    let linkpath1 = "/test_persist_link1.txt";
    let linkpath2 = "/test_persist_link2.txt";

    let _ = file_open(oldpath, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);

    // 创建两个硬链接
    let result1 = file_link(oldpath, linkpath1);
    let result2 = file_link(oldpath, linkpath2);

    if result1.is_ok() && result2.is_ok() {
        println!("test:    Created two hard links");

        // 删除原始文件名
        match file_unlink(oldpath) {
            Ok(()) => {
                println!("test:    Deleted original file name");

                // 验证链接仍然存在
                let sb = unsafe { crate::fs::rootfs::get_rootfs() };
                if !sb.is_null() {
                    let link1 = unsafe { (*sb).lookup(linkpath1) };
                    let link2 = unsafe { (*sb).lookup(linkpath2) };

                    if link1.is_some() && link2.is_some() {
                        println!("test:    SUCCESS - both hard links still exist after original deleted");
                    } else {
                        println!("test:    FAILED - hard links disappeared");
                    }
                }
            }
            Err(e) => {
                println!("test:    FAILED - unlink failed: {}", e);
            }
        }
    } else {
        println!("test:    SKIPPED - could not create hard links");
    }

    // 清理
    let _ = file_unlink(linkpath1);
    let _ = file_unlink(linkpath2);
    let _ = file_unlink(oldpath);
}

fn test_link_errors() {
    // 测试 1: 链接到不存在的文件
    println!("test:    Testing link to nonexistent file...");
    match file_link("/nonexistent.txt", "/newlink.txt") {
        Ok(()) => {
            println!("test:    FAILED - should not link to nonexistent file");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected nonexistent source");
        }
    }

    // 测试 2: 创建已存在的链接
    println!("test:    Testing link to existing target...");
    let file1 = "/test_link_exist1.txt";
    let file2 = "/test_link_exist2.txt";
    let _ = file_open(file1, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);
    let _ = file_open(file2, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);

    match file_link(file1, file2) {
        Ok(()) => {
            println!("test:    FAILED - should not overwrite existing file");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected existing target");
        }
    }

    // 清理
    let _ = file_unlink(file1);
    let _ = file_unlink(file2);

    // 测试 3: 为目录创建硬链接（应该失败）
    println!("test:    Testing link to directory...");
    let dirname = "/test_link_dir";
    let linkname = "/test_link_dir_link";

    let _ = file_mkdir(dirname, 0o755);

    match file_link(dirname, linkname) {
        Ok(()) => {
            println!("test:    FAILED - should not link to directory");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected directory link");
        }
    }

    // 清理
    let _ = file_rmdir(dirname);

    // 测试 4: 新链接的父目录不存在
    println!("test:    Testing link with nonexistent parent...");
    let file = "/test_link_file.txt";
    let link = "/nonexistent_dir/link.txt";
    let _ = file_open(file, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);

    match file_link(file, link) {
        Ok(()) => {
            println!("test:    FAILED - should not create link without parent directory");
        }
        Err(_) => {
            println!("test:    SUCCESS - correctly rejected nonexistent parent");
        }
    }

    // 清理
    let _ = file_unlink(file);
}
