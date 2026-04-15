//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Printk with log levels and ring buffer
//!
//! Provides leveled kernel logging (pr_emerg through pr_debug),
//! a ring buffer for storing all messages, and a syslog(2) syscall
//! for userspace `dmesg` to read/manage kernel logs.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

extern crate alloc;

use crate::sync::spinlock::Spinlock;

// ==================== Log Level Constants ====================

/// Log levels.
/// Lower value = higher priority.
pub mod loglevel {
    pub const KERN_EMERG:   u8 = 0; // System is unusable
    pub const KERN_ALERT:   u8 = 1; // Action must be taken immediately
    pub const KERN_CRIT:    u8 = 2; // Critical conditions
    pub const KERN_ERR:     u8 = 3; // Error conditions
    pub const KERN_WARNING: u8 = 4; // Warning conditions
    pub const KERN_NOTICE:  u8 = 5; // Normal but significant condition
    pub const KERN_INFO:    u8 = 6; // Informational
    pub const KERN_DEBUG:   u8 = 7; // Debug-level messages

    /// Default console log level: show up to KERN_INFO (6).
    /// Use `dmesg -n 7` to enable debug messages on console.
    pub const DEFAULT_CONSOLE_LOGLEVEL: u8 = KERN_INFO;
}

// ==================== Ring Buffer ====================

/// Maximum text payload per record (bytes).
const RECORD_TEXT_SIZE: usize = 256;

/// Record metadata size: level(1) + pid(4) + text_len(2) + seq(8) + timestamp(8) = 24 bytes.
const RECORD_META_SIZE: usize = 24;

/// Total size of one record (metadata + text).
const RECORD_TOTAL_SIZE: usize = RECORD_META_SIZE + RECORD_TEXT_SIZE; // 280 bytes

/// Number of ring buffer record slots, computed from configurable total size.
const RING_BUFFER_CAPACITY: usize = crate::config::PRINTK_RING_BUFFER_SIZE / RECORD_TOTAL_SIZE;

/// A single log record in the ring buffer.
#[repr(C)]
#[derive(Clone, Copy)]
struct LogRecord {
    /// Log level (0-7).
    level: u8,
    /// CPU ID that generated this message.
    cpu_id: u16,
    /// Padding for alignment.
    _pad: u8,
    /// PID of the process that generated this message.
    pid: u32,
    /// Length of valid text in `text` (0..RECORD_TEXT_SIZE).
    text_len: u16,
    /// Monotonically increasing sequence number.
    seq: u64,
    /// Timestamp: raw cycles from clint::read_time().
    timestamp: u64,
    /// The message text (not NUL-terminated; use text_len).
    text: [u8; RECORD_TEXT_SIZE],
}

/// The printk ring buffer.
struct RingBuffer {
    /// Fixed array of record slots.
    records: [LogRecord; RING_BUFFER_CAPACITY],
    /// Next slot index to write to.
    write_idx: usize,
    /// Sequence counter: monotonically increasing.
    next_seq: u64,
    /// Sequence number that the next sequential syslog READ will start from.
    read_seq: u64,
}

impl RingBuffer {
    const fn new() -> Self {
        Self {
            records: [LogRecord {
                level: 0,
                cpu_id: 0,
                _pad: 0,
                pid: 0,
                text_len: 0,
                seq: 0,
                timestamp: 0,
                text: [0u8; RECORD_TEXT_SIZE],
            }; RING_BUFFER_CAPACITY],
            write_idx: 0,
            next_seq: 0,
            read_seq: 0,
        }
    }
}

/// Global ring buffer instance.
static RING_BUFFER: Spinlock<RingBuffer> = Spinlock::new(RingBuffer::new());

// ==================== Global State ====================

/// Runtime console log level. Messages with level <= this value are printed to UART.
/// Starts at 7 (KERN_DEBUG) so all boot messages are visible.
static CONSOLE_LOGLEVEL: AtomicU8 = AtomicU8::new(loglevel::DEFAULT_CONSOLE_LOGLEVEL);

/// Set during boot after printk is ready.
static PRINTK_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Re-entrancy guard to prevent recursive printk.
static IN_PRINTK: AtomicBool = AtomicBool::new(false);

/// RAII guard for the `IN_PRINTK` re-entrancy flag.
///
/// On drop, clears the flag so that a panic inside printk never permanently
/// disables the logging subsystem.
struct PrintkGuard(());

impl PrintkGuard {
    /// Try to acquire the printk re-entrancy guard.
    ///
    /// Returns `None` if already inside printk (re-entrant call).
    fn try_new() -> Option<Self> {
        if IN_PRINTK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Some(PrintkGuard(()))
        } else {
            None
        }
    }
}

impl Drop for PrintkGuard {
    fn drop(&mut self) {
        IN_PRINTK.store(false, Ordering::Release);
    }
}

// ==================== Public API ====================

