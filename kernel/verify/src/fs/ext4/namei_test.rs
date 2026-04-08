//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 directory entry helpers invariant tests.
//!
//! Functions copied from: kernel/src/fs/ext4/namei.rs

use proptest::prelude::*;

// ============================================================================
// Copied functions from kernel/src/fs/ext4/namei.rs
// ============================================================================

/// EXT4 file type for directories (copied from kernel/src/fs/ext4/dir.rs)
pub const EXT4_FT_DIR: u8 = 2;

/// Find space for new entry in directory block
fn find_entry_space(block_data: &[u8], name_len: usize, block_size: usize) -> Option<usize> {
    let mut offset = 0;
    let required_len = ((8 + name_len + 3) & !3) as u16;

    while offset + 8 <= block_size {
        let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);

        if rec_len == 0 {
            break;
        }

        let name_len_entry = block_data[offset + 6] as usize;
        let used_len = ((8 + name_len_entry + 3) & !3) as u16;

        if rec_len >= used_len + required_len {
            return Some(offset);
        }

        offset += rec_len as usize;
    }

    None
}

/// Add entry to directory block at given offset
fn add_entry_to_block(
    block_data: &mut [u8],
    offset: usize,
    name: &[u8],
    ino: u32,
    file_type: u8,
    _block_size: usize,
) {
    let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);
    let existing_name_len = block_data[offset + 6] as usize;
    let used_len = ((8 + existing_name_len + 3) & !3) as u16;

    if rec_len <= used_len {
        return;
    }

    let new_rec_len = rec_len - used_len;

    let used_bytes = used_len.to_le_bytes();
    block_data[offset + 4] = used_bytes[0];
    block_data[offset + 5] = used_bytes[1];

    let new_offset = offset + used_len as usize;

    let ino_bytes = ino.to_le_bytes();
    block_data[new_offset..new_offset + 4].copy_from_slice(&ino_bytes);

    let new_rec_bytes = new_rec_len.to_le_bytes();
    block_data[new_offset + 4] = new_rec_bytes[0];
    block_data[new_offset + 5] = new_rec_bytes[1];

    block_data[new_offset + 6] = name.len() as u8;
    block_data[new_offset + 7] = file_type;

    block_data[new_offset + 8..new_offset + 8 + name.len()].copy_from_slice(name);
}

/// Create initial entry in empty block
fn create_initial_entry(
    block_data: &mut [u8],
    name: &[u8],
    ino: u32,
    file_type: u8,
    block_size: usize,
) {
    let rec_len = block_size as u16;

    let ino_bytes = ino.to_le_bytes();
    block_data[0..4].copy_from_slice(&ino_bytes);

    let rec_bytes = rec_len.to_le_bytes();
    block_data[4] = rec_bytes[0];
    block_data[5] = rec_bytes[1];

    block_data[6] = name.len() as u8;
    block_data[7] = file_type;

    block_data[8..8 + name.len()].copy_from_slice(name);
}

/// Create "." entry
fn create_dot_entry(ino: u32, rec_len: u16) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0..4].copy_from_slice(&ino.to_le_bytes());
    entry[4..6].copy_from_slice(&rec_len.to_le_bytes());
    entry[6] = 1;
    entry[7] = EXT4_FT_DIR;
    entry
}

/// Create ".." entry
fn create_dotdot_entry(ino: u32, rec_len: u16) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0..4].copy_from_slice(&ino.to_le_bytes());
    entry[4..6].copy_from_slice(&rec_len.to_le_bytes());
    entry[6] = 2;
    entry[7] = EXT4_FT_DIR;
    entry
}

