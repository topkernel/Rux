//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for page offset/index arithmetic.
//! Copied from: kernel/src/fs/buffer.rs (AddressSpace read/write logic)

use proptest::prelude::*;

pub const PAGE_SIZE: usize = 4096;

// Extracted pure functions from AddressSpace::read/write
pub fn page_index(offset: usize) -> usize {
    offset / PAGE_SIZE
}

pub fn page_offset(offset: usize) -> usize {
    offset % PAGE_SIZE
}

pub fn copy_len(data_len: usize, page_offset: usize) -> usize {
    let available = PAGE_SIZE - page_offset;
    core::cmp::min(data_len, available)
}

// Page-aligned boundary calculations
pub fn page_align_down(offset: usize) -> usize {
    offset & !(PAGE_SIZE - 1)
}

pub fn page_align_up(offset: usize) -> usize {
    (offset + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

pub fn is_page_aligned(offset: usize) -> bool {
    (offset & (PAGE_SIZE - 1)) == 0
}

proptest! {
    #[test]
    fn test_page_index_roundtrip(offset in 0usize..1_000_000usize) {
        let idx = page_index(offset);
        let off = page_offset(offset);
        assert_eq!(idx * PAGE_SIZE + off, offset);
    }

    #[test]
    fn test_page_index_monotone(offset in 0usize..1_000_000usize) {
        assert!(page_index(offset + 1) >= page_index(offset));
    }

    #[test]
    fn test_page_offset_in_range(offset in 0usize..1_000_000usize) {
        let off = page_offset(offset);
        assert!(off < PAGE_SIZE);
    }

    #[test]
    fn test_page_offset_zero_for_aligned(offset in 1usize..100usize) {
        let aligned = offset * PAGE_SIZE;
        assert_eq!(page_offset(aligned), 0);
    }

    #[test]
    fn test_copy_len_no_overflow(data_len in 0usize..100_000usize, page_off in 0usize..PAGE_SIZE) {
        let len = copy_len(data_len, page_off);
        assert!(len <= data_len);
        assert!(len <= PAGE_SIZE - page_off);
    }

    #[test]
    fn test_copy_len_at_end_of_page(data_len in 0usize..100_000usize) {
        let page_off = PAGE_SIZE - 1;
        let len = copy_len(data_len, page_off);
        assert!(len <= 1);
    }

    #[test]
    fn test_copy_len_at_start_of_page(data_len in 0usize..100_000usize) {
        let page_off = 0;
        let len = copy_len(data_len, page_off);
        assert_eq!(len, core::cmp::min(data_len, PAGE_SIZE));
    }

    #[test]
    fn test_page_align_down(offset in 0usize..1_000_000usize) {
        let aligned = page_align_down(offset);
        assert!(aligned <= offset);
        assert!(is_page_aligned(aligned));
    }

    #[test]
    fn test_page_align_up(offset in 0usize..1_000_000usize) {
        let aligned = page_align_up(offset);
        assert!(aligned >= offset);
        assert!(is_page_aligned(aligned));
    }

    #[test]
    fn test_align_down_up_diff(offset in 1usize..1_000_000usize) {
        let down = page_align_down(offset);
        let up = page_align_up(offset);
        // If offset is already aligned, down == up
        if is_page_aligned(offset) {
            assert_eq!(down, up);
            assert_eq!(down, offset);
        } else {
            assert!(up > down);
            assert_eq!(up - down, PAGE_SIZE);
        }
    }

    #[test]
    fn test_page_index_boundary(offset in 0usize..PAGE_SIZE) {
        assert_eq!(page_index(offset), 0);
    }

    #[test]
    fn test_page_index_after_one_page(offset in PAGE_SIZE..PAGE_SIZE*2) {
        assert_eq!(page_index(offset), 1);
    }

    #[test]
    fn test_copy_len_exact_page(data_len in PAGE_SIZE..PAGE_SIZE*2) {
        let len = copy_len(data_len, 0);
        assert_eq!(len, PAGE_SIZE);
    }
}