/// Set the console log level.
/// Only messages with level <= this value will be printed to UART.
pub fn set_console_loglevel(level: u8) {
    CONSOLE_LOGLEVEL.store(level, Ordering::Relaxed);
}

/// Get the current console log level.
pub fn get_console_loglevel() -> u8 {
    CONSOLE_LOGLEVEL.load(Ordering::Relaxed)
}

/// Initialize the printk subsystem.
/// Must be called after console::init().
pub fn init() {
    PRINTK_INITIALIZED.store(true, Ordering::Relaxed);
}

// ==================== Core printk ====================

/// Simple fmt::Write target that writes into a fixed-size buffer.
struct BufferWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> fmt::Write for BufferWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let remaining = &mut self.buf[self.pos..];
        let to_write = s.as_bytes().len().min(remaining.len());
        remaining[..to_write].copy_from_slice(&s.as_bytes()[..to_write]);
        self.pos += to_write;
        Ok(())
    }
}

/// Write a formatted message to the kernel log (no trailing newline).
///
/// This is the core function called by all printk macros.
/// It:
/// 1. Formats the message into a stack buffer
/// 2. If level <= console_loglevel, writes to ring buffer only
///
/// UART is reserved for userspace I/O and panic output.
/// Boot [ok] messages use putchar() directly (not printk).
/// Panic handler uses putchar_no_lock() directly.
pub fn printk(level: u8, args: fmt::Arguments) {
    // Re-entrancy guard: if already in printk, discard
    let _guard = match PrintkGuard::try_new() {
        Some(g) => g,
        None => return,
    };

    // Check console log level — controls ring buffer writes.
    // Messages above this level are discarded entirely.
    if level > CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        return;
    }

    // Format into a stack-allocated buffer
    let mut buf = [0u8; RECORD_TEXT_SIZE];
    let mut writer = BufferWriter { buf: &mut buf, pos: 0 };
    let _ = fmt::Write::write_fmt(&mut writer, args);
    let text_len = writer.pos.min(RECORD_TEXT_SIZE);

    // Write to ring buffer (if initialized)
    if PRINTK_INITIALIZED.load(Ordering::Relaxed) {
        let timestamp = crate::drivers::intc::clint::read_time();
        write_to_ring_buffer(level, &buf[..text_len], timestamp);
    }
}

/// Write a formatted message with trailing newline to the kernel log.
/// Used by the `println!` macro.
pub fn printk_ln(level: u8, args: fmt::Arguments) {
    // Format with newline into a local buffer to avoid borrow conflict
    let mut buf = [0u8; RECORD_TEXT_SIZE];
    let mut writer = BufferWriter { buf: &mut buf, pos: 0 };
    let _ = fmt::Write::write_fmt(&mut writer, format_args!("{}\n", args));
    let text_len = writer.pos.min(RECORD_TEXT_SIZE);
    printk_bytes(level, &buf[..text_len]);
}

/// Write raw bytes to the kernel log (used by printk_ln to avoid double formatting).
fn printk_bytes(level: u8, text: &[u8]) {
    // Re-entrancy guard: if already in printk, discard
    let _guard = match PrintkGuard::try_new() {
        Some(g) => g,
        None => return,
    };

    // Check console log level — controls ring buffer writes.
    if level > CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        return;
    }

    // Write to ring buffer (if initialized)
    if PRINTK_INITIALIZED.load(Ordering::Relaxed) {
        let timestamp = crate::drivers::intc::clint::read_time();
        write_to_ring_buffer(level, text, timestamp);
    }
}

// ==================== Ring Buffer Write ====================

fn write_to_ring_buffer(level: u8, text: &[u8], timestamp: u64) {
    let pid = crate::process::current_pid() as u32;
    let cpu_id: u16 = 0;

    let mut rb = RING_BUFFER.lock_irqsave();

    let idx = rb.write_idx;
    let seq = rb.next_seq;

    let record = &mut rb.records[idx];
    record.level = level;
    record.pid = pid;
    record.cpu_id = cpu_id;
    record.timestamp = timestamp;
    record.seq = seq;
    let copy_len = text.len().min(RECORD_TEXT_SIZE);
    record.text[..copy_len].copy_from_slice(&text[..copy_len]);
    record.text_len = copy_len as u16;

    // Advance write index (wrap around)
    rb.write_idx = (idx + 1) % RING_BUFFER_CAPACITY;
    rb.next_seq = seq + 1;

    // If sequential reader is behind, advance past overwritten records
    if rb.read_seq < rb.next_seq.saturating_sub(RING_BUFFER_CAPACITY as u64) {
        rb.read_seq = rb.next_seq - RING_BUFFER_CAPACITY as u64;
    }

    // Drop lock before persistent log write to avoid deadlock
    drop(rb);

    // Write to persistent log file (if initialized)
    persistent_log::append(level, text, seq, pid, cpu_id, timestamp);
}

// ==================== syslog Syscall ====================

