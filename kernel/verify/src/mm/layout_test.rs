//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for kernel memory layout arithmetic.
//! Copied from: kernel/src/mm/layout.rs

use proptest::prelude::*;

pub const PAGE_SIZE: usize = 4096;
pub const PHYS_MEMORY_BASE: usize = 0x80000000;
pub const DEFAULT_HEAP_SIZE: usize = 32 * 1024 * 1024;
pub const DEFAULT_SLAB_SIZE: usize = 4 * 1024 * 1024;

// Copied KernelMemoryLayout
#[derive(Clone, Copy)]
pub struct KernelMemoryLayout {
    pub phys_base: usize,
    pub phys_size: usize,
    pub kernel_start: usize,
    pub kernel_end: usize,
    pub heap_start: usize,
    pub heap_size: usize,
    pub slab_start: usize,
    pub slab_size: usize,
    pub user_phys_start: usize,
    pub user_phys_size: usize,
    pub frame_alloc_start: usize,
    pub frame_alloc_size: usize,
}

impl KernelMemoryLayout {
    pub fn init_from_memblock(
        phys_base: usize,
        phys_size: usize,
        kernel_start: usize,
        kernel_end: usize,
    ) -> Self {
        let heap_start = (kernel_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let heap_size = DEFAULT_HEAP_SIZE;

        let slab_start = heap_start + heap_size;
        let slab_size = DEFAULT_SLAB_SIZE;

        let remaining_after_slab = phys_base + phys_size - slab_start - slab_size;
        let user_phys_size = (remaining_after_slab / 4).min(64 * 1024 * 1024);
        let user_phys_start = slab_start + slab_size;

        let frame_alloc_start = user_phys_start + user_phys_size;
        let frame_alloc_size = phys_base + phys_size - frame_alloc_start;

        Self {
            phys_base, phys_size, kernel_start, kernel_end,
            heap_start, heap_size, slab_start, slab_size,
            user_phys_start, user_phys_size,
            frame_alloc_start, frame_alloc_size,
        }
    }
}

fn page_aligned(addr: usize) -> bool {
    addr % PAGE_SIZE == 0
}

proptest! {
    #[test]
    fn test_heap_start_page_aligned(kernel_end in 0x80200000usize..0x80A00000usize) {
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, 512 * 1024 * 1024, 0x80200000, kernel_end
        );
        assert!(page_aligned(layout.heap_start),
            "heap_start {:#x} not page-aligned", layout.heap_start);
    }

    #[test]
    fn test_slab_follows_heap(kernel_end in 0x80200000usize..0x80A00000usize) {
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, 512 * 1024 * 1024, 0x80200000, kernel_end
        );
        assert_eq!(layout.slab_start, layout.heap_start + layout.heap_size);
        assert_eq!(layout.slab_size, DEFAULT_SLAB_SIZE);
    }

    #[test]
    fn test_user_phys_capped(kernel_end in 0x80200000usize..0x80A00000usize) {
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, 2048 * 1024 * 1024, 0x80200000, kernel_end
        );
        assert!(layout.user_phys_size <= 64 * 1024 * 1024,
            "user_phys_size {} exceeds 64MB cap", layout.user_phys_size);
    }

    #[test]
    fn test_user_phys_is_quarter_rule(kernel_end in 0x80200000usize..0x80A00000usize) {
        let phys_size = 512 * 1024 * 1024usize;
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, phys_size, 0x80200000, kernel_end
        );
        let remaining = PHYS_MEMORY_BASE + phys_size - layout.slab_start - layout.slab_size;
        let quarter = remaining / 4;
        // user_phys_size is min(quarter, 64MB)
        if quarter < 64 * 1024 * 1024 {
            assert_eq!(layout.user_phys_size, quarter);
        } else {
            assert_eq!(layout.user_phys_size, 64 * 1024 * 1024);
        }
    }

    #[test]
    fn test_frame_alloc_accounts_for_all(
        kernel_end in 0x80200000usize..0x80A00000usize,
        phys_size in 128 * 1024 * 1024usize..2048 * 1024 * 1024usize
    ) {
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, phys_size, 0x80200000, kernel_end
        );
        // frame_alloc_end must equal phys_base + phys_size
        let frame_alloc_end = layout.frame_alloc_start + layout.frame_alloc_size;
        prop_assume!(frame_alloc_end <= usize::MAX / 2); // avoid overflow
        assert_eq!(frame_alloc_end, PHYS_MEMORY_BASE + phys_size,
            "frame alloc doesn't cover all memory");
    }

    #[test]
    fn test_regions_contiguous(kernel_end in 0x80200000usize..0x80A00000usize) {
        let phys_size = 512 * 1024 * 1024usize;
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, phys_size, 0x80200000, kernel_end
        );
        // slab follows heap
        assert_eq!(layout.slab_start, layout.heap_start + layout.heap_size);
        // user follows slab
        assert_eq!(layout.user_phys_start, layout.slab_start + layout.slab_size);
        // frame follows user
        assert_eq!(layout.frame_alloc_start, layout.user_phys_start + layout.user_phys_size);
    }

    #[test]
    fn test_frame_alloc_non_negative(
        kernel_end in 0x80200000usize..0x80A00000usize,
        phys_size in 128 * 1024 * 1024usize..4096 * 1024 * 1024usize
    ) {
        let layout = KernelMemoryLayout::init_from_memblock(
            PHYS_MEMORY_BASE, phys_size, 0x80200000, kernel_end
        );
        // For reasonably large phys_size, frame_alloc_size should be >= 0
        assert!(layout.frame_alloc_start >= layout.user_phys_start);
        // frame_alloc_start + frame_alloc_size = phys_base + phys_size
        let end = layout.frame_alloc_start + layout.frame_alloc_size;
        assert!(end >= layout.frame_alloc_start);
    }

    #[test]
    fn test_default_sizes(_v in 0u8..1u8) {
        assert_eq!(DEFAULT_HEAP_SIZE, 32 * 1024 * 1024);
        assert_eq!(DEFAULT_SLAB_SIZE, 4 * 1024 * 1024);
        assert!(DEFAULT_HEAP_SIZE % PAGE_SIZE == 0);
        assert!(DEFAULT_SLAB_SIZE % PAGE_SIZE == 0);
    }
}
