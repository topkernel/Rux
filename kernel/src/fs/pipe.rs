//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Pipe Filesystem
//!
//!
//! Core concepts:
//! - `struct pipe_inode_info`: Pipe information
//! - `struct pipe_buffer`: Pipe buffer
//! - Synchronous read/write operations

use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::sync::Arc;
use crate::process::wait::WaitQueueHead;

/// Pipe buffer size - from config
const PIPE_BUF_SIZE: usize = crate::config::PIPE_BUFFER_SIZE;

#[repr(C)]
pub struct PipeBuffer {
    /// Buffer data
    data: Vec<u8>,
    /// Read pointer
    read_pos: AtomicUsize,
    /// Write pointer
    write_pos: AtomicUsize,
    /// Buffer size
    size: usize,
}

impl PipeBuffer {
    /// Create new pipe buffer
    pub fn new(size: usize) -> Self {
        // Manually allocate and initialize vector to avoid vec! macro
        let mut data = Vec::with_capacity(size);
        unsafe {
            core::ptr::write_bytes(data.as_mut_ptr(), 0, size);
            data.set_len(size);
        }

        Self {
            data,
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            size,
        }
    }

    /// Read data from ring buffer
    ///
    /// Handles wrap-around: data may span [read_pos..size) and [0..write_pos).
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if read_pos == write_pos {
            return 0; // Buffer empty
        }

        let total_available = if write_pos > read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        };

        let to_read = core::cmp::min(total_available, buf.len());

        for i in 0..to_read {
            buf[i] = self.data[(read_pos + i) % self.size];
        }

        self.read_pos.store((read_pos + to_read) % self.size, Ordering::Release);
        to_read
    }

    /// Write data to ring buffer
    ///
    /// Handles wrap-around: write may span [write_pos..size) and [0..gap).
    pub fn write(&mut self, buf: &[u8]) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        // Calculate available space (keep one slot empty to distinguish full from empty)
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

    /// Get available read bytes
    pub fn available_read(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            self.size - read_pos + write_pos
        }
    }

    /// Get available write space
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

#[repr(C)]
pub struct Pipe {
    /// Pipe buffer
    buffer: Spinlock<PipeBuffer>,
    /// Read end closed
    read_closed: AtomicUsize,
    /// Write end closed
    write_closed: AtomicUsize,
    /// Read wait queue (for read blocking)
    read_queue: WaitQueueHead,
    /// Write wait queue (for write blocking)
    write_queue: WaitQueueHead,
}

impl Pipe {
    /// Create new pipe
    pub fn new() -> Self {
        Self {
            buffer: Spinlock::new(PipeBuffer::new(PIPE_BUF_SIZE)),
            read_closed: AtomicUsize::new(0),
            write_closed: AtomicUsize::new(0),
            read_queue: WaitQueueHead::new(),
            write_queue: WaitQueueHead::new(),
        }
    }

    /// Close read end
    pub fn close_read(&self) {
        self.read_closed.store(1, Ordering::Release);
        // Wake up all write waiters (read end closed causes write to return SIGPIPE)
        self.write_queue.wake_up_all();
    }

    /// Close write end
    pub fn close_write(&self) {
        self.write_closed.store(1, Ordering::Release);
        // Wake up all read waiters (EOF)
        self.read_queue.wake_up_all();
    }

    /// Check if read end is closed
    pub fn is_read_closed(&self) -> bool {
        self.read_closed.load(Ordering::Acquire) == 1
    }

    /// Check if write end is closed
    pub fn is_write_closed(&self) -> bool {
        self.write_closed.load(Ordering::Acquire) == 1
    }

    /// Get read wait queue
    pub fn read_queue(&self) -> &WaitQueueHead {
        &self.read_queue
    }

    /// Get write wait queue
    pub fn write_queue(&self) -> &WaitQueueHead {
        &self.write_queue
    }
}

pub fn pipe_read(pipe: &Pipe, buf: &mut [u8]) -> isize {
    if pipe.is_write_closed() && pipe.buffer.lock().available_read() == 0 {
        return 0; // EOF
    }

    let count = pipe.buffer.lock().read(buf);
    count as isize
}

pub fn pipe_write(pipe: &Pipe, buf: &[u8]) -> isize {
    if pipe.is_read_closed() {
        // Write to pipe with no readers -> SIGPIPE + EPIPE
        if let Some(current) = crate::sched::current() {
            let _ = crate::signal::send_signal((*current).pid(), crate::signal::Signal::SIGPIPE as i32);
        }
        return -(crate::errno::constants::EPIPE) as isize;
    }

    let count = pipe.buffer.lock().write(buf);
    if count == 0 {
        // Buffer full, non-blocking mode returns EAGAIN
        -11_i32 as isize // EAGAIN
    } else {
        count as isize
    }
}

use crate::fs::file::{File, FileOps, FileFlags};

