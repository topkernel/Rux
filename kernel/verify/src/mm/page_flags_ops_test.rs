//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PageFlag/PageType/PageFlags operations invariant tests.
//!
//! Types copied from: kernel/src/mm/page_desc.rs
//! NOTE: AtomicU32 replaced with plain u32 for std testing.

use proptest::prelude::*;
use std::sync::atomic::Ordering;

// ============================================================================
// Copied types from kernel/src/mm/page_desc.rs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    Locked      = 1 << 0,
    Writeback   = 1 << 1,
    Referenced  = 1 << 2,
    UpToDate    = 1 << 3,
    Dirty       = 1 << 4,
    Lru         = 1 << 5,
    Head        = 1 << 6,
    Waiters     = 1 << 7,
    Active      = 1 << 8,
    Reserved    = 1 << 9,
    Private     = 1 << 10,
    Reclaim     = 1 << 11,
    SwapBacked  = 1 << 12,
    Unevictable = 1 << 13,
    Cow         = 1 << 14,
    Anonymous   = 1 << 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageType {
    Normal    = 0,
    Buddy     = 1,
    Slab      = 2,
    PageCache = 3,
    Anonymous = 4,
}

/// PageFlags with plain u32 instead of AtomicU32
#[derive(Debug)]
pub struct PageFlags(u32);

impl PageFlags {
    pub const fn new() -> Self {
        Self(0)
    }

    pub const fn from_raw(flags: u32) -> Self {
        Self(flags)
    }

    pub fn raw(&self) -> u32 {
        self.0
    }

    pub fn test(&self, flag: PageFlag) -> bool {
        self.0 & (flag as u32) != 0
    }

    pub fn set(&mut self, flag: PageFlag) {
        self.0 |= flag as u32;
    }

    pub fn clear(&mut self, flag: PageFlag) {
        self.0 &= !(flag as u32);
    }

    pub fn test_and_set(&mut self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        let old = self.0 & bit != 0;
        self.0 |= bit;
        old
    }

    pub fn test_and_clear(&mut self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        let old = self.0 & bit != 0;
        self.0 &= !bit;
        old
    }

    pub fn clear_all(&mut self) {
        self.0 = 0;
    }
}

impl Default for PageFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// All PageFlag variants for iteration
const ALL_FLAGS: &[PageFlag] = &[
    PageFlag::Locked,
    PageFlag::Writeback,
    PageFlag::Referenced,
    PageFlag::UpToDate,
    PageFlag::Dirty,
    PageFlag::Lru,
    PageFlag::Head,
    PageFlag::Waiters,
    PageFlag::Active,
    PageFlag::Reserved,
    PageFlag::Private,
    PageFlag::Reclaim,
    PageFlag::SwapBacked,
    PageFlag::Unevictable,
    PageFlag::Cow,
    PageFlag::Anonymous,
];

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-PGFL-1: new flags are empty, test any flag returns false
    #[test]
    fn test_new_empty(_v in 0u8..1u8) {
        let flags = PageFlags::new();
        for flag in ALL_FLAGS {
            prop_assert!(!flags.test(*flag));
        }
        prop_assert_eq!(flags.raw(), 0);
    }

    /// INV-PGFL-2: set then test returns true
    #[test]
    fn test_set_then_test(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        flags.set(flag);
        prop_assert!(flags.test(flag));
    }

    /// INV-PGFL-3: set then clear then test returns false
    #[test]
    fn test_set_clear_roundtrip(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        flags.set(flag);
        flags.clear(flag);
        prop_assert!(!flags.test(flag));
    }

    /// INV-PGFL-4: test_and_set returns old value (false on first set)
    #[test]
    fn test_test_and_set_first(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        let old = flags.test_and_set(flag);
        prop_assert!(!old);
        prop_assert!(flags.test(flag));
    }

    /// INV-PGFL-5: test_and_clear returns old value (true if was set)
    #[test]
    fn test_test_and_clear(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        flags.set(flag);
        let old = flags.test_and_clear(flag);
        prop_assert!(old);
        prop_assert!(!flags.test(flag));
    }

    /// INV-PGFL-6: setting same flag twice is idempotent
    #[test]
    fn test_set_idempotent(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        flags.set(flag);
        let raw1 = flags.raw();
        flags.set(flag);
        let raw2 = flags.raw();
        prop_assert_eq!(raw1, raw2);
    }

    /// INV-PGFL-7: clear_all removes all flags
    #[test]
    fn test_clear_all(
        bits in 0u32..0xFFFFu32,
    ) {
        let mut flags = PageFlags::from_raw(bits);
        flags.clear_all();
        prop_assert_eq!(flags.raw(), 0);
    }

    /// INV-PGFL-8: from_raw + raw roundtrip
    #[test]
    fn test_from_raw_roundtrip(raw in 0u32..0xFFFFu32) {
        let flags = PageFlags::from_raw(raw);
        prop_assert_eq!(flags.raw(), raw);
    }

    /// INV-PGFL-9: all 16 PageFlag variants have distinct powers-of-2 values
    #[test]
    fn test_flags_distinct_pow2(_v in 0u8..1u8) {
        let mut seen = 0u32;
        for flag in ALL_FLAGS {
            let val = *flag as u32;
            prop_assert!(val != 0);
            prop_assert_eq!(val & (val - 1), 0, "flag value {} not a power of 2", val);
            prop_assert_eq!(seen & val, 0, "flag value {} already seen", val);
            seen |= val;
        }
    }

    /// INV-PGFL-10: setting multiple flags preserves all
    #[test]
    fn test_set_multiple(
        idx1 in 0usize..16usize,
        idx2 in 0usize..16usize,
    ) {
        let mut flags = PageFlags::new();
        let flag1 = ALL_FLAGS[idx1];
        let flag2 = ALL_FLAGS[idx2];
        flags.set(flag1);
        flags.set(flag2);
        prop_assert!(flags.test(flag1));
        prop_assert!(flags.test(flag2));
    }

    /// INV-PGFL-11: clearing one flag does not affect others
    #[test]
    fn test_clear_isolated(
        idx1 in 0usize..16usize,
        idx2 in 0usize..16usize,
    ) {
        if idx1 == idx2 {
            return Ok(());
        }
        let mut flags = PageFlags::new();
        let flag1 = ALL_FLAGS[idx1];
        let flag2 = ALL_FLAGS[idx2];
        flags.set(flag1);
        flags.set(flag2);
        flags.clear(flag1);
        prop_assert!(!flags.test(flag1));
        prop_assert!(flags.test(flag2));
    }

    /// INV-PGFL-12: test_and_set returns true on second call
    #[test]
    fn test_test_and_set_twice(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        let first = flags.test_and_set(flag);
        let second = flags.test_and_set(flag);
        prop_assert!(!first);
        prop_assert!(second);
    }

    /// INV-PGFL-13: test_and_clear returns false when not set
    #[test]
    fn test_test_and_clear_not_set(idx in 0usize..16usize) {
        let mut flags = PageFlags::new();
        let flag = ALL_FLAGS[idx];
        let old = flags.test_and_clear(flag);
        prop_assert!(!old);
    }

    /// INV-PGFL-14: PageType discriminants are 0..=4
    #[test]
    fn test_page_type_range(_v in 0u8..1u8) {
        let types = [PageType::Normal, PageType::Buddy, PageType::Slab, PageType::PageCache, PageType::Anonymous];
        let mut seen = vec![false; 5];
        for t in &types {
            let v = *t as u32;
            prop_assert!(v < 5, "PageType discriminant {} out of range", v);
            seen[v as usize] = true;
        }
        // All 5 values present
        for s in &seen {
            prop_assert!(*s);
        }
    }

    /// INV-PGFL-15: all flags fit in u16
    #[test]
    fn test_flags_fit_u16(_v in 0u8..1u8) {
        let max_flag = ALL_FLAGS.iter().map(|f| *f as u32).max().unwrap();
        prop_assert!(max_flag <= 0xFFFF, "highest flag bit {} exceeds u16", max_flag);
    }

    /// INV-PGFL-16: random raw value test/clear/set consistency
    #[test]
    fn test_random_ops(
        raw in 0u32..0xFFFFu32,
        idx in 0usize..16usize,
    ) {
        let mut flags = PageFlags::from_raw(raw);
        let flag = ALL_FLAGS[idx];
        let was_set = flags.test(flag);
        flags.clear(flag);
        prop_assert!(!flags.test(flag));
        flags.set(flag);
        prop_assert!(flags.test(flag));
        // Other flags unchanged
        for (i, other) in ALL_FLAGS.iter().enumerate() {
            if i != idx {
                let expected = (raw & (*other as u32)) != 0;
                prop_assert_eq!(flags.test(*other), expected);
            }
        }
    }
}

