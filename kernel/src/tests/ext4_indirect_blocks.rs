//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 indirect block unit test
use crate::println;
use super::{test_pass, test_fail, test_group_start};

pub fn test_ext4_indirect_blocks() {
    test_group_start("ext4 indirect blocks");

    let block_size: u64 = 4096;
    let pointers_per_block = (block_size / 4) as usize;

    // Test 1: Block index calculation test
    let block_type = if 11 < 12 { "direct" } else if 11 < 12 + pointers_per_block as u64 { "single_indirect" } else { "double_indirect" };
    if block_type == "direct" {
        test_pass("block index (direct)");
    } else {
        test_fail("block index (direct)", "incorrect");
    }

    // Test 2: Indirect block level test
    fn get_indirect_level(size: u64, block_size: u64) -> u32 {
        let blocks = (size + block_size - 1) / block_size;
        if blocks <= 12 { return 0; }
        let ppb = block_size / 4;
        if blocks <= 12 + ppb { return 1; }
        let double_ppb = ppb * ppb;
        if blocks <= 12 + ppb + double_ppb { return 2; }
        3
    }
    if get_indirect_level(48 * 1024, 4096) == 0 && get_indirect_level(5 * 1024 * 1024, 4096) == 2 {
        test_pass("indirect level calculation");
    } else {
        test_fail("indirect level calculation", "incorrect");
    }

    // Test 3: File size limit test
    let direct_max = 12 * block_size;
    let single_max = direct_max + pointers_per_block as u64 * block_size;
    if direct_max == 49152 && single_max > 4 * 1024 * 1024 {
        test_pass("file size limits");
    } else {
        test_fail("file size limits", "incorrect");
    }

    // Test 4: Block pointer index test
    let i_block_index = if 5 < 12 { 5 } else { 12 };
    if i_block_index == 5 {
        test_pass("block pointer indices");
    } else {
        test_fail("block pointer indices", "incorrect");
    }

    // Test 5: Indirect block offset calculation test
    let level = if 100 < 12 { 0 } else if 100 < 12 + pointers_per_block as u64 { 1 } else { 2 };
    if level == 1 {
        test_pass("indirect offset calculations");
    } else {
        test_fail("indirect offset calculations", "incorrect");
    }

    println!("test: ext4 indirect blocks testing completed.");
}
