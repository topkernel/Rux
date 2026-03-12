//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! file_open() functionality test
//!
//! Tests VFS layer file_open function, including file lookup, creation, and flag handling

use crate::println;
use alloc::vec::Vec;
use crate::fs::vfs;
use crate::fs::file::{FileFlags, close_file_fd};
use crate::fs::rootfs;
use crate::sched;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_file_open() {
    test_group_start("file_open() functionality");

    // First get RootFS superblock
    let sb_ptr = rootfs::get_rootfs();
    if sb_ptr.is_null() {
        test_fail("RootFS initialization", "superblock is null");
        return;
    }

    // Initialize current task's fdtable (for testing)
    unsafe {
        if sched::get_current_fdtable().is_none() {
            test_skip("fdtable tests", "no fdtable available");

            let sb = &*sb_ptr;

            // Test 1: File lookup
            let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
            match sb.lookup("/test_existing.txt") {
                Some(_) => test_pass("RootFS lookup existing file"),
                None => test_fail("RootFS lookup existing file", "not found"),
            }

            // Test 2: File does not exist
            match sb.lookup("/nonexistent") {
                Some(_) => test_fail("RootFS lookup nonexistent", "should not find"),
                None => test_pass("RootFS lookup nonexistent"),
            }

            // Test 3: O_CREAT create file
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => test_pass("RootFS create_file"),
                Err(e) => test_fail("RootFS create_file", "error"),
            }

            // Test 4: Verify file was created
            match sb.lookup("/test_new_file") {
                Some(_) => test_pass("RootFS verify created file"),
                None => test_fail("RootFS verify created file", "not found"),
            }

            // Test 5: Create existing file (should fail)
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => test_fail("RootFS create existing", "should fail"),
                Err(_) => test_pass("RootFS create existing"),
            }

            return;
        }
    }

    // If fdtable is available, run full tests
    unsafe {
        let sb = &*sb_ptr;
        // Create /test_existing.txt
        let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
    }

    // Test 1: Open existing file (should succeed)
    match vfs::file_open("/test_existing.txt", FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            test_pass("open existing file");
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(_) => {
            test_fail("open existing file", "open failed");
        }
    }

    // Test 2: Open nonexistent file (should fail)
    match vfs::file_open("/nonexistent", FileFlags::O_RDONLY, 0) {
        Ok(_) => {
            test_fail("open nonexistent file", "should fail");
        }
        Err(_) => {
            test_pass("open nonexistent file");
        }
    }

    // Test 3: O_CREAT - create new file
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            test_pass("O_CREAT new file");
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(_) => {
            test_fail("O_CREAT new file", "create failed");
        }
    }

    // Test 4: O_EXCL - exclusive create existing file (should fail)
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(_) => {
            test_fail("O_EXCL existing file", "should fail with EEXIST");
        }
        Err(_) => {
            test_pass("O_EXCL existing file");
        }
    }

    // Test 5: O_EXCL - exclusive create new file (should succeed)
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
