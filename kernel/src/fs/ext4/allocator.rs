//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 block and inode allocator

use alloc::vec::Vec;

use crate::errno;
use crate::fs::bio;
use crate::fs::ext4::superblock::Ext4GroupDesc;

pub struct BlockAllocator<'a> {
    fs: &'a super::Ext4FileSystem,
}

impl<'a> BlockAllocator<'a> {
    /// Create new block allocator
    pub fn new(fs: &'a super::Ext4FileSystem) -> Self {
        Self { fs }
    }

    /// Allocate a block
    ///
    ///
    /// # Returns
    /// Block number on success, error code on failure
    pub fn alloc_block(&self) -> Result<u64, i32> {
        // 1. Find block group with free blocks
        let block_groups = self.fs.group_count;
        let blocks_per_group = self.fs.blocks_per_group as u64;
        let first_data_block = self.fs.sb_info.as_ref()
            .map(|sb| sb.s_first_data_block as u64)
            .unwrap_or(0);

        // Iterate all block groups to find free blocks
        for group_idx in 0..block_groups {
            let (free_blocks, block_bitmap_block) = {
                let group_descs = self.fs.group_descs.lock();
                let group_desc = &group_descs[group_idx as usize];
                (group_desc.bg_free_blocks_count, group_desc.bg_block_bitmap as u64)
            };

            // Check if there are free blocks
            if free_blocks == 0 {
                continue;
            }

            // Block bitmap block number
            if block_bitmap_block == 0 {
                continue;
            }

            // Read block bitmap
            let bitmap = self.read_block_bitmap(block_bitmap_block)?;

            // Find free bit in bitmap
            // For group 0, never allocate block 0 (contains superblock)
            let start = if group_idx == 0 {
                core::cmp::max(first_data_block, 1)
            } else {
                0
            };

            if let Some(block_offset) = self.find_free_bit(&bitmap, start, blocks_per_group) {
                // Calculate actual block number
                let block_number = (group_idx as u64) * blocks_per_group + block_offset;

                // Mark block as used
                self.mark_block_used(group_idx as u64, block_offset as usize, block_bitmap_block)?;

                // Update in-memory group descriptor
                {
                    let mut group_descs = self.fs.group_descs.lock();
                    group_descs[group_idx as usize].bg_free_blocks_count -= 1;
                }

                // Update group descriptor on disk (decrement free block count)
                self.update_group_desc_free_blocks(group_idx as u64, free_blocks - 1)?;

                // Update superblock (decrement free block count)
                self.update_superblock_free_blocks(-1)?;

                return Ok(block_number);
            }
        }

        // No available free blocks
        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// Free a block
    ///
    ///
    /// # Parameters
    /// - `block`: Block number to free
    pub fn free_block(&self, block: u64) -> Result<(), i32> {
        let blocks_per_group = self.fs.blocks_per_group as u64;
        let block_groups = self.fs.group_count as u64;

        // Calculate which group the block is in
        let group_idx = block / blocks_per_group;
        if group_idx >= block_groups {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let block_offset = (block % blocks_per_group) as usize;

        // Read group descriptor
        let (free_blocks, block_bitmap_block) = {
            let group_descs = self.fs.group_descs.lock();
            let group_desc = &group_descs[group_idx as usize];
            (group_desc.bg_free_blocks_count, group_desc.bg_block_bitmap as u64)
        };

        // Read block bitmap
        let mut bitmap = self.read_block_bitmap(block_bitmap_block)?;

        // Clear corresponding bit in bitmap
        let byte_idx = block_offset / 8;
        let bit_idx = block_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] &= !(1 << bit_idx);

            // Write back bitmap
            self.write_block_bitmap(block_bitmap_block, &bitmap)?;

            // Update in-memory group descriptor
            {
                let mut group_descs = self.fs.group_descs.lock();
                group_descs[group_idx as usize].bg_free_blocks_count += 1;
            }

            // Update group descriptor on disk (increment free block count)
            self.update_group_desc_free_blocks(group_idx, free_blocks + 1)?;

            // Update superblock (increment free block count)
            self.update_superblock_free_blocks(1)?;

            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    /// Read block bitmap
    fn read_block_bitmap(&self, bitmap_block: u64) -> Result<Vec<u8>, i32> {
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &(*bh).b_data;
            let bitmap = data.to_vec();

            bio::brelse(bh);

            Ok(bitmap)
        }
    }

    /// Write back block bitmap
    fn write_block_bitmap(&self, bitmap_block: u64, bitmap: &[u8]) -> Result<(), i32> {
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            data.copy_from_slice(bitmap);

            // Mark as dirty and sync
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// Find free bit in bitmap
    fn find_free_bit(&self, bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
        let start_bit = start as usize;

        for (i, &byte) in bitmap.iter().enumerate() {
            let bit_offset = i * 8;

            // Skip bits before start position
            if bit_offset + 8 <= start_bit {
                continue;
            }

            // Check if byte has unset bits
            if byte != 0xFF {
                for bit in 0..8 {
                    let abs_bit = bit_offset + bit;

                    // Beyond max bits
                    if abs_bit as u64 >= max_bits {
                        break;
                    }

                    // Skip bits before start position
                    if abs_bit < start_bit {
                        continue;
                    }

                    // Check if bit is 0 (free)
                    if (byte & (1 << bit)) == 0 {
                        return Some(abs_bit as u64);
                    }
                }
            }
        }

        None
    }

    /// Mark block as used
    fn mark_block_used(&self, _group_idx: u64, block_offset: usize, bitmap_block: u64) -> Result<(), i32> {
        let mut bitmap = self.read_block_bitmap(bitmap_block)?;

        let byte_idx = block_offset / 8;
        let bit_idx = block_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] |= 1 << bit_idx;
            self.write_block_bitmap(bitmap_block, &bitmap)?;

            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    /// Update free block count in group descriptor
    fn update_group_desc_free_blocks(&self, group_idx: u64, free_blocks: u16) -> Result<(), i32> {
        // In ext4, group descriptor location on disk is fixed
        // We need to find the block containing the group descriptor and update it

        let group_desc_size = self.fs.desc_size as usize;  // Use actual size from superblock
        let group_desc_start_block = if self.fs.block_size == 1024 {
            2  // Group descriptors start at block 2 (block 0=boot, block 1=superblock)
        } else {
            1  // Group descriptors start at block 1 (block 0 contains superblock)
        };

        let desc_per_block = self.fs.block_size as u64 / group_desc_size as u64;
        let desc_block = group_desc_start_block + (group_idx / desc_per_block);
        let desc_offset = ((group_idx % desc_per_block) as usize) * group_desc_size;

        unsafe {
            let bh = bio::bread(self.fs.device, desc_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            // Update free block count (offset = position of bg_free_blocks_count in Ext4GroupDesc)
            let free_blocks_ptr = data.as_mut_ptr().add(desc_offset + 12) as *mut u16;
            free_blocks_ptr.write_volatile(free_blocks);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// Update free block count in superblock
    fn update_superblock_free_blocks(&self, delta: i32) -> Result<(), i32> {
        unsafe {
            // Superblock is always at block 1 (for 1024 byte blocks) or block 0 (for larger blocks)
            let sb_block = if self.fs.block_size == 1024 { 1 } else { 0 };

            let bh = bio::bread(self.fs.device, sb_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;

            // Superblock starts at byte 1024 within the block
            // s_free_blocks_count is at offset 12 within the superblock (4th u32 field)
            let sb_start = if self.fs.block_size == 1024 { 0 } else { 1024 };
            let free_blocks_ptr = data.as_mut_ptr().add(sb_start + 12) as *mut u32;

            let current = free_blocks_ptr.read_volatile();
            let new = (current as i32 + delta) as u32;
            free_blocks_ptr.write_volatile(new);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }
}

pub struct InodeAllocator<'a> {
    fs: &'a super::Ext4FileSystem,
}

impl<'a> InodeAllocator<'a> {
    /// Create new inode allocator
    pub fn new(fs: &'a super::Ext4FileSystem) -> Self {
        Self { fs }
    }

    /// Allocate an inode
    ///
    ///
    /// # Returns
    /// Inode number on success, error code on failure
    pub fn alloc_inode(&self) -> Result<u32, i32> {
        let block_groups = self.fs.group_count;
        let inodes_per_group = self.fs.inodes_per_group as u64;

        // Iterate all block groups to find free inodes
        for group_idx in 0..block_groups {
            let (free_inodes, inode_bitmap_block) = {
                let group_descs = self.fs.group_descs.lock();
                let group_desc = &group_descs[group_idx as usize];
                (group_desc.bg_free_inodes_count, group_desc.bg_inode_bitmap as u64)
            };

            // Check if there are free inodes
            if free_inodes == 0 {
                continue;
            }

            // Inode bitmap block number
            if inode_bitmap_block == 0 {
                continue;
            }

            // Read inode bitmap
            let bitmap = self.read_inode_bitmap(inode_bitmap_block)?;

            // Find free inode in bitmap
            // In ext4, inodes start counting from 1 (0 is reserved)
            if let Some(inode_offset) = self.find_free_bit(&bitmap, 1, inodes_per_group) {
                // Calculate actual inode number
                let inode_number = (group_idx as u64) * inodes_per_group + inode_offset;

                // Mark inode as used
                self.mark_inode_used(group_idx as u64, inode_offset as usize, inode_bitmap_block)?;

                // Update group descriptor (decrement free inode count)
                self.update_group_desc_free_inodes(group_idx as u64, free_inodes - 1)?;

                // Update superblock (decrement free inode count)
                self.update_superblock_free_inodes(-1)?;

                return Ok(inode_number as u32);
            }
        }

        // No available free inodes
        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// Free an inode
    ///
    ///
    /// # Parameters
    /// - `ino`: Inode number to free
    pub fn free_inode(&self, ino: u32) -> Result<(), i32> {
        let inodes_per_group = self.fs.inodes_per_group as u64;
        let block_groups = self.fs.group_count as u64;

        // Calculate which group the inode is in
        let group_idx = (ino as u64 - 1) / inodes_per_group;
        if group_idx >= block_groups {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let inode_offset = ((ino as u64 - 1) % inodes_per_group) as usize;

        // Read group descriptor
        let (free_inodes, inode_bitmap_block) = {
            let group_descs = self.fs.group_descs.lock();
            let group_desc = &group_descs[group_idx as usize];
            (group_desc.bg_free_inodes_count, group_desc.bg_inode_bitmap as u64)
        };

        // Read inode bitmap
        let mut bitmap = self.read_inode_bitmap(inode_bitmap_block)?;

        // Clear corresponding bit in bitmap
        let byte_idx = inode_offset / 8;
        let bit_idx = inode_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] &= !(1 << bit_idx);

            // Write back bitmap
            self.write_inode_bitmap(inode_bitmap_block, &bitmap)?;

            // Update group descriptor (increment free inode count)
            self.update_group_desc_free_inodes(group_idx, free_inodes + 1)?;

            // Update superblock (increment free inode count)
            self.update_superblock_free_inodes(1)?;

            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    /// Read inode bitmap
    fn read_inode_bitmap(&self, bitmap_block: u64) -> Result<Vec<u8>, i32> {
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &(*bh).b_data;
            let bitmap = data.to_vec();

            bio::brelse(bh);

            Ok(bitmap)
        }
    }

    /// Write back inode bitmap
    fn write_inode_bitmap(&self, bitmap_block: u64, bitmap: &[u8]) -> Result<(), i32> {
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            data.copy_from_slice(bitmap);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// Find free bit in bitmap
    fn find_free_bit(&self, bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
        let start_bit = start as usize;

        for (i, &byte) in bitmap.iter().enumerate() {
            let bit_offset = i * 8;

            // Skip bits before start position
            if bit_offset + 8 <= start_bit {
                continue;
            }

            // Check if byte has unset bits
            if byte != 0xFF {
                for bit in 0..8 {
                    let abs_bit = bit_offset + bit;

                    // Beyond max bits
                    if abs_bit as u64 >= max_bits {
                        break;
                    }

                    // Skip bits before start position
                    if abs_bit < start_bit {
                        continue;
                    }

                    // Check if bit is 0 (free)
                    if (byte & (1 << bit)) == 0 {
                        return Some(abs_bit as u64);
                    }
                }
            }
        }

        None
    }

    /// Mark inode as used
    fn mark_inode_used(&self, _group_idx: u64, inode_offset: usize, bitmap_block: u64) -> Result<(), i32> {
        let mut bitmap = self.read_inode_bitmap(bitmap_block)?;

        let byte_idx = inode_offset / 8;
        let bit_idx = inode_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] |= 1 << bit_idx;
            self.write_inode_bitmap(bitmap_block, &bitmap)?;
            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    /// Update free inode count in group descriptor
    fn update_group_desc_free_inodes(&self, group_idx: u64, free_inodes: u16) -> Result<(), i32> {
        let group_desc_size = self.fs.desc_size as usize;  // Use actual size from superblock
        let group_desc_start_block = if self.fs.block_size == 1024 {
            2
        } else {
            1
        };

        let desc_per_block = self.fs.block_size as u64 / group_desc_size as u64;
        let desc_block = group_desc_start_block + (group_idx / desc_per_block);
        let desc_offset = ((group_idx % desc_per_block) as usize) * group_desc_size;

        unsafe {
            let bh = bio::bread(self.fs.device, desc_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            // Update free inode count (bg_free_inodes_count offset in Ext4GroupDesc)
            let free_inodes_ptr = data.as_mut_ptr().add(desc_offset + 14) as *mut u16;
            free_inodes_ptr.write_volatile(free_inodes);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// Update free inode count in superblock
    fn update_superblock_free_inodes(&self, delta: i32) -> Result<(), i32> {
        unsafe {
            let sb_block = if self.fs.block_size == 1024 { 1 } else { 0 };

            let bh = bio::bread(self.fs.device, sb_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;

            // Superblock starts at byte 1024 within the block
            // s_free_inodes_count is at offset 16 within the superblock (5th u32 field)
            let sb_start = if self.fs.block_size == 1024 { 0 } else { 1024 };
            let free_inodes_ptr = data.as_mut_ptr().add(sb_start + 16) as *mut u32;

            let current = free_inodes_ptr.read_volatile();
            let new = (current as i32 + delta) as u32;
            free_inodes_ptr.write_volatile(new);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }
}
