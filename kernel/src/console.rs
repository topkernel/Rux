//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Console driver — UART 16550A with interrupt-driven RX
//!
//! Split initialization:
//! - `early_init()`: basic MMIO setup, enable FIFOs (before PLIC)
//! - `init_irq()`: enable RX interrupt, register IRQ handler (after PLIC)

use core::fmt;
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::spinlock::{Spinlock, SpinlockGuard, SpinlockIrqGuard};

#[cfg(feature = "riscv64")]
use crate::arch::mm::fixmap::uart_virt_addr;

// ============================================================================
// UART 16550A Register Offsets
// ============================================================================

const UART_THR: usize = 0; // Transmit Holding Register (write)
const UART_RBR: usize = 0; // Receive Buffer Register (read)
const UART_IER: usize = 1; // Interrupt Enable Register
const UART_IIR: usize = 2; // Interrupt Identification Register (read)
const UART_FCR: usize = 2; // FIFO Control Register (write)
const UART_LCR: usize = 3; // Line Control Register
const UART_MCR: usize = 4; // Modem Control Register
const UART_LSR: usize = 5; // Line Status Register

// IER bits
const IER_RX_ENABLE: u8 = 0x01; // Enable RX data interrupt
const IER_TX_ENABLE: u8 = 0x02; // Enable TX holding register empty interrupt

// FCR bits
const FCR_ENABLE_FIFO: u8 = 0x01; // Enable FIFOs
const FCR_CLEAR_RX: u8 = 0x02; // Clear receive FIFO
const FCR_CLEAR_TX: u8 = 0x04; // Clear transmit FIFO
const FCR_TRIGGER_8: u8 = 0x80; // Trigger at 8 bytes (half of 16-byte FIFO)

// LSR bits
const LSR_DR: u8 = 0x01; // Data Ready
const LSR_THRE: u8 = 0x20; // THR Empty

/// UART IRQ number on QEMU virt
const UART_IRQ: u32 = 10;

// ============================================================================
// SPSC Ring Buffer for RX
// ============================================================================

/// Ring buffer size (power of 2 for cheap modulo)
const UART_RX_BUF_SIZE: usize = 1024;
const UART_RX_BUF_MASK: usize = UART_RX_BUF_SIZE - 1;

struct UartRxBuf {
    data: core::cell::UnsafeCell<[u8; UART_RX_BUF_SIZE]>,
    /// Consumer index (read path)
    head: AtomicUsize,
    /// Producer index (IRQ handler)
    tail: AtomicUsize,
}

// SAFETY: SPSC pattern — single producer (IRQ), single consumer (task).
// Atomic head/tail provide the synchronization barrier.
unsafe impl Send for UartRxBuf {}
unsafe impl Sync for UartRxBuf {}

