//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for PCI configuration space constants.
//! Copied from: kernel/src/drivers/pci/mod.rs

use proptest::prelude::*;

// Copied PCI config offsets
pub const VENDOR_ID: u8 = 0x00;
pub const DEVICE_ID: u8 = 0x02;
pub const COMMAND: u8 = 0x04;
pub const STATUS: u8 = 0x06;
pub const REVISION: u8 = 0x08;
pub const BAR0: u8 = 0x10;
pub const BAR1: u8 = 0x14;
pub const BAR2: u8 = 0x18;
pub const BAR3: u8 = 0x1C;
pub const BAR4: u8 = 0x20;
pub const BAR5: u8 = 0x24;
pub const INT_LINE: u8 = 0x3C;

// PCI command bits
pub mod command {
    pub const IO_SPACE: u16 = 0x0001;
    pub const MEMORY_SPACE: u16 = 0x0002;
    pub const BUS_MASTER: u16 = 0x0004;
}

// BAR type detection
pub fn is_io_bar(bar_raw: u32) -> bool { (bar_raw & 1) != 0 }
pub fn is_memory_bar(bar_raw: u32) -> bool { (bar_raw & 1) == 0 && bar_raw != 0 }
pub fn is_64bit_memory_bar(bar_raw: u32) -> bool { (bar_raw & 0x06) == 0x04 }

proptest! {
    #[test]
    fn test_config_offsets_increasing(_v in 0u8..1u8) {
        let offsets = [
            VENDOR_ID, DEVICE_ID, COMMAND, STATUS, REVISION,
            BAR0, BAR1, BAR2, BAR3, BAR4, BAR5, INT_LINE,
        ];
        for i in 0..offsets.len()-1 {
            assert!(offsets[i] < offsets[i+1], "offset {} ({:#x}) < offset {} ({:#x})",
                    i, offsets[i], i+1, offsets[i+1]);
        }
    }

    #[test]
    fn test_bar_offsets_sequential(_v in 0u8..1u8) {
        let bar_offsets = [BAR0, BAR1, BAR2, BAR3, BAR4, BAR5];
        for i in 1..bar_offsets.len() {
            assert_eq!(bar_offsets[i] - bar_offsets[i-1], 4);
        }
    }

    #[test]
    fn test_bar_index_to_offset(bar_index in 0usize..6usize) {
        assert_eq!(BAR0 + bar_index as u8 * 4, [BAR0, BAR1, BAR2, BAR3, BAR4, BAR5][bar_index]);
    }

    #[test]
    fn test_command_bits_distinct(_v in 0u8..1u8) {
        let bits = [command::IO_SPACE, command::MEMORY_SPACE, command::BUS_MASTER];
        for i in 0..bits.len() {
            for j in (i+1)..bits.len() {
                assert_eq!(bits[i] & bits[j], 0);
            }
        }
    }

    #[test]
    fn test_command_bits_powers_of_two(_v in 0u8..1u8) {
        assert_eq!(command::IO_SPACE, 1 << 0);
        assert_eq!(command::MEMORY_SPACE, 1 << 1);
        assert_eq!(command::BUS_MASTER, 1 << 2);
    }

    #[test]
    fn test_io_bar_detection(bar_raw in 0u32..) {
        // I/O BAR has bit 0 set
        let is_io = is_io_bar(bar_raw);
        if bar_raw & 1 != 0 {
            assert!(is_io);
        } else {
            assert!(!is_io);
        }
    }

    #[test]
    fn test_memory_bar_detection(bar_raw in 0u32..) {
        let is_mem = is_memory_bar(bar_raw);
        if bar_raw == 0 {
            assert!(!is_mem);
        } else if (bar_raw & 1) == 0 {
            assert!(is_mem);
        } else {
            assert!(!is_mem);
        }
    }

    #[test]
    fn test_64bit_bar_detection(bar_raw in 0u32..) {
        let is_64 = is_64bit_memory_bar(bar_raw);
        if (bar_raw & 0x06) == 0x04 {
            assert!(is_64);
        } else {
            assert!(!is_64);
        }
    }
}
