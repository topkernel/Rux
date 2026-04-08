//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for ext4 directory entry parsing and iteration.
//! Copied from: kernel/src/fs/ext4/dir.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/fs/ext4/dir.rs
// ============================================================================

pub mod file_type {
    pub const EXT4_FT_UNKNOWN: u8 = 0;
    pub const EXT4_FT_REG_FILE: u8 = 1;
    pub const EXT4_FT_DIR: u8 = 2;
    pub const EXT4_FT_CHRDEV: u8 = 3;
    pub const EXT4_FT_BLKDEV: u8 = 4;
    pub const EXT4_FT_FIFO: u8 = 5;
    pub const EXT4_FT_SOCK: u8 = 6;
    pub const EXT4_FT_SYMLINK: u8 = 7;
}

#[derive(Debug, Clone)]
pub struct Ext4DirEntry {
    pub inode: u32,
    pub rec_len: u16,
    pub name_len: u8,
    pub file_type: u8,
    pub name: [u8; 255],
}

impl Ext4DirEntry {
    pub fn from_bytes(bytes: &[u8], block_size: usize) -> Self {
        let inode = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let rec_len = u16::from_le_bytes([bytes[4], bytes[5]]);
        let name_len = bytes[6];
        let file_type = bytes[7];

        let mut name = [0u8; 255];
        if name_len as usize + 8 <= block_size {
            let end = core::cmp::min(8 + name_len as usize, bytes.len());
            name[..end - 8].copy_from_slice(&bytes[8..end]);
        }

        Self { inode, rec_len, name_len, file_type, name }
    }

    pub fn get_name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    pub fn is_dir(&self) -> bool { self.file_type == file_type::EXT4_FT_DIR }
    pub fn is_reg(&self) -> bool { self.file_type == file_type::EXT4_FT_REG_FILE }
    pub fn is_symlink(&self) -> bool { self.file_type == file_type::EXT4_FT_SYMLINK }
}

// Iterator equivalent (simplified from kernel which uses Vec)
pub fn iterate_dir_entries(data: &[u8], block_size: usize) -> Vec<Ext4DirEntry> {
    let mut entries = Vec::new();
    let mut offset = 0usize;

    while offset < data.len() {
        if offset + 8 > data.len() { break; }
        let entry = Ext4DirEntry::from_bytes(&data[offset..], block_size);
        let rec = entry.rec_len as usize;
        if rec == 0 { break; } // guard against infinite loop
        offset += rec;

        if entry.inode != 0 {
            entries.push(entry);
        }
    }
    entries
}

pub fn ext4_find_entry(dir_data: &[u8], block_size: usize, name: &str) -> Option<Ext4DirEntry> {
    let entries = iterate_dir_entries(dir_data, block_size);
    for entry in entries {
        if entry.get_name() == name {
            return Some(entry);
        }
    }
    None
}

// ============================================================================
// Helper: build a valid directory block with given entries
// ============================================================================