/// syslog(2) syscall implementation.
///
/// syslog ABI: `int syslog(int type, char *bufp, int len);`
///
/// # Arguments (via SyscallArgs)
/// - args[0]: type - syslog action type (0-10)
/// - args[1]: bufp - user buffer pointer (for read actions)
/// - args[2]: len - buffer length (for read actions); reused as level for type 8
pub fn sys_syslog(args: [u64; 6]) -> u64 {
    let action = args[0] as i32;
    let bufp = args[1] as *mut u8;
    let len = args[2] as usize;

    match action {
        // Close/Open: no-op, return success
        0 | 1 => 0,

        // Read from log sequentially
        2 => syslog_read_sequential(bufp, len),

        // Read all records from log
        3 => syslog_read_all(bufp, len, false),

        // Read all records and clear
        4 => syslog_read_all(bufp, len, true),

        // Clear buffer
        5 => {
            syslog_clear();
            0
        }

        // Console off: suppress all console output except emergencies
        6 => {
            CONSOLE_LOGLEVEL.store(loglevel::KERN_EMERG, Ordering::Relaxed);
            0
        }

        // Console on: restore console output to default
        7 => {
            CONSOLE_LOGLEVEL.store(loglevel::DEFAULT_CONSOLE_LOGLEVEL, Ordering::Relaxed);
            0
        }

        // Set console log level
        8 => {
            let new_level = args[2] as u8;
            if new_level > loglevel::KERN_DEBUG {
                return (-crate::syscall::errno::EINVAL) as u64;
            }
            CONSOLE_LOGLEVEL.store(new_level, Ordering::Relaxed);
            0
        }

        // Return unread bytes (approximation)
        9 => {
            let rb = RING_BUFFER.lock_irqsave();
            let unread = rb.next_seq.saturating_sub(rb.read_seq);
            // Approximate: assume each record is ~128 bytes average
            (unread * 128) as u64
        }

        // Return total buffer size
        10 => (RING_BUFFER_CAPACITY * RECORD_TEXT_SIZE) as u64,

        _ => (-crate::syscall::errno::EINVAL) as u64,
    }
}

// ==================== syslog Read Helpers ====================

/// Maximum length of a formatted record header (human-readable, for /proc/kmsg).
/// Format: `[    0.000000] info: pid(1) cpu(0): `
const MAX_HEADER_LEN: usize = 52;

/// Maximum length of syslog-format header.
/// Format: `<6>[    0.000000] pid(12345) cpu(3): ` — ~40 bytes max.
const SYSLOG_HEADER_LEN: usize = 48;

/// Format a syslog-format record header for syslog(2) reads.
///
/// Format: `<level>[SSSSSS.MMMMMM] pid(N) cpu(M): `
fn format_syslog_header(buf: &mut [u8; SYSLOG_HEADER_LEN], level: u8, timestamp: u64, pid: u32, cpu_id: u16) -> usize {
    let secs = timestamp / 10_000_000;
    let frac_us = ((timestamp % 10_000_000) * 1_000_000) / 10_000_000;
    let mut pos = 0;

    // "<level>"
    buf[pos] = b'<';
    pos += 1;
    buf[pos] = b'0' + level;
    pos += 1;
    buf[pos] = b'>';
    pos += 1;

    // "[SSSSSS.MMMMMM]"
    buf[pos] = b'[';
    pos += 1;
    let mut sec_buf = [b' '; 6];
    let mut s = secs;
    let mut digits = 0usize;
    if s == 0 {
        sec_buf[5] = b'0';
    } else {
        while s > 0 && digits < 6 {
            sec_buf[5 - digits] = b'0' + (s % 10) as u8;
            s /= 10;
            digits += 1;
        }
    }
    buf[pos..pos + 6].copy_from_slice(&sec_buf);
    pos += 6;
    buf[pos] = b'.';
    pos += 1;
    let mut us_buf = [b'0'; 6];
    let mut u = frac_us.min(999999);
    let mut udigits = 0usize;
    if u > 0 {
        while u > 0 && udigits < 6 {
            us_buf[5 - udigits] = b'0' + (u % 10) as u8;
            u /= 10;
            udigits += 1;
        }
    }
    buf[pos..pos + 6].copy_from_slice(&us_buf);
    pos += 6;
    buf[pos] = b']';
    pos += 1;
    buf[pos] = b' ';
    pos += 1;

    // "pid(N) "
    buf[pos..pos + 4].copy_from_slice(b"pid(");
    pos += 4;
    let mut pid_buf = [0u8; 10];
    let mut pdigits = 0usize;
    let mut p = pid as usize;
    if p == 0 {
        pid_buf[0] = b'0';
        pdigits = 1;
    } else {
        while p > 0 && pdigits < 10 {
            pid_buf[9 - pdigits] = b'0' + (p % 10) as u8;
            p /= 10;
            pdigits += 1;
        }
    }
    buf[pos..pos + pdigits].copy_from_slice(&pid_buf[10 - pdigits..]);
    pos += pdigits;
    buf[pos..pos + 2].copy_from_slice(b") ");
    pos += 2;

    // "cpu(M): "
    buf[pos..pos + 4].copy_from_slice(b"cpu(");
    pos += 4;
    let mut cpu_buf = [0u8; 4];
    let mut cdigits = 0usize;
    let mut c = cpu_id as usize;
    if c == 0 {
        cpu_buf[0] = b'0';
        cdigits = 1;
    } else {
        while c > 0 && cdigits < 4 {
            cpu_buf[3 - cdigits] = b'0' + (c % 10) as u8;
            c /= 10;
            cdigits += 1;
        }
    }
    buf[pos..pos + cdigits].copy_from_slice(&cpu_buf[4 - cdigits..]);
    pos += cdigits;
    buf[pos..pos + 3].copy_from_slice(b"): ");
    pos += 3;

    pos
}

