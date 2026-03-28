//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
use core::fmt;
use core::arch::asm;
use spin::Mutex;

#[cfg(feature = "riscv64")]
use crate::arch::riscv64::mm::fixmap::uart_virt_addr;

// UART base address - selected by architecture
#[cfg(feature = "aarch64")]
const UART0_BASE: usize = 0x0900_0000;  // ARM PL011 UART

#[cfg(feature = "riscv64")]
/// Get UART base address (uses fixmap virtual address after MMU is initialized)
fn get_uart_base() -> usize {
    uart_virt_addr()
}

/// Simple UART driver - dedicated for QEMU virt
pub struct Uart;

impl Uart {
    pub const fn new() -> Self {
        Self
    }

    /// Write single character to UART (use inline assembly for correctness)
    #[inline(never)]
    pub fn putc(&self, c: u8) {
        #[cfg(feature = "aarch64")]
        unsafe {
            let addr = UART0_BASE + 0x00;  // UART_DR offset
            asm!(
                "str w1, [x0]",
                in("x0") addr,
                in("w1") c as u32,
                options(nostack, nomem)
            );
        }

        #[cfg(feature = "riscv64")]
        unsafe {
            let addr = get_uart_base();  // Use fixmap virtual address
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
static UART: Mutex<Uart> = Mutex::new(Uart::new());

/// Initialize console (QEMU virt UART is pre-initialized, no operation needed)
pub fn init() {
    // QEMU virt's UART is already pre-initialized, no operation needed
}

/// Write single character (SMP safe)
pub fn putchar(c: u8) {
    // Use spinlock to protect UART access
    let uart = UART.lock();
    uart.putc(c);
}

/// Write string (SMP safe, acquire lock only once)
pub fn puts(s: &str) {
    let uart = UART.lock();
    for b in s.bytes() {
        uart.putc(b);
    }
}

/// Acquire UART lock (for batch output)
///
/// Returns lock guard, caller can safely call putc within its scope
pub fn lock() -> spin::MutexGuard<'static, Uart> {
    UART.lock()
}

/// Interrupt-safe character output (no lock, write directly to UART)
///
/// Only use in interrupt handlers
/// Note: If multiple CPUs call this simultaneously, output may interleave
pub fn putchar_no_lock(c: u8) {
    let uart = Uart::new();
    uart.putc(c);
}

/// Interrupt-safe string output (no lock)
///
/// Only use in interrupt handlers
pub fn puts_no_lock(s: &str) {
    let uart = Uart::new();
    for b in s.bytes() {
        uart.putc(b);
    }
}

/// Check if UART has data ready to read (non-destructive)
#[cfg(feature = "riscv64")]
pub fn uart_data_ready() -> bool {
    const UART_LSR: usize = 5;
    let uart_base = get_uart_base();
    unsafe {
        let lsr_addr = uart_base + UART_LSR;
        let lsr: u8;
        asm!(
            "lb t0, 0(a0)",
            in("a0") lsr_addr,
            out("t0") lsr,
            options(nostack)
        );
        lsr & 1 != 0
    }
}

/// Read single character (non-blocking)
/// Returns Some(c) if data is available, otherwise None
///
/// Echo behavior depends on terminal settings (ECHO flag)
pub fn getchar() -> Option<u8> {    #[cfg(feature = "riscv64")]
    {
        const UART_LSR: usize = 5;  // Line Status Register

        // Get UART base address (use fixmap virtual address)
        let uart_base = get_uart_base();

        // Check if echo is enabled
        let echo_enabled = crate::syscall::io::tty_echo_enabled();

        unsafe {
            // Check LSR bit 0 (DR - Data Ready)
            let lsr_addr = uart_base + UART_LSR;
            let lsr: u8;
            asm!(
                "lb t0, 0(a0)",
                in("a0") lsr_addr,
                out("t0") lsr,
                options(nostack)
            );

            if lsr & 1 == 1 {
                // Data available, read from RBR
                let c: u8;
                asm!(
                    "lb t0, 0(a0)",
                    in("a0") uart_base,
                    out("t0") c,
                    options(nostack)
                );

                // Echo character only if ECHO flag is set
                if echo_enabled {
                    if c == b'\n' || c == b'\r' {
                        // Enter key: echo \r\n, but return \n to program
                        putchar(b'\r');
                        putchar(b'\n');
                        return Some(b'\n');
                    } else if c == 127 || c == 8 {
                        // Backspace/Delete key
                        putchar(8);      // backspace
                        putchar(b' ');   // space to overwrite
                        putchar(8);      // backspace again
                        return Some(c);  // return original character for program to handle
                    } else {
                        putchar(c);
                    }
                } else {
                    // No echo: just handle newline translation
                    if c == b'\r' {
                        return Some(b'\n');
                    }
                }

                Some(c)
            } else {
                None
            }
        }
    }

    #[cfg(feature = "aarch64")]
    {
        // TODO: Implement aarch64 getchar
        None
    }

    #[cfg(not(any(feature = "riscv64", feature = "aarch64")))]
    {
        None
    }
}

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
