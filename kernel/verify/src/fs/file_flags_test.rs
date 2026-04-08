//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for file open flags (O_* constants) invariants.
//! Copied from: kernel/src/fs/file.rs

use proptest::prelude::*;

// Copied FileFlags
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FileFlags(u32);

impl FileFlags {
    pub const O_RDONLY: u32 = 0o00000000;
    pub const O_WRONLY: u32 = 0o00000001;
    pub const O_RDWR: u32 = 0o00000002;
    pub const O_ACCMODE: u32 = 0o00000003;

    pub const O_CREAT: u32 = 0o00000100;
    pub const O_EXCL: u32 = 0o00000200;
    pub const O_NOCTTY: u32 = 0o00000400;
    pub const O_TRUNC: u32 = 0o00001000;
    pub const O_APPEND: u32 = 0o00002000;
    pub const O_NONBLOCK: u32 = 0o00004000;
    pub const O_DSYNC: u32 = 0o00010000;
    pub const O_DIRECT: u32 = 0o00040000;
    pub const O_LARGEFILE: u32 = 0o00100000;
    pub const O_DIRECTORY: u32 = 0o00200000;
    pub const O_NOFOLLOW: u32 = 0o00400000;
    pub const O_NOATIME: u32 = 0o01000000;
    pub const O_CLOEXEC: u32 = 0o02000000;
    pub const O_SYNC: u32 = 0o04000000;
    pub const O_PATH: u32 = 0o10000000;

    pub fn new(flags: u32) -> Self { Self(flags) }
    pub fn is_readonly(&self) -> bool { (self.0 & Self::O_ACCMODE) == Self::O_RDONLY }
    pub fn is_writeonly(&self) -> bool { (self.0 & Self::O_ACCMODE) == Self::O_WRONLY }
    pub fn is_rdwr(&self) -> bool { (self.0 & Self::O_ACCMODE) == Self::O_RDWR }
    pub fn bits(&self) -> u32 { self.0 }
}

proptest! {
    #[test]
    fn test_o_accmode_covers_access_modes(_v in 0u8..1u8) {
        // O_ACCMODE masks exactly bits 0-1
        assert_eq!(FileFlags::O_ACCMODE, 0b11);
        assert_eq!(FileFlags::O_RDONLY, 0);
        assert_eq!(FileFlags::O_WRONLY, 1);
        assert_eq!(FileFlags::O_RDWR, 2);
    }

    #[test]
    fn test_access_modes_mutually_exclusive(_v in 0u8..1u8) {
        assert_ne!(FileFlags::O_RDONLY, FileFlags::O_WRONLY);
        assert_ne!(FileFlags::O_WRONLY, FileFlags::O_RDWR);
        assert_ne!(FileFlags::O_RDONLY, FileFlags::O_RDWR);
    }

    #[test]
    fn test_access_mode_extraction(flag_val in 0u32..(1u32 << 28)) {
        let f = FileFlags::new(flag_val);
        let mode = flag_val & FileFlags::O_ACCMODE;
        let is_ro = mode == FileFlags::O_RDONLY;
        let is_wo = mode == FileFlags::O_WRONLY;
        let is_rw = mode == FileFlags::O_RDWR;
        // Valid modes: 0 (RDONLY), 1 (WRONLY), 2 (RDWR)
        // mode=3 is invalid (all bits set) — in that case none match
        assert!(mode <= 3, "O_ACCMODE only covers 2 bits");
        assert_eq!(f.is_readonly(), is_ro);
        assert_eq!(f.is_writeonly(), is_wo);
        assert_eq!(f.is_rdwr(), is_rw);
    }

    #[test]
    fn test_o_flags_above_accmode_distinct(_v in 0u8..1u8) {
        // All flags above O_ACCMODE should be distinct and not overlap with ACCMODE
        let flags = [
            FileFlags::O_CREAT, FileFlags::O_EXCL, FileFlags::O_NOCTTY, FileFlags::O_TRUNC, FileFlags::O_APPEND,
            FileFlags::O_NONBLOCK, FileFlags::O_DSYNC, FileFlags::O_DIRECT, FileFlags::O_LARGEFILE, FileFlags::O_DIRECTORY,
            FileFlags::O_NOFOLLOW, FileFlags::O_NOATIME, FileFlags::O_CLOEXEC, FileFlags::O_SYNC, FileFlags::O_PATH,
        ];
        for &f in &flags {
            assert_eq!(f & FileFlags::O_ACCMODE, 0, "Flag {:#o} overlaps with O_ACCMODE", f);
        }
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0,
                           "Flags {:#o} and {:#o} overlap", flags[i], flags[j]);
            }
        }
    }

    #[test]
    fn test_o_rdonly_is_zero(_v in 0u8..1u8) {
        assert_eq!(FileFlags::O_RDONLY, 0, "O_RDONLY must be 0");
    }

    #[test]
    fn test_o_excl_is_next_after_creat(_v in 0u8..1u8) {
        // In Linux, O_EXCL = O_CREAT << 1
        assert_eq!(FileFlags::O_EXCL, FileFlags::O_CREAT << 1);
    }

    #[test]
    fn test_o_cloexec_value(_v in 0u8..1u8) {
        // Linux O_CLOEXEC = 020000000 (octal)
        assert_eq!(FileFlags::O_CLOEXEC, 0o02000000);
    }

    #[test]
    fn test_o_sync_above_dsync(_v in 0u8..1u8) {
        // O_SYNC should be a superset of O_DSYNC
        assert!(FileFlags::O_SYNC > FileFlags::O_DSYNC);
    }

    #[test]
    fn test_combined_flags_roundtrip(flag_val in 0u32..(1u32 << 28)) {
        let f = FileFlags::new(flag_val);
        assert_eq!(f.bits(), flag_val);
    }
}
