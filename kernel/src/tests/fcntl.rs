//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_fcntl 测试

use crate::println;
use crate::fs::{file_open, file_close, file_fcntl, fcntl, FileFlags};

pub fn test_fcntl() {
    println!("test: ===== Starting fcntl() Tests =====");

    // 测试 1: F_GETFD / F_SETFD
    println!("test: 1. Testing F_GETFD/F_SETFD...");
    test_getfd_setfd();

    // 测试 2: F_GETFL
    println!("test: 2. Testing F_GETFL...");
    test_getfl();

    // 测试 3: F_DUPFD
    println!("test: 3. Testing F_DUPFD...");
    test_dupfd();

    // 测试 4: F_SETFL
    println!("test: 4. Testing F_SETFL...");
    test_setfl();

    println!("test: ===== fcntl() Tests Completed =====");
}

fn test_getfd_setfd() {
    // 打开一个文件
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            println!("test:    Opened file '{}', fd={}", filename, fd);

            // 测试 F_GETFD
            match file_fcntl(fd, fcntl::F_GETFD, 0) {
                Ok(flags) => {
                    println!("test:    F_GETFD returned: {}", flags);
                    if flags == 0 {
                        println!("test:    SUCCESS - FD_CLOEXEC is not set (default)");
                    } else {
                        println!("test:    Note - FD_CLOEXEC is set");
                    }
                }
                Err(e) => {
                    println!("test:    F_GETFD failed: {}", e);
                }
            }

            // 测试 F_SETFD - 设置 FD_CLOEXEC
            match file_fcntl(fd, fcntl::F_SETFD, fcntl::FD_CLOEXEC) {
                Ok(_) => {
                    println!("test:    F_SETFD(FD_CLOEXEC) succeeded");
                }
                Err(e) => {
                    println!("test:    F_SETFD failed: {}", e);
                }
            }

            // 再次测试 F_GETFD
            match file_fcntl(fd, fcntl::F_GETFD, 0) {
                Ok(flags) => {
                    println!("test:    F_GETFD after SETFD: {}", flags);
                    if flags == fcntl::FD_CLOEXEC {
                        println!("test:    SUCCESS - FD_CLOEXEC is now set");
                    } else {
                        println!("test:    FAILED - FD_CLOEXEC not set");
                    }
                }
                Err(e) => {
                    println!("test:    F_GETFD failed: {}", e);
                }
            }

            // 关闭文件
            let _ = file_close(fd);
        }
        Err(_) => {
            println!("test:    SKIPPED - Could not open file '{}'", filename);
        }
    }
}

fn test_getfl() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            // 测试 F_GETFL
            match file_fcntl(fd, fcntl::F_GETFL, 0) {
                Ok(flags) => {
                    println!("test:    F_GETFL returned flags: {:#x}", flags);
                    println!("test:    SUCCESS - F_GETFL works");
                }
                Err(e) => {
                    println!("test:    F_GETFL failed: {}", e);
                }
            }

            let _ = file_close(fd);
        }
        Err(_) => {
            println!("test:    SKIPPED - Could not open file '{}'", filename);
        }
    }
}

fn test_dupfd() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(old_fd) => {
            println!("test:    Opened file '{}', old_fd={}", filename, old_fd);

            // 测试 F_DUPFD
            match file_fcntl(old_fd, fcntl::F_DUPFD, 0) {
                Ok(new_fd) => {
                    println!("test:    F_DUPFD returned new_fd={}", new_fd);
                    if new_fd != old_fd {
                        println!("test:    SUCCESS - F_DUPFD created different fd");
                    } else {
                        println!("test:    Note - F_DUPFD returned same fd");
                    }

                    // 关闭新文件描述符
                    let _ = file_close(new_fd);
                }
                Err(e) => {
                    println!("test:    F_DUPFD failed: {}", e);
                }
            }

            let _ = file_close(old_fd);
        }
        Err(_) => {
            println!("test:    SKIPPED - Could not open file '{}'", filename);
        }
    }
}

fn test_setfl() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            println!("test:    Testing F_SETFL with O_NONBLOCK...");

            // 获取原始标志
            let original_flags = match file_fcntl(fd, fcntl::F_GETFL, 0) {
                Ok(f) => f,
                Err(e) => {
                    println!("test:    F_GETFL failed: {}", e);
                    let _ = file_close(fd);
                    return;
                }
            };
            println!("test:    Original flags: {:#x}", original_flags);

            // 设置 O_NONBLOCK
            let set_arg = ((original_flags as u32) | FileFlags::O_NONBLOCK) as usize;
            match file_fcntl(fd, fcntl::F_SETFL, set_arg) {
                Ok(_) => {
                    println!("test:    F_SETFL succeeded");
                }
                Err(e) => {
                    println!("test:    F_SETFL failed: {}", e);
                    let _ = file_close(fd);
                    return;
                }
            }

            // 验证标志已设置
            match file_fcntl(fd, fcntl::F_GETFL, 0) {
                Ok(new_flags) => {
                    println!("test:    New flags: {:#x}", new_flags);
                    if (new_flags as u32) & FileFlags::O_NONBLOCK != 0 {
                        println!("test:    SUCCESS - O_NONBLOCK flag is set");
                    } else {
                        println!("test:    FAILED - O_NONBLOCK flag not set");
                    }
                }
                Err(e) => {
                    println!("test:    F_GETFL failed: {}", e);
                }
            }

            let _ = file_close(fd);
        }
        Err(_) => {
            println!("test:    SKIPPED - Could not open file '{}'", filename);
        }
    }

    // 测试无效的文件描述符
    println!("test:    Testing F_GETFL with invalid fd...");
    match file_fcntl(9999, fcntl::F_GETFL, 0) {
        Ok(_) => {
            println!("test:    FAILED - should have returned error");
        }
        Err(e) => {
            println!("test:    SUCCESS - correctly returned error: {}", e);
        }
    }
}