fn pipe_file_read(file: &File, buf: &mut [u8]) -> isize {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // Check if non-blocking mode
        let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;

        loop {
            // Check EOF condition: write end closed and buffer empty
            if pipe.is_write_closed() && pipe.buffer.lock().available_read() == 0 {
                return 0; // EOF
            }

            // Try to read data
            let count = pipe.buffer.lock().read(buf);
            if count > 0 {
                // Read successful, wake up write waiters (space available)
                pipe.write_queue().wake_up_all();
                return count as isize;
            }

            // Buffer empty
            if nonblock {
                // Non-blocking mode: return EAGAIN
                return -11_i32 as isize; // EAGAIN
            }

            // Blocking mode: use wait queue to wait for data
            // Condition: buffer has data or write end closed
            {
                // Create wait queue entry
                let current = match crate::sched::current() {
                    Some(task) => task,
                    None => return 0, // Cannot get current task, return EOF
                };

                let entry = crate::process::wait::WaitQueueEntry::new(current, false);
                pipe.read_queue().add(entry);

                // Yield CPU
                crate::sched::schedule();

                // Remove from wait queue after wakeup
                pipe.read_queue().remove(current);

                // Recheck condition
                continue;
            }
        }
    } else {
        -9  // EBADF
    }
}

fn pipe_file_write(file: &File, buf: &[u8]) -> isize {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // Check if read end is closed
        if pipe.is_read_closed() {
            // Write to pipe with no readers -> SIGPIPE + EPIPE
            if let Some(current) = crate::sched::current() {
                let _ = crate::signal::send_signal((*current).pid(), crate::signal::Signal::SIGPIPE as i32);
            }
            return -(crate::errno::constants::EPIPE) as isize;
        }

        // Check if non-blocking mode
        let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;

        let mut total_written = 0;

        // Loop write until all data written or error encountered
        while total_written < buf.len() {
            let remaining = &buf[total_written..];

            // Try to write data
            let count = pipe.buffer.lock().write(remaining);

            if count > 0 {
                // Write successful
                total_written += count;
                // Wake up read waiters (data available)
                pipe.read_queue().wake_up_all();
                continue;
            }

            // Buffer full
            if nonblock {
                // Non-blocking mode: return bytes written or EAGAIN
                if total_written > 0 {
                    return total_written as isize;
                } else {
                    return -11_i32 as isize; // EAGAIN
                }
            }

            // Blocking mode: use wait queue to wait for space
            {
                // Create wait queue entry
                let current = match crate::sched::current() {
                    Some(task) => task,
                    None => return total_written as isize, // Cannot get current task, return bytes written
                };

                let entry = crate::process::wait::WaitQueueEntry::new(current, false);
                pipe.write_queue().add(entry);

                // Yield CPU
                crate::sched::schedule();

                // Remove from wait queue after wakeup
                pipe.write_queue().remove(current);

                // Check if read end closed while we were sleeping
                if pipe.is_read_closed() {
                    if total_written > 0 {
                        return total_written as isize;
                    }
                    if let Some(current) = crate::sched::current() {
                        let _ = crate::signal::send_signal((*current).pid(), crate::signal::Signal::SIGPIPE as i32);
                    }
                    return -(crate::errno::constants::EPIPE) as isize;
                }

                // Retry write
                continue;
            }
        }

        total_written as isize
    } else {
        -9  // EBADF
    }
}

fn pipe_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        if events & POLLIN != 0 {
            if pipe.buffer.lock().available_read() > 0 {
                ready |= POLLIN | POLLRDNORM;
            }
            if pipe.is_write_closed() {
                ready |= POLLHUP;
            }
        }

        if events & POLLOUT != 0 {
            if pipe.buffer.lock().available_write() > 0 {
                ready |= POLLOUT | POLLWRNORM;
            }
            if pipe.is_read_closed() {
                ready |= POLLERR;
            }
        }
    }

    ready
}

fn pipe_file_close(file: &File) -> i32 {
    if let Some(pipe_ptr) = unsafe { *file.private_data.get() } {
        let pipe = unsafe { &*(pipe_ptr as *const Pipe) };

        // Check file flags to determine whether to close read or write end
        if file.flags().is_readonly() || file.flags().is_rdwr() {
            // Close read end
            pipe.close_read();
        }

        if file.flags().is_writeonly() || file.flags().is_rdwr() {
            // Close write end
            pipe.close_write();
        }

        // If both ends are closed, free pipe memory
        if pipe.is_read_closed() && pipe.is_write_closed() {
            unsafe {
                // Convert raw pointer back to Box, which will be automatically freed when Box goes out of scope
                // ...
                let _ = Box::from_raw(pipe_ptr as *mut Pipe);
            }
        }

        0  // Success
    } else {
        -9  // EBADF
    }
}

pub fn create_pipe() -> (Arc<File>, Arc<File>) {
    // Create pipe and allocate on heap (use Box::leak to ensure lifetime until manual release)
    let pipe = Box::new(Pipe::new());
    let pipe_ptr = Box::leak(pipe) as *mut Pipe as *mut u8;

    // Pipe file operations
    static PIPE_OPS: FileOps = FileOps {
        read: Some(pipe_file_read),
        write: Some(pipe_file_write),
        lseek: None,  // Pipe doesn't support lseek
        close: Some(pipe_file_close),
        poll: Some(pipe_file_poll),
    };

    // Create read end file
    let read_file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    read_file.set_ops(&PIPE_OPS);
    read_file.set_private_data(pipe_ptr);

    // Create write end file
    let write_file = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    write_file.set_ops(&PIPE_OPS);
    write_file.set_private_data(pipe_ptr);

    (read_file, write_file)
}
