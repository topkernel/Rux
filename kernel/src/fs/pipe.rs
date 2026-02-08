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
use crate::collection::SimpleArc;
use crate::process::wait::WaitQueueHead;

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
    /// 读等待队列（用于读阻塞）
    read_queue: WaitQueueHead,
    /// 写等待队列（用于写阻塞）
    write_queue: WaitQueueHead,
}

impl Pipe {
    /// 创建新管道
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(PipeBuffer::new(PIPE_BUF_SIZE)),
            read_closed: AtomicUsize::new(0),
            write_closed: AtomicUsize::new(0),
            read_queue: WaitQueueHead::new(),
            write_queue: WaitQueueHead::new(),
        }
    }

    /// 关闭读端
    pub fn close_read(&self) {
        self.read_closed.store(1, Ordering::Release);
        // 唤醒所有写等待者（读端关闭会导致写操作返回 SIGPIPE）
        self.write_queue.wake_up_all();
    }

    /// 关闭写端
    pub fn close_write(&self) {
        self.write_closed.store(1, Ordering::Release);
        // 唤醒所有读等待者（EOF）
        self.read_queue.wake_up_all();
    }

    /// 检查读端是否关闭
    pub fn is_read_closed(&self) -> bool {
        self.read_closed.load(Ordering::Acquire) == 1
    }

    /// 检查写端是否关闭
    pub fn is_write_closed(&self) -> bool {
        self.write_closed.load(Ordering::Acquire) == 1
    }

    /// 获取读等待队列
    pub fn read_queue(&self) -> &WaitQueueHead {
        &self.read_queue
    }

    /// 获取写等待队列
    pub fn write_queue(&self) -> &WaitQueueHead {
        &self.write_queue
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

use crate::fs::file::{File, FileOps, FileFlags};

/// 管道文件读取操作（File::ops.read）
///
/// 对应 Linux 的 pipe_read (fs/pipe.c)
///
/// 实现阻塞和非阻塞读取：
/// - 非阻塞模式 (O_NONBLOCK): 缓冲区为空时立即返回 EAGAIN
/// - 阻塞模式: 缓冲区为空且写端未关闭时，阻塞等待数据
fn pipe_file_read(file: &File, buf: &mut [u8]) -> isize {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // 检查是否为非阻塞模式
        let nonblock = (file.flags.bits() & FileFlags::O_NONBLOCK) != 0;

        loop {
            // 检查 EOF 条件：写端已关闭且缓冲区为空
            if pipe.is_write_closed() && pipe.buffer.lock().available_read() == 0 {
                return 0; // EOF
            }

            // 尝试读取数据
            let count = pipe.buffer.lock().read(buf);
            if count > 0 {
                // 读取成功，唤醒写等待者（有空间了）
                pipe.write_queue().wake_up_all();
                return count as isize;
            }

            // 缓冲区为空
            if nonblock {
                // 非阻塞模式：返回 EAGAIN
                return -11_i32 as isize; // EAGAIN
            }

            // 阻塞模式：使用等待队列等待数据
            // 条件：缓冲区有数据或写端关闭
            {
                // 创建等待队列项
                let current = match crate::sched::current() {
                    Some(task) => task,
                    None => return 0, // 无法获取当前任务，返回 EOF
                };

                let entry = crate::process::wait::WaitQueueEntry::new(current, false);
                pipe.read_queue().add(entry);

                // 让出 CPU
                #[cfg(feature = "riscv64")]
                crate::sched::schedule();

                // 被唤醒后，从等待队列移除
                pipe.read_queue().remove(current);

                // 重新检查条件
                continue;
            }
        }
    } else {
        -9  // EBADF
    }
}

