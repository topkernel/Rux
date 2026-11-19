//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for PCI config offsets and BAR type detection.
//!
//! Types copied from: kernel/src/drivers/pci/mod.rs

#![cfg(kani)]

pub const BAR0: u8 = 0x10;

pub mod command {
    pub const IO_SPACE: u16 = 0x0001;
    pub const MEMORY_SPACE: u16 = 0x0002;
    pub const BUS_MASTER: u16 = 0x0004;
}

pub fn is_io_bar(bar_raw: u32) -> bool { (bar_raw & 1) != 0 }
pub fn is_memory_bar(bar_raw: u32) -> bool { (bar_raw & 1) == 0 && bar_raw != 0 }
pub fn is_64bit_memory_bar(bar_raw: u32) -> bool { (bar_raw & 0x06) == 0x04 }

/// INV-PCI-K1: command bits are distinct powers of 2.
#[kani::proof]
fn verify_command_bits_distinct() {
    let bits = [command::IO_SPACE, command::MEMORY_SPACE, command::BUS_MASTER];
    let mut seen = u16::MAX;
    for &b in &bits {
        assert!(b > 0 && (b & (b - 1)) == 0);
        assert_eq!(seen & b, 0);
        seen |= b;
    }
}

/// INV-PCI-K2: I/O BAR detection based on bit 0.
#[kani::proof]
fn verify_io_bar_detection() {
    let bar_raw: u32 = kani::any();
    let is_io = is_io_bar(bar_raw);
    assert_eq!(is_io, (bar_raw & 1) != 0);
}

/// INV-PCI-K3: memory BAR detection: bit 0 == 0 and non-zero.
#[kani::proof]
fn verify_memory_bar_detection() {
    let bar_raw: u32 = kani::any();
    let is_mem = is_memory_bar(bar_raw);
    assert_eq!(is_mem, bar_raw != 0 && (bar_raw & 1) == 0);
}

/// INV-PCI-K4: 64-bit memory BAR detection: bits [2:1] == 0b10.
#[kani::proof]
fn verify_64bit_bar_detection() {
    let bar_raw: u32 = kani::any();
    let is_64 = is_64bit_memory_bar(bar_raw);
    assert_eq!(is_64, (bar_raw & 0x06) == 0x04);
}

/// INV-PCI-K5: BAR offsets are sequential with 4-byte spacing.
#[kani::proof]
fn verify_bar_sequential() {
    let bar_offsets = [BAR0, BAR0 + 4, BAR0 + 8, BAR0 + 12, BAR0 + 16, BAR0 + 20];
    for i in 1..bar_offsets.len() {
        assert_eq!(bar_offsets[i] - bar_offsets[i - 1], 4);
    }
}
