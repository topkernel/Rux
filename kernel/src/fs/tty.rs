//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! TTY line-discipline core + termios state
//!
//! Shared substrate for:
//! - the UART console (`CONSOLE_TTY` — termios/winsize/fg-pgrp state used by
//!   the global TCGETS/TCSETS ioctl path and console.rs ISIG/echo decisions;
//!   the console READ path stays on its own UART ring buffer for compat)
//! - pty slaves (fs/pty.rs) — full line discipline: canonical editing,
//!   echo, ISIG signal generation, VEOF, blocking reads
//!
//! Linux-semantics simplifications (documented per feature):
//! - single input ring; canonical mode commits a line on NL / VEOL / VEOF /
//!   buffer pressure; ERASE/VKILL edit only the uncommitted tail
//! - VMIN/VTIME are not timed (raw mode delivers immediately)
//! - IEXTEN/VLNEXT/VWERASE/VREPRINT/VDISCARD are accepted but not acted on

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::process::wait::WaitQueueHead;
use crate::signal::Signal;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// termios constants (asm-generic ABI — RISC-V uses this ABI)
// ============================================================================

/// c_cc array size in the kernel/user termios layout
pub const NCCS: usize = 32;

// c_iflag
#[allow(dead_code)] // UAPI completeness
pub const IGNBRK: u32 = 0o001;
#[allow(dead_code)] // UAPI completeness
pub const BRKINT: u32 = 0o002;
#[allow(dead_code)] // UAPI completeness
pub const IGNPAR: u32 = 0o004;
#[allow(dead_code)] // UAPI completeness
pub const PARMRK: u32 = 0o010;
#[allow(dead_code)] // UAPI completeness
pub const INPCK: u32 = 0o020;
#[allow(dead_code)] // UAPI completeness
pub const ISTRIP: u32 = 0o040;
pub const INLCR: u32 = 0o100;
pub const IGNCR: u32 = 0o200;
pub const ICRNL: u32 = 0o400;
pub const IXON: u32 = 0o2000;

// c_oflag
pub const OPOST: u32 = 0o001;
pub const ONLCR: u32 = 0o004;

// c_cflag
pub const B38400: u32 = 0o017;
#[allow(dead_code)] // UAPI completeness
pub const CS7: u32 = 0o060;
pub const CS8: u32 = 0o070;
#[allow(dead_code)] // UAPI completeness
pub const CSTOPB: u32 = 0o100;
pub const CREAD: u32 = 0o200;
#[allow(dead_code)] // UAPI completeness
pub const PARENB: u32 = 0o400;
pub const HUPCL: u32 = 0o1000;
#[allow(dead_code)] // UAPI completeness
pub const CLOCAL: u32 = 0o2000;

// c_lflag
pub const ISIG: u32 = 0x0001;
pub const ICANON: u32 = 0x0002;
pub const ECHO: u32 = 0x0008;
pub const ECHOE: u32 = 0x0010;
pub const ECHOK: u32 = 0x0020;
#[allow(dead_code)] // UAPI completeness
pub const ECHONL: u32 = 0x0040;
#[allow(dead_code)] // UAPI completeness
pub const NOFLSH: u32 = 0x0080;
#[allow(dead_code)] // UAPI completeness
pub const TOSTOP: u32 = 0x0100;
#[allow(dead_code)] // UAPI completeness
pub const ECHOCTL: u32 = 0x0200;
#[allow(dead_code)] // UAPI completeness
pub const IEXTEN: u32 = 0x8000;

// c_cc indices (asm-generic)
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;

/// _POSIX_VDISABLE — control chars with this value are disabled.
pub const VDISABLE: u8 = 0;

/// User-space termios size on the asm-generic ABI:
/// 4 x u32 flags + c_line + c_cc[32] + padding = 52 bytes.
pub const TERMIOS_USER_SIZE: usize = 52;

/// User-space termio (BSD-style, TCGETA/TCSETA) size:
/// 4 x u16 flags + c_line + c_cc[8] = 17 bytes.
pub const TERMIO_USER_SIZE: usize = 17;

/// Kernel-side termios state
#[derive(Clone, Copy, Debug)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; NCCS],
}

