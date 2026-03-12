//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 extent tree support

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

/// Parse extent header from i_block array
pub fn get_extent_header(i_block: &[u32; 15]) -> &Ext4ExtentHeader {
    unsafe {
        &*(i_block.as_ptr() as *const Ext4ExtentHeader)
    }
}

/// Find physical block corresponding to logical block (using extent)
///
/// # Parameters
/// - `fs`: ext4 filesystem
/// - `i_block`: inode's i_block array
/// - `logical_block`: logical block number to find
///
/// # Returns
/// Physical block number, returns 0 if not found
pub fn ext4_ext_get_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<u64, i32> {
    let header = get_extent_header(i_block);

    // Verify magic
    if header.eh_magic != EXT4_EXT_MAGIC {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Recursively search extent
    find_block_in_extent_tree(fs, i_block, logical_block, 0)
}

/// Find logical block in extent tree
fn find_block_in_extent_tree(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u32; 15],
    logical_block: u64,
    depth: u32,
) -> Result<u64, i32> {
    let header = unsafe { &*(data.as_ptr() as *const Ext4ExtentHeader) };

    if header.eh_depth == 0 {
        // Leaf node: search for extent in i_block array
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
                // Found! Calculate offset
                let offset = logical_block - start;
                return Ok(ext.start_block() + offset);
            }
        }

        // Not found
        Ok(0)
    } else {
        // Internal node: need to read child node block
        // For simple rootfs, usually depth = 0, not implementing depth > 0 case yet
        Err(errno::Errno::IOError.as_neg_i32())
    }
}

/// Read extent from external block and find logical block
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
            // Leaf node
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
            // Internal node: recursive search
            let indices = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx,
                header.eh_entries as usize
            );

            // Binary search for appropriate index
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

            // Recursive search
            find_block_in_external_extent(fs, child_block, logical_block)
        }
    }
}