/// Format a record header into a buffer, return bytes written.
///
/// Format: `[SSSSSS.MMMMMMM] level_name: pid(N) cpu(M): `
/// Timestamp is seconds from boot (cycles / 10_000_000).
fn format_record_header(buf: &mut [u8; MAX_HEADER_LEN], level: u8, pid: u32, cpu_id: u16, timestamp: u64) -> usize {
    // Convert cycles to seconds (TIMER_FREQ = 10MHz)
    let secs = timestamp / 10_000_000;
    let frac_us = ((timestamp % 10_000_000) * 1_000_000) / 10_000_000;

    // Level name
    let name: &[u8] = match level {
        0 => b"emerg",
        1 => b"alert",
        2 => b"crit",
        3 => b"err",
        4 => b"warn",
        5 => b"notice",
        6 => b"info",
        7 => b"debug",
        _ => b"unknown",
    };

    let mut pos = 0;

    // Write "[SSSSSS.MMMMMM] " (dmesg-style timestamp)
    buf[pos] = b'[';
    pos += 1;
    // Format seconds as right-aligned in 6 chars (e.g. "     0" or "  1234")
    let mut sec_buf = [b' '; 6];
    let mut s = secs;
    let mut digits = 0usize;
    if s == 0 {
        sec_buf[5] = b'0';
        digits = 1;
    } else {
        while s > 0 && digits < 6 {
            sec_buf[5 - digits] = b'0' + (s % 10) as u8;
            s /= 10;
            digits += 1;
        }
    }
    buf[pos..pos + 6].copy_from_slice(&sec_buf);
    pos += 6;
    buf[pos] = b'.';
    pos += 1;
    // Format fractional microseconds as 6 digits
    let mut us_buf = [b'0'; 6];
    let mut u = frac_us.min(999999);
    let mut udigits = 0usize;
    if u == 0 {
        // already all zeros
    } else {
        while u > 0 && udigits < 6 {
            us_buf[5 - udigits] = b'0' + (u % 10) as u8;
            u /= 10;
            udigits += 1;
        }
    }
    buf[pos..pos + 6].copy_from_slice(&us_buf);
    pos += 6;
    buf[pos] = b']';
    pos += 1;
    buf[pos] = b' ';
    pos += 1;

    // Write "level: "
    buf[pos..pos + name.len()].copy_from_slice(name);
    pos += name.len();
    buf[pos..pos + 2].copy_from_slice(b": ");
    pos += 2;

    // Write "pid(N): " — manual integer formatting
    buf[pos..pos + 4].copy_from_slice(b"pid(");
    pos += 4;
    let mut pid_buf = [0u8; 10];
    let mut pdigits = 0usize;
    let mut p = pid as usize;
    if p == 0 {
        pid_buf[0] = b'0';
        pdigits = 1;
    } else {
        while p > 0 && pdigits < 10 {
            pid_buf[9 - pdigits] = b'0' + (p % 10) as u8;
            p /= 10;
            pdigits += 1;
        }
    }
    buf[pos..pos + pdigits].copy_from_slice(&pid_buf[10 - pdigits..]);
    pos += pdigits;
    buf[pos..pos + 2].copy_from_slice(b") ");
    pos += 2;

    // Write "cpu(M): "
    buf[pos..pos + 4].copy_from_slice(b"cpu(");
    pos += 4;
    let mut cpu_buf = [0u8; 4];
    let mut cdigits = 0usize;
    let mut c = cpu_id as usize;
    if c == 0 {
        cpu_buf[0] = b'0';
        cdigits = 1;
    } else {
        while c > 0 && cdigits < 4 {
            cpu_buf[3 - cdigits] = b'0' + (c % 10) as u8;
            c /= 10;
            cdigits += 1;
        }
    }
    buf[pos..pos + cdigits].copy_from_slice(&cpu_buf[4 - cdigits..]);
    pos += cdigits;
    buf[pos..pos + 3].copy_from_slice(b"): ");
    pos += 3;

    pos
}

