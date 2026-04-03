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
use crate::arch::riscv64::mm::fixmap::uart_virt_addr;

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
        unsafe {
            core::ptr::write_volatile(&mut (*self.data.get())[tail], c);
        }
        self.tail.store(next_tail, Ordering::Release);
    }

    /// Consumer: read byte. Called from task context only (single consumer).
    #[inline]
    fn get(&self) -> Option<u8> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None; // Empty
        }
        let c = unsafe { core::ptr::read_volatile(&(*self.data.get())[head]) };
        self.head.store((head + 1) & UART_RX_BUF_MASK, Ordering::Release);
        Some(c)
    }
}

/// Global UART RX ring buffer
static UART_RX_BUF: UartRxBuf = UartRxBuf::new();

/// Wait queue for blocking reads — readers sleep here when buffer is empty.
static UART_READ_WAITQ: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

// ============================================================================
// UART base address
// ============================================================================

#[cfg(feature = "aarch64")]
const UART0_BASE: usize = 0x0900_0000;

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
        unsafe {
            let addr = get_uart_base();
            asm!(
                "sb t1, 0(a0)",
                in("a0") addr,
                in("t1") c,
                options(nostack, nomem)
            );
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
    // QEMU virt's UART is pre-initialized with FIFOs enabled.
    // Do not touch UART registers here — just leave defaults.
    // Interrupt setup is done in init_irq() after PLIC is ready.
}

#[cfg(not(feature = "riscv64"))]
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
    unsafe {
        // Enable RX data available interrupt
        write_reg(base, UART_IER, IER_RX_ENABLE);
    }

    // Register UART IRQ handler
    crate::interrupt::request_irq(
        UART_IRQ,
        uart_irq_handler,
        0, // Not shared
        "UART",
        0,
    ).ok();

    crate::pr_debug!("console: UART IRQ {} registered (interrupt-driven RX)", UART_IRQ);
}

#[cfg(not(feature = "riscv64"))]
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

// ============================================================================
// UART IRQ handler
// ============================================================================

/// UART interrupt handler — drain hardware FIFO into ring buffer.
fn uart_irq_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    #[cfg(feature = "riscv64")]
    {
        let base = get_uart_base();
        let mut chars_received = false;

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
                chars_received = true;
            }
        }

        if chars_received {
            UART_READ_WAITQ.wake_up_one();
        }
    }

    crate::interrupt::IrqReturn::Handled
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
#[cfg(feature = "riscv64")]
pub fn uart_data_ready() -> bool {
    uart_has_data()
}

/// Read single character (non-blocking).
///
/// Returns Some(c) if data is available (from ring buffer or hardware),
/// otherwise None. Handles ISIG and echo processing.
pub fn getchar() -> Option<u8> {
    #[cfg(feature = "riscv64")]
    {
        // Try ring buffer first (interrupt-driven path)
        if let Some(c) = UART_RX_BUF.get() {
            return process_input(c);
        }

        // Fall back to hardware polling (for early boot or if IRQ not enabled)
        let uart_base = get_uart_base();
        unsafe {
            let lsr = read_reg(uart_base, UART_LSR);
            if lsr & LSR_DR != 0 {
                let c = read_reg(uart_base, UART_RBR);
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
    // Check hardware
    let uart_base = get_uart_base();
    unsafe {
        let lsr = read_reg(uart_base, UART_LSR);
        lsr & LSR_DR != 0
    }
}

#[cfg(not(feature = "riscv64"))]
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
        let pgid = crate::process::current_pgid();
        match c {
            0x03 => {  // ^C -> SIGINT
                if echo_enabled {
                    putchar(b'^');
                    putchar(b'C');
                    putchar(b'\r');
                    putchar(b'\n');
                }
                crate::signal::send_signal_to_pgid(
                    pgid,
                    crate::signal::Signal::SIGINT as i32,
                );
                return Some(c);
            }
            0x1a => {  // ^Z -> SIGTSTP
                if echo_enabled {
                    putchar(b'^');
                    putchar(b'Z');
                    putchar(b'\r');
                    putchar(b'\n');
                }
                crate::signal::send_signal_to_pgid(
                    pgid,
                    crate::signal::Signal::SIGTSTP as i32,
                );
                return Some(c);
            }
            0x1c => {  // ^\ -> SIGQUIT
                if echo_enabled {
                    putchar(b'^');
                    putchar(b'\\');
                    putchar(b'\r');
                    putchar(b'\n');
                }
                crate::signal::send_signal_to_pgid(
                    pgid,
                    crate::signal::Signal::SIGQUIT as i32,
                );
                return Some(c);
            }
            _ => {}
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
