//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 extent tree 支持
//!
//! 参考: Linux kernel fs/ext4/extents.c, fs/ext4/ext4_extents.h

use crate::errno;
use crate::fs::bio;

/// Extent header magic number
pub const EXT4_EXT_MAGIC: u16 = 0xF30A;

/// Extent header (in i_block or external block)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4ExtentHeader {
    /// Magic number (0xF30A)
    pub eh_magic: u16,
    /// Number of valid entries
    pub eh_entries: u16,
    /// Maximum number of entries that could follow
    pub eh_max: u16,
    /// Depth of extent tree (0 = leaf)
    pub eh_depth: u16,
    /// Generation number
    pub eh_generation: u32,
}

/// Extent entry (leaf node)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4Extent {
    /// First logical block covered by this extent
    pub ee_block: u32,
    /// Number of blocks covered by this extent
    pub ee_len: u16,
    /// High 16 bits of physical block
    pub ee_start_hi: u16,
    /// Low 32 bits of physical block
    pub ee_start_lo: u32,
}

impl Ext4Extent {
    /// Get the starting physical block number
    pub fn start_block(&self) -> u64 {
        ((self.ee_start_hi as u64) << 32) | (self.ee_start_lo as u64)
    }

    /// Get the length (number of blocks)
    pub fn length(&self) -> u32 {
        self.ee_len as u32
    }
}

/// Index entry (internal node)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4ExtentIdx {
    /// Logical block covered by children
    pub ei_block: u32,
    /// Low 32 bits of child block
    pub ei_leaf_lo: u32,
    /// High 16 bits of child block
    pub ei_leaf_hi: u16,
    /// Reserved
    pub ei_unused: u16,
}

impl Ext4ExtentIdx {
    /// Get the child block number
    pub fn leaf_block(&self) -> u64 {
        ((self.ei_leaf_hi as u64) << 32) | (self.ei_leaf_lo as u64)
    }
}

/// 从 i_block 数组解析 extent header
pub fn get_extent_header(i_block: &[u32; 15]) -> &Ext4ExtentHeader {
    unsafe {
        &*(i_block.as_ptr() as *const Ext4ExtentHeader)
    }
}

/// 查找逻辑块对应的物理块（使用 extent）
///
/// # 参数
/// - `fs`: ext4 文件系统
/// - `i_block`: inode 的 i_block 数组
/// - `logical_block`: 要查找的逻辑块号
///
/// # 返回
/// 物理块号，如果未找到返回 0
pub fn ext4_ext_get_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<u64, i32> {
    let header = get_extent_header(i_block);

    // 验证 magic
    if header.eh_magic != EXT4_EXT_MAGIC {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // 递归查找 extent
    find_block_in_extent_tree(fs, i_block, logical_block, 0)
}

/// 在 extent 树中查找逻辑块
fn find_block_in_extent_tree(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u32; 15],
    logical_block: u64,
    depth: u32,
) -> Result<u64, i32> {
    let header = unsafe { &*(data.as_ptr() as *const Ext4ExtentHeader) };

    if header.eh_depth == 0 {
        // 叶子节点：在 i_block 数组中查找 extent
        let entries = unsafe {
            core::slice::from_raw_parts(
                (data.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
                header.eh_entries as usize
            )
        };

        for ext in entries {
            let start = ext.ee_block as u64;
            let end = start + ext.length() as u64;

            if logical_block >= start && logical_block < end {
                // 找到了！计算偏移
                let offset = logical_block - start;
                return Ok(ext.start_block() + offset);
            }
        }

        // 未找到
        Ok(0)
    } else {
        // 内部节点：需要读取子节点块
        // 对于简单的 rootfs，通常 depth = 0，这里暂不实现 depth > 0 的情况
        Err(errno::Errno::IOError.as_neg_i32())
    }
}

/// 从外部块读取 extent 并查找逻辑块
#[allow(dead_code)]
fn find_block_in_external_extent(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_num: u64,
    logical_block: u64,
) -> Result<u64, i32> {
    unsafe {
        let bh = bio::bread(fs.device, block_num)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &(*bh).b_data;
        let header = &*(data.as_ptr() as *const Ext4ExtentHeader);

        if header.eh_magic != EXT4_EXT_MAGIC {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        if header.eh_depth == 0 {
            // 叶子节点
            let entries = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
                header.eh_entries as usize
            );

            for ext in entries {
                let start = ext.ee_block as u64;
                let end = start + ext.length() as u64;

                if logical_block >= start && logical_block < end {
                    let offset = logical_block - start;
                    bio::brelse(bh);
                    return Ok(ext.start_block() + offset);
                }
            }

            bio::brelse(bh);
            Ok(0)
        } else {
            // 内部节点：递归查找
            let indices = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx,
                header.eh_entries as usize
            );

            // 二分查找合适的索引
            let mut child_block = 0;
            for idx in indices {
                if logical_block >= idx.ei_block as u64 {
                    child_block = idx.leaf_block();
                } else {
                    break;
                }
            }

            bio::brelse(bh);

            if child_block == 0 {
                return Ok(0);
            }

            // 递归查找
            find_block_in_external_extent(fs, child_block, logical_block)
        }
    }
}
