//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 filesystem feature flag invariant tests.
//!
//! Types copied from: kernel/src/fs/ext4/superblock.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/ext4/superblock.rs
// ============================================================================

#[repr(C)]
pub struct Ext4FsState {
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub inode_size: u16,
}

impl Ext4FsState {
    pub fn new() -> Self {
        Self {
            feature_compat: 0,
            feature_incompat: 0,
            feature_ro_compat: 0,
            inode_size: 256,
        }
    }

    pub fn has_64bit(&self) -> bool {
        (self.feature_incompat & 0x80) != 0
    }

    pub fn has_extents(&self) -> bool {
        (self.feature_incompat & 0x40) != 0
    }

    pub fn has_flex_bg(&self) -> bool {
        (self.feature_incompat & 0x200) != 0
    }
}

impl Default for Ext4FsState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SB-1: new() has all features clear and inode_size 256
    #[test]
    fn test_new(_v in 0u8..1u8) {
        let s = Ext4FsState::new();
        prop_assert_eq!(s.feature_compat, 0);
        prop_assert_eq!(s.feature_incompat, 0);
        prop_assert_eq!(s.feature_ro_compat, 0);
        prop_assert_eq!(s.inode_size, 256);
    }

    /// INV-SB-2: has_64bit checks bit 7
    #[test]
    fn test_has_64bit(extra in 0u32..0xFFu32) {
        let mut s = Ext4FsState::new();
        s.feature_incompat = 0x80 | (extra & !0x80);
        prop_assert!(s.has_64bit());
    }

    /// INV-SB-3: has_extents checks bit 6
    #[test]
    fn test_has_extents(extra in 0u32..0xFFu32) {
        let mut s = Ext4FsState::new();
        s.feature_incompat = 0x40 | (extra & !0x40);
        prop_assert!(s.has_extents());
    }

    /// INV-SB-4: has_flex_bg checks bit 9
    #[test]
    fn test_has_flex_bg(extra in 0u32..0xFFFu32) {
        let mut s = Ext4FsState::new();
        s.feature_incompat = 0x200 | (extra & !0x200);
        prop_assert!(s.has_flex_bg());
    }

    /// INV-SB-5: no features set → all has_* return false
    #[test]
    fn test_no_features(
        compat in 0u32..0xFFu32,
        incompat in 0u32..0xFFu32,
        ro_compat in 0u32..0xFFu32,
    ) {
        let s = Ext4FsState {
            feature_compat: compat & !0x80 & !0x40 & !0x200,
            feature_incompat: incompat & !0x80 & !0x40 & !0x200,
            feature_ro_compat: ro_compat,
            inode_size: 256,
        };
        prop_assert!(!s.has_64bit());
        prop_assert!(!s.has_extents());
        prop_assert!(!s.has_flex_bg());
    }

    /// INV-SB-6: features are independent
    #[test]
    fn test_independent(_v in 0u8..1u8) {
        let mut s = Ext4FsState::new();
        s.feature_incompat = 0x80 | 0x40 | 0x200;
        prop_assert!(s.has_64bit());
        prop_assert!(s.has_extents());
        prop_assert!(s.has_flex_bg());
    }

    /// INV-SB-7: feature flag bits don't overlap
    #[test]
    fn test_no_overlap(_v in 0u8..1u8) {
        prop_assert_eq!(0x80 & 0x40, 0);
        prop_assert_eq!(0x80 & 0x200, 0);
        prop_assert_eq!(0x40 & 0x200, 0);
    }

    /// INV-SB-8: feature bits are powers of 2
    #[test]
    fn test_pow2(_v in 0u8..1u8) {
        for flag in [0x80u32, 0x40, 0x200] {
            prop_assert_eq!(flag & (flag - 1), 0);
        }
    }
}
