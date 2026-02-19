//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 字符设备文件操作
//!
//! 实现字符设备的读写操作，主要支持 UART 设备
//!

use crate::console;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum CharDevType {
    /// UART 控制台
    UartConsole,
    /// 其他字符设备
    Other,
}

#[repr(C)]
pub struct CharDev {
    /// 设备类型
    pub dev_type: CharDevType,
    /// 设备号
    pub dev: u64,
}

impl CharDev {
    /// 创建新字符设备
    pub const fn new(dev_type: CharDevType, dev: u64) -> Self {
        Self { dev_type, dev }
    }

    /// 读取字符设备
    pub unsafe fn read(&self, buf: *mut u8, count: usize) -> isize {
        match self.dev_type {
            CharDevType::UartConsole => uart_read(buf, count),
            CharDevType::Other => -38_i32 as isize, // ENOSYS
        }
    }

    /// 写入字符设备
    pub unsafe fn write(&self, buf: *const u8, count: usize) -> isize {
        match self.dev_type {
            CharDevType::UartConsole => uart_write(buf, count),
            CharDevType::Other => -38_i32 as isize, // ENOSYS
        }
    }
}

pub unsafe fn uart_read(buf: *mut u8, count: usize) -> isize {
    let mut bytes_read: usize = 0;
    let slice = core::slice::from_raw_parts_mut(buf, count);

    // 忙等待第一个字符
    while bytes_read == 0 {
        if let Some(c) = console::getchar() {
            slice[bytes_read] = c;
            bytes_read += 1;
        }
        // 短暂延迟，避免过度占用 CPU
        for _ in 0..1000 {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }

    // 继续读取更多字符（非阻塞）
    while bytes_read < count {
        if let Some(c) = console::getchar() {
            slice[bytes_read] = c;
            bytes_read += 1;
            if c == b'\n' {
                break;
            }
        } else {
            break;
        }
    }

    bytes_read as isize
}

pub unsafe fn uart_write(buf: *const u8, count: usize) -> isize {
    let slice = core::slice::from_raw_parts(buf, count);
    for &b in slice {
        console::putchar(b);
    }
    count as isize
}

/// UART 字符设备的文件操作（公开访问）
pub static UART_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(uart_file_read),
    write: Some(uart_file_write),
    lseek: None,
    close: None,
};

fn uart_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { char_dev.read(buf.as_mut_ptr(), buf.len()) }
    } else {
        -9  // EBADF
    }
}

fn uart_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { char_dev.write(buf.as_ptr(), buf.len()) }
    } else {
        -9  // EBADF
    }
}

/// 检查文件是否为字符设备并填充 stat 结构
///
/// 返回 Some(()) 如果是字符设备，None 如果不是
pub fn char_dev_stat(file: &crate::fs::File, stat: &mut crate::fs::Stat) -> Option<()> {
    unsafe {
        let ops_opt = &*file.ops.get();
        if let Some(ops) = ops_opt {
            // 检查是否为 UART 字符设备（通过比较 ops 指针）
            let ops_ptr = *ops as *const crate::fs::FileOps;
            let uart_ops_ptr = &UART_OPS as *const crate::fs::FileOps;

            if ops_ptr == uart_ops_ptr {
                // 这是 UART 字符设备
                stat.st_dev = 0;
                stat.st_ino = 0;
                stat.st_nlink = 1;
                stat.st_uid = 0;
                stat.st_gid = 0;
                stat.st_rdev = 0x0500;  // ttyS0 的设备号
                stat.st_size = 0;
                stat.st_blksize = 1024;
                stat.st_blocks = 0;
                stat.set_char_device();
                stat.set_mode(0o620);  // crw--w---- (tty 权限)
                stat.st_atime = 0;
                stat.st_atime_nsec = 0;
                stat.st_mtime = 0;
                stat.st_mtime_nsec = 0;
                stat.st_ctime = 0;
                stat.st_ctime_nsec = 0;
                return Some(());
            }
        }
    }
    None
}
