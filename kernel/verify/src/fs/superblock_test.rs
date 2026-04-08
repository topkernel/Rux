//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for VFS superblock flags invariants.
//! Copied from: kernel/src/fs/superblock.rs

use proptest::prelude::*;

// Copied SuperBlockFlags
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SuperBlockFlags(u64);

impl SuperBlockFlags {
    pub const SB_RDONLY: u64 = 1;
    pub const SB_NOATIME: u64 = 1 << 5;
    pub const SB_NODIRATIME: u64 = 1 << 6;
    pub const SB_SYNCHRONOUS: u64 = 1 << 7;
    pub const SB_MANDLOCK: u64 = 1 << 8;
    pub const SB_DIRSYNC: u64 = 1 << 9;
    pub const SB_NOSEC: u64 = 1 << 10;
    pub const SB_ACTIVE: u64 = 1 << 11;
    pub const SB_WRITERS: u64 = 1 << 12;

    pub fn new(flags: u64) -> Self { Self(flags) }
    pub fn is_readonly(&self) -> bool { (self.0 & Self::SB_RDONLY) != 0 }
    pub fn is_active(&self) -> bool { (self.0 & Self::SB_ACTIVE) != 0 }
    pub fn bits(&self) -> u64 { self.0 }
}

proptest! {
    #[test]
    fn test_sb_flags_are_powers_of_two(_v in 0u8..1u8) {
        let flags = [
            ("SB_RDONLY", SuperBlockFlags::SB_RDONLY),
            ("SB_NOATIME", SuperBlockFlags::SB_NOATIME),
            ("SB_NODIRATIME", SuperBlockFlags::SB_NODIRATIME),
            ("SB_SYNCHRONOUS", SuperBlockFlags::SB_SYNCHRONOUS),
            ("SB_MANDLOCK", SuperBlockFlags::SB_MANDLOCK),
            ("SB_DIRSYNC", SuperBlockFlags::SB_DIRSYNC),
            ("SB_NOSEC", SuperBlockFlags::SB_NOSEC),
            ("SB_ACTIVE", SuperBlockFlags::SB_ACTIVE),
            ("SB_WRITERS", SuperBlockFlags::SB_WRITERS),
        ];
        for (name, val) in &flags {
            assert!(*val > 0 && (val & (val - 1)) == 0,
                    "{} ({:#x}) must be a power of two", name, val);
        }
    }

    #[test]
    fn test_sb_flags_distinct(_v in 0u8..1u8) {
        let flags = [
            SuperBlockFlags::SB_RDONLY, SuperBlockFlags::SB_NOATIME,
            SuperBlockFlags::SB_NODIRATIME, SuperBlockFlags::SB_SYNCHRONOUS,
            SuperBlockFlags::SB_MANDLOCK, SuperBlockFlags::SB_DIRSYNC,
            SuperBlockFlags::SB_NOSEC, SuperBlockFlags::SB_ACTIVE,
            SuperBlockFlags::SB_WRITERS,
        ];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0,
                           "SB flags {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_sb_rdonly_is_bit_0(_v in 0u8..1u8) {
        assert_eq!(SuperBlockFlags::SB_RDONLY, 1, "SB_RDONLY must be bit 0");
    }

    #[test]
    fn test_sb_rdonly_check(flag_val in 0u64..(1u64 << 13)) {
        let f = SuperBlockFlags::new(flag_val);
        let expected = (flag_val & SuperBlockFlags::SB_RDONLY) != 0;
        assert_eq!(f.is_readonly(), expected);
    }

    #[test]
    fn test_sb_active_check(flag_val in 0u64..(1u64 << 13)) {
        let f = SuperBlockFlags::new(flag_val);
        let expected = (flag_val & SuperBlockFlags::SB_ACTIVE) != 0;
        assert_eq!(f.is_active(), expected);
    }

    #[test]
    fn test_sb_combined_flags(_v in 0u8..1u8) {
        let combined = SuperBlockFlags::new(
            SuperBlockFlags::SB_RDONLY | SuperBlockFlags::SB_ACTIVE
        );
        assert!(combined.is_readonly());
        assert!(combined.is_active());
        assert_eq!(combined.bits(), SuperBlockFlags::SB_RDONLY | SuperBlockFlags::SB_ACTIVE);
    }

    #[test]
    fn test_sb_bits_roundtrip(flag_val in 0u64..(1u64 << 13)) {
        let f = SuperBlockFlags::new(flag_val);
        assert_eq!(f.bits(), flag_val);
    }
}
