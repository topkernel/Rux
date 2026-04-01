//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::fs::ext4::allocator::find_free_bit;
use super::{test_pass, test_fail, test_group_start};

pub fn test_ext4_allocator() {
    test_group_start("ext4 allocator");

    // Test 1: Empty bitmap — first bit free
    test_assert_eq!(find_free_bit(&[0x00], 0, 8), Some(0), "find_free_bit all-zero bitmap");

    // Test 2: Full bitmap — no free bit
    test_assert_eq!(find_free_bit(&[0xFF], 0, 8), None, "find_free_bit all-ones bitmap");

    // Test 3: Lower nibble occupied
    test_assert_eq!(find_free_bit(&[0x0F], 0, 8), Some(4), "find_free_bit 0x0F");

    // Test 4: Two bytes — first full, second has free
    test_assert_eq!(find_free_bit(&[0xFF, 0x00], 0, 16), Some(8), "find_free_bit two-byte first free");

    // Test 5: Two bytes — second nibble free
    test_assert_eq!(find_free_bit(&[0xFF, 0x0F], 0, 16), Some(12), "find_free_bit two-byte nibble");

    // Test 6: start parameter — skip first 3 bits
    test_assert_eq!(find_free_bit(&[0x07], 3, 8), Some(3), "find_free_bit start=3 in 0x07");

    // Test 7: start parameter — skip past first byte
    test_assert_eq!(find_free_bit(&[0xFF, 0x00], 8, 16), Some(8), "find_free_bit start=8");

    // Test 8: max_bits limits search — bits 0-2 free but max_bits=3 means search 0..2
    test_assert_eq!(find_free_bit(&[0x07], 0, 3), None, "find_free_bit max_bits=3 in 0x07");

    // Test 9: Empty bitmap (zero length)
    test_assert_eq!(find_free_bit(&[], 0, 0), None, "find_free_bit empty bitmap");

    // Test 10: Large bitmap — all full
    let full: [u8; 100] = [0xFF; 100];
    test_assert_eq!(find_free_bit(&full, 0, 800), None, "find_free_bit large full bitmap");

    // Test 11: Large bitmap — single free bit in middle
    let mut almost_full = [0xFFu8; 100];
    almost_full[50] = 0xFE; // bit 0 of byte 50 is free (bit 400)
    test_assert_eq!(find_free_bit(&almost_full, 0, 800), Some(400), "find_free_bit large single free");

    // Test 12: Large bitmap — free bit after start offset
    let mut data = [0xFFu8; 100];
    data[3] = 0x00; // byte 3 fully free (bits 24-31)
    test_assert_eq!(find_free_bit(&data, 0, 800), Some(24), "find_free_bit free byte at 3");
    test_assert_eq!(find_free_bit(&data, 30, 800), Some(30), "find_free_bit start=30 within free byte");

    // Test 13: Bitmap arithmetic — block group calculation
    let block_number: u64 = 8192;
    let blocks_per_group: u64 = 8192;
    let group = block_number / blocks_per_group;
    let offset = block_number % blocks_per_group;
    test_assert!(group == 1 && offset == 0, "block group calculation");

    // Test 14: Inode group calculation
    let inode_number: u32 = 8193;
    let inodes_per_group: u64 = 8192;
    let group = (inode_number as u64 - 1) / inodes_per_group;
    let offset = (inode_number as u64 - 1) % inodes_per_group;
    test_assert!(group == 1 && offset == 0, "inode group calculation");
}
