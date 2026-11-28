//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Printk with log levels and ring buffer
//!
//! Provides leveled kernel logging (pr_emerg through pr_debug),
//! a ring buffer for storing all messages, and a syslog(2) syscall
//! for userspace `dmesg` to read/manage kernel logs.
//!
//! Zero-unsafe (mainline minimal-first collection, 2026-11-27): the
//! ring's raw slot edges, the Sync declaration of its UnsafeCell
//! holder and the global-logger install live in the root audited
//! file `crate::boot_edges` (INV-PRINTK-RING-1 / INV-PRINTK-LOGGER-1).

#![forbid(unsafe_code)]

use core::cell::UnsafeCell;
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
pub(crate) struct LogRecord {
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
    /// Monotonically increasing sequence number (plain data — only
    /// trustworthy after `pub_seq` validated the record).
    seq: u64,
    /// Timestamp: raw cycles from clint::read_time().
    timestamp: u64,
    /// The message text (not NUL-terminated; use text_len).
    text: [u8; RECORD_TEXT_SIZE],
    /// Lock-free publication marker (Rule 2, grq-lock-1st): 0 = free or
    /// being (re)written; `seq + 1` = the record with sequence `seq` is
    /// fully published. The writer stores 0, then fills, then stores
    /// `seq + 1` with Release; readers Acquire-load this FIRST and touch
    /// no other field unless it matches the sequence they expect. This
    /// is the reference prb descriptor-state discipline collapsed to one
    /// word, and it is what makes the writer lock-free — printk can no
    /// longer spin on a ring lock while holding an arbitrary caller lock
    /// (the KVM deschedule lesson: an IRQs-off spin can stretch to
    /// milliseconds while the box burns).
    pub_seq: AtomicU64,
}

impl LogRecord {
    const fn empty() -> Self {
        Self {
            level: 0,
            cpu_id: 0,
            _pad: 0,
            pid: 0,
            text_len: 0,
            seq: 0,
            timestamp: 0,
            text: [0u8; RECORD_TEXT_SIZE],
            pub_seq: AtomicU64::new(0),
        }
    }
}

/// The lock-free record slots (Rule 2). Writers reserve a sequence with
/// one `fetch_add` and publish with the per-record `pub_seq`; readers
/// snapshot under the publication protocol. Distinct sequences map to
/// distinct slots, so at most one writer fills a slot at a time — the
/// one exception is a writer stalling (preempted mid-fill) for a full
/// ring's worth of records, at which point the next writer of that slot
/// interleaves with it: the tear window is documented, bounded by
/// `RING_BUFFER_CAPACITY` prints inside ONE preemption window, and
/// memory-safe by construction (every reader clamps `text_len`).
pub(crate) struct RecordsArray(pub(crate) UnsafeCell<[LogRecord; RING_BUFFER_CAPACITY]>);
// Sync is declared in the root audited file (crate::boot_edges,
// INV-PRINTK-RING-1 Sync arm — every access goes through the audited
// slot edges).

static RING_RECORDS: RecordsArray = RecordsArray(UnsafeCell::new([const {
    LogRecord::empty()
}; RING_BUFFER_CAPACITY]));

/// Lock-free slot accessor (Rule 2 writer/reader entry point).
///
/// SAFETY contract (callers): the slot protocol above — a reader may
/// only trust the record's fields between an Acquire load of `pub_seq`
/// that matched its expected sequence and the reads it performs after
/// that load; a writer owns the slot between the 0-store and the
/// publish-store of its reserved sequence.
#[inline]
fn ring_rec(seq: u64) -> &'static LogRecord {
    let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
    // INV-PRINTK-RING-1: bounds-reduced idx, slot-protocol discipline —
    // the raw edge lives in the root audited file.
    crate::boot_edges::ring_slot_get(&RING_RECORDS, idx)
}

/// Writable slot view for the lock-free writer (Rule 2). Same slot
/// protocol as `ring_rec`; the writer owns the slot between the
/// invalidation store and the publication store of its reserved
/// sequence.
#[inline]
fn ring_rec_mut(seq: u64) -> &'static mut LogRecord {
    let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
    // INV-PRINTK-RING-1 (writer arm): the writer owns this slot between
    // the invalidate and publish stores — the raw edge lives in the root
    // audited file.
    crate::boot_edges::ring_slot_get_mut(&RING_RECORDS, idx)
}

/// Clamped text length for a record: never trust the raw u16 (a torn
/// record in the documented storm window must garble, never panic).
#[inline]
fn rec_text_len(rec: &LogRecord) -> usize {
    (rec.text_len as usize).min(RECORD_TEXT_SIZE)
}

/// The ring's write sequence counter — sequences are reserved LOCK-FREE
/// (Rule 2). Sequence `s` lives at slot `s % CAP`; `RING_NEXT_SEQ` is
/// the NEXT sequence to assign. Reset only by syslog clear (racing one
/// in-flight record is accepted, same as the reference's clear).
static RING_NEXT_SEQ: AtomicU64 = AtomicU64::new(0);

/// The printk ring's READER cursor state (the write side is lock-free).
///
/// Only the sequential-read cursor lives here; the lock serializes
/// READERS against readers (syslog(2), /proc/kmsg). It no longer
/// excludes writers — writers never take it (see `RING_RECORDS`).
struct RingBuffer {
    /// Sequence number that the next sequential syslog READ will start from.
    read_seq: u64,
}

/// Global reader-cursor instance.
static RING_BUFFER: Spinlock<RingBuffer> = Spinlock::new(RingBuffer { read_seq: 0 });

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
        crate::debug::syscost::note_printk_filtered(); // syscost probe
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
    printk_ring_always_ex(level, args, true);
}

