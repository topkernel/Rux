//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 字符设备文件操作
//!
//! 实现字符设备的读写操作，主要支持 UART 设备
//!
//! 对应 Linux 的 drivers/char/ 和 drivers/tty/

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
    pub fn new(dev_type: CharDevType, dev: u64) -> Self {
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
    // 阻塞读取：等待至少一个字符或换行符
    let mut bytes_read: usize = 0;
    let slice = core::slice::from_raw_parts_mut(buf, count);

    // 首先阻塞等待第一个字符
    while bytes_read == 0 {
        if let Some(c) = console::getchar() {
            slice[bytes_read] = c;
            bytes_read += 1;
        } else {
            // 使用 wfi 等待中断，但需要允许内存访问
            // 这样当中断到来时（可能有数据），可以正确读取
            core::arch::asm!("wfi", options(nostack));
        }
    }

    // 继续读取更多字符（非阻塞），直到遇到换行符或缓冲区满
    while bytes_read < count {
        if let Some(c) = console::getchar() {
            slice[bytes_read] = c;
            bytes_read += 1;

            // 遇到换行符就停止
            if c == b'\n' {
                break;
            }
        } else {
            // 没有更多数据
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