/// Read records sequentially from where the last read left off.
fn syslog_read_sequential(bufp: *mut u8, maxlen: usize) -> u64 {
    if maxlen == 0 || bufp.is_null() {
        return (-crate::syscall::errno::EINVAL) as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(bufp as usize, maxlen) {
        return (-crate::syscall::errno::EFAULT) as u64;
    }

    let mut rb = RING_BUFFER.lock_irqsave();
    let read_seq = rb.read_seq;
    let next_seq = rb.next_seq;

    if read_seq >= next_seq {
        // Nothing new to read
        return 0;
    }

    let mut offset = 0usize;
    let mut header_buf = [0u8; SYSLOG_HEADER_LEN];

    // Iterate through all slots, find records with seq >= read_seq
    for i in 0..RING_BUFFER_CAPACITY {
        let record = &rb.records[i];
        if record.text_len == 0 || record.seq < read_seq {
            continue;
        }

        // Format: <level>[timestamp] pid(N) cpu(M): text\n
        let header_len = format_syslog_header(&mut header_buf, record.level, record.timestamp, record.pid, record.cpu_id);
        let text_bytes = &record.text[..record.text_len as usize];
        let trailing_nl = text_bytes.last() == Some(&b'\n');
        let needed = header_len + text_bytes.len() + if trailing_nl { 0 } else { 1 };

        if offset + needed > maxlen {
            break;
        }

        // SAFETY: bufp has capacity maxlen and offset + needed <= maxlen; header_buf and
        // text_bytes are valid stack/buffer slices of exactly header_len/text_bytes.len().
        unsafe {
            core::ptr::copy_nonoverlapping(header_buf.as_ptr(), bufp.add(offset), header_len);
            offset += header_len;
            core::ptr::copy_nonoverlapping(text_bytes.as_ptr(), bufp.add(offset), text_bytes.len());
            offset += text_bytes.len();
            if !trailing_nl {
                *bufp.add(offset) = b'\n';
                offset += 1;
            }
        }
    }

    // Advance read cursor
    rb.read_seq = next_seq;

    offset as u64
}

/// Read all records from the ring buffer, oldest first.
fn syslog_read_all(bufp: *mut u8, maxlen: usize, clear: bool) -> u64 {
    if maxlen == 0 || bufp.is_null() {
        return (-crate::syscall::errno::EINVAL) as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(bufp as usize, maxlen) {
        return (-crate::syscall::errno::EFAULT) as u64;
    }

    let mut rb = RING_BUFFER.lock_irqsave();
    let next_seq = rb.next_seq;

    if next_seq == 0 {
        return 0;
    }

    let mut offset = 0usize;
    let mut header_buf = [0u8; SYSLOG_HEADER_LEN];
    let write_idx = rb.write_idx;

    // Read records from oldest to newest.
    // The oldest record is at write_idx (the slot about to be overwritten).
    for i in 0..RING_BUFFER_CAPACITY {
        let idx = (write_idx + i) % RING_BUFFER_CAPACITY;
        let record = &rb.records[idx];

        // Skip empty/zeroed records
        if record.text_len == 0 {
            continue;
        }

        // Format: <level>[timestamp] pid(N) cpu(M): text\n
        let header_len = format_syslog_header(&mut header_buf, record.level, record.timestamp, record.pid, record.cpu_id);
        let text_bytes = &record.text[..record.text_len as usize];
        let trailing_nl = text_bytes.last() == Some(&b'\n');
        let needed = header_len + text_bytes.len() + if trailing_nl { 0 } else { 1 };

        if offset + needed > maxlen {
            break;
        }

        // SAFETY: same as above — bufp has capacity maxlen and offset + needed <= maxlen.
        unsafe {
            core::ptr::copy_nonoverlapping(header_buf.as_ptr(), bufp.add(offset), header_len);
            offset += header_len;
            core::ptr::copy_nonoverlapping(text_bytes.as_ptr(), bufp.add(offset), text_bytes.len());
            offset += text_bytes.len();
            if !trailing_nl {
                *bufp.add(offset) = b'\n';
                offset += 1;
            }
        }
    }

    if clear {
        // Clear all records
        for record in rb.records.iter_mut() {
            record.text_len = 0;
        }
        rb.write_idx = 0;
        rb.read_seq = 0;
        rb.next_seq = 0;
    }

    offset as u64
}

/// Clear the ring buffer.
fn syslog_clear() {
    let mut rb = RING_BUFFER.lock_irqsave();
    for record in rb.records.iter_mut() {
        record.text_len = 0;
    }
    rb.write_idx = 0;
    rb.read_seq = 0;
    rb.next_seq = 0;
}

// ==================== ProcFS /dev/kmsg Support ====================

/// Generate kmsg content for /proc/kmsg.
/// Format: "<level>text\n" per record, matching dmesg expected format.
pub fn generate_kmsg() -> alloc::vec::Vec<u8> {
    let rb = RING_BUFFER.lock_irqsave();
    let next_seq = rb.next_seq;

    if next_seq == 0 {
        return alloc::vec::Vec::new();
    }

    let mut result = alloc::vec::Vec::new();
    let mut header_buf = [0u8; MAX_HEADER_LEN];
    let write_idx = rb.write_idx;

    // Read records from oldest to newest
    for i in 0..RING_BUFFER_CAPACITY {
        let idx = (write_idx + i) % RING_BUFFER_CAPACITY;
        let record = &rb.records[idx];

        if record.text_len == 0 {
            continue;
        }

        // Skip whitespace-only records (e.g. bare newlines from println!(""))
        let text_bytes = &record.text[..record.text_len as usize];
        if text_bytes.iter().all(|&b| b == b'\n' || b == b'\r') {
            continue;
        }

        // Format: [timestamp] level: pid(N) cpu(M): text\n
        let header_len = format_record_header(&mut header_buf, record.level, record.pid, record.cpu_id, record.timestamp);
        result.extend_from_slice(&header_buf[..header_len]);
        result.extend_from_slice(text_bytes);
        // Only append newline if text doesn't already end with one
        if text_bytes.last() != Some(&b'\n') {
            result.push(b'\n');
        }
    }

    result
}

// ==================== /dev/kmsg Character Device ====================

/// Read handler for /dev/kmsg.
///
/// Each read() returns one record in /dev/kmsg format:
/// `priority,sequence,timestamp_us,-;text\n`
///
/// Uses the file position as the sequence number to track which record to read next.
fn kmsg_file_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    let mut rb = RING_BUFFER.lock_irqsave();
    let pos = file.get_pos();
    let next_seq = rb.next_seq;

    // No new records
    if pos >= next_seq {
        return 0;
    }

    // Find oldest available sequence
    let oldest = if next_seq > RING_BUFFER_CAPACITY as u64 {
        next_seq - RING_BUFFER_CAPACITY as u64
    } else {
        0
    };

    // If requested position is too old, skip to oldest
    let seq = if pos < oldest { oldest } else { pos };

    let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
    let record = &rb.records[idx];

    // Check if record was overwritten
    if record.text_len == 0 || record.seq != seq {
        // Record was overwritten, skip to next available
        file.set_pos(next_seq);
        return 0;
    }

    // Format: "level,seq,timestamp_us,-;text\n"
    let timestamp_us = record.timestamp / 10; // cycles to microseconds (10MHz clock)

    // Format into a local buffer
    let mut line_buf = [0u8; 300];
    let mut pos = 0;

    // Write "level,seq,timestamp_us,-;"
    let header = alloc::format!("{},{},{},-;", record.level, record.seq, timestamp_us);
    let header_bytes = header.as_bytes();
    let header_len = header_bytes.len().min(line_buf.len());
    line_buf[..header_len].copy_from_slice(&header_bytes[..header_len]);
    pos = header_len;

    // Write message text
    let text_bytes = &record.text[..record.text_len as usize];
    let text_len = text_bytes.len().min(line_buf.len() - pos);
    line_buf[pos..pos + text_len].copy_from_slice(&text_bytes[..text_len]);
    pos += text_len;

    // Ensure newline
    if pos > 0 && line_buf[pos - 1] != b'\n' {
        if pos < line_buf.len() {
            line_buf[pos] = b'\n';
            pos += 1;
        }
    }

    let data = &line_buf[..pos];
    let copy_len = data.len().min(buf.len());
    if copy_len == 0 {
        return 0;
    }
    buf[..copy_len].copy_from_slice(&data[..copy_len]);

    // Advance position to next record
    file.set_pos(seq + 1);

    // Drop lock before returning
    drop(rb);

    copy_len as isize
}

/// lseek handler for /dev/kmsg.
fn kmsg_file_lseek(file: &crate::fs::file::File, offset: isize, whence: i32) -> isize {
    match whence {
        0 => {
            // SEEK_SET
            file.set_pos(offset as u64);
            offset
        }
        1 => {
            // SEEK_CUR
            let new_pos = file.get_pos() as i64 + offset as i64;
            file.set_pos(new_pos as u64);
            new_pos as isize
        }
        2 => {
            // SEEK_END: set to next_seq (to read future messages)
            let rb = RING_BUFFER.lock_irqsave();
            let next_seq = rb.next_seq;
            drop(rb);
            file.set_pos(next_seq);
            next_seq as isize
        }
        _ => -crate::syscall::errno::EINVAL as isize,
    }
}

/// FileOps for /dev/kmsg
static KMSG_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(kmsg_file_read),
    write: None,
    lseek: Some(kmsg_file_lseek),
    close: None,
    poll: None,
};

