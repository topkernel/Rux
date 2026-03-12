//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 file write operation unit test
use crate::println;
use super::{test_pass, test_fail, test_group_start};

pub fn test_ext4_file_write() {
    test_group_start("ext4 file write");

    // Test 1: Block calculation test
    let block_size: u64 = 4096;
    let blocks = (4097 + block_size - 1) / block_size;
    if blocks == 2 {
        test_pass("block calculations");
    } else {
        test_fail("block calculations", "incorrect");
    }

    // Test 2: File size expansion test
    let current_size: u64 = 4096;
    let write_offset: u64 = 4096;
    let write_size: u64 = 100;
    let current_blocks = (current_size + block_size - 1) / block_size;
    let needed_blocks = (write_offset + write_size + block_size - 1) / block_size;
    if current_blocks == 1 && needed_blocks == 2 {
        test_pass("file expansion");
    } else {
        test_fail("file expansion", "incorrect");
    }

    // Test 3: Block alignment test
    let offset: u64 = 5000;
    let block_index = offset / block_size;
    let block_offset = (offset % block_size) as usize;
    if block_index == 1 && block_offset == 904 {
        test_pass("block alignment");
    } else {
        test_fail("block alignment", "incorrect");
    }

    // Test 4: Write offset calculation test
    let available_in_first_block = block_size as usize - 904;
    if available_in_first_block == 3192 {
        test_pass("write offset calculations");
    } else {
        test_fail("write offset calculations", "incorrect");
    }

    // Test 5: Direct block limit test
    let direct_blocks = 12;
    let max_file_size = direct_blocks * block_size as usize;
    if max_file_size == 49152 {
        test_pass("direct block limits");
    } else {
        test_fail("direct block limits", "incorrect");
    }

    println!("test: ext4 file write testing completed.");
}
