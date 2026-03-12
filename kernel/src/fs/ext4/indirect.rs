//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 indirect block handling

use crate::errno;
use crate::fs::bio;

pub const POINTERS_PER_BLOCK: usize = 1024;

#[derive(Debug, Clone)]
struct BlockMappingLayer {
    /// Level index (0=direct block, 1=single indirect, 2=double indirect, 3=triple indirect)
    level: u32,
    /// Offset in block
    offset: usize,
    /// Pointed block number
    block: u64,
    /// Indirect block's own block number (for allocation)
    indirect_block: u64,
}

pub struct Ext4BlockIterator {
    /// Current block index
    current_block: u64,
    /// Total block count
    total_blocks: u64,
}

impl Ext4BlockIterator {
    /// Create new block iterator
    pub fn new(total_blocks: u64) -> Self {
        Self {
            current_block: 0,
            total_blocks,
        }
    }

    /// Get next block's mapping information
    ///
    /// Returns (level, offset within level)
    pub fn next_mapping(&mut self) -> Option<(u32, usize)> {
        if self.current_block >= self.total_blocks {
            return None;
        }

        let block = self.current_block;
        self.current_block += 1;

        // Direct blocks (0-11)
        if block < 12 {
            return Some((0, block as usize));
        }

        // Single indirect blocks (12 - 1035)
        let indirect = block - 12;
        if indirect < POINTERS_PER_BLOCK as u64 {
            return Some((1, indirect as usize));
        }

        // Double indirect blocks (1036 - 1048603)
        let double = indirect - POINTERS_PER_BLOCK as u64;
        if double < (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK) as u64 {
            let _first = double as usize / POINTERS_PER_BLOCK;
            let _second = double as usize % POINTERS_PER_BLOCK;
            // Return (2, (first, second)) but we need to handle separately
            return Some((2, double as usize));
        }

        // Triple indirect blocks
        let triple = double - (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK) as u64;
        Some((3, triple as usize))
    }
}

pub fn ext4_get_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_array: &[u32; 15],
    block_index: u64,
) -> Result<u64, i32> {
    let block_size = fs.block_size as u64;

    // Direct blocks (0-11)
    if block_index < 12 {
        let block_num = block_array[block_index as usize];
        if block_num == 0 {
            return Ok(0);  // Sparse file, block not allocated
        }
        return Ok(block_num as u64);
    }

    // Single indirect blocks
    let indirect_offset = block_index - 12;
    let pointers_per_block = block_size / 4;

    if indirect_offset < pointers_per_block {
        // Single indirect block is at i_block[12]
        let indirect_block = block_array[12];
        if indirect_block == 0 {
            return Ok(0);  // Not allocated
        }
        return read_indirect_block(fs, indirect_block as u64, indirect_offset as usize);
    }

    // Double indirect blocks
    let double_offset = indirect_offset - pointers_per_block;
    let double_pointers = pointers_per_block * pointers_per_block;

    if double_offset < double_pointers {
        // Double indirect block is at i_block[13]
        let double_block = block_array[13];
        if double_block == 0 {
            return Ok(0);
        }

        // First level: get single indirect block number
        let first_index = (double_offset / pointers_per_block) as usize;
        let indirect_block = read_indirect_block(fs, double_block as u64, first_index)?;

        if indirect_block == 0 {
            return Ok(0);
        }

        // Second level: get data block number
        let second_index = (double_offset % pointers_per_block) as usize;
        return read_indirect_block(fs, indirect_block, second_index);
    }

    // Triple indirect blocks
    let triple_offset = double_offset - double_pointers;

    // Triple indirect block is at i_block[14]
    let triple_block = block_array[14];
    if triple_block == 0 {
        return Ok(0);
    }

    // First level: get double indirect block number
    let first_index = (triple_offset / double_pointers) as usize;
    let double_block = read_indirect_block(fs, triple_block as u64, first_index)?;

    if double_block == 0 {
        return Ok(0);
    }

    // Second level: get single indirect block number
    let remaining = triple_offset % double_pointers;
    let second_index = (remaining / pointers_per_block) as usize;
    let indirect_block = read_indirect_block(fs, double_block, second_index)?;

    if indirect_block == 0 {
        return Ok(0);
    }

    // Third level: get data block number
    let third_index = (remaining % pointers_per_block) as usize;
    read_indirect_block(fs, indirect_block, third_index)
}

