//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File type/permission mode invariant tests.
//!
//! Types copied from: kernel/src/fs/stat.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/stat.rs
// ============================================================================

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Stat {
    pub st_mode: u32,
}

impl Stat {
    pub fn new() -> Self {
        Self { st_mode: 0 }
    }

    pub fn set_regular_file(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o100000;
    }

    pub fn set_directory(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o040000;
    }

    pub fn set_char_device(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o020000;
    }

    pub fn set_block_device(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o060000;
    }

    pub fn set_fifo(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o010000;
    }

    pub fn set_symlink(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o120000;
    }

    pub fn set_socket(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o140000;
    }

    pub fn is_regular_file(&self) -> bool {
        (self.st_mode & 0o170000) == 0o100000
    }

    pub fn is_directory(&self) -> bool {
        (self.st_mode & 0o170000) == 0o040000
    }

    pub fn is_char_device(&self) -> bool {
        (self.st_mode & 0o170000) == 0o020000
    }

    pub fn is_block_device(&self) -> bool {
        (self.st_mode & 0o170000) == 0o060000
    }

    pub fn is_fifo(&self) -> bool {
        (self.st_mode & 0o170000) == 0o010000
    }

    pub fn is_symlink(&self) -> bool {
        (self.st_mode & 0o170000) == 0o120000
    }

    pub fn is_socket(&self) -> bool {
        (self.st_mode & 0o170000) == 0o140000
    }

    pub fn set_mode(&mut self, mode: u32) {
        self.st_mode &= 0o170000;
        self.st_mode |= mode & 0o777;
    }

    pub fn get_mode(&self) -> u32 {
        self.st_mode & 0o777
    }
}

/// Count how many is_* methods return true.
fn count_types(s: &Stat) -> usize {
    let mut count = 0;
    if s.is_regular_file() { count += 1; }
    if s.is_directory() { count += 1; }
    if s.is_char_device() { count += 1; }
    if s.is_block_device() { count += 1; }
    if s.is_fifo() { count += 1; }
    if s.is_symlink() { count += 1; }
    if s.is_socket() { count += 1; }
    count
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-STAT-1: set_regular_file makes is_regular_file true
    #[test]
    fn test_set_regular_file(perm in 0u32..0o777u32) {
        let mut s = Stat::new();
        s.set_mode(perm);
        s.set_regular_file();
        prop_assert!(s.is_regular_file());
        prop_assert_eq!(s.get_mode(), perm);
    }

    /// INV-STAT-2: set_directory makes is_directory true
    #[test]
    fn test_set_directory(perm in 0u32..0o777u32) {
        let mut s = Stat::new();
        s.set_mode(perm);
        s.set_directory();
        prop_assert!(s.is_directory());
        prop_assert_eq!(s.get_mode(), perm);
    }

    /// INV-STAT-3: set_mode/get_mode roundtrip
    #[test]
    fn test_mode_roundtrip(mode in 0u32..0o777u32) {
        let mut s = Stat::new();
        s.set_mode(mode);
        prop_assert_eq!(s.get_mode(), mode);
    }

    /// INV-STAT-4: set_mode preserves file type
    #[test]
    fn test_set_mode_preserves_type(mode in 0u32..0o777u32) {
        let mut s = Stat::new();
        s.set_directory();
        s.set_mode(mode);
        prop_assert!(s.is_directory());
    }

    /// INV-STAT-5: set_* preserves permission bits
    #[test]
    fn test_set_type_preserves_mode(perm in 0u32..0o777u32) {
        let mut s = Stat::new();
        s.set_mode(perm);
        s.set_regular_file();
        prop_assert_eq!(s.get_mode(), perm);
        s.set_socket();
        prop_assert_eq!(s.get_mode(), perm);
    }

    /// INV-STAT-6: get_mode returns only low 9 bits
    #[test]
    fn test_get_mode_low_bits(raw in 0u32..u32::MAX) {
        let s = Stat { st_mode: raw };
        prop_assert_eq!(s.get_mode(), raw & 0o777);
    }

    /// INV-STAT-7: File type bits are mutually exclusive for any raw mode
    #[test]
    fn test_mutual_exclusivity(raw in 0u32..u32::MAX) {
        let s = Stat { st_mode: raw };
        prop_assert!(count_types(&s) <= 1, "at most one file type should be true");
    }

    /// INV-STAT-8: set_type overwrites previous type
    #[test]
    fn test_type_overwrite(
        perm in 0u32..0o777u32,
    ) {
        let mut s = Stat::new();
        s.set_mode(perm);
        s.set_regular_file();
        prop_assert!(s.is_regular_file());
        s.set_directory();
        prop_assert!(s.is_directory());
        prop_assert!(!s.is_regular_file());
        prop_assert_eq!(s.get_mode(), perm);
    }

    /// INV-STAT-9: new() has no type and no mode
    #[test]
    fn test_new(_v in 0u8..1u8) {
        let s = Stat::new();
        prop_assert_eq!(count_types(&s), 0);
        prop_assert_eq!(s.get_mode(), 0);
    }

    /// INV-STAT-10: Randomized set_type + set_mode + check
    #[test]
    fn test_random_type_mode(
        type_code in 0u32..7u32,
        perm in 0u32..0o777u32,
    ) {
        let mut s = Stat::new();
        s.set_mode(perm);
        match type_code {
            0 => s.set_regular_file(),
            1 => s.set_directory(),
            2 => s.set_char_device(),
            3 => s.set_block_device(),
            4 => s.set_fifo(),
            5 => s.set_symlink(),
            6 => s.set_socket(),
            _ => {}
        }
        prop_assert_eq!(count_types(&s), 1);
        prop_assert_eq!(s.get_mode(), perm);
    }
}

#[test]
/// INV-STAT-11: All 7 file type codes are distinct
fn test_file_type_codes_distinct() {
    let codes = [
        0o100000, // regular
        0o040000, // directory
        0o020000, // char device
        0o060000, // block device
        0o010000, // fifo
        0o120000, // symlink
        0o140000, // socket
    ];
    let mut seen = std::collections::HashSet::new();
    for &c in &codes {
        assert!(seen.insert(c), "duplicate file type code: {:o}", c);
        assert_ne!(c, 0, "file type code should not be zero");
    }
}
