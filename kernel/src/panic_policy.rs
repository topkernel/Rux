//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The panic policy — Linux's `panic=` kernel parameter.
//!
//! Linux's `panic=` decides what the kernel does after a panic:
//!
//! ```text
//! panic=seconds   reboot that many seconds after the panic
//! panic=o         reboot immediately (panic=O is the same)
//! panic=immediate reboot immediately
//! ```
//!
//! With no `panic=` on the command line the kernel just halts — that is
//! Linux's default and Rux's behaviour until now: the panic handler
//! stops the other CPUs, flushes the persistent log and parks in a
//! `wfi` loop forever.
//!
//! The reboot path deliberately does NOT sync the filesystems: Linux's
//! panic path reboots without touching the disks, because the panic may
//! well be the storage stack itself.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::string::String;

/// What the kernel does after a panic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanicPolicy {
    /// Stay halted (the default, and what `panic=0` means).
    Halt,
    /// Reboot this many seconds after the panic.
    RebootAfter(u32),
    /// Reboot at once.
    RebootNow,
}

impl PanicPolicy {
    /// Parse the value of the `panic=` parameter.
    ///
    /// * no value or an empty one — halt;
    /// * `0` — halt (the sysctl of the same name documents 0 as
    ///   "wait forever");
    /// * a number — reboot that many seconds later;
    /// * `o`, `O` or `immediate` — reboot at once;
    /// * anything else — halt (an unparsable value leaves the default).
    pub fn parse(value: &str) -> PanicPolicy {
        let value = value.trim();
        if value.is_empty() {
            return PanicPolicy::Halt;
        }
        if value.eq_ignore_ascii_case("o") || value.eq_ignore_ascii_case("immediate") {
            return PanicPolicy::RebootNow;
        }
        match value.parse::<u32>() {
            Ok(0) => PanicPolicy::Halt,
            Ok(seconds) => PanicPolicy::RebootAfter(seconds),
            Err(_) => PanicPolicy::Halt,
        }
    }

    /// The policy in force on this boot.
    pub fn current() -> PanicPolicy {
        match crate::cmdline::get_param("panic") {
            Some(value) => PanicPolicy::parse(&value),
            None => PanicPolicy::Halt,
        }
    }
}

/// Run the policy. Never returns.
pub fn apply() -> ! {
    match PanicPolicy::current() {
        PanicPolicy::Halt => loop {
            crate::arch::cpu::wfi();
        },
        PanicPolicy::RebootNow => {
            crate::println!("panic: rebooting (panic= immediate)");
            crate::syscall::process::panic_reboot();
        }
        PanicPolicy::RebootAfter(seconds) => {
            crate::println!("panic: rebooting in {}s (panic=)", seconds);
            // Wait for the deadline. The jiffies counter only advances
            // while the timer interrupt runs, so a panic inside an
            // interrupt context cannot honour the timeout — it parks
            // like the default does.
            let deadline = crate::drivers::timer::get_jiffies()
                + (seconds as u64) * crate::config::KERNEL_HZ as u64;
            while crate::drivers::timer::get_jiffies() < deadline {
                crate::arch::cpu::wfi();
            }
            crate::syscall::process::panic_reboot();
        }
    }
}