/// Initialize /dev/kmsg device node.
///
/// Must be called after devfs is initialized.
pub fn init_kmsg_device() {
    use crate::fs::devfs;
    use crate::fs::dev_t::DEV_KMSG;

    devfs::registry::register_char_device(DEV_KMSG, &KMSG_OPS);
    let _ = devfs::mknod("kmsg", DEV_KMSG, 0o666 | 0o20000);
}

// ==================== Convenience Macros ====================

/// printk with KERN_EMERG level (0) - system is unusable
#[macro_export]
macro_rules! pr_emerg {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_EMERG, format_args!($($arg)*))
    });
}

/// printk with KERN_ALERT level (1) - action must be taken immediately
#[macro_export]
macro_rules! pr_alert {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_ALERT, format_args!($($arg)*))
    });
}

/// printk with KERN_CRIT level (2) - critical conditions
#[macro_export]
macro_rules! pr_crit {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_CRIT, format_args!($($arg)*))
    });
}

/// printk with KERN_ERR level (3) - error conditions
#[macro_export]
macro_rules! pr_err {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_ERR, format_args!($($arg)*))
    });
}

/// printk with KERN_WARNING level (4) - warning conditions
#[macro_export]
macro_rules! pr_warn {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_WARNING, format_args!($($arg)*))
    });
}

