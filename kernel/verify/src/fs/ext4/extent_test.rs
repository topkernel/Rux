//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for ext4 extent tree struct composition and interval matching.
//! Copied from: kernel/src/fs/ext4/extent.rs

use proptest::prelude::*;

pub const EXT4_EXT_MAGIC: u16 = 0xF30A;

// Copied Ext4ExtentHeader
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4ExtentHeader {
    pub eh_magic: u16,
    pub eh_entries: u16,
    pub eh_max: u16,
    pub eh_depth: u16,
    pub eh_generation: u32,
}

// Copied Ext4Extent
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4Extent {
    pub ee_block: u32,
    pub ee_len: u16,
    pub ee_start_hi: u16,
    pub ee_start_lo: u32,
}

// Copied Ext4ExtentIdx
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4ExtentIdx {
    pub ei_block: u32,
    pub ei_leaf_lo: u32,
    pub ei_leaf_hi: u16,
    pub ei_unused: u16,
}

impl Ext4Extent {
    pub fn start_block(&self) -> u64 {
        ((self.ee_start_hi as u64) << 32) | (self.ee_start_lo as u64)
    }

    pub fn length(&self) -> u32 {
        self.ee_len as u32
    }

    /// Logical range covered: [ee_block, ee_block + ee_len)
    pub fn logical_end(&self) -> u64 {
        self.ee_block as u64 + self.ee_len as u64
    }

    /// Physical range end: start_block + length
    pub fn physical_end(&self) -> u64 {
        self.start_block() + self.length() as u64
    }
}

impl Ext4ExtentIdx {
    pub fn leaf_block(&self) -> u64 {
        ((self.ei_leaf_hi as u64) << 32) | (self.ei_leaf_lo as u64)
    }
}

/// Copied leaf-level extent search: find extent containing logical_block
pub fn find_extent_in_list(extents: &[Ext4Extent], logical_block: u64) -> Option<(usize, u64)> {
    for (i, ext) in extents.iter().enumerate() {
        if logical_block >= ext.ee_block as u64 && logical_block < ext.logical_end() {
            let offset = logical_block - ext.ee_block as u64;
            return Some((i, ext.start_block() + offset));
        }
    }
    None
}

