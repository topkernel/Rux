//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 journal on-disk header and tag arithmetic invariant tests.
//!
//! Types copied from: kernel/src/fs/jbd2/types.rs

use proptest::prelude::*;
use std::mem::size_of;

// ============================================================================
// Copied types from kernel/src/fs/jbd2/types.rs
// ============================================================================

pub const JBD2_MAGIC_NUMBER: u32 = 0xC03B3998;
pub const JBD2_MIN_JOURNAL_BLOCKS: u32 = 1024;
pub const JBD2_DEFAULT_MAX_COMMIT_AGE: u32 = 5;
pub const JBD2_DEFAULT_FAST_COMMIT_BLOCKS: u32 = 256;

pub const JBD2_DESCRIPTOR_BLOCK: u32 = 1;
pub const JBD2_COMMIT_BLOCK: u32 = 2;
pub const JBD2_SUPERBLOCK_V1: u32 = 3;
pub const JBD2_SUPERBLOCK_V2: u32 = 4;
pub const JBD2_REVOKE_BLOCK: u32 = 5;

pub const JBD2_CRC32_CHKSUM: u8 = 1;
pub const JBD2_MD5_CHKSUM: u8 = 2;
pub const JBD2_SHA1_CHKSUM: u8 = 3;
pub const JBD2_CRC32C_CHKSUM: u8 = 4;

pub const JBD2_CRC32_CHKSUM_SIZE: usize = 4;
pub const JBD2_CHECKSUM_BYTES: usize = 32 / size_of::<u32>();

pub const JBD2_FLAG_ESCAPE: u32 = 1;
pub const JBD2_FLAG_SAME_UUID: u32 = 2;
pub const JBD2_FLAG_DELETED: u32 = 4;
pub const JBD2_FLAG_LAST_TAG: u32 = 8;

pub const JBD2_FEATURE_COMPAT_CHECKSUM: u32 = 0x00000001;
pub const JBD2_FEATURE_INCOMPAT_REVOKE: u32 = 0x00000001;
pub const JBD2_FEATURE_INCOMPAT_64BIT: u32 = 0x00000002;
pub const JBD2_FEATURE_INCOMPAT_ASYNC_COMMIT: u32 = 0x00000004;
pub const JBD2_FEATURE_INCOMPAT_CSUM_V2: u32 = 0x00000008;
pub const JBD2_FEATURE_INCOMPAT_CSUM_V3: u32 = 0x00000010;
pub const JBD2_FEATURE_INCOMPAT_FAST_COMMIT: u32 = 0x00000020;

pub const JBD2_KNOWN_INCOMPAT_FEATURES: u32 = JBD2_FEATURE_INCOMPAT_REVOKE
    | JBD2_FEATURE_INCOMPAT_64BIT
    | JBD2_FEATURE_INCOMPAT_ASYNC_COMMIT
    | JBD2_FEATURE_INCOMPAT_CSUM_V2
    | JBD2_FEATURE_INCOMPAT_CSUM_V3
    | JBD2_FEATURE_INCOMPAT_FAST_COMMIT;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_header_t {
    pub h_magic: u32,
    pub h_blocktype: u32,
    pub h_sequence: u32,
}

impl journal_header_t {
    pub fn new(blocktype: u32, sequence: u32) -> Self {
        Self {
            h_magic: JBD2_MAGIC_NUMBER.to_be(),
            h_blocktype: blocktype.to_be(),
            h_sequence: sequence.to_be(),
        }
    }

    pub fn is_valid(&self) -> bool {
        u32::from_be(self.h_magic) == JBD2_MAGIC_NUMBER
    }

    pub fn block_type(&self) -> u32 {
        u32::from_be(self.h_blocktype)
    }