impl UartRxBuf {
    const fn new() -> Self {
        Self {
            data: core::cell::UnsafeCell::new([0; UART_RX_BUF_SIZE]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer: write byte. Called from IRQ context only (single producer).
    #[inline]
    fn put(&self, c: u8) {
        let tail = self.tail.load(Ordering::Relaxed);
        let next_tail = (tail + 1) & UART_RX_BUF_MASK;
        // Check if full
        if next_tail == self.head.load(Ordering::Acquire) {
            return; // Drop byte — buffer full
        }
        // SAFETY: tail is within bounds (0..UART_RX_BUF_SIZE); self.data is a static
        // UnsafeCell initialized with a fixed-size array; no concurrent mutation.
        unsafe {
            core::ptr::write_volatile(&mut (*self.data.get())[tail], c);
        }
        self.tail.store(next_tail, Ordering::Release);
    }

    /// Consumer: read byte. CAS loop — fork'd children sharing stdin are
    /// MULTIPLE consumers; the old load/read/store could duplicate a byte
    /// and skip the next (R27-1, the cold-boot first-byte loss family).
    #[inline]
    fn get(&self) -> Option<u8> {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            if head == self.tail.load(Ordering::Acquire) {
                return None; // Empty
            }
            // SAFETY: head is within bounds (0..UART_RX_BUF_SIZE); self.data is a static
            // UnsafeCell initialized with a fixed-size array.
            let c = unsafe { core::ptr::read_volatile(&(*self.data.get())[head]) };
            let next = (head + 1) & UART_RX_BUF_MASK;
            match self.head.compare_exchange_weak(
                head,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(c),
                Err(h) => head = h, // another consumer advanced — retry
            }
        }
    }
}

/// Global UART RX ring buffer
static UART_RX_BUF: UartRxBuf = UartRxBuf::new();

/// Set once init_irq() has registered the RX IRQ handler. While clear,
/// task-context code may poll RBR directly (early boot); once set, the
/// IRQ handler is the ONLY context allowed to consume hardware RX —
/// a task-context RBR read racing the handler on another CPU pops bytes
/// out of order (getchar got 'c' while 'e' went to the ring →
/// "echo" arriving as "ceho") or duplicates the stale RBR value.
static UART_IRQ_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Rolling match position for the DFX "DUMP!" magic (RX IRQ context only).
static mut DUMP_MAGIC_POS: usize = 0;

/// Rolling match position for the serial Ctrl-Alt-Del sequence (RX IRQ
/// context only). The xterm/modifier encoding of Ctrl+Alt+Delete is
/// CSI 3;7~ (ESC [ 3 ; 7 ~): tilcode 3 = Delete, modifier 7 = 1 + Ctrl(4)
/// + Alt(2) — what a real terminal emulator sends for the combo. The
/// sequence stays in the RX ring (a raw-mode reader just sees unknown
/// escape bytes); only the CAD latch side effect fires.
static mut CAD_SEQ_POS: usize = 0;
/// Ctrl-Alt-Del over the serial console: ESC [ 3 ; 7 ~ (xterm CSI-u style
/// modifier encoding — see CAD_SEQ_POS).
const CAD_MAGIC: &[u8; 6] = b"\x1b[3;7~";

/// Wait queue for blocking reads — readers sleep here when buffer is empty.
static UART_READ_WAITQ: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

// ============================================================================
// UART base address
// ============================================================================

#[cfg(feature = "aarch64")]
const UART0_BASE: usize = 0x0900_0000;

#[cfg(feature = "x86_64")]
const COM1_BASE: u16 = 0x3f8;

#[cfg(feature = "riscv64")]
fn get_uart_base() -> usize {
    uart_virt_addr()
}

// ============================================================================
// UART driver
// ============================================================================

pub struct Uart;

impl Uart {
    pub const fn new() -> Self {
        Self
    }

    /// Write single character to UART
    #[inline(never)]
    pub fn putc(&self, c: u8) {
        #[cfg(feature = "aarch64")]
        // SAFETY: UART0_BASE is a valid MMIO address for the UART data register.
        unsafe {
            let addr = UART0_BASE + 0x00;
            asm!(
                "str w1, [x0]",
                in("x0") addr,
                in("w1") c as u32,
                options(nostack, nomem)
            );
        }

        #[cfg(feature = "riscv64")]
        // SAFETY: get_uart_base() returns a valid MMIO address for the UART data register.
        unsafe {
            let addr = get_uart_base();
            asm!(
                "sb t1, 0(a0)",
                in("a0") addr,
                in("t1") c,
                options(nostack, nomem)
            );
        }

        #[cfg(feature = "x86_64")]
        {
            // Poll LSR bit 5 (THR Empty) before writing THR — the 8250
            // drops characters written while the shift register is busy.
            while read_reg(COM1_BASE, UART_LSR) & LSR_THRE == 0 {
                core::hint::spin_loop();
            }
            // SAFETY: COM1 THR is the standard 8250 data port.
            unsafe { write_reg(COM1_BASE, UART_THR, c) };
        }
    }
}

/// Global UART console (protected by spinlock, SMP safe)
static UART: Spinlock<Uart> = Spinlock::new(Uart::new());

// ============================================================================
// Initialization
// ============================================================================

/// Early console initialization — basic MMIO setup, enable FIFOs.
///
/// Called before PLIC init. Safe to use for polling putchar/getchar.
#[cfg(feature = "riscv64")]
pub fn early_init() {
    // Enable UART FIFOs — QEMU virt does NOT pre-enable them.
    // Without FIFOs, the UART has only a 1-byte receive buffer,
    // causing overrun errors and data loss.
    // Use trigger level 1 (default) for lowest latency — every
    // character generates an interrupt. QEMU does not implement
    // the character timeout interrupt, so higher trigger levels
    // can lose short inputs.
    let base = get_uart_base();
    unsafe {
        // RESCUE any byte already sitting in RBR before the FIFO switch.
        //
        // Enabling the FIFOs (FCR bit 0, 0->1) FORCE-CLEARS the receiver:
        // QEMU's serial.c turns the enable-flag change into an RX-FIFO
        // reset (RFR), and a real 16550A behaves the same. With the FIFO
        // still off at most ONE byte can be pending in RBR (QEMU's
        // chardev back-pressures the rest into the host socket), so
        // without this drain the FIRST character of the first serial
        // input after boot is destroyed exactly here ("ls" -> "s",
        // "for i in..." -> "or i in..."). Read it into the kernel ring
        // so the first reader still gets it — the byte ordering is
        // preserved (ring first, then the FIFO bytes drained later).
        //
        // Producer-safety: at this point IRQs are off, the UART RX
        // interrupt is not armed, and only the boot hart runs — the
        // ring's single-producer invariant holds.
        if read_reg(base, UART_LSR) & LSR_DR != 0 {
            UART_RX_BUF.put(read_reg(base, UART_RBR));
        }
        write_reg(base, UART_FCR, FCR_ENABLE_FIFO);
    }
}

#[cfg(feature = "x86_64")]
pub fn early_init() {
    // Standard 8250 bring-up on COM1: program the divisor latch (115200
    // baud), 8N1, enable+clear FIFOs, and raise DTR/RTS/OUT2 so the
    // serial link is live. Polling TX/RX only — no IRQ wiring here.
    // SAFETY: COM1 ports 0x3f8..0x3ff are the canonical 8250 range.
    unsafe {
        write_reg(COM1_BASE, UART_IER, 0x00); // mask interrupts for now
        write_reg(COM1_BASE, UART_LCR, 0x80); // DLAB on
        write_reg(COM1_BASE, 0x00, 0x01); // divisor low = 1 (115200)
        write_reg(COM1_BASE, 0x01, 0x00); // divisor high
        write_reg(COM1_BASE, UART_LCR, 0x03); // 8N1, DLAB off
        write_reg(COM1_BASE, UART_FCR, FCR_ENABLE_FIFO | FCR_CLEAR_RX | FCR_CLEAR_TX);
        write_reg(COM1_BASE, UART_MCR, 0x0b); // DTR | RTS | OUT2
    }
}

#[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
pub fn early_init() {}

/// Legacy init — forwards to early_init for backward compatibility.
pub fn init() {
    early_init();
}

/// Late initialization — enable RX interrupt and register IRQ handler.
///
/// Must be called after PLIC and IRQ framework are initialized.
#[cfg(feature = "riscv64")]
pub fn init_irq() {
    let base = get_uart_base();
    // SAFETY: base is a valid UART MMIO base address; the register accesses
    // below are safe in this single-hart, IRQs-off boot context.
    unsafe {
        // Drain bytes that accumulated in the 16-byte RX FIFO between
        // early_init() and now (the RX IRQ was not armed yet, so nothing
        // else moved them) into the ring. Without this, input sent during
        // that window is capped by the FIFO depth and overrun-lost beyond
        // 16 bytes — the same cold-boot first-command family as the
        // early_init() rescue above. External IRQs are still masked at
        // this point on every hart (SEIE is enabled later, secondaries
        // are not up yet), so the drain cannot race the IRQ handler.
        while read_reg(base, UART_LSR) & LSR_DR != 0 {
            UART_RX_BUF.put(read_reg(base, UART_RBR));
        }

        // Enable RX data available interrupt
        write_reg(base, UART_IER, IER_RX_ENABLE);

        // Read LSR to clear any pending error flags (OE, PE, FE, BI).
        let _lsr = read_reg(base, UART_LSR);
    }

    // Complete any stale PLIC claims for UART IRQ on all harts.
    for hart in 0..crate::config::MAX_CPUS {
        if let Some(claimed) = crate::drivers::intc::plic::claim(hart) {
            crate::drivers::intc::plic::complete(hart, claimed);
        }
    }

    // Register UART IRQ handler. From here on the handler owns hardware RX:
    // arm the flag only on success so a failed registration keeps the
    // (safe) task-context polling fallback.
    if crate::interrupt::request_irq(
        UART_IRQ,
        uart_irq_handler,
        0, // Not shared
        "UART",
        0,
    ).is_ok() {
        UART_IRQ_ARMED.store(true, Ordering::Release);
    }
}

/// x86_64: interrupt-driven RX on PIC line 4 (COM1). Same contract as the
/// riscv64 twin — once armed, the IRQ handler is the only context that
/// consumes hardware RX; task-context readers go through the ring.
#[cfg(feature = "x86_64")]
pub fn init_irq() {
    fn uart4_irq_line() {
        let mut received = 0usize;
        while read_reg(COM1_BASE, UART_LSR) & LSR_DR != 0 {
            // SAFETY: COM1 RBR read consumes the byte; the handler owns
            // hardware RX (see UART_IRQ_ARMED).
            let c = unsafe { read_reg(COM1_BASE, UART_RBR) };
            UART_RX_BUF.put(c);
            tty_isig_record(c);
            // DUMP!/Ctrl-Alt-Del magic matchers (riscv64 twin parity —
            // the x86 handler used to lack both, costing a whole debug
            // session on the INTx-storm hang: no diagnostic input path
            // exists when every task is starved).
            uart_rx_debug_magic(c);
            received += 1;
        }
        if received > 0 {
            UART_READ_WAITQ.wake_up_one();
        }
    }

    if crate::arch::trap::request_irq_line(4, uart4_irq_line) {
        // SAFETY: COM1 IER is the standard 8250 interrupt-enable port.
        unsafe { write_reg(COM1_BASE, UART_IER, IER_RX_ENABLE) };
        UART_IRQ_ARMED.store(true, Ordering::Release);
    }
}

#[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
pub fn init_irq() {}

// ============================================================================
// UART register helpers
// ============================================================================

#[cfg(feature = "riscv64")]
unsafe fn write_reg(base: usize, offset: usize, val: u8) {
    asm!(
        "sb t1, 0(a0)",
        in("a0") base + offset,
        in("t1") val,
        options(nostack, nomem)
    );
}

#[cfg(feature = "riscv64")]
unsafe fn read_reg(base: usize, offset: usize) -> u8 {
    let val: u8;
    asm!(
        "lb t0, 0(a0)",
        in("a0") base + offset,
        out("t0") val,
        options(nostack)
    );
    val
}

#[cfg(feature = "x86_64")]
unsafe fn write_reg(base: u16, offset: usize, val: u8) {
    crate::arch::cpu::outb(base + offset as u16, val);
}

#[cfg(feature = "x86_64")]
fn read_reg(base: u16, offset: usize) -> u8 {
    // SAFETY: `base + offset` is a valid 8250 register port.
    unsafe { crate::arch::cpu::inb(base + offset as u16) }
}

// ============================================================================
// UART IRQ handler
// ============================================================================

/// RX-byte debug magics shared by both arch UART RX paths.
///
/// - "DUMP!" (armed by `dfx=taskdump`): the RX interrupt is the only code
///   guaranteed to still run when every task is wedged (silent-hang form —
///   no spinlock to trip the deadlock watchdog), which is exactly when a
///   task snapshot is needed. Rolling-window match so the sequence may
///   sit anywhere in the byte stream.
/// - Serial Ctrl-Alt-Del (always armed — C.A.D is a standard console
///   feature, not a debug switch): only latches an atomic; the actual
///   signal/cascade runs in task context via cad_deliver_pending() (the
///   ISIG rule: never walk the pid hash from the RX IRQ).
fn uart_rx_debug_magic(c: u8) {
    // SAFETY: RX-IRQ context only (single consumer), matching the riscv64
    // original these matchers were factored out of.
    unsafe {
        // DFX taskdump magic trigger.
        if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::TaskDumpKey) {
            const MAGIC: &[u8; 5] = b"DUMP!";
            DUMP_MAGIC_POS = if DUMP_MAGIC_POS < MAGIC.len() && c == MAGIC[DUMP_MAGIC_POS] {
                DUMP_MAGIC_POS + 1
            } else if c == MAGIC[0] {
                1
            } else {
                0
            };
            if DUMP_MAGIC_POS == MAGIC.len() {
                DUMP_MAGIC_POS = 0;
                crate::dfx::taskdump::dump_all_tasks("uart-magic");
            }
        }

        CAD_SEQ_POS = if CAD_SEQ_POS < CAD_MAGIC.len() && c == CAD_MAGIC[CAD_SEQ_POS] {
            CAD_SEQ_POS + 1
        } else if c == CAD_MAGIC[0] {
            1
        } else {
            0
        };
        if CAD_SEQ_POS == CAD_MAGIC.len() {
            CAD_SEQ_POS = 0;
            crate::syscall::process::ctrl_alt_del_latch();
        }
    }
}

/// UART interrupt handler — drain hardware FIFO into ring buffer.
fn uart_irq_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    #[cfg(feature = "riscv64")]
    {
        let base = get_uart_base();
        let mut chars_received: usize = 0;

                // SAFETY: base is a valid UART MMIO base address; reading IIR/LSR/RBR registers
                // is safe in IRQ handler context.
                unsafe {
                    // Check IIR to confirm interrupt source
                    let iir = read_reg(base, UART_IIR);
                    // Bit 0 = 0 means interrupt pending
                    if iir & 0x01 != 0 {
                        return crate::interrupt::IrqReturn::None;
                    }

                    // Drain hardware FIFO — read while Data Ready
                    while read_reg(base, UART_LSR) & LSR_DR != 0 {
                        let c = read_reg(base, UART_RBR);
                        UART_RX_BUF.put(c);
                        chars_received += 1;

                        // ^C interrupt path (review批次8): record ISIG characters
                        // the moment they arrive instead of waiting for a reader to
                        // consume them — a busy foreground task (not blocked in
                        // read) must still be interruptible. The byte STAYS in the
                        // ring buffer; the signal itself is delivered in TASK
                        // context (check_and_deliver_signals / process_input):
                        // send_signal_to_pgid walks the pid hash under bucket
                        // spinlocks, and the interrupted task may itself hold one —
                        // sending from the IRQ here could self-deadlock the CPU.
                        tty_isig_record(c);

                        // DUMP!/CAD magic matchers (shared with the x86_64 RX
                        // path — see uart_rx_debug_magic).
                        uart_rx_debug_magic(c);
                    }
                }

        if chars_received > 0 {
            UART_READ_WAITQ.wake_up_one();
        }
    }