proptest! {
    #[test]
    fn test_extent_header_size(_v in 0u8..1u8) {
        // eh_magic(2) + eh_entries(2) + eh_max(2) + eh_depth(2) + eh_generation(4) = 12
        assert_eq!(core::mem::size_of::<Ext4ExtentHeader>(), 12);
    }

    #[test]
    fn test_extent_size(_v in 0u8..1u8) {
        // ee_block(4) + ee_len(2) + ee_start_hi(2) + ee_start_lo(4) = 12
        assert_eq!(core::mem::size_of::<Ext4Extent>(), 12);
    }

    #[test]
    fn test_extent_idx_size(_v in 0u8..1u8) {
        // ei_block(4) + ei_leaf_lo(4) + ei_leaf_hi(2) + ei_unused(2) = 12
        assert_eq!(core::mem::size_of::<Ext4ExtentIdx>(), 12);
    }

    #[test]
    fn test_start_block_roundtrip(hi in 0u16..65535u16, lo in 0u32..) {
        let ext = Ext4Extent {
            ee_block: 0, ee_len: 1, ee_start_hi: hi, ee_start_lo: lo,
        };
        let composed = ext.start_block();
        assert_eq!((composed >> 32) as u16, hi);
        assert_eq!(composed as u32, lo);
    }

    #[test]
    fn test_leaf_block_roundtrip(hi in 0u16..65535u16, lo in 0u32..) {
        let idx = Ext4ExtentIdx {
            ei_block: 0, ei_leaf_lo: lo, ei_leaf_hi: hi, ei_unused: 0,
        };
        let composed = idx.leaf_block();
        assert_eq!((composed >> 32) as u16, hi);
        assert_eq!(composed as u32, lo);
    }

    #[test]
    fn test_start_block_max(hi in 0u16..65535u16) {
        let ext = Ext4Extent {
            ee_block: 0, ee_len: 1, ee_start_hi: hi, ee_start_lo: u32::MAX,
        };
        let sb = ext.start_block();
        assert!(sb > (hi as u64) << 32);
    }

    #[test]
    fn test_extent_magic(_v in 0u8..1u8) {
        assert_eq!(EXT4_EXT_MAGIC, 0xF30A);
    }

    #[test]
    fn test_find_extent_exact_match(ee_block in 0u32..10000u32, ee_len in 1u32..1000u32) {
        let ext = Ext4Extent {
            ee_block, ee_len,
            ee_start_hi: 0, ee_start_lo: 100000,
        };
        let extents = [ext];
        // Block at ee_block should be found
        let result = find_extent_in_list(&extents, ee_block as u64);
        assert!(result.is_some());
        let (idx, phys) = result.unwrap();
        assert_eq!(idx, 0);
        assert_eq!(phys, 100000);
    }

    #[test]
    fn test_find_extent_mid_range(ee_block in 0u32..10000u32, ee_len in 2u32..1000u32) {
        let ext = Ext4Extent {
            ee_block, ee_len,
            ee_start_hi: 0, ee_start_lo: 500000,
        };
        let extents = [ext];
        // Block at ee_block + ee_len/2
        let mid = ee_block as u64 + (ee_len as u64) / 2;
        let result = find_extent_in_list(&extents, mid);
        assert!(result.is_some());
        let (_, phys) = result.unwrap();
        let offset = mid - ee_block as u64;
        assert_eq!(phys, 500000 + offset);
    }

    #[test]
    fn test_find_extent_end_exclusive(ee_block in 0u32..10000u32, ee_len in 1u32..1000u32) {
        let ext = Ext4Extent {
            ee_block, ee_len,
            ee_start_hi: 0, ee_start_lo: 200000,
        };
        let extents = [ext];
        // Block at ee_block + ee_len should NOT be found (end is exclusive)
        let past_end = ee_block as u64 + ee_len as u64;
        let result = find_extent_in_list(&extents, past_end);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_extent_before(ee_block in 1u32..10000u32, ee_len in 1u32..1000u32) {
        let ext = Ext4Extent {
            ee_block, ee_len,
            ee_start_hi: 0, ee_start_lo: 300000,
        };
        let extents = [ext];
        let before = (ee_block - 1) as u64;
        let result = find_extent_in_list(&extents, before);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_extent_multiple(ee_block in 0u32..1000u32) {
        let ext1 = Ext4Extent { ee_block, ee_len: 100, ee_start_hi: 0, ee_start_lo: 1000 };
        let ext2 = Ext4Extent { ee_block: ee_block + 100, ee_len: 200, ee_start_hi: 0, ee_start_lo: 2000 };
        let ext3 = Ext4Extent { ee_block: ee_block + 300, ee_len: 50, ee_start_hi: 0, ee_start_lo: 3000 };
        let extents = [ext1, ext2, ext3];

        // Find in ext2
        let result = find_extent_in_list(&extents, (ee_block + 150) as u64);
        assert!(result.is_some());
        let (idx, phys) = result.unwrap();
        assert_eq!(idx, 1);
        assert_eq!(phys, 2050);

        // Find in ext3
        let result = find_extent_in_list(&extents, (ee_block + 320) as u64);
        assert!(result.is_some());
        let (idx, phys) = result.unwrap();
        assert_eq!(idx, 2);
        assert_eq!(phys, 3020);
    }

    #[test]
    fn test_extent_header_entries_le_max(entries in 0u16..1000u16, max in 1u16..1000u16) {
        let hdr = Ext4ExtentHeader {
            eh_magic: EXT4_EXT_MAGIC,
            eh_entries: entries.min(max),
            eh_max: max,
            eh_depth: 0,
            eh_generation: 0,
        };
        assert!(hdr.eh_entries <= hdr.eh_max);
    }
}