impl Termios {
    /// Default settings — identical to what the console TCGETS path used to
    /// report, so existing userspace sees no behavior change.
    pub const fn default() -> Self {
        let mut c_cc = [0u8; NCCS];
        c_cc[VINTR] = 0x03; // ^C
        c_cc[VQUIT] = 0x1c; // ^\
        c_cc[VERASE] = 0x7f; // DEL
        c_cc[VKILL] = 0x15; // ^U
        c_cc[VEOF] = 0x04; // ^D
        c_cc[VTIME] = 0;
        c_cc[VMIN] = 1;
        c_cc[VSTART] = 0x11; // ^Q
        c_cc[VSTOP] = 0x13; // ^S
        c_cc[VSUSP] = 0x1a; // ^Z
        c_cc[VREPRINT] = 0x12; // ^R
        c_cc[VDISCARD] = 0x0f; // ^O
        c_cc[VWERASE] = 0x17; // ^W
        c_cc[VLNEXT] = 0x16; // ^V
        Self {
            c_iflag: ICRNL | IXON,
            c_oflag: OPOST | ONLCR,
            c_cflag: B38400 | CS8 | CREAD | HUPCL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK,
            c_line: 0,
            c_cc,
        }
    }

    pub const fn canonical(&self) -> bool {
        self.c_lflag & ICANON != 0
    }
}

/// Serialize a Termios into the 52-byte asm-generic user layout.
pub fn termios_to_user_bytes(t: &Termios, out: &mut [u8; TERMIOS_USER_SIZE]) {
    out.fill(0);
    // SAFETY: out is a 52-byte buffer; unaligned u32 writes at 0/4/8/12 are
    // in bounds.
    unsafe {
        let p = out.as_mut_ptr() as *mut u32;
        p.add(0).write_unaligned(t.c_iflag);
        p.add(1).write_unaligned(t.c_oflag);
        p.add(2).write_unaligned(t.c_cflag);
        p.add(3).write_unaligned(t.c_lflag);
    }
    out[16] = t.c_line;
    out[17..17 + NCCS].copy_from_slice(&t.c_cc);
}

/// Parse a Termios from the 52-byte asm-generic user layout.
pub fn termios_from_user_bytes(buf: &[u8; TERMIOS_USER_SIZE]) -> Termios {
    // SAFETY: buf is a 52-byte buffer; unaligned u32 reads at 0/4/8/12.
    unsafe {
        let p = buf.as_ptr() as *const u32;
        Termios {
            c_iflag: p.add(0).read_unaligned(),
            c_oflag: p.add(1).read_unaligned(),
            c_cflag: p.add(2).read_unaligned(),
            c_lflag: p.add(3).read_unaligned(),
            c_line: buf[16],
            c_cc: {
                let mut cc = [0u8; NCCS];
                cc.copy_from_slice(&buf[17..17 + NCCS]);
                cc
            },
        }
    }
}

/// Serialize a Termios into the 17-byte termio (TCGETA) layout.
pub fn termios_to_termio_bytes(t: &Termios, out: &mut [u8; TERMIO_USER_SIZE]) {
    out.fill(0);
    // SAFETY: out is a 17-byte buffer; unaligned u16 writes at 0/2/4/6 are in
    // bounds.
    unsafe {
        let p = out.as_mut_ptr() as *mut u16;
        p.add(0).write_unaligned(t.c_iflag as u16);
        p.add(1).write_unaligned(t.c_oflag as u16);
        p.add(2).write_unaligned(t.c_cflag as u16);
        p.add(3).write_unaligned(t.c_lflag as u16);
    }
    out[8] = t.c_line;
    out[9..17].copy_from_slice(&t.c_cc[..8]);
}

/// Parse a Termios from the 17-byte termio (TCSETA) layout. Flags above
/// 16 bits keep their previous values (termio cannot carry them).
pub fn termios_from_termio_bytes(buf: &[u8; TERMIO_USER_SIZE], base: &Termios) -> Termios {
    let mut t = *base;
    // SAFETY: buf is a 17-byte buffer; unaligned u16 reads at 0/2/4/6.
    unsafe {
        let p = buf.as_ptr() as *const u16;
        t.c_iflag = p.add(0).read_unaligned() as u32;
        t.c_oflag = p.add(1).read_unaligned() as u32;
        t.c_cflag = p.add(2).read_unaligned() as u32;
        t.c_lflag = p.add(3).read_unaligned() as u32;
    }
    t.c_line = buf[8];
    t.c_cc[..8].copy_from_slice(&buf[9..17]);
    t
}