    crate::interrupt::IrqReturn::Handled
}

/// Pending ISIG signal recorded by the RX IRQ (producer) and delivered in
/// task context by tty_isig_deliver_pending() (consumer). 0 = none.
static PENDING_ISIG_SIGNO: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

/// Last ISIG signal delivered and when (jiffies) — the de-duplication
/// window that keeps process_input() from double-sending a signal for the
/// same byte the IRQ-recorded path already delivered.
static LAST_ISIG_SIGNO: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
static LAST_ISIG_JIFFY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// IRQ-context half of the ^C path: record an ISIG character (INTR ^C /
/// QUIT ^\ / SUSP ^Z) for task-context delivery. Never sends directly.
fn tty_isig_record(c: u8) {
    let lflag = crate::syscall::io::tty_get_lflag();
    const L_ISIG: u32 = 0x0001;
    if lflag & L_ISIG == 0 {
        return;
    }
    let signo = match c {
        0x03 => crate::signal::Signal::SIGINT as i32,
        0x1a => crate::signal::Signal::SIGTSTP as i32,
        0x1c => crate::signal::Signal::SIGQUIT as i32,
        _ => return,
    };
    // Last writer wins (rapid double ^C is redundant).
    PENDING_ISIG_SIGNO.store(signo, core::sync::atomic::Ordering::Release);
}

