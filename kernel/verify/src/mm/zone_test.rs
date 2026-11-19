//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Zone allocator arithmetic invariant tests.
//!
//! Types copied from: kernel/src/mm/zone.rs

use proptest::prelude::*;

// ============================================================================
// Copied functions from kernel/src/mm/zone.rs
// ============================================================================

pub const PAGE_SIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneType {
    ZoneDma = 0,
    ZoneDma32 = 1,
    ZoneNormal = 2,
    ZoneMovable = 3,
    ZoneCount = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfpFlags(pub u32);

impl GfpFlags {
    pub const GFP_KERNEL: GfpFlags = GfpFlags(0x01);
    pub const GFP_USER: GfpFlags = GfpFlags(0x02);
    pub const GFP_ATOMIC: GfpFlags = GfpFlags(0x04);
    pub const GFP_DMA: GfpFlags = GfpFlags(0x08);
    pub const GFP_DMA32: GfpFlags = GfpFlags(0x10);
    pub const __GFP_ZERO: GfpFlags = GfpFlags(0x100);
    pub const __GFP_MOVABLE: GfpFlags = GfpFlags(0x400);

    pub fn zone_type(&self) -> ZoneType {
        if self.0 & Self::GFP_DMA.0 != 0 {
            ZoneType::ZoneDma
        } else if self.0 & Self::GFP_DMA32.0 != 0 {
            ZoneType::ZoneDma32
        } else if self.0 & Self::__GFP_MOVABLE.0 != 0 {
            ZoneType::ZoneMovable
        } else {
            ZoneType::ZoneNormal
        }
    }
}

pub fn int_sqrt(n: usize) -> usize {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

pub fn pfn_to_phys(pfn: usize) -> usize {
    pfn.checked_mul(PAGE_SIZE).unwrap_or(0)
}

pub fn phys_to_pfn(phys: usize) -> usize {
    phys / PAGE_SIZE
}

/// Watermark check formula (extracted from Zone::watermark_ok)
pub fn watermark_ok(free_pages: usize, watermark: usize, order: usize) -> bool {
    let min = watermark.saturating_add((1usize << order).saturating_sub(1));
    free_pages >= min
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-ISQRT-1: int_sqrt(n)^2 <= n < (int_sqrt(n)+1)^2
    #[test]
    fn test_int_sqrt_bounds(n in 0usize..10_000_000usize) {
        let s = int_sqrt(n);
        prop_assert!(s * s <= n);
        if s < usize::MAX {
            prop_assert!(n < (s + 1) * (s + 1));
        }
    }

    /// INV-ISQRT-2: int_sqrt(0) == 0, int_sqrt(1) == 1
    #[test]
    fn test_int_sqrt_edge(_v in 0u8..1u8) {
        prop_assert_eq!(int_sqrt(0), 0);
        prop_assert_eq!(int_sqrt(1), 1);
    }

    /// INV-ISQRT-3: int_sqrt(n*n) == n for small n
    #[test]
    fn test_int_sqrt_perfect(n in 0usize..100_000usize) {
        prop_assert_eq!(int_sqrt(n * n), n);
    }

    /// INV-ISQRT-4: int_sqrt is monotonically non-decreasing
    #[test]
    fn test_int_sqrt_monotone(
        a in 0usize..10_000usize,
        b in 0usize..10_000usize,
    ) {
        let (small, large) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(int_sqrt(small) <= int_sqrt(large));
    }

    /// INV-PFN-1: phys_to_pfn(pfn_to_phys(pfn)) == pfn for non-overflowing
    #[test]
    fn test_pfn_phys_roundtrip(pfn in 0usize..1_000_000usize) {
        let phys = pfn_to_phys(pfn);
        prop_assert_eq!(phys_to_pfn(phys), pfn);
    }

    /// INV-PFN-2: pfn_to_phys(0) == 0
    #[test]
    fn test_pfn_zero(_v in 0u8..1u8) {
        prop_assert_eq!(pfn_to_phys(0), 0);
        prop_assert_eq!(phys_to_pfn(0), 0);
    }

    /// INV-GFP-1: GFP_KERNEL maps to ZoneNormal
    #[test]
    fn test_gfp_kernel(_v in 0u8..1u8) {
        prop_assert_eq!(GfpFlags::GFP_KERNEL.zone_type(), ZoneType::ZoneNormal);
    }

    /// INV-GFP-2: GFP_DMA maps to ZoneDma
    #[test]
    fn test_gfp_dma(_v in 0u8..1u8) {
        prop_assert_eq!(GfpFlags::GFP_DMA.zone_type(), ZoneType::ZoneDma);
    }

    /// INV-GFP-3: GFP_DMA32 maps to ZoneDma32
    #[test]
    fn test_gfp_dma32(_v in 0u8..1u8) {
        prop_assert_eq!(GfpFlags::GFP_DMA32.zone_type(), ZoneType::ZoneDma32);
    }

    /// INV-GFP-4: __GFP_MOVABLE maps to ZoneMovable
    #[test]
    fn test_gfp_movable(_v in 0u8..1u8) {
        prop_assert_eq!(GfpFlags::__GFP_MOVABLE.zone_type(), ZoneType::ZoneMovable);
    }

    /// INV-GFP-5: DMA takes priority over MOVABLE
    #[test]
    fn test_gfp_dma_priority(_v in 0u8..1u8) {
        let flags = GfpFlags(GfpFlags::GFP_DMA.0 | GfpFlags::__GFP_MOVABLE.0);
        prop_assert_eq!(flags.zone_type(), ZoneType::ZoneDma);
    }

    /// INV-WM-1: watermark_ok with order 0
    #[test]
    fn test_watermark_order0(
        free in 0usize..10_000usize,
        wmark in 0usize..10_000usize,
    ) {
        prop_assert_eq!(watermark_ok(free, wmark, 0), free >= wmark);
    }

    /// INV-WM-2: watermark_ok with order > 0
    #[test]
    fn test_watermark_order(
        free in 0usize..100_000usize,
        wmark in 0usize..10_000usize,
        order in 0usize..10usize,
    ) {
        let min = wmark.saturating_add((1usize << order).saturating_sub(1));
        prop_assert_eq!(watermark_ok(free, wmark, order), free >= min);
    }
}