// ============================================================================
// Window size
// ============================================================================

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

impl WinSize {
    pub const fn default() -> Self {
        // 80x25 — same defaults the old hardcoded TIOCGWINSZ reported.
        Self {
            ws_row: 25,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    pub fn to_le_bytes(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&self.ws_row.to_le_bytes());
        b[2..4].copy_from_slice(&self.ws_col.to_le_bytes());
        b[4..6].copy_from_slice(&self.ws_xpixel.to_le_bytes());
        b[6..8].copy_from_slice(&self.ws_ypixel.to_le_bytes());
        b
    }

    pub fn from_le_bytes(b: &[u8; 8]) -> Self {
        Self {
            ws_row: u16::from_le_bytes([b[0], b[1]]),
            ws_col: u16::from_le_bytes([b[2], b[3]]),
            ws_xpixel: u16::from_le_bytes([b[4], b[5]]),
            ws_ypixel: u16::from_le_bytes([b[6], b[7]]),
        }
    }
}

// ============================================================================
// Input ring with canonical watermark
// ============================================================================

/// TTY input ring capacity (post line-discipline bytes).
const TTY_INPUT_BUF: usize = 4096;

/// Input state guarded by a spinlock.
///
/// One ring plus absolute counters implements both canonical and raw mode:
/// - `consumed` — bytes already read by the reader
/// - `committed` — watermark: bytes readable NOW (in canonical mode only
///   bytes up to the last line terminator are committed; in raw mode every
///   byte is committed on arrival)
/// - `written` — bytes stored, including the uncommitted canonical line
///   currently being edited (ERASE/VKILL retract this tail)
struct TtyInput {
    data: [u8; TTY_INPUT_BUF],
    consumed: usize,
    committed: usize,
    written: usize,
    /// One-shot EOF (VEOF on an empty line): the NEXT read returns 0 once.
    eof_pending: bool,
}

impl TtyInput {
    const fn new() -> Self {
        Self {
            data: [0; TTY_INPUT_BUF],
            consumed: 0,
            committed: 0,
            written: 0,
            eof_pending: false,
        }
    }

    /// Append a byte to the uncommitted tail. false = ring full.
    fn push(&mut self, c: u8) -> bool {
        if self.written - self.consumed >= TTY_INPUT_BUF {
            return false;
        }
        self.data[self.written % TTY_INPUT_BUF] = c;
        self.written += 1;
        true
    }

    /// Canonical line currently being edited (not yet readable).
    fn uncommitted_len(&self) -> usize {
        self.written - self.committed
    }

    /// ERASE: retract one uncommitted byte. true if a byte was erased.
    fn erase_last(&mut self) -> bool {
        if self.written > self.committed {
            self.written -= 1;
            true
        } else {
            false
        }
    }

    /// VKILL: retract the whole uncommitted line.
    fn kill_line(&mut self) {
        self.written = self.committed;
    }

    /// Commit the uncommitted line (terminator / VEOF / ring pressure).
    fn commit(&mut self) {
        self.committed = self.written;
    }

    fn set_eof(&mut self) {
        self.eof_pending = true;
    }

    /// Bytes available to the reader right now.
    fn readable_len(&self) -> usize {
        self.committed - self.consumed
    }

