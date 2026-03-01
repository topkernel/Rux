//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 文件系统相关系统调用测试
//!
//! 包含：open, close, read, write, lseek, fstat, mkdir, rmdir, unlink, getdents64

use crate::fs::{file_open, file_close, file_stat, Stat, FileFlags};
use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_file() {
    test_group_start("syscall: file operations");

    // 测试 1: open/close 系统调用
    test_sys_open_close();

    // 测试 2: fstat 系统调用
    test_sys_fstat();

    // 测试 3: lseek 系统调用
    test_sys_lseek();

    // 测试 4: mkdir/rmdir 系统调用
    test_sys_mkdir_rmdir();

    // 测试 5: unlink 系统调用
    test_sys_unlink();

    // 测试 6: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_open_close() {
    // 测试打开已存在的文件
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            test_pass("sys_open (existing file)");

            // 关闭文件
            match file_close(fd) {
                Ok(()) => test_pass("sys_close"),
                Err(e) => test_fail("sys_close", &alloc::format!("error: {}", e)),
            }
        }
        Err(_) => {
            test_skip("sys_open/close", "no test file");
        }
    }

    // 测试打开不存在的文件（应该失败）
    match file_open("/nonexistent_file.txt", FileFlags::O_RDONLY, 0) {
        Ok(_) => {
            test_fail("sys_open", "should fail for nonexistent file");
        }
        Err(_) => {
            test_pass("sys_open (nonexistent rejected)");
        }
    }

    // 测试创建文件
    match file_open("/test_create.txt", FileFlags::O_CREAT | FileFlags::O_WRONLY | FileFlags::O_TRUNC, 0o644) {
        Ok(fd) => {
            test_pass("sys_open (O_CREAT)");
            let _ = file_close(fd);
            // 清理
            let _ = crate::fs::vfs::file_unlink("/test_create.txt");
        }
        Err(_) => {
            test_skip("sys_open O_CREAT", "filesystem not writable");
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
                    // 验证 stat 结构体的基本字段
                    let has_ino = stat.st_ino > 0;
                    let has_mode = stat.st_mode != 0;

                    if has_ino && has_mode {
                        test_pass("sys_fstat");
                    } else {
                        test_fail("sys_fstat", "missing required fields");
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

    // 测试无效 fd
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
    use crate::fs::get_file_fd;

    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            unsafe {
                match get_file_fd(fd) {
                    Some(file) => {
                        // SEEK_SET = 0
                        let result = file.lseek(0, 0);
                        if result >= 0 {
                            test_pass("sys_lseek SEEK_SET");
                        } else {
                            test_fail("sys_lseek SEEK_SET", "negative result");
                        }

                        // SEEK_CUR = 1
                        let result = file.lseek(10, 1);
                        if result >= 10 {
                            test_pass("sys_lseek SEEK_CUR");
                        } else {
                            test_fail("sys_lseek SEEK_CUR", "unexpected result");
                        }

                        // SEEK_END = 2
                        let result = file.lseek(0, 2);
                        if result >= 0 {
                            test_pass("sys_lseek SEEK_END");
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
    use crate::fs::vfs;

    // 测试创建目录
    let dirname = "/test_syscall_dir";
    match vfs::file_mkdir(dirname, 0o755) {
        Ok(()) => {
            test_pass("sys_mkdir");

            // 测试删除空目录
            match vfs::file_rmdir(dirname) {
                Ok(()) => {
                    test_pass("sys_rmdir");
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

    // 测试删除不存在的目录
    match vfs::file_rmdir("/nonexistent_dir") {
        Ok(()) => {
            test_fail("sys_rmdir (nonexistent)", "should fail");
        }
        Err(_) => {
            test_pass("sys_rmdir (nonexistent rejected)");
        }
    }
}

fn test_sys_unlink() {
    use crate::fs::vfs;

    // 测试删除不存在的文件
    match vfs::file_unlink("/nonexistent_file_for_unlink.txt") {
        Ok(()) => {
            test_fail("sys_unlink (nonexistent)", "should fail");
        }
        Err(_) => {
            test_pass("sys_unlink (nonexistent rejected)");
        }
    }

    // 测试删除根目录（应该失败）
    match vfs::file_unlink("/") {
        Ok(()) => {
            test_fail("sys_unlink root", "should fail");
        }
        Err(_) => {
            test_pass("sys_unlink root rejected");
        }
    }
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
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
        test_fail("file syscall numbers", "mismatch with Linux");
    }
}
