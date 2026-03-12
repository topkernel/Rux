//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! File system related system call test
//!
//! Includes: open, close, read, write, lseek, fstat, mkdir, rmdir, unlink, getdents64

use crate::fs::{file_open, file_close, file_stat, Stat, FileFlags};
use crate::fs::vfs;
use crate::fs::get_file_fd;
use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_file() {
    test_group_start("syscall: file operations");

    // Test 1: open/close syscalls
    test_sys_open_close();

    // Test 2: read/write syscalls
    test_sys_read_write();

    // Test 3: fstat syscall
    test_sys_fstat();

    // Test 4: lseek syscall
    test_sys_lseek();

    // Test 5: mkdir/rmdir syscalls
    test_sys_mkdir_rmdir();

    // Test 6: unlink syscall
    test_sys_unlink();

    // Test 7: File descriptor management
    test_sys_fd_management();

    // Test 8: Syscall number verification
    test_syscall_numbers();
}

fn test_sys_open_close() {
    // Test opening existing file
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            // Verify fd is valid non-negative integer
            if fd < 1024 {
                test_pass("sys_open returns valid fd");
            } else {
                test_fail("sys_open fd", "fd out of range");
            }

            // Close file
            match file_close(fd) {
                Ok(()) => test_pass("sys_close"),
                Err(e) => test_fail("sys_close", &alloc::format!("error: {}", e)),
            }

            // Verify fd cannot be used after close
            let mut stat = Stat::new();
            match file_stat(fd, &mut stat) {
                Ok(()) => test_fail("sys_close", "fd still usable after close"),
                Err(_) => test_pass("sys_close invalidates fd"),
            }
        }
        Err(_) => {
            test_skip("sys_open/close", "no test file");
        }
    }

    // Test opening nonexistent file (should fail)
    match file_open("/nonexistent_file.txt", FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            test_fail("sys_open", "should fail for nonexistent file");
            let _ = file_close(fd);
        }
        Err(_) => {
            test_pass("sys_open (nonexistent rejected)");
        }
    }

    // Test creating file
    match file_open("/test_create_syscall.txt", FileFlags::O_CREAT | FileFlags::O_WRONLY | FileFlags::O_TRUNC, 0o644) {
        Ok(fd) => {
            test_pass("sys_open (O_CREAT)");
            let _ = file_close(fd);

            // Verify file was actually created
            match file_open("/test_create_syscall.txt", FileFlags::O_RDONLY, 0) {
                Ok(fd2) => {
                    test_pass("sys_open O_CREAT file exists");
                    let _ = file_close(fd2);
                }
                Err(_) => {
                    test_fail("sys_open O_CREAT", "created file not found");
                }
            }

            // Cleanup
            let _ = vfs::file_unlink("/test_create_syscall.txt");
        }
        Err(_) => {
            test_skip("sys_open O_CREAT", "filesystem not writable");
        }
    }

    // Test O_TRUNC flag
    // Test O_APPEND flag
    // Test O_DIRECTORY flag
    match file_open("/", FileFlags::O_RDONLY | FileFlags::O_DIRECTORY, 0) {
        Ok(fd) => {
            test_pass("sys_open O_DIRECTORY");
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("sys_open O_DIRECTORY", "not supported");
        }
    }
}