/// printk with KERN_NOTICE level (5) - normal but significant
#[macro_export]
macro_rules! pr_notice {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_NOTICE, format_args!($($arg)*))
    });
}

/// printk with KERN_INFO level (6) - informational
#[macro_export]
macro_rules! pr_info {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_INFO, format_args!($($arg)*))
    });
}

/// printk with KERN_DEBUG level (7) - debug-level messages
/// Only compiled in with debug_assertions.
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! pr_debug {
    ($($arg:tt)*) => ({
        $crate::printk::printk($crate::printk::loglevel::KERN_DEBUG, format_args!($($arg)*))
    });
}

#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! pr_debug {
    ($($arg:tt)*) => ({
        // No-op in release mode
    });
}

// ==================== log Crate Integration ====================

use log::{Level, LevelFilter, Log, Metadata, Record as LogRecordTrait};

struct PrintkLogger;

impl Log for PrintkLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let level = match metadata.level() {
            Level::Error => loglevel::KERN_ERR,
            Level::Warn => loglevel::KERN_WARNING,
            Level::Info => loglevel::KERN_INFO,
            Level::Debug | Level::Trace => loglevel::KERN_DEBUG,
        };
        level <= CONSOLE_LOGLEVEL.load(Ordering::Relaxed)
    }

    fn log(&self, record: &LogRecordTrait) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            Level::Error => loglevel::KERN_ERR,
            Level::Warn => loglevel::KERN_WARNING,
            Level::Info => loglevel::KERN_INFO,
            Level::Debug | Level::Trace => loglevel::KERN_DEBUG,
        };
        printk(level, *record.args());
    }

    fn flush(&self) {
        // UART writes are synchronous, nothing to flush
    }
}

/// Install the printk logger as the global logger.
/// Must be called once during early boot.
pub fn init_logger() {
    static LOGGER: PrintkLogger = PrintkLogger;
    // SAFETY: called once during early boot; LOGGER is a static with 'static lifetime.
    unsafe {
        log::set_logger(&LOGGER).ok();
    }
    log::set_max_level(LevelFilter::Trace);
}

// ==================== Persistent Log (kmsg to disk) ====================

/// Persistent kernel log module.
///
/// Writes every printk message to `/var/log/kmsg` on the ext4 filesystem.
/// Uses ring buffer behavior: fixed max size (256KB), wraps around to overwrite
/// oldest data. This ensures that after a panic/reboot, the most recent kernel
/// messages can be recovered from disk.
mod persistent_log {
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    /// Maximum log file size (1MB)
    const MAX_LOG_SIZE: u64 = 1024 * 1024;

    /// Path to the persistent log file
    const LOG_PATH: &str = "/var/log/kmsg";

    /// Whether the persistent log has been initialized (ext4 mounted)
    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    /// Cached inode number for the log file (0 = not yet resolved)
    static FILE_INO: AtomicU32 = AtomicU32::new(0);

    /// Current write offset in the log file (ring buffer style)
    static WRITE_OFFSET: AtomicU64 = AtomicU64::new(0);

    /// Log level name strings
    const LEVEL_NAMES: &[&str] = &[
        "emerg", "alert", "crit", "err", "warn", "notice", "info", "debug",
    ];

    /// Initialize persistent logging.
    ///
    /// Must be called after ext4 filesystem is mounted.
    /// Safe to call multiple times.
    pub fn init() {
        // Pre-create the log file so it exists before first append
        if let Some(fs_ptr) = crate::fs::ext4::get_ext4_fs() {
            if !fs_ptr.is_null() {
                // SAFETY: fs_ptr is a valid pointer returned by get_ext4_fs() when non-null.
                let fs = unsafe { &*fs_ptr };
                if let Ok((ino, _inode)) = fs.lookup_path(LOG_PATH) {
                    FILE_INO.store(ino, Ordering::Relaxed);
                } else if crate::fs::ext4::create_file(LOG_PATH, 0o644).is_ok() {
                    // File created, inode will be resolved on first write
                }
            }
        }
        INITIALIZED.store(true, Ordering::Relaxed);
    }

