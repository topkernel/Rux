//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Reverse mapping VPN extraction and mapcount invariant tests.
//!
//! Types copied from: kernel/src/mm/rmap.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/mm/rmap.rs
// ============================================================================

pub const PAGE_SIZE: usize = 4096;

/// Extract virtual page number from virtual address.
pub fn addr_to_vpn(address: usize) -> usize {
    address / PAGE_SIZE
}

/// Extract Sv39 three-level VPN indices from virtual address.
pub fn sv39_vpn_indices(vaddr: usize) -> (usize, usize, usize) {
    let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    (vpn2, vpn1, vpn0)
}

/// Reconstruct virtual address from Sv39 VPN indices.
pub fn sv39_vpn_to_addr(vpn2: usize, vpn1: usize, vpn0: usize, offset: usize) -> usize {
    ((vpn2 as usize) << 30)
        | ((vpn1 as usize) << 21)
        | ((vpn0 as usize) << 12)
        | offset
}

/// Page is mapped if mapcount >= 0.
/// (In kernel: mapcount starts at -1 for unmapped, 0 for first mapping.)
pub fn page_mapped(mapcount: i32) -> bool {
    mapcount >= 0
}

/// First-mapping guard: should add to LRU only if mapcount was -1 before inc.
/// After inc, mapcount == 0 means "first mapping just added".
pub fn should_add_to_lru(old_mapcount: i32) -> bool {
    old_mapcount < 0
}

/// Last-unmapping guard: should clear flags and remove from LRU if
/// mapcount was 0 before dec (meaning this was the last mapping).
pub fn should_remove_from_lru(old_mapcount: i32) -> bool {
    old_mapcount == 0
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-RMAP-1: addr_to_vpn(0) is 0
    #[test]
    fn test_vpn_zero(_v in 0u8..1u8) {
        prop_assert_eq!(addr_to_vpn(0), 0);
    }

    /// INV-RMAP-2: addr_to_vpn is monotone
    #[test]
    fn test_vpn_monotone(
        a in 0usize..1_000_000usize,
        b in 0usize..1_000_000usize,
    ) {
        let (small, large) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(addr_to_vpn(small) <= addr_to_vpn(large));
    }

    /// INV-RMAP-3: VPN * PAGE_SIZE <= addr < (VPN+1) * PAGE_SIZE
    #[test]
    fn test_vpn_bounds(addr in 0usize..100_000_000usize) {
        let vpn = addr_to_vpn(addr);
        prop_assert!(vpn * PAGE_SIZE <= addr);
        prop_assert!(addr < (vpn + 1) * PAGE_SIZE);
    }

    /// INV-RMAP-4: VPN roundtrip: addr -> vpn -> vpn*PAGE_SIZE <= addr
    #[test]
    fn test_vpn_roundtrip(addr in 0usize..usize::MAX) {
        let vpn = addr_to_vpn(addr);
        prop_assert!(vpn * PAGE_SIZE <= addr);
    }

    /// INV-RMAP-5: Sv39 VPN indices are in [0, 511]
    #[test]
    fn test_sv39_vpn_range(vaddr in 0usize..usize::MAX) {
        let (vpn2, vpn1, vpn0) = sv39_vpn_indices(vaddr);
        prop_assert!(vpn2 < 512);
        prop_assert!(vpn1 < 512);
        prop_assert!(vpn0 < 512);
    }

    /// INV-RMAP-6: Sv39 VPN roundtrip with offset
    #[test]
    fn test_sv39_vpn_roundtrip(vaddr in 0usize..(1usize << 39)) {
        let offset = vaddr & 0xFFF;
        let (vpn2, vpn1, vpn0) = sv39_vpn_indices(vaddr);
        let reconstructed = sv39_vpn_to_addr(vpn2, vpn1, vpn0, offset);
        // Mask to 39 bits for comparison
        let mask = (1usize << 39) - 1;
        prop_assert_eq!(reconstructed & mask, vaddr & mask);
    }

    /// INV-RMAP-7: sv39_vpn_indices(addr=0) is (0,0,0)
    #[test]
    fn test_sv39_zero(_v in 0u8..1u8) {
        prop_assert_eq!(sv39_vpn_indices(0), (0, 0, 0));
    }

    /// INV-RMAP-8: page_mapped: negative mapcount = not mapped
    #[test]
    fn test_mapped_negative(mc in -10i32..=-1i32) {
        prop_assert!(!page_mapped(mc));
    }

    /// INV-RMAP-9: page_mapped: non-negative mapcount = mapped
    #[test]
    fn test_mapped_nonnegative(mc in 0i32..100i32) {
        prop_assert!(page_mapped(mc));
    }

    /// INV-RMAP-10: should_add_to_lru when old_mapcount < 0
    #[test]
    fn test_add_lru(mc in -10i32..=-1i32) {
        prop_assert!(should_add_to_lru(mc));
    }

    /// INV-RMAP-11: should NOT add to_lru when old_mapcount >= 0
    #[test]
    fn test_no_add_lru(mc in 0i32..100i32) {
        prop_assert!(!should_add_to_lru(mc));
    }

    /// INV-RMAP-12: should_remove_from_lru only when old_mapcount == 0
    #[test]
    fn test_remove_lru(mc in 0i32..100i32) {
        if mc == 0 {
            prop_assert!(should_remove_from_lru(mc));
        } else {
            prop_assert!(!should_remove_from_lru(mc));
        }
    }

    /// INV-RMAP-13: addr_to_vpn(PAGE_SIZE) is 1
    #[test]
    fn test_vpn_page_size(_v in 0u8..1u8) {
        prop_assert_eq!(addr_to_vpn(PAGE_SIZE), 1);
    }

    /// INV-RMAP-14: addr_to_vpn(PAGE_SIZE - 1) is 0
    #[test]
    fn test_vpn_last_byte(_v in 0u8..1u8) {
        prop_assert_eq!(addr_to_vpn(PAGE_SIZE - 1), 0);
    }

    /// INV-RMAP-15: sv39_vpn_to_addr with zero offsets gives page-aligned address
    #[test]
    fn test_sv39_page_aligned(
        vpn2 in 0usize..511usize,
        vpn1 in 0usize..511usize,
        vpn0 in 0usize..511usize,
    ) {
        let addr = sv39_vpn_to_addr(vpn2, vpn1, vpn0, 0);
        prop_assert_eq!(addr & 0xFFF, 0);
    }

    /// INV-RMAP-16: sv39 offset preserved
    #[test]
    fn test_sv39_offset(
        vaddr in 0usize..(1usize << 39),
    ) {
        let offset = vaddr & 0xFFF;
        let (vpn2, vpn1, vpn0) = sv39_vpn_indices(vaddr);
        let reconstructed = sv39_vpn_to_addr(vpn2, vpn1, vpn0, offset);
        prop_assert_eq!(reconstructed & 0xFFF, offset);
    }
}