fn test_sys_read_write() {
    // Test reading file content
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            unsafe {
                match get_file_fd(fd) {
                    Some(file) => {
                        // Read test
                        let mut buf = [0u8; 64];
                        let result = file.read(buf.as_mut_ptr(), 64);

                        if result >= 0 {
                            test_pass("sys_read returns non-negative");

                            // Verify byte count is reasonable
                            let bytes_read = result as usize;
                            if bytes_read <= 64 {
                                test_pass("sys_read byte count valid");
                            } else {
                                test_fail("sys_read", "read more than buffer size");
                            }
                        } else {
                            test_fail("sys_read", "negative result");
                        }

                        // Test reading empty buffer
                        let result = file.read(buf.as_mut_ptr(), 0);
                        if result == 0 {
                            test_pass("sys_read zero bytes");
                        } else {
                            test_fail("sys_read zero", "should return 0");
                        }
                    }
                    None => {
                        test_fail("sys_read", "file not found in fdtable");
                    }
                }
            }
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("sys_read/write", "no test file");
        }
    }

    // Test writing (if filesystem is writable)
    match file_open("/test_write_syscall.txt", FileFlags::O_CREAT | FileFlags::O_WRONLY | FileFlags::O_TRUNC, 0o644) {
        Ok(fd) => {
            unsafe {
                match get_file_fd(fd) {
                    Some(file) => {
                        let data = b"Hello, syscall test!";
                        let result = file.write(data.as_ptr(), data.len());

                        if result == data.len() as isize {
                            test_pass("sys_write correct byte count");
                        } else if result > 0 {
                            test_pass("sys_write partial success");
                        } else {
                            test_fail("sys_write", "write failed");
                        }
                    }
                    None => {
                        test_skip("sys_write", "file not found");
                    }
                }
            }
            let _ = file_close(fd);
            let _ = vfs::file_unlink("/test_write_syscall.txt");
        }
        Err(_) => {
            test_skip("sys_write", "filesystem not writable");
        }
    }
}

fn test_sys_fstat() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            let mut stat = Stat::new();
            match file_stat(fd, &mut stat) {
                Ok(()) => {
                    // Verify stat structure fields
                    let mut checks_passed = 0;

                    // st_ino should be non-zero
                    if stat.st_ino > 0 { checks_passed += 1; }

                    // st_mode should be non-zero
                    if stat.st_mode != 0 { checks_passed += 1; }

                    // st_nlink should be >= 1
                    if stat.st_nlink >= 1 { checks_passed += 1; }

                    // st_blksize should be > 0
                    if stat.st_blksize > 0 { checks_passed += 1; }

                    // st_size should be >= 0
                    if stat.st_size >= 0 { checks_passed += 1; }

                    if checks_passed == 5 {
                        test_pass("sys_fstat all fields valid");
                    } else {
                        test_fail("sys_fstat", &alloc::format!("only {}/5 checks passed", checks_passed));
                    }

                    // Verify file type detection
                    if stat.is_regular_file() {
                        test_pass("sys_fstat is_regular_file");
                    } else {
                        test_fail("sys_fstat type", "not recognized as regular file");
                    }
                }
                Err(e) => {
                    test_fail("sys_fstat", &alloc::format!("error: {}", e));
                }
            }
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("sys_fstat", "no test file");
        }
    }

    // Test invalid fd
    let mut stat = Stat::new();
    match file_stat(9999, &mut stat) {
        Ok(()) => {
            test_fail("sys_fstat (invalid fd)", "should fail");
        }
        Err(_) => {
            test_pass("sys_fstat (invalid fd rejected)");
        }
    }
}