/// Deliver any ISIG the RX IRQ recorded. Called in TASK context from
/// check_and_deliver_signals (every return to user) — this is what makes
/// ^C interrupt a busy foreground task that is not blocked in read().
pub fn tty_isig_deliver_pending() {
    let signo = PENDING_ISIG_SIGNO.swap(0, core::sync::atomic::Ordering::AcqRel);
    if signo != 0 {
        tty_isig_deliver(signo);
    }
}

/// Send an ISIG signal to the tty foreground process group (task context
/// only — walks the pid hash). De-duplicated across delivery paths.
fn tty_isig_deliver(signo: i32) {
    // De-dup window: the same byte can traverse both the recorded-pending
    // path and process_input()'s consume-time send.
    let now = crate::drivers::timer::get_jiffies();
    let last_s = LAST_ISIG_SIGNO.load(core::sync::atomic::Ordering::Acquire);
    let last_j = LAST_ISIG_JIFFY.load(core::sync::atomic::Ordering::Acquire);
    if last_s == signo && now.saturating_sub(last_j) < 4 {
        return;
    }
    LAST_ISIG_SIGNO.store(signo, core::sync::atomic::Ordering::Release);
    LAST_ISIG_JIFFY.store(now, core::sync::atomic::Ordering::Release);

    // Target the tty foreground process group; fall back to the caller's.
    let mut pgid = crate::syscall::io::tty_get_fg_pgrp();
    if pgid == 0 {
        pgid = crate::process::current_pgid();
    }
    if pgid != 0 {
        crate::signal::send_signal_to_pgid(pgid, signo);
    }
}

