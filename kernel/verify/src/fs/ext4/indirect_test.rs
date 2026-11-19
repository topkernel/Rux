//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 indirect block mapping invariant tests.
//!
//! Types copied from: kernel/src/fs/ext4/indirect.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/ext4/indirect.rs
// ============================================================================

pub const POINTERS_PER_BLOCK: usize = 1024;

pub struct Ext4BlockIterator {
    pub current_block: u64,
    pub total_blocks: u64,
}

impl Ext4BlockIterator {
    pub fn new(total_blocks: u64) -> Self {
        Self {
            current_block: 0,
            total_blocks,
        }
    }

    pub fn next_mapping(&mut self) -> Option<(u32, usize)> {
        if self.current_block >= self.total_blocks {
            return None;
        }

        let block = self.current_block;
        self.current_block += 1;

        if block < 12 {
            return Some((0, block as usize));
        }

        let indirect = block - 12;
        if indirect < POINTERS_PER_BLOCK as u64 {
            return Some((1, indirect as usize));
        }

        let double = indirect - POINTERS_PER_BLOCK as u64;
        if double < (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK) as u64 {
            return Some((2, double as usize));
        }

        let triple = double - (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK) as u64;
        Some((3, triple as usize))
    }
}

pub fn max_file_size(block_size: u64) -> u64 {
    let pointers_per_block = block_size / 4;
    let direct = 12 * block_size;
    let single = pointers_per_block * block_size;
    let double = pointers_per_block * pointers_per_block * block_size;
    let triple = pointers_per_block * pointers_per_block * pointers_per_block * block_size;
    direct + single + double + triple
}

pub fn get_indirect_level(size: u64, block_size: u64) -> u32 {
    let blocks = (size + block_size - 1) / block_size;

    if blocks <= 12 {
        return 0;
    }

    let pointers_per_block = block_size / 4;

    if blocks <= 12 + pointers_per_block {
        return 1;
    }

    let double_pointers = pointers_per_block * pointers_per_block;

    if blocks <= 12 + pointers_per_block + double_pointers {
        return 2;
    }

    3
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-BLOCK-1: Direct blocks (0-11) map to level 0
    #[test]
    fn test_direct_blocks(block in 0u64..12u64) {
        let mut iter = Ext4BlockIterator::new(20);
        // Advance to the target block
        for _ in 0..block {
            iter.next_mapping();
        }
        let (level, offset) = iter.next_mapping().unwrap();
        prop_assert_eq!(level, 0);
        prop_assert_eq!(offset, block as usize);
    }

    /// INV-BLOCK-2: Single indirect blocks (12-1035) map to level 1
    #[test]
    fn test_single_indirect(offset in 0u64..1024u64) {
        let block = 12 + offset;
        let mut iter = Ext4BlockIterator::new(block + 1);
        for _ in 0..block {
            iter.next_mapping();
        }
        let (level, off) = iter.next_mapping().unwrap();
        prop_assert_eq!(level, 1);
        prop_assert_eq!(off, offset as usize);
    }

    /// INV-BLOCK-3: next_mapping returns None after total_blocks
    #[test]
    fn test_iteration_count(total in 0u64..2000u64) {
        let mut iter = Ext4BlockIterator::new(total);
        let mut count = 0u64;
        while iter.next_mapping().is_some() {
            count += 1;
        }
        prop_assert_eq!(count, total);
    }

    /// INV-BLOCK-4: max_file_size(4096) is reasonable
    #[test]
    fn test_max_file_size_4k(_v in 0u8..1u8) {
        let size = max_file_size(4096);
        prop_assert!(size > 4_000_000_000_000u64); // > 4TB
    }

    /// INV-BLOCK-5: get_indirect_level for direct
    #[test]
    fn test_indirect_level_direct(size in 1u64..12u64) {
        let level = get_indirect_level(size * 4096, 4096);
        prop_assert_eq!(level, 0);
    }

    /// INV-BLOCK-6: get_indirect_level for single indirect
    #[test]
    fn test_indirect_level_single(offset in 0u64..1024u64) {
        let blocks = 13 + offset;
        let level = get_indirect_level(blocks * 4096, 4096);
        prop_assert_eq!(level, 1);
    }

    /// INV-BLOCK-7: get_indirect_level for double indirect
    #[test]
    fn test_indirect_level_double(offset in 0u64..1000u64) {
        let blocks = 12 + 1024 + 1 + offset;
        let level = get_indirect_level(blocks * 4096, 4096);
        prop_assert_eq!(level, 2);
    }

    /// INV-BLOCK-8: Block 12 boundary
    #[test]
    fn test_boundary_12(_v in 0u8..1u8) {
        let mut iter = Ext4BlockIterator::new(13);
        // Advance to block 11
        for _ in 0..11 {
            iter.next_mapping();
        }
        // Block 11: level 0
        let (l11, _) = iter.next_mapping().unwrap();
        prop_assert_eq!(l11, 0);
        // Block 12: level 1, offset 0
        let (l12, o12) = iter.next_mapping().unwrap();
        prop_assert_eq!(l12, 1);
        prop_assert_eq!(o12, 0);
    }

    /// INV-BLOCK-9: max_file_size with various block sizes
    #[test]
    fn test_max_file_size_monotone(
        bs1 in 512u64..4096u64,
        bs2 in 512u64..4096u64,
    ) {
        let (small, large) = if bs1 <= bs2 { (bs1, bs2) } else { (bs2, bs1) };
        prop_assert!(max_file_size(small) <= max_file_size(large));
    }

    /// INV-BLOCK-10: get_indirect_level is monotone
    #[test]
    fn test_indirect_level_monotone(
        s1 in 1u64..100_000u64,
        s2 in 1u64..100_000u64,
    ) {
        let (small, large) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
        let l1 = get_indirect_level(small, 4096);
        let l2 = get_indirect_level(large, 4096);
        prop_assert!(l1 <= l2);
    }
}