    /// Consume readable bytes into `buf`. In canonical mode a read returns
    /// at most one line (up to and including the first newline).
    fn read(&mut self, buf: &mut [u8], canon: bool) -> usize {
        let avail = self.readable_len();
        if avail == 0 {
            return 0;
        }
        let mut n = avail.min(buf.len());
        if canon {
            for i in 0..n {
                if self.data[(self.consumed + i) % TTY_INPUT_BUF] == b'\n' {
                    n = i + 1;
                    break;
                }
            }
        }
        for i in 0..n {
            buf[i] = self.data[(self.consumed + i) % TTY_INPUT_BUF];
        }
        self.consumed += n;
        n
    }
}

// ============================================================================
// TtyDevice
// ============================================================================

/// Output sink: function pointer + caller-supplied context.
/// Console → UART putchar; pty slave → append to the master read buffer.
pub type TtyOutputFn = fn(ctx: usize, bytes: &[u8]);

/// Does byte `c` trigger ISIG control char `cc`? (_POSIX_VDISABLE-aware)
fn isig_char(c: u8, cc: u8) -> bool {
    cc != VDISABLE && c == cc
}

/// A terminal device: line-discipline state shared by console and ptys.
pub struct TtyDevice {
    /// termios settings (lock_irqsave: read from the console RX IRQ path)
    termios: Spinlock<Termios>,
    /// window size
    winsize: Spinlock<WinSize>,
    /// foreground process group (0 = none set)
    pub fg_pgrp: AtomicU32,
    /// line-discipline input state
    input: Spinlock<TtyInput>,
    /// reader wait queue (blocking reads)
    pub read_waitq: WaitQueueHead,
    /// other end gone (pty master closed) → reads return EOF (0)
    pub hungup: AtomicBool,
    /// output sink context (0 = none), passed through to `output_fn`.
    /// Set once before the device is published (pty pair table / fd).
    output_ctx: core::sync::atomic::AtomicUsize,
    /// output function pointer: where echo/output goes (console UART or the
    /// pty master's read buffer)
    output_fn: TtyOutputFn,
}

// SAFETY: all mutable state is behind Spinlock/atomics; fn pointers are
// Send+Sync.
unsafe impl Sync for TtyDevice {}
unsafe impl Send for TtyDevice {}

impl TtyDevice {
    pub const fn new(output_fn: TtyOutputFn, output_ctx: usize) -> Self {
        Self {
            termios: Spinlock::new(Termios::default()),
            winsize: Spinlock::new(WinSize::default()),
            fg_pgrp: AtomicU32::new(0),
            input: Spinlock::new(TtyInput::new()),
            read_waitq: WaitQueueHead::new(),
            hungup: AtomicBool::new(false),
            output_ctx: core::sync::atomic::AtomicUsize::new(output_ctx),
            output_fn,
        }
    }

    /// Set the output sink context (back-pointer to the owning PtyPair).
    /// Must be called once, before the device is shared.
    pub fn set_output_ctx(&self, ctx: usize) {
        self.output_ctx.store(ctx, Ordering::Release);
    }

    // ---------------- termios / winsize / pgrp accessors ----------------

    /// Snapshot of the current termios.
    pub fn get_termios(&self) -> Termios {
        *self.termios.lock_irqsave()
    }

    /// Replace the termios.
    pub fn set_termios(&self, t: Termios) {
        *self.termios.lock_irqsave() = t;
    }

    /// c_lflag snapshot (called from the console RX IRQ path — IRQ-safe).
    pub fn lflag(&self) -> u32 {
        self.termios.lock_irqsave().c_lflag
    }

    pub fn echo_enabled(&self) -> bool {
        self.lflag() & ECHO != 0
    }

    pub fn get_winsize(&self) -> WinSize {
        *self.winsize.lock_irqsave()
    }

    /// Set the window size. Returns true if the size actually changed.
    pub fn set_winsize(&self, ws: WinSize) -> bool {
        let mut guard = self.winsize.lock_irqsave();
        let changed = *guard != ws;
        *guard = ws;
        changed
    }

    /// Send SIGWINCH to the foreground process group (after TIOCSWINSZ).
    pub fn send_sigwinch(&self) {
        let pgid = self.fg_pgrp.load(Ordering::Acquire);
        if pgid != 0 {
            crate::signal::send_signal_to_pgid(pgid, Signal::SIGWINCH as i32);
        }
    }

    // ---------------- input (receive path: line discipline) ----------------

