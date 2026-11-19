//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for pfn_valid / phys_valid range checks.
//! Copied from: kernel/src/mm/page_desc.rs

use proptest::prelude::*;

// ============================================================================
// Copied constants and functions from kernel/src/mm/page_desc.rs
// ============================================================================

pub const PAGE_SIZE: usize = 4096;

// Typical Rux configuration values
pub const PHYS_MEMORY_BASE: usize = 0x8020_0000;
pub const PHYS_MEMORY_SIZE: usize = 0x8000_0000; // 2GB

pub const MIN_PFN: usize = PHYS_MEMORY_BASE / PAGE_SIZE;
pub const MAX_PFN: usize = (PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE) / PAGE_SIZE;

#[inline]
pub const fn pfn_valid(pfn: usize) -> bool {
    pfn >= MIN_PFN && pfn < MAX_PFN
}

#[inline]
pub const fn phys_valid(phys: usize) -> bool {
    phys >= PHYS_MEMORY_BASE && phys < PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    #[test]
    fn test_pfn_valid_at_boundaries(pfn in 0usize..MIN_PFN + 2usize) {
        let valid = pfn_valid(pfn);
        if pfn >= MIN_PFN {
            prop_assert!(valid);
        } else {
            prop_assert!(!valid);
        }
    }

    #[test]
    fn test_pfn_valid_at_max(pfn in MAX_PFN - 2..MAX_PFN + 2usize) {
        let valid = pfn_valid(pfn);
        prop_assert_eq!(valid, pfn < MAX_PFN);
    }

    #[test]
    fn test_pfn_valid_far_below(pfn in 0usize..100usize) {
        prop_assert!(!pfn_valid(pfn));
    }

    #[test]
    fn test_pfn_valid_far_above(offset in 0usize..1000usize) {
        let pfn = MAX_PFN + offset;
        prop_assert!(!pfn_valid(pfn));
    }

    #[test]
    fn test_phys_valid_at_base(phys in PHYS_MEMORY_BASE - PAGE_SIZE..PHYS_MEMORY_BASE + PAGE_SIZE) {
        let valid = phys_valid(phys);
        prop_assert_eq!(valid, phys >= PHYS_MEMORY_BASE);
    }

    #[test]
    fn test_phys_valid_at_end(phys in PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE - PAGE_SIZE
                                     ..PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE + PAGE_SIZE) {
        let valid = phys_valid(phys);
        prop_assert_eq!(valid, phys < PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE);
    }

    #[test]
    fn test_phys_valid_far_below(phys in 0usize..0x1000usize) {
        prop_assert!(!phys_valid(phys));
    }

    #[test]
    fn test_phys_valid_far_above(offset in 0usize..1000usize) {
        let phys = PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE + offset;
        prop_assert!(!phys_valid(phys));
    }

    #[test]
    fn test_pfn_phys_roundtrip(pfn in MIN_PFN..MAX_PFN) {
        let phys = pfn * PAGE_SIZE;
        prop_assert!(phys_valid(phys));
        // phys / PAGE_SIZE should give back pfn
        prop_assert_eq!(phys / PAGE_SIZE, pfn);
    }

    #[test]
    fn test_phys_pfn_roundtrip(phys in PHYS_MEMORY_BASE..PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE) {
        let pfn = phys / PAGE_SIZE;
        prop_assert!(pfn_valid(pfn));
        // pfn * PAGE_SIZE should give back phys (page-aligned)
        prop_assert_eq!(pfn * PAGE_SIZE, phys - (phys % PAGE_SIZE));
    }

    #[test]
    fn test_pfn_valid_range_is_contiguous(
        pfn1 in MIN_PFN..MAX_PFN,
        pfn2 in MIN_PFN..MAX_PFN,
    ) {
        prop_assert!(pfn_valid(pfn1));
        prop_assert!(pfn_valid(pfn2));
    }

    #[test]
    fn test_min_max_pfn_constants(_v in 0u8..1u8) {
        prop_assert!(MIN_PFN < MAX_PFN);
        prop_assert_eq!(MIN_PFN * PAGE_SIZE, PHYS_MEMORY_BASE);
        prop_assert_eq!(MAX_PFN * PAGE_SIZE, PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE);
    }
}
