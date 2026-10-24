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
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

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

/// Re-entrancy guard to prevent recursive printk. PER-CPU (review批次8):
/// a single global flag meant a printk running on CPU A silently dropped
/// every concurrent printk from CPU B (lost logs under exactly the
/// multi-CPU conditions printk matters most).
static IN_PRINTK: [AtomicBool; crate::config::MAX_CPUS] =
    [const { AtomicBool::new(false) }; crate::config::MAX_CPUS];

/// RAII guard for the per-CPU `IN_PRINTK` re-entrancy flag.
///
/// On drop, clears the flag so that a panic inside printk never permanently
/// disables the logging subsystem.
struct PrintkGuard(usize);

impl PrintkGuard {
    /// Try to acquire THIS CPU's printk re-entrancy guard.
    ///
    /// Returns `None` if already inside printk on this CPU (re-entrant call).
    fn try_new() -> Option<Self> {
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= crate::config::MAX_CPUS {
            return None; // unknown CPU: refuse rather than corrupt memory
        }
        if IN_PRINTK[cpu].compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            Some(PrintkGuard(cpu))
        } else {
            None
        }
    }
}

impl Drop for PrintkGuard {
    fn drop(&mut self) {
        IN_PRINTK[self.0].store(false, Ordering::Release);
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
/// 2. If level <= console_loglevel, writes to the ring buffer AND emits
///    synchronously to the console UART (review批次8: the console path was
///    missing entirely — kernel messages were invisible outside panics)
///
/// The console_loglevel filter here drops the message BEFORE the ring:
/// that is a quiet-boot cost guard for internal kernel traffic, but it
/// is NOT Linux /dev/kmsg semantics — userspace kmsg writes must always
/// reach the ring (see printk_ring_always, pa9).
pub fn printk(level: u8, args: fmt::Arguments) {
    // Perf: filter BEFORE formatting (same rationale as printk_ln below —
    // discarded messages must not pay the fmt pass).
    if level > CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        crate::dfx::syscost::note_printk_filtered(); // syscost probe
        return;
    }
    printk_ring_always(level, args);
}

/// printk variant with Linux devkmsg semantics: the record ALWAYS enters
/// the printk ring; CONSOLE_LOGLEVEL only gates the console emission.
/// The old /dev/kmsg write passthrough went through pr_info! and the
/// printk() pre-filter above, so whenever console_loglevel < 6 the whole
/// record was dropped before the ring — OH's boot markers written to
/// /dev/kmsg never reached any hilog consumer (pa9 correctness fix).
fn printk_ring_always(level: u8, args: fmt::Arguments) {
    // Re-entrancy guard: if already in printk, discard
    let _guard = match PrintkGuard::try_new() {
        Some(g) => g,
        None => return,
    };

    // Console TX stall notice (rate-limited): must run OUTSIDE the guard,
    // or the notice itself would be dropped as re-entrant.
    crate::console::report_tx_stall_recovery();

    // Format into a stack-allocated buffer
    let mut buf = [0u8; RECORD_TEXT_SIZE];
    let mut writer = BufferWriter { buf: &mut buf, pos: 0 };
    let _ = fmt::Write::write_fmt(&mut writer, args);
    let text_len = writer.pos.min(RECORD_TEXT_SIZE);

    // Write to ring buffer (if initialized)
    if PRINTK_INITIALIZED.load(Ordering::Relaxed) {
        let timestamp = crate::arch::cpu::read_time();
        write_to_ring_buffer(level, &buf[..text_len], timestamp);
    }

    // Console emit (level gated here: printk() pre-filtered already,
    // printk_ring_always callers pass user-chosen levels).
    if level <= CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        emit_to_console(&buf[..text_len]);
    }
}

/// Console-sink stash (`quietcon` boot param, sc-series launcher race):
/// bytes diverted from the /dev/console UART sink by fs::tty's
/// `console_output` land in the printk ring as a single KERN_INFO
/// record — the UART stays idle while dmesg keeps the full stream.
/// Re-entrant and pre-init safe: drops the bytes in both cases.
pub fn ring_stash_console(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let _guard = match PrintkGuard::try_new() {
        Some(g) => g,
        None => return,
    };
    if PRINTK_INITIALIZED.load(Ordering::Relaxed) {
        let timestamp = crate::arch::cpu::read_time();
        write_to_ring_buffer(loglevel::KERN_INFO, bytes, timestamp);
    }
}