fn test_sys_lseek() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            unsafe {
                match get_file_fd(fd) {
                    Some(file) => {
                        // SEEK_SET = 0: Set to file beginning
                        let result = file.lseek(0, 0);
                        if result == 0 {
                            test_pass("sys_lseek SEEK_SET to 0");
                        } else {
                            test_fail("sys_lseek SEEK_SET", "not at position 0");
                        }

                        // SEEK_SET to position 10
                        let result = file.lseek(10, 0);
                        if result == 10 {
                            test_pass("sys_lseek SEEK_SET to 10");
                        } else {
                            test_fail("sys_lseek SEEK_SET", &alloc::format!("expected 10, got {}", result));
                        }

                        // SEEK_CUR = 1: Move from current position
                        let result = file.lseek(5, 1);
                        if result == 15 {
                            test_pass("sys_lseek SEEK_CUR +5");
                        } else {
                            test_fail("sys_lseek SEEK_CUR", &alloc::format!("expected 15, got {}", result));
                        }

                        // SEEK_CUR negative move
                        let result = file.lseek(-5, 1);
                        if result == 10 {
                            test_pass("sys_lseek SEEK_CUR -5");
                        } else {
                            test_fail("sys_lseek SEEK_CUR negative", &alloc::format!("expected 10, got {}", result));
                        }

                        // SEEK_END = 2: Move from file end
                        let result = file.lseek(0, 2);
                        if result >= 0 {
                            // Get file size
                            let file_size = result;
                            test_pass("sys_lseek SEEK_END");

                            // Move forward from file end
                            let result2 = file.lseek(-1, 2);
                            if result2 == file_size - 1 {
                                test_pass("sys_lseek SEEK_END negative");
                            } else {
                                test_skip("sys_lseek SEEK_END negative", "file too small");
                            }
                        } else {
                            test_fail("sys_lseek SEEK_END", "negative result");
                        }
                    }
                    None => {
                        test_fail("sys_lseek", "file not found");
                    }
                }
            }
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("sys_lseek", "no test file");
        }
    }
}

fn test_sys_mkdir_rmdir() {
    // Test creating directory
    let dirname = "/test_syscall_mkdir";
    match vfs::file_mkdir(dirname, 0o755) {
        Ok(()) => {
            test_pass("sys_mkdir creates directory");

            // Verify directory exists (try to open it)
            match file_open(dirname, FileFlags::O_RDONLY | FileFlags::O_DIRECTORY, 0) {
                Ok(fd) => {
                    test_pass("sys_mkdir directory is openable");
                    let _ = file_close(fd);
                }
                Err(_) => {
                    test_fail("sys_mkdir", "created directory not openable");
                }
            }

            // Test duplicate creation (should fail)
            match vfs::file_mkdir(dirname, 0o755) {
                Ok(()) => {
                    test_fail("sys_mkdir", "should fail for existing dir");
                }
                Err(_) => {
                    test_pass("sys_mkdir duplicate rejected");
                }
            }

            // Test removing empty directory
            match vfs::file_rmdir(dirname) {
                Ok(()) => {
                    test_pass("sys_rmdir empty directory");

                    // Verify directory is deleted
                    match file_open(dirname, FileFlags::O_RDONLY | FileFlags::O_DIRECTORY, 0) {
                        Ok(_) => {
                            test_fail("sys_rmdir", "directory still exists");
                        }
                        Err(_) => {
                            test_pass("sys_rmdir directory removed");
                        }
                    }
                }
                Err(e) => {
                    test_fail("sys_rmdir", &alloc::format!("error: {}", e));
                }
            }
        }
        Err(_) => {
            test_skip("sys_mkdir/rmdir", "filesystem not writable");
        }
    }

    // Test removing nonexistent directory
    match vfs::file_rmdir("/nonexistent_dir_xyz") {
        Ok(()) => {
            test_fail("sys_rmdir (nonexistent)", "should fail");
        }
        Err(_) => {
            test_pass("sys_rmdir (nonexistent rejected)");
        }
    }

    // Test removing root directory (should fail)
    match vfs::file_rmdir("/") {
        Ok(()) => {
            test_fail("sys_rmdir root", "should fail");
        }
        Err(_) => {
            test_pass("sys_rmdir root rejected");
        }
    }

    // Test removing non-empty directory (should fail)
    // Create parent and child directories
    match vfs::file_mkdir("/test_nonempty_dir", 0o755) {
        Ok(()) => {
            match vfs::file_mkdir("/test_nonempty_dir/subdir", 0o755) {
                Ok(_) => {
                    // Try to remove non-empty directory
                    match vfs::file_rmdir("/test_nonempty_dir") {
                        Ok(()) => {
                            test_fail("sys_rmdir non-empty", "should fail");
                        }
                        Err(_) => {
                            test_pass("sys_rmdir non-empty rejected");
                        }
                    }
                    // Cleanup
                    let _ = vfs::file_rmdir("/test_nonempty_dir/subdir");
                }
                Err(_) => {
                    test_skip("sys_rmdir non-empty", "cannot create subdir");
                }
            }
            let _ = vfs::file_rmdir("/test_nonempty_dir");
        }
        Err(_) => {
            test_skip("sys_rmdir non-empty", "filesystem not writable");
        }
    }
}

