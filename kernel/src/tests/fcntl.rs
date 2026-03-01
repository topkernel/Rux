//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! sys_fcntl 测试

use alloc::format;
use crate::fs::{file_open, file_close, file_fcntl, fcntl, FileFlags};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_fcntl() {
    test_group_start("fcntl");

    // 测试 1: F_GETFD / F_SETFD
    test_getfd_setfd();

    // 测试 2: F_GETFL
    test_getfl();

    // 测试 3: F_DUPFD
    test_dupfd();

    // 测试 4: F_SETFL
    test_setfl();
}

fn test_getfd_setfd() {
    // 打开一个文件
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            // 测试 F_GETFD
            let getfd_ok = match file_fcntl(fd, fcntl::F_GETFD, 0) {
                Ok(flags) => flags == 0,
                Err(_) => false,
            };
            if getfd_ok {
                test_pass("F_GETFD default");
            } else {
                test_fail("F_GETFD default", "should return 0");
            }

            // 测试 F_SETFD - 设置 FD_CLOEXEC
            let setfd_ok = file_fcntl(fd, fcntl::F_SETFD, fcntl::FD_CLOEXEC).is_ok();
            if !setfd_ok {
                test_fail("F_SETFD", "failed to set FD_CLOEXEC");
                let _ = file_close(fd);
                return;
            }

            // 再次测试 F_GETFD
            match file_fcntl(fd, fcntl::F_GETFD, 0) {
                Ok(flags) => {
                    if flags == fcntl::FD_CLOEXEC {
                        test_pass("F_GETFD/F_SETFD");
                    } else {
                        test_fail("F_GETFD after SETFD", "FD_CLOEXEC not set");
                    }
                }
                Err(e) => {
                    test_fail("F_GETFD after SETFD", &format!("error: {}", e));
                }
            }

            // 关闭文件
            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("F_GETFD/F_SETFD", "no test file");
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
                    if (flags as u32) & FileFlags::O_RDONLY != 0 || (flags as u32) & 0x3 == 0 {
                        test_pass("F_GETFL");
                    } else {
                        test_fail("F_GETFL", "unexpected flags");
                    }
                }
                Err(e) => {
                    test_fail("F_GETFL", &format!("error: {}", e));
                }
            }

            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("F_GETFL", "no test file");
        }
    }
}

fn test_dupfd() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(old_fd) => {
            // 测试 F_DUPFD
            match file_fcntl(old_fd, fcntl::F_DUPFD, 0) {
                Ok(new_fd) => {
                    if new_fd != old_fd {
                        test_pass("F_DUPFD");
                    } else {
                        test_fail("F_DUPFD", "returned same fd");
                    }

                    // 关闭新文件描述符
                    let _ = file_close(new_fd);
                }
                Err(e) => {
                    test_fail("F_DUPFD", &format!("error: {}", e));
                }
            }

            let _ = file_close(old_fd);
        }
        Err(_) => {
            test_skip("F_DUPFD", "no test file");
        }
    }
}

fn test_setfl() {
    let filename = "/test_existing.txt";
    match file_open(filename, FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            // 获取原始标志
            let original_flags = match file_fcntl(fd, fcntl::F_GETFL, 0) {
                Ok(f) => f,
                Err(e) => {
                    test_fail("F_SETFL", &format!("F_GETFL failed: {}", e));
                    let _ = file_close(fd);
                    return;
                }
            };

            // 设置 O_NONBLOCK
            let set_arg = ((original_flags as u32) | FileFlags::O_NONBLOCK) as usize;
            let setfl_ok = file_fcntl(fd, fcntl::F_SETFL, set_arg).is_ok();
            if !setfl_ok {
                test_fail("F_SETFL", "failed to set O_NONBLOCK");
                let _ = file_close(fd);
                return;
            }

            // 验证标志已设置
            match file_fcntl(fd, fcntl::F_GETFL, 0) {
                Ok(new_flags) => {
                    if (new_flags as u32) & FileFlags::O_NONBLOCK != 0 {
                        test_pass("F_SETFL O_NONBLOCK");
                    } else {
                        test_fail("F_SETFL", "O_NONBLOCK flag not set");
                    }
                }
                Err(e) => {
                    test_fail("F_SETFL verify", &format!("error: {}", e));
                }
            }

            let _ = file_close(fd);
        }
        Err(_) => {
            test_skip("F_SETFL", "no test file");
        }
    }

    // 测试无效的文件描述符
    match file_fcntl(9999, fcntl::F_GETFL, 0) {
        Ok(_) => {
            test_fail("F_GETFL invalid fd", "should return error");
        }
        Err(_) => {
            test_pass("F_GETFL invalid fd");
        }
    }
}
