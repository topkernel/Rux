//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_link 测试

use alloc::format;
use crate::fs::{file_link, file_unlink, file_open, file_close, file_mkdir, file_rmdir, FileFlags};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_link() {
    test_group_start("link");

    // 测试 1: link 创建硬链接
    test_basic_link();

    // 测试 2: link 删除任一名称不影响文件
    test_link_persistence();

    // 测试 3: link 错误处理
    test_link_errors();
}

fn test_basic_link() {
    // 创建原始文件
    let oldpath = "/test_link_original.txt";

    // 先创建文件（通过打开方式）
    match file_open(oldpath, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(fd) => {
            let _ = file_close(fd);

            // 创建硬链接
            let newpath = "/test_link_hardlink.txt";
            match file_link(oldpath, newpath) {
                Ok(()) => {
                    // 验证两个路径都指向同一个文件
                    let sb = unsafe { crate::fs::rootfs::get_rootfs() };
                    if !sb.is_null() {
                        let old_node = unsafe { (*sb).lookup(oldpath) };
                        let new_node = unsafe { (*sb).lookup(newpath) };

                        match (old_node, new_node) {
                            (Some(o), Some(n)) => {
                                // 检查 inode 号是否相同
                                if o.ino == n.ino {
                                    test_pass("link same inode");
                                } else {
                                    test_fail("link", "different inodes");
                                }
                            }
                            _ => {
                                test_fail("link", "path not found");
                            }
                        }
                    }
                }
                Err(e) => {
                    test_fail("link", &format!("error: {}", e));
                }
            }

            // 清理
            let _ = file_unlink(oldpath);
            let _ = file_unlink(newpath);
        }
        Err(_) => {
            test_skip("basic link", "cannot create file");
        }
    }
}

fn test_link_persistence() {
    // 创建原始文件
    let oldpath = "/test_persist_original.txt";
    let linkpath1 = "/test_persist_link1.txt";
    let linkpath2 = "/test_persist_link2.txt";

    let fd = file_open(oldpath, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);
    if fd.is_err() {
        test_skip("link persistence", "cannot create file");
        return;
    }
    let _ = file_close(fd.unwrap());

    // 创建两个硬链接
    let result1 = file_link(oldpath, linkpath1);
    let result2 = file_link(oldpath, linkpath2);

    if result1.is_ok() && result2.is_ok() {
        // 删除原始文件名
        match file_unlink(oldpath) {
            Ok(()) => {
                // 验证链接仍然存在
                let sb = unsafe { crate::fs::rootfs::get_rootfs() };
                if !sb.is_null() {
                    let link1 = unsafe { (*sb).lookup(linkpath1) };
                    let link2 = unsafe { (*sb).lookup(linkpath2) };

                    if link1.is_some() && link2.is_some() {
                        test_pass("link persistence after unlink");
                    } else {
                        test_fail("link persistence", "links disappeared");
                    }
                }
            }
            Err(e) => {
                test_fail("link persistence", &format!("unlink error: {}", e));
            }
        }
    } else {
        test_skip("link persistence", "cannot create links");
    }

    // 清理
    let _ = file_unlink(linkpath1);
    let _ = file_unlink(linkpath2);
    let _ = file_unlink(oldpath);
}

fn test_link_errors() {
    // 测试 1: 链接到不存在的文件
    match file_link("/nonexistent.txt", "/newlink.txt") {
        Ok(()) => {
            test_fail("link nonexistent", "should fail");
        }
        Err(_) => {
            test_pass("link nonexistent rejected");
        }
    }

    // 测试 2: 创建已存在的链接
    let file1 = "/test_link_exist1.txt";
    let file2 = "/test_link_exist2.txt";
    let fd1 = file_open(file1, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);
    let fd2 = file_open(file2, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);

    if fd1.is_ok() && fd2.is_ok() {
        let _ = file_close(fd1.unwrap());
        let _ = file_close(fd2.unwrap());

        match file_link(file1, file2) {
            Ok(()) => {
                test_fail("link existing target", "should fail");
            }
            Err(_) => {
                test_pass("link existing target rejected");
            }
        }
    }

    // 清理
    let _ = file_unlink(file1);
    let _ = file_unlink(file2);

    // 测试 3: 为目录创建硬链接（应该失败）
    let dirname = "/test_link_dir";
    let linkname = "/test_link_dir_link";

    let _ = file_mkdir(dirname, 0o755);

    match file_link(dirname, linkname) {
        Ok(()) => {
            test_fail("link directory", "should fail");
        }
        Err(_) => {
            test_pass("link directory rejected");
        }
    }

    // 清理
    let _ = file_rmdir(dirname);

    // 测试 4: 新链接的父目录不存在
    let file = "/test_link_file.txt";
    let link = "/nonexistent_dir/link.txt";
    let fd = file_open(file, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644);
    if fd.is_ok() {
        let _ = file_close(fd.unwrap());

        match file_link(file, link) {
            Ok(()) => {
                test_fail("link nonexistent parent", "should fail");
            }
            Err(_) => {
                test_pass("link nonexistent parent rejected");
            }
        }
    }

    // 清理
    let _ = file_unlink(file);
}
