//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Device number packing/unpacking invariant tests.
//!
//! Types copied from: kernel/src/fs/dev_t.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/dev_t.rs
// ============================================================================

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DevNo {
    pub major: u32,
    pub minor: u32,
}

impl DevNo {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    pub const fn from_u64(v: u64) -> Self {
        Self {
            major: (v >> 32) as u32,
            minor: v as u32,
        }
    }

    pub const fn to_u64(&self) -> u64 {
        ((self.major as u64) << 32) | (self.minor as u64)
    }
}

pub const MEM_MAJOR: u32 = 1;
pub const TTY_MAJOR: u32 = 4;
pub const INPUT_MAJOR: u32 = 13;
pub const FB_MAJOR: u32 = 29;
pub const LP_MAJOR: u32 = 6;
pub const SCSI_DISK_MAJOR: u32 = 8;
pub const MTD_BLOCK_MAJOR: u32 = 31;
pub const IDE_DISK_MAJOR: u32 = 33;

pub const DEV_NULL: DevNo = DevNo::new(MEM_MAJOR, 3);
pub const DEV_ZERO: DevNo = DevNo::new(MEM_MAJOR, 5);
pub const DEV_RANDOM: DevNo = DevNo::new(MEM_MAJOR, 8);
pub const DEV_URANDOM: DevNo = DevNo::new(MEM_MAJOR, 9);

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-DEV-1: to_u64/from_u64 roundtrip
    #[test]
    fn test_roundtrip(
        major in 0u32..1000u32,
        minor in 0u32..1000u32,
    ) {
        let dev = DevNo::new(major, minor);
        let v = dev.to_u64();
        let dev2 = DevNo::from_u64(v);
        prop_assert_eq!(dev2, dev);
    }

    /// INV-DEV-2: from_u64(0) gives (0,0)
    #[test]
    fn test_zero(_v in 0u8..1u8) {
        let dev = DevNo::from_u64(0);
        prop_assert_eq!(dev.major, 0);
        prop_assert_eq!(dev.minor, 0);
    }

    /// INV-DEV-3: major packed in upper 32 bits
    #[test]
    fn test_major_upper(major in 1u32..1000u32) {
        let dev = DevNo::new(major, 0);
        let v = dev.to_u64();
        prop_assert_eq!((v >> 32) as u32, major);
        prop_assert_eq!(v as u32, 0);
    }

    /// INV-DEV-4: minor packed in lower 32 bits
    #[test]
    fn test_minor_lower(minor in 1u32..1000u32) {
        let dev = DevNo::new(0, minor);
        let v = dev.to_u64();
        prop_assert_eq!((v >> 32) as u32, 0);
        prop_assert_eq!(v as u32, minor);
    }

    /// INV-DEV-5: DEV_NULL has correct values
    #[test]
    fn test_dev_null(_v in 0u8..1u8) {
        prop_assert_eq!(DEV_NULL.major, MEM_MAJOR);
        prop_assert_eq!(DEV_NULL.minor, 3);
    }

    /// INV-DEV-6: DEV_ZERO has correct values
    #[test]
    fn test_dev_zero(_v in 0u8..1u8) {
        prop_assert_eq!(DEV_ZERO.major, MEM_MAJOR);
        prop_assert_eq!(DEV_ZERO.minor, 5);
    }

    /// INV-DEV-7: DEV_RANDOM and DEV_URANDOM distinct
    #[test]
    fn test_random_urandom_distinct(_v in 0u8..1u8) {
        prop_assert_ne!(DEV_RANDOM, DEV_URANDOM);
        prop_assert_eq!(DEV_RANDOM.major, MEM_MAJOR);
        prop_assert_eq!(DEV_URANDOM.major, MEM_MAJOR);
    }

    /// INV-DEV-8: standard majors are distinct
    #[test]
    fn test_majors_distinct(_v in 0u8..1u8) {
        let majors = [MEM_MAJOR, TTY_MAJOR, INPUT_MAJOR, FB_MAJOR, LP_MAJOR, SCSI_DISK_MAJOR, IDE_DISK_MAJOR];
        let mut seen = [false; 256];
        for m in &majors {
            prop_assert!(*m > 0);
            prop_assert!(!seen[*m as usize]);
            seen[*m as usize] = true;
        }
    }

    /// INV-DEV-9: Ord ordering — major first, then minor
    #[test]
    fn test_ordering(
        m1 in 0u32..10u32,
        m2 in 0u32..10u32,
        min1 in 0u32..10u32,
        min2 in 0u32..10u32,
    ) {
        let d1 = DevNo::new(m1, min1);
        let d2 = DevNo::new(m2, min2);
        prop_assert_eq!(d1.cmp(&d2), (m1, min1).cmp(&(m2, min2)));
    }

    /// INV-DEV-10: from_u64 preserves max values
    #[test]
    fn test_max_values(_v in 0u8..1u8) {
        let dev = DevNo::from_u64(u64::MAX);
        prop_assert_eq!(dev.major, u32::MAX);
        prop_assert_eq!(dev.minor, u32::MAX);
    }
}