/// Whole-record console atomicity flag: with several harts printing
/// concurrently, per-byte lock-free emission interleaved records
/// mid-line (garbled boot log). Holders only push bytes to the UART
/// (no other locks taken), and same-CPU re-entry is already excluded by
/// the per-CPU IN_PRINTK guard.
///
/// # Bounded acquisition (printk-wedge, OH b140)
///
/// The acquire spin below used to be UNBOUNDED, and the UART byte push
/// itself could stall forever in the x86_64 THRE poll — so a CPU in
/// interrupt context (IF=0) could spin here for eternity while the
/// holder sat wedged in the UART: exactly the b140 wedge (CPU0 inside
/// emit_to_console with IF=0, CPU1 mid-line under the emit lock; serial
/// frozen; 1-in-5 KVM boots). Invariants now:
///
/// - Acquisition is BOUNDED by wall time; the budget depends on context:
///   interrupt context (hardirq/NMI/softirq) gives up almost immediately
///   — spinning with IF=0 must never outlive a slow holder — and task
///   context waits generously (the holder only pushes ≤256 bytes through
///   a *bounded* putc, but may be preempted mid-line or sit behind one
///   bounded UART-stall episode).
/// - On timeout the console emission is SKIPPED. The record is already
///   in the ring buffer (dmesg/syslog still have it); losing one console
///   line under pathological contention beats wedging the box. Linux
///   defers console output under contention for the same reason.
static CONSOLE_EMIT_LOCK: AtomicBool = AtomicBool::new(false);

/// Console emissions skipped because CONSOLE_EMIT_LOCK could not be
/// acquired within the context budget (diagnostic counter; those records
/// remain in the ring buffer).
static CONSOLE_EMIT_SKIPPED: AtomicU64 = AtomicU64::new(0);

/// Read-only view of CONSOLE_EMIT_SKIPPED for probes: the fsw dump prints
/// it (`es=`) so a console round that vanished from the serial is
/// attributable from a surviving line alone — es climbing by the number of
/// lost lines means emit-lock loss (softirq's 100µs budget losing the
/// race); es flat means the records never reached the console path at all
/// (level-filter drop or dead probe — the two pa28 failure classes looked
/// identical from serial and cost two boot iterations to separate).
pub fn console_emit_skipped() -> u64 {
    CONSOLE_EMIT_SKIPPED.load(Ordering::Relaxed)
}

/// Acquire budget in interrupt context (hardirq/NMI/softirq): 100µs of
/// read_time() units. Interrupt context must not spin long — the holder
/// may itself be an interrupt-context emitter on another CPU or a
/// preempted task whose progress this CPU cannot help.
const EMIT_WAIT_IRQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ / 10_000;

/// Acquire budget in task context: 2s. Generous on purpose: a healthy
/// holder finishes a 256-byte line in ~20ms of emulated-baud time, and a
/// stalled one costs at most one bounded putc episode; the slack covers
/// scheduling latency under host contention so healthy consoles never
/// drop lines.
const EMIT_WAIT_TASK: u64 = crate::config::TIMER_CLOCK_FREQ_HZ * 2;

/// Bounded TTAS acquire of CONSOLE_EMIT_LOCK. Returns false when the
/// budget expires (caller skips console output — the record is already
/// in the ring buffer).
fn acquire_emit_lock() -> bool {
    let irq_ctx = crate::interrupt::preempt::in_interrupt();
    let budget = if irq_ctx { EMIT_WAIT_IRQ } else { EMIT_WAIT_TASK };
    let start = crate::arch::cpu::read_time();
    loop {
        if CONSOLE_EMIT_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
        // Check the deadline on EVERY outer iteration: under pure CAS
        // contention (lock bouncing free/held between rivals) the inner
        // TTAS load below sees "free" immediately and never re-checks the
        // clock — without this check the spin would not be bounded.
        if crate::arch::cpu::read_time().saturating_sub(start) >= budget {
            return false;
        }
        // TTAS: park on a relaxed load until the holder releases instead
        // of re-issuing an atomic RMW every iteration (same rationale as
        // RawSpinlock::lock).
        let mut parked: u32 = 0;
        while CONSOLE_EMIT_LOCK.load(Ordering::Relaxed) {
            parked += 1;
            if parked >= 512 {
                parked = 0;
                if crate::arch::cpu::read_time().saturating_sub(start) >= budget {
                    return false;
                }
            }
            core::hint::spin_loop();
        }
    }
}

/// Console (UART) emission path for printk records.
///
/// Uses the LOCK-FREE MMIO writer (putchar_no_lock), never the UART
/// spinlock: printk can run with arbitrary locks held or in IRQ context.
/// Re-entrancy is already excluded by the IN_PRINTK guard in the callers.
fn emit_to_console(text: &[u8]) {
    if !acquire_emit_lock() {
        CONSOLE_EMIT_SKIPPED.fetch_add(1, Ordering::Relaxed);
        return; // record is in the ring buffer; console line skipped
    }
    let mut emitted = 0usize; // syscost probe
    for &b in text {
        if b == b'\n' {
            crate::console::putchar_no_lock(b'\r');
            emitted += 1;
        }
        crate::console::putchar_no_lock(b);
        emitted += 1;
    }
    crate::dfx::syscost::note_printk_con(emitted); // syscost probe
    CONSOLE_EMIT_LOCK.store(false, Ordering::Release);
}

