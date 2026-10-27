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
    // POSIX: read(fd, buf, 0) returns 0 without touching the buffer. The
    // loop below stores slice[bytes_read] BEFORE checking bytes_read >=
    // count, so a zero-length slice would panic on index-out-of-bounds the
    // moment a character is available (sys_read filters count==0 today, but
    // File::read is callable from other kernel paths).
    if count == 0 {
        return 0;
    }
    let mut bytes_read: usize = 0;
    let slice = core::slice::from_raw_parts_mut(buf, count);

    loop {
        // Try to read a character from console (ring buffer + hardware)
        if let Some(c) = console::getchar() {
            slice[bytes_read] = c;
            bytes_read += 1;
            if c == b'\n' || bytes_read >= count {
                break;
            }
        } else {
            // No data available — sleep
            let wq = console::read_waitq();
            let ret = crate::wait_event_interruptible!(wq, console::uart_has_data());

            if ret != 0 {
                if bytes_read > 0 {
                    return bytes_read as isize;
                }
                return -(crate::errno::constants::EINTR) as isize;
            }
        }
    }

    bytes_read as isize
}

pub unsafe fn uart_write(buf: *const u8, count: usize) -> isize {
    let slice = core::slice::from_raw_parts(buf, count);
    // R20-4 (PIPE2 pp=0 root cause): console output must stay atomic
    // against other task-context writers, or the nettest PIPE2 marker
    // splits and the gate greps 0 hits. The old fix took the UART lock
    // ONCE for the whole buffer.
    //
    // UART perf gate (2026-10-20): that whole-buffer acquisition is
    // `lock_irqsave` — a 256-byte condev chunk then held IRQs disabled
    // for the entire chunk's THRE pacing (and up to the full putc stall
    // budget on a slow chardev), which under the OH hilog volume
    // (~34k lines/boot through /dev/ttyS0) produced the multi-second
    // IRQ-off windows behind TIMER-SOFTIRQ-STALL / TIMER-LATE-FIRE /
    // WAKE-STRANDED and the 102 s IPC SendRequest regressions.
    // Compromise: re-acquire per UART_TX_CHUNK bytes. Atomicity is
    // preserved at 32-byte granularity — every status marker the test
    // suites grep (PIPE-OK\n, PASS/FAIL lines) fits well inside 32
    // bytes, so two task-context writers still cannot split one, while
    // the IRQ-off windows shrink from whole-buffer to sub-chunk (the
    // per-byte putc stall bound lives in console.rs). Byte dropping on
    // TX_STALLED is unchanged (putc latches and drops per its budget).
    const UART_TX_CHUNK: usize = 32;
    let mut idx = 0usize;
    while idx < slice.len() {
        let end = core::cmp::min(idx + UART_TX_CHUNK, slice.len());
        {
            let uart = console::lock();
            let mut i = idx;
            while i < end {
                uart.putc(slice[i]);
                i += 1;
            }
        }
        idx = end;
        // leg-32B (WAKE-STRANDED strand family): a console burst is
        // hundreds of back-to-back chunks (~2.8 ms each at 115200 THRE
        // pacing) in ONE task context. With no IRQ-exit preemption the
        // writer monopolizes its CPU for the whole burst — sc326B/
        // sc328B stranded every other runnable task 1-13.8 s behind
        // hilogd's serial mirror (the recurring strand incumbents,
        // pc!=0, tp=-confirmed). Between chunks the console lock is
        // dropped and we are in plain task context — a cond_resched
        // point per the 2026-10-20 discipline (the teardown sweeps got
        // theirs; this loop did not). 32-byte marker atomicity is
        // unchanged: the lock is still held across each whole chunk.
        crate::sched::cond_resched();
    }
    count as isize
}

/// UART character device file operations (public access)
fn uart_file_poll(_file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;
    if events & POLLIN != 0 && crate::console::uart_data_ready() {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 {
        ready |= POLLOUT | POLLWRNORM;
    }
    ready
}

/// UART character device file operations (public access)
pub static UART_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(uart_file_read),
    write: Some(uart_file_write),
    lseek: None,
    close: None,
    poll: Some(uart_file_poll),
    read_at: None,
};

/// Synthetic st_dev for kernel-console char-device Files (no other Rux
/// filesystem uses this id, so it can never alias a pipe or a real file).
const CHAR_DEV_STAT_DEV: u64 = 0x636f_6e73_6f6c_6501;

/// fstat(2) for console char-device file descriptions (no backing inode).
///
/// The std fds created by `init_std_fds_for_task` are inode-less UART
/// Files; fstat on them used to fail (and the re-negated +9 made glibc
/// skip writing the buffer entirely — the cat "input file is output
/// file" false positive). Report a stable S_IFCHR identity keyed on the
/// CharDev itself: all console fds are the same device, and never equal
/// to a pipe or a regular file.
///
/// Returns `Some(0)` when `file` is a UART File, `None` otherwise.
pub fn char_dev_file_stat(file: &crate::fs::File, stat: &mut crate::fs::Stat) -> Option<i32> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &UART_OPS as *const _) {
        return None;
    }
    // SAFETY: ops identity confirms a UART File; private_data points at the
    // static CharDev installed by init_std_fds_for_task (or equivalent).
    let ptr = unsafe { *file.private_data.get() }?;
    let dev = unsafe { &*(ptr as *const CharDev) };
    stat.st_dev = CHAR_DEV_STAT_DEV;
    stat.st_ino = ptr as usize as u64;
    stat.st_nlink = 1;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = dev.dev;
    stat.st_size = 0;
    stat.st_blocks = 0;
    stat.st_blksize = 1024;
    stat.set_char_device();
    stat.st_mode |= 0o620;
    Some(0)
}

fn uart_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // Check O_NONBLOCK flag
    let nonblock = (file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK) != 0;

    if nonblock {
        // Non-blocking: check once and return if no data
        if !console::uart_data_ready() {
            return -(crate::errno::constants::EAGAIN) as isize;
        }
    }

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
