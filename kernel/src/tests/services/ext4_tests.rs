//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The ext4 crate's guest tests (the service-test seam): the modules
//! that lived framework-side (tests/ext4_allocator.rs,
//! tests/ext4_file_write.rs) plus the ext4 halves of tests/fs_pins.rs
//! (csum + indirect pins) — flipped WITH the crate at rung 5. The
//! framework's run_all_tests invokes run_all through
//! rux::tests::register_service_tests (bin wires it); pass/fail
//! lands in the framework's counters.

pub mod ext4_allocator;
pub mod ext4_file_write;

use crate::fs::ext4::csum::{
    crc32c, group_desc_csum_ok, group_desc_csum_set, inode_csum_ok, inode_csum_set,
    superblock_csum_ok, superblock_csum_set, CRC32C_TABLE,
};
use crate::fs::ext4::indirect::{get_indirect_level, max_file_size, Ext4BlockIterator};

pub fn run_all() {
    rux::tests::test_group_start("ext4_allocator");
    ext4_allocator::test_ext4_allocator();

    rux::tests::test_group_start("ext4_file_write");
    ext4_file_write::test_ext4_file_write();

    rux::tests::test_group_start("ext4_pins");
    test_ext4_csum_pins();
    test_ext4_indirect_pins();
}

fn test_ext4_csum_pins() {
    // Castagnoli table prefix.
    rux::test_assert_eq!(CRC32C_TABLE[0], 0, "crc32c table[0]");
    // The dead block pinned table[1] == 0x82F6_3B78 — that is the POLYNOMIAL
    // constant, not the table entry. The standard reflected Castagnoli
    // table has T[1] = 0xF26B8303 / T[255] = 0xAD7D5351, and the RFC 3720
    // iSCSI vector below passes, so the kernel table is correct.
    rux::test_assert_eq!(CRC32C_TABLE[1], 0xF26B_8303, "crc32c table[1]");
    rux::test_assert_eq!(CRC32C_TABLE[255], 0xAD7D_5351, "crc32c table[255]");

    // set/ok round-trips per object size class.
    for size in [128usize, 256] {
        let mut raw = alloc::vec![0u8; size];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = (i * 7 + 3) as u8;
        }
        inode_csum_set(0x1234_5678, 42, &mut raw);
        rux::test_assert!(inode_csum_ok(0x1234_5678, 42, &raw), "inode csum ok size {size}");
        raw[8] ^= 0xFF;
        rux::test_assert!(!inode_csum_ok(0x1234_5678, 42, &raw), "inode csum breaks on flip size {size}");
    }
    for dsize in [32usize, 64] {
        let mut desc = alloc::vec![0u8; dsize];
        for (i, b) in desc.iter_mut().enumerate() {
            *b = (i * 13 + 5) as u8;
        }
        group_desc_csum_set(0x9ABC_DEF0u32, 7, &mut desc);
        rux::test_assert!(group_desc_csum_ok(0x9ABC_DEF0u32, 7, &desc), "desc csum ok size {dsize}");
        desc[4] ^= 0x01;
        rux::test_assert!(!group_desc_csum_ok(0x9ABC_DEF0u32, 7, &desc), "desc csum breaks on flip size {dsize}");
    }
    let mut sb = alloc::vec![0u8; 1024];
    for (i, b) in sb.iter_mut().enumerate().take(0x3FC) {
        *b = (i * 31 + 11) as u8;
    }
    superblock_csum_set(&mut sb);
    rux::test_assert!(superblock_csum_ok(&sb), "superblock csum ok");
    sb[0] ^= 0x80;
    rux::test_assert!(!superblock_csum_ok(&sb), "superblock csum breaks on flip");

    // RFC 3720 / iSCSI vector: crc32c("123456789").
    let raw = crc32c(0xFFFF_FFFFu32, b"123456789");
    rux::test_assert_eq!(raw ^ 0xFFFF_FFFFu32, 0xE306_9283u32, "iSCSI crc32c vector");
    rux::tests::test_pass("ext4-csum");
}

fn test_ext4_indirect_pins() {
    let block_size = 4096u64;
    let max_size = max_file_size(block_size);
    rux::test_assert!(max_size > 4_000_000_000_000, "max file size over 4TB");

    rux::test_assert_eq!(get_indirect_level(48 * 1024, block_size), 0, "direct only");
    rux::test_assert_eq!(get_indirect_level(100 * 1024, block_size), 1, "single indirect");
    rux::test_assert_eq!(get_indirect_level(4 * 1024 * 1024, block_size), 1, "still single");
    rux::test_assert_eq!(get_indirect_level(5 * 1024 * 1024, block_size), 2, "double indirect");
    rux::test_assert_eq!(get_indirect_level(4 * 1024 * 1024 * 1024, block_size), 2, "still double");
    rux::test_assert_eq!(get_indirect_level(5 * 1024 * 1024 * 1024u64, block_size), 3, "triple indirect");

    let mut iter = Ext4BlockIterator::new(20);
    let mut ok = true;
    for i in 0..12 {
        match iter.next_mapping() {
            Some((level, offset)) => ok &= level == 0 && offset == i,
            None => ok = false,
        }
    }
    for i in 0..8 {
        match iter.next_mapping() {
            Some((level, offset)) => ok &= level == 1 && offset == i,
            None => ok = false,
        }
    }
    ok &= iter.next_mapping().is_none();
    rux::test_assert!(ok, "block iterator level/offset walk");
    rux::tests::test_pass("ext4-indirect");
}
