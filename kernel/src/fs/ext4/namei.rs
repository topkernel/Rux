//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 inode operations (mkdir, create, unlink, rmdir)
//!
//! Based on Linux kernel fs/ext4/namei.c

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use core::mem::size_of;

use crate::errno;
use crate::fs::bio::{self, BufferHead, BufferState};
use crate::fs::ext4::superblock::Ext4GroupDesc;
use crate::fs::ext4::inode::{Ext4Inode, Ext4InodeOnDisk};
use crate::fs::ext4::dir::file_type;
use crate::fs::ext4::allocator::BlockAllocator;

use super::Ext4FileSystem;

// ============================================================================
// Current transaction handle (single-core, no concurrency)
// ============================================================================

use core::sync::atomic::{AtomicUsize, Ordering};

/// Global slot for the current journal handle.
/// When a journal transaction is active, this stores a pointer to the Handle.
/// Single-core: no synchronization needed beyond the atomic itself.
static CURRENT_JOURNAL_HANDLE: AtomicUsize = AtomicUsize::new(0);

/// Set the current journal handle for this thread of execution
pub(crate) unsafe fn set_current_handle(handle: *mut crate::fs::jbd2::Handle) {
    CURRENT_JOURNAL_HANDLE.store(handle as usize, Ordering::SeqCst);
}

/// Clear the current journal handle
pub(crate) unsafe fn clear_current_handle() {
    CURRENT_JOURNAL_HANDLE.store(0, Ordering::SeqCst);
}

/// Get the current journal handle, if any
pub(crate) unsafe fn get_current_handle() -> Option<*mut crate::fs::jbd2::Handle> {
    let ptr = CURRENT_JOURNAL_HANDLE.load(Ordering::SeqCst) as *mut crate::fs::jbd2::Handle;
    if ptr.is_null() { None } else { Some(ptr) }
}

// ============================================================================
// Constants
// ============================================================================

/// Maximum link count for directories
pub const EXT4_LINK_MAX: u16 = 65000;

/// Inode mode bits
pub const S_IFMT: u16 = 0o170000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFREG: u16 = 0o100000;
pub const S_IFLNK: u16 = 0o120000;

/// Permission bits
pub const S_IRWXU: u16 = 0o0700;
pub const S_IRWXG: u16 = 0o0070;
pub const S_IRWXO: u16 = 0o0007;

// ============================================================================
// Helper functions for block I/O
// ============================================================================

/// Read a block into a Vec<u8>
unsafe fn read_block_to_vec(device: *const crate::drivers::blkdev::GenDisk, blocknr: u64, block_size: usize) -> Result<Vec<u8>, i32> {
    let bh = bio::bread(device, blocknr).ok_or(errno::Errno::IOError.as_neg_i32())?;
    let data = (*bh).b_data.clone();
    bio::brelse(bh);
    Ok(data)
}

/// Write a Vec<u8> to a block
unsafe fn write_block_from_vec(device: *const crate::drivers::blkdev::GenDisk, blocknr: u64, data: &[u8]) -> Result<(), i32> {
    let bh = bio::bread(device, blocknr).ok_or(errno::Errno::IOError.as_neg_i32())?;

    // Get mutable reference to buffer head
    let bh_ref = &mut *bh;

    // Copy data to buffer
    let buf_len = bh_ref.b_data.len().min(data.len());
    bh_ref.b_data[0..buf_len].copy_from_slice(&data[0..buf_len]);

    // Mark dirty and sync
    bh_ref.set_state_bit(BufferState::BH_Dirty);

    // If a journal handle is active, register this buffer
    if let Some(handle) = get_current_handle() {
        let _ = crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh);
    }

    bio::sync_dirty_buffer(bh)?;
    bio::brelse(bh);

    Ok(())
}

// ============================================================================
// Inode allocation
// ============================================================================

/// Find a suitable block group for new inode
///
/// Uses Orlov's allocator for directories to spread them across groups.
pub fn find_group_orlov(fs: &Ext4FileSystem, _parent: u32, is_dir: bool) -> Result<u32, i32> {
    let group_count = fs.group_count;
    let _inodes_per_group = fs.inodes_per_group;

    // Simple implementation: find first group with free inodes
    for group in 0..group_count {
        let free_inodes = get_group_free_inodes(fs, group)?;
        if free_inodes > 0 {
            return Ok(group);
        }
    }

    Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
}

/// Get free inode count for a group
pub fn get_group_free_inodes(fs: &Ext4FileSystem, group: u32) -> Result<u32, i32> {
    let group_descs = fs.group_descs.lock();
    if group as usize >= group_descs.len() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    Ok(group_descs[group as usize].bg_free_inodes_count as u32)
}

