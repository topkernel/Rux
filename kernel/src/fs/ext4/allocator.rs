//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 block and inode allocator

use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;

/// Byte offset of `bg_free_blocks_count_lo` within ext4 group descriptor (32-bit desc):
/// bg_block_bitmap_lo(u32) + bg_inode_bitmap_lo(u32) + bg_inode_table_lo(u32) = 12
const BG_FREE_BLOCKS_OFF: usize = 12;

/// Byte offset of `s_free_blocks_count` within ext4 superblock:
/// s_inodes_count(u32) + s_blocks_count(u32) + s_r_blocks_count(u32) = 12
const SB_FREE_BLOCKS_OFF: usize = 12;

use crate::errno;
use crate::fs::bio;
use crate::fs::ext4::superblock::Ext4GroupDesc;

// ============================================================================
// Preallocation State
// ============================================================================

/// Preallocation window for a single inode.
///
/// When `alloc_block_with_prealloc` finds a cache miss, it allocates one
/// block for immediate use and then pre-allocates extra blocks in the same
/// group. Subsequent calls for the same inode consume from this cache.
pub struct PreallocState {
    /// Starting physical block number of the preallocated window
    pub start: u64,
    /// Number of blocks already consumed from the window
    pub len: u32,
    /// Total number of preallocated blocks
    pub total: u32,
    /// Target inode number
    pub ino: u32,
}

/// Number of extra blocks to preallocate beyond the requested one.
const PREALLOC_SIZE: u32 = 8;

pub struct BlockAllocator<'a> {
    fs: &'a super::Ext4FileSystem,
}

impl<'a> BlockAllocator<'a> {
    /// Create new block allocator
    pub fn new(fs: &'a super::Ext4FileSystem) -> Self {
        Self { fs }
    }

