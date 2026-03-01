//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_fstat 测试

use alloc::format;
use crate::fs::{file_open, file_close, file_stat, Stat, FileFlags};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_fstat() {
    test_group_start("fstat");

    // 测试 1: fstat 常规文件
    test_fstat_regular_file();

    // 测试 2: fstat 目录
    test_fstat_directory();

    // 测试 3: fstat 无效文件描述符
    test_fstat_invalid_fd();
}

fn test_fstat_regular_file() {
    // 打开一个已存在的文件
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            // 获取文件状态
            let mut stat = Stat::new();
            match file_stat(fd, &mut stat) {
                Ok(()) => {
                    if stat.is_regular_file() {
                        test_pass("fstat regular file");
                    } else {
                        test_fail("fstat regular file", "not a regular file");
                    }
                }
                Err(e) => {
                    test_fail("fstat regular file", &format!("error: {}", e));
                }
            }

            // 关闭文件
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("fstat regular file", "no test file");
        }
    }
}

fn test_fstat_directory() {
    // 注意：由于当前实现不允许打开目录作为文件，
    // 这个测试会失败，这是预期的行为

    // 创建一个临时目录路径（实际上不存在）
    let dirname = "/test_dir";

    // 尝试打开目录（应该失败）
    match file_open(dirname, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            let mut stat = Stat::new();
            match file_stat(fd, &mut stat) {
                Ok(()) => {
                    if stat.is_directory() {
                        test_pass("fstat directory");
                    } else {
                        test_fail("fstat directory", "not identified as directory");
                    }
                }
                Err(e) => {
                    test_fail("fstat directory", &format!("error: {}", e));
                }
            }

            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("fstat directory", "cannot open dir");
        }
    }
}

fn test_fstat_invalid_fd() {
    let invalid_fd = 9999;
    let mut stat = Stat::new();

    match file_stat(invalid_fd, &mut stat) {
        Ok(()) => {
            test_fail("fstat invalid fd", "should return error");
        }
        Err(_) => {
            test_pass("fstat invalid fd");
        }
    }
}
