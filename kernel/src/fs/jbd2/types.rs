//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 on-disk data structures
//!
//! JBD2 data types

#![allow(non_camel_case_types)]

use core::mem::size_of;

// ============================================================================
// Constants
// ============================================================================

/// JBD2 magic number
pub const JBD2_MAGIC_NUMBER: u32 = 0xC03B3998;

/// Minimum journal blocks
pub const JBD2_MIN_JOURNAL_BLOCKS: u32 = 1024;

/// Default maximum commit age in seconds
pub const JBD2_DEFAULT_MAX_COMMIT_AGE: u32 = 5;

/// Default fast commit blocks
pub const JBD2_DEFAULT_FAST_COMMIT_BLOCKS: u32 = 256;

// ============================================================================
// Block types
// ============================================================================

/// Descriptor block types
pub const JBD2_DESCRIPTOR_BLOCK: u32 = 1;
pub const JBD2_COMMIT_BLOCK: u32 = 2;
pub const JBD2_SUPERBLOCK_V1: u32 = 3;
pub const JBD2_SUPERBLOCK_V2: u32 = 4;
pub const JBD2_REVOKE_BLOCK: u32 = 5;

// ============================================================================
// Checksum types
// ============================================================================

pub const JBD2_CRC32_CHKSUM: u8 = 1;
pub const JBD2_MD5_CHKSUM: u8 = 2;
pub const JBD2_SHA1_CHKSUM: u8 = 3;
pub const JBD2_CRC32C_CHKSUM: u8 = 4;

pub const JBD2_CRC32_CHKSUM_SIZE: usize = 4;
pub const JBD2_CHECKSUM_BYTES: usize = 32 / size_of::<u32>();

// ============================================================================
// Tag flags
// ============================================================================

/// On-disk block is escaped
pub const JBD2_FLAG_ESCAPE: u32 = 1;
/// Block has same uuid as previous
pub const JBD2_FLAG_SAME_UUID: u32 = 2;
/// Block deleted by this transaction
pub const JBD2_FLAG_DELETED: u32 = 4;
/// Last tag in this descriptor block
pub const JBD2_FLAG_LAST_TAG: u32 = 8;

// ============================================================================
// Feature flags
// ============================================================================

/// Compatible feature: checksum
pub const JBD2_FEATURE_COMPAT_CHECKSUM: u32 = 0x00000001;

/// Incompatible feature: revoke
pub const JBD2_FEATURE_INCOMPAT_REVOKE: u32 = 0x00000001;
/// Incompatible feature: 64-bit
pub const JBD2_FEATURE_INCOMPAT_64BIT: u32 = 0x00000002;
/// Incompatible feature: async commit
pub const JBD2_FEATURE_INCOMPAT_ASYNC_COMMIT: u32 = 0x00000004;
/// Incompatible feature: checksum v2
pub const JBD2_FEATURE_INCOMPAT_CSUM_V2: u32 = 0x00000008;
/// Incompatible feature: checksum v3
pub const JBD2_FEATURE_INCOMPAT_CSUM_V3: u32 = 0x00000010;
/// Incompatible feature: fast commit
pub const JBD2_FEATURE_INCOMPAT_FAST_COMMIT: u32 = 0x00000020;

// Known features
pub const JBD2_KNOWN_COMPAT_FEATURES: u32 = JBD2_FEATURE_COMPAT_CHECKSUM;
pub const JBD2_KNOWN_ROCOMPAT_FEATURES: u32 = 0;
pub const JBD2_KNOWN_INCOMPAT_FEATURES: u32 = JBD2_FEATURE_INCOMPAT_REVOKE
    | JBD2_FEATURE_INCOMPAT_64BIT
    | JBD2_FEATURE_INCOMPAT_ASYNC_COMMIT
    | JBD2_FEATURE_INCOMPAT_CSUM_V2
    | JBD2_FEATURE_INCOMPAT_CSUM_V3
    | JBD2_FEATURE_INCOMPAT_FAST_COMMIT;

// ============================================================================
// Journal buffer state bits
// ============================================================================

pub const BH_JBD: u32 = 0;          // Has an attached journal_head
pub const BH_JWrite: u32 = 1;       // Being written to log
pub const BH_Freed: u32 = 2;        // Has been freed (truncated)
pub const BH_Revoked: u32 = 3;      // Has been revoked from the log
pub const BH_RevokeValid: u32 = 4;  // Revoked flag is valid
pub const BH_JBDDirty: u32 = 5;     // Is dirty but journaled
pub const BH_JournalHead: u32 = 6;  // Pins bh->b_private and jh->b_bh
pub const BH_Shadow: u32 = 7;       // IO on shadow buffer is running
pub const BH_Verified: u32 = 8;     // Metadata block has been verified ok

// ============================================================================
// On-disk structures
// ============================================================================

