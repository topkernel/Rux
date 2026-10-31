//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel print macros
//!
//! print! and println! are convenience aliases for pr_info! (log level 6).
//! They route through printk, which stores to the ring buffer and
//! conditionally outputs to UART based on console_loglevel.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::fmt;
use crate::console;

/// Console struct for direct UART output (used by panic handler and early boot).
pub struct Console;

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Acquire lock only once; batched TX with \n expansion staged
        // into a stack buffer (perf-console: the old per-byte loop paid
        // per-byte 16550 register traffic — this writer carries panic
        // output, which can be thousands of bytes).
        let uart = console::lock();
        let mut buf = [0u8; 256];
        let mut n = 0usize;
        for b in s.bytes() {
            if b == b'\n' {
                if n == buf.len() {
                    uart.puts_batch(&buf);
                    n = 0;
                }
                buf[n] = b'\r';
                n += 1;
            }
            if n == buf.len() {
                uart.puts_batch(&buf);
                n = 0;
            }
            buf[n] = b;
            n += 1;
        }
        if n > 0 {
            uart.puts_batch(&buf[..n]);
        }
        Ok(())
    }
}

/// Print to kernel log at KERN_INFO level (no newline).
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        $crate::printk::printk(
            $crate::printk::loglevel::KERN_INFO,
            format_args!($($arg)*)
        )
    });
}

/// Print to kernel log at KERN_INFO level (with newline).
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln(
            $crate::printk::loglevel::KERN_INFO,
            format_args!($($arg)*)
        )
    });
}

/// Debug println - only prints in debug mode
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => ({
        $crate::printk::printk_ln(
            $crate::printk::loglevel::KERN_DEBUG,
            format_args!($($arg)*)
        )
    });
}

/// Release version - debug_println does nothing
#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => ({
        // Empty in release mode
    });
}
