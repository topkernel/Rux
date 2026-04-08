//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Swap entry encoding/decoding invariant tests.
//!
//! Functions copied from: kernel/src/mm/swap.rs

use proptest::prelude::*;

// ============================================================================
// Copied functions from kernel/src/mm/swap.rs
// ============================================================================

pub const SWAP_ENTRY_SIGNATURE: u64 = 1u64 << 62;

pub const fn make_swap_entry(swap_type: u32, swap_offset: u64) -> u64 {
    SWAP_ENTRY_SIGNATURE
        | ((swap_type as u64 & 0x3) << 8)
        | ((swap_offset & 0x003F_FFFF_FFFF) << 10)
}

pub const fn is_swap_entry(pte: u64) -> bool {
    (pte & SWAP_ENTRY_SIGNATURE) != 0
}

pub const fn swap_entry_type(pte: u64) -> u32 {
    ((pte >> 8) & 0x3) as u32
}

pub const fn swap_entry_offset(pte: u64) -> u64 {
    (pte >> 10) & 0x003F_FFFF_FFFF
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SWAP-1: make + is_swap_entry roundtrip
    #[test]
    fn test_make_is_swap(
        swap_type in 0u32..4u32,
        swap_offset in 0u64..0x100000u64,
    ) {
        let entry = make_swap_entry(swap_type, swap_offset);
        prop_assert!(is_swap_entry(entry));
    }

    /// INV-SWAP-2: make + type roundtrip
    #[test]
    fn test_type_roundtrip(
        swap_type in 0u32..4u32,
        swap_offset in 0u64..0x100000u64,
    ) {
        let entry = make_swap_entry(swap_type, swap_offset);
        prop_assert_eq!(swap_entry_type(entry), swap_type & 0x3);
    }

    /// INV-SWAP-3: make + offset roundtrip
    #[test]
    fn test_offset_roundtrip(
        swap_type in 0u32..4u32,
        swap_offset in 0u64..0x100000u64,
    ) {
        let entry = make_swap_entry(swap_type, swap_offset);
        prop_assert_eq!(swap_entry_offset(entry), swap_offset & 0x003F_FFFF_FFFF);
    }

    /// INV-SWAP-4: zero PTE is not a swap entry
    #[test]
    fn test_zero_not_swap(_v in 0u8..1u8) {
        prop_assert!(!is_swap_entry(0));
    }

    /// INV-SWAP-5: type field only uses bits 9:8
    #[test]
    fn test_type_isolated(
        swap_type in 0u32..4u32,
        extra in 0u64..0x100u64,
    ) {
        let entry = make_swap_entry(swap_type, 0);
        let entry_with_extra = entry | (extra << 16);
        prop_assert_eq!(swap_entry_type(entry_with_extra), swap_type);
    }

    /// INV-SWAP-6: offset field only uses bits 53:10
    #[test]
    fn test_offset_isolated(
        swap_offset in 0u64..0x10000u64,
    ) {
        let entry = make_swap_entry(0, swap_offset);
        prop_assert_eq!(swap_entry_offset(entry), swap_offset);
    }

    /// INV-SWAP-7: swap entry has bit 0 cleared (triggers page fault)
    #[test]
    fn test_bit0_clear(
        swap_type in 0u32..4u32,
        swap_offset in 0u64..0x10000u64,
    ) {
        let entry = make_swap_entry(swap_type, swap_offset);
        prop_assert_eq!(entry & 1, 0);
    }

    /// INV-SWAP-8: full roundtrip encode→decode
    #[test]
    fn test_full_roundtrip(
        swap_type in 0u32..4u32,
        swap_offset in 0u64..0x3FFFFFFFFFFu64,
    ) {
        let entry = make_swap_entry(swap_type, swap_offset);
        prop_assert!(is_swap_entry(entry));
        prop_assert_eq!(swap_entry_type(entry), swap_type & 0x3);
        prop_assert_eq!(swap_entry_offset(entry), swap_offset & 0x003F_FFFF_FFFF);
    }

    /// INV-SWAP-9: type masks to 2 bits (max 3)
    #[test]
    fn test_type_masked(
        raw_type in 0u32..256u32,
    ) {
        let entry = make_swap_entry(raw_type, 0);
        prop_assert!(swap_entry_type(entry) < 4);
    }

    /// INV-SWAP-10: different types produce different entries
    #[test]
    fn test_different_types(
        t1 in 0u32..4u32,
        t2 in 0u32..4u32,
    ) {
        if t1 != t2 {
            let e1 = make_swap_entry(t1, 42);
            let e2 = make_swap_entry(t2, 42);
            prop_assert_ne!(swap_entry_type(e1), swap_entry_type(e2));
        }
    }
}
