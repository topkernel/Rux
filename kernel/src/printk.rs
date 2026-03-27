//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Linux-style printk with log levels and ring buffer
//!
//! Provides leveled kernel logging (pr_emerg through pr_debug),
//! a ring buffer for storing all messages, and a syslog(2) syscall
//! for userspace `dmesg` to read/manage kernel logs.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

extern crate alloc;

use spin::Mutex;

// ==================== Log Level Constants ====================

/// Log levels matching Linux kernel exactly.
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

    /// Default console log level: show everything up to and including KERN_DEBUG.
    pub const DEFAULT_CONSOLE_LOGLEVEL: u8 = KERN_DEBUG;
}

// ==================== Ring Buffer ====================

/// Maximum text payload per record (bytes).
const RECORD_TEXT_SIZE: usize = 256;

/// Record metadata size: level(1) + pad(1) + text_len(2) + seq(8) + timestamp(8) = 20 bytes.
const RECORD_META_SIZE: usize = 20;

/// Total size of one record (metadata + text).
const RECORD_TOTAL_SIZE: usize = RECORD_META_SIZE + RECORD_TEXT_SIZE; // 276 bytes

/// Number of ring buffer record slots, computed from configurable total size.
const RING_BUFFER_CAPACITY: usize = crate::config::PRINTK_RING_BUFFER_SIZE / RECORD_TOTAL_SIZE;

/// A single log record in the ring buffer.
#[repr(C)]
#[derive(Clone, Copy)]
struct LogRecord {
    /// Log level (0-7).
    level: u8,
    /// Padding for alignment.
    _pad: u8,
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
                _pad: 0,
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
static RING_BUFFER: Mutex<RingBuffer> = Mutex::new(RingBuffer::new());

// ==================== Global State ====================

/// Runtime console log level. Messages with level <= this value are printed to UART.
/// Starts at 7 (KERN_DEBUG) so all boot messages are visible.
static CONSOLE_LOGLEVEL: AtomicU8 = AtomicU8::new(loglevel::DEFAULT_CONSOLE_LOGLEVEL);

/// Set during boot after printk is ready.
static PRINTK_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Re-entrancy guard to prevent recursive printk.
static IN_PRINTK: AtomicBool = AtomicBool::new(false);

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
/// 2. Writes to the ring buffer
/// 3. If level <= console_loglevel, also writes to UART
pub fn printk(level: u8, args: fmt::Arguments) {
    // Re-entrancy guard: if already in printk, use direct UART output as fallback
    if IN_PRINTK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        let mut buf = [0u8; RECORD_TEXT_SIZE];
        let mut writer = BufferWriter { buf: &mut buf, pos: 0 };
        let _ = fmt::Write::write_fmt(&mut writer, args);
        let len = writer.pos;
        if len > 0 {
            crate::console::puts_no_lock(
                core::str::from_utf8(&buf[..len]).unwrap_or("(non-utf8)"),
            );
        }
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

    // Check console log level and write to UART
    if level <= CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        let uart = crate::console::lock();
        for &b in &buf[..text_len] {
            if b == b'\n' {
                uart.putc(b'\r');
            }
            uart.putc(b);
        }
    }

    IN_PRINTK.store(false, Ordering::Release);
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
    // Re-entrancy guard
    if IN_PRINTK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        crate::console::puts_no_lock(
            core::str::from_utf8(text).unwrap_or("(non-utf8)"),
        );
        return;
    }

    // Write to ring buffer (if initialized)
    if PRINTK_INITIALIZED.load(Ordering::Relaxed) {
        let timestamp = crate::drivers::intc::clint::read_time();
        write_to_ring_buffer(level, text, timestamp);
    }

    // Check console log level and write to UART
    if level <= CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        let uart = crate::console::lock();
        for &b in text {
            if b == b'\n' {
                uart.putc(b'\r');
            }
            uart.putc(b);
        }
    }

    IN_PRINTK.store(false, Ordering::Release);
}