    /// Receive one input byte (from a pty master write or console RX),
    /// run the line discipline: ISIG, canonical editing, input translation,
    /// echo. Delivers signals for ISIG characters.
    pub fn receive_byte(&self, c: u8) {
        let tio = self.get_termios();

        // --- ISIG: ^C / ^\ / ^Z ---
        if tio.c_lflag & ISIG != 0 {
            let signo = if isig_char(c, tio.c_cc[VINTR]) {
                Signal::SIGINT as i32
            } else if isig_char(c, tio.c_cc[VQUIT]) {
                Signal::SIGQUIT as i32
            } else if isig_char(c, tio.c_cc[VSUSP]) {
                Signal::SIGTSTP as i32
            } else {
                0
            };
            if signo != 0 {
                if tio.c_lflag & ECHO != 0 {
                    // Echo "^X\r\n" (raw, no OPOST — matches console.rs).
                    let vis = b'@' + (c & 0x1f);
                    self.write_output_raw(&[b'^', vis, b'\r', b'\n']);
                }
                self.send_isig(signo);
                return; // ISIG characters are never queued
            }
        }

        let canon = tio.c_lflag & ICANON != 0;

        // --- canonical line editing ---
        if canon {
            if c == tio.c_cc[VERASE] && tio.c_cc[VERASE] != VDISABLE {
                let erased = self.input.lock_irqsave().erase_last();
                if erased && tio.c_lflag & ECHO != 0 && tio.c_lflag & ECHOE != 0 {
                    self.write_output_raw(b"\x08 \x08");
                }
                return;
            }
            if c == tio.c_cc[VKILL] && tio.c_cc[VKILL] != VDISABLE {
                self.input.lock_irqsave().kill_line();
                if tio.c_lflag & ECHO != 0 && tio.c_lflag & ECHOK != 0 {
                    self.write_output_raw(b"\r\n");
                }
                return;
            }
            if c == tio.c_cc[VEOF] && tio.c_cc[VEOF] != VDISABLE {
                let became_readable = {
                    let mut inp = self.input.lock_irqsave();
                    if inp.uncommitted_len() > 0 {
                        inp.commit(); // deliver the partial line
                        true
                    } else {
                        inp.set_eof(); // empty line → one-shot EOF
                        false
                    }
                };
                if became_readable {
                    self.read_waitq.wake_up_all();
                }
                return;
            }
        }

        // --- input translation (c_iflag) ---
        let mut ch = c;
        if ch == b'\r' {
            if tio.c_iflag & IGNCR != 0 {
                return; // dropped
            }
            if tio.c_iflag & ICRNL != 0 {
                ch = b'\n';
            }
        } else if ch == b'\n' && tio.c_iflag & INLCR != 0 {
            ch = b'\r';
        }

        // --- terminator detection (canonical commit points) ---
        let terminate = canon
            && (ch == b'\n'
                || (tio.c_cc[VEOL] != VDISABLE && ch == tio.c_cc[VEOL])
                || (tio.c_cc[VEOL2] != VDISABLE && ch == tio.c_cc[VEOL2]));

        let became_readable = {
            let mut inp = self.input.lock_irqsave();
            if inp.push(ch) {
                if terminate || !canon {
                    inp.commit();
                    true
                } else {
                    false
                }
            } else {
                // Ring full: in canonical mode force-commit what we have so
                // the line is not lost (MAX_CANON-style pressure release);
                // in raw mode the byte is dropped.
                if canon {
                    inp.commit();
                    true
                } else {
                    false
                }
            }
        };
        if became_readable {
            self.read_waitq.wake_up_all();
        }

        // --- echo (raw bytes, matches the console.rs echo behavior) ---
        if tio.c_lflag & ECHO != 0 {
            if ch == b'\n' || ch == b'\r' {
                self.write_output_raw(b"\r\n");
            } else if ch == 127 || ch == 8 {
                // Raw-mode backspace passthrough: canonical ERASE was handled
                // above; here (non-canonical, or ERASE redefined) mirror the
                // console's BS-SP-BS echo.
                self.write_output_raw(b"\x08 \x08");
            } else {
                self.write_output_raw(&[ch]);
            }
        }
    }

    /// Deliver an ISIG signal to this tty's foreground process group
    /// (falling back to the caller's, like the console path).
    fn send_isig(&self, signo: i32) {
        let mut pgid = self.fg_pgrp.load(Ordering::Acquire);
        if pgid == 0 {
            pgid = crate::process::current_pgid();
        }
        if pgid != 0 {
            crate::signal::send_signal_to_pgid(pgid, signo);
        }
    }