pub fn read_indirect_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    indirect_block: u64,
    index: usize,
) -> Result<u64, i32> {
    unsafe {
        let bh = bio::bread(fs.device, indirect_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &(*bh).b_data;
        let block_numbers = reinterpret_slice::<u32>(data);

        if index >= block_numbers.len() {
            bio::brelse(bh);
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let block_num = block_numbers[index] as u64;

        bio::brelse(bh);
        Ok(block_num)
    }
}

pub fn write_indirect_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    indirect_block: u64,
    index: usize,
    block_num: u32,
) -> Result<(), i32> {
    unsafe {
        let bh = bio::bread(fs.device, indirect_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &mut (*bh).b_data;
        let block_numbers = reinterpret_slice_mut::<u32>(data);

        if index >= block_numbers.len() {
            bio::brelse(bh);
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        block_numbers[index] = block_num;

        (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
        bio::sync_dirty_buffer(bh)?;
        bio::brelse(bh);
        Ok(())
    }
}

pub fn max_file_size(block_size: u64) -> u64 {
    let pointers_per_block = block_size / 4;

    // Direct blocks
    let direct = 12 * block_size;

    // Single indirect blocks
    let single = pointers_per_block * block_size;

    // Double indirect blocks
    let double = pointers_per_block * pointers_per_block * block_size;

    // Triple indirect blocks
    let triple = pointers_per_block * pointers_per_block * pointers_per_block * block_size;

    direct + single + double + triple
}

pub fn get_indirect_level(size: u64, block_size: u64) -> u32 {
    let blocks = (size + block_size - 1) / block_size;

    if blocks <= 12 {
        return 0;
    }

    let pointers_per_block = block_size / 4;

    if blocks <= 12 + pointers_per_block {
        return 1;
    }

    let double_pointers = pointers_per_block * pointers_per_block;

    if blocks <= 12 + pointers_per_block + double_pointers {
        return 2;
    }

    3
}

unsafe fn reinterpret_slice<T>(data: &[u8]) -> &[T] {
    core::slice::from_raw_parts(
        data.as_ptr() as *const T,
        data.len() / core::mem::size_of::<T>(),
    )
}

unsafe fn reinterpret_slice_mut<T>(data: &mut [u8]) -> &mut [T] {
    core::slice::from_raw_parts_mut(
        data.as_ptr() as *mut T,
        data.len() / core::mem::size_of::<T>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_file_size() {
        let block_size = 4096u64;
        let max_size = max_file_size(block_size);

        // Should support files over 4TB
        assert!(max_size > 4_000_000_000_000);
    }

    #[test]
    fn test_indirect_level() {
        let block_size = 4096u64;

        // Small file: only uses direct blocks
        assert_eq!(get_indirect_level(48 * 1024, block_size), 0);

        // Medium file: needs single indirect blocks
        assert_eq!(get_indirect_level(100 * 1024, block_size), 1);
        assert_eq!(get_indirect_level(4 * 1024 * 1024, block_size), 1);

        // Large file: needs double indirect blocks
        assert_eq!(get_indirect_level(5 * 1024 * 1024, block_size), 2);
        assert_eq!(get_indirect_level(4 * 1024 * 1024 * 1024, block_size), 2);

        // Very large file: needs triple indirect blocks
        assert_eq!(get_indirect_level(5 * 1024 * 1024 * 1024u64, block_size), 3);
    }

    #[test]
    fn test_block_iterator() {
        let mut iter = Ext4BlockIterator::new(20);

        // First 12 should be direct blocks
        for i in 0..12 {
            let (level, offset) = iter.next_mapping().unwrap();
            assert_eq!(level, 0);
            assert_eq!(offset, i as usize);
        }

        // Next 8 should be single indirect blocks
        for i in 0..8 {
            let (level, offset) = iter.next_mapping().unwrap();
            assert_eq!(level, 1);
            assert_eq!(offset, i as usize);
        }

        // 21st should return None
        assert!(iter.next_mapping().is_none());
    }
}
