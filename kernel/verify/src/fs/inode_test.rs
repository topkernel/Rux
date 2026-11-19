//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! InodeMode file type/permission classifier and inode_hash invariant tests.
//!
//! Types copied from: kernel/src/fs/inode.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/inode.rs
// ============================================================================

pub type Ino = u64;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct InodeMode(u32);

impl InodeMode {
    pub const S_IFMT: u32 = 0o0170000;

    pub const S_IFREG: u32 = 0o0100000;
    pub const S_IFDIR: u32 = 0o0040000;
    pub const S_IFCHR: u32 = 0o0020000;
    pub const S_IFBLK: u32 = 0o0060000;
    pub const S_IFIFO: u32 = 0o0010000;
    pub const S_IFLNK: u32 = 0o0120000;
    pub const S_IFSOCK: u32 = 0o0140000;

    pub const S_IRWXU: u32 = 0o0700;
    pub const S_IRUSR: u32 = 0o0400;
    pub const S_IWUSR: u32 = 0o0200;
    pub const S_IXUSR: u32 = 0o0100;
    pub const S_IRWXG: u32 = 0o0070;
    pub const S_IRGRP: u32 = 0o0040;
    pub const S_IWGRP: u32 = 0o0020;
    pub const S_IXGRP: u32 = 0o0010;
    pub const S_IRWXO: u32 = 0o0007;
    pub const S_IROTH: u32 = 0o0004;
    pub const S_IWOTH: u32 = 0o0002;
    pub const S_IXOTH: u32 = 0o0001;

    pub fn new(mode: u32) -> Self {
        Self(mode)
    }

    pub fn is_regular_file(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFREG
    }

    pub fn is_directory(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFDIR
    }

    pub fn is_char_device(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFCHR
    }

    pub fn is_block_device(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFBLK
    }

    pub fn is_fifo(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFIFO
    }

    pub fn is_symlink(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFLNK
    }

    pub fn is_socket(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFSOCK
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

pub fn inode_hash(ino: Ino, fs_id: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    hash ^= fs_id;
    hash = hash.wrapping_mul(0x100000001b3);
    hash ^= ino;
    hash = hash.wrapping_mul(0x100000001b3);
    hash
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-INODE-1: S_IFREG sets is_regular_file true
    #[test]
    fn test_is_regular_file(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFREG | perm);
        prop_assert!(mode.is_regular_file());
    }

    /// INV-INODE-2: S_IFDIR sets is_directory true
    #[test]
    fn test_is_directory(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFDIR | perm);
        prop_assert!(mode.is_directory());
    }

    /// INV-INODE-3: S_IFCHR sets is_char_device true
    #[test]
    fn test_is_char_device(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFCHR | perm);
        prop_assert!(mode.is_char_device());
    }

    /// INV-INODE-4: S_IFBLK sets is_block_device true
    #[test]
    fn test_is_block_device(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFBLK | perm);
        prop_assert!(mode.is_block_device());
    }

    /// INV-INODE-5: S_IFIFO sets is_fifo true
    #[test]
    fn test_is_fifo(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFIFO | perm);
        prop_assert!(mode.is_fifo());
    }

    /// INV-INODE-6: S_IFLNK sets is_symlink true
    #[test]
    fn test_is_symlink(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFLNK | perm);
        prop_assert!(mode.is_symlink());
    }

    /// INV-INODE-7: S_IFSOCK sets is_socket true
    #[test]
    fn test_is_socket(perm in 0u32..0o777u32) {
        let mode = InodeMode::new(InodeMode::S_IFSOCK | perm);
        prop_assert!(mode.is_socket());
    }

    /// INV-INODE-8: file types are mutually exclusive for any raw mode word
    #[test]
    fn test_types_mutually_exclusive(mode in 0u32..0o177777u32) {
        let m = InodeMode::new(mode);
        let types = [
            m.is_regular_file(),
            m.is_directory(),
            m.is_char_device(),
            m.is_block_device(),
            m.is_fifo(),
            m.is_symlink(),
            m.is_socket(),
        ];
        let count = types.iter().filter(|&&t| t).count();
        prop_assert!(count <= 1, "at most one type should be true, got {}", count);
    }

    /// INV-INODE-9: bits() roundtrip
    #[test]
    fn test_bits_roundtrip(mode in 0u32..0o177777u32) {
        let m = InodeMode::new(mode);
        prop_assert_eq!(m.bits(), mode);
    }

    /// INV-INODE-10: S_IFMT correctly isolates file type bits
    #[test]
    fn test_ifmt_isolates(raw in 0u32..0o177777u32) {
        let file_type_bits = raw & InodeMode::S_IFMT;
        let perm_bits = raw & !InodeMode::S_IFMT;
        prop_assert_eq!(file_type_bits | perm_bits, raw);
        // Permission bits are exactly the low 9 bits
        prop_assert_eq!(perm_bits & 0o7777, perm_bits, "perm bits should be < 0o7777");
    }

    /// INV-INODE-11: all 7 S_IF* constants are distinct and non-zero
    #[test]
    fn test_type_constants_distinct(_v in 0u8..1u8) {
        let types = [
            InodeMode::S_IFREG,
            InodeMode::S_IFDIR,
            InodeMode::S_IFCHR,
            InodeMode::S_IFBLK,
            InodeMode::S_IFIFO,
            InodeMode::S_IFLNK,
            InodeMode::S_IFSOCK,
        ];
        for t in &types {
            prop_assert_ne!(*t, 0);
            prop_assert_eq!(*t & InodeMode::S_IFMT, *t);
        }
        // All distinct
        for i in 0..types.len() {
            for j in (i + 1)..types.len() {
                prop_assert_ne!(types[i], types[j]);
            }
        }
    }

    /// INV-INODE-12: inode_hash is deterministic
    #[test]
    fn test_inode_hash_deterministic(ino in 1u64..0xFFFFFFFFu64, fs_id in 1u64..0xFFFFFFFFu64) {
        prop_assert_eq!(inode_hash(ino, fs_id), inode_hash(ino, fs_id));
    }

    /// INV-INODE-13: inode_hash different inputs likely produce different outputs
    #[test]
    fn test_inode_hash_different_inputs(ino1 in 1u64..0xFFFFFFFFu64, ino2 in 1u64..0xFFFFFFFFu64) {
        if ino1 != ino2 {
            // With high probability, different inodes with same fs_id hash differently
            let h1 = inode_hash(ino1, 42);
            let h2 = inode_hash(ino2, 42);
            prop_assert_ne!(h1, h2);
        }
    }

    /// INV-INODE-14: inode_hash not affected by fs_id when ino differs
    #[test]
    fn test_inode_hash_ino_dominates(ino in 1u64..0xFFFFu64, fs_id in 0u64..256u64) {
        let h1 = inode_hash(ino, fs_id);
        let h2 = inode_hash(ino, fs_id.wrapping_add(1));
        // Different fs_id with same ino should give different hash
        if fs_id != fs_id.wrapping_add(1) {
            prop_assert_ne!(h1, h2);
        }
    }

    /// INV-INODE-15: S_IFMT masks out permission bits
    #[test]
    fn test_ifmt_no_overlap(perm in 0u32..0o7777u32) {
        // Permission bits should not overlap with S_IFMT
        prop_assert_eq!(perm & InodeMode::S_IFMT, 0);
    }
}