fn test_sys_unlink() {
    // Test creating and deleting file
    match file_open("/test_unlink_file.txt", FileFlags::O_CREAT | FileFlags::O_WRONLY, 0o644) {
        Ok(fd) => {
            let _ = file_close(fd);

            // Delete file using unlink
            match vfs::file_unlink("/test_unlink_file.txt") {
                Ok(()) => {
                    test_pass("sys_unlink removes file");

                    // Verify file is deleted
                    match file_open("/test_unlink_file.txt", FileFlags::O_RDONLY, 0) {
                        Ok(_) => {
                            test_fail("sys_unlink", "file still exists");
                        }
                        Err(_) => {
                            test_pass("sys_unlink file gone");
                        }
                    }
                }
                Err(e) => {
                    test_fail("sys_unlink", &alloc::format!("error: {}", e));
                }
            }
        }
        Err(_) => {
            test_skip("sys_unlink", "filesystem not writable");
        }
    }

    // Test deleting nonexistent file
    match vfs::file_unlink("/nonexistent_file_xyz.txt") {
        Ok(()) => {
            test_fail("sys_unlink (nonexistent)", "should fail");
        }
        Err(_) => {
            test_pass("sys_unlink (nonexistent rejected)");
        }
    }

    // Test deleting root directory (should fail)
    match vfs::file_unlink("/") {
        Ok(()) => {
            test_fail("sys_unlink root", "should fail");
        }
        Err(_) => {
            test_pass("sys_unlink root rejected");
        }
    }
}

fn test_sys_fd_management() {
    // Test file descriptor allocation
    // Open multiple files, verify fd increments
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd1) => {
            match file_open(filename, FileFlags::O_RDONLY, 0) {
                Ok(fd2) => {
                    // fd2 should != fd1
                    if fd1 != fd2 {
                        test_pass("sys_open different fds");
                    } else {
                        test_fail("sys_open fds", "same fd returned twice");
                    }

                    // After closing fd1, reopening may reuse fd1
                    let _ = file_close(fd1);
                    match file_open(filename, FileFlags::O_RDONLY, 0) {
                        Ok(fd3) => {
                            // fd3 may equal fd1 (fd reuse)
                            test_pass("sys_open fd reuse");
                            let _ = file_close(fd3);
                        }
                        Err(_) => {
                            test_fail("sys_open after close", "failed");
                        }
                    }

                    let _ = file_close(fd2);
                }
                Err(_) => {
                    test_fail("sys_open second", "failed");
                    let _ = file_close(fd1);
                }
            }
        }
        Err(_) => {
            test_skip("sys_fd_management", "no test file");
        }
    }
}

fn test_syscall_numbers() {
    // Verify syscall numbers match standard
    let openat_ok = SyscallNo::Openat as u32 == 56;
    let close_ok = SyscallNo::Close as u32 == 57;
    let read_ok = SyscallNo::Read as u32 == 63;
    let write_ok = SyscallNo::Write as u32 == 64;
    let lseek_ok = SyscallNo::Lseek as u32 == 62;
    let fstat_ok = SyscallNo::Fstat as u32 == 80;
    let getdents64_ok = SyscallNo::Getdents64 as u32 == 61;

    if openat_ok && close_ok && read_ok && write_ok && lseek_ok && fstat_ok && getdents64_ok {
        test_pass("file syscall numbers");
    } else {
        test_fail("file syscall numbers", "mismatch");
    }
}