    pub fn sequence(&self) -> u32 {
        u32::from_be(self.h_sequence)
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_block_tag_t {
    pub t_blocknr: u32,
    pub t_checksum: u16,
    pub t_flags: u16,
    pub t_blocknr_high: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_block_tag3_t {
    pub t_blocknr: u32,
    pub t_flags: u32,
    pub t_blocknr_high: u32,
    pub t_checksum: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_block_tail_t {
    pub t_checksum: u32,
}

pub const fn journal_tag_size(has_64bit: bool, has_csum_v3: bool) -> usize {
    if has_csum_v3 {
        size_of::<journal_block_tag3_t>()
    } else if has_64bit {
        size_of::<journal_block_tag_t>()
    } else {
        size_of::<journal_block_tag_t>() - size_of::<u32>()
    }
}

pub const fn journal_tags_per_block(block_size: u32, tag_size: usize) -> usize {
    let header_size = size_of::<journal_header_t>();
    let tail_size = size_of::<journal_block_tail_t>();
    let count = (block_size as usize - header_size - tail_size) / tag_size;
    if count < 1 { 1 } else { count }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-JBD2-1: Magic number is valid
    #[test]
    fn test_magic_valid(_v in 0u8..1u8) {
        let hdr = journal_header_t::new(JBD2_DESCRIPTOR_BLOCK, 1);
        prop_assert!(hdr.is_valid());
    }

    /// INV-JBD2-2: Corrupted magic is invalid
    #[test]
    fn test_magic_invalid(corrupt in 1u32..0xFFFF_FFFFu32) {
        let mut hdr = journal_header_t::new(JBD2_DESCRIPTOR_BLOCK, 1);
        hdr.h_magic = corrupt;
        // Very unlikely to hit exact magic by chance
        if corrupt != JBD2_MAGIC_NUMBER {
            prop_assert!(!hdr.is_valid());
        }
    }

    /// INV-JBD2-3: block_type roundtrip
    #[test]
    fn test_block_type_roundtrip(
        bt in 0u32..100u32,
        seq in 0u32..1_000_000u32,
    ) {
        let hdr = journal_header_t::new(bt, seq);
        prop_assert_eq!(hdr.block_type(), bt);
    }

    /// INV-JBD2-4: sequence roundtrip
    #[test]
    fn test_sequence_roundtrip(
        bt in 0u32..100u32,
        seq in 0u32..u32::MAX,
    ) {
        let hdr = journal_header_t::new(bt, seq);
        prop_assert_eq!(hdr.sequence(), seq);
    }

    /// INV-JBD2-5: All block type constants are distinct and non-zero
    #[test]
    fn test_block_types_distinct(_v in 0u8..1u8) {
        let types = [
            JBD2_DESCRIPTOR_BLOCK, JBD2_COMMIT_BLOCK,
            JBD2_SUPERBLOCK_V1, JBD2_SUPERBLOCK_V2, JBD2_REVOKE_BLOCK,
        ];
        let mut seen = std::collections::HashSet::new();
        for &t in &types {
            prop_assert!(t > 0, "block type must be non-zero");
            prop_assert!(seen.insert(t), "duplicate block type: {}", t);
        }
    }

    /// INV-JBD2-6: journal_tag_size depends on feature flags
    #[test]
    fn test_tag_size_v3_larger(_v in 0u8..1u8) {
        let v3_size = journal_tag_size(true, true);
        let v1_size = journal_tag_size(false, false);
        let v2_size = journal_tag_size(true, false);
        prop_assert!(v3_size >= v2_size);
        prop_assert!(v2_size >= v1_size);
    }

    /// INV-JBD2-7: journal_tags_per_block >= 1
    #[test]
    fn test_tags_per_block_minimum(
        bs in 20u32..8192u32,
        ts in 1usize..1000usize,
    ) {
        let count = journal_tags_per_block(bs, ts);
        prop_assert!(count >= 1);
    }

    /// INV-JBD2-8: tags_per_block for 4096-byte blocks is reasonable
    #[test]
    fn test_tags_per_block_4k(_v in 0u8..1u8) {
        let count = journal_tags_per_block(4096, journal_tag_size(false, false));
        prop_assert!(count > 100); // At least 100 tags per 4K block
    }

    /// INV-JBD2-9: INCOMPAT feature flags are power-of-2 (distinct bits)
    #[test]
    fn test_incompat_flags_pow2(_v in 0u8..1u8) {
        let flags = [
            JBD2_FEATURE_INCOMPAT_REVOKE,
            JBD2_FEATURE_INCOMPAT_64BIT,
            JBD2_FEATURE_INCOMPAT_ASYNC_COMMIT,
            JBD2_FEATURE_INCOMPAT_CSUM_V2,
            JBD2_FEATURE_INCOMPAT_CSUM_V3,
            JBD2_FEATURE_INCOMPAT_FAST_COMMIT,
        ];
        for &f in &flags {
            prop_assert!(f > 0 && (f & (f - 1)) == 0, "not power of 2: {:#x}", f);
        }
    }

    /// INV-JBD2-10: Tag flags are power-of-2
    #[test]
    fn test_tag_flags_pow2(_v in 0u8..1u8) {
        let flags = [
            JBD2_FLAG_ESCAPE, JBD2_FLAG_SAME_UUID,
            JBD2_FLAG_DELETED, JBD2_FLAG_LAST_TAG,
        ];
        for &f in &flags {
            prop_assert!(f > 0 && (f & (f - 1)) == 0, "not power of 2: {:#x}", f);
        }
    }

    /// INV-JBD2-11: Header struct is 12 bytes
    #[test]
    fn test_header_size(_v in 0u8..1u8) {
        prop_assert_eq!(size_of::<journal_header_t>(), 12);
    }

    /// INV-JBD2-12: Tail struct is 4 bytes
    #[test]
    fn test_tail_size(_v in 0u8..1u8) {
        prop_assert_eq!(size_of::<journal_block_tail_t>(), 4);
    }

    /// INV-JBD2-13: Checksum types are distinct and non-zero
    #[test]
    fn test_checksum_types_distinct(_v in 0u8..1u8) {
        let types = [
            JBD2_CRC32_CHKSUM, JBD2_MD5_CHKSUM,
            JBD2_SHA1_CHKSUM, JBD2_CRC32C_CHKSUM,
        ];
        let mut seen = std::collections::HashSet::new();
        for &t in &types {
            prop_assert!(t > 0);
            prop_assert!(seen.insert(t), "duplicate checksum type: {}", t);
        }
    }

    /// INV-JBD2-14: tags_per_block increases with block_size
    #[test]
    fn test_tags_per_block_monotone(
        bs1 in 512u32..4096u32,
        bs2 in 512u32..4096u32,
        ts in 8usize..100usize,
    ) {
        let (small, large) = if bs1 <= bs2 { (bs1, bs2) } else { (bs2, bs1) };
        let c1 = journal_tags_per_block(small, ts);
        let c2 = journal_tags_per_block(large, ts);
        prop_assert!(c1 <= c2);
    }

    /// INV-JBD2-15: tags_per_block decreases with tag_size
    #[test]
    fn test_tags_per_block_tag_size(
        bs in 4096u32..8192u32,
        ts1 in 8usize..50usize,
        ts2 in 8usize..50usize,
    ) {
        let (small, large) = if ts1 <= ts2 { (ts1, ts2) } else { (ts2, ts1) };
        let c1 = journal_tags_per_block(bs, small);
        let c2 = journal_tags_per_block(bs, large);
        prop_assert!(c1 >= c2);
    }

    /// INV-JBD2-16: Default header has valid magic
    #[test]
    fn test_default_header_invalid(_v in 0u8..1u8) {
        let hdr = journal_header_t::default();
        prop_assert!(!hdr.is_valid()); // default h_magic = 0
    }
}
