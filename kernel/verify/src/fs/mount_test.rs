//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for mount flags (MntFlags) invariants.
//! Copied from: kernel/src/fs/mount.rs

use proptest::prelude::*;

// Copied MntFlags
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MntFlags(u64);

impl MntFlags {
    pub const MNT_READONLY: u64 = 0x01;
    pub const MNT_NOATIME: u64 = 0x02;
    pub const MNT_NODIRATIME: u64 = 0x04;
    pub const MNT_SYNCHRONOUS: u64 = 0x08;
    pub const MNT_NOEXEC: u64 = 0x10;
    pub const MNT_NOSUID: u64 = 0x20;
    pub const MNT_NODEV: u64 = 0x40;
    pub const MNT_PRIVATE: u64 = 0x80;
    pub const MNT_SHARED: u64 = 0x100;
    pub const MNT_SLAVE: u64 = 0x200;
    pub const MNT_UNBINDABLE: u64 = 0x400;
    pub const MNT_FORCE: u64 = 0x800;

    pub fn new(flags: u64) -> Self { Self(flags) }
    pub fn is_readonly(&self) -> bool { (self.0 & Self::MNT_READONLY) != 0 }
    pub fn is_noexec(&self) -> bool { (self.0 & Self::MNT_NOEXEC) != 0 }
    pub fn is_nosuid(&self) -> bool { (self.0 & Self::MNT_NOSUID) != 0 }
    pub fn bits(&self) -> u64 { self.0 }
}

proptest! {
    #[test]
    fn test_mnt_flags_are_powers_of_two(_v in 0u8..1u8) {
        let flags = [
            ("MNT_READONLY", MntFlags::MNT_READONLY),
            ("MNT_NOATIME", MntFlags::MNT_NOATIME),
            ("MNT_NODIRATIME", MntFlags::MNT_NODIRATIME),
            ("MNT_SYNCHRONOUS", MntFlags::MNT_SYNCHRONOUS),
            ("MNT_NOEXEC", MntFlags::MNT_NOEXEC),
            ("MNT_NOSUID", MntFlags::MNT_NOSUID),
            ("MNT_NODEV", MntFlags::MNT_NODEV),
            ("MNT_PRIVATE", MntFlags::MNT_PRIVATE),
            ("MNT_SHARED", MntFlags::MNT_SHARED),
            ("MNT_SLAVE", MntFlags::MNT_SLAVE),
            ("MNT_UNBINDABLE", MntFlags::MNT_UNBINDABLE),
            ("MNT_FORCE", MntFlags::MNT_FORCE),
        ];
        for (name, val) in &flags {
            assert!(*val > 0 && (val & (val - 1)) == 0,
                    "{} ({:#x}) must be a power of two", name, val);
        }
    }

    #[test]
    fn test_mnt_flags_distinct(_v in 0u8..1u8) {
        let flags = [
            MntFlags::MNT_READONLY, MntFlags::MNT_NOATIME,
            MntFlags::MNT_NODIRATIME, MntFlags::MNT_SYNCHRONOUS,
            MntFlags::MNT_NOEXEC, MntFlags::MNT_NOSUID,
            MntFlags::MNT_NODEV, MntFlags::MNT_PRIVATE,
            MntFlags::MNT_SHARED, MntFlags::MNT_SLAVE,
            MntFlags::MNT_UNBINDABLE, MntFlags::MNT_FORCE,
        ];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0,
                           "MNT flags {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_mnt_flags_sequential_bits(_v in 0u8..1u8) {
        // All MNT flags should be contiguous powers of two (bit 0 through bit 11)
        let flags = [
            MntFlags::MNT_READONLY, MntFlags::MNT_NOATIME,
            MntFlags::MNT_NODIRATIME, MntFlags::MNT_SYNCHRONOUS,
            MntFlags::MNT_NOEXEC, MntFlags::MNT_NOSUID,
            MntFlags::MNT_NODEV, MntFlags::MNT_PRIVATE,
            MntFlags::MNT_SHARED, MntFlags::MNT_SLAVE,
            MntFlags::MNT_UNBINDABLE, MntFlags::MNT_FORCE,
        ];
        for (i, &f) in flags.iter().enumerate() {
            assert_eq!(f, 1u64 << i, "MNT flag {} should be 1<<{}", i, i);
        }
    }

    #[test]
    fn test_mnt_readonly_check(flag_val in 0u64..(1u64 << 12)) {
        let f = MntFlags::new(flag_val);
        let expected = (flag_val & MntFlags::MNT_READONLY) != 0;
        assert_eq!(f.is_readonly(), expected);
    }

    #[test]
    fn test_mnt_noexec_check(flag_val in 0u64..(1u64 << 12)) {
        let f = MntFlags::new(flag_val);
        let expected = (flag_val & MntFlags::MNT_NOEXEC) != 0;
        assert_eq!(f.is_noexec(), expected);
    }

    #[test]
    fn test_mnt_nosuid_check(flag_val in 0u64..(1u64 << 12)) {
        let f = MntFlags::new(flag_val);
        let expected = (flag_val & MntFlags::MNT_NOSUID) != 0;
        assert_eq!(f.is_nosuid(), expected);
    }

    #[test]
    fn test_mnt_combined_flags(_v in 0u8..1u8) {
        let combined = MntFlags::new(MntFlags::MNT_READONLY | MntFlags::MNT_NOEXEC);
        assert!(combined.is_readonly());
        assert!(combined.is_noexec());
        assert!(!combined.is_nosuid());
    }

    #[test]
    fn test_mnt_bits_roundtrip(flag_val in 0u64..(1u64 << 12)) {
        let f = MntFlags::new(flag_val);
        assert_eq!(f.bits(), flag_val);
    }
}