/// Allocate a new inode
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir` - Parent directory inode number
/// * `mode` - Mode for new inode
/// * `name` - Name for new entry
///
/// # Returns
/// * Ok((inode_number, inode)) on success
/// * Err(i32) on failure
pub fn ext4_new_inode(
    fs: &Ext4FileSystem,
    dir: u32,
    mode: u16,
    _name: &[u8],
) -> Result<(u32, Ext4InodeOnDisk), i32> {
    // Find suitable group
    let is_dir = (mode & S_IFMT) == S_IFDIR;
    let group = find_group_orlov(fs, dir, is_dir)?;

    // Get group descriptor
    let inode_bitmap_block = {
        let group_descs = fs.group_descs.lock();
        group_descs[group as usize].bg_inode_bitmap
    };
    let bitmap_data = unsafe {
        read_block_to_vec(fs.device, inode_bitmap_block as u64, fs.block_size as usize)?
    };

    // Find free inode in bitmap
    let inodes_per_group = fs.inodes_per_group as usize;
    let mut free_ino_in_group: usize = 0;
    let mut found = false;

    for byte_idx in 0..bitmap_data.len() {
        let byte = bitmap_data[byte_idx];
        if byte != 0xff {
            // Find first zero bit
            for bit in 0..8 {
                if (byte & (1 << bit)) == 0 {
                    free_ino_in_group = byte_idx * 8 + bit;
                    if free_ino_in_group < inodes_per_group {
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
    }

    if !found || free_ino_in_group >= inodes_per_group {
        return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
    }

    // Calculate global inode number
    let ino = group * fs.inodes_per_group + free_ino_in_group as u32 + 1;

    // Mark inode as used in bitmap
    mark_inode_used(fs, group, free_ino_in_group, &bitmap_data, inode_bitmap_block as u64)?;

    // Create new inode
    let mut inode = Ext4InodeOnDisk::default();
    inode.i_mode = mode;
    inode.i_links_count = 1;
    inode.i_uid = 0;
    inode.i_gid = 0;
    inode.i_size = 0;
    inode.i_blocks = 0;
    // Don't set EXT4_EXTENTS_FL by default - the caller should set it if needed
    // and properly initialize the extent tree
    inode.i_flags = 0;
    inode.i_atime = 0; // TODO: get current time
    inode.i_mtime = 0;
    inode.i_ctime = 0;

    // Update group descriptor
    update_group_descriptor_inodes(fs, group, -1)?;

    // Update superblock
    update_superblock_free_inodes(fs, -1)?;

    Ok((ino, inode))
}

/// Mark inode as used in bitmap
fn mark_inode_used(
    fs: &Ext4FileSystem,
    _group: u32,
    ino_in_group: usize,
    bitmap_data: &[u8],
    bitmap_block: u64,
) -> Result<(), i32> {
    let byte_idx = ino_in_group / 8;
    let bit_idx = ino_in_group % 8;

    // Create new bitmap with bit set
    let mut new_bitmap = bitmap_data.to_vec();
    new_bitmap[byte_idx] |= 1 << bit_idx;

    // Write bitmap back
    unsafe {
        write_block_from_vec(fs.device, bitmap_block, &new_bitmap)?;
    }

    Ok(())
}

/// Update group descriptor free inode count
fn update_group_descriptor_inodes(fs: &Ext4FileSystem, group: u32, delta: i32) -> Result<(), i32> {
    {
        let mut group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        if delta < 0 {
            group_descs[group as usize].bg_free_inodes_count =
                group_descs[group as usize].bg_free_inodes_count.saturating_sub((-delta) as u16);
        } else {
            group_descs[group as usize].bg_free_inodes_count =
                group_descs[group as usize].bg_free_inodes_count.saturating_add(delta as u16);
        }
    }

    // Write group descriptor to disk
    write_group_descriptor(fs, group)?;

    Ok(())
}

/// Update superblock free inode count
fn update_superblock_free_inodes(fs: &Ext4FileSystem, delta: i32) -> Result<(), i32> {
    // Update in-memory sb_info
    let sb_info_ptr = fs.sb_info.as_ref().map(|x| x.as_ref() as *const super::superblock::Ext4SuperBlockInfo);
    if let Some(sb_info_ptr) = sb_info_ptr {
        unsafe {
            let sb_info = &mut *(sb_info_ptr as *mut super::superblock::Ext4SuperBlockInfo);
            if delta < 0 {
                sb_info.s_free_inodes_count =
                    sb_info.s_free_inodes_count.saturating_sub((-delta) as u32);
            } else {
                sb_info.s_free_inodes_count =
                    sb_info.s_free_inodes_count.saturating_add(delta as u32);
            }
        }
    }

    // Write to on-disk superblock
    // s_free_inodes_count is at offset 16 within the superblock
    unsafe {
        let sb_block = if fs.block_size == 1024 { 1u64 } else { 0u64 };
        let bh = bio::bread(fs.device, sb_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &mut (*bh).b_data;
        let sb_start = if fs.block_size == 1024 { 0usize } else { 1024usize };
        let ptr = data.as_mut_ptr().add(sb_start + 16) as *mut u32;

        let current = ptr.read_volatile();
        ptr.write_volatile((current as i32 + delta) as u32);

        (*bh).set_state_bit(BufferState::BH_Dirty);
        bio::sync_dirty_buffer(bh)?;
        bio::brelse(bh);
    }

    Ok(())
}

/// Write group descriptor to disk
fn write_group_descriptor(fs: &Ext4FileSystem, group: u32) -> Result<(), i32> {
    let gd = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }
        *group_descs[group as usize]
    };

    // Calculate descriptor table location
    let desc_per_block = fs.block_size / fs.desc_size as u32;
    let desc_block = fs.sb_info.as_ref()
        .map(|sb| sb.s_first_data_block + 1 + group / desc_per_block)
        .unwrap_or(1);
    let desc_offset = (group % desc_per_block) as usize;

    // Read descriptor block
    let mut block_data = unsafe {
        read_block_to_vec(fs.device, desc_block as u64, fs.block_size as usize)?
    };

    // Write descriptor (only the low 32 bytes that Ext4GroupDesc covers;
    // the high 32 bytes in the 64-bit on-disk descriptor are preserved from
    // the block_data we just read).
    let gd_ptr: *const Ext4GroupDesc = &gd;
    let gd_bytes = unsafe {
        core::slice::from_raw_parts(
            gd_ptr as *const u8,
            core::mem::size_of::<Ext4GroupDesc>()
        )
    };
    let offset = desc_offset * fs.desc_size as usize;
    block_data[offset..offset + gd_bytes.len()].copy_from_slice(gd_bytes);

    // Write back
    unsafe {
        write_block_from_vec(fs.device, desc_block as u64, &block_data)?;
    }

    Ok(())
}

// ============================================================================
// Directory entry operations
// ============================================================================

/// Get block number from directory inode, supporting both extents and direct blocks
fn get_dir_block_nr(fs: &Ext4FileSystem, dir: &Ext4InodeOnDisk, block_idx: u64) -> Result<u64, i32> {
    // Check if using extents
    if (dir.i_flags & 0x80000) != 0 {
        // Use extent tree
        super::extent::ext4_ext_get_block(fs, &dir.i_block, block_idx)
    } else {
        // Use direct/indirect blocks
        if block_idx < 12 {
            Ok(dir.i_block[block_idx as usize] as u64)
        } else {
            // TODO: Handle indirect blocks
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }
}

/// Add entry to directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Directory inode number
/// * `name` - Entry name
/// * `new_ino` - New entry's inode number
/// * `file_type` - File type (1=file, 2=dir, etc.)
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_add_entry(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    new_ino: u32,
    file_type: u8,
) -> Result<(), i32> {
    // Read directory inode
    let dir = super::inode::read_inode(fs, dir_ino)?;

    // Read directory data blocks
    let block_size = fs.block_size as usize;
    let dir_size = dir.i_size as usize;

    // Calculate number of blocks
    let num_blocks = if block_size > 0 {
        (dir_size + block_size - 1) / block_size
    } else {
        0
    };

    // Iterate through directory blocks looking for space
    for block_idx in 0..num_blocks as u64 {
        let block_nr = match get_dir_block_nr(fs, &dir, block_idx) {
            Ok(nr) => nr,
            Err(_) => {
                continue;
            }
        };

        if block_nr == 0 {
            continue;
        }

        let block_data = unsafe {
            let bh = bio::bread(fs.device, block_nr)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let data = (*bh).b_data.clone();
            bio::brelse(bh);
            data
        };

        // Try to find space in this block
        if let Some(offset) = find_entry_space(&block_data, name.len(), block_size) {
            // Found space, create entry
            let mut new_block = block_data.clone();
            add_entry_to_block(&mut new_block, offset, name, new_ino, file_type, block_size);

            // Write block back
            unsafe {
                write_block_from_vec(fs.device, block_nr, &new_block)?;
            }

            return Ok(());
        }
    }

    // No space in existing blocks, allocate new block
    // No space in existing blocks, allocate new block
    let allocator = BlockAllocator::new(fs);
    let new_block_nr = allocator.alloc_block()?;

    // Create new block with entry
    let mut new_block = alloc::vec![0u8; block_size];
    create_initial_entry(&mut new_block, name, new_ino, file_type, block_size);

    // Write new block
    unsafe {
        write_block_from_vec(fs.device, new_block_nr, &new_block)?;
    }

    // Update directory inode to reference new block
    add_block_to_inode(fs, dir_ino, &dir, new_block_nr)?;

    Ok(())
}

/// Find space for new entry in directory block
fn find_entry_space(block_data: &[u8], name_len: usize, block_size: usize) -> Option<usize> {
    let mut offset = 0;
    let required_len = ((8 + name_len + 3) & !3) as u16; // Align to 4 bytes

    while offset + 8 <= block_size {
        let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);

        if rec_len == 0 {
            break;
        }

        let name_len_entry = block_data[offset + 6] as usize;
        let used_len = ((8 + name_len_entry + 3) & !3) as u16;

        // Check if there's space in this entry
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

    // Safety: ensure rec_len > used_len before subtracting
    if rec_len <= used_len {
        return;
    }

    // Calculate new entry length
    let new_rec_len = rec_len - used_len;

    // Update existing entry's record length
    let used_bytes = used_len.to_le_bytes();
    block_data[offset + 4] = used_bytes[0];
    block_data[offset + 5] = used_bytes[1];

    // Create new entry after existing
    let new_offset = offset + used_len as usize;

    // Write inode number
    let ino_bytes = ino.to_le_bytes();
    block_data[new_offset..new_offset + 4].copy_from_slice(&ino_bytes);

    // Write record length
    let new_rec_bytes = new_rec_len.to_le_bytes();
    block_data[new_offset + 4] = new_rec_bytes[0];
    block_data[new_offset + 5] = new_rec_bytes[1];

    // Write name length
    block_data[new_offset + 6] = name.len() as u8;

    // Write file type
    block_data[new_offset + 7] = file_type;

    // Write name
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
    let _entry_len = ((8 + name.len() + 3) & !3) as u16;
    let rec_len = block_size as u16;

    // Write inode number
    let ino_bytes = ino.to_le_bytes();
    block_data[0..4].copy_from_slice(&ino_bytes);

    // Write record length (entire block)
    let rec_bytes = rec_len.to_le_bytes();
    block_data[4] = rec_bytes[0];
    block_data[5] = rec_bytes[1];

    // Write name length
    block_data[6] = name.len() as u8;

    // Write file type
    block_data[7] = file_type;

    // Write name
    block_data[8..8 + name.len()].copy_from_slice(name);
}

/// Add block to inode's block list
fn add_block_to_inode(
    fs: &Ext4FileSystem,
    ino: u32,
    inode: &Ext4InodeOnDisk,
    block_nr: u64,
) -> Result<(), i32> {
    let mut new_inode = *inode;
    let block_size = fs.block_size;

    // Check if using extents
    if (new_inode.i_flags & 0x80000) != 0 {
        return add_block_to_inode_extent(fs, ino, &mut new_inode, block_nr, block_size);
    }

    // Direct/indirect block mode: find free slot in i_block array
    for i in 0..12 {
        if new_inode.i_block[i] == 0 {
            new_inode.i_block[i] = block_nr as u32;
            new_inode.i_size += block_size;
            new_inode.i_blocks += (block_size / 512) as u32;

            super::inode::write_inode_disk(fs, ino, &new_inode)?;
            return Ok(());
        }
    }

    // Need to use indirect blocks - for now, return error
    Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
}

/// Add block to an extent-based inode by extending the inline extent tree.
fn add_block_to_inode_extent(
    fs: &Ext4FileSystem,
    ino: u32,
    inode: &mut Ext4InodeOnDisk,
    block_nr: u64,
    block_size: u32,
) -> Result<(), i32> {
    use super::extent::{Ext4ExtentHeader, Ext4Extent, EXT4_EXT_MAGIC};

    // Calculate which logical block this new block will be
    let current_blocks = inode.i_size / block_size;
    let logical_block = current_blocks;

    let header = unsafe {
        &mut *(inode.i_block.as_mut_ptr() as *mut Ext4ExtentHeader)
    };

    if header.eh_magic != EXT4_EXT_MAGIC {
        // Initialize extent header (shouldn't happen for properly created extent inodes)
        header.eh_magic = EXT4_EXT_MAGIC;
        header.eh_entries = 0;
        header.eh_max = 4;
        header.eh_depth = 0;
        header.eh_generation = 0;
    }

    // Get mutable extent entries (max 4 inline)
    let entries = unsafe {
        core::slice::from_raw_parts_mut(
            (inode.i_block.as_mut_ptr() as *mut u8)
                .add(core::mem::size_of::<Ext4ExtentHeader>()) as *mut Ext4Extent,
            header.eh_max as usize,
        )
    };

    // Try to extend last extent if blocks are physically contiguous
    if header.eh_entries > 0 {
        let last = &mut entries[(header.eh_entries - 1) as usize];
        let last_end = last.ee_block as u64 + last.length() as u64;

        if last_end == logical_block as u64 && last.length() < 0x8000 {
            let expected_physical = last.start_block() + last.length() as u64;
            if block_nr == expected_physical {
                // Contiguous — just extend length
                last.ee_len += 1;
                inode.i_size += block_size;
                inode.i_blocks += (block_size / 512) as u32;
                super::inode::write_inode_disk(fs, ino, inode)?;
                return Ok(());
            }
        }
    }

    // Need a new extent entry
    if header.eh_entries >= header.eh_max {
        // No inline space — would need an external extent node (not implemented)
        return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
    }

    let new_entry = &mut entries[header.eh_entries as usize];
    new_entry.ee_block = logical_block as u32;
    new_entry.ee_len = 1;
    new_entry.ee_start_hi = (block_nr >> 32) as u16;
    new_entry.ee_start_lo = block_nr as u32;
    header.eh_entries += 1;

    inode.i_size += block_size;
    inode.i_blocks += (block_size / 512) as u32;

    super::inode::write_inode_disk(fs, ino, inode)?;
    Ok(())
}

// ============================================================================
// mkdir implementation
// ============================================================================

/// Create a new directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - New directory name
/// * `mode` - Mode for new directory
///
/// # Returns
/// * Ok(new_inode_number) on success
/// * Err(i32) on failure
pub fn ext4_mkdir(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    // Wrap in journal transaction if journal is available
    if fs.journal.is_some() {
        return ext4_mkdir_inner(fs, dir_ino, name, mode);
    }
    ext4_mkdir_no_journal(fs, dir_ino, name, mode)
}

fn ext4_mkdir_no_journal(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    // Check name length
    if name.is_empty() || name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Check if parent link count would overflow
    let parent_inode = super::inode::read_inode(fs, dir_ino)?;
    if parent_inode.i_links_count >= EXT4_LINK_MAX {
        return Err(errno::Errno::TooManyLinks.as_neg_i32());
    }

    // Allocate new inode
    let dir_mode = mode & !S_IFMT | S_IFDIR;
    let (new_ino, mut new_inode) = ext4_new_inode(fs, dir_ino, dir_mode, name)?;

    // Allocate block for directory entries
    let allocator = BlockAllocator::new(fs);
    let block_nr = allocator.alloc_block()?;

    // Initialize directory with "." and ".."
    let block_size = fs.block_size as usize;
    let mut block_data = alloc::vec![0u8; block_size];

    // Create "." entry
    let dot_rec_len = block_size as u16;
    let dot_entry = create_dot_entry(new_ino, dot_rec_len);
    block_data[0..8].copy_from_slice(&dot_entry);

    // Create ".." entry
    let dotdot_offset = 8; // After "." entry
    let dotdot_rec_len = (block_size - dotdot_offset) as u16;
    let dotdot_entry = create_dotdot_entry(dir_ino, dotdot_rec_len);
    block_data[dotdot_offset..dotdot_offset + 8].copy_from_slice(&dotdot_entry);

    // Write directory block
    unsafe {
        write_block_from_vec(fs.device, block_nr, &block_data)?;
    }

    // Update new inode
    new_inode.i_block[0] = block_nr as u32;
    new_inode.i_size = block_size as u32;
    new_inode.i_blocks = (block_size / 512) as u32;
    new_inode.i_links_count = 2; // "." and parent's entry

    // Write new inode
    super::inode::write_inode_disk(fs, new_ino, &new_inode)?;

    // Add entry to parent directory
    ext4_add_entry(fs, dir_ino, name, new_ino, file_type::EXT4_FT_DIR)?;

    // Update parent link count
    let mut parent = parent_inode;
    parent.i_links_count += 1;
    super::inode::write_inode_disk(fs, dir_ino, &parent)?;

    // Sync all buffers to ensure directory is fully written
    bio::sync_buffers()?;

    Ok(new_ino)
}

fn ext4_mkdir_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    let mut handle = super::journal::ext4_journal_start(fs, 12)?;
    unsafe { set_current_handle(&mut handle); }

    let result = ext4_mkdir_no_journal(fs, dir_ino, name, mode);

    unsafe { clear_current_handle(); }
    super::journal::ext4_journal_stop(&mut handle)?;
    result
}

/// Create "." entry
fn create_dot_entry(ino: u32, rec_len: u16) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0..4].copy_from_slice(&ino.to_le_bytes());
    entry[4..6].copy_from_slice(&rec_len.to_le_bytes());
    entry[6] = 1; // name_len = 1
    entry[7] = file_type::EXT4_FT_DIR;
    entry
}

/// Create ".." entry
fn create_dotdot_entry(ino: u32, rec_len: u16) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0..4].copy_from_slice(&ino.to_le_bytes());
    entry[4..6].copy_from_slice(&rec_len.to_le_bytes());
    entry[6] = 2; // name_len = 2
    entry[7] = file_type::EXT4_FT_DIR;
    entry
}

// ============================================================================
// create implementation
// ============================================================================

/// Create a new regular file
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - New file name
/// * `mode` - Mode for new file
///
/// # Returns
/// * Ok(new_inode_number) on success
/// * Err(i32) on failure
pub fn ext4_create(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 8)?;
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_create_inner(fs, dir_ino, name, mode);
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_create_inner(fs, dir_ino, name, mode)
}

fn ext4_create_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    // Check name length
    if name.is_empty() || name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Allocate new inode
    let file_mode = mode & !S_IFMT | S_IFREG;
    let (new_ino, new_inode) = ext4_new_inode(fs, dir_ino, file_mode, name)?;

    // Write new inode (empty file)
    super::inode::write_inode_disk(fs, new_ino, &new_inode)?;

    // Add entry to parent directory
    ext4_add_entry(fs, dir_ino, name, new_ino, file_type::EXT4_FT_REG_FILE)?;

    Ok(new_ino)
}

// ============================================================================
// symlink implementation
// ============================================================================

/// Create a symbolic link
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Link name
/// * `target` - Symlink target path
///
/// # Returns
/// * Ok(inode number) on success
/// * Err(i32) on failure
pub fn ext4_symlink(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    target: &[u8],
) -> Result<u32, i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 8)?;
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_symlink_inner(fs, dir_ino, name, target);
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_symlink_inner(fs, dir_ino, name, target)
}

fn ext4_symlink_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    target: &[u8],
) -> Result<u32, i32> {
    // Check name length
    if name.is_empty() || name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Allocate new inode with S_IFLNK mode
    let (new_ino, mut new_inode) = ext4_new_inode(fs, dir_ino, S_IFLNK | 0o777, name)?;

    if target.len() <= 60 {
        // Fast symlink: target stored inline in i_block array (60 bytes = 15 * 4)
        unsafe {
            let block_ptr = new_inode.i_block.as_mut_ptr() as *mut u8;
            core::ptr::copy_nonoverlapping(target.as_ptr(), block_ptr, target.len());
        }
        new_inode.i_size = target.len() as u32;
    } else {
        // Slow symlink: target stored in data block
        let mut allocator = BlockAllocator::new(fs);
        let blocknr = allocator.alloc_block()? as u32;

        // Write target path to data block
        let mut block_data = alloc::vec![0u8; fs.block_size as usize];
        block_data[..target.len()].copy_from_slice(target);
        unsafe { write_block_from_vec(fs.device, blocknr as u64, &block_data)?; }

        new_inode.i_block[0] = blocknr;
        new_inode.i_size = target.len() as u32;
        // i_blocks counts 512-byte sectors
        new_inode.i_blocks += (fs.block_size as u32) / 512;
    }

    // Write inode to disk
    super::inode::write_inode_disk(fs, new_ino, &new_inode)?;

    // Add directory entry
    ext4_add_entry(fs, dir_ino, name, new_ino, file_type::EXT4_FT_SYMLINK)?;

    Ok(new_ino)
}

// ============================================================================
// link implementation
// ============================================================================

/// Create a hard link
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `target_ino` - Target inode number to link to
/// * `name` - New link name
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_link(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    target_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 6)?;
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_link_inner(fs, dir_ino, target_ino, name);
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_link_inner(fs, dir_ino, target_ino, name)
}

fn ext4_link_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    target_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    // Validate name
    if name.is_empty() || name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Read target inode
    let mut target_inode = super::inode::read_inode(fs, target_ino)?;

    // Cannot hard link to directories
    if (target_inode.i_mode & S_IFMT) == S_IFDIR {
        return Err(errno::Errno::IsADirectory.as_neg_i32());
    }

    // Check link count limit
    if target_inode.i_links_count >= EXT4_LINK_MAX {
        return Err(errno::Errno::TooManyLinks.as_neg_i32());
    }

    // Check if name already exists in parent directory
    let dir_inode = super::inode::read_inode(fs, dir_ino)?;
    if find_dir_entry(fs, &dir_inode, name).is_ok() {
        return Err(errno::Errno::FileExists.as_neg_i32());
    }

    // Increment link count
    target_inode.i_links_count += 1;

    // Update timestamp
    let cycles = crate::drivers::intc::clint::read_time();
    let sec = (cycles / 10_000_000) as u32;
    target_inode.i_ctime = sec;

    // Write updated inode back
    super::inode::write_inode_disk(fs, target_ino, &target_inode)?;

    // Add directory entry
    ext4_add_entry(fs, dir_ino, name, target_ino, file_type::EXT4_FT_REG_FILE)?;

    Ok(())
}

// ============================================================================
// unlink implementation
// ============================================================================

/// Delete a directory entry
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Entry name to delete
///
/// # Returns
/// * Ok(deleted_inode_number) on success
/// * Err(i32) on failure
pub fn ext4_delete_entry(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<u32, i32> {
    // Read parent directory inode
    let dir_inode = super::inode::read_inode(fs, dir_ino)?;

    // Find the entry
    let (block_nr, offset, entry_ino) = find_dir_entry(fs, &dir_inode, name)?;

    // Read the block
    let block_size = fs.block_size as usize;
    let mut block_data = unsafe {
        read_block_to_vec(fs.device, block_nr, block_size)?
    };

    // Get current entry's record length
    let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);

    // Find previous entry
    let prev_offset = find_prev_entry(&block_data, offset, block_size);

    if prev_offset != offset {
        // Merge with previous entry
        let prev_rec_len = u16::from_le_bytes([
            block_data[prev_offset + 4],
            block_data[prev_offset + 5],
        ]);

        let new_rec_len = prev_rec_len + rec_len;
        let new_rec_bytes = new_rec_len.to_le_bytes();
        block_data[prev_offset + 4] = new_rec_bytes[0];
        block_data[prev_offset + 5] = new_rec_bytes[1];
    }

    // Clear the entry (set inode to 0)
    block_data[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());

    // Write block back
    unsafe {
        write_block_from_vec(fs.device, block_nr, &block_data)?;
    }

    Ok(entry_ino)
}

/// Find directory entry
fn find_dir_entry(
    fs: &Ext4FileSystem,
    dir_inode: &Ext4InodeOnDisk,
    name: &[u8],
) -> Result<(u64, usize, u32), i32> {
    let block_size = fs.block_size as usize;
    let dir_size = dir_inode.i_size as usize;
    let num_blocks = if block_size > 0 {
        (dir_size + block_size - 1) / block_size
    } else {
        0
    };

    for block_idx in 0..num_blocks as u64 {
        let block_nr = get_dir_block_nr(fs, dir_inode, block_idx)?;

        if block_nr == 0 {
            continue;
        }

        let block_data = unsafe {
            read_block_to_vec(fs.device, block_nr, block_size)?
        };

        // Search for entry in this block
        let mut offset = 0;
        while offset + 8 <= block_size {
            let ino = u32::from_le_bytes([
                block_data[offset],
                block_data[offset + 1],
                block_data[offset + 2],
                block_data[offset + 3],
            ]);

            if ino == 0 {
                break;
            }

            let rec_len = u16::from_le_bytes([
                block_data[offset + 4],
                block_data[offset + 5],
            ]);

            if rec_len == 0 {
                break;
            }

            let entry_name_len = block_data[offset + 6] as usize;

            // Compare name
            if entry_name_len == name.len() && offset + 8 + entry_name_len <= block_data.len() {
                let entry_name = &block_data[offset + 8..offset + 8 + entry_name_len];
                if entry_name == name {
                    return Ok((block_nr, offset, ino));
                }
            }

            offset += rec_len as usize;
        }
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
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

    target_offset // No previous entry found
}

/// Unlink a file
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Entry name to unlink
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_unlink(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 8)?;
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_unlink_inner(fs, dir_ino, name);
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_unlink_inner(fs, dir_ino, name)
}

fn ext4_unlink_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    // Check name
    if name.is_empty() || name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Delete directory entry
    let entry_ino = ext4_delete_entry(fs, dir_ino, name)?;

    // Read the unlinked inode
    let mut inode = super::inode::read_inode(fs, entry_ino)?;

    // Decrement link count
    if inode.i_links_count > 0 {
        inode.i_links_count -= 1;
    }

    // If link count is 0, free data blocks and inode
    if inode.i_links_count == 0 {
        inode.i_dtime = 1; // TODO: get current time

        // Free data blocks
        free_inode_blocks(fs, &inode)?;

        // Free inode (mark in bitmap)
        free_inode(fs, entry_ino)?;
    }

    // Write inode back
    super::inode::write_inode_disk(fs, entry_ino, &inode)?;

    Ok(())
}

// ============================================================================
// rmdir implementation
// ============================================================================

/// Remove an empty directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Directory name to remove
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_rmdir(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 10)?;
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_rmdir_inner(fs, dir_ino, name);
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_rmdir_inner(fs, dir_ino, name)
}

fn ext4_rmdir_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    // Check name
    if name.is_empty() || name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Find the directory entry first
    let parent_inode = super::inode::read_inode(fs, dir_ino)?;
    let (_, _, target_ino) = find_dir_entry(fs, &parent_inode, name)?;

    // Read target directory inode
    let target_inode = super::inode::read_inode(fs, target_ino)?;

    // Verify it's a directory
    if (target_inode.i_mode & S_IFMT) != S_IFDIR {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Check if directory is empty (only "." and "..")
    if !is_dir_empty(fs, &target_inode)? {
        return Err(errno::Errno::DirectoryNotEmpty.as_neg_i32());
    }

    // Delete directory entry from parent
    ext4_delete_entry(fs, dir_ino, name)?;

    // Update parent link count
    let mut parent = parent_inode;
    if parent.i_links_count > 0 {
        parent.i_links_count -= 1;
    }
    super::inode::write_inode_disk(fs, dir_ino, &parent)?;

    // Free the target inode
    let mut target = target_inode;
    target.i_links_count = 0;
    target.i_dtime = 1; // TODO: get current time
    super::inode::write_inode_disk(fs, target_ino, &target)?;

    // Free inode in bitmap
    free_inode(fs, target_ino)?;

    // Free data blocks
    free_inode_blocks(fs, &target)?;

    Ok(())
}

/// Check if directory is empty
fn is_dir_empty(fs: &Ext4FileSystem, inode: &Ext4InodeOnDisk) -> Result<bool, i32> {
    let block_size = fs.block_size as usize;
    let dir_size = inode.i_size as usize;

    if dir_size == 0 {
        return Ok(true);
    }

    // Read first block
    let block_nr = get_dir_block_nr(fs, inode, 0)?;
    if block_nr == 0 {
        return Ok(true);
    }

    let block_data = unsafe {
        read_block_to_vec(fs.device, block_nr, block_size)?
    };

    // Check entries - skip "." and ".."
    let mut offset = 0;
    let mut entry_count = 0;

    while offset + 8 <= block_size {
        let ino = u32::from_le_bytes([
            block_data[offset],
            block_data[offset + 1],
            block_data[offset + 2],
            block_data[offset + 3],
        ]);

        if ino == 0 {
            break;
        }

        let rec_len = u16::from_le_bytes([
            block_data[offset + 4],
            block_data[offset + 5],
        ]);

        if rec_len == 0 {
            break;
        }

        entry_count += 1;

        // More than 2 entries means not empty (".", "..", and others)
        if entry_count > 2 {
            return Ok(false);
        }

        offset += rec_len as usize;
    }

    Ok(true)
}

/// Free an inode in the bitmap
fn free_inode(fs: &Ext4FileSystem, ino: u32) -> Result<(), i32> {
    let inodes_per_group = fs.inodes_per_group;
    let group = (ino - 1) / inodes_per_group;
    let ino_in_group = (ino - 1) % inodes_per_group;

    // Get group descriptor
    let bitmap_block = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }
        group_descs[group as usize].bg_inode_bitmap
    };

    // Read bitmap
    let bitmap_data = unsafe {
        read_block_to_vec(fs.device, bitmap_block as u64, fs.block_size as usize)?
    };

    // Clear bit
    let byte_idx = ino_in_group as usize / 8;
    let bit_idx = ino_in_group as usize % 8;

    let mut new_bitmap = bitmap_data.to_vec();
    new_bitmap[byte_idx] &= !(1 << bit_idx);

    // Write bitmap back
    unsafe {
        write_block_from_vec(fs.device, bitmap_block as u64, &new_bitmap)?;
    }

    // Update group descriptor
    update_group_descriptor_inodes(fs, group, 1)?;

    // Update superblock
    update_superblock_free_inodes(fs, 1)?;

    Ok(())
}

/// Free an indirect block and all data blocks it references (recursive for multi-level)
///
/// # Arguments
/// * `fs` - Filesystem
/// * `allocator` - Block allocator
/// * `blocknr` - Block number of the indirect block
/// * `depth` - Indirection depth (1=single, 2=double, 3=triple)
fn free_indirect_block(
    fs: &Ext4FileSystem,
    allocator: &BlockAllocator,
    blocknr: u32,
    depth: u32,
) -> Result<(), i32> {
    let ptrs_per_block = (fs.block_size as usize) / 4;

    let data = unsafe {
        read_block_to_vec(fs.device, blocknr as u64, fs.block_size as usize)?
    };

    let pointers: &[u32] = unsafe {
        core::slice::from_raw_parts(data.as_ptr() as *const u32, ptrs_per_block)
    };

    for &ptr in pointers {
        if ptr == 0 { continue; }
        if depth > 1 {
            free_indirect_block(fs, allocator, ptr, depth - 1)?;
        } else {
            allocator.free_block(ptr as u64)?;
        }
    }

    // Free the indirect block itself
    allocator.free_block(blocknr as u64)?;
    Ok(())
}

/// Free all blocks associated with an inode
fn free_inode_blocks(fs: &Ext4FileSystem, inode: &Ext4InodeOnDisk) -> Result<(), i32> {
    let allocator = BlockAllocator::new(fs);

    // Check if using extents
    if (inode.i_flags & 0x80000) != 0 {
        // Free blocks referenced by extent entries
        let header = unsafe {
            &*(inode.i_block.as_ptr() as *const super::extent::Ext4ExtentHeader)
        };
        if header.eh_magic == super::extent::EXT4_EXT_MAGIC && header.eh_depth == 0 {
            let entries = unsafe {
                core::slice::from_raw_parts(
                    (inode.i_block.as_ptr() as *const u8)
                        .add(core::mem::size_of::<super::extent::Ext4ExtentHeader>())
                        as *const super::extent::Ext4Extent,
                    header.eh_entries as usize,
                )
            };
            for ext in entries {
                let start = ext.start_block();
                for i in 0..ext.length() as u64 {
                    allocator.free_block(start + i)?;
                }
            }
        }
        return Ok(());
    }

    // Direct/indirect block mode: free direct blocks
    for i in 0..12 {
        if inode.i_block[i] != 0 {
            allocator.free_block(inode.i_block[i] as u64)?;
        }
    }

    // Free single indirect block
    if inode.i_block[12] != 0 {
        free_indirect_block(fs, &allocator, inode.i_block[12], 1)?;
    }
    // Free double indirect block
    if inode.i_block[13] != 0 {
        free_indirect_block(fs, &allocator, inode.i_block[13], 2)?;
    }
    // Free triple indirect block
    if inode.i_block[14] != 0 {
        free_indirect_block(fs, &allocator, inode.i_block[14], 3)?;
    }

    Ok(())
}

// ============================================================================
// rename implementation
// ============================================================================

/// Rename a file or directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `old_dir_ino` - Old parent directory inode number
/// * `old_name` - Old entry name
/// * `new_dir_ino` - New parent directory inode number
/// * `new_name` - New entry name
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_rename(
    fs: &Ext4FileSystem,
    old_dir_ino: u32,
    old_name: &[u8],
    new_dir_ino: u32,
    new_name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 16)?;
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_rename_inner(fs, old_dir_ino, old_name, new_dir_ino, new_name);
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_rename_inner(fs, old_dir_ino, old_name, new_dir_ino, new_name)
}

fn ext4_rename_inner(
    fs: &Ext4FileSystem,
    old_dir_ino: u32,
    old_name: &[u8],
    new_dir_ino: u32,
    new_name: &[u8],
) -> Result<(), i32> {
    // Validate names
    if old_name.is_empty() || old_name.len() > 255 || new_name.is_empty() || new_name.len() > 255 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // Read parent directory inodes
    let old_dir_inode = super::inode::read_inode(fs, old_dir_ino)?;
    let new_dir_inode = super::inode::read_inode(fs, new_dir_ino)?;

    // Find old entry
    let (_, _, old_ino) = find_dir_entry(fs, &old_dir_inode, old_name)?;

    // Read the inode being renamed
    let old_inode = super::inode::read_inode(fs, old_ino)?;
    let old_is_dir = (old_inode.i_mode & S_IFMT) == S_IFDIR;

    // Determine file type for new directory entry
    let new_file_type = if old_is_dir {
        file_type::EXT4_FT_DIR
    } else {
        file_type::EXT4_FT_REG_FILE
    };

    // Check if new name already exists
    let target_exists = find_dir_entry(fs, &new_dir_inode, new_name).ok();

    if let Some((_, _, target_ino)) = target_exists {
        // Cannot rename to self
        if target_ino == old_ino {
            return Ok(());
        }

        let target_inode = super::inode::read_inode(fs, target_ino)?;
        let target_is_dir = (target_inode.i_mode & S_IFMT) == S_IFDIR;

        // Type checks
        if old_is_dir && !target_is_dir {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }
        if !old_is_dir && target_is_dir {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }
        if target_is_dir && !is_dir_empty(fs, &target_inode)? {
            return Err(errno::Errno::DirectoryNotEmpty.as_neg_i32());
        }

        // Delete existing target entry
        ext4_delete_entry(fs, new_dir_ino, new_name)?;

        // Clean up the replaced inode
        let mut target_mut = target_inode;
        if target_is_dir {
            // Decrement new parent's link count (was incremented by mkdir)
            let mut new_parent = new_dir_inode;
            if new_parent.i_links_count > 0 {
                new_parent.i_links_count -= 1;
            }
            super::inode::write_inode_disk(fs, new_dir_ino, &new_parent)?;

            // Free target directory
            target_mut.i_links_count = 0;
            target_mut.i_dtime = 1;
            super::inode::write_inode_disk(fs, target_ino, &target_mut)?;
            free_inode(fs, target_ino)?;
            free_inode_blocks(fs, &target_mut)?;
        } else {
            // Decrement link count of replaced file
            if target_mut.i_links_count > 0 {
                target_mut.i_links_count -= 1;
            }
            if target_mut.i_links_count == 0 {
                target_mut.i_dtime = 1;
                free_inode(fs, target_ino)?;
            }
            super::inode::write_inode_disk(fs, target_ino, &target_mut)?;
        }
    }

    // Prevent renaming a directory into its own subdirectory
    if old_is_dir && old_dir_ino == new_dir_ino && old_name == new_name {
        return Ok(());
    }

    // Add new directory entry
    ext4_add_entry(fs, new_dir_ino, new_name, old_ino, new_file_type)?;

    // Delete old directory entry
    ext4_delete_entry(fs, old_dir_ino, old_name)?;

    // Update timestamp on renamed inode
    let cycles = crate::drivers::intc::clint::read_time();
    let sec = (cycles / 10_000_000) as u32;
    let mut renamed_inode = old_inode;
    renamed_inode.i_ctime = sec;
    renamed_inode.i_mtime = sec;
    super::inode::write_inode_disk(fs, old_ino, &renamed_inode)?;

    // If renaming a directory, update parent link counts and ".." entry
    if old_is_dir {
        let mut old_parent = old_dir_inode;
        let mut new_parent = new_dir_inode;

        if old_dir_ino != new_dir_ino {
            // Decrement old parent's link count
            if old_parent.i_links_count > 0 {
                old_parent.i_links_count -= 1;
            }
            // Increment new parent's link count
            new_parent.i_links_count += 1;

            // Update ".." entry in the renamed directory to point to new parent
            update_dotdot(fs, old_ino, new_dir_ino)?;
        }

        // Update timestamps on parent directories
        old_parent.i_ctime = sec;
        old_parent.i_mtime = sec;
        new_parent.i_ctime = sec;
        new_parent.i_mtime = sec;

        super::inode::write_inode_disk(fs, old_dir_ino, &old_parent)?;
        if old_dir_ino != new_dir_ino {
            super::inode::write_inode_disk(fs, new_dir_ino, &new_parent)?;
        }
    } else {
        // Update timestamps on parent directories for file rename
        let mut old_parent = old_dir_inode;
        old_parent.i_ctime = sec;
        old_parent.i_mtime = sec;
        super::inode::write_inode_disk(fs, old_dir_ino, &old_parent)?;

        if old_dir_ino != new_dir_ino {
            let mut new_parent = new_dir_inode;
            new_parent.i_ctime = sec;
            new_parent.i_mtime = sec;
            super::inode::write_inode_disk(fs, new_dir_ino, &new_parent)?;
        }
    }

    Ok(())
}

/// Update the ".." entry of a directory to point to a new parent
fn update_dotdot(fs: &Ext4FileSystem, dir_ino: u32, new_parent_ino: u32) -> Result<(), i32> {
    let dir_inode = super::inode::read_inode(fs, dir_ino)?;
    let block_size = fs.block_size as usize;

    // ".." is always the first entry in the first block
    let block_nr = get_dir_block_nr(fs, &dir_inode, 0)?;
    if block_nr == 0 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    let mut block_data = unsafe {
        read_block_to_vec(fs.device, block_nr, block_size)?
    };

    // First entry is ".", second is ".."
    // Skip "." entry (at offset 0)
    let dot_rec_len = u16::from_le_bytes([block_data[4], block_data[5]]);
    let dotdot_offset = dot_rec_len as usize;

    // Verify this is ".." entry
    if block_data.len() < dotdot_offset + 8 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Update inode number of ".." entry
    let new_parent_bytes = new_parent_ino.to_le_bytes();
    block_data[dotdot_offset] = new_parent_bytes[0];
    block_data[dotdot_offset + 1] = new_parent_bytes[1];
    block_data[dotdot_offset + 2] = new_parent_bytes[2];
    block_data[dotdot_offset + 3] = new_parent_bytes[3];

    // Write block back
    unsafe {
        write_block_from_vec(fs.device, block_nr, &block_data)?;
    }

    Ok(())
}
