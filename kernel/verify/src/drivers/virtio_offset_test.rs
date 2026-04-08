//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for VirtIO PCI register offsets and status bits.
//! Copied from: kernel/src/drivers/virtio/offset.rs

use proptest::prelude::*;

// Copied register offsets
pub const DEVICE_FEATURE_SELECT: u32 = 0;
pub const DEVICE_FEATURES: u32 = 4;
pub const DRIVER_FEATURE_SELECT: u32 = 8;
pub const DRIVER_FEATURES: u32 = 12;
pub const CONFIG_MSIX_VECTOR: u32 = 16;
pub const NUM_QUEUES: u32 = 18;
pub const DEVICE_STATUS: u32 = 20;
pub const CONFIG_GENERATION: u32 = 21;
pub const COMMON_CFG_QUEUE_SELECT: u32 = 22;
pub const COMMON_CFG_QUEUE_SIZE: u32 = 24;
pub const COMMON_CFG_QUEUE_MSIX_VECTOR: u32 = 26;
pub const COMMON_CFG_QUEUE_ENABLE: u32 = 28;
pub const COMMON_CFG_QUEUE_NOTIFY_OFF: u32 = 30;
pub const COMMON_CFG_QUEUE_DESC_LO: u32 = 32;
pub const COMMON_CFG_QUEUE_DESC_HI: u32 = 36;
pub const COMMON_CFG_QUEUE_DRIVER_LO: u32 = 40;
pub const COMMON_CFG_QUEUE_DRIVER_HI: u32 = 44;
pub const COMMON_CFG_QUEUE_DEVICE_LO: u32 = 48;
pub const COMMON_CFG_QUEUE_DEVICE_HI: u32 = 52;

// Status bits
pub mod status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}

proptest! {
    #[test]
    fn test_register_offsets_strictly_increasing(_v in 0u8..1u8) {
        let offsets = [
            DEVICE_FEATURE_SELECT, DEVICE_FEATURES, DRIVER_FEATURE_SELECT, DRIVER_FEATURES,
            CONFIG_MSIX_VECTOR, NUM_QUEUES, DEVICE_STATUS, CONFIG_GENERATION,
            COMMON_CFG_QUEUE_SELECT, COMMON_CFG_QUEUE_SIZE, COMMON_CFG_QUEUE_MSIX_VECTOR,
            COMMON_CFG_QUEUE_ENABLE, COMMON_CFG_QUEUE_NOTIFY_OFF,
            COMMON_CFG_QUEUE_DESC_LO, COMMON_CFG_QUEUE_DESC_HI,
            COMMON_CFG_QUEUE_DRIVER_LO, COMMON_CFG_QUEUE_DRIVER_HI,
            COMMON_CFG_QUEUE_DEVICE_LO, COMMON_CFG_QUEUE_DEVICE_HI,
        ];
        for i in 0..offsets.len()-1 {
            assert!(offsets[i] < offsets[i+1], "offset {} ({}) should be < offset {} ({})",
                    i, offsets[i], i+1, offsets[i+1]);
        }
    }

    #[test]
    fn test_queue_lo_hi_spacing(_v in 0u8..1u8) {
        // Each 64-bit register pair has LO at X and HI at X+4
        assert_eq!(COMMON_CFG_QUEUE_DESC_HI - COMMON_CFG_QUEUE_DESC_LO, 4);
        assert_eq!(COMMON_CFG_QUEUE_DRIVER_HI - COMMON_CFG_QUEUE_DRIVER_LO, 4);
        assert_eq!(COMMON_CFG_QUEUE_DEVICE_HI - COMMON_CFG_QUEUE_DEVICE_LO, 4);
    }

    #[test]
    fn test_status_bits_distinct(_v in 0u8..1u8) {
        let bits = [
            status::ACKNOWLEDGE, status::DRIVER, status::FAILED,
            status::FEATURES_OK, status::DRIVER_OK, status::DEVICE_NEEDS_RESET,
        ];
        for i in 0..bits.len() {
            for j in (i+1)..bits.len() {
                assert_eq!(bits[i] & bits[j], 0, "status bits {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_status_bits_powers_of_two(_v in 0u8..1u8) {
        let bits = [
            status::ACKNOWLEDGE, status::DRIVER, status::FAILED,
            status::FEATURES_OK, status::DRIVER_OK, status::DEVICE_NEEDS_RESET,
        ];
        for &b in &bits {
            assert!(b > 0 && (b & (b - 1)) == 0, "status bit {:#x} not power of two", b);
        }
    }

    #[test]
    fn test_bar_offset_arithmetic(bar_index in 0usize..6usize) {
        // PCI BAR offsets: BAR0=0x10, BAR1=0x14, ..., BAR5=0x24
        // BAR_offset = 0x10 + bar_index * 4
        let bar_offsets = [0x10u8, 0x14, 0x18, 0x1C, 0x20, 0x24];
        prop_assert!(bar_index < bar_offsets.len());
        assert_eq!(usize::from(bar_offsets[bar_index]), 0x10 + bar_index * 4);
    }

    #[test]
    fn test_num_queues_offset_2byte(_v in 0u8..1u8) {
        // NUM_QUEUES is 2-byte register (u16), offset should be 2-byte aligned
        assert_eq!(NUM_QUEUES % 2, 0);
        assert_eq!(NUM_QUEUES, 18); // follows 16-byte CONFIG_MSIX_VECTOR
    }

    #[test]
    fn test_config_generation_after_status(_v in 0u8..1u8) {
        // CONFIG_GENERATION immediately follows DEVICE_STATUS
        assert_eq!(CONFIG_GENERATION, DEVICE_STATUS + 1);
    }
}