// ============================================================================
// Public API — output
// ============================================================================

/// Write single character (SMP safe, IRQ safe)
pub fn putchar(c: u8) {
    // Use lock_irqsave: putchar may be called from interrupt context
    // (panic from IRQ, oops, etc.). A plain lock() would self-deadlock
    // if an interrupt fires while UART lock is held on the same CPU.
    let uart = UART.lock_irqsave();
    uart.putc(c);
}

/// Write string (SMP safe, IRQ safe, acquire lock only once)
pub fn puts(s: &str) {
    let uart = UART.lock_irqsave();
    for b in s.bytes() {
        uart.putc(b);
    }
}

/// Acquire UART lock (for batch output, IRQ safe)
pub fn lock() -> SpinlockIrqGuard<'static, Uart> {
    UART.lock_irqsave()
}

/// Interrupt-safe character output (no lock)
pub fn putchar_no_lock(c: u8) {
    let uart = Uart::new();
    uart.putc(c);
}

/// Interrupt-safe string output (no lock)
pub fn puts_no_lock(s: &str) {
    let uart = Uart::new();
    for b in s.bytes() {
        uart.putc(b);
    }
}

// ============================================================================
// Public API — input
// ============================================================================

/// Check if UART has data ready to read (non-destructive).
/// Used by poll() to check for readable data.
#[cfg(any(feature = "riscv64", feature = "x86_64"))]
pub fn uart_data_ready() -> bool {
    uart_has_data()
}

