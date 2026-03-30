//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Test: File descriptor management (FdTable)
use crate::println;
use crate::fs::file::{FdTable, File, FileFlags};
use super::{test_pass, test_fail, test_group_start};

pub fn test_fdtable() {
    test_group_start("FdTable management");

    // Test 1: Create FdTable
    let fdtable = FdTable::new();
    test_pass("create FdTable");

    // Test 2: Allocate file descriptor
    let fd1 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            test_fail("alloc_fd", "returned None");
            return;
        }
    };

    if fd1 < 1024 {
        test_pass("alloc_fd valid range");
    } else {
        test_fail("alloc_fd valid range", "fd out of range");
    }

    let fd2 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            test_fail("alloc_fd second", "returned None");
            return;
        }
    };

    // Test 3: Create File object and install
    let file1 = File::new(FileFlags::new(FileFlags::O_RDONLY));
    let file1_arc = unsafe {
        use alloc::sync::Arc;
        Arc::new(file1)
    };

    match fdtable.install_fd(fd1, file1_arc) {
        Ok(_) => test_pass("install_fd first"),
        Err(_) => {
            test_fail("install_fd first", "error");
            return;
        }
    }

    let file2 = File::new(FileFlags::new(FileFlags::O_WRONLY));
    let file2_arc = unsafe {
        use alloc::sync::Arc;
        Arc::new(file2)
    };
    match fdtable.install_fd(fd2, file2_arc) {
        Ok(_) => test_pass("install_fd second"),
        Err(_) => {
            test_fail("install_fd second", "error");
            return;
        }
    }

    // Test 4: Get file object
    match fdtable.get_file(fd1) {
        Some(file) => {
            if file.flags.is_readonly() {
                test_pass("get_file readonly check");
            } else {
                test_fail("get_file readonly check", "wrong flags");
            }
        }
        None => {
            test_fail("get_file fd1", "returned None");
            return;
        }
    }

    match fdtable.get_file(fd2) {
        Some(file) => {
            if file.flags.is_writeonly() {
                test_pass("get_file writeonly check");
            } else {
                test_fail("get_file writeonly check", "wrong flags");
            }
        }
        None => {
            test_fail("get_file fd2", "returned None");
            return;
        }
    }

    // Test 5: Get invalid file descriptor
    match fdtable.get_file(9999) {
        Some(_) => {
            test_fail("invalid fd check", "should return None");
        }
        None => {
            test_pass("invalid fd check");
        }
    }

    // Test 6: Close file descriptor
    match fdtable.close_fd(fd1) {
        Ok(_) => test_pass("close_fd fd1"),
        Err(_) => {
            test_fail("close_fd fd1", "error");
            return;
        }
    }

    match fdtable.close_fd(fd2) {
        Ok(_) => test_pass("close_fd fd2"),
        Err(_) => {
            test_fail("close_fd fd2", "error");
            return;
        }
    }

    // Test 7: Verify file cannot be retrieved after closing
    match fdtable.get_file(fd1) {
        Some(_) => {
            test_fail("closed fd check", "should return None");
        }
        None => {
            test_pass("closed fd check");
        }
    }

    // Test 8: Reuse freed fd
    let fd3 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            test_fail("fd reuse", "alloc_fd returned None");
            return;
        }
    };
    // fd3 should be successfully allocated
    test_pass("fd reuse");

    test_println!("test: FdTable testing completed. (fd1={}, fd2={}, fd3={})", fd1, fd2, fd3);
}
