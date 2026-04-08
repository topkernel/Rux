//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for ext4 inode mode decoding and block pointer access.
//! Copied from: kernel/src/fs/ext4/inode.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/ext4/inode.rs
// ============================================================================

pub mod file_type {
    pub const S_IFIFO: u16 = 0o010000;
    pub const S_IFCHR: u16 = 0o020000;
    pub const S_IFDIR: u16 = 0o040000;
    pub const S_IFBLK: u16 = 0o060000;
    pub const S_IFREG: u16 = 0o100000;
    pub const S_IFLNK: u16 = 0o120000;
    pub const S_IFSOCK: u16 = 0o140000;
    pub const S_IFMT: u16 = 0o170000;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Ext4InodeOnDisk {
    pub i_mode: u16,
    pub i_uid: u16,
    pub i_size: u32,
    pub i_atime: u32,
    pub i_ctime: u32,
    pub i_mtime: u32,
    pub i_dtime: u32,
    pub i_gid: u16,
    pub i_links_count: u16,
    pub i_blocks: u32,
    pub i_flags: u32,
    pub osd1: u32,
    pub i_block: [u32; 15],
    pub i_generation: u32,
    pub i_file_acl: u32,
    pub i_file_acl_high: u32,
    pub i_dir_acl: u32,
    pub i_dir_acl_high: u32,
    pub i_faddr: u32,
    pub osd2: [u8; 12],
    pub i_extra_isize: u16,
    pub i_checksum: u16,
    pub i_ctime_extra: u32,
    pub i_mtime_extra: u32,
    pub i_atime_extra: u32,
    pub i_crtime: u32,
    pub i_crtime_extra: u32,
    pub i_projid: u32,
    pub i_reserved: [u32; 4],
}

impl Ext4InodeOnDisk {
    pub fn is_dir(&self) -> bool { (self.i_mode & 0xF000) == 0x4000 }
    pub fn is_reg(&self) -> bool { (self.i_mode & 0xF000) == 0x8000 }
    pub fn is_symlink(&self) -> bool { (self.i_mode & 0xF000) == 0xA000 }
    pub fn has_extent(&self) -> bool { (self.i_flags & 0x80000) != 0 }
}

#[derive(Debug, Clone)]
pub struct Ext4Inode {
    pub ino: u32,
    pub mode: u16,
    pub uid: u16,
    pub gid: u16,
    pub size: u64,
    pub blocks: u64,
    pub links_count: u16,
    pub flags: u32,
    pub block: [u32; 15],
    pub atime: u32,
    pub mtime: u32,
    pub ctime: u32,
}

impl Ext4Inode {
    pub fn from_disk(disk: &Ext4InodeOnDisk, ino: u32) -> Self {
        Self {
            ino,
            mode: disk.i_mode,
            uid: disk.i_uid,
            gid: disk.i_gid,
            size: disk.i_size as u64,
            blocks: disk.i_blocks as u64,
            links_count: disk.i_links_count,
            flags: disk.i_flags,
            block: disk.i_block,
            atime: disk.i_atime,
            mtime: disk.i_mtime,
            ctime: disk.i_ctime,
        }
    }

