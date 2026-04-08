//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for memblock region arithmetic.
//! Copied from: kernel/src/mm/memblock.rs

use proptest::prelude::*;

pub const PAGE_SIZE: usize = 4096;

// Copied MemBlockFlags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemBlockFlags(u32);

impl MemBlockFlags {
    pub const NONE: MemBlockFlags = MemBlockFlags(0);
    pub const NOMAP: MemBlockFlags = MemBlockFlags(1 << 0);
    pub const MIRROR: MemBlockFlags = MemBlockFlags(1 << 1);
}

// Copied MemBlockRegion
#[derive(Debug, Clone, Copy)]
pub struct MemBlockRegion {
    pub base: usize,
    pub size: usize,
    pub flags: MemBlockFlags,
    pub nid: u32,
}

impl MemBlockRegion {
    pub const fn new(base: usize, size: usize) -> Self {
        Self { base, size, flags: MemBlockFlags::NONE, nid: 0 }
    }

    pub fn end(&self) -> usize { self.base + self.size }

    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.end()
    }

    pub fn base_pfn(&self) -> usize { self.base / PAGE_SIZE }

    pub fn end_pfn(&self) -> usize { self.end() / PAGE_SIZE }

    pub fn page_count(&self) -> usize { self.size / PAGE_SIZE }
}

proptest! {
    #[test]
    fn test_region_contains(addr in 0usize..1_000_000usize) {
        let region = MemBlockRegion::new(4096, 8192);
        // region covers [4096, 12288)
        let in_range = addr >= 4096 && addr < 12288;
        assert_eq!(region.contains(addr), in_range);
    }

    #[test]
    fn test_region_end(base in 0usize..(1usize << 48), size in 4096usize..(1usize << 30)) {
        let region = MemBlockRegion::new(base, size);
        assert_eq!(region.end(), base + size);
    }

    #[test]
    fn test_region_page_count(size in 0usize..10_000_000usize) {
        let region = MemBlockRegion::new(0, size);
        assert_eq!(region.page_count(), size / PAGE_SIZE);
    }

    #[test]
    fn test_region_pfn_roundtrip(base_page in 0usize..(1usize << 20), size_pages in 1usize..10000usize) {
        let base = base_page * PAGE_SIZE;
        let size = size_pages * PAGE_SIZE;
        let region = MemBlockRegion::new(base, size);
        assert_eq!(region.end_pfn(), (base + size) / PAGE_SIZE);
        assert_eq!(region.page_count(), region.end_pfn() - region.base_pfn());
    }

    #[test]
    fn test_region_aligned_page_count(page_aligned_size in 1usize..10000usize) {
        let size = page_aligned_size * PAGE_SIZE;
        let region = MemBlockRegion::new(0, size);
        assert_eq!(region.page_count(), page_aligned_size);
        assert_eq!(region.end_pfn() - region.base_pfn(), page_aligned_size);
    }

    #[test]
    fn test_flags_distinct(_v in 0u8..1u8) {
        let flags = [MemBlockFlags::NONE, MemBlockFlags::NOMAP, MemBlockFlags::MIRROR];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_ne!(flags[i], flags[j]);
            }
        }
    }

    #[test]
    fn test_flags_powers_of_two(_v in 0u8..1u8) {
        assert!(MemBlockFlags::NOMAP.0 == 1);
        assert!(MemBlockFlags::MIRROR.0 == 2);
    }

    #[test]
    fn test_region_contains_boundary(_v in 0u8..1u8) {
        let region = MemBlockRegion::new(4096, 8192);
        assert!(region.contains(4096));   // start inclusive
        assert!(!region.contains(12288));  // end exclusive
    }

    #[test]
    fn test_region_not_overlap(r1_base in 0usize..100_000usize, r2_base in 0usize..100_000usize) {
        let size = 4096usize;
        let r1 = MemBlockRegion::new(r1_base, size);
        let r2 = MemBlockRegion::new(r2_base, size);
        // Two regions with same size should not overlap if bases differ
        if r1_base != r2_base {
            // They may still overlap if bases are close
            let overlap = !(r1.end() <= r2.base || r2.end() <= r1.base);
            // Simple check: if |r1_base - r2_base| >= size, they don't overlap
            let no_overlap = (r1_base as isize - r2_base as isize).unsigned_abs() >= size;
            assert_eq!(overlap, !no_overlap);
        }
    }

    #[test]
    fn test_saturating_available(total in 0usize..1_000_000usize, reserved in 0usize..500_000usize) {
        let available = total.saturating_sub(reserved);
        assert!(available <= total);
    }
}