    /// Allocate a block, preferring `goal_group`.
    ///
    /// Search order:
    /// 1. `goal_group` (locality hint)
    /// 2. `goal_group ± 1, ± 2, ...` (spiral outward)
    /// 3. All groups from 0 (fallback)
    pub fn alloc_block(&self, goal_group: u32) -> Result<u64, i32> {
        let block_groups = self.fs.group_count;

        // Phase 1: Try goal_group first
        if goal_group < block_groups {
            if let Some(block) = self.try_alloc_from_group(goal_group)? {
                return Ok(block);
            }
        }

        // Phase 2: Spiral outward from goal_group
        for dist in 1..block_groups {
            let forward = goal_group as i32 + dist as i32;
            let backward = goal_group as i32 - dist as i32;

            if forward >= 0 && (forward as u32) < block_groups {
                if let Some(block) = self.try_alloc_from_group(forward as u32)? {
                    return Ok(block);
                }
            }
            if backward >= 0 && (backward as u32) < block_groups && backward != forward as i32 {
                if let Some(block) = self.try_alloc_from_group(backward as u32)? {
                    return Ok(block);
                }
            }
        }

        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// Try to allocate a single block from a specific group.
    ///
    /// Reads bitmap once, finds free bit, marks it used, writes back once.
    /// Returns `Some(block_number)` on success, `None` if group has no free blocks.
    fn try_alloc_from_group(&self, group_idx: u32) -> Result<Option<u64>, i32> {
        let blocks_per_group = self.fs.blocks_per_group as u64;
        let first_data_block = self.fs.sb_info.as_ref()
            .map(|sb| sb.s_first_data_block as u64)
            .unwrap_or(0);

        let (free_blocks, block_bitmap_block) = {
            let group_descs = self.fs.group_descs.lock();
            let group_desc = &group_descs[group_idx as usize];
            (group_desc.bg_free_blocks_count_lo, group_desc.bg_block_bitmap_lo as u64)
        };

        if free_blocks == 0 || block_bitmap_block == 0 {
            return Ok(None);
        }

        // Read bitmap ONCE
        let mut bitmap = self.read_block_bitmap(block_bitmap_block)?;

        // For group 0, skip block 0 (superblock)
        let start = if group_idx == 0 {
            core::cmp::max(first_data_block, 1)
        } else {
            0
        };

        // Find free bit using buddy-aligned scan (order=0 = single block)
        if let Some(block_offset) = find_free_bit(&bitmap, start, blocks_per_group) {
            let block_number = (group_idx as u64) * blocks_per_group + block_offset;

            // Mark block as used IN PLACE (no second read)
            let byte_idx = block_offset as usize / 8;
            let bit_idx = block_offset as usize % 8;
            bitmap[byte_idx] |= 1 << bit_idx;

            // Write back modified bitmap
            self.write_block_bitmap(block_bitmap_block, &bitmap)?;

            // Update in-memory group descriptor
            {
                let mut group_descs = self.fs.group_descs.lock();
                group_descs[group_idx as usize].bg_free_blocks_count_lo =
                    group_descs[group_idx as usize].bg_free_blocks_count_lo.saturating_sub(1);
            }

            // Update on-disk group descriptor and superblock
            self.update_group_desc_free_blocks(group_idx as u64, free_blocks - 1)?;
            self.update_superblock_free_blocks(-1)?;

            return Ok(Some(block_number));
        }

        Ok(None)
    }

    /// Free a block
    pub fn free_block(&self, block: u64) -> Result<(), i32> {
        let blocks_per_group = self.fs.blocks_per_group as u64;
        let block_groups = self.fs.group_count as u64;

        let group_idx = block / blocks_per_group;
        if group_idx >= block_groups {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let block_offset = (block % blocks_per_group) as usize;

        let (free_blocks, block_bitmap_block) = {
            let group_descs = self.fs.group_descs.lock();
            let group_desc = &group_descs[group_idx as usize];
            (group_desc.bg_free_blocks_count_lo, group_desc.bg_block_bitmap_lo as u64)
        };

        let mut bitmap = self.read_block_bitmap(block_bitmap_block)?;

        let byte_idx = block_offset / 8;
        let bit_idx = block_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] &= !(1 << bit_idx);

            self.write_block_bitmap(block_bitmap_block, &bitmap)?;

            {
                let mut group_descs = self.fs.group_descs.lock();
                group_descs[group_idx as usize].bg_free_blocks_count_lo =
                    group_descs[group_idx as usize].bg_free_blocks_count_lo.saturating_add(1);
            }

            self.update_group_desc_free_blocks(group_idx, free_blocks + 1)?;
            self.update_superblock_free_blocks(1)?;

            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    fn read_block_bitmap(&self, bitmap_block: u64) -> Result<Vec<u8>, i32> {
        // SAFETY: self.fs.device is a valid GenDisk pointer; bitmap_block comes from group descriptors.
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let bitmap = (*bh).b_data.to_vec();
            bio::brelse(bh);
            Ok(bitmap)
        }
    }

    fn write_block_bitmap(&self, bitmap_block: u64, bitmap: &[u8]) -> Result<(), i32> {
        // SAFETY: bh is from bio::bread; b_data is block_size bytes; bitmap fits within.
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            (*bh).b_data.copy_from_slice(bitmap);
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;
            bio::brelse(bh);
            Ok(())
        }
    }