/// Bounded best-effort acquire of the console emit lock for diagnostic
/// writers that bypass printk (dfx `ConsoleWriter`: softlockup reports,
/// stack dumps). Keeps their output from interleaving mid-line with an
/// in-flight printk record (b140's "…len BUG: soft lockup" garble).
///
/// Crash/diagnostic output must never block on a possibly-wedged holder,
/// so the budget is small: 100µs in interrupt context, 10ms otherwise.
/// Returns false on timeout; the caller falls back to lockless output.
pub fn try_console_emit_lock() -> bool {
    let irq_ctx = crate::interrupt::preempt::in_interrupt();
    let budget = if irq_ctx {
        EMIT_WAIT_IRQ
    } else {
        crate::config::TIMER_CLOCK_FREQ_HZ / 100 // 10ms
    };
    let start = crate::arch::cpu::read_time();
    loop {
        if CONSOLE_EMIT_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
        if crate::arch::cpu::read_time().saturating_sub(start) >= budget {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Release CONSOLE_EMIT_LOCK acquired via `try_console_emit_lock`.
pub fn console_emit_unlock() {
    CONSOLE_EMIT_LOCK.store(false, Ordering::Release);
}

/// Write a formatted message with trailing newline to the kernel log.
/// Used by the `println!` macro.
pub fn printk_ln(level: u8, args: fmt::Arguments) {
    // Perf: filter BEFORE formatting. core::fmt argument construction and
    // the format pass itself dominate the cost of a printk — with the
    // check only in printk_bytes, every discarded pr_debug! on a hot path
    // (the per-trap trace in trap.rs runs on EVERY syscall, page fault and
    // interrupt) still paid full formatting into a stack buffer that was
    // then thrown away. Semantics are unchanged: printk_bytes drops
    // filtered messages after its re-entrancy guard anyway; skipping the
    // guard+format first only removes work (measured ~70% of the getpid
    // syscall cost under TCG debug builds).
    if level > CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
        return;
    }
    // Format with newline into a local buffer to avoid borrow conflict
    let mut buf = [0u8; RECORD_TEXT_SIZE];
    let mut writer = BufferWriter { buf: &mut buf, pos: 0 };
    let _ = fmt::Write::write_fmt(&mut writer, format_args!("{}\n", args));
    let text_len = writer.pos.min(RECORD_TEXT_SIZE);
    printk_bytes(level, &buf[..text_len]);
}

/// Write raw bytes to the kernel log (used by printk_ln to avoid double formatting).
fn printk_bytes(level: u8, text: &[u8]) {
    // Console TX stall notice (rate-limited): must run OUTSIDE the guard,
    // or the notice itself would be dropped as re-entrant.
    crate::console::report_tx_stall_recovery();

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
        let timestamp = crate::arch::cpu::read_time();
        write_to_ring_buffer(level, text, timestamp);
    }

    // Console emit (level filter already passed above)
    emit_to_console(text);
}

// ==================== Ring Buffer Write ====================

fn write_to_ring_buffer(level: u8, text: &[u8], timestamp: u64) {
    crate::dfx::syscost::note_printk_ring(text.len()); // syscost probe
    let pid = crate::process::current_pid() as u32;
    // Real CPU id (review批次8: hard-coded 0 attributed every message to
    // CPU 0, hiding cross-CPU issues in the logs).
    let cpu_id: u16 = (crate::arch::cpu_id() as u16).min(u16::MAX);

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

    // Wake /dev/kmsg readers: a blocking kmsg read (Linux semantics)
    // sleeps until the next record lands. The waitqueue wake is
    // IRQ-context safe (irqsave discipline inside).
    KMSG_READ_WQ.wake_up_all();

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
pub fn sys_syslog(args: [u64; 6]) -> i64 {
    let action = args[0] as i32;
    let bufp = args[1] as *mut u8;
    let len = args[2] as usize;

    // Permission check (Linux check_syslog_permissions): everything except
    // the size queries (3/10 with dmesg_restrict=0 semantics) requires
    // CAP_SYSLOG — otherwise any process could read kernel logs (kernel
    // address leaks), wipe them, or silence the console.
    if !matches!(action, 0 | 1 | 10) {
        if !crate::security::capable(crate::security::CAP_SYSLOG) {
            return -(crate::syscall::errno::EPERM as i64);
        }
    }

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
                return -(crate::syscall::errno::EINVAL as i64);
            }
            CONSOLE_LOGLEVEL.store(new_level, Ordering::Relaxed);
            0
        }

        // Return unread bytes (approximation)
        9 => {
            let rb = RING_BUFFER.lock_irqsave();
            let unread = rb.next_seq.saturating_sub(rb.read_seq);
            // Approximate: assume each record is ~128 bytes average
            (unread * 128) as i64
        }

        // Return total buffer size
        10 => (RING_BUFFER_CAPACITY * RECORD_TEXT_SIZE) as i64,

        _ => -(crate::syscall::errno::EINVAL as i64),
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
    let secs = timestamp / crate::config::TIMER_CLOCK_FREQ_HZ;
    let frac_us = ((timestamp % crate::config::TIMER_CLOCK_FREQ_HZ) * 1_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ;
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
/// Timestamp is seconds from boot (cycles / TIMER_CLOCK_FREQ_HZ).
fn format_record_header(buf: &mut [u8; MAX_HEADER_LEN], level: u8, pid: u32, cpu_id: u16, timestamp: u64) -> usize {
    // Convert cycles to seconds (TIMER_FREQ = 10MHz)
    let secs = timestamp / crate::config::TIMER_CLOCK_FREQ_HZ;
    let frac_us = ((timestamp % crate::config::TIMER_CLOCK_FREQ_HZ) * 1_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ;

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
fn syslog_read_sequential(bufp: *mut u8, maxlen: usize) -> i64 {
    if maxlen == 0 || bufp.is_null() {
        return -(crate::syscall::errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(bufp as usize, maxlen) {
        return -(crate::syscall::errno::EFAULT as i64);
    }
    // Bound the transfer: maxlen is user-controlled and must never drive an
    // unbounded heap allocation (alloc failure panics the kernel). Callers
    // read in chunks, like dmesg(1).
    const SYSLOG_MAX_READ: usize = 256 * 1024;
    let maxlen = maxlen.min(SYSLOG_MAX_READ);
    // Stage into a kernel buffer: writing directly to user memory while
    // holding the printk spinlock (irqs off) would deadlock/panic on an
    // unmapped user page.
    let mut kbuf = alloc::vec![0u8; maxlen];

    let (produced, next_read_seq) = {
        let mut rb = RING_BUFFER.lock_irqsave();
        let read_seq = rb.read_seq;
        let next_seq = rb.next_seq;

        if read_seq >= next_seq {
            (0usize, read_seq)
        } else {
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

                kbuf[offset..offset + header_len].copy_from_slice(&header_buf[..header_len]);
                offset += header_len;
                kbuf[offset..offset + text_bytes.len()].copy_from_slice(text_bytes);
                offset += text_bytes.len();
                if !trailing_nl {
                    kbuf[offset] = b'\n';
                    offset += 1;
                }
            }

            (offset, next_seq)
        }
    };

    if produced > 0 {
        let uncopied = unsafe {
            crate::arch::uaccess::copy_to_user(bufp, kbuf.as_ptr(), produced)
        };
        if uncopied > 0 {
            return -(crate::syscall::errno::EFAULT as i64);
        }
        // Advance read cursor only after a successful copy
        RING_BUFFER.lock_irqsave().read_seq = next_read_seq;
    }

    produced as i64
}

/// Read all records from the ring buffer, oldest first.
fn syslog_read_all(bufp: *mut u8, maxlen: usize, clear: bool) -> i64 {
    if maxlen == 0 || bufp.is_null() {
        return -(crate::syscall::errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(bufp as usize, maxlen) {
        return -(crate::syscall::errno::EFAULT as i64);
    }
    // Bound the transfer and stage into a kernel buffer: writing directly to
    // user memory while holding the printk spinlock (irqs off) would
    // deadlock/panic on an unmapped user page.
    const SYSLOG_MAX_READ: usize = 256 * 1024;
    let maxlen = maxlen.min(SYSLOG_MAX_READ);
    let mut kbuf = alloc::vec![0u8; maxlen];

    let produced = {
        let mut rb = RING_BUFFER.lock_irqsave();
        let next_seq = rb.next_seq;

        if next_seq == 0 {
            0usize
        } else {
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

                kbuf[offset..offset + header_len].copy_from_slice(&header_buf[..header_len]);
                offset += header_len;
                kbuf[offset..offset + text_bytes.len()].copy_from_slice(text_bytes);
                offset += text_bytes.len();
                if !trailing_nl {
                    kbuf[offset] = b'\n';
                    offset += 1;
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

            offset
        }
    };

    if produced > 0 {
        let uncopied = unsafe {
            crate::arch::uaccess::copy_to_user(bufp, kbuf.as_ptr(), produced)
        };
        if uncopied > 0 {
            return -(crate::syscall::errno::EFAULT as i64);
        }
    }

    produced as i64
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

/// Newest `n` records, oldest-first, syslog-style one line each.
///
/// dfx=schb wedge census (sc-series quiet-quiet hunt): quietcon diverts
/// the userspace /dev/console stream into this ring, so the tail around
/// a soft-wedge is the userspace last-known-good boundary. Records are
/// COPIED out under the ring lock (bounded hold) and formatted after it
/// drops — formatting ~hundreds of records under irqsave would block
/// every other CPU's printk for the duration.
pub fn ring_tail(n: usize) -> alloc::string::String {
    struct TailRec {
        level: u8,
        timestamp: u64,
        pid: u32,
        cpu_id: u16,
        text: [u8; RECORD_TEXT_SIZE],
        text_len: u16,
    }
    let mut out = alloc::string::String::new();
    let mut recs: alloc::vec::Vec<TailRec> = alloc::vec::Vec::new();
    {
        let rb = RING_BUFFER.lock_irqsave();
        if rb.next_seq == 0 {
            return out;
        }
        let oldest_kept = rb.next_seq.saturating_sub(RING_BUFFER_CAPACITY as u64);
        let start = rb.next_seq.saturating_sub(n as u64).max(oldest_kept);
        let mut seq = start;
        while seq < rb.next_seq {
            let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
            let record = &rb.records[idx];
            if record.text_len != 0 && record.seq == seq {
                let mut text = [0u8; RECORD_TEXT_SIZE];
                text[..record.text_len as usize]
                    .copy_from_slice(&record.text[..record.text_len as usize]);
                recs.push(TailRec {
                    level: record.level,
                    timestamp: record.timestamp,
                    pid: record.pid,
                    cpu_id: record.cpu_id,
                    text,
                    text_len: record.text_len,
                });
            }
            seq += 1;
        }
    }
    let mut header_buf = [0u8; SYSLOG_HEADER_LEN];
    for r in &recs {
        let hlen = format_syslog_header(
            &mut header_buf,
            r.level,
            r.timestamp,
            r.pid,
            r.cpu_id,
        );
        out.push_str(unsafe { core::str::from_utf8_unchecked(&header_buf[..hlen]) });
        // Lossy-UTF8 substitute that stays in core: kernel and console
        // records are overwhelmingly valid UTF-8; anything else prints
        // with invalid bytes masked rather than dropped.
        let raw = &r.text[..r.text_len as usize];
        match core::str::from_utf8(raw) {
            Ok(s) => out.push_str(s),
            Err(_) => {
                for &b in raw {
                    out.push(if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    });
                }
            }
        }
        if out.as_bytes().last() != Some(&b'\n') {
            out.push('\n');
        }
    }
    out
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

/// Readers blocked in /dev/kmsg read waiting for the next record.
/// Woken from write_to_ring_buffer after each record lands (printk
/// context — IRQs may be off; WaitQueueHead takes irqsave locks).
static KMSG_READ_WQ: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

/// perf-audit counters for the hilogd.rd_kmsg spin hunt (pa3: 92.6K
/// read()/s sustained — which arm of the read does that?): records
/// returned, EAGAINs, blocking sleeps, and the open flags of the first
/// reader (O_NONBLOCK visible in the bits). Printed from syscost dumps.
pub static KMSG_N_RECORD: AtomicUsize = AtomicUsize::new(0);
pub static KMSG_N_EAGAIN: AtomicUsize = AtomicUsize::new(0);
pub static KMSG_N_SLEEP: AtomicUsize = AtomicUsize::new(0);
pub static KMSG_FIRST_FLAGS: AtomicUsize = AtomicUsize::new(0);

/// Read handler for /dev/kmsg.
///
/// Each read() returns one record in /dev/kmsg format:
/// `priority,sequence,timestamp_us,-;text\n`
///
/// Uses the file position as the sequence number to track which record to read next.
///
/// Linux semantics for the no-records case: a BLOCKING read sleeps until
/// the next record arrives; O_NONBLOCK returns EAGAIN. The old plain
/// `return 0` (EOF) made every kmsg consumer spin: hilogd's kmsg parser
/// thread (`hilogd_kmsg`) read-EOF-looped at 100% CPU on one of two
/// vCPUs for the whole OH boot.
fn kmsg_file_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    let nonblock = file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0;
    KMSG_FIRST_FLAGS
        .compare_exchange(0, file.flags().bits() as usize, Ordering::Relaxed, Ordering::Relaxed)
        .ok();
    loop {
        {
            let mut rb = RING_BUFFER.lock_irqsave();
            let pos = file.get_pos();
            let next_seq = rb.next_seq;

            if pos >= next_seq {
                drop(rb);
                if nonblock {
                    KMSG_N_EAGAIN.fetch_add(1, Ordering::Relaxed);
                    return -(crate::syscall::errno::EAGAIN) as isize;
                }
                // Block until a record lands. prepare→re-check→schedule
                // (lost-wake discipline, same as signalfd_read).
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return -(crate::syscall::errno::EAGAIN) as isize,
                };
                KMSG_READ_WQ.prepare_to_wait(current, false, true);
                {
                    let rb2 = RING_BUFFER.lock_irqsave();
                    let ready = file.get_pos() < rb2.next_seq;
                    drop(rb2);
                    if ready {
                        KMSG_READ_WQ.finish_wait(current);
                        // SAFETY: current is the running task; undo a
                        // racing wake enqueue (NEW-C2 discipline).
                        unsafe { crate::sched::dequeue_task(&*current); }
                        continue;
                    }
                }
                KMSG_N_SLEEP.fetch_add(1, Ordering::Relaxed);
                crate::arch::cpu::restore_irq(true);
                crate::sched::schedule();
                KMSG_READ_WQ.finish_wait(current);
                continue;
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
                drop(rb);
                continue;
            }

            // Format: "level,seq,timestamp_us,-;text\n"
            let timestamp_us = record.timestamp / 10; // cycles to microseconds (10MHz clock)

            // Format into a local buffer
            let mut line_buf = [0u8; 300];
            let mut lpos = 0;

            // Write "level,seq,timestamp_us,-;"
            let header = alloc::format!("{},{},{},-;", record.level, record.seq, timestamp_us);
            let header_bytes = header.as_bytes();
            let header_len = header_bytes.len().min(line_buf.len());
            line_buf[..header_len].copy_from_slice(&header_bytes[..header_len]);
            lpos = header_len;

            // Write message text
            let text_bytes = &record.text[..record.text_len as usize];
            let text_len = text_bytes.len().min(line_buf.len() - lpos);
            line_buf[lpos..lpos + text_len].copy_from_slice(&text_bytes[..text_len]);
            lpos += text_len;

            // Ensure newline
            if lpos > 0 && line_buf[lpos - 1] != b'\n' {
                if lpos < line_buf.len() {
                    line_buf[lpos] = b'\n';
                    lpos += 1;
                }
            }

            let data = &line_buf[..lpos];
            let copy_len = data.len().min(buf.len());
            if copy_len == 0 {
                drop(rb);
                return 0;
            }
            buf[..copy_len].copy_from_slice(&data[..copy_len]);

            // Advance position to next record
            file.set_pos(seq + 1);
            KMSG_N_RECORD.fetch_add(1, Ordering::Relaxed);

            // Drop lock before returning
            drop(rb);

            return copy_len as isize;
        }
    }
}

/// Write handler for /dev/kmsg — userspace passthrough (OH's init and
/// services log boot markers this way). The line lands in the printk
/// ring (so /dev/kmsg readers see it) unconditionally — Linux devkmsg
/// semantics: the ring always records, CONSOLE_LOGLEVEL only gates the
/// console. The old pr_info! passthrough went through printk()'s
/// pre-ring level filter and was dropped whole whenever console_loglevel
/// < 6 (pa9: OH's boot markers never reached any hilog consumer).
/// An optional `<N>` level prefix (N = 0..7) is honored like Linux;
/// without one the record logs at KERN_INFO.
fn kmsg_file_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let text = core::str::from_utf8(buf).unwrap_or("<binary>");
    let (level, text) = match text.as_bytes() {
        [b'<', d, b'>', rest @ ..] if (b'0'..=b'7').contains(d) => {
            (d - b'0', core::str::from_utf8(rest).unwrap_or(text))
        }
        _ => (loglevel::KERN_INFO, text),
    };
    printk_ring_always(level, format_args!("[kmsg] {}", text));
    buf.len() as isize
}

/// Poll handler for /dev/kmsg: POLLIN iff a record newer than the fd's
/// position exists; always writable. The old stub returned POLLERR —
/// epoll treats that as permanently ready, so every kmsg waiter
/// woke, read EOF, and re-armed in a tight loop.
fn kmsg_file_poll(file: &crate::fs::file::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;
    if events & POLLIN != 0 {
        let rb = RING_BUFFER.lock_irqsave();
        if file.get_pos() < rb.next_seq {
            ready |= POLLIN | POLLRDNORM;
        }
    }
    if events & POLLOUT != 0 {
        ready |= POLLOUT | POLLWRNORM;
    }
    ready
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
pub static KMSG_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(kmsg_file_read),
    write: Some(kmsg_file_write),
    lseek: Some(kmsg_file_lseek),
    close: None,
    poll: Some(kmsg_file_poll),
    read_at: None,
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

// ==================== /proc/kmsg Blocking Stream ====================
//
// perf-audit (pa9/pa10): hilogd.rd_kmsg reads /proc/kmsg, not /dev/kmsg.
// The old procfs dynamic-file implementation SNAPSHOTS the whole ring at
// open and serves the copy from a heap Box; after the snapshot drains,
// every read returns 0 (EOF). Linux /proc/kmsg is a BLOCKING,
// record-at-a-time stream (syslog(2) SYSLOG_ACTION_READ semantics:
// read() blocks until the next record, returns exactly one record,
// consumes it). OH's kmsg reader relies on the blocking semantics for
// pacing — against the snapshot it EOF-looped at 60-92K read()/s for
// the whole boot (~0.5 vCPU, pa1/pa3/pa9/pa10 flood counters).

/// Single-reader gate. Linux fs/proc/kmsg.c: `xchg(&kmsg_open, 1)` — a
/// second concurrent open gets -EBUSY. Released from the ops close.
static PROC_KMSG_OPEN: AtomicBool = AtomicBool::new(false);

/// /proc/kmsg open hook (called from procfs_open instead of snapshotting).
pub fn proc_kmsg_open() -> i32 {
    if PROC_KMSG_OPEN.swap(true, Ordering::AcqRel) {
        return -crate::syscall::errno::EBUSY;
    }
    0
}

/// /proc/kmsg close hook: release the single-reader gate.
fn proc_kmsg_close(_file: &crate::fs::file::File) -> i32 {
    PROC_KMSG_OPEN.store(false, Ordering::Release);
    0
}

/// /proc/kmsg read: one record per call, syslog human format
/// `<level>[SSSSSS.MMMMMM] pid(N) cpu(M): text\n`, destructive via the
/// SHARED `read_seq` cursor (the same cursor syslog(2) READ uses —
/// Linux shares the syslog buffer consumption between klogctl(2) and
/// /proc/kmsg). Blocks while caught up (no O_NONBLOCK mode exists for
/// /proc/kmsg on Linux); a too-small buffer truncates but still
/// consumes the record.
///
/// The "block" is a ~10 ms timed poll (kmsg_poll_sleep), NOT a
/// KMSG_READ_WQ wait. pa11: parking the reader on KMSG_READ_WQ made
/// wake_up_all hold the queue lock across wake_up_process (WQ->GRQ,
/// the R12-1 deferred-wake discipline) while scheduler-internal
/// diagnostics printk under a GRQ lock (W3-RUNWAKE-REFUSED et al.) —
/// a textbook ABBA that wedged both CPUs. The wake-side invariant
/// ("no GRQ-held path takes a waitqueue lock") is violated by
/// scheduler prints that predate this reader and is not enforceable
/// from the printk lane; a timer-woken poll needs no queue entry, so
/// the printk-side wake_up_all never holds the WQ lock against a
/// waiter. Cost: up to ~1 jiffy of record-delivery latency, vs the
/// 60-92K EOF reads/s the snapshot produced.
fn proc_kmsg_read(_file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    loop {
        // Produce one record if the cursor has not caught up.
        let produced = {
            let mut rb = RING_BUFFER.lock_irqsave();
            let next_seq = rb.next_seq;
            if rb.read_seq < next_seq {
                // Skip records the ring has already overwritten.
                let oldest = next_seq.saturating_sub(RING_BUFFER_CAPACITY as u64);
                let seq = rb.read_seq.max(oldest);
                let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
                let record = &rb.records[idx];
                if record.text_len == 0 || record.seq != seq {
                    // Overwritten: jump the cursor to live records.
                    rb.read_seq = next_seq;
                    drop(rb);
                    continue;
                }
                let mut header_buf = [0u8; SYSLOG_HEADER_LEN];
                let header_len = format_syslog_header(
                    &mut header_buf,
                    record.level,
                    record.timestamp,
                    record.pid,
                    record.cpu_id,
                );
                let text_bytes = &record.text[..record.text_len as usize];

                // Stage header+text in one line buffer, then truncate to
                // the caller's request (record consumed regardless).
                let mut line = [0u8; SYSLOG_HEADER_LEN + RECORD_TEXT_SIZE + 1];
                let mut len = 0usize;
                let hcopy = header_len.min(line.len());
                line[..hcopy].copy_from_slice(&header_buf[..hcopy]);
                len += hcopy;
                let tcopy = text_bytes.len().min(line.len() - len);
                line[len..len + tcopy].copy_from_slice(&text_bytes[..tcopy]);
                len += tcopy;
                if len > 0 && len < line.len() && line[len - 1] != b'\n' {
                    line[len] = b'\n';
                    len += 1;
                }

                let copy = len.min(buf.len());
                buf[..copy].copy_from_slice(&line[..copy]);
                rb.read_seq = seq + 1;
                copy as isize
            } else {
                -1 // caught up: go sleep one poll interval
            }
        };
        if produced >= 0 {
            return produced;
        }

        if !kmsg_poll_sleep(10) {
            // No timer available (pool exhausted): fail soft rather
            // than hot-spin — the caller retries like EAGAIN.
            return -(crate::syscall::errno::EAGAIN as isize);
        }
    }
}

/// Interruptible ~ms timed sleep for the /proc/kmsg poll loop (the
/// evdev sleep_ms_interruptible discipline: one-shot jiffy timer as
/// the only waker, state-first re-check, NEW-C2 self-dequeue on a
/// racing wake). Returns false when no task/timer was available.
fn kmsg_poll_sleep(ms: u64) -> bool {
    use crate::process::task::{Task, TaskState};

    let target =
        crate::drivers::timer::get_jiffies() + crate::drivers::timer::msecs_to_jiffies(ms);

    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return false,
    };
    // SAFETY: current is the running task (we are it); pid() is a const read.
    let pid = unsafe { (*current).pid() };

    let timer_id = crate::timer::add_timer_wakeup(target, pid);
    if timer_id == 0 {
        return false;
    }

    loop {
        if crate::drivers::timer::get_jiffies() >= target || crate::signal::signal_pending() {
            crate::timer::del_timer(timer_id);
            return true;
        }
        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first
        // lost-wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        if crate::drivers::timer::get_jiffies() >= target || crate::signal::signal_pending() {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            }
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            unsafe {
                crate::sched::dequeue_task(&*current);
            }
            crate::timer::del_timer(timer_id);
            return true;
        }
        // Syscall context runs with SIE=0 — re-arm so the timer tick can
        // reach this CPU and wake us.
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        // Woke up (timer expiry or signal): loop tail re-checks and exits.
    }
}

/// /proc/kmsg poll: POLLIN iff unconsumed records exist; always writable
/// (matches kmsg_file_poll's shape; epoll waiters need the edge).
fn proc_kmsg_poll(_file: &crate::fs::file::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let rb = RING_BUFFER.lock_irqsave();
    let readable = rb.read_seq < rb.next_seq;
    drop(rb);
    let mut ready = 0u16;
    if readable && events & POLLIN != 0 {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 {
        ready |= POLLOUT | POLLWRNORM;
    }
    ready
}

/// /proc/kmsg lseek: the stream is unseekable (Linux no_llseek → ESPIPE).
fn proc_kmsg_lseek(_file: &crate::fs::file::File, _offset: isize, _whence: i32) -> isize {
    -(crate::syscall::errno::ESPIPE as isize)
}

/// FileOps for the /proc/kmsg stream (attached by procfs_get_file_ops
/// for the node whose content_generator is generate_kmsg).
pub static PROC_KMSG_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(proc_kmsg_read),
    write: None,
    lseek: Some(proc_kmsg_lseek),
    close: Some(proc_kmsg_close),
    poll: Some(proc_kmsg_poll),
    read_at: None,
};

// ==================== Convenience Macros ====================

/// printk with KERN_EMERG level (0) - system is unusable
#[macro_export]
macro_rules! pr_emerg {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_EMERG, format_args!($($arg)*))
    });
}

/// printk with KERN_ALERT level (1) - action must be taken immediately
#[macro_export]
macro_rules! pr_alert {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_ALERT, format_args!($($arg)*))
    });
}

/// printk with KERN_CRIT level (2) - critical conditions
#[macro_export]
macro_rules! pr_crit {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_CRIT, format_args!($($arg)*))
    });
}

/// printk with KERN_ERR level (3) - error conditions
#[macro_export]
macro_rules! pr_err {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_ERR, format_args!($($arg)*))
    });
}

/// printk with KERN_WARNING level (4) - warning conditions
#[macro_export]
macro_rules! pr_warn {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_WARNING, format_args!($($arg)*))
    });
}

/// printk with KERN_NOTICE level (5) - normal but significant
#[macro_export]
macro_rules! pr_notice {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_NOTICE, format_args!($($arg)*))
    });
}

/// printk with KERN_INFO level (6) - informational
#[macro_export]
macro_rules! pr_info {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_INFO, format_args!($($arg)*))
    });
}

/// printk with KERN_DEBUG level (7) - debug-level messages
/// Only compiled in with debug_assertions.
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! pr_debug {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln($crate::printk::loglevel::KERN_DEBUG, format_args!($($arg)*))
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
        let timestamp_us = timestamp / 10; // cycles to microseconds (10MHz clock)

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