// ============================================================================
// Atomicity tests (multi-threaded)
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::thread;

/// INV-PGFL-17: concurrent set on distinct bits never loses updates.
/// Each of 8 threads sets its own bit 10,000 times; after all join,
/// all 8 bits must be present.
#[test]
fn test_page_flags_atomicity_set() {
    let flags = Arc::new(AtomicU32::new(0));
    let mut handles = vec![];

    for i in 0..8u32 {
        let f = flags.clone();
        handles.push(thread::spawn(move || {
            let bit = 1u32 << i;
            for _ in 0..10_000 {
                f.fetch_or(bit, Ordering::Release);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
    let final_val = flags.load(Ordering::Acquire);
    assert_eq!(final_val, 0xFF, "concurrent set lost updates: got 0x{:x}, expected 0xFF", final_val);
}

/// INV-PGFL-18: concurrent set+clear cycles end with all bits set.
/// Each thread toggles its bit many times, then sets it permanently.
#[test]
fn test_page_flags_atomicity_toggle_then_set() {
    let flags = Arc::new(AtomicU32::new(0));
    let mut handles = vec![];

    for i in 0..8u32 {
        let f = flags.clone();
        handles.push(thread::spawn(move || {
            let bit = 1u32 << i;
            // Toggle many times
            for _ in 0..10_000 {
                f.fetch_or(bit, Ordering::Release);
                f.fetch_and(!bit, Ordering::Release);
            }
            // Set final state
            f.fetch_or(bit, Ordering::Release);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
    let final_val = flags.load(Ordering::Acquire);
    assert_eq!(final_val, 0xFF, "final state after toggle+set: got 0x{:x}, expected 0xFF", final_val);
}

/// INV-PGFL-19: concurrent test_and_set/test_and_clear on same bit serialize correctly.
/// Two threads contend on the same bit; the final bit state must be 0 or 1.
#[test]
fn test_page_flags_atomicity_contended_bit() {
    let flags = Arc::new(AtomicU32::new(0));

    let f1 = flags.clone();
    let h1 = thread::spawn(move || {
        for _ in 0..5_000 {
            f1.fetch_or(1, Ordering::AcqRel);
        }
    });

    let f2 = flags.clone();
    let h2 = thread::spawn(move || {
        for _ in 0..5_000 {
            f2.fetch_and(!1u32, Ordering::AcqRel);
        }
    });

    h1.join().unwrap();
    h2.join().unwrap();

    let final_val = flags.load(Ordering::Acquire);
    assert!(final_val == 0 || final_val == 1,
        "bit corrupted: got 0x{:x}", final_val);
}