fn build_dir_block(block_size: usize, entries: &[(&str, u32, u8)]) -> Vec<u8> {
    let mut block = vec![0u8; block_size];
    let mut offset = 0usize;

    for (i, (name, inode, ft)) in entries.iter().enumerate() {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u8;
        // rec_len must be 8-aligned and >= 8 + name_len
        let min_rec = 8 + name_bytes.len();
        let rec_len = if i == entries.len() - 1 {
            // Last entry: rec_len fills remaining space
            ((block_size - offset + 7) / 8 * 8).max(min_rec)
        } else {
            ((min_rec + 7) / 8 * 8)
        } as u16;

        block[offset..offset+4].copy_from_slice(&inode.to_le_bytes());
        block[offset+4..offset+6].copy_from_slice(&rec_len.to_le_bytes());
        block[offset+6] = name_len;
        block[offset+7] = *ft;
        block[offset+8..offset+8+name_bytes.len()].copy_from_slice(name_bytes);
        offset += rec_len as usize;
    }

    block
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    #[test]
    fn test_from_bytes_roundtrip(
        inode in 1u32..0xFFFF_FFFFu32,
        rec_len in 8u16..256u16,
        name_len in 0u8..100u8,
        file_type in 0u8..8u8,
    ) {
        let name_bytes: Vec<u8> = (0..name_len).map(|i| b'a' + (i % 26)).collect();
        let mut data = vec![0u8; 8 + name_len as usize];
        data[0..4].copy_from_slice(&inode.to_le_bytes());
        data[4..6].copy_from_slice(&rec_len.to_le_bytes());
        data[6] = name_len;
        data[7] = file_type;
        data[8..].copy_from_slice(&name_bytes);

        let entry = Ext4DirEntry::from_bytes(&data, 4096);
        prop_assert_eq!(entry.inode, inode);
        prop_assert_eq!(entry.rec_len, rec_len);
        prop_assert_eq!(entry.name_len, name_len);
        prop_assert_eq!(entry.file_type, file_type);
        prop_assert_eq!(&entry.name[..name_len as usize], &name_bytes[..]);
    }

    #[test]
    fn test_file_type_classification(ft in 0u8..8u8) {
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..6].copy_from_slice(&16u16.to_le_bytes());
        data[7] = ft;
        let entry = Ext4DirEntry::from_bytes(&data, 4096);

        prop_assert_eq!(entry.is_dir(), ft == file_type::EXT4_FT_DIR);
        prop_assert_eq!(entry.is_reg(), ft == file_type::EXT4_FT_REG_FILE);
        prop_assert_eq!(entry.is_symlink(), ft == file_type::EXT4_FT_SYMLINK);
    }

    #[test]
    fn test_get_name_valid_utf8(
        name_len in 1u8..50u8,
        seed in 0u32..1_000_000u32,
    ) {
        // Generate a valid ASCII name
        let name: Vec<u8> = (0..name_len)
            .map(|i| b'a' + ((seed + i as u32) % 26) as u8)
            .collect();
        let name_str = core::str::from_utf8(&name).unwrap();

        let mut data = vec![0u8; 8 + name_len as usize];
        data[0..4].copy_from_slice(&42u32.to_le_bytes());
        data[4..6].copy_from_slice(&16u16.to_le_bytes());
        data[6] = name_len;
        data[7] = 1;
        data[8..].copy_from_slice(&name);

        let entry = Ext4DirEntry::from_bytes(&data, 4096);
        prop_assert_eq!(entry.get_name(), name_str);
    }

    #[test]
    fn test_deleted_entry_skipped(
        valid_inode in 1u32..100u32,
        seed in 0u32..10_000u32,
    ) {
        let name1 = format!("file{}", seed % 100);
        let name2 = format!("file{}", (seed + 1) % 100);
        let block = build_dir_block(256, &[
            ("", 0, 0),           // deleted entry
            (&name1, valid_inode, 1),
            ("", 0, 0),           // another deleted
            (&name2, valid_inode + 1, 2),
        ]);

        let entries = iterate_dir_entries(&block, 256);
        prop_assert_eq!(entries.len(), 2);
        prop_assert_eq!(entries[0].inode, valid_inode);
        prop_assert_eq!(entries[1].inode, valid_inode + 1);
    }

    #[test]
    fn test_find_entry_exists(seed in 0u32..10_000u32) {
        let name = format!("target{}", seed % 100);
        let block = build_dir_block(512, &[
            ("other1", 10, 1),
            (&name, 42, 1),
            ("other2", 11, 2),
        ]);

        let found = ext4_find_entry(&block, 512, &name);
        prop_assert!(found.is_some());
        prop_assert_eq!(found.unwrap().inode, 42);
    }

    #[test]
    fn test_find_entry_not_found(seed in 0u32..10_000u32) {
        let missing = format!("missing{}", seed % 100);
        let block = build_dir_block(512, &[
            ("file1", 10, 1),
            ("file2", 11, 2),
        ]);

        let found = ext4_find_entry(&block, 512, &missing);
        prop_assert!(found.is_none());
    }

    #[test]
    fn test_iterator_respects_rec_len(
        inode1 in 1u32..100u32,
        inode2 in 1u32..100u32,
    ) {
        let block = build_dir_block(1024, &[
            ("aaa", inode1, 1),
            ("bbbb", inode2, 2),
        ]);

        let entries = iterate_dir_entries(&block, 1024);
        prop_assert_eq!(entries.len(), 2);
        prop_assert_eq!(entries[0].inode, inode1);
        prop_assert_eq!(entries[1].inode, inode2);
        prop_assert!(entries[0].get_name() == "aaa");
        prop_assert!(entries[1].get_name() == "bbbb");
    }

    #[test]
    fn test_empty_block_yields_no_entries(_v in 0u8..1u8) {
        let block = vec![0u8; 4096];
        let entries = iterate_dir_entries(&block, 4096);
        prop_assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_name_len_boundary(
        name_len in 1u8..=255u8,
        inode in 1u32..100u32,
    ) {
        let name: Vec<u8> = (0..name_len).map(|i| b'x').collect();
        let block_size = 4096;

        // Entry with maximum name length
        let rec_len = (((8 + name_len as usize) + 7) / 8 * 8) as u16;
        let mut data = vec![0u8; rec_len as usize];
        data[0..4].copy_from_slice(&inode.to_le_bytes());
        data[4..6].copy_from_slice(&rec_len.to_le_bytes());
        data[6] = name_len;
        data[7] = 1;
        data[8..8 + name_len as usize].copy_from_slice(&name);

        let entry = Ext4DirEntry::from_bytes(&data, block_size);
        prop_assert_eq!(entry.name_len, name_len);
        prop_assert_eq!(entry.get_name().len(), name_len as usize);
    }

    #[test]
    fn test_from_bytes_name_truncated_by_block_size(
        name_len in 100u8..200u8,
        inode in 1u32..100u32,
    ) {
        // block_size is small enough that name_len + 8 > block_size
        let small_block = 64usize;
        let name: Vec<u8> = (0..name_len).map(|i| b'a' + (i % 26)).collect();

        let rec_len = 64u16;
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&inode.to_le_bytes());
        data[4..6].copy_from_slice(&rec_len.to_le_bytes());
        data[6] = name_len; // oversized name_len
        data[7] = 1;
        let copy_end = core::cmp::min(8 + name_len as usize, small_block);
        data[8..copy_end].copy_from_slice(&name[..copy_end - 8]);

        let entry = Ext4DirEntry::from_bytes(&data, small_block);
        // When name_len + 8 > block_size, from_bytes skips copy → name stays all zeros
        if name_len as usize + 8 > small_block {
            prop_assert_eq!(&entry.name[..8], &[0u8; 8]);
        } else {
            prop_assert_eq!(&entry.name[..name_len as usize], &name[..]);
        }
    }
}
