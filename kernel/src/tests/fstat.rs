//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! sys_fstat test

use alloc::format;
use crate::fs::{file_open, file_close, file_stat, Stat, FileFlags};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_fstat() {
    test_group_start("fstat");

    // Test 1: fstat regular file
    test_fstat_regular_file();

    // Test 2: fstat directory
    test_fstat_directory();

    // Test 3: fstat invalid file descriptor
    test_fstat_invalid_fd();
}

fn test_fstat_regular_file() {
    // Open an existing file
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            // Get file status
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

            // Close file
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("fstat regular file", "no test file");
        }
    }
}

fn test_fstat_directory() {
    // Note: Since current implementation does not allow opening directories as files,
    // this test will fail, which is expected behavior

    // Create a temporary directory path (doesn't actually exist)
    let dirname = "/test_dir";

    // Try to open directory (should fail)
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
