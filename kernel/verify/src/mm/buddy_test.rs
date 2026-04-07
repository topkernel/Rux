//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Buddy allocator mathematics invariant tests.
//!
//! Types and functions copied from: kernel/src/mm/buddy_allocator.rs

use proptest::prelude::*;

// ============================================================================
// Constants and functions copied from kernel/src/mm/buddy_allocator.rs
// ============================================================================

const PAGE_SIZE: usize = 4096;
const DEFAULT_MAX_ORDER: usize = 10;

/// Check if PFN is aligned to the given order.
fn is_aligned(pfn: usize, order: usize) -> bool {
    let size = 1usize << order;
    (pfn & (size - 1)) == 0
}

/// Compute buddy PFN: flip the bit at position `order`.
fn buddy_pfn(pfn: usize, order: usize) -> usize {
    pfn ^ (1usize << order)
}

/// Convert size in bytes to allocation order.
fn size_to_order(size: usize, max_order: usize) -> usize {
    if size <= PAGE_SIZE {
        return 0;
    }
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
    if order > max_order { max_order } else { order }
}

/// Get the buddy index relative to a block start.
fn get_buddy_idx(page_idx: usize, order: usize) -> usize {
    page_idx ^ (1usize << order)
}

/// Convert order to number of pages.
fn order_to_pages(order: usize) -> usize {
    1usize << order
}

/// Convert order to number of bytes.
fn order_to_bytes(order: usize) -> usize {
    PAGE_SIZE << order
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-BUDDY-ALIGN-1: order-0 alignment: any pfn is aligned
    #[test]
    fn test_order0_any_pfn_aligned(pfn in 0usize..1_000_000) {
        prop_assert!(is_aligned(pfn, 0));
    }

    /// INV-BUDDY-ALIGN-2: aligned PFN passes alignment check
    #[test]
    fn test_alignment_roundtrip(
        order in 1usize..DEFAULT_MAX_ORDER,
        multiplier in 0usize..1000usize,
    ) {
        let size = 1usize << order;
        let aligned_pfn = multiplier * size;
        prop_assert!(is_aligned(aligned_pfn, order));
    }

    /// INV-BUDDY-ALIGN-3: unaligned PFN fails alignment check
    #[test]
    fn test_unaligned_fails(
        order in 1usize..DEFAULT_MAX_ORDER,
    ) {
        let size = 1usize << order;
        let unaligned_pfn = if size > 1 { size - 1 } else { 0 };
        if unaligned_pfn > 0 {
            prop_assert!(!is_aligned(unaligned_pfn, order));
        }
    }

    /// INV-BUDDY-MERGE-1: buddy(buddy(pfn, order), order) == pfn
    #[test]
    fn test_buddy_is_involution(
        pfn in 0usize..1_000_000usize,
        order in 0usize..DEFAULT_MAX_ORDER,
    ) {
        let b = buddy_pfn(pfn, order);
        let b2 = buddy_pfn(b, order);
        prop_assert_eq!(b2, pfn);
    }

    /// INV-BUDDY-MERGE-2: buddy of aligned pfn differs only at bit `order`
    #[test]
    fn test_buddy_differs_at_order_bit(order in 1usize..DEFAULT_MAX_ORDER) {
        let size = 1usize << order;
        for &aligned_pfn in &[0usize, size, size * 3, size * 7] {
            let b = buddy_pfn(aligned_pfn, order);
            let xor = aligned_pfn ^ b;
            prop_assert_eq!(xor, size, "buddy should differ only at bit {}", order);
        }
    }

    /// INV-BUDDY-MERGE-3: buddy pair covers a contiguous aligned block
    #[test]
    fn test_buddy_pair_alignment(order in 1usize..DEFAULT_MAX_ORDER) {
        let size = 1usize << order;
        let aligned_pfn = 12345usize / size * size;
        let b = buddy_pfn(aligned_pfn, order);
        let (lo, hi) = if b < aligned_pfn { (b, aligned_pfn) } else { (aligned_pfn, b) };
        prop_assert_eq!(lo + size, hi);
        prop_assert!(is_aligned(lo, order));
    }

    /// INV-BUDDY-SIZE-1: block size grows as 2^order
    #[test]
    fn test_block_size_power_of_two(order in 0usize..DEFAULT_MAX_ORDER) {
        let byte_size = order_to_bytes(order);
        prop_assert!(byte_size.is_power_of_two());
        prop_assert_eq!(byte_size, PAGE_SIZE << order);
    }

    /// INV-BUDDY-SIZE-2: order_to_pages matches
    #[test]
    fn test_order_to_pages(order in 0usize..DEFAULT_MAX_ORDER) {
        let pages = order_to_pages(order);
        prop_assert_eq!(pages, 1usize << order);
    }

    /// INV-BUDDY-ORDER-1: size_to_order roundtrip
    #[test]
    fn test_size_to_order_roundtrip(order in 0usize..DEFAULT_MAX_ORDER) {
        let size = order_to_bytes(order);
        let computed_order = size_to_order(size, DEFAULT_MAX_ORDER);
        prop_assert_eq!(computed_order, order);
    }

    /// INV-BUDDY-ORDER-2: size_to_order clamped to max
    #[test]
    fn test_size_to_order_clamped(extra_bits in 1usize..20usize) {
        let size = PAGE_SIZE << (DEFAULT_MAX_ORDER + extra_bits);
        let order = size_to_order(size, DEFAULT_MAX_ORDER);
        prop_assert_eq!(order, DEFAULT_MAX_ORDER);
    }

    /// INV-BUDDY-IDX-1: get_buddy_idx matches buddy_pfn
    #[test]
    fn test_get_buddy_idx(
        page_idx in 0usize..10_000usize,
        order in 0usize..DEFAULT_MAX_ORDER,
    ) {
        let buddy = get_buddy_idx(page_idx, order);
        let buddy2 = buddy_pfn(page_idx, order);
        prop_assert_eq!(buddy, buddy2);
    }
}
