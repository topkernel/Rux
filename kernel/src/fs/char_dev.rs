//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Character Device File Operations
//!
//! Implements read/write operations for character devices, mainly supports UART devices

use crate::console;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum CharDevType {
    /// UART console
    UartConsole,
    /// Other character devices
    Other,
}

#[repr(C)]
pub struct CharDev {
    /// Device type
    pub dev_type: CharDevType,
    /// Device number
    pub dev: u64,
}

impl CharDev {
    /// Create new character device
    pub const fn new(dev_type: CharDevType, dev: u64) -> Self {
        Self { dev_type, dev }
    }

    /// Read from character device
    pub unsafe fn read(&self, buf: *mut u8, count: usize) -> isize {
        match self.dev_type {
            CharDevType::UartConsole => uart_read(buf, count),
            CharDevType::Other => -38_i32 as isize, // ENOSYS
        }
    }

    /// Write to character device
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

    // Busy wait for first character
    while bytes_read == 0 {
        if let Some(c) = console::getchar() {
            slice[bytes_read] = c;
            bytes_read += 1;
        }
        // Short delay to avoid excessive CPU usage
        for _ in 0..1000 {
            core::arch::asm!("nop", options(nomem, nostack));
        }
    }

    // Continue reading more characters (non-blocking)
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

/// UART character device file operations (public access)
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

/// Check if file is a character device and fill stat structure
///
/// Returns Some(()) if it's a character device, None if not
pub fn char_dev_stat(file: &crate::fs::File, stat: &mut crate::fs::Stat) -> Option<()> {
    unsafe {
        let ops_opt = &*file.ops.get();
        if let Some(ops) = ops_opt {
            // Check if it's a UART character device (by comparing ops pointer)
            let ops_ptr = *ops as *const crate::fs::FileOps;
            let uart_ops_ptr = &UART_OPS as *const crate::fs::FileOps;

            if ops_ptr == uart_ops_ptr {
                // This is a UART character device
                stat.st_dev = 0;
                stat.st_ino = 0;
                stat.st_nlink = 1;
                stat.st_uid = 0;
                stat.st_gid = 0;
                stat.st_rdev = 0x0500;  // ttyS0 device number
                stat.st_size = 0;
                stat.st_blksize = 1024;
                stat.st_blocks = 0;
                stat.set_char_device();
                stat.set_mode(0o620);  // crw--w---- (tty permissions)
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