    fn update_group_desc_free_blocks(&self, group_idx: u64, free_blocks: u16) -> Result<(), i32> {
        let group_desc_size = self.fs.desc_size as usize;
        let group_desc_start_block = if self.fs.block_size == 1024 { 2 } else { 1 };

        let desc_per_block = self.fs.block_size as u64 / group_desc_size as u64;
        let desc_block = group_desc_start_block + (group_idx / desc_per_block);
        let desc_offset = ((group_idx % desc_per_block) as usize) * group_desc_size;

        // SAFETY: BG_FREE_BLOCKS_OFF (u32*3 = 12 bytes) is within the block;
        // volatile write ensures the store is not optimized away.
        unsafe {
            let bh = bio::bread(self.fs.device, desc_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let free_blocks_ptr = (*bh).b_data.as_mut_ptr().add(desc_offset + BG_FREE_BLOCKS_OFF) as *mut u16;
            free_blocks_ptr.write_volatile(free_blocks);
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;
            bio::brelse(bh);
            Ok(())
        }
    }

    // SAFETY: SB_FREE_BLOCKS_OFF (u32*3 = 12 bytes) is within the superblock;
    // volatile read/write ensures ordering.
    fn update_superblock_free_blocks(&self, delta: i32) -> Result<(), i32> {
        unsafe {
            let sb_block = if self.fs.block_size == 1024 { 1 } else { 0 };
            let bh = bio::bread(self.fs.device, sb_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let sb_start = if self.fs.block_size == 1024 { 0 } else { 1024 };
            let free_blocks_ptr = (*bh).b_data.as_mut_ptr().add(sb_start + SB_FREE_BLOCKS_OFF) as *mut u32;
            let current = free_blocks_ptr.read_volatile();
            let new_count = (current as i64 + delta as i64) as u32;
            free_blocks_ptr.write_volatile(new_count);
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;
            bio::brelse(bh);
            Ok(())
        }
    }
}

/// Find a free bit in a block bitmap (single block, order=0).
///
/// Scans byte-by-byte, skipping fully-occupied bytes (0xFF).
/// This is the buddy allocator's order-0 search — fast path for
/// the common single-block allocation case.
/// NOTE: Visibility is `pub(crate)` for unit testing (see tests/ext4_allocator.rs).
/// Used internally by `alloc_block_in_group` and `alloc_inode_in_group`.
pub(crate) fn find_free_bit(bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
    let start_bit = start as usize;

    for (i, &byte) in bitmap.iter().enumerate() {
        let bit_offset = i * 8;

        if bit_offset + 8 <= start_bit {
            continue;
        }

        // Skip fully-occupied bytes (fast path)
        if byte == 0xFF {
            continue;
        }

        for bit in 0..8 {
            let abs_bit = bit_offset + bit;
            if abs_bit as u64 >= max_bits {
                return None;
            }
            if abs_bit < start_bit {
                continue;
            }
            if (byte & (1 << bit)) == 0 {
                return Some(abs_bit as u64);
            }
        }
    }

    None
}

// ============================================================================
// Preallocation-Aware Allocation
// ============================================================================

/// Allocate a block for a specific inode, using preallocation cache.
///
/// If a preallocation window exists for `ino` with remaining blocks, consumes
/// one block from it (no bitmap I/O needed). Otherwise, delegates to
/// `alloc_block(goal_group)` and creates a new preallocation window.
pub fn alloc_block_with_prealloc(
    fs: &super::Ext4FileSystem,
    goal_group: u32,
    ino: u32,
) -> Result<u64, i32> {
    // Phase 1: Check prealloc cache
    {
        let mut prealloc = fs.prealloc.lock();
        if let Some(ref mut pa) = *prealloc {
            if pa.ino == ino && pa.len < pa.total {
                let block = pa.start + pa.len as u64;
                pa.len += 1;
                return Ok(block);
            }
            // Inode mismatch or exhausted — discard old window
            *prealloc = None;
        }
    }

    // Phase 2: Cache miss — allocate one block
    let allocator = BlockAllocator::new(fs);
    let block = allocator.alloc_block(goal_group)?;

    // Phase 3: Preallocate extra blocks in the same group
    let blocks_per_group = fs.blocks_per_group as u64;
    let group_idx = block / blocks_per_group;
    let group_offset = block % blocks_per_group;

    // Try to allocate extra blocks starting right after the just-allocated one
    let mut prealloc_total = 1u32; // the block we already allocated
    {
        let (free_blocks, block_bitmap_block) = {
            let group_descs = fs.group_descs.lock();
            let group_desc = &group_descs[group_idx as usize];
            (group_desc.bg_free_blocks_count_lo, group_desc.bg_block_bitmap_lo as u64)
        };

        // Don't preallocate if the group is nearly full
        if free_blocks as u32 > PREALLOC_SIZE {
            if let Ok(mut bitmap) = allocator.read_block_bitmap(block_bitmap_block) {
                let first_data_block = fs.sb_info.as_ref()
                    .map(|sb| sb.s_first_data_block as u64)
                    .unwrap_or(0);
                let start = if group_idx == 0 {
                    core::cmp::max(first_data_block, 1)
                } else {
                    0
                };

                // Scan for contiguous free blocks after the allocated one
                let mut scan = group_offset as usize + 1;
                let max_scan = PREALLOC_SIZE as usize;

                for _ in 0..max_scan {
                    if scan as u64 >= blocks_per_group {
                        break;
                    }

                    let byte_idx = scan / 8;
                    let bit_idx = scan % 8;

                    if byte_idx >= bitmap.len() {
                        break;
                    }

                    if (bitmap[byte_idx] & (1 << bit_idx)) != 0 {
                        break; // not free — stop preallocation window
                    }

                    // Mark as used in bitmap
                    bitmap[byte_idx] |= 1 << bit_idx;
                    prealloc_total += 1;
                    scan += 1;
                }

                // Write back bitmap if we preallocated anything.
                // If bitmap write fails, do NOT update metadata (blocks aren't
                // actually reserved on disk), and fall back to single-block alloc.
                if prealloc_total > 1 {
                    if allocator.write_block_bitmap(block_bitmap_block, &bitmap).is_err() {
                        prealloc_total = 1; // rollback: only the original block counts
                    } else {
                        // Update in-memory and on-disk group descriptor
                        let extra = (prealloc_total - 1) as u16;
                        {
                            let mut group_descs = fs.group_descs.lock();
                            group_descs[group_idx as usize].bg_free_blocks_count_lo =
                                group_descs[group_idx as usize].bg_free_blocks_count_lo.saturating_sub(extra);
                        }
                        let new_free = free_blocks.saturating_sub(extra);
                        let _ = allocator.update_group_desc_free_blocks(group_idx, new_free);
                        let _ = allocator.update_superblock_free_blocks(-((prealloc_total - 1) as i32));
                    }
                }
            }
        }
    }

    // Store preallocation state
    if prealloc_total > 1 {
        let mut prealloc = fs.prealloc.lock();
        *prealloc = Some(PreallocState {
            start: block + 1, // next block to hand out
            len: 0,
            total: prealloc_total - 1,
            ino,
        });
    }

    Ok(block)
}

/// Discard any preallocation for the given inode.
pub fn discard_prealloc(fs: &super::Ext4FileSystem, ino: u32) {
    let mut prealloc = fs.prealloc.lock();
    if let Some(ref pa) = *prealloc {
        if pa.ino == ino {
            *prealloc = None;
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
                (group_desc.bg_free_inodes_count_lo, group_desc.bg_inode_bitmap_lo as u64)
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
            if let Some(inode_offset) = find_free_bit(&bitmap, 1, inodes_per_group) {
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
            (group_desc.bg_free_inodes_count_lo, group_desc.bg_inode_bitmap_lo as u64)
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
        // SAFETY: self.fs.device is valid; bitmap_block from group descriptors.
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
        // SAFETY: bh from bio::bread; b_data is block_size bytes; bitmap fits within.
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

        // SAFETY: desc_offset + 14 is within the block (bg_free_inodes_count_lo field);
        // volatile write ensures the store is not optimized away.
        unsafe {
            let bh = bio::bread(self.fs.device, desc_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            // Update free inode count (bg_free_inodes_count_lo offset in Ext4GroupDesc)
            let free_inodes_ptr = data.as_mut_ptr().add(desc_offset + 14) as *mut u16;
            free_inodes_ptr.write_volatile(free_inodes);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// Update free inode count in superblock
    // SAFETY: sb_start + 16 is within the superblock (s_free_inodes_count field);
    // volatile read/write ensures ordering.
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
