//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PageFlags bitmap invariant tests.
//!
//! Types copied from: kernel/src/mm/page_desc.rs

use proptest::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Copied types from kernel/src/mm/page_desc.rs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    Locked = 1 << 0,
    Writeback = 1 << 1,
    Referenced = 1 << 2,
    UpToDate = 1 << 3,
    Dirty = 1 << 4,
    Lru = 1 << 5,
    Head = 1 << 6,
    Waiters = 1 << 7,
    Active = 1 << 8,
    Reserved = 1 << 9,
    Private = 1 << 10,
    Reclaim = 1 << 11,
    SwapBacked = 1 << 12,
    Cow = 1 << 14,
    Anonymous = 1 << 15,
}

#[derive(Debug, Default)]
pub struct PageFlags(AtomicU32);

impl PageFlags {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    pub const fn from_raw(flags: u32) -> Self {
        Self(AtomicU32::new(flags))
    }

    pub fn raw(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn test(&self, flag: PageFlag) -> bool {
        self.0.load(Ordering::Relaxed) & (flag as u32) != 0
    }

    pub fn set(&self, flag: PageFlag) {
        self.0.fetch_or(flag as u32, Ordering::Release);
    }

    pub fn clear(&self, flag: PageFlag) {
        self.0.fetch_and(!(flag as u32), Ordering::Release);
    }

    pub fn test_and_set(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_or(bit, Ordering::AcqRel) & bit) != 0
    }

    pub fn test_and_clear(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_and(!bit, Ordering::AcqRel) & bit) != 0
    }

    pub fn clear_all(&self) {
        self.0.store(0, Ordering::Release);
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-FLAGS-1: from_raw preserves exact value
    #[test]
    fn test_from_raw_exact(val in 0u32..(1u32 << 16)) {
        let flags = PageFlags::from_raw(val);
        prop_assert_eq!(flags.raw(), val);
    }

    /// INV-FLAGS-2: test for known flags matches bit positions
    #[test]
    fn test_known_flags(val in 0u32..(1u32 << 16)) {
        let flags = PageFlags::from_raw(val);
        prop_assert_eq!(flags.test(PageFlag::Locked), val & (1 << 0) != 0);
        prop_assert_eq!(flags.test(PageFlag::Dirty), val & (1 << 4) != 0);
        prop_assert_eq!(flags.test(PageFlag::Anonymous), val & (1 << 15) != 0);
    }

    /// INV-FLAGS-3: clear_all resets all bits
    #[test]
    fn test_clear_all(val in 0u32..(1u32 << 16)) {
        let flags = PageFlags::from_raw(val);
        flags.clear_all();
        prop_assert_eq!(flags.raw(), 0);
    }

    /// INV-FLAGS-4: test_and_set returns previous value and sets bit
    #[test]
    fn test_and_set(flag in 0u8..16u8) {
        let flag_val = 1u32 << flag;
        let flags = PageFlags::from_raw(flag_val);
        let was_set = flags.test_and_set(PageFlag::Locked);
        prop_assert_eq!(was_set, flag_val & (1 << 0) != 0);
        prop_assert!(flags.test(PageFlag::Locked));
    }

    /// INV-FLAGS-5: test_and_clear returns previous value and clears bit
    #[test]
    fn test_and_clear(flag in 0u8..16u8) {
        let flag_val = 1u32 << flag;
        let flags = PageFlags::from_raw(flag_val);
        let was_set = flags.test_and_clear(PageFlag::Locked);
        prop_assert_eq!(was_set, flag_val & (1 << 0) != 0);
        // Locked should now be clear
        prop_assert_eq!(flags.raw() & (1 << 0), 0);
    }

    /// INV-FLAGS-6: new() creates zero flags
    #[test]
    fn test_new_zero(_unit in proptest::strategy::Just(())) {
        let flags = PageFlags::new();
        assert_eq!(flags.raw(), 0);
    }
}