/// Read single character (non-blocking).
///
/// Returns Some(c) if data is available (from ring buffer or hardware),
/// otherwise None. Handles ISIG and echo processing.
/// Task-context bridge: move pending UART RX bytes into the tty console's
/// input queue (line discipline). The RX IRQ fills only the char_dev ring
/// (UART_RX_BUF) that exec-provided stdin (UART_OPS) drains; /dev/console
/// and /dev/ttyS0 (CONDEV_OPS) read the tty queue — without this bridge
/// those readers starved while bytes sat in the ring.
pub fn uart_rx_bridge_to_tty() {
    // Bounded so a flood cannot hold the CPU forever in task context.
    for _ in 0..256 {
        let head = UART_RX_BUF.head.load(Ordering::Relaxed);
        let tail = UART_RX_BUF.tail.load(Ordering::Acquire);
        if head == tail {
            break;
        }
        match UART_RX_BUF.get() {
            Some(c) => crate::fs::tty::console().receive_byte(c),
            None => break,
        }
    }
}

pub fn getchar() -> Option<u8> {
    #[cfg(feature = "riscv64")]
    {
        // Try ring buffer first (interrupt-driven path)
        let head = UART_RX_BUF.head.load(Ordering::Relaxed);
        let tail = UART_RX_BUF.tail.load(Ordering::Acquire);
        if head != tail {
            // R32-2: the emptiness check above is only a hint — another
            // consumer (fork'd child sharing stdin, the reason get() is a
            // CAS loop) can drain the byte between the check and the CAS.
            // get() then legitimately returns None; unwrap()'ing it was a
            // kernel panic on userspace input racing. Fall through to the
            // hardware poll instead.
            if let Some(c) = UART_RX_BUF.get() {
                return process_input(c);
            }
        }

        // Hardware polling fallback — ONLY before the RX IRQ is armed.
        // After init_irq(), the IRQ handler owns RBR: a concurrent
        // task-context RBR read steals bytes the handler is about to move
        // to the ring (out-of-order delivery, e.g. "echo" → "ceho") or
        // re-reads a stale RBR while the real byte waits in the ring.
        // Armed ⇒ empty ring ⇒ sleep on the wait queue; the handler will
        // fill the ring and wake us.
        if !UART_IRQ_ARMED.load(Ordering::Acquire) {
            let uart_base = get_uart_base();
            // SAFETY: uart_base is a valid UART MMIO base address; polling
            // LSR/RBR is safe while no interrupt handler competes for RBR.
            unsafe {
                let lsr = read_reg(uart_base, UART_LSR);
                if lsr & LSR_DR != 0 {
                    let c = read_reg(uart_base, UART_RBR);
                    return process_input(c);
                }
            }
        }
    }

    #[cfg(feature = "x86_64")]
    {
        // Ring first (IRQ-driven path), mirroring the riscv64 ordering.
        let head = UART_RX_BUF.head.load(Ordering::Relaxed);
        let tail = UART_RX_BUF.tail.load(Ordering::Acquire);
        if head != tail {
            if let Some(c) = UART_RX_BUF.get() {
                return process_input(c);
            }
        }

        // Hardware polling fallback — ONLY before the RX IRQ is armed
        // (same ownership rule as riscv64: once armed, RBR belongs to the
        // IRQ handler; a task-context read would steal or duplicate bytes).
        if !UART_IRQ_ARMED.load(Ordering::Acquire) {
            if read_reg(COM1_BASE, UART_LSR) & LSR_DR != 0 {
                // SAFETY: COM1 RBR read consumes the byte; no IRQ handler
                // competes for RBR while the flag is clear.
                let c = unsafe { read_reg(COM1_BASE, UART_RBR) };
                return process_input(c);
            }
        }
    }

    #[cfg(feature = "aarch64")]
    {
        // TODO: Implement aarch64 getchar
    }

    None
}

