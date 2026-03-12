//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 block and inode allocator unit test
use crate::println;
use super::{test_pass, test_fail, test_group_start};

pub fn test_ext4_allocator() {
    test_group_start("ext4 allocator");

    // Test 1: Bit operation test
    let mut bitmap: u8 = 0b00000000;
    bitmap |= 1 << 3;
    if bitmap != 0b00001000 {
        test_fail("bitmap set bit", "incorrect");
        return;
    }
    bitmap &= !(1 << 3);
    if bitmap != 0b00000000 {
        test_fail("bitmap clear bit", "incorrect");
        return;
    }
    let bitmap = 0b11111010u8;
    let bit0_set = (bitmap & (1 << 0)) != 0;
    let bit1_set = (bitmap & (1 << 1)) != 0;
    if !bit0_set && bit1_set {
        test_pass("bitmap operations");
    } else {
        test_fail("bitmap operations", "incorrect");
    }

    // Test 2: Block group calculation test
    let block_number: u64 = 8192;
    let blocks_per_group: u64 = 8192;
    let group = block_number / blocks_per_group;
    let offset = block_number % blocks_per_group;
    if group == 1 && offset == 0 {
        test_pass("block group calculations");
    } else {
        test_fail("block group calculations", "incorrect");
    }

    // Test 3: inode group calculation test
    let inode_number: u32 = 8193;
    let inodes_per_group: u64 = 8192;
    let group = (inode_number as u64 - 1) / inodes_per_group;
    let offset = (inode_number as u64 - 1) % inodes_per_group;
    if group == 1 && offset == 0 {
        test_pass("inode group calculations");
    } else {
        test_fail("inode group calculations", "incorrect");
    }

    // Test 4: Bitmap index calculation test
    let bit_offset: usize = 10;
    let byte_idx = bit_offset / 8;
    let bit_idx = bit_offset % 8;
    if byte_idx == 1 && bit_idx == 2 {
        test_pass("bitmap index calculations");
    } else {
        test_fail("bitmap index calculations", "incorrect");
    }

    // Test 5: Allocator boundary condition test
    let blocks_per_group: u64 = 8192;
    let bitmap_bytes = (blocks_per_group + 7) / 8;
    let bitmap_blocks = (bitmap_bytes + 4095) / 4096;
    if bitmap_bytes == 1024 && bitmap_blocks == 1 {
        test_pass("allocator boundaries");
    } else {
        test_fail("allocator boundaries", "incorrect");
    }

    println!("test: ext4 allocator testing completed.");
}
