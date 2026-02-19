//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 块和 inode 分配器
//!
//! 完全...
//! 参考: fs/ext4/mballoc.c, fs/ext4/ialloc.c

use alloc::vec::Vec;

use crate::errno;
use crate::fs::bio;
use crate::fs::ext4::superblock::Ext4GroupDesc;

pub struct BlockAllocator<'a> {
    fs: &'a super::Ext4FileSystem,
}

impl<'a> BlockAllocator<'a> {
    /// 创建新的块分配器
    pub fn new(fs: &'a super::Ext4FileSystem) -> Self {
        Self { fs }
    }

    /// 分配一个块
    ///
    ///
    /// # 返回
    /// 成功返回块号，失败返回错误码
    pub fn alloc_block(&self) -> Result<u64, i32> {
        // 1. 查找有空闲块的块组
        let block_groups = self.fs.group_count;
        let blocks_per_group = self.fs.blocks_per_group as u64;
        let first_data_block = self.fs.sb_info.as_ref()
            .map(|sb| sb.s_first_data_block as u64)
            .unwrap_or(0);

        // 遍历所有块组寻找空闲块
        for group_idx in 0..block_groups {
            let group_desc = &self.fs.group_descs[group_idx as usize];

            // 检查是否有空闲块
            let free_blocks = group_desc.bg_free_blocks_count;
            if free_blocks == 0 {
                continue;
            }

            // 块位图的块号
            let block_bitmap_block = group_desc.bg_block_bitmap as u64;
            if block_bitmap_block == 0 {
                continue;
            }

            // 读取块位图
            let bitmap = self.read_block_bitmap(block_bitmap_block)?;

            // 在位图中查找空闲块
            let start = if group_idx == 0 {
                first_data_block
            } else {
                0
            };

            if let Some(block_offset) = self.find_free_bit(&bitmap, start, blocks_per_group) {
                // 计算实际块号
                let block_number = (group_idx as u64) * blocks_per_group + block_offset;

                // 标记块为已使用
                self.mark_block_used(group_idx as u64, block_offset as usize, block_bitmap_block)?;

                // 更新块组描述符（减少空闲块计数）
                self.update_group_desc_free_blocks(group_idx as u64, free_blocks - 1)?;

                // 更新 superblock（减少空闲块计数）
                self.update_superblock_free_blocks(-1)?;

                return Ok(block_number);
            }
        }

        // 没有可用的空闲块
        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// 释放一个块
    ///
    ///
    /// # 参数
    /// - `block`: 要释放的块号
    pub fn free_block(&self, block: u64) -> Result<(), i32> {
        let blocks_per_group = self.fs.blocks_per_group as u64;
        let block_groups = self.fs.group_count as u64;

        // 计算块所在的组
        let group_idx = block / blocks_per_group;
        if group_idx >= block_groups {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let block_offset = (block % blocks_per_group) as usize;

        // 读取块组描述符
        let group_desc = &self.fs.group_descs[group_idx as usize];
        let block_bitmap_block = group_desc.bg_block_bitmap as u64;

        // 读取块位图
        let mut bitmap = self.read_block_bitmap(block_bitmap_block)?;

        // 清除位图中的对应位
        let byte_idx = block_offset / 8;
        let bit_idx = block_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] &= !(1 << bit_idx);

            // 写回位图
            self.write_block_bitmap(block_bitmap_block, &bitmap)?;

            // 更新块组描述符（增加空闲块计数）
            self.update_group_desc_free_blocks(group_idx, group_desc.bg_free_blocks_count + 1)?;

            // 更新 superblock（增加空闲块计数）
            self.update_superblock_free_blocks(1)?;

            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    /// 读取块位图
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

    /// 写回块位图
    fn write_block_bitmap(&self, bitmap_block: u64, bitmap: &[u8]) -> Result<(), i32> {
        unsafe {
            let bh = bio::bread(self.fs.device, bitmap_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            data.copy_from_slice(bitmap);

            // 标记为脏并同步
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// 在位图中查找空闲位
    fn find_free_bit(&self, bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
        let start_bit = start as usize;

        for (i, &byte) in bitmap.iter().enumerate() {
            let bit_offset = i * 8;

            // 跳过起始位置之前的位
            if bit_offset + 8 <= start_bit {
                continue;
            }

            // 检查字节中是否有未设置的位
            if byte != 0xFF {
                for bit in 0..8 {
                    let abs_bit = bit_offset + bit;

                    // 超出最大位数
                    if abs_bit as u64 >= max_bits {
                        break;
                    }

                    // 跳过起始位置之前的位
                    if abs_bit < start_bit {
                        continue;
                    }

                    // 检查该位是否为0（空闲）
                    if (byte & (1 << bit)) == 0 {
                        return Some(abs_bit as u64);
                    }
                }
            }
        }

        None
    }

    /// 标记块为已使用
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

    /// 更新块组描述符中的空闲块计数
    fn update_group_desc_free_blocks(&self, group_idx: u64, free_blocks: u16) -> Result<(), i32> {
        // 在 ext4 中，块组描述符在磁盘上的位置是固定的
        // 我们需要找到块组描述符所在的块并更新它

        let group_desc_size = core::mem::size_of::<Ext4GroupDesc>();
        let group_desc_start_block = if self.fs.block_size == 1024 {
            2  // 块组描述符从块2开始（块0=引导，块1=superblock）
        } else {
            1  // 块组描述符从块1开始（块0包含superblock）
        };

        let desc_per_block = self.fs.block_size as u64 / group_desc_size as u64;
        let desc_block = group_desc_start_block + (group_idx / desc_per_block);
        let desc_offset = ((group_idx % desc_per_block) as usize) * group_desc_size;

        unsafe {
            let bh = bio::bread(self.fs.device, desc_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            // 更新空闲块计数（偏移量 = bg_free_blocks_count 在 Ext4GroupDesc 中的位置）
            let free_blocks_ptr = data.as_mut_ptr().add(desc_offset + 12) as *mut u16;
            free_blocks_ptr.write_volatile(free_blocks);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// 更新 superblock 中的空闲块计数
    fn update_superblock_free_blocks(&self, delta: i16) -> Result<(), i32> {
        unsafe {
            // superblock 总是在块 1 (对于 1024 字节块) 或块 0 (对于更大的块)
            let sb_block = if self.fs.block_size == 1024 { 1 } else { 0 };

            let bh = bio::bread(self.fs.device, sb_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;

            // 更新空闲块计数（s_free_blocks_count 在 Ext4SuperBlockOnDisk 中的偏移）
            // 偏移量需要从结构体定义中计算
            let free_blocks_ptr = data.as_mut_ptr().add(16) as *mut u16;  // s_free_blocks_count 在偏移16

            let current = free_blocks_ptr.read_volatile();
            let new = (current as i16 + delta) as u16;
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
    /// 创建新的 inode 分配器
    pub fn new(fs: &'a super::Ext4FileSystem) -> Self {
        Self { fs }
    }

    /// 分配一个 inode
    ///
    ///
    /// # 返回
    /// 成功返回 inode 号，失败返回错误码
    pub fn alloc_inode(&self) -> Result<u32, i32> {
        let block_groups = self.fs.group_count;
        let inodes_per_group = self.fs.inodes_per_group as u64;

        // 遍历所有块组寻找空闲 inode
        for group_idx in 0..block_groups {
            let group_desc = &self.fs.group_descs[group_idx as usize];

            // 检查是否有空闲 inode
            let free_inodes = group_desc.bg_free_inodes_count;
            if free_inodes == 0 {
                continue;
            }

            // inode 位图的块号
            let inode_bitmap_block = group_desc.bg_inode_bitmap as u64;
            if inode_bitmap_block == 0 {
                continue;
            }

            // 读取 inode 位图
            let bitmap = self.read_inode_bitmap(inode_bitmap_block)?;

            // 在位图中查找空闲 inode
            // ext4 中 inode 从 1 开始计数（0 保留）
            if let Some(inode_offset) = self.find_free_bit(&bitmap, 1, inodes_per_group) {
                // 计算实际 inode 号
                let inode_number = (group_idx as u64) * inodes_per_group + inode_offset;

                // 标记 inode 为已使用
                self.mark_inode_used(group_idx as u64, inode_offset as usize, inode_bitmap_block)?;

                // 更新块组描述符（减少空闲 inode 计数）
                self.update_group_desc_free_inodes(group_idx as u64, free_inodes - 1)?;

                // 更新 superblock（减少空闲 inode 计数）
                self.update_superblock_free_inodes(-1)?;

                return Ok(inode_number as u32);
            }
        }

        // 没有可用的空闲 inode
        Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
    }

    /// 释放一个 inode
    ///
    ///
    /// # 参数
    /// - `ino`: 要释放的 inode 号
    pub fn free_inode(&self, ino: u32) -> Result<(), i32> {
        let inodes_per_group = self.fs.inodes_per_group as u64;
        let block_groups = self.fs.group_count as u64;

        // 计算 inode 所在的组
        let group_idx = (ino as u64 - 1) / inodes_per_group;
        if group_idx >= block_groups {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let inode_offset = ((ino as u64 - 1) % inodes_per_group) as usize;

        // 读取块组描述符
        let group_desc = &self.fs.group_descs[group_idx as usize];
        let inode_bitmap_block = group_desc.bg_inode_bitmap as u64;

        // 读取 inode 位图
        let mut bitmap = self.read_inode_bitmap(inode_bitmap_block)?;

        // 清除位图中的对应位
        let byte_idx = inode_offset / 8;
        let bit_idx = inode_offset % 8;

        if byte_idx < bitmap.len() {
            bitmap[byte_idx] &= !(1 << bit_idx);

            // 写回位图
            self.write_inode_bitmap(inode_bitmap_block, &bitmap)?;

            // 更新块组描述符（增加空闲 inode 计数）
            self.update_group_desc_free_inodes(group_idx, group_desc.bg_free_inodes_count + 1)?;

            // 更新 superblock（增加空闲 inode 计数）
            self.update_superblock_free_inodes(1)?;

            Ok(())
        } else {
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }

    /// 读取 inode 位图
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

    /// 写回 inode 位图
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

    /// 在位图中查找空闲位
    fn find_free_bit(&self, bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
        let start_bit = start as usize;

        for (i, &byte) in bitmap.iter().enumerate() {
            let bit_offset = i * 8;

            // 跳过起始位置之前的位
            if bit_offset + 8 <= start_bit {
                continue;
            }

            // 检查字节中是否有未设置的位
            if byte != 0xFF {
                for bit in 0..8 {
                    let abs_bit = bit_offset + bit;

                    // 超出最大位数
                    if abs_bit as u64 >= max_bits {
                        break;
                    }

                    // 跳过起始位置之前的位
                    if abs_bit < start_bit {
                        continue;
                    }

                    // 检查该位是否为0（空闲）
                    if (byte & (1 << bit)) == 0 {
                        return Some(abs_bit as u64);
                    }
                }
            }
        }

        None
    }

    /// 标记 inode 为已使用
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

    /// 更新块组描述符中的空闲 inode 计数
    fn update_group_desc_free_inodes(&self, group_idx: u64, free_inodes: u16) -> Result<(), i32> {
        let group_desc_size = core::mem::size_of::<Ext4GroupDesc>();
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
            // 更新空闲 inode 计数（bg_free_inodes_count 在 Ext4GroupDesc 中的偏移）
            let free_inodes_ptr = data.as_mut_ptr().add(desc_offset + 14) as *mut u16;
            free_inodes_ptr.write_volatile(free_inodes);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }

    /// 更新 superblock 中的空闲 inode 计数
    fn update_superblock_free_inodes(&self, delta: i16) -> Result<(), i32> {
        unsafe {
            let sb_block = if self.fs.block_size == 1024 { 1 } else { 0 };

            let bh = bio::bread(self.fs.device, sb_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;

            // 更新空闲 inode 计数（s_free_inodes_count 在 Ext4SuperBlockOnDisk 中的偏移）
            let free_inodes_ptr = data.as_mut_ptr().add(20) as *mut u16;

            let current = free_inodes_ptr.read_volatile();
            let new = (current as i16 + delta) as u16;
            free_inodes_ptr.write_volatile(new);

            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;

            bio::brelse(bh);

            Ok(())
        }
    }
}