    // ---------------- input (read path) ----------------

    /// Bytes currently readable (FIONREAD support).
    pub fn fionread_count(&self) -> usize {
        self.input.lock_irqsave().readable_len()
    }

    /// Reader-relevant readiness: committed data OR a pending one-shot EOF
    /// (poll must wake a reader that is blocked after a ^D).
    pub fn input_poll_ready(&self) -> bool {
        let inp = self.input.lock_irqsave();
        inp.readable_len() > 0 || inp.eof_pending
    }

    /// Read processed input. Canonical mode returns at most one line.
    /// Returns the byte count (0 = EOF) or a negative errno.
    pub fn read_input(&self, buf: &mut [u8], nonblock: bool) -> isize {
        use crate::errno::constants::{EAGAIN, EINTR};

        if buf.is_empty() {
            return 0;
        }

        loop {
            let canon = self.get_termios().canonical();
            let (n, eof) = {
                let mut inp = self.input.lock_irqsave();
                let n = inp.read(buf, canon);
                // Consume the one-shot EOF flag atomically with the empty
                // read (two racing readers must not both see it).
                let eof = n == 0 && inp.eof_pending;
                if eof {
                    inp.eof_pending = false;
                }
                (n, eof)
            };
            if n > 0 {
                return n as isize;
            }

            // Queue empty: one-shot VEOF first, then hangup = persistent EOF.
            if eof {
                return 0;
            }
            if self.hungup.load(Ordering::Acquire) {
                return 0;
            }

            if nonblock {
                return -(EAGAIN as isize);
            }

            // Blocking read (pipe.rs wait discipline).
            let current = match crate::sched::current() {
                Some(task) => task,
                None => return 0,
            };

            self.read_waitq.prepare_to_wait(current, false, true);

            // Re-check AFTER registering (lost-wakeup guard).
            let has_data = {
                let inp = self.input.lock_irqsave();
                inp.readable_len() > 0 || inp.eof_pending
            };
            if has_data || self.hungup.load(Ordering::Acquire) {
                self.read_waitq.finish_wait(current);
                crate::sched::dequeue_task(&*current);
                continue;
            }

            // A signal that arrived while still RUNNING generated no wakeup.
            if crate::signal::signal_pending() {
                self.read_waitq.finish_wait(current);
                crate::sched::dequeue_task(&*current);
                return -(EINTR as isize);
            }

            // R54: re-arm interrupts so ticks/IPIs reach this CPU.
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();

            self.read_waitq.finish_wait(current);

            if crate::signal::signal_pending() {
                return -(EINTR as isize);
            }
        }
    }

    /// Flush the input queue (TCSETSF discipline flush).
    pub fn flush_input(&self) {
        let mut inp = self.input.lock_irqsave();
        inp.consumed = inp.written;
        inp.committed = inp.written;
        inp.eof_pending = false;
    }

    // ---------------- output ----------------

    /// OPOST/ONLCR decision for the program-output path (used by the pty
    /// slave write loop, which performs its own blocking).
    pub fn output_translates_nl(&self) -> bool {
        let tio = self.get_termios();
        tio.c_oflag & OPOST != 0 && tio.c_oflag & ONLCR != 0
    }

    /// Write output bytes verbatim to the sink (echo path).
    pub fn write_output_raw(&self, bytes: &[u8]) {
        let ctx = self.output_ctx.load(Ordering::Acquire);
        (self.output_fn)(ctx, bytes);
    }
}

// ============================================================================
// Console tty singleton
// ============================================================================

/// Console output sink: raw UART bytes (translation already applied).
fn console_output(_ctx: usize, bytes: &[u8]) {
    for &b in bytes {
        crate::console::putchar(b);
    }
}

/// The console (UART) terminal. Owns the SHARED termios/winsize/fg-pgrp
/// state consulted by console.rs (ISIG/echo decisions) and the global
/// console tty ioctls. Its input ring is unused — the console read path
/// keeps its own UART ring buffer (compat).
static CONSOLE_TTY: TtyDevice = TtyDevice::new(console_output, 0);

/// Access the console tty device.
pub fn console() -> &'static TtyDevice {
    &CONSOLE_TTY
}