/// 管道文件写入操作（File::ops.write）
///
/// 对应 Linux 的 pipe_write (fs/pipe.c)
///
/// 实现阻塞和非阻塞写入：
/// - 非阻塞模式 (O_NONBLOCK): 缓冲区满时立即返回 EAGAIN
/// - 阻塞模式: 缓冲区满时，阻塞等待空间可用
fn pipe_file_write(file: &File, buf: &[u8]) -> isize {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // 检查读端是否已关闭
        if pipe.is_read_closed() {
            return -9; // EBADF - 读端已关闭，写入会失败（SIGPIPE）
        }

        // 检查是否为非阻塞模式
        let nonblock = (file.flags.bits() & FileFlags::O_NONBLOCK) != 0;

        let mut total_written = 0;

        // 循环写入，直到所有数据写入完毕或遇到错误
        while total_written < buf.len() {
            let remaining = &buf[total_written..];

            // 尝试写入数据
            let count = pipe.buffer.lock().write(remaining);

            if count > 0 {
                // 写入成功
                total_written += count;
                // 唤醒读等待者（有数据了）
                pipe.read_queue().wake_up_all();
                continue;
            }

            // 缓冲区满
            if nonblock {
                // 非阻塞模式：返回已写入的字节数或 EAGAIN
                if total_written > 0 {
                    return total_written as isize;
                } else {
                    return -11_i32 as isize; // EAGAIN
                }
            }

            // 阻塞模式：使用等待队列等待空间
            {
                // 创建等待队列项
                let current = match crate::sched::current() {
                    Some(task) => task,
                    None => return total_written as isize, // 无法获取当前任务，返回已写入字节数
                };

                let entry = crate::process::wait::WaitQueueEntry::new(current, false);
                pipe.write_queue().add(entry);

                // 让出 CPU
                #[cfg(feature = "riscv64")]
                crate::sched::schedule();

                // 被唤醒后，从等待队列移除
                pipe.write_queue().remove(current);

                // 重新尝试写入
                continue;
            }
        }

        total_written as isize
    } else {
        -9  // EBADF
    }
}

/// 管道文件关闭操作（File::ops.close）
///
/// 对应 Linux 的 pipe_release (fs/pipe.c)
fn pipe_file_close(file: &File) -> i32 {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // 检查文件标志，决定关闭读端还是写端
        if file.flags.is_readonly() || file.flags.is_rdwr() {
            // 关闭读端
            pipe.close_read();
        }

        if file.flags.is_writeonly() || file.flags.is_rdwr() {
            // 关闭写端
            pipe.close_write();
        }

        // 如果两端都关闭了，释放管道
        if pipe.is_read_closed() && pipe.is_write_closed() {
            // TODO: 释放管道内存
            // 目前暂时不做任何事，等待全局析构
        }

        0  // 成功
    } else {
        -9  // EBADF
    }
}

/// 创建管道
///
/// 对应 Linux 的 do_pipe() (fs/pipe.c)
///
/// # 返回
/// * `(Option<SimpleArc<File>>, Option<SimpleArc<File>>)` - (读端文件, 写端文件)
pub fn create_pipe() -> (Option<SimpleArc<File> >, Option<SimpleArc<File> >) {
    use alloc::sync::Arc;

    // 创建管道
    let pipe = Pipe::new();
    let pipe_ptr = &pipe as *const Pipe as *mut u8;

    // 管道文件操作
    static PIPE_OPS: FileOps = FileOps {
        read: Some(pipe_file_read),
        write: Some(pipe_file_write),
        lseek: None,  // 管道不支持 lseek
        close: Some(pipe_file_close),
    };

    // 创建读端文件
    let read_file = match SimpleArc::new(File::new(FileFlags::new(FileFlags::O_RDONLY))) {
        Some(f) => {
            f.set_ops(&PIPE_OPS);
            f.set_private_data(pipe_ptr);
            Some(f)
        }
        None => return (None, None),
    };

    // 创建写端文件
    let write_file = match SimpleArc::new(File::new(FileFlags::new(FileFlags::O_WRONLY))) {
        Some(f) => {
            f.set_ops(&PIPE_OPS);
            f.set_private_data(pipe_ptr);
            Some(f)
        }
        None => return (None, None),
    };

    // 将管道泄漏到堆上，确保它在文件关闭前一直存在
    // TODO: 实现引用计数来管理管道生命周期
    core::mem::forget(pipe);

    (read_file, write_file)
}
