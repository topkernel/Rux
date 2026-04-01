//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::fs::ext4::indirect::{Ext4BlockIterator, POINTERS_PER_BLOCK, max_file_size, get_indirect_level};
use super::{test_pass, test_fail, test_group_start};

pub fn test_ext4_file_write() {
    test_group_start("ext4 file write");

    let block_size: u64 = 4096;

    // ===== Block iterator tests =====

    // Test 1: POINTERS_PER_BLOCK
    test_assert_eq!(POINTERS_PER_BLOCK, 1024, "POINTERS_PER_BLOCK == 1024");

    // Test 2: Empty iterator (0 blocks)
    let mut iter = Ext4BlockIterator::new(0);
    test_assert_eq!(iter.next_mapping(), None, "Ext4BlockIterator::new(0) immediate None");

    // Test 3: Direct blocks only (12 blocks)
    let mut iter = Ext4BlockIterator::new(12);
    let mut all_direct = true;
    for i in 0..12 {
        match iter.next_mapping() {
            Some((level, offset)) if level == 0 && offset == i => {}
            _ => { all_direct = false; break; }
        }
    }
    test_assert!(all_direct, "12 blocks all direct (level=0)");
    test_assert_eq!(iter.next_mapping(), None, "Exhausted after 12 direct blocks");

    // Test 4: First single indirect block (block 12)
    let mut iter = Ext4BlockIterator::new(13);
    // Skip first 12 direct blocks
    for _ in 0..12 {
        iter.next_mapping();
    }
    match iter.next_mapping() {
        Some((level, offset)) if level == 1 && offset == 0 => {
            test_pass("block 13 is first single indirect");
        }
        other => {
            test_fail("block 13 level", "expected (1, 0)");
            let _ = other;
        }
    }

    // Test 5: Last single indirect block (block 1035 = 12 + 1023)
    let mut iter = Ext4BlockIterator::new(1036);
    for _ in 0..1035 {
        iter.next_mapping();
    }
    match iter.next_mapping() {
        Some((level, offset)) if level == 1 && offset == 1023 => {
            test_pass("block 1035 is last single indirect");
        }
        other => {
            test_fail("block 1035 level", "expected (1, 1023)");
            let _ = other;
        }
    }

    // Test 6: First double indirect block (block 1036)
    let mut iter = Ext4BlockIterator::new(1037);
    for _ in 0..1036 {
        iter.next_mapping();
    }
    match iter.next_mapping() {
        Some((level, offset)) if level == 2 && offset == 0 => {
            test_pass("block 1036 is first double indirect");
        }
        other => {
            test_fail("block 1036 level", "expected (2, 0)");
            let _ = other;
        }
    }

    // ===== Indirect level calculation tests =====

    // Test 7: get_indirect_level — small file (direct only)
    test_assert_eq!(get_indirect_level(48 * 1024, block_size), 0, "get_indirect_level 48KB == 0");

    // Test 8: get_indirect_level — medium file
    // 5MB = 5242880 bytes = 1280 blocks. 12+1024=1036 < 1280, so needs double indirect → level 2
    test_assert_eq!(get_indirect_level(5 * 1024 * 1024, block_size), 2, "get_indirect_level 5MB == 2");

    // Test 9: get_indirect_level — large file
    // 5GB = 5368709120 bytes = 1310720 blocks. 12+1024+1024*1024 = 1049652 < 1310720, so needs triple → level 3
    test_assert_eq!(get_indirect_level(5 * 1024 * 1024 * 1024, block_size), 3, "get_indirect_level 5GB == 3");

    // ===== File size calculations =====

    // Test 10: Direct block limit
    let direct_max = 12 * block_size;
    test_assert_eq!(direct_max, 49152, "direct block max == 49152");

    // Test 11: Single indirect limit
    let single_max = direct_max + (POINTERS_PER_BLOCK as u64) * block_size;
    test_assert!(single_max > 4 * 1024 * 1024, "single indirect max > 4MB");

    // Test 12: max_file_size
    let max_size = max_file_size(block_size);
    test_assert!(max_size > 0, "max_file_size > 0");

    // Test 13: Block alignment calculation
    let offset: u64 = 5000;
    let block_index = offset / block_size;
    let block_offset = (offset % block_size) as usize;
    test_assert!(block_index == 1 && block_offset == 904, "block alignment: 5000 → block 1 offset 904");

    // Test 14: Available space in first block
    let available = block_size as usize - 904;
    test_assert_eq!(available, 3192, "available in first block after offset 904");
}
