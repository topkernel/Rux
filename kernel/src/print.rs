//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel print macros
//!
//! print! and println! are convenience aliases for pr_info! (log level 6).
//! They route through printk, which stores to the ring buffer and
//! conditionally outputs to UART based on console_loglevel.

use core::fmt;
use crate::console;

/// Console struct for direct UART output (used by panic handler and early boot).
pub struct Console;

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Acquire lock only once, output entire string
        let uart = console::lock();
        for b in s.bytes() {
            if b == b'\n' {
                uart.putc(b'\r');
            }
            uart.putc(b);
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