/// Wait queue accessor — for char_dev.rs blocking read
pub fn read_waitq() -> &'static crate::process::wait::WaitQueueHead {
    &UART_READ_WAITQ
}

/// Check if input data is available without consuming it.
/// Used as the wait condition for blocking reads.
#[cfg(feature = "riscv64")]
pub fn uart_has_data() -> bool {
    // Check ring buffer
    let head = UART_RX_BUF.head.load(Ordering::Relaxed);
    let tail = UART_RX_BUF.tail.load(Ordering::Acquire);
    if head != tail {
        return true;
    }
    // Check hardware — only while the RX IRQ is not armed. Once armed, a
    // DR=1 byte is about to be moved to the ring by the IRQ handler;
    // reporting it here would busy-spin readers that can no longer take
    // it from RBR themselves (see getchar()).
    if UART_IRQ_ARMED.load(Ordering::Acquire) {
        return false;
    }
    let uart_base = get_uart_base();
    // SAFETY: uart_base is a valid UART MMIO base address; reading LSR is safe.
    unsafe {
        let lsr = read_reg(uart_base, UART_LSR);
        lsr & LSR_DR != 0
    }
}

#[cfg(feature = "x86_64")]
pub fn uart_has_data() -> bool {
    // Ring first; LSR only while the RX IRQ is not armed (same ownership
    // rule as riscv64 — an armed DR=1 byte is about to move to the ring).
    let head = UART_RX_BUF.head.load(Ordering::Relaxed);
    let tail = UART_RX_BUF.tail.load(Ordering::Acquire);
    if head != tail {
        return true;
    }
    if UART_IRQ_ARMED.load(Ordering::Acquire) {
        return false;
    }
    read_reg(COM1_BASE, UART_LSR) & LSR_DR != 0
}

