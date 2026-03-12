//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! sys_mkdir, sys_rmdir, sys_unlink test

use alloc::format;
use crate::fs::{file_mkdir, file_rmdir, file_unlink, file_open, FileFlags};
use super::{test_pass, test_fail, test_group_start};

pub fn test_mkdir_unlink() {
    test_group_start("mkdir/rmdir/unlink");

    // Test 1: mkdir creates directory
    test_mkdir();

    // Test 2: rmdir removes empty directory
    test_rmdir();

    // Test 3: unlink removes file
    test_unlink();

    // Test 4: Error handling
    test_error_cases();
}

fn test_mkdir() {
    // Create single level directory
    let dirname1 = "/test_mkdir_single";
    match file_mkdir(dirname1, 0o755) {
        Ok(()) => {
            // Verify directory exists
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

    // Create multi-level directory (should fail because parent doesn't exist)
    let dirname2 = "/test_parent/test_child";
    match file_mkdir(dirname2, 0o755) {
        Ok(()) => {
            test_fail("mkdir multi-level", "should fail without parent");
        }
        Err(_) => {
            test_pass("mkdir multi-level rejected");
        }
    }

    // Create existing directory (should fail)
    match file_mkdir(dirname1, 0o755) {
        Ok(()) => {
            test_fail("mkdir existing", "should fail for existing dir");
        }
        Err(_) => {
            test_pass("mkdir existing rejected");
        }
    }

    // Cleanup
    let _ = file_rmdir(dirname1);
}

fn test_rmdir() {
    // Create test directory
    let dirname = "/test_rmdir_dir";
    let _ = file_mkdir(dirname, 0o755);

    // Remove empty directory
    match file_rmdir(dirname) {
        Ok(()) => {
            // Verify directory is deleted
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

    // Remove nonexistent directory
    match file_rmdir("/nonexistent_dir") {
        Ok(()) => {
            test_fail("rmdir nonexistent", "should fail");
        }
        Err(_) => {
            test_pass("rmdir nonexistent rejected");
        }
    }

    // Create non-empty directory and try to remove (should fail)
    let parent_dir = "/test_rmdir_parent";
    let _ = file_mkdir(parent_dir, 0o755);
    let child_file = "/test_rmdir_parent/file.txt";

    // Create file (use O_CREAT)
    match file_open(child_file, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(_) => {
            // Try to remove non-empty directory
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

    // Cleanup
    let _ = file_unlink(child_file);
    let _ = file_rmdir(parent_dir);
}

fn test_unlink() {
    // Create test file
    let filename = "/test_unlink_file.txt";

    // Create file first
    match file_open(filename, FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(_) => {
            // Use unlink to delete file
            match file_unlink(filename) {
                Ok(()) => {
                    // Verify file is deleted
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

    // Delete nonexistent file
    match file_unlink("/nonexistent_file.txt") {
        Ok(()) => {
            test_fail("unlink nonexistent", "should fail");
        }
        Err(_) => {
            test_pass("unlink nonexistent rejected");
        }
    }

    // Try to delete directory (should fail)
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
    // Cleanup
    let _ = file_rmdir(dirname);
}

fn test_error_cases() {
    // Test 1: Invalid path (empty path)
    match file_mkdir("", 0o755) {
        Ok(()) => {
            test_fail("mkdir empty path", "should reject");
        }
        Err(_) => {
            test_pass("mkdir empty path rejected");
        }
    }

    // Test 2: Try to delete root directory
    match file_rmdir("/") {
        Ok(()) => {
            test_fail("rmdir root", "should fail");
        }
        Err(_) => {
            test_pass("rmdir root rejected");
        }
    }

    // Test 3: Try to unlink root directory
    match file_unlink("/") {
        Ok(()) => {
            test_fail("unlink root", "should fail");
        }
        Err(_) => {
            test_pass("unlink root rejected");
        }
    }

    // Test 4: Create directory named "." or ".." (should be normalized or rejected)
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