/// printk variant with an explicit console-emission policy.
///
/// `echo_console == false` records the message in the printk ring (so every
/// hilog consumer reading /dev/kmsg sees it) but does NOT echo it to the
/// console. That split is what the reference's `printk_devkmsg` policy
/// knob exists for: the ring is the log, the console is a view of it.
///
/// With `echo_console == true` the historical behavior applies — the record
/// enters the ring unconditionally and the console copy is gated by
/// CONSOLE_LOGLEVEL.
fn printk_ring_always_ex(level: u8, args: fmt::Arguments, echo_console: bool) {
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

    // The printk:console tracepoint — the reference fires it in
    // printk_sprint, right after the text is formatted into the record
    // (KERN prefix stripped by the formatter; the trailing newline cut
    // per the fast_assign). Marshals only behind armed().
    if crate::trace::EVENT_CONSOLE.armed() {
        let mut tlen = text_len;
        if tlen > 0 && buf[tlen - 1] == b'\n' {
            tlen -= 1;
        }
        crate::trace::trace_console(&buf[..tlen]);
    }

    // Write to ring buffer (if initialized)
    if PRINTK_INITIALIZED.load(Ordering::Relaxed) {
        let timestamp = crate::arch::cpu::read_time();
        write_to_ring_buffer(level, &buf[..text_len], timestamp);
    }

    // Console emit (level gated here: printk() pre-filtered already,
    // printk_ring_always callers pass user-chosen levels).
    if echo_console && level <= CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
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
    // Leg-54 item C: one batched TX pass per record (CR expansion into a
    // stack buffer, then Uart::puts_batch — one THRE poll + one string-IO
    // per 16 bytes instead of two exits per byte).
    let mut buf = [0u8; RECORD_TEXT_SIZE * 2];
    let mut n = 0usize;
    for &b in text {
        if b == b'\n' {
            if n < buf.len() {
                buf[n] = b'\r';
                n += 1;
            }
        }
        if n < buf.len() {
            buf[n] = b;
            n += 1;
        }
    }
    let uart = crate::console::Uart::new();
    uart.puts_batch(&buf[..n]);
    let emitted = n; // syscost probe
    crate::debug::syscost::note_printk_con(emitted); // syscost probe
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

/// Bounded acquire of the console emit lock with a caller-chosen budget
/// in milliseconds (obs-hygiene line-atomicity round).
///
/// `try_console_emit_lock`'s 100µs interrupt-context budget is tuned for
/// crash contexts where the holder may be wedged. The diagnostic
/// line-atomic writers (the taskdump family) run on hunt boots where the
/// console is FLOODED — the OH kmsg flood holds this lock 0.4-17ms per
/// record — so a 100µs acquire fails on nearly every line and the dump
/// bytes splice into flood records again. This variant spends an
/// explicit caller budget (the dump already costs far more than that in
/// UART time per line) so a staged line and a printk record can never
/// interleave. Same fallback contract: false on timeout, the caller
/// writes locklessly.
pub fn try_console_emit_lock_ms(budget_ms: u64) -> bool {
    let budget = crate::config::TIMER_CLOCK_FREQ_HZ.saturating_mul(budget_ms) / 1000;
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

    // Console emit (level filter already passed above).
    //
    // DEFERRED IN UNSAFE CONTEXTS (Rule 2, grq-lock-1st): the serial
    // emit pays real baud time (a full line can be tens of ms of THRE
    // polling) and that cost must never run under the CALLER's locks or
    // with interrupts off. When the caller is preemptible task context
    // (preempt_count == 0, IRQs on — holding no spinlock) the emit is
    // inline, exactly as before; every atomic/locked/IRQs-off caller
    // only marks the console pending flag and returns — the record is
    // already safe in the ring, and the tick/idle drain emits it from a
    // context that holds nothing. Early boot (before PRINTK_INITIALIZED)
    // stays inline: there is no tick to drain yet and the box is
    // single-CPU.
    // DEFERRED EMISSION DISABLED (oh-dbg lane, 2026-11-27): the deferral
    // below broke the OpenHarmony boot storm. Bisect over
    // 017bf556b..f1900270e convicted this deferral (first-bad
    // 6d6ae3f2e); neutralizing ONLY this arm on the first-bad tree
    // healed the boot (daemon-exit churn in the first 30 s: 69-120 -> 1;
    // SAFWK "init finished" lines return at 2.5-4.5 s). Two structural
    // costs, both load-bearing only under the OH storm:
    //   1. The drain ceiling. Console lines from atomic contexts then
    //      drain at 1 line/tick (+4/idle pass) ~100 lines/s, vs the
    //      callers' own inline pace; the emit-lock skips + TX-stall
    //      drops shed the rest. The OH boot stalls at the SAFWK SA-init
    //      chain (no service completes OnStart; daemons exit 0 and
    //      churn; bootanimation never spawns; screen stays black).
    //   2. An IF=0 UART print on the tick path. service_deferred_kmsg_
    //      wake() drains from scheduler_tick inside the timer IRQ, so a
    //      pending line pays its bounded THRE wait at IF=0 every tick —
    //      the "no new IRQ-OFF print classes" gate class, invisible on
    //      quiet gates (pending stays clear) and hot under load.
    // The rest of Rule 2 stays: the ring is still lock-free, the kmsg
    // reader wake is still always-deferred (KMSG_WAKE_PENDING serviced
    // by the tick — no print-path scheduler re-entry), and the drain
    // machinery below remains for its cov-dump caller and any future
    // rate-honest redesign; with the setter gone the drains no-op.
    // printk_should_defer_output() (the Rule-2 switch, formerly
    // preempt_count!=0 || IRQs-off || grq_held_here) is deleted with
    // the arm; re-land it only behind a drain that keeps up with the
    // storm and drains off the tick's IF=0 window.
    emit_to_console(text);
}

/// A console line is waiting to be emitted by the drain (set by printk
/// callers in unsafe contexts; cleared when the drain catches up).
static CONSOLE_EMIT_PENDING: AtomicBool = AtomicBool::new(false);

/// Drain cursor: the next sequence whose console line has NOT been
/// emitted yet. Lagging the ring is fine (the drain skips overrun);
/// a syslog clear resets the ring and this cursor clamps down to it.
static CONSOLE_DRAIN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Read side of `CONSOLE_EMIT_PENDING` for the idle-loop drain gate
/// (one Acquire load; the idle loop skips the drain call when nothing
/// is pending).
#[inline]
pub fn console_emit_pending_flag() -> bool {
    CONSOLE_EMIT_PENDING.load(Ordering::Acquire)
}

/// Emit at most `budget` pending console lines, oldest first.
///
/// Called from contexts that hold NO scheduler lock and NO caller lock:
/// the tick (`service_deferred_kmsg_wake`) and the idle loop's poll
/// window. `emit_to_console` is bounded-CAS on the emit lock and the
/// UART writer is lock-free MMIO, so a bounded budget per call keeps
/// the hardirq extension small (1 line/tick ≈ the serial line's own
/// physical rate at 115200 baud).
///
/// Returns the number of lines emitted (diagnostics).
pub fn console_drain_pending(budget: usize) -> usize {
    // A coverage dump owns the console stream right now: emitting here
    // would interleave into the hex. Lines stay in the ring.
    #[cfg(feature = "cov-dump")]
    if crate::cov::console_dump_active() {
        return 0;
    }
    let mut emitted = 0usize;
    while emitted < budget {
        let next = RING_NEXT_SEQ.load(Ordering::Relaxed);
        let mut d = CONSOLE_DRAIN_SEQ.load(Ordering::Relaxed);
        // Clamp both ways: ring overran us (skip the overwritten tail)
        // or a syslog clear reset the sequence space.
        let oldest = next.saturating_sub(RING_BUFFER_CAPACITY as u64);
        if d < oldest {
            d = oldest;
        }
        if d > next {
            d = next;
        }
        if d >= next {
            CONSOLE_EMIT_PENDING.store(false, Ordering::Release);
            break;
        }
        let rec = ring_rec(d);
        if rec.pub_seq.load(Ordering::Acquire) != d + 1 {
            // A writer is mid-fill at this slot (or the documented storm
            // tear) — stop here, keep order, retry next service.
            break;
        }
        if rec.level <= CONSOLE_LOGLEVEL.load(Ordering::Relaxed) {
            let tl = rec_text_len(rec);
            emit_to_console(&rec.text[..tl]);
        }
        CONSOLE_DRAIN_SEQ.store(d + 1, Ordering::Release);
        emitted += 1;
    }
    emitted
}

// ==================== Ring Buffer Write ====================

fn write_to_ring_buffer(level: u8, text: &[u8], timestamp: u64) {
    crate::debug::syscost::note_printk_ring(text.len()); // syscost probe
    let pid = crate::process::current_pid() as u32;
    // Real CPU id (review批次8: hard-coded 0 attributed every message to
    // CPU 0, hiding cross-CPU issues in the logs).
    let cpu_id: u16 = (crate::arch::cpu_id() as u16).min(u16::MAX);

    // LOCK-FREE writer (Rule 2, grq-lock-1st): one fetch_add reserves a
    // sequence (and with it a slot — distinct sequences map to distinct
    // slots), the fill is plain stores, and the per-record pub_seq
    // publishes with Release. printk can thus be called while holding
    // ANY lock, from any interrupt context, without ever spinning on a
    // ring lock (the old spinlock made every caller pay an IRQs-off
    // spin whenever the holder was KVM-descheduled — the same stretched
    // window that convicted the GRQ re-acquire).
    let seq = RING_NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
    let rec = ring_rec_mut(seq);
    // Invalidate the previous publication of this slot BEFORE filling:
    // readers of the OLD sequence must not consume half-new data. The
    // Release fence orders the invalidation before the fills on weak
    // archs (riscv64); the publish store below orders the fills before
    // the publication.
    rec.pub_seq.store(0, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::Release);
    rec.level = level;
    rec.pid = pid;
    rec.cpu_id = cpu_id;
    rec.timestamp = timestamp;
    rec.seq = seq;
    let copy_len = text.len().min(RECORD_TEXT_SIZE);
    rec.text[..copy_len].copy_from_slice(&text[..copy_len]);
    rec.text_len = copy_len as u16;
    // Publish (Release pairs with the readers' Acquire of pub_seq).
    rec.pub_seq.store(seq + 1, Ordering::Release);

    // The sequential reader's cursor self-heals on overwrite (it detects
    // `pub_seq` divergence and jumps to the oldest kept sequence), so the
    // writer no longer repairs read_seq under the lock.

    // Wake /dev/kmsg readers (Linux semantics: a blocking kmsg read
    // sleeps until the next record lands).
    //
    // DEFERRED ONLY UNDER THE GRQ (back to the pre-Rule-2 shape, oh-dbg
    // lane 2026-11-27): `WaitQueueHead::wake_up` holds the queue's list
    // lock ACROSS `wake_up_process` -> `wake_up_enqueue_opt` ->
    // `grq().lock_irqsave()` — a wake from the print path ENTERS THE
    // SCHEDULER, and from a GRQ holder that is the convicted same-cpu
    // re-acquire wedge. The unconditional Rule-2 deferral (tick
    // services the pending flag) widened the reader lost-wake window to
    // a full tick and moved every reader wake into the tick's hardirq.
    // A/B on the OH boot: this unconditional form dies at the hdcd
    // null-jump kernel panic on every boot (63.5-167 s across 8 boots,
    // rip=0x28 kernel-mode jump); with the wake inline-when-GRQ-free
    // the same rig ran 180 s with zero panics (fixC1) — the bisect +
    // A/B convict this arm as the panic face. The GRQ stays protected
    // (defer when held — the pre-Rule-2 pa11 discipline); every other
    // caller wakes inline at syscall context.
    if crate::sched::sched::grq_held_here() {
        KMSG_WAKE_PENDING.store(true, Ordering::Release);
    } else {
        KMSG_READ_WQ.wake_up_all();
    }

    // NOTE (klogd redesign): the persistent /var/log/kmsg copy is NO
    // LONGER taken here. The old per-record synchronous ext4 write ran
    // on every printk — a disk write on the logging hot path (and the
    // disabled era's "ext4 write corrupts filesystem" comment). The
    // record itself already lives in this ring; the klogd kernel thread
    // (persistent_log::init) walks the ring every 30 s and appends the
    // delta to /var/log/kmsg with the size cap, from task context.
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
    // syslog(2) declares `int len`: truncate to 32 bits and sign-extend.
    // A C caller passing -1 through varargs leaves the register's upper
    // 32 bits undefined (typically zeroed), so the raw u64 reads as
    // 0x00000000FFFFFFFF — positive. Without the int-width read the
    // negative-length EINVAL below never fires and read_all happily
    // copies ring bytes past the caller's buffer (LTP r21 syslog12:
    // "negative length" returned 8836/SUCCESS, then the smashed test
    // SIGSEGVd).
    let len_signed = args[2] as u32 as i32 as i64;
    if len_signed < 0 {
        return -(crate::errno::constants::EINVAL as i64);
    }
    let len = len_signed as usize;

    // Permission check (Linux check_syslog_permissions): everything except
    // the size queries (3/10 with dmesg_restrict=0 semantics) requires
    // CAP_SYSLOG — otherwise any process could read kernel logs (kernel
    // address leaks), wipe them, or silence the console.
    if !matches!(action, 0 | 1 | 10) {
        if !crate::security::capable(crate::security::CAP_SYSLOG) {
            return -(crate::errno::constants::EPERM as i64);
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

        // Set console log level (`int level`: <0 and >8 are EINVAL)
        8 => {
            let new_level = args[2] as u32 as i32;
            if !(0..=loglevel::KERN_DEBUG as i32).contains(&new_level) {
                return -(crate::errno::constants::EINVAL as i64);
            }
            CONSOLE_LOGLEVEL.store(new_level as u8, Ordering::Relaxed);
            0
        }

        // Return unread bytes (approximation)
        9 => {
            let rb = RING_BUFFER.lock_irqsave();
            let unread = RING_NEXT_SEQ.load(Ordering::Relaxed).saturating_sub(rb.read_seq);
            // Approximate: assume each record is ~128 bytes average
            (unread * 128) as i64
        }

        // Return total buffer size
        10 => (RING_BUFFER_CAPACITY * RECORD_TEXT_SIZE) as i64,

        _ => -(crate::errno::constants::EINVAL as i64),
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
    // Linux do_syslog: NULL buffer is EINVAL, but a zero length is a
    // successful empty read (LTP syslog11 calls read/read_all with
    // len=0 and expects >= 0).
    if bufp.is_null() {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if maxlen == 0 {
        return 0;
    }
    if !crate::arch::uaccess::access_ok(bufp as usize, maxlen) {
        return -(crate::errno::constants::EFAULT as i64);
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
        let next_seq = RING_NEXT_SEQ.load(Ordering::Relaxed);

        if read_seq >= next_seq {
            (0usize, read_seq)
        } else {
            let mut offset = 0usize;
            let mut header_buf = [0u8; SYSLOG_HEADER_LEN];

            // Iterate through all slots, find records with seq >= read_seq
            for i in 0..RING_BUFFER_CAPACITY {
                let record = ring_rec(i as u64);
                let pub_ok = record.pub_seq.load(Ordering::Acquire);
                if pub_ok == 0 || pub_ok - 1 < read_seq {
                    continue;
                }

                // Format: <level>[timestamp] pid(N) cpu(M): text\n
                let header_len = format_syslog_header(&mut header_buf, record.level, record.timestamp, record.pid, record.cpu_id);
                let text_bytes = &record.text[..rec_text_len(record)];
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
        let uncopied = crate::arch::uaccess_safe::copy_out(bufp as usize, &kbuf[..produced]);
        if uncopied > 0 {
            return -(crate::errno::constants::EFAULT as i64);
        }
        // Advance read cursor only after a successful copy
        RING_BUFFER.lock_irqsave().read_seq = next_read_seq;
    }

    produced as i64
}

/// Read all records from the ring buffer, oldest first.
fn syslog_read_all(bufp: *mut u8, maxlen: usize, clear: bool) -> i64 {
    // Same as the sequential read: NULL buffer is EINVAL, zero length
    // is a successful empty read.
    if bufp.is_null() {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if maxlen == 0 {
        return 0;
    }
    if !crate::arch::uaccess::access_ok(bufp as usize, maxlen) {
        return -(crate::errno::constants::EFAULT as i64);
    }
    // Bound the transfer and stage into a kernel buffer: writing directly to
    // user memory while holding the printk spinlock (irqs off) would
    // deadlock/panic on an unmapped user page.
    const SYSLOG_MAX_READ: usize = 256 * 1024;
    let maxlen = maxlen.min(SYSLOG_MAX_READ);
    let mut kbuf = alloc::vec![0u8; maxlen];

    let produced = {
        let mut rb = RING_BUFFER.lock_irqsave();
        let next_seq = RING_NEXT_SEQ.load(Ordering::Relaxed);

        if next_seq == 0 {
            0usize
        } else {
            let mut offset = 0usize;
            let mut header_buf = [0u8; SYSLOG_HEADER_LEN];
            let write_idx = next_seq as usize % RING_BUFFER_CAPACITY;

            // Read records from oldest to newest.
            // The oldest record is at write_idx (the slot about to be overwritten).
            for i in 0..RING_BUFFER_CAPACITY {
                let idx = (write_idx + i) % RING_BUFFER_CAPACITY;
                let record = ring_rec(idx as u64);

                // Skip empty/zeroed records
                if record.text_len == 0 {
                    continue;
                }

                // Format: <level>[timestamp] pid(N) cpu(M): text\n
                let header_len = format_syslog_header(&mut header_buf, record.level, record.timestamp, record.pid, record.cpu_id);
                let text_bytes = &record.text[..rec_text_len(record)];
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
                for s in 0..RING_BUFFER_CAPACITY as u64 {
                    ring_rec(s).pub_seq.store(0, Ordering::Release);
                }
                RING_NEXT_SEQ.store(0, Ordering::Release);
                CONSOLE_DRAIN_SEQ.store(0, Ordering::Release);
                rb.read_seq = 0;
            }

            offset
        }
    };

    if produced > 0 {
        let uncopied = crate::arch::uaccess_safe::copy_out(bufp as usize, &kbuf[..produced]);
        if uncopied > 0 {
            return -(crate::errno::constants::EFAULT as i64);
        }
    }

    produced as i64
}

/// Clear the ring buffer.
fn syslog_clear() {
    let mut rb = RING_BUFFER.lock_irqsave();
    for s in 0..RING_BUFFER_CAPACITY as u64 {
        ring_rec(s).pub_seq.store(0, Ordering::Release);
    }
    RING_NEXT_SEQ.store(0, Ordering::Release);
    CONSOLE_DRAIN_SEQ.store(0, Ordering::Release);
    rb.read_seq = 0;
}

/// Two properties `ring_tail` does not have, both for the panic context
/// the pstore capture runs in:
///
/// - **No allocation.** Headers format into a stack buffer, record text
///   copies straight into `dst` — the panic being captured may be the
///   heap's own failure, so the heap is trusted with nothing.
/// - **No waiting.** The ring lock is taken with `try_lock_irqsave`
///   only — a stopped CPU may own it after `smp_send_stop`, and the
///   panic path must still reach the `panic=` timeout. When the lock is
///   held, a marker line is written instead of the tail.
///
/// Returns the number of bytes written.
pub fn ring_tail_into(dst: &mut [u8]) -> usize {
    fn push(dst: &mut [u8], at: &mut usize, bytes: &[u8]) {
        let room = dst.len().saturating_sub(*at);
        let n = bytes.len().min(room);
        dst[*at..*at + n].copy_from_slice(&bytes[..n]);
        *at += n;
    }

    let mut at = 0usize;
    let Some(rb) = RING_BUFFER.try_lock_irqsave() else {
        push(
            dst,
            &mut at,
            b"[pstore: printk ring lock held by a stopped CPU - console tail skipped]\n",
        );
        return at;
    };
    let next_seq_probe = RING_NEXT_SEQ.load(Ordering::Relaxed);
    if next_seq_probe == 0 || dst.is_empty() {
        return at;
    }

    // Pass 1 (still under the guard — the other CPUs are stopped and
    // this hold is bounded by the copy below): walk newest-first until
    // the formatted records no longer fit, remembering the first seq
    // the tail starts from.
    let ring_next = RING_NEXT_SEQ.load(Ordering::Relaxed);
        let oldest_kept = ring_next.saturating_sub(RING_BUFFER_CAPACITY as u64);
    let mut first = ring_next;
    let mut used = 0usize;
    let mut seq = ring_next;
    while seq > oldest_kept {
        seq -= 1;
        let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
        let record = ring_rec(idx as u64);
        if record.pub_seq.load(Ordering::Acquire) != seq + 1 {
            continue;
        }
        let cost = record.text_len as usize + SYSLOG_HEADER_LEN + 1;
        if used + cost > dst.len() {
            break;
        }
        used += cost;
        first = seq;
    }

    // Pass 2: emit oldest-first.
    let mut header_buf = [0u8; SYSLOG_HEADER_LEN];
    for seq in first..ring_next {
        let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
        let record = ring_rec(idx as u64);
        if record.pub_seq.load(Ordering::Acquire) != seq + 1 {
            continue;
        }
        let hlen = format_syslog_header(
            &mut header_buf,
            record.level,
            record.timestamp,
            record.pid,
            record.cpu_id,
        );
        push(dst, &mut at, &header_buf[..hlen]);
        push(dst, &mut at, &record.text[..rec_text_len(record)]);
        // One record per line, the ring_tail convention.
        if at == 0 || dst[at - 1] != b'\n' {
            push(dst, &mut at, b"\n");
        }
        if at == dst.len() {
            break;
        }
    }
    at
}

// ==================== ProcFS /dev/kmsg Support ====================

/// Generate kmsg content for /proc/kmsg.
/// Format: "<level>text\n" per record, matching dmesg expected format.
pub fn generate_kmsg() -> alloc::vec::Vec<u8> {
    let rb = RING_BUFFER.lock_irqsave();
    let next_seq = RING_NEXT_SEQ.load(Ordering::Relaxed);

    if next_seq == 0 {
        return alloc::vec::Vec::new();
    }

    let mut result = alloc::vec::Vec::new();
    let mut header_buf = [0u8; MAX_HEADER_LEN];
    let write_idx = next_seq as usize % RING_BUFFER_CAPACITY;

    // Read records from oldest to newest
    for i in 0..RING_BUFFER_CAPACITY {
        let idx = (write_idx + i) % RING_BUFFER_CAPACITY;
        let record = ring_rec(idx as u64);

        if record.text_len == 0 {
            continue;
        }

        // Skip whitespace-only records (e.g. bare newlines from println!(""))
        let text_bytes = &record.text[..rec_text_len(record)];
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

/// Deferred /dev/kmsg reader wake (grq-lock-1st).
///
/// Set by the ring-buffer write when the PRINTING cpu holds the GRQ —
/// the wake would otherwise re-acquire the GRQ on that cpu (self-
/// deadlock) or form the GRQ->WQ / WQ->GRQ ABAA with another cpu.
/// Serviced from the tick path, which holds no scheduler lock.
static KMSG_WAKE_PENDING: AtomicBool = AtomicBool::new(false);

/// Service the printk deferrals (Rule 2 drain): the pending console
/// lines and the deferred /dev/kmsg reader wake.
///
/// Called from the tick path — a context that holds no scheduler lock
/// and no caller lock. The console drain budget is 1 line per tick
/// (the serial line's own physical rate class at 115200 baud); the
/// idle loop's poll window drains a larger batch (see
/// `console_drain_pending`). The wake swap makes exactly one caller do
/// the waitqueue walk; the wake takes the queue lock and calls
/// `wake_up_process` (GRQ) — legal here because the caller holds
/// nothing.
pub fn service_deferred_kmsg_wake() {
    if CONSOLE_EMIT_PENDING.load(Ordering::Acquire) {
        console_drain_pending(1);
    }
    if KMSG_WAKE_PENDING.swap(false, Ordering::AcqRel) {
        KMSG_READ_WQ.wake_up_all();
    }
}

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
            let next_seq = RING_NEXT_SEQ.load(Ordering::Relaxed);

            if pos >= next_seq {
                drop(rb);
                if nonblock {
                    KMSG_N_EAGAIN.fetch_add(1, Ordering::Relaxed);
                    return -(crate::errno::constants::EAGAIN) as isize;
                }
                // Block until a record lands. prepare→re-check→schedule
                // (lost-wake discipline, same as signalfd_read).
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return -(crate::errno::constants::EAGAIN) as isize,
                };
                KMSG_READ_WQ.prepare_to_wait(current, false, true);
                {
                    let rb2 = RING_BUFFER.lock_irqsave();
                    let ready = (file.get_pos() as u64) < RING_NEXT_SEQ.load(Ordering::Relaxed);
                    drop(rb2);
                    if ready {
                        KMSG_READ_WQ.finish_wait(current);
                        // Undo a racing wake enqueue (NEW-C2 discipline):
                        // current is the running task reference handed to
                        // prepare_to_wait.
                        crate::sched::dequeue_task(current);
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
            let record = ring_rec(idx as u64);

            // Check if record was overwritten
            if record.pub_seq.load(Ordering::Acquire) != seq + 1 {
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
            let text_bytes = &record.text[..rec_text_len(record)];
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

/// Console-echo policy for userspace writes to /dev/kmsg.
///
/// The reference exposes the same choice as the `printk_devkmsg` knob
/// (`on` / `ratelimit` / `off`); the modes here keep the reference's
/// meaning: the printk RING always records every line, and this setting
/// only decides how much of it the CONSOLE shows.
pub mod kmsg_console {
    /// Every write is echoed to the console (subject to CONSOLE_LOGLEVEL).
    /// Reference parity — the default.
    pub const ON: u8 = 0;
    /// The console shows a bounded trickle; the rest is recorded in the
    /// ring only. The reference's devkmsg ratelimit spirit: the log keeps
    /// everything, the console stays readable.
    pub const RATELIMIT: u8 = 1;
    /// Nothing is echoed; the ring is the only sink.
    pub const OFF: u8 = 2;

    /// Burst allowance per window in RATELIMIT mode.
    pub const BURST: u32 = 20;
    /// Window length in jiffies (5 s at the 250 Hz tick).
    pub const WINDOW_JIFFIES: u64 = 1250;
}

/// Active /dev/kmsg console-echo policy (see `kmsg_console`).
static KMSG_CONSOLE_MODE: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(kmsg_console::ON);

/// Rate-limiter state for the RATELIMIT mode: the window start (jiffies)
/// and how many lines the console has already shown inside it.
static KMSG_RL_WINDOW: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static KMSG_RL_COUNT: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Set the /dev/kmsg console-echo policy. Unknown values are rejected so a
/// typo cannot silently disable the console view.
pub fn set_kmsg_console_mode(mode: u8) -> bool {
    if mode > kmsg_console::OFF {
        return false;
    }
    KMSG_CONSOLE_MODE.store(mode, Ordering::Relaxed);
    true
}

/// Current /dev/kmsg console-echo policy.
pub fn kmsg_console_mode() -> u8 {
    KMSG_CONSOLE_MODE.load(Ordering::Relaxed)
}

/// Parse a policy name (`on` / `ratelimit` / `off`) into its mode value.
pub fn kmsg_console_mode_from_str(name: &str) -> Option<u8> {
    match name {
        "on" => Some(kmsg_console::ON),
        "ratelimit" => Some(kmsg_console::RATELIMIT),
        "off" => Some(kmsg_console::OFF),
        _ => None,
    }
}

/// Decide whether the next userspace /dev/kmsg line may be echoed to the
/// console. In ON mode the answer is always yes; in RATELIMIT mode the
/// console gets `BURST` lines per window; in OFF mode never.
fn kmsg_console_allows_echo() -> bool {
    match kmsg_console_mode() {
        kmsg_console::ON => true,
        kmsg_console::OFF => false,
        _ => {
            let now = crate::drivers::timer::get_jiffies();
            // Start a new window when the previous one expired (or when
            // the limiter was never armed yet).
            let window = KMSG_RL_WINDOW.load(Ordering::Relaxed);
            if window == 0 || now.wrapping_sub(window) >= kmsg_console::WINDOW_JIFFIES {
                KMSG_RL_WINDOW.store(now, Ordering::Relaxed);
                KMSG_RL_COUNT.store(1, Ordering::Relaxed);
                true
            } else {
                let n = KMSG_RL_COUNT.fetch_add(1, Ordering::Relaxed);
                n < kmsg_console::BURST
            }
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
///
/// The console copy of userspace writes follows the `printk.devkmsg`
/// policy (see kmsg_console_mode): `on` (reference parity — every write
/// is echoed subject to CONSOLE_LOGLEVEL), `ratelimit` (the console sees
/// a bounded trickle — the reference's own devkmsg ratelimit spirit) and
/// `off` (ring only). The RING always records every line in all three
/// modes, so hilog consumers lose nothing.
fn kmsg_file_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let text = core::str::from_utf8(buf).unwrap_or("<binary>");
    let (level, text) = match text.as_bytes() {
        [b'<', d, b'>', rest @ ..] if (b'0'..=b'7').contains(d) => {
            (d - b'0', core::str::from_utf8(rest).unwrap_or(text))
        }
        _ => (loglevel::KERN_INFO, text),
    };
    let echo = kmsg_console_allows_echo();
    printk_ring_always_ex(level, format_args!("[kmsg] {}", text), echo);
    buf.len() as isize
}

/// Poll handler for /dev/kmsg: POLLIN iff a record newer than the fd's
/// position exists; always writable. The old stub returned POLLERR —
/// epoll treats that as permanently ready, so every kmsg waiter
/// woke, read EOF, and re-armed in a tight loop.
fn kmsg_file_poll(file: &crate::fs::file::File, events: u16) -> u16 {
    use crate::fs::event_notify::poll_events::*;
    let mut ready = 0u16;
    if events & POLLIN != 0 {
        let rb = RING_BUFFER.lock_irqsave();
        if (file.get_pos() as u64) < RING_NEXT_SEQ.load(Ordering::Relaxed) {
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
            let next_seq = RING_NEXT_SEQ.load(Ordering::Relaxed);
            drop(rb);
            file.set_pos(next_seq);
            next_seq as isize
        }
        _ => -crate::errno::constants::EINVAL as isize,
    }
}

/// FileOps for /dev/kmsg
pub static KMSG_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps::builder()
    .read(kmsg_file_read)
    .write(kmsg_file_write)
    .lseek(kmsg_file_lseek)
    .poll(kmsg_file_poll)
    .build();

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
        return -crate::errno::constants::EBUSY;
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
            let next_seq = RING_NEXT_SEQ.load(Ordering::Relaxed);
            if rb.read_seq < next_seq {
                // Skip records the ring has already overwritten.
                let oldest = next_seq.saturating_sub(RING_BUFFER_CAPACITY as u64);
                let seq = rb.read_seq.max(oldest);
                let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
                let record = ring_rec(idx as u64);
                if record.pub_seq.load(Ordering::Acquire) != seq + 1 {
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
                let text_bytes = &record.text[..rec_text_len(record)];

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
            return -(crate::errno::constants::EAGAIN as isize);
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

    // The running-task borrow is re-derived at each use window and never
    // held across schedule() — the reborrow discipline that replaces the
    // raw-pointer dodge (two live &mut Task aliases across schedule() were
    // the unsound shape this used to paper over).
    let pid = match crate::sched::current() {
        Some(c) => c.pid(),
        None => return false,
    };

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
        crate::sched::current().map(|c| c.set_state(TaskState::new(TaskState::INTERRUPTIBLE)));
        if crate::drivers::timer::get_jiffies() >= target || crate::signal::signal_pending() {
            if let Some(c) = crate::sched::current() {
                c.set_state(TaskState::new(TaskState::RUNNING));
                // A racing wake may have enqueued us while still executing —
                // take ourselves back off (NEW-C2 discipline).
                crate::sched::dequeue_task(c);
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
    use crate::fs::event_notify::poll_events::*;
    let rb = RING_BUFFER.lock_irqsave();
    let readable = rb.read_seq < RING_NEXT_SEQ.load(Ordering::Relaxed);
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
    -(crate::errno::constants::ESPIPE as isize)
}

/// FileOps for the /proc/kmsg stream (attached by procfs_get_file_ops
/// for the node whose content_generator is generate_kmsg).
pub static PROC_KMSG_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps::builder()
    .read(proc_kmsg_read)
    .lseek(proc_kmsg_lseek)
    .close(proc_kmsg_close)
    .poll(proc_kmsg_poll)
    .build();

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
    // INV-PRINTK-LOGGER-1: single early-boot install — the raw edge
    // lives in the root audited file.
    crate::boot_edges::set_global_logger(&LOGGER);
    log::set_max_level(LevelFilter::Trace);
}

// ==================== Persistent Log (kmsg to disk) ====================

/// The persistent kernel log — /var/log/kmsg, the console tail on disk.
///
/// Why it exists: the pstore record survives a RESET (RAM keeps its
/// bytes) but not a power loss, and it only captures what the panic
/// handler arms — an early-boot death or a hard hang leaves nothing.
/// The disk copy covers both: everything the printk ring held at the
/// last flush survives the power cut.
///
/// Design (the klogd redesign — the old per-printk synchronous ext4
/// write is gone):
///
/// - **Zero hot-path cost.** No code runs on the printk write path any
///   more. The records already live in the printk ring; the flusher is
///   the only reader of them (a private cursor — it never touches the
///   syslog(2) sequential-read cursor).
/// - **klogd kernel thread.** A kthread (the khungtaskd pattern: a
///   one-shot timer as the waker, INTERRUPTIBLE sleep between) wakes
///   every 30 s and appends the ring delta (records newer than the
///   last flushed seq, newest-bound 512 records per flush) to
///   /var/log/kmsg. ext4 writes run in task context only — sync block
///   I/O from a timer softirq is not allowed.
/// - **Size cap.** The file wraps at 256 KiB: the offset restarts at 0
///   when it passes the cap, keeping the NEWEST ~256 KiB (the inode
///   size stays at the high-water mark so a plain read sees the whole
///   window; after the first wrap the file is a torn window — old tail
///   then new head, the usual rotating-log artifact).
/// - **Early-boot coverage.** init() flushes once immediately, so the
///   whole boot log up to that point lands on disk in one write;
///   after that the staleness bound is the 30 s period (plus whatever
///   the ring still holds — the ring is the real buffer, the disk the
///   archive).
/// - **No disk writes on panic/oops.** The panic path never touches
///   the storage stack (the panic may BE the storage stack — the same
///   reason the reference kernel's panic path does not sync). pstore
///   owns the panic moment; this file owns steady state.
mod persistent_log {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::{RING_BUFFER, RING_BUFFER_CAPACITY};

    /// The file's size cap — the newest 256 KiB of console tail.
    const MAX_LOG_SIZE: u64 = 256 * 1024;

    /// Flush period in seconds.
    const FLUSH_PERIOD_SECS: u64 = 30;

    /// Upper bound on records per flush (bounds the work and the
    /// write size; a busy 30 s window can outgrow the cap anyway).
    const MAX_RECORDS_PER_FLUSH: u64 = 512;

    /// Path to the persistent log file.
    const LOG_PATH: &str = "/var/log/kmsg";

    /// The kthread is running.
    static RUNNING: AtomicBool = AtomicBool::new(false);

    /// Cached inode number for the log file (0 = not yet resolved).
    static FILE_INO: AtomicU64 = AtomicU64::new(0);

    /// Current write offset in the log file (wraps at the cap).
    static WRITE_OFFSET: AtomicU64 = AtomicU64::new(0);

    /// The seq of the last record written to disk (the flush cursor —
    /// private to this module; the syslog(2) cursor is untouched).
    static LAST_SEQ: AtomicU64 = AtomicU64::new(0);

    /// One log line, staged for the file: "<seq> <timestamp_us> <level>
    /// <pid> <cpu> <text>". Pre-rendered because ext4_file_write takes
    /// one contiguous slice and the record text lives in the printk
    /// ring's fixed arrays.
    struct Line {
        buf: [u8; 320],
        len: usize,
    }

    impl Line {
        fn push(&mut self, bytes: &[u8]) {
            let n = bytes.len().min(self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
            self.len += n;
        }
        fn push_dec(&mut self, mut v: u64) {
            let mut digs = [0u8; 20];
            let mut k = 0;
            if v == 0 {
                digs[0] = b'0';
                k = 1;
            }
            while v > 0 {
                digs[k] = b'0' + (v % 10) as u8;
                k += 1;
                v /= 10;
            }
            let mut tmp = [0u8; 20];
            tmp[..k].copy_from_slice(&digs[..k]);
            tmp[..k].reverse();
            self.push(&tmp[..k]);
        }
    }

    /// Is klogd running?
    pub fn is_running() -> bool {
        RUNNING.load(Ordering::Acquire)
    }

    /// Initialize the persistent log and start the klogd thread.
    ///
    /// Must be called after the ext4 root is mounted (main boot, right
    /// where the old disabled call sat). The immediate first flush
    /// archives the boot log; the thread keeps it current afterwards.
    pub fn init() {
        if RUNNING.load(Ordering::Acquire) {
            return;
        }
        // Resolve or create the file before the first flush so the
        // flush path never creates.
        resolve_file();
        if crate::process::kthread::kthread_run(
            klogd_fn,
            core::ptr::null_mut(),
            "klogd",
        )
        .is_some()
        {
            RUNNING.store(true, Ordering::Release);
            // Seed the archive with the boot log now (the thread's
            // first wake is a full period away).
            flush_once();
        } else {
            crate::pr_warn!("printk: klogd failed to start (no persistent log)");
        }
    }

    /// Resolve or create the log file; returns the inode or None.
    fn resolve_file() -> Option<u32> {
        let fs_ptr = match crate::fs::ext4::get_ext4_fs() {
            Some(ptr) if !ptr.is_null() => ptr,
            _ => return None,
        };
        // fs_ptr is the mount's instance word (get_ext4_fs's non-null
        // GLOBAL) — resolved through the framework edge (INV-FS-GLOBALSB-1
        // provenance).
        let fs = crate::fs::edges::static_ref_unchecked(fs_ptr as *mut crate::fs::ext4::Ext4FileSystem);
        let cached = FILE_INO.load(Ordering::Relaxed);
        if cached != 0 {
            // Still alive? A deleted file's inode read fails; reset and
            // fall through to re-create next time.
            match fs.read_inode(cached as u32) {
                Ok(_) => return Some(cached as u32),
                Err(_) => {
                    FILE_INO.store(0, Ordering::Relaxed);
                    return None;
                }
            }
        }
        if let Ok((ino, _inode)) = fs.lookup_path(LOG_PATH) {
            FILE_INO.store(ino as u64, Ordering::Relaxed);
            return Some(ino);
        }
        match crate::fs::ext4::create_file(LOG_PATH, 0o644) {
            Ok(vfs_inode) => {
                FILE_INO.store(vfs_inode.ino, Ordering::Relaxed);
                WRITE_OFFSET.store(0, Ordering::Relaxed);
                Some(vfs_inode.ino as u32)
            }
            Err(_) => None,
        }
    }

    /// klogd main loop: sleep one period, flush, repeat (the khungtaskd
    /// wake discipline — a one-shot timer as the waker so the sleep is
    /// real, not wake-by-luck).
    extern "C" fn klogd_fn(_arg: *mut core::ffi::c_void) -> i32 {
        use crate::process::task::TaskState;
        let interval_jiffies =
            (FLUSH_PERIOD_SECS * crate::config::KERNEL_HZ as u64).max(1);
        loop {
            if crate::process::kthread::kthread_should_stop() {
                break;
            }
            let my_pid = match crate::sched::current() {
                Some(t) => t.pid(),
                None => break,
            };
            let target = crate::drivers::timer::get_jiffies() + interval_jiffies;
            let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
            if timer_id != 0 {
                // Re-derive the running-task borrow per window (never held
                // across schedule() — the discipline that replaces the raw
                // pointer this loop used to carry).
                crate::sched::current().map(|t| t.set_state(TaskState::new(TaskState::INTERRUPTIBLE)));
                // Re-check AFTER marking sleeping: the one-shot timer may
                // have fired between arming and set_state — either its wake
                // captured the INTERRUPTIBLE state, or the deadline is
                // already visible.
                if crate::drivers::timer::get_jiffies() >= target {
                    if let Some(t) = crate::sched::current() {
                        t.set_state(TaskState::new(TaskState::RUNNING));
                        // NEW-C2: a racing wake may have enqueued us while we
                        // are still executing — take ourselves back off.
                        crate::sched::dequeue_task(t);
                    }
                    crate::timer::del_timer(timer_id);
                } else {
                    // Enable interrupts so the tick can reach us, then sleep
                    // (kthread context, like syscall context, runs SIE=0 —
                    // without this the sleeping CPU takes no ticks and the
                    // 30 s flush never fires; the smoke saw the file stuck
                    // at the init flush, 3 lines, forever).
                    crate::arch::cpu::restore_irq(true);
                    crate::sched::schedule();
                    // Spurious wake before the deadline: drop the timer and
                    // loop (re-armed next iteration).
                    if crate::timer::timer_pending(timer_id) {
                        crate::timer::del_timer(timer_id);
                    }
                }
            } else {
                // Timer table full — stay runnable, yield one scheduling
                // round and retry the arm.
                crate::sched::schedule();
            }
            if crate::process::kthread::kthread_should_stop() {
                break;
            }
            flush_once();
        }
        0
    }

    /// One flush: append every ring record newer than LAST_SEQ (bounded
    /// by MAX_RECORDS_PER_FLUSH) to the file. Errors are silent except
    /// for one warn line — the persistent log must never affect normal
    /// operation, and a transient ext4 failure just means the delta
    /// stays buffered in the ring until the next period.
    pub fn flush_once() {
        let Some(ino) = resolve_file() else {
            return;
        };
        let fs_ptr = match crate::fs::ext4::get_ext4_fs() {
            Some(ptr) if !ptr.is_null() => ptr,
            _ => return,
        };
        // Same provenance as resolve_file (INV-FS-GLOBALSB-1 edge).
        let fs = crate::fs::edges::static_ref_unchecked(fs_ptr as *mut crate::fs::ext4::Ext4FileSystem);

        // Stage the delta: records with seq > LAST_SEQ, oldest-first,
        // at most MAX_RECORDS_PER_FLUSH of the newest ones.
        let mut lines: alloc::vec::Vec<(u64, Line)> = alloc::vec::Vec::new();
        {
            let _rb = RING_BUFFER.lock_irqsave();
            let ring_next = super::RING_NEXT_SEQ.load(Ordering::Relaxed);
        let oldest_kept = ring_next.saturating_sub(RING_BUFFER_CAPACITY as u64);
            let start = (LAST_SEQ.load(Ordering::Relaxed) + 1).max(oldest_kept);
            let end = ring_next;
            // Bound the count (newest-bound): if the window is huge,
            // skip to the newest MAX_RECORDS_PER_FLUSH records.
            let start = start.max(end.saturating_sub(MAX_RECORDS_PER_FLUSH));
            let mut seq = start;
            while seq < end {
                let idx = (seq % RING_BUFFER_CAPACITY as u64) as usize;
                let record = super::ring_rec(idx as u64);
                if record.pub_seq.load(Ordering::Acquire) == seq + 1 && record.text_len != 0 {
                    let mut line = Line { buf: [0; 320], len: 0 };
                    let timestamp_us = record.timestamp / 10; // 10 MHz ticks -> us
                    line.push_dec(record.seq);
                    line.push(b" ");
                    line.push_dec(timestamp_us);
                    line.push(b" ");
                    line.push_dec(record.level as u64);
                    line.push(b" ");
                    line.push_dec(record.pid as u64);
                    line.push(b" ");
                    line.push_dec(record.cpu_id as u64);
                    line.push(b" ");
                    line.push(&record.text[..super::rec_text_len(record)]);
                    if line.buf[line.len - 1] != b'\n' {
                        line.push(b"\n");
                    }
                    lines.push((seq, line));
                }
                seq += 1;
            }
        }

        if lines.is_empty() {
            return;
        }

        // Write each line, advancing the rolling offset (wraps at the
        // cap) and the flush cursor (the seq of the last line that
        // actually landed). A failed write stops the flush: the
        // remaining records stay buffered in the ring for the next
        // period — the cursor must never pass an unwritten record.
        //
        // COHERENCE (2026-10-26, the stuck-at-init-flush root cause): the
        // flush writes through ext4_kernel_write_at — the write(2) core —
        // NOT the low-level ext4_file_write. The low-level path only
        // touches the buffer cache and the on-disk inode; it skips the
        // three surfaces write(2) keeps coherent (the VFS dentry inode's
        // size, the sb-boxed Ext4Inode copy that bounds every read, and
        // the page-cache write-through), so every kernel-side write was
        // invisible to userspace until some lookup re-read the inode from
        // disk — the file looked stuck at the init flush forever while
        // the writes had been landing all along (probe-printed offsets
        // advancing next to a guest wc -l that never moved).
        let mut offset = WRITE_OFFSET.load(Ordering::Relaxed);
        let mut final_seq = LAST_SEQ.load(Ordering::Relaxed);
        let mut written = 0usize;
        for (seq, line) in &lines {
            let data = &line.buf[..line.len];
            let n = crate::fs::ext4::file::ext4_kernel_write_at(fs, ino, offset, data);
            if n > 0 {
                offset += n as u64;
                if offset >= MAX_LOG_SIZE {
                    offset = 0;
                }
                final_seq = *seq;
                written += 1;
            } else {
                break;
            }
        }
        WRITE_OFFSET.store(offset, Ordering::Relaxed);
        LAST_SEQ.store(final_seq, Ordering::Relaxed);
        if written < lines.len() {
            // One warn line per failing flush (the doc contract): the
            // delta stays in the ring; without this the archive silently
            // stops and the only symptom is a stale /var/log/kmsg.
            crate::pr_warn!(
                "klogd: flush wrote {}/{} records (ext4 write error at seq {}); delta stays buffered",
                written,
                lines.len(),
                final_seq + 1
            );
        }
        // No explicit size bookkeeping: ext4_file_write only ever GROWS
        // the size (never lowers it), so the rotating file's high-water
        // mark survives the wrap exactly as the old manual pin intended.
    }
}

/// Initialize the persistent log (start klogd + archive the boot log).
///
/// Must be called after the ext4 root is mounted.
pub fn persistent_log_init() {
    persistent_log::init();
}

/// Is the persistent log running? (reporting/diagnostics)
pub fn persistent_log_running() -> bool {
    persistent_log::is_running()
}