/// Journal header - standard header for all descriptor blocks
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_header_t {
    pub h_magic: u32,       // Magic number (big-endian)
    pub h_blocktype: u32,   // Block type (big-endian)
    pub h_sequence: u32,    // Transaction sequence (big-endian)
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

/// Commit block header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct commit_header {
    pub h_magic: u32,
    pub h_blocktype: u32,
    pub h_sequence: u32,
    pub h_chksum_type: u8,
    pub h_chksum_size: u8,
    pub h_padding: [u8; 2],
    pub h_chksum: [u32; JBD2_CHECKSUM_BYTES],
    pub h_commit_sec: u64,
    pub h_commit_nsec: u32,
}

impl Default for commit_header {
    fn default() -> Self {
        Self {
            h_magic: JBD2_MAGIC_NUMBER.to_be(),
            h_blocktype: JBD2_COMMIT_BLOCK.to_be(),
            h_sequence: 0,
            h_chksum_type: 0,
            h_chksum_size: 0,
            h_padding: [0; 2],
            h_chksum: [0; JBD2_CHECKSUM_BYTES],
            h_commit_sec: 0,
            h_commit_nsec: 0,
        }
    }
}

/// Block tag (v1 with truncated checksum)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_block_tag_t {
    pub t_blocknr: u32,        // On-disk block number
    pub t_checksum: u16,       // Truncated crc32c(uuid+seq+block)
    pub t_flags: u16,           // Tag flags
    pub t_blocknr_high: u32,    // High 32 bits of block number (64-bit)
}

/// Block tag (v3 with full checksum)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_block_tag3_t {
    pub t_blocknr: u32,        // On-disk block number
    pub t_flags: u32,          // Tag flags
    pub t_blocknr_high: u32,   // High 32 bits of block number
    pub t_checksum: u32,       // Full crc32c(uuid+seq+block)
}

/// Block tail for checksumming
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct journal_block_tail_t {
    pub t_checksum: u32,       // crc32c(uuid+descr_block)
}

/// Revoke header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct journal_revoke_header_t {
    pub r_header: journal_header_t,
    pub r_count: u32,          // Count of bytes used in the block
}

impl Default for journal_revoke_header_t {
    fn default() -> Self {
        Self {
            r_header: journal_header_t::new(JBD2_REVOKE_BLOCK, 0),
            r_count: 0,
        }
    }
}

/// Journal superblock - all fields in big-endian
#[repr(C, packed)]
pub struct journal_superblock_t {
    // 0x0000 - Header
    pub s_header: journal_header_t,

    // 0x000C - Static information
    pub s_blocksize: u32,       // Journal device blocksize
    pub s_maxlen: u32,          // Total blocks in journal file
    pub s_first: u32,           // First block of log information

    // 0x0018 - Dynamic state
    pub s_sequence: u32,        // First commit ID expected in log
    pub s_start: u32,           // Blocknr of start of log

    // 0x0020 - Error value
    pub s_errno: u32,

    // 0x0024 - V2 features
    pub s_feature_compat: u32,
    pub s_feature_incompat: u32,
    pub s_feature_ro_compat: u32,

    // 0x0030 - UUID
    pub s_uuid: [u8; 16],

    // 0x0040 - Users
    pub s_nr_users: u32,
    pub s_dynsuper: u32,

    // 0x0048 - Limits
    pub s_max_transaction: u32,  // Limit of journal blocks per trans
    pub s_max_trans_data: u32,   // Limit of data blocks per trans

    // 0x0050 - Checksum
    pub s_checksum_type: u8,
    pub s_padding2: [u8; 3],

    // 0x0054 - Fast commit
    pub s_num_fc_blks: u32,     // Number of fast commit blocks
    pub s_head: u32,            // Blocknr of head of log

    // 0x005C - Padding
    pub s_padding: [u32; 40],
    pub s_checksum: u32,        // crc32c(superblock)

    // 0x0100 - User IDs (768 bytes)
    pub s_users: [u8; 768],
}

// ============================================================================
// Size calculations
// ============================================================================

/// Calculate tag size based on features
pub const fn journal_tag_size(has_64bit: bool, has_csum_v3: bool) -> usize {
    if has_csum_v3 {
        size_of::<journal_block_tag3_t>()
    } else if has_64bit {
        size_of::<journal_block_tag_t>()
    } else {
        size_of::<journal_block_tag_t>() - size_of::<u32>() // No t_blocknr_high
    }
}

/// Calculate how many tags fit in a descriptor block
pub const fn journal_tags_per_block(block_size: u32, tag_size: usize) -> usize {
    let header_size = size_of::<journal_header_t>();
    let tail_size = size_of::<journal_block_tail_t>();
    let count = (block_size as usize - header_size - tail_size) / tag_size;
    if count < 1 { 1 } else { count }
}