    pub fn is_dir(&self) -> bool { (self.mode & 0xF000) == 0x4000 }
    pub fn is_reg(&self) -> bool { (self.mode & 0xF000) == 0x8000 }
    pub fn is_symlink(&self) -> bool { (self.mode & 0xF000) == 0xA000 }
    pub fn has_extent(&self) -> bool { (self.flags & 0x80000) != 0 }
}

/// Get block number from inode at given index (direct blocks only: 0-11)
pub fn get_block_nr(inode: &Ext4InodeOnDisk, block_idx: usize) -> Option<u64> {
    if block_idx < 12 {
        Some(inode.i_block[block_idx] as u64)
    } else {
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    #[test]
    fn test_ondisk_is_dir(mode in 0u16..0xFFFFu16) {
        let inode = Ext4InodeOnDisk { i_mode: mode, ..Default::default() };
        let expected = (mode & 0xF000) == 0x4000;
        prop_assert_eq!(inode.is_dir(), expected);
        prop_assert_eq!((mode & file_type::S_IFMT) == file_type::S_IFDIR, expected);
    }

    #[test]
    fn test_ondisk_is_reg(mode in 0u16..0xFFFFu16) {
        let inode = Ext4InodeOnDisk { i_mode: mode, ..Default::default() };
        let expected = (mode & 0xF000) == 0x8000;
        prop_assert_eq!(inode.is_reg(), expected);
        prop_assert_eq!((mode & file_type::S_IFMT) == file_type::S_IFREG, expected);
    }

    #[test]
    fn test_ondisk_is_symlink(mode in 0u16..0xFFFFu16) {
        let inode = Ext4InodeOnDisk { i_mode: mode, ..Default::default() };
        let expected = (mode & 0xF000) == 0xA000;
        prop_assert_eq!(inode.is_symlink(), expected);
        prop_assert_eq!((mode & file_type::S_IFMT) == file_type::S_IFLNK, expected);
    }

    #[test]
    fn test_mode_types_are_mutually_exclusive(mode in 0u16..0xFFFFu16) {
        let inode = Ext4InodeOnDisk { i_mode: mode, ..Default::default() };
        let type_bits = mode & 0xF000;
        // At most one of the three common types can be true
        let count = [inode.is_dir(), inode.is_reg(), inode.is_symlink()]
            .iter()
            .filter(|&&x| x)
            .count();
        prop_assert!(count <= 1, "at most one file type should match");
        // If type_bits matches a known type, exactly one should match
        if type_bits == 0x4000 || type_bits == 0x8000 || type_bits == 0xA000 {
            prop_assert_eq!(count, 1);
        }
    }

    #[test]
    fn test_has_extent(flags in 0u32..0x100000u32) {
        let inode = Ext4InodeOnDisk { i_flags: flags, ..Default::default() };
        prop_assert_eq!(inode.has_extent(), (flags & 0x80000) != 0);
    }

    #[test]
    fn test_has_extent_isolated_from_other_bits(
        base_flags in 0u32..0x7FFFFu32,
        extra in 0u32..0x100000u32,
    ) {
        let flags_without = base_flags & 0x7FFFF; // bit 19 clear
        let flags_with = flags_without | 0x80000;

        let inode_without = Ext4InodeOnDisk { i_flags: flags_without, ..Default::default() };
        let inode_with = Ext4InodeOnDisk { i_flags: flags_with, ..Default::default() };

        prop_assert!(!inode_without.has_extent());
        prop_assert!(inode_with.has_extent());
    }

    #[test]
    fn test_from_disk_copies_fields(
        ino in 1u32..1_000_000u32,
        mode in 0u16..0xFFFFu16,
        uid in 0u16..0xFFFFu16,
        gid in 0u16..0xFFFFu16,
        size in 0u32..0xFFFF_FFFFu32,
        flags in 0u32..0xFFFF_FFFFu32,
    ) {
        let mut disk = Ext4InodeOnDisk::default();
        disk.i_mode = mode;
        disk.i_uid = uid;
        disk.i_gid = gid;
        disk.i_size = size;
        disk.i_flags = flags;
        disk.i_links_count = 3;
        disk.i_blocks = 100;
        disk.i_atime = 1000;
        disk.i_mtime = 2000;
        disk.i_ctime = 3000;
        disk.i_block[0] = 42;
        disk.i_block[11] = 99;

        let inode = Ext4Inode::from_disk(&disk, ino);
        prop_assert_eq!(inode.ino, ino);
        prop_assert_eq!(inode.mode, mode);
        prop_assert_eq!(inode.uid, uid);
        prop_assert_eq!(inode.gid, gid);
        prop_assert_eq!(inode.size, size as u64);
        prop_assert_eq!(inode.flags, flags);
        prop_assert_eq!(inode.links_count, 3);
        prop_assert_eq!(inode.blocks, 100);
        prop_assert_eq!(inode.atime, 1000);
        prop_assert_eq!(inode.mtime, 2000);
        prop_assert_eq!(inode.ctime, 3000);
        prop_assert_eq!(inode.block[0], 42);
        prop_assert_eq!(inode.block[11], 99);
    }

    #[test]
    fn test_inode_mode_matches_ondisk(mode in 0u16..0xFFFFu16) {
        let disk = Ext4InodeOnDisk { i_mode: mode, ..Default::default() };
        let inode = Ext4Inode::from_disk(&disk, 1);
        prop_assert_eq!(inode.is_dir(), disk.is_dir());
        prop_assert_eq!(inode.is_reg(), disk.is_reg());
        prop_assert_eq!(inode.is_symlink(), disk.is_symlink());
    }

    #[test]
    fn test_inode_flags_matches_ondisk(flags in 0u32..0xFFFF_FFFFu32) {
        let disk = Ext4InodeOnDisk { i_flags: flags, ..Default::default() };
        let inode = Ext4Inode::from_disk(&disk, 1);
        prop_assert_eq!(inode.has_extent(), disk.has_extent());
    }

    #[test]
    fn test_get_block_nr_direct(
        block_val in 0u32..0xFFFF_FFFFu32,
        idx in 0usize..12usize,
    ) {
        let mut disk = Ext4InodeOnDisk::default();
        disk.i_block[idx] = block_val;
        prop_assert_eq!(get_block_nr(&disk, idx), Some(block_val as u64));
    }

    #[test]
    fn test_get_block_nr_indirect_fails(idx in 12usize..20usize) {
        let disk = Ext4InodeOnDisk::default();
        prop_assert_eq!(get_block_nr(&disk, idx), None);
    }

    #[test]
    fn test_get_block_nr_independent_slots(
        v0 in 0u32..0xFFFF_FFFFu32,
        v1 in 0u32..0xFFFF_FFFFu32,
    ) {
        let mut disk = Ext4InodeOnDisk::default();
        disk.i_block[0] = v0;
        disk.i_block[1] = v1;
        prop_assert_eq!(get_block_nr(&disk, 0), Some(v0 as u64));
        prop_assert_eq!(get_block_nr(&disk, 1), Some(v1 as u64));
    }

    #[test]
    fn test_s_ifmt_extracts_type_bits(mode in 0u16..0xFFFFu16) {
        prop_assert_eq!(mode & file_type::S_IFMT, mode & 0xF000);
    }
}