#[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
pub fn uart_has_data() -> bool {
    false
}

/// Process input character: handle ISIG, echo, and newline translation.
fn process_input(c: u8) -> Option<u8> {
    let echo_enabled = crate::syscall::io::tty_echo_enabled();

    // ISIG processing (signal generation characters)
    let lflag = crate::syscall::io::tty_get_lflag();
    const L_ISIG: u32 = 0x0001;
    if lflag & L_ISIG != 0 {
        let signo = match c {
            0x03 => crate::signal::Signal::SIGINT as i32,  // ^C
            0x1a => crate::signal::Signal::SIGTSTP as i32, // ^Z
            0x1c => crate::signal::Signal::SIGQUIT as i32, // ^\
            _ => 0,
        };
        if signo != 0 {
            if echo_enabled {
                match signo {
                    s if s == crate::signal::Signal::SIGINT as i32 => {
                        putchar(b'^'); putchar(b'C');
                    }
                    s if s == crate::signal::Signal::SIGTSTP as i32 => {
                        putchar(b'^'); putchar(b'Z');
                    }
                    _ => {
                        putchar(b'^'); putchar(b'\\');
                    }
                }
                putchar(b'\r');
                putchar(b'\n');
            }
            // Route through the de-duplicating deliverer: the RX IRQ may
            // already have delivered this byte via the recorded-pending
            // path (check_and_deliver_signals).
            tty_isig_deliver(signo);
            return Some(c);
        }
    }

    // Echo character only if ECHO flag is set
    if echo_enabled {
        if c == b'\n' || c == b'\r' {
            putchar(b'\r');
            putchar(b'\n');
            return Some(b'\n');
        } else if c == 127 || c == 8 {
            putchar(8);
            putchar(b' ');
            putchar(8);
            return Some(c);
        } else {
            putchar(c);
        }
    } else {
        if c == b'\r' {
            return Some(b'\n');
        }
    }

    Some(c)
}

// ============================================================================
// fmt::Write
// ============================================================================

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
        Ok(())
    }
}