    /// Append a log message to the persistent log file.
    ///
    /// Called from `write_to_ring_buffer()` after the ring buffer lock is dropped.
    /// All errors are silently ignored — persistent logging must not affect normal operation.
    pub fn append(level: u8, text: &[u8], seq: u64, pid: u32, cpu_id: u16, timestamp: u64) {
        if !INITIALIZED.load(Ordering::Relaxed) {
            return;
        }

        // Get ext4 filesystem
        let fs_ptr = match crate::fs::ext4::get_ext4_fs() {
            Some(ptr) if !ptr.is_null() => ptr,
            _ => return,
        };
        // SAFETY: fs_ptr is a valid pointer returned by get_ext4_fs() when non-null.
        let fs = unsafe { &*fs_ptr };

        // Resolve or create the log file
        let file_ino = FILE_INO.load(Ordering::Relaxed);
        let file_ino = if file_ino != 0 {
            file_ino
        } else {
            // Try to lookup existing file
            match fs.lookup_path(LOG_PATH) {
                Ok((ino, _inode)) => {
                    FILE_INO.store(ino, Ordering::Relaxed);
                    ino
                }
                Err(_) => {
                    // File doesn't exist, try to create it
                    match crate::fs::ext4::create_file(LOG_PATH, 0o644) {
                        Ok(vfs_inode) => {
                            let ino = vfs_inode.ino as u32;
                            FILE_INO.store(ino, Ordering::Relaxed);
                            WRITE_OFFSET.store(0, Ordering::Relaxed);
                            ino
                        }
                        Err(_) => return, // Cannot create file, give up silently
                    }
                }
            }
        };

        // Read current inode to get file size
        let mut ext4_inode = match fs.read_inode(file_ino) {
            Ok(inode) => inode,
            Err(_) => {
                // Inode may have been deleted, reset and retry next time
                FILE_INO.store(0, Ordering::Relaxed);
                return;
            }
        };

        // Format log line: "[seq] [timestamp_us] level: pid(N) cpu(M): text\n"
        let level_name = if (level as usize) < LEVEL_NAMES.len() {
            LEVEL_NAMES[level as usize]
        } else {
            "unk"
        };
        let timestamp_us = timestamp / 1000;

        let mut line_buf = [0u8; 300];
        let mut pos = 0;

        // Write header: [seq] [timestamp_us] level: pid(N) cpu(M):
        let header = alloc::format!("[{}] [{}] {}: pid({}) cpu({}): ", seq, timestamp_us, level_name, pid, cpu_id);
        let header_bytes = header.as_bytes();
        let header_len = header_bytes.len().min(line_buf.len());
        line_buf[..header_len].copy_from_slice(&header_bytes[..header_len]);
        pos = header_len;

        // Write message text
        let text_len = text.len().min(line_buf.len() - pos - 1);
        line_buf[pos..pos + text_len].copy_from_slice(&text[..text_len]);
        pos += text_len;

        // Ensure newline
        if pos > 0 && line_buf[pos - 1] != b'\n' {
            if pos < line_buf.len() {
                line_buf[pos] = b'\n';
                pos += 1;
            }
        }

        let data = &line_buf[..pos];

        // Get write offset
        let mut offset = WRITE_OFFSET.load(Ordering::Relaxed);

        // Write data
        match crate::fs::ext4::file::ext4_file_write(fs, &mut ext4_inode, offset, data) {
            Ok(written) => {
                offset += written as u64;

                // If we exceeded max size, wrap around
                if offset >= MAX_LOG_SIZE {
                    offset = 0;
                }

                WRITE_OFFSET.store(offset, Ordering::Relaxed);

                // Update inode size (for ring buffer, size = max of current offset and previous size)
                let new_size = if offset == 0 {
                    MAX_LOG_SIZE // Wrapped: file is full
                } else {
                    offset.max(ext4_inode.get_size())
                };
                ext4_inode.size = new_size;

                // Write back inode
                let _ = crate::fs::ext4::inode::write_inode(fs, file_ino, &ext4_inode);
            }
            Err(_) => {
                // Write failed, reset inode cache
                FILE_INO.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Flush any pending log data.
    ///
    /// ext4_file_write already writes synchronously to disk, so this is a no-op.
    /// Provided for API completeness (called from panic handler).
    pub fn flush() {
        // No-op: ext4 writes are synchronous
    }
}

/// Initialize persistent kernel log.
///
/// Must be called after ext4 filesystem is mounted.
pub fn persistent_log_init() {
    persistent_log::init();
}

/// Flush persistent log to disk (no-op since ext4 writes are synchronous).
pub fn persistent_log_flush() {
    persistent_log::flush();
}
