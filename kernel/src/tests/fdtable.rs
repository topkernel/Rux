//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// 测试：文件描述符管理 (FdTable)
use crate::println;
use crate::fs::file::{FdTable, File, FileFlags};
use super::{test_pass, test_fail, test_group_start};

pub fn test_fdtable() {
    test_group_start("FdTable management");

    // 测试 1: 创建 FdTable
    let fdtable = FdTable::new();
    test_pass("create FdTable");

    // 测试 2: 分配文件描述符
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

    // 测试 3: 创建 File 对象并安装
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

    // 测试 4: 获取文件对象
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

    // 测试 5: 获取无效的文件描述符
    match fdtable.get_file(9999) {
        Some(_) => {
            test_fail("invalid fd check", "should return None");
        }
        None => {
            test_pass("invalid fd check");
        }
    }

    // 测试 6: 关闭文件描述符
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

    // 测试 7: 验证关闭后无法获取文件
    match fdtable.get_file(fd1) {
        Some(_) => {
            test_fail("closed fd check", "should return None");
        }
        None => {
            test_pass("closed fd check");
        }
    }

    // 测试 8: 重复使用已释放的 fd
    let fd3 = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            test_fail("fd reuse", "alloc_fd returned None");
            return;
        }
    };
    // fd3 应该能被成功分配
    test_pass("fd reuse");

    println!("test: FdTable testing completed. (fd1={}, fd2={}, fd3={})", fd1, fd2, fd3);
}
