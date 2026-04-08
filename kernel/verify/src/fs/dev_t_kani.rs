//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for device number packing/unpacking.
//!
//! Types copied from: kernel/src/fs/dev_t.rs

#![cfg(kani)]

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DevNo {
    pub major: u32,
    pub minor: u32,
}

impl DevNo {
    pub const fn new(major: u32, minor: u32) -> Self { Self { major, minor } }
    pub const fn from_u64(v: u64) -> Self {
        Self { major: (v >> 32) as u32, minor: v as u32 }
    }
    pub const fn to_u64(&self) -> u64 {
        ((self.major as u64) << 32) | (self.minor as u64)
    }
}

pub const MEM_MAJOR: u32 = 1;

/// INV-DEV-K1: to_u64/from_u64 roundtrip for all u32 values.
#[kani::proof]
fn verify_devno_roundtrip() {
    let major: u32 = kani::any();
    let minor: u32 = kani::any();
    let dev = DevNo::new(major, minor);
    let v = dev.to_u64();
    let dev2 = DevNo::from_u64(v);
    assert_eq!(dev2, dev);
}

/// INV-DEV-K2: major packed in upper 32 bits, minor in lower 32 bits.
#[kani::proof]
fn verify_major_minor_packing() {
    let major: u32 = kani::any();
    let minor: u32 = kani::any();
    let dev = DevNo::new(major, minor);
    let v = dev.to_u64();
    assert_eq!((v >> 32) as u32, major);
    assert_eq!(v as u32, minor);
}

/// INV-DEV-K3: from_u64(0) == from_u64(u64::MAX) edge cases.
#[kani::proof]
fn verify_edge_cases() {
    let zero = DevNo::from_u64(0);
    assert_eq!(zero.major, 0);
    assert_eq!(zero.minor, 0);

    let max = DevNo::from_u64(u64::MAX);
    assert_eq!(max.major, u32::MAX);
    assert_eq!(max.minor, u32::MAX);
}

/// INV-DEV-K4: Ord ordering matches (major, minor) tuple ordering.
#[kani::proof]
fn verify_ordering() {
    let m1: u32 = kani::any();
    let m2: u32 = kani::any();
    let min1: u32 = kani::any();
    let min2: u32 = kani::any();
    let d1 = DevNo::new(m1, min1);
    let d2 = DevNo::new(m2, min2);
    assert_eq!(d1.cmp(&d2), (m1, min1).cmp(&(m2, min2)));
}
