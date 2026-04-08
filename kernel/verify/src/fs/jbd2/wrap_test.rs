//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for JBD2 wrap_block arithmetic and journal space calculations.
//! Copied from: kernel/src/fs/jbd2/recovery.rs, kernel/src/fs/jbd2/commit.rs, kernel/src/fs/jbd2/checkpoint.rs

use proptest::prelude::*;

// Copied from recovery.rs and commit.rs
#[inline]
pub fn wrap_block(block: u64, first: u64, last: u64) -> u64 {
    let n = block + 1;
    if n >= last { first } else { n }
}

// Copied from commit.rs (identical logic)
#[inline]
pub fn wrap_journal_block(mut block: u64, first: u64, last: u64) -> u64 {
    block += 1;
    if block >= last { block = first; }
    block
}

// Copied from checkpoint.rs: freed space calculation
pub fn update_log_tail_freed(old_tail: u64, blocknr: u64, first: u64, last: u64) -> i64 {
    let mut freed = blocknr as i64 - old_tail as i64;
    if blocknr < old_tail {
        freed += (last - first) as i64;
    }
    freed
}

// Copied from checkpoint.rs: log space left
pub fn log_space_left(free: i32, reserved: i32) -> i32 {
    (free - reserved).max(0)
}

// Copied from types.rs: journal_tag_size
pub const fn journal_tag_size(has_64bit: bool, has_csum_v3: bool) -> usize {
    if has_csum_v3 {
        16 // size_of::<journal_block_tag3_t>()
    } else if has_64bit {
        12 // size_of::<journal_block_tag_t>()
    } else {
        8  // size_of::<journal_block_tag_t>() - size_of::<u32>()
    }
}

// Copied from types.rs: journal_tags_per_block
pub const fn journal_tags_per_block(block_size: u32, tag_size: usize) -> usize {
    let header_size = 12usize; // size_of::<journal_header_t>()
    let tail_size = 4usize;   // size_of::<journal_block_tail_t>()
    let count = (block_size as usize - header_size - tail_size) / tag_size;
    if count < 1 { 1 } else { count }
}

// Copied commit arithmetic: ceil division for descriptor blocks
pub fn ceil_div(n: usize, d: usize) -> usize {
    (n + d - 1) / d
}

proptest! {
    #[test]
    fn test_wrap_block_in_range(first in 0u64..100u64, span in 10u64..1000u64) {
        let last = first + span;
        // Only test with block in valid range [first, last)
        for block in first..last {
            let result = wrap_block(block, first, last);
            assert!(result >= first && result < last,
                "block={} first={} last={} result={}", block, first, last, result);
        }
    }

    #[test]
    fn test_wrap_block_advances(block in 0u64..1000u64, first in 0u64..100u64, span in 10u64..1000u64) {
        let last = first + span;
        let block = block % span + first;
        let result = wrap_block(block, first, last);
        if block < last - 1 {
            assert_eq!(result, block + 1);
        } else {
            // Wraps to first
            assert_eq!(result, first);
        }
    }

    #[test]
    fn test_wrap_block_at_last_minus_one(first in 0u64..100u64, span in 10u64..1000u64) {
        let last = first + span;
        let block = last - 1;
        assert_eq!(wrap_block(block, first, last), first);
    }

    #[test]
    fn test_wrap_journal_block_matches_wrap_block(
        block in 0u64..1000u64, first in 0u64..100u64, span in 10u64..1000u64
    ) {
        let last = first + span;
        let block = block % span + first;
        assert_eq!(
            wrap_journal_block(block, first, last),
            wrap_block(block, first, last),
            "wrap_journal_block should match wrap_block"
        );
    }

    #[test]
    fn test_log_space_left_clamps(free in 0i32..1000i32, reserved in 0i32..1000i32) {
        let result = log_space_left(free, reserved);
        assert!(result >= 0);
        if free >= reserved {
            assert_eq!(result, free - reserved);
        } else {
            assert_eq!(result, 0);
        }
    }

    #[test]
    fn test_log_space_left_zero_when_over_reserved(reserved in 1i32..1000i32) {
        assert_eq!(log_space_left(0, reserved), 0);
    }

    #[test]
    fn test_update_log_tail_freed_no_wrap(old_tail in 0u64..1000u64, delta in 1u64..1000u64) {
        let blocknr = old_tail + delta;
        let freed = update_log_tail_freed(old_tail, blocknr, 0, 10000);
        assert_eq!(freed, delta as i64);
    }

    #[test]
    fn test_update_log_tail_freed_with_wrap(first in 0u64..100u64, span in 100u64..1000u64) {
        let last = first + span;
        let old_tail = last - 50; // near end
        let blocknr = first + 30;  // wrapped to beginning
        let freed = update_log_tail_freed(old_tail, blocknr, first, last);
        // freed = (first+30) - (last-50) + (last - first) = first+30-last+50+last-first = 80
        assert_eq!(freed, 80);
    }

    #[test]
    fn test_update_log_tail_same_block(old_tail in 0u64..1000u64) {
        let freed = update_log_tail_freed(old_tail, old_tail, 0, 10000);
        assert_eq!(freed, 0);
    }

    #[test]
    fn test_tag_size_combinations(_v in 0u8..1u8) {
        assert_eq!(journal_tag_size(false, false), 8);
        assert_eq!(journal_tag_size(true, false), 12);
        assert_eq!(journal_tag_size(false, true), 16);
        assert_eq!(journal_tag_size(true, true), 16);
    }

    #[test]
    fn test_tags_per_block_minimum_1(block_size in 17u32..64u32) {
        // Block sizes where header(12) + tail(4) = 16, barely fits 1 tag
        let result = journal_tags_per_block(block_size, 16);
        assert!(result >= 1);
    }

    #[test]
    fn test_tags_per_block_4k(_v in 0u8..1u8) {
        // 4096 - 12 (header) - 4 (tail) = 4080
        // 4080 / 8 = 510 tags (v1 no-64bit)
        assert_eq!(journal_tags_per_block(4096, 8), 510);
        // 4080 / 12 = 340 tags (v1 64bit)
        assert_eq!(journal_tags_per_block(4096, 12), 340);
        // 4080 / 16 = 255 tags (v3)
        assert_eq!(journal_tags_per_block(4096, 16), 255);
    }

    #[test]
    fn test_tags_per_block_monotone(block_size in 512u32..8192u32) {
        let t8 = journal_tags_per_block(block_size, 8);
        let t12 = journal_tags_per_block(block_size, 12);
        let t16 = journal_tags_per_block(block_size, 16);
        assert!(t8 >= t12, "smaller tag -> more tags");
        assert!(t12 >= t16, "smaller tag -> more tags");
    }

    #[test]
    fn test_ceil_div(n in 0usize..10000usize, d in 1usize..1000usize) {
        let result = ceil_div(n, d);
        assert!(result * d >= n, "ceil_div too small");
        assert!(result <= n + d, "ceil_div too large");
        if n % d == 0 {
            assert_eq!(result, n / d);
        } else {
            assert_eq!(result, n / d + 1);
        }
    }

    #[test]
    fn test_desc_blocks_formula(num_buffers in 1usize..10000usize, tags_per_block in 1usize..1000usize) {
        let desc_blocks = ceil_div(num_buffers, tags_per_block);
        let total = desc_blocks + num_buffers + 1;
        assert!(total >= num_buffers + 2, "need at least 1 desc + 1 commit + buffers");
    }
}
