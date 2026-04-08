//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! FileFlags access-mode classifier and O_* constant invariant tests.
//!
//! Types copied from: kernel/src/fs/file.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/file.rs
// ============================================================================

#[repr(C)]
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

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_readonly(&self) -> bool {
        (self.0 & Self::O_ACCMODE) == Self::O_RDONLY
    }

    pub fn is_writeonly(&self) -> bool {
        (self.0 & Self::O_ACCMODE) == Self::O_WRONLY
    }

    pub fn is_rdwr(&self) -> bool {
        (self.0 & Self::O_ACCMODE) == Self::O_RDWR
    }

    pub fn bits(&self) -> u32 {
        self.0
    }

    pub fn set_bits(&mut self, flags: u32) {
        self.0 = flags;
    }

    pub fn add_flags(&mut self, flags: u32) {
        self.0 |= flags;
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-FILE-1: O_RDONLY is readonly
    #[test]
    fn test_readonly_basic(extra in 0u32..0o1777u32) {
        let flags = FileFlags::new(FileFlags::O_RDONLY | extra);
        // If extra doesn't touch access mode bits, it stays readonly
        if (extra & FileFlags::O_ACCMODE) == 0 {
            prop_assert!(flags.is_readonly());
        }
    }

    /// INV-FILE-2: O_WRONLY is writeonly
    #[test]
    fn test_writeonly_basic(_v in 0u8..1u8) {
        let flags = FileFlags::new(FileFlags::O_WRONLY);
        prop_assert!(flags.is_writeonly());
        prop_assert!(!flags.is_readonly());
        prop_assert!(!flags.is_rdwr());
    }

    /// INV-FILE-3: O_RDWR is rdwr
    #[test]
    fn test_rdwr_basic(_v in 0u8..1u8) {
        let flags = FileFlags::new(FileFlags::O_RDWR);
        prop_assert!(flags.is_rdwr());
        prop_assert!(!flags.is_readonly());
        prop_assert!(!flags.is_writeonly());
    }

    /// INV-FILE-4: access modes are mutually exclusive
    #[test]
    fn test_access_modes_exclusive(raw in 0u32..0o20000000u32) {
        let flags = FileFlags::new(raw);
        let modes = [
            flags.is_readonly(),
            flags.is_writeonly(),
            flags.is_rdwr(),
        ];
        let count = modes.iter().filter(|&&m| m).count();
        prop_assert!(count <= 1, "at most one access mode should be true, got {}", count);
    }

    /// INV-FILE-5: bits() roundtrip
    #[test]
    fn test_bits_roundtrip(raw in 0u32..0o20000000u32) {
        let flags = FileFlags::new(raw);
        prop_assert_eq!(flags.bits(), raw);
    }

    /// INV-FILE-6: O_ACCMODE correctly isolates access mode (2-bit mask)
    #[test]
    fn test_accmode_mask(_v in 0u8..1u8) {
        prop_assert_eq!(FileFlags::O_ACCMODE, 0o00000003);
        // All three access mode values fit in the mask
        prop_assert!(FileFlags::O_RDONLY <= FileFlags::O_ACCMODE);
        prop_assert!(FileFlags::O_WRONLY <= FileFlags::O_ACCMODE);
        prop_assert!(FileFlags::O_RDWR <= FileFlags::O_ACCMODE);
    }

    /// INV-FILE-7: non-access-mode flags don't affect access classification
    #[test]
    fn test_extra_flags_dont_change_access(access in 0u32..3u32) {
        let base = FileFlags::new(access);
        let with_creat = FileFlags::new(access | FileFlags::O_CREAT);
        let with_append = FileFlags::new(access | FileFlags::O_APPEND);
        let with_cloexec = FileFlags::new(access | FileFlags::O_CLOEXEC);
        prop_assert_eq!(base.is_readonly(), with_creat.is_readonly());
        prop_assert_eq!(base.is_writeonly(), with_append.is_writeonly());
        prop_assert_eq!(base.is_rdwr(), with_cloexec.is_rdwr());
    }

    /// INV-FILE-8: add_flags is OR operation
    #[test]
    fn test_add_flags(raw in 0u32..0o10000u32, extra in 0u32..0o10000u32) {
        let mut flags = FileFlags::new(raw);
        flags.add_flags(extra);
        prop_assert_eq!(flags.bits(), raw | extra);
    }

    /// INV-FILE-9: set_bits replaces flags
    #[test]
    fn test_set_bits(raw in 0u32..0o777u32, new_flags in 0u32..0o777u32) {
        let mut flags = FileFlags::new(raw);
        flags.set_bits(new_flags);
        prop_assert_eq!(flags.bits(), new_flags);
    }

    /// INV-FILE-10: O_ACCMODE does not overlap with non-access flags
    #[test]
    fn test_accmode_no_overlap(_v in 0u8..1u8) {
        let non_access = FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_TRUNC
            | FileFlags::O_APPEND | FileFlags::O_NONBLOCK;
        prop_assert_eq!(non_access & FileFlags::O_ACCMODE, 0);
    }

    /// INV-FILE-11: O_CREAT with O_EXCL is valid combination
    #[test]
    fn test_creat_excl(_v in 0u8..1u8) {
        let flags = FileFlags::new(FileFlags::O_RDONLY | FileFlags::O_CREAT | FileFlags::O_EXCL);
        prop_assert!(flags.is_readonly());
    }
}