// ==================== Ring Buffer Write ====================

fn write_to_ring_buffer(level: u8, text: &[u8], timestamp: u64) {
    let mut rb = RING_BUFFER.lock();

    let idx = rb.write_idx;
    let seq = rb.next_seq;

    let record = &mut rb.records[idx];
    record.level = level;
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
}

// ==================== syslog Syscall ====================

/// syslog(2) syscall implementation.
///
/// Linux ABI: `int syslog(int type, char *bufp, int len);`
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
        // Close/Open: no-op in Linux, return success
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
            let rb = RING_BUFFER.lock();
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

/// Maximum length of a level header string (e.g. "<notice:>").
const MAX_LEVEL_HEADER_LEN: usize = 10;

/// Format a `<name:N>` header into a buffer, return bytes written.
fn format_level_header(buf: &mut [u8; MAX_LEVEL_HEADER_LEN], level: u8) -> usize {
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
    // Format: "<name:N>"  e.g. "<info:6>"
    buf[0] = b'<';
    let n = name.len();
    buf[1..1 + n].copy_from_slice(name);
    buf[1 + n] = b':';
    buf[2 + n] = b'0' + level;
    buf[3 + n] = b'>';
    4 + n
}

/// Read records sequentially from where the last read left off.
fn syslog_read_sequential(bufp: *mut u8, maxlen: usize) -> u64 {
    if maxlen == 0 || bufp.is_null() {
        return (-crate::syscall::errno::EINVAL) as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(bufp as usize, maxlen) {
        return (-crate::syscall::errno::EFAULT) as u64;
    }

    let mut rb = RING_BUFFER.lock();
    let read_seq = rb.read_seq;
    let next_seq = rb.next_seq;

    if read_seq >= next_seq {
        // Nothing new to read
        return 0;
    }

    let mut offset = 0usize;
    let mut header_buf = [0u8; MAX_LEVEL_HEADER_LEN];

    // Iterate through all slots, find records with seq >= read_seq
    for i in 0..RING_BUFFER_CAPACITY {
        let record = &rb.records[i];
        if record.text_len == 0 || record.seq < read_seq {
            continue;
        }

        // Format: "<level>text\n"
        let header_len = format_level_header(&mut header_buf, record.level);
        let text_bytes = &record.text[..record.text_len as usize];
        let trailing_nl = text_bytes.last() == Some(&b'\n');
        let needed = header_len + text_bytes.len() + if trailing_nl { 0 } else { 1 };

        if offset + needed > maxlen {
            break;
        }

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

    let mut rb = RING_BUFFER.lock();
    let next_seq = rb.next_seq;

    if next_seq == 0 {
        return 0;
    }

    let mut offset = 0usize;
    let mut header_buf = [0u8; MAX_LEVEL_HEADER_LEN];
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

        let header_len = format_level_header(&mut header_buf, record.level);
        let text_bytes = &record.text[..record.text_len as usize];
        let trailing_nl = text_bytes.last() == Some(&b'\n');
        let needed = header_len + text_bytes.len() + if trailing_nl { 0 } else { 1 };

        if offset + needed > maxlen {
            break;
        }

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
    let mut rb = RING_BUFFER.lock();
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
    let rb = RING_BUFFER.lock();
    let next_seq = rb.next_seq;

    if next_seq == 0 {
        return alloc::vec::Vec::new();
    }

    let mut result = alloc::vec::Vec::new();
    let mut header_buf = [0u8; MAX_LEVEL_HEADER_LEN];
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

        // Format: "<level>text\n"
        let header_len = format_level_header(&mut header_buf, record.level);
        result.extend_from_slice(&header_buf[..header_len]);
        result.extend_from_slice(text_bytes);
        // Only append newline if text doesn't already end with one
        if text_bytes.last() != Some(&b'\n') {
            result.push(b'\n');
        }
    }

    result
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
    unsafe {
        log::set_logger(&LOGGER).ok();
    }
    log::set_max_level(LevelFilter::Trace);
}
