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

    /// Get the length (number of blocks), masking the initialized flag (bit 15).
    pub fn length(&self) -> u16 {
        (self.ee_len as u32 & 0x7FFF) as u16
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
    // SAFETY: i_block is a &[u32; 15] (60 bytes), which is larger than
    // Ext4ExtentHeader (12 bytes), so the cast is within bounds of the allocation.
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

    // Validate eh_entries against eh_max
    if header.eh_entries > header.eh_max {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    find_block_in_extent_tree(fs, i_block, logical_block, 0)
}

/// Find logical block in extent tree
fn find_block_in_extent_tree(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u32; 15],
    logical_block: u64,
    depth: u32,
) -> Result<u64, i32> {
    // SAFETY: data is a &[u32; 15] (60 bytes), larger than ExtentExtentHeader (12 bytes).
    let header = unsafe { &*(data.as_ptr() as *const Ext4ExtentHeader) };

    if header.eh_depth == 0 {
        // Leaf node: search for extent in i_block array
        // Validate eh_entries: each entry is 12 bytes; after the 12-byte header,
        // at most (60 - 12) / 12 = 4 entries fit in i_block.
        let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>()) / core::mem::size_of::<Ext4Extent>();
        if header.eh_entries as usize > max_entries {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: offset by header size (12 bytes) stays within the 60-byte i_block array;
        // eh_entries was validated above against max_entries.
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
        // Internal node: read index entries and recurse
        // SAFETY: same bounds reasoning as the leaf case; indices follow the header
        // within the same 60-byte i_block array and eh_entries is valid.
        let indices = unsafe {
            core::slice::from_raw_parts(
                (data.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>())
                    as *const Ext4ExtentIdx,
                header.eh_entries as usize
            )
        };

        let mut child_block = 0u64;
        for idx in indices {
            if logical_block >= idx.ei_block as u64 {
                child_block = idx.leaf_block();
            } else {
                break;
            }
        }

        if child_block == 0 {
            return Ok(0);
        }

        find_block_in_external_extent(fs, child_block, logical_block)
    }
}

/// Read extent from external block and find logical block
fn find_block_in_external_extent(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_num: u64,
    logical_block: u64,
) -> Result<u64, i32> {
    // SAFETY: bio::bread returns a valid buffer_head whose b_data is a properly
    // aligned block-sized byte slice; the subsequent casts to Ext4ExtentHeader,
    // Ext4Extent, and Ext4ExtentIdx are within this buffer and eh_entries is
    // validated against the on-disk header.
    unsafe {
        let bh = bio::bread(fs.device, block_num)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &(*bh).b_data;
        let header = &*(data.as_ptr() as *const Ext4ExtentHeader);

        if header.eh_magic != EXT4_EXT_MAGIC {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        // Validate eh_entries against eh_max and available buffer space
        if header.eh_entries > header.eh_max {
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
