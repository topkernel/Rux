//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for vmemmap layout invariants.
//! Copied from: kernel/src/mm/vmemmap.rs

use proptest::prelude::*;

// From kernel: struct Page is #[repr(C, align(64))] with 8 fields = 64 bytes
pub const PAGE_SIZE: usize = 4096;
pub const STRUCT_PAGE_SIZE: usize = 64; // kernel struct Page is 64 bytes
pub const PAGES_PER_VMEMMAP_PAGE: usize = PAGE_SIZE / STRUCT_PAGE_SIZE; // 64

// Simulated vmemmap arithmetic (with known start_pfn)
fn pfn_to_vmemmap(pfn: usize, start_pfn: usize, vmemmap_start: usize) -> usize {
    vmemmap_start + (pfn - start_pfn) * STRUCT_PAGE_SIZE
}

fn vmemmap_to_pfn(vaddr: usize, start_pfn: usize, vmemmap_start: usize) -> usize {
    start_pfn + (vaddr - vmemmap_start) / STRUCT_PAGE_SIZE
}

proptest! {
    #[test]
    fn test_struct_page_size_is_64(_v in 0u8..1u8) {
        // struct Page in kernel is #[repr(C, align(64))] = 64 bytes
        assert_eq!(STRUCT_PAGE_SIZE, 64, "struct Page must be 64 bytes");
    }

    #[test]
    fn test_pages_per_vmemmap_page(_v in 0u8..1u8) {
        assert_eq!(PAGES_PER_VMEMMAP_PAGE, 64, "One 4KB page holds 64 page descriptors");
        assert_eq!(PAGES_PER_VMEMMAP_PAGE * STRUCT_PAGE_SIZE, PAGE_SIZE);
    }

    #[test]
    fn test_vmemmap_roundtrip(pfn in 0x80000usize..0x90000usize) {
        let start_pfn = 0x80000usize;
        let vmemmap_start = 0xFFFFC00000000000usize;
        let vaddr = pfn_to_vmemmap(pfn, start_pfn, vmemmap_start);
        let pfn_back = vmemmap_to_pfn(vaddr, start_pfn, vmemmap_start);
        prop_assert_eq!(pfn, pfn_back, "pfn→vmemmap→pfn round-trip failed");
    }

    #[test]
    fn test_vmemmap_pfn_roundtrip(vaddr_offset in 0usize..100_000usize) {
        let start_pfn = 0x80000usize;
        let vmemmap_start = 0xFFFFC00000000000usize;
        let vaddr = vmemmap_start + vaddr_offset * STRUCT_PAGE_SIZE;
        let pfn = vmemmap_to_pfn(vaddr, start_pfn, vmemmap_start);
        let vaddr_back = pfn_to_vmemmap(pfn, start_pfn, vmemmap_start);
        prop_assert_eq!(vaddr, vaddr_back, "vmemmap→pfn→vmemmap round-trip failed");
    }

    #[test]
    fn test_vmemmap_pages_needed(nr_pages in 1usize..10_000_000usize) {
        let vmemmap_pages = (nr_pages + PAGES_PER_VMEMMAP_PAGE - 1) / PAGES_PER_VMEMMAP_PAGE;
        // Should cover all pages
        assert!(vmemmap_pages * PAGES_PER_VMEMMAP_PAGE >= nr_pages);
        // Removing one page should not be enough
        if vmemmap_pages > 1 {
            assert!((vmemmap_pages - 1) * PAGES_PER_VMEMMAP_PAGE < nr_pages);
        }
    }

    #[test]
    fn test_vmemmap_alignment(vaddr_offset in 0usize..10_000usize) {
        let vmemmap_start = 0xFFFFC00000000000usize;
        let vaddr = vmemmap_start + vaddr_offset * STRUCT_PAGE_SIZE;
        // Each page descriptor should be STRUCT_PAGE_SIZE-aligned
        assert_eq!(vaddr % STRUCT_PAGE_SIZE, 0);
    }
}
