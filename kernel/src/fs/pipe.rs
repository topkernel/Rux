//! 管道 (Pipe) 文件系统
//!
//! 完全遵循 Linux 内核的管道设计 (fs/pipe.c, include/linux/pipe_fs_i.h)
//!
//! 核心概念：
//! - `struct pipe_inode_info`: 管道信息
//! - `struct pipe_buffer`: 管道缓冲区
//! - 同步读写操作

use alloc::vec::Vec;
use alloc::alloc::{alloc, dealloc, Layout};
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 管道缓冲区大小 (默认 16KB)
const PIPE_BUF_SIZE: usize = 16384;

/// 管道缓冲区
///
/// 对应 Linux 的 struct pipe_buffer (include/linux/pipe_fs_i.h)
#[repr(C)]
pub struct PipeBuffer {
    /// 缓冲区数据
    data: Vec<u8>,
    /// 读指针
    read_pos: AtomicUsize,
    /// 写指针
    write_pos: AtomicUsize,
    /// 缓冲区大小
    size: usize,
}

impl PipeBuffer {
    /// 创建新的管道缓冲区
    pub fn new(size: usize) -> Self {
        // 手动分配并初始化向量，避免使用 vec! 宏
        let mut data = Vec::with_capacity(size);
        unsafe {
            data.set_len(size);
            core::ptr::write_bytes(data.as_mut_ptr(), 0, size);
        }

        Self {
            data,
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            size,
        }
    }

    /// 读取数据
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if read_pos == write_pos {
            return 0; // 缓冲区为空
        }

        let available = if write_pos > read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos
        };

        let to_read = core::cmp::min(available, buf.len());

        for i in 0..to_read {
            buf[i] = self.data[(read_pos + i) % self.size];
        }

        self.read_pos.store((read_pos + to_read) % self.size, Ordering::Release);
        to_read
    }

    /// 写入数据
    pub fn write(&mut self, buf: &[u8]) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        // 计算可用空间
        let available = if write_pos >= read_pos {
            self.size - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        };

        let to_write = core::cmp::min(available, buf.len());

        for i in 0..to_write {
            self.data[(write_pos + i) % self.size] = buf[i];
        }

        self.write_pos.store((write_pos + to_write) % self.size, Ordering::Release);
        to_write
    }

    /// 获取可用读取字节数
    pub fn available_read(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        }
    }

    /// 获取可用写入空间
    pub fn available_write(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            self.size - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        }
    }
}

/// 管道信息
///
/// 对应 Linux 的 struct pipe_inode_info (include/linux/pipe_fs_i.h)
#[repr(C)]
pub struct Pipe {
    /// 管道缓冲区
    buffer: Mutex<PipeBuffer>,
    /// 读端是否已关闭
    read_closed: AtomicUsize,
    /// 写端是否已关闭
    write_closed: AtomicUsize,
}

impl Pipe {
    /// 创建新管道
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(PipeBuffer::new(PIPE_BUF_SIZE)),
            read_closed: AtomicUsize::new(0),
            write_closed: AtomicUsize::new(0),
        }
    }

    /// 关闭读端
    pub fn close_read(&self) {
        self.read_closed.store(1, Ordering::Release);
    }

    /// 关闭写端
    pub fn close_write(&self) {
        self.write_closed.store(1, Ordering::Release);
    }

    /// 检查读端是否关闭
    pub fn is_read_closed(&self) -> bool {
        self.read_closed.load(Ordering::Acquire) == 1
    }

    /// 检查写端是否关闭
    pub fn is_write_closed(&self) -> bool {
        self.write_closed.load(Ordering::Acquire) == 1
    }
}

/// 管道读取操作
///
/// 对应 Linux 的 pipe_read (fs/pipe.c)
pub fn pipe_read(pipe: &Pipe, buf: &mut [u8]) -> isize {
    if pipe.is_write_closed() && pipe.buffer.lock().available_read() == 0 {
        return 0; // EOF
    }

    let count = pipe.buffer.lock().read(buf);
    count as isize
}

/// 管道写入操作
///
/// 对应 Linux 的 pipe_write (fs/pipe.c)
pub fn pipe_write(pipe: &Pipe, buf: &[u8]) -> isize {
    if pipe.is_read_closed() {
        return -9; // EBADF - 读端已关闭，写入会失败
    }

    let count = pipe.buffer.lock().write(buf);
    if count == 0 {
        // 缓冲区满，非阻塞模式下返回 EAGAIN
        -11_i32 as isize // EAGAIN
    } else {
        count as isize
    }
}
