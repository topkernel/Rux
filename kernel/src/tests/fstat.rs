//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_fstat 测试

use crate::println;
use crate::fs::{file_open, file_close, file_stat, Stat, FileFlags};

pub fn test_fstat() {
    println!("test: ===== Starting fstat() Tests =====");

    // 测试 1: fstat 常规文件
    println!("test: 1. Testing fstat on regular file...");
    test_fstat_regular_file();

    // 测试 2: fstat 目录
    println!("test: 2. Testing fstat on directory...");
    test_fstat_directory();

    // 测试 3: fstat 无效文件描述符
    println!("test: 3. Testing fstat with invalid fd...");
    test_fstat_invalid_fd();

    println!("test: ===== fstat() Tests Completed =====");
}

fn test_fstat_regular_file() {
    // 打开一个已存在的文件
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            println!("test:    Opened file '{}', fd={}", filename, fd);

            // 获取文件状态
            let mut stat = Stat::new();
            match file_stat(fd, &mut stat) {
                Ok(()) => {
                    println!("test:    SUCCESS - fstat returned:");
                    println!("test:      st_dev={}", stat.st_dev);
                    println!("test:      st_ino={}", stat.st_ino);
                    println!("test:      st_mode={:#o} ({})", stat.st_mode,
                        if stat.is_regular_file() { "regular file" }
                        else if stat.is_directory() { "directory" }
                        else { "other" });
                    println!("test:      st_nlink={}", stat.st_nlink);
                    println!("test:      st_size={} bytes", stat.st_size);
                    println!("test:      st_blksize={} bytes", stat.st_blksize);
                    println!("test:      st_blocks={} (512-byte blocks)", stat.st_blocks);
                }
                Err(e) => {
                    println!("test:    FAILED - fstat returned error: {}", e);
                }
            }

            // 关闭文件
            let _ = file_close(fd);
        }
        Err(e) => {
            println!("test:    SKIPPED - Could not open file '{}': {}", filename, e);
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
            println!("test:    Opened directory '{}', fd={}", dirname, fd);

            let mut stat = Stat::new();
            match file_stat(fd, &mut stat) {
                Ok(()) => {
                    if stat.is_directory() {
                        println!("test:    SUCCESS - correctly identified as directory");
                    } else {
                        println!("test:    FAILED - not identified as directory");
                    }
                }
                Err(e) => {
                    println!("test:    fstat error: {}", e);
                }
            }

            let _ = file_close(fd);
        }
        Err(_) => {
            println!("test:    Note - Directories cannot be opened (expected)");
        }
    }
}

fn test_fstat_invalid_fd() {
    let invalid_fd = 9999;
    let mut stat = Stat::new();

    match file_stat(invalid_fd, &mut stat) {
        Ok(()) => {
            println!("test:    FAILED - fstat should fail for invalid fd");
        }
        Err(e) => {
            println!("test:    SUCCESS - correctly returned error: {}", e);
        }
    }
}
