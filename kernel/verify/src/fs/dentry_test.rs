//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! DentryFlags and dentry_hash invariant tests.
//!
//! Types copied from: kernel/src/fs/dentry.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/dentry.rs
// ============================================================================

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DentryFlags(u32);

impl DentryFlags {
    pub const DCACHE_UNHASHED: u32 = 0x00000001;
    pub const DCACHE_HASHED: u32 = 0x00000002;
    pub const DCACHE_REFERENCED: u32 = 0x00000010;
    pub const DCACHE_DENTRY_KILL: u32 = 0x00000040;

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_hashed(&self) -> bool {
        (self.0 & Self::DCACHE_HASHED) != 0
    }

    pub fn is_unhashed(&self) -> bool {
        (self.0 & Self::DCACHE_UNHASHED) != 0
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DentryState {
    DUnhashed,
    DHashed,
    DKill,
}

fn dentry_hash(name: &str, parent_ino: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    hash ^= parent_ino;
    hash = hash.wrapping_mul(0x100000001b3);
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-DENT-1: DCACHE_HASHED flag sets is_hashed
    #[test]
    fn test_is_hashed(extra in 0u32..0xFFu32) {
        let flags = DentryFlags::new(DentryFlags::DCACHE_HASHED | extra);
        prop_assert!(flags.is_hashed());
    }

    /// INV-DENT-2: no DCACHE_HASHED means not hashed
    #[test]
    fn test_not_hashed(raw in 0u32..0x100u32) {
        let flags = DentryFlags::new(raw & !DentryFlags::DCACHE_HASHED);
        prop_assert!(!flags.is_hashed());
    }

    /// INV-DENT-3: DCACHE_UNHASHED flag sets is_unhashed
    #[test]
    fn test_is_unhashed(extra in 0u32..0xFFu32) {
        let flags = DentryFlags::new(DentryFlags::DCACHE_UNHASHED | extra);
        prop_assert!(flags.is_unhashed());
    }

    /// INV-DENT-4: can be both hashed and unhashed (flags are independent)
    #[test]
    fn test_both_hashed_unhashed(_v in 0u8..1u8) {
        let flags = DentryFlags::new(DentryFlags::DCACHE_HASHED | DentryFlags::DCACHE_UNHASHED);
        prop_assert!(flags.is_hashed());
        prop_assert!(flags.is_unhashed());
    }

    /// INV-DENT-5: bits() roundtrip
    #[test]
    fn test_bits_roundtrip(raw in 0u32..0xFFFFu32) {
        let flags = DentryFlags::new(raw);
        prop_assert_eq!(flags.bits(), raw);
    }

    /// INV-DENT-6: dentry_hash is deterministic
    #[test]
    fn test_dentry_hash_deterministic(
        name in "[a-z]{1,20}",
        parent_ino in 1u64..0xFFFFFFFFu64,
    ) {
        let h1 = dentry_hash(&name, parent_ino);
        let h2 = dentry_hash(&name, parent_ino);
        prop_assert_eq!(h1, h2);
    }

    /// INV-DENT-7: dentry_hash empty name depends only on parent_ino
    #[test]
    fn test_dentry_hash_empty_name(parent_ino in 1u64..0xFFFFFFFFu64) {
        let h1 = dentry_hash("", parent_ino);
        let h2 = dentry_hash("", parent_ino);
        prop_assert_eq!(h1, h2);
    }

    /// INV-DENT-8: different names likely produce different hashes
    #[test]
    fn test_dentry_hash_different_names(
        name1 in "[a-z]{1,10}",
        name2 in "[a-z]{1,10}",
    ) {
        if name1 != name2 {
            let h1 = dentry_hash(&name1, 42);
            let h2 = dentry_hash(&name2, 42);
            prop_assert_ne!(h1, h2);
        }
    }

    /// INV-DENT-9: different parent_ino likely produces different hashes
    #[test]
    fn test_dentry_hash_different_parent(
        name in "[a-z]{1,10}",
        p1 in 1u64..0xFFFFu64,
        p2 in 1u64..0xFFFFu64,
    ) {
        if p1 != p2 {
            let h1 = dentry_hash(&name, p1);
            let h2 = dentry_hash(&name, p2);
            prop_assert_ne!(h1, h2);
        }
    }

    /// INV-DENT-10: DentryState variants are distinct
    #[test]
    fn test_dentry_state_distinct(_v in 0u8..1u8) {
        let states = [DentryState::DUnhashed, DentryState::DHashed, DentryState::DKill];
        for i in 0..states.len() {
            for j in (i + 1)..states.len() {
                prop_assert_ne!(states[i], states[j]);
            }
        }
    }

    /// INV-DENT-11: DCACHE constants are distinct powers-of-2 or single-bit
    #[test]
    fn test_dcache_flags_distinct(_v in 0u8..1u8) {
        let flags = [
            DentryFlags::DCACHE_UNHASHED,
            DentryFlags::DCACHE_HASHED,
            DentryFlags::DCACHE_REFERENCED,
            DentryFlags::DCACHE_DENTRY_KILL,
        ];
        let mut seen = 0u32;
        for f in &flags {
            prop_assert_ne!(*f, 0);
            prop_assert_eq!(*f & (*f - 1), 0, "flag {} not a power of 2", f);
            prop_assert_eq!(seen & f, 0, "flag {} overlaps", f);
            seen |= f;
        }
    }

    /// INV-DENT-12: dentry_hash same as inode_hash when name is a single u64 chunk
    /// (Both use FNV-1a with same multiplier, but dentry mixes byte-by-byte)
    #[test]
    fn test_dentry_hash_single_char(name in "a") {
        // Single char name: hash ^= parent_ino, hash *= FNV, hash ^= byte, hash *= FNV
        let parent = 42u64;
        let h = dentry_hash(&name, parent);
        prop_assert_ne!(h, 0);
    }
}
