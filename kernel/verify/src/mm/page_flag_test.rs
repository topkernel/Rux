//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for PageFlag bitfield invariants.
//! Copied from: kernel/src/mm/page_desc.rs

use proptest::prelude::*;

// PageFlag enum — copied from page_desc.rs
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

proptest! {
    #[test]
    fn test_page_flags_are_powers_of_two(_v in 0u8..1u8) {
        let flags = [
            PageFlag::Locked, PageFlag::Writeback, PageFlag::Referenced,
            PageFlag::UpToDate, PageFlag::Dirty, PageFlag::Lru,
            PageFlag::Head, PageFlag::Waiters, PageFlag::Active,
            PageFlag::Reserved, PageFlag::Private, PageFlag::Reclaim,
            PageFlag::SwapBacked, PageFlag::Unevictable, PageFlag::Cow,
            PageFlag::Anonymous,
        ];
        for (i, &f) in flags.iter().enumerate() {
            let val = f as u32;
            assert!(val > 0 && (val & (val - 1)) == 0,
                    "PageFlag {:?} ({:#x}) must be power of two", f, val);
            assert_eq!(val, 1u32 << i, "PageFlag {:?} should be 1<<{}", f, i);
        }
    }

    #[test]
    fn test_page_flags_distinct(_v in 0u8..1u8) {
        let flags = [
            PageFlag::Locked as u32, PageFlag::Writeback as u32,
            PageFlag::Referenced as u32, PageFlag::UpToDate as u32,
            PageFlag::Dirty as u32, PageFlag::Lru as u32,
            PageFlag::Head as u32, PageFlag::Waiters as u32,
            PageFlag::Active as u32, PageFlag::Reserved as u32,
            PageFlag::Private as u32, PageFlag::Reclaim as u32,
            PageFlag::SwapBacked as u32, PageFlag::Unevictable as u32,
            PageFlag::Cow as u32, PageFlag::Anonymous as u32,
        ];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0,
                           "PageFlags {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_page_flags_fit_in_u32(_v in 0u8..1u8) {
        let all_flags: u32 = PageFlag::Locked as u32
            | PageFlag::Writeback as u32
            | PageFlag::Referenced as u32
            | PageFlag::UpToDate as u32
            | PageFlag::Dirty as u32
            | PageFlag::Lru as u32
            | PageFlag::Head as u32
            | PageFlag::Waiters as u32
            | PageFlag::Active as u32
            | PageFlag::Reserved as u32
            | PageFlag::Private as u32
            | PageFlag::Reclaim as u32
            | PageFlag::SwapBacked as u32
            | PageFlag::Unevictable as u32
            | PageFlag::Cow as u32
            | PageFlag::Anonymous as u32;
        // Highest bit is bit 15, so all flags fit in 16 bits
        assert!(all_flags < (1u32 << 16));
    }

    #[test]
    fn test_page_flag_set_unset(raw_flags in 0u32..(1u32 << 16)) {
        // Simulate PageFlags test/set/unset logic
        let locked_bit = PageFlag::Locked as u32;
        let dirty_bit = PageFlag::Dirty as u32;
        let cow_bit = PageFlag::Cow as u32;

        // test
        assert_eq!((raw_flags & locked_bit) != 0, raw_flags & 1 != 0);
        assert_eq!((raw_flags & dirty_bit) != 0, (raw_flags >> 4) & 1 != 0);
        assert_eq!((raw_flags & cow_bit) != 0, (raw_flags >> 14) & 1 != 0);

        // set
        let with_locked = raw_flags | locked_bit;
        assert!(with_locked & locked_bit != 0);

        // unset — should remove the bit but leave other bits unchanged
        let without_locked = with_locked & !locked_bit;
        assert_eq!(without_locked & locked_bit, 0);
        assert_eq!(without_locked, raw_flags & !locked_bit, "unset should restore non-locked bits");
    }

    #[test]
    fn test_page_flag_toggle(raw_flags in 0u32..(1u32 << 16)) {
        let dirty_bit = PageFlag::Dirty as u32;
        // Toggle: if set, unset; if unset, set
        let has = (raw_flags & dirty_bit) != 0;
        let toggled = if has { raw_flags & !dirty_bit } else { raw_flags | dirty_bit };
        assert_ne!((raw_flags & dirty_bit) != 0, (toggled & dirty_bit) != 0,
                   "Toggle should flip the bit");
    }

    #[test]
    fn test_page_flag_combine(raw_flags in 0u32..(1u32 << 16)) {
        let dirty_bit = PageFlag::Dirty as u32;
        let writeback_bit = PageFlag::Writeback as u32;

        // Combining should be additive
        let just_dirty = raw_flags | dirty_bit;
        let both = just_dirty | writeback_bit;
        assert!(both & dirty_bit != 0);
        assert!(both & writeback_bit != 0);
        // Other bits unchanged
        assert_eq!(both & !(dirty_bit | writeback_bit), raw_flags & !(dirty_bit | writeback_bit));
    }

    #[test]
    fn test_cow_and_anonymous_lsb14_15(_v in 0u8..1u8) {
        // Rux extension flags: Cow=bit14, Anonymous=bit15
        assert_eq!(PageFlag::Cow as u32, 1 << 14);
        assert_eq!(PageFlag::Anonymous as u32, 1 << 15);
    }
}