/// Find previous entry in directory block
fn find_prev_entry(block_data: &[u8], target_offset: usize, block_size: usize) -> usize {
    let mut offset = 0;

    while offset + 8 <= block_size {
        let rec_len = u16::from_le_bytes([
            block_data[offset + 4],
            block_data[offset + 5],
        ]) as usize;

        if rec_len == 0 {
            break;
        }

        if offset + rec_len == target_offset {
            return offset;
        }

        offset += rec_len;
    }

    target_offset
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-NAMEI-1: find_entry_space in empty block finds offset 0
    #[test]
    fn test_find_space_empty_block(
        name_len in 1usize..20usize,
        block_size in 64usize..4096usize,
    ) {
        let block = vec![0u8; block_size];
        let result = find_entry_space(&block, name_len, block_size);
        // Empty block: rec_len is 0 at offset 0 (all zeros), so breaks immediately → None
        // Actually, block is all zeros, rec_len = 0 at first entry → break → None
        prop_assert!(result.is_none());
    }

    /// INV-NAMEI-2: find_entry_space in block with single large entry finds space
    #[test]
    fn test_find_space_single_entry(
        name_len in 1usize..12usize,
        block_size in 64usize..4096usize,
    ) {
        let mut block = vec![0u8; block_size];
        // Create an initial entry with rec_len = block_size
        create_initial_entry(&mut block, b"test", 42, 1, block_size);
        let result = find_entry_space(&block, name_len, block_size);
        prop_assert!(result.is_some());
        prop_assert_eq!(result.unwrap(), 0);
    }

    /// INV-NAMEI-3: find_entry_space with no space returns None
    #[test]
    fn test_find_space_no_room(
        block_size in 32usize..256usize,
    ) {
        let mut block = vec![0u8; block_size];
        // Create an entry whose name fills the entire block (block_size - 8 header bytes)
        let name_len = (block_size - 8) as u8;
        let name = vec![b'x'; name_len as usize];
        create_initial_entry(&mut block, &name, 42, 1, block_size);
        // Try to find space for a 12-byte name — no room should remain
        let result = find_entry_space(&block, 12, block_size);
        prop_assert!(result.is_none());
    }

    /// INV-NAMEI-4: create_dot_entry has correct structure
    #[test]
    fn test_dot_entry(ino in 1u32..1000u32, rec_len in 12u16..512u16) {
        let entry = create_dot_entry(ino, rec_len);
        let read_ino = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let read_rec_len = u16::from_le_bytes([entry[4], entry[5]]);
        prop_assert_eq!(read_ino, ino);
        prop_assert_eq!(read_rec_len, rec_len);
        prop_assert_eq!(entry[6], 1); // name_len = 1 for "."
        prop_assert_eq!(entry[7], EXT4_FT_DIR);
    }

    /// INV-NAMEI-5: create_dotdot_entry has correct structure
    #[test]
    fn test_dotdot_entry(ino in 1u32..1000u32, rec_len in 12u16..512u16) {
        let entry = create_dotdot_entry(ino, rec_len);
        let read_ino = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let read_rec_len = u16::from_le_bytes([entry[4], entry[5]]);
        prop_assert_eq!(read_ino, ino);
        prop_assert_eq!(read_rec_len, rec_len);
        prop_assert_eq!(entry[6], 2); // name_len = 2 for ".."
        prop_assert_eq!(entry[7], EXT4_FT_DIR);
    }

    /// INV-NAMEI-6: dot and dotdot have different name_len
    #[test]
    fn test_dot_dotdot_different(_v in 0u8..1u8) {
        let dot = create_dot_entry(1, 12);
        let dotdot = create_dotdot_entry(1, 12);
        prop_assert_ne!(dot[6], dotdot[6]);
        prop_assert_eq!(dot[6], 1);
        prop_assert_eq!(dotdot[6], 2);
    }

    /// INV-NAMEI-7: create_initial_entry writes correct fields
    #[test]
    fn test_create_initial_entry(
        ino in 1u32..10000u32,
        block_size in 64usize..4096usize,
    ) {
        let mut block = vec![0u8; block_size];
        let name = b"hello";
        create_initial_entry(&mut block, name, ino, 1, block_size);

        let read_ino = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let read_rec_len = u16::from_le_bytes([block[4], block[5]]);
        prop_assert_eq!(read_ino, ino);
        prop_assert_eq!(read_rec_len, block_size as u16);
        prop_assert_eq!(block[6], name.len() as u8);
        prop_assert_eq!(block[7], 1);
        prop_assert_eq!(&block[8..8 + name.len()], name);
    }

    /// INV-NAMEI-8: add_entry_to_block splits existing entry
    #[test]
    fn test_add_entry_split(
        ino1 in 1u32..1000u32,
        ino2 in 1u32..1000u32,
        block_size in 64usize..4096usize,
    ) {
        let mut block = vec![0u8; block_size];
        create_initial_entry(&mut block, b"first", ino1, 1, block_size);
        add_entry_to_block(&mut block, 0, b"second", ino2, 2, block_size);

        // First entry's rec_len should be trimmed to actual size
        let first_rec = u16::from_le_bytes([block[4], block[5]]);
        let first_used = ((8 + 5 + 3) & !3) as u16; // "first" = 5 chars
        prop_assert_eq!(first_rec, first_used);

        // Second entry at offset = first_used
        let second_offset = first_used as usize;
        let second_ino = u32::from_le_bytes([
            block[second_offset],
            block[second_offset + 1],
            block[second_offset + 2],
            block[second_offset + 3],
        ]);
        prop_assert_eq!(second_ino, ino2);
        prop_assert_eq!(block[second_offset + 6], 6); // "second" = 6 chars
        prop_assert_eq!(&block[second_offset + 8..second_offset + 14], b"second");
    }

    /// INV-NAMEI-9: find_prev_entry returns previous offset
    #[test]
    fn test_find_prev_entry(
        block_size in 128usize..4096usize,
    ) {
        let mut block = vec![0u8; block_size];
        create_initial_entry(&mut block, b"first", 1, 1, block_size);
        add_entry_to_block(&mut block, 0, b"second", 2, 2, block_size);

        let first_rec = u16::from_le_bytes([block[4], block[5]]) as usize;
        let second_offset = first_rec;

        let prev = find_prev_entry(&block, second_offset, block_size);
        prop_assert_eq!(prev, 0);
    }

    /// INV-NAMEI-10: find_prev_entry for first entry returns target_offset (no prev)
    #[test]
    fn test_find_prev_entry_first(
        block_size in 64usize..4096usize,
    ) {
        let mut block = vec![0u8; block_size];
        create_initial_entry(&mut block, b"only", 1, 1, block_size);

        let prev = find_prev_entry(&block, 0, block_size);
        prop_assert_eq!(prev, 0);
    }

    /// INV-NAMEI-11: entry length alignment is 4-byte aligned
    #[test]
    fn test_entry_alignment(name_len in 0usize..20usize) {
        let used_len = (8 + name_len + 3) & !3;
        prop_assert_eq!(used_len % 4, 0, "entry length {} not 4-aligned for name_len {}", used_len, name_len);
    }

    /// INV-NAMEI-12: find_prev_entry for nonexistent target returns target
    #[test]
    fn test_find_prev_nonexistent(
        block_size in 64usize..4096usize,
        target in 50usize..200usize,
    ) {
        let mut block = vec![0u8; block_size];
        create_initial_entry(&mut block, b"first", 1, 1, block_size);

        let result = find_prev_entry(&block, target, block_size);
        // Target is beyond the first entry → no match found → returns target
        prop_assert_eq!(result, target);
    }

    /// INV-NAMEI-13: rec_len sum of all entries equals block_size after split
    #[test]
    fn test_rec_len_sum(
        block_size in 128usize..4096usize,
    ) {
        let mut block = vec![0u8; block_size];
        create_initial_entry(&mut block, b"first", 1, 1, block_size);
        add_entry_to_block(&mut block, 0, b"second", 2, 2, block_size);

        // Read first entry's trimmed rec_len
        let first_rec = u16::from_le_bytes([block[4], block[5]]) as usize;
        let first_offset = first_rec;
        // Read second entry's rec_len
        let second_rec = u16::from_le_bytes([
            block[first_offset + 4],
            block[first_offset + 5],
        ]) as usize;

        prop_assert_eq!(first_rec + second_rec, block_size);
    }

    /// INV-NAMEI-14: create_dot/dotdot entries are exactly 8 bytes
    #[test]
    fn test_dot_entries_size(_v in 0u8..1u8) {
        let dot = create_dot_entry(1, 12);
        let dotdot = create_dotdot_entry(2, 12);
        prop_assert_eq!(dot.len(), 8);
        prop_assert_eq!(dotdot.len(), 8);
    }
}
