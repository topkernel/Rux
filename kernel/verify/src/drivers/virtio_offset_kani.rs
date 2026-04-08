//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for VirtIO register offsets and status bits.
//!
//! Types copied from: kernel/src/drivers/virtio/offset.rs

#![cfg(kani)]

pub const DEVICE_FEATURE_SELECT: u32 = 0;
pub const DEVICE_FEATURES: u32 = 4;
pub const DRIVER_FEATURE_SELECT: u32 = 8;
pub const DEVICE_STATUS: u32 = 20;
pub const COMMON_CFG_QUEUE_DESC_LO: u32 = 32;
pub const COMMON_CFG_QUEUE_DESC_HI: u32 = 36;
pub const COMMON_CFG_QUEUE_DRIVER_LO: u32 = 40;
pub const COMMON_CFG_QUEUE_DRIVER_HI: u32 = 44;

pub mod status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}

/// INV-VIRTIO-K1: status bits are distinct powers of 2.
#[kani::proof]
fn verify_status_bits_distinct() {
    let bits = [
        status::ACKNOWLEDGE, status::DRIVER, status::FAILED,
        status::FEATURES_OK, status::DRIVER_OK, status::DEVICE_NEEDS_RESET,
    ];
    let mut seen = 0u32;
    for &b in &bits {
        assert!(b > 0 && (b & (b - 1)) == 0, "not power of 2: {:#x}", b);
        assert_eq!(seen & b, 0, "overlap: {:#x}", b);
        seen |= b;
    }
}

/// INV-VIRTIO-K2: 64-bit register LO/HI pairs have 4-byte spacing.
#[kani::proof]
fn verify_lo_hi_spacing() {
    assert_eq!(COMMON_CFG_QUEUE_DESC_HI - COMMON_CFG_QUEUE_DESC_LO, 4);
    assert_eq!(COMMON_CFG_QUEUE_DRIVER_HI - COMMON_CFG_QUEUE_DRIVER_LO, 4);
}

/// INV-VIRTIO-K3: register offsets are strictly increasing.
#[kani::proof]
fn verify_offsets_increasing() {
    let offsets = [
        DEVICE_FEATURE_SELECT, DEVICE_FEATURES, DRIVER_FEATURE_SELECT,
        DEVICE_STATUS, COMMON_CFG_QUEUE_DESC_LO, COMMON_CFG_QUEUE_DESC_HI,
        COMMON_CFG_QUEUE_DRIVER_LO, COMMON_CFG_QUEUE_DRIVER_HI,
    ];
    for i in 0..offsets.len() - 1 {
        assert!(offsets[i] < offsets[i + 1]);
    }
}
