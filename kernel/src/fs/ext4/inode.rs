//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 inode operations

use core::mem;
use alloc::vec::Vec;

use crate::errno;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4InodeOnDisk {
    /// File mode (type and permissions)
    pub i_mode: u16,
    /// User ID
    pub i_uid: u16,
    /// File size
    pub i_size: u32,
    /// Last access time
    pub i_atime: u32,
    /// Last inode modification time
    pub i_ctime: u32,
    /// Last data modification time
    pub i_mtime: u32,
    /// Deletion time
    pub i_dtime: u32,
    /// Group ID
    pub i_gid: u16,
    /// Link count
    pub i_links_count: u16,
    /// Block count
    pub i_blocks: u32,
    /// Flags
    pub i_flags: u32,
    /// OS specific value 1
    pub osd1: u32,
    /// Direct block pointers
    pub i_block: [u32; 15],
    /// Generation number
    pub i_generation: u32,
    /// File access control
    pub i_file_acl: u32,
    /// File access control (high)
    pub i_file_acl_high: u32,
    /// Directory ACL
    pub i_dir_acl: u32,
    /// Block address (high)
    pub i_dir_acl_high: u32,
    /// Fragment address
    pub i_faddr: u32,
    /// OS specific value 2
    pub osd2: [u8; 12],
    /// Extra inode size
    pub i_extra_isize: u16,
    /// Checksum
    pub i_checksum: u16,
    /// ctime extension
    pub i_ctime_extra: u32,
    /// mtime extension
    pub i_mtime_extra: u32,
    /// atime extension
    pub i_atime_extra: u32,
    /// crtime (creation time)
    pub i_crtime: u32,
    /// crtime extension
    pub i_crtime_extra: u32,
    /// Project ID
    pub i_projid: u32,
    /// Reserved
    pub i_reserved: [u32; 4],
}

impl Default for Ext4InodeOnDisk {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

impl Ext4InodeOnDisk {
    /// Check if directory
    pub fn is_dir(&self) -> bool {
        (self.i_mode & 0xF000) == 0x4000
    }

    /// Check if regular file
    pub fn is_reg(&self) -> bool {
        (self.i_mode & 0xF000) == 0x8000
    }

    /// Check if symbolic link
    pub fn is_symlink(&self) -> bool {
        (self.i_mode & 0xF000) == 0xA000
    }

    /// Check if using extent
    pub fn has_extent(&self) -> bool {
        (self.i_flags & 0x80000) != 0
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Ext4Inode {
    /// Inode number
    pub ino: u32,
    /// File mode
    pub mode: u16,
    /// User ID
    pub uid: u16,
    /// Group ID
    pub gid: u16,
    /// File size
    pub size: u64,
    /// Block count
    pub blocks: u64,
    /// Link count
    pub links_count: u16,
    /// Flags
    pub flags: u32,
    /// Direct block pointers
    pub block: [u32; 15],
    /// Last access time
    pub atime: u32,
    /// Last modification time
    pub mtime: u32,
    /// Creation time
    pub ctime: u32,
}

impl Ext4Inode {
    /// Create from disk format
    pub fn from_disk(disk: &Ext4InodeOnDisk, ino: u32) -> Self {
        Self {
            ino,
            mode: disk.i_mode,
            uid: disk.i_uid,
            gid: disk.i_gid,
            size: disk.i_size as u64,
            blocks: disk.i_blocks as u64,
            links_count: disk.i_links_count,
            flags: disk.i_flags,
            block: disk.i_block,
            atime: disk.i_atime,
            mtime: disk.i_mtime,
            ctime: disk.i_ctime,
        }
    }

    /// Check if directory
    pub fn is_dir(&self) -> bool {
        (self.mode & 0xF000) == 0x4000
    }

    /// Check if regular file
    pub fn is_reg(&self) -> bool {
        (self.mode & 0xF000) == 0x8000
    }

    /// Check if symbolic link
    pub fn is_symlink(&self) -> bool {
        (self.mode & 0xF000) == 0xA000
    }

    /// Check if using extent
    pub fn has_extent(&self) -> bool {
        (self.flags & 0x80000) != 0
    }

    /// Get file size
    pub fn get_size(&self) -> u64 {
        self.size
    }

    /// Set file size
    pub fn set_size(&mut self, size: u64) {
        self.size = size;
    }

    /// Get data block list
    ///
    /// Supports both extent and indirect block modes
    pub fn get_data_blocks(&self, fs: &super::super::ext4::Ext4FileSystem) -> Result<Vec<u64>, i32> {
        let mut blocks = Vec::new();

        let remaining_blocks = (self.size + fs.block_size as u64 - 1) / (fs.block_size as u64);

        // Check if using extent
        if self.has_extent() {
            // Search using extent tree
            for i in 0..remaining_blocks {
                match super::extent::ext4_ext_get_block(fs, &self.block, i) {
                    Ok(block_num) => {
                        if block_num != 0 {
                            blocks.push(block_num);
                        } else {
                            // Sparse file, block not allocated
                            blocks.push(0);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        } else {
            // Use indirect block module to get all data blocks
            for i in 0..remaining_blocks {
                match super::indirect::ext4_get_block(fs, &self.block, i) {
                    Ok(block_num) => {
                        if block_num != 0 {
                            blocks.push(block_num);
                        } else {
                            // Sparse file, block not allocated
                            blocks.push(0);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(blocks)
    }

    /// Get data block number at specified block index
    ///
    /// Supports both extent and indirect block modes
    pub fn get_data_block(&self, fs: &super::super::ext4::Ext4FileSystem, block_index: u64) -> Result<u64, i32> {
        if self.has_extent() {
            super::extent::ext4_ext_get_block(fs, &self.block, block_index)
        } else {
            super::indirect::ext4_get_block(fs, &self.block, block_index)
        }
    }

    /// Read file data
    ///
    /// Read data from specified offset
    pub fn read_data(
        &self,
        fs: &super::super::ext4::Ext4FileSystem,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, i32> {
        use crate::fs::bio;

        let file_size = self.get_size();
        if offset >= file_size {
            return Ok(0);
        }

        let available = file_size - offset;
        let to_read = core::cmp::min(buf.len() as u64, available) as usize;

        let blocks = self.get_data_blocks(fs)?;
        let block_size = fs.block_size as usize;

        let mut total_read = 0;
        let mut current_offset = offset as usize;
        let mut buf_offset = 0;

        while total_read < to_read {
            let block_index = current_offset / block_size;
            let block_offset = current_offset % block_size;

            if block_index >= blocks.len() {
                break;
            }

            unsafe {
                let bh = bio::bread(fs.device, blocks[block_index])
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let remaining = to_read - total_read;
                let available_in_block = block_size - block_offset;
                let read_in_block = core::cmp::min(remaining, available_in_block);

                buf[buf_offset..buf_offset + read_in_block]
                    .copy_from_slice(&data[block_offset..block_offset + read_in_block]);

                total_read += read_in_block;
                buf_offset += read_in_block;
                current_offset += read_in_block;

                bio::brelse(bh);
            }
        }

        Ok(total_read)
    }
}

/// Read ext4 inode from disk
///
/// # Arguments
/// - `fs`: ext4 filesystem reference
/// - `ino`: inode number
///
/// # Returns
/// - `Ok(Ext4InodeOnDisk)` on success
/// - `Err(errno)` on failure
pub fn read_inode(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
) -> Result<Ext4InodeOnDisk, i32> {
    use crate::fs::bio;

    // Calculate block group and inode table index
    let group = (ino - 1) / fs.inodes_per_group;
    let index = (ino - 1) % fs.inodes_per_group;

    let inode_table_start = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }
        group_descs[group as usize].bg_inode_table as u64
    };

    let inodes_per_block = fs.block_size / (fs.inode_size as u32);
    let block_offset = index / inodes_per_block;
    let in_block_offset = ((index % inodes_per_block) * (fs.inode_size as u32)) as usize;

    // Read block containing inode
    let bh = bio::bread(fs.device, inode_table_start + block_offset as u64)
        .ok_or(errno::Errno::IOError.as_neg_i32())?;

    let data = unsafe { &(*bh).b_data };

    // Read inode from block buffer
    let mut inode_on_disk = Ext4InodeOnDisk::default();
    let inode_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut inode_on_disk as *mut _ as *mut u8,
            core::mem::size_of::<Ext4InodeOnDisk>(),
        )
    };
    inode_bytes.copy_from_slice(&data[in_block_offset..in_block_offset + inode_bytes.len()]);

    bio::brelse(bh);

    Ok(inode_on_disk)
}

/// Get block number from inode at given index
///
/// # Arguments
/// - `inode`: inode reference (on-disk format)
/// - `block_idx`: block index (0-11 for direct blocks)
///
/// # Returns
/// - `Ok(block_number)` on success
/// - `Err(errno)` on failure
pub fn get_block_nr(inode: &Ext4InodeOnDisk, block_idx: usize) -> Result<u64, i32> {
    // For now, only handle direct blocks (0-11)
    if block_idx < 12 {
        Ok(inode.i_block[block_idx] as u64)
    } else {
        // TODO: Handle indirect blocks (12-14)
        Err(errno::Errno::InvalidArgument.as_neg_i32())
    }
}

/// Write ext4 inode (Ext4Inode format) back to disk
///
/// # Arguments
/// - `fs`: ext4 filesystem reference
/// - `ino`: inode number
/// - `inode`: ext4 inode data to write
///
/// # Returns
/// - `Ok(())` on success
/// - `Err(errno)` on failure
pub fn write_inode(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
    inode: &Ext4Inode,
) -> Result<(), i32> {
    use crate::fs::bio;

    // Calculate block group and inode table index
    let group = (ino - 1) / fs.inodes_per_group;
    let index = (ino - 1) % fs.inodes_per_group;

    let inode_table_start = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }
        group_descs[group as usize].bg_inode_table as u64
    };

    let inodes_per_block = fs.block_size / (fs.inode_size as u32);
    let block_offset = index / inodes_per_block;
    let in_block_offset = ((index % inodes_per_block) * (fs.inode_size as u32)) as usize;

    // Read block containing inode
    let bh = bio::bread(fs.device, inode_table_start + block_offset as u64)
        .ok_or(errno::Errno::IOError.as_neg_i32())?;

    let data = unsafe { &mut (*bh).b_data };

    // Convert Ext4Inode to on-disk format
    // Read existing on-disk inode first to preserve untracked fields
    let mut inode_on_disk = Ext4InodeOnDisk::default();
    let src_ptr = data[in_block_offset..].as_ptr() as *const Ext4InodeOnDisk;
    unsafe { core::ptr::copy_nonoverlapping(src_ptr, &mut inode_on_disk, 1) };

    // Update tracked fields
    inode_on_disk.i_mode = inode.mode;
    inode_on_disk.i_uid = inode.uid;
    inode_on_disk.i_size = inode.size as u32;
    inode_on_disk.i_atime = inode.atime;
    inode_on_disk.i_ctime = inode.ctime;
    inode_on_disk.i_mtime = inode.mtime;
    inode_on_disk.i_gid = inode.gid;
    inode_on_disk.i_links_count = inode.links_count;
    inode_on_disk.i_blocks = inode.blocks as u32;
    inode_on_disk.i_flags = inode.flags;
    inode_on_disk.i_block = inode.block;
    inode_on_disk.i_dir_acl = (inode.size >> 32) as u32;

    // Write inode to block buffer
    let inode_bytes = unsafe {
        core::slice::from_raw_parts(
            &inode_on_disk as *const _ as *const u8,
            core::mem::size_of::<Ext4InodeOnDisk>(),
        )
    };
    data[in_block_offset..in_block_offset + inode_bytes.len()].copy_from_slice(inode_bytes);

    // Mark buffer dirty and sync
    unsafe { (*bh).set_state_bit(bio::BufferState::BH_Dirty) };

    // Journal the inode table block if a transaction is active
    unsafe {
        if let Some(handle) = crate::fs::ext4::namei::get_current_handle() {
            let _ = crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh);
        }
    }

    bio::sync_dirty_buffer(bh)?;
    bio::brelse(bh);

    Ok(())
}

/// Write ext4 inode (on-disk format) back to disk
///
/// # Arguments
/// - `fs`: ext4 filesystem reference
/// - `ino`: inode number
/// - `inode`: ext4 inode data to write (on-disk format)
///
/// # Returns
/// - `Ok(())` on success
/// - `Err(errno)` on failure
pub fn write_inode_disk(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
    inode: &Ext4InodeOnDisk,
) -> Result<(), i32> {
    use crate::fs::bio;

    // Calculate block group and inode table index
    let group = (ino - 1) / fs.inodes_per_group;
    let index = (ino - 1) % fs.inodes_per_group;

    let inode_table_start = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }
        group_descs[group as usize].bg_inode_table as u64
    };

    let inodes_per_block = fs.block_size / (fs.inode_size as u32);
    let block_offset = index / inodes_per_block;
    let in_block_offset = ((index % inodes_per_block) * (fs.inode_size as u32)) as usize;

    // Read block containing inode
    let bh = bio::bread(fs.device, inode_table_start + block_offset as u64)
        .ok_or(errno::Errno::IOError.as_neg_i32())?;

    let data = unsafe { &mut (*bh).b_data };

    // Write inode to block buffer
    let inode_bytes = unsafe {
        core::slice::from_raw_parts(
            inode as *const _ as *const u8,
            core::mem::size_of::<Ext4InodeOnDisk>(),
        )
    };
    data[in_block_offset..in_block_offset + inode_bytes.len()].copy_from_slice(inode_bytes);

    // Mark buffer dirty and sync
    unsafe { (*bh).set_state_bit(bio::BufferState::BH_Dirty) };

    // Journal the inode table block if a transaction is active
    unsafe {
        let handle_ptr = crate::fs::ext4::namei::get_current_handle();
        if let Some(handle) = handle_ptr {
            let _ = crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh);
        }
    }

    bio::sync_dirty_buffer(bh)?;
    bio::brelse(bh);

    Ok(())
}

pub mod file_type {
    /// FIFO
    pub const S_IFIFO: u16 = 0o010000;
    /// Character device
    pub const S_IFCHR: u16 = 0o020000;
    /// Directory
    pub const S_IFDIR: u16 = 0o040000;
    /// Block device
    pub const S_IFBLK: u16 = 0o060000;
    /// Regular file
    pub const S_IFREG: u16 = 0o100000;
    /// Symbolic link
    pub const S_IFLNK: u16 = 0o120000;
    /// Socket
    pub const S_IFSOCK: u16 = 0o140000;

    /// File type mask
    pub const S_IFMT: u16 = 0o170000;
}

pub mod perm {
    /// Owner read
    pub const S_IRUSR: u16 = 0o400;
    /// Owner write
    pub const S_IWUSR: u16 = 0o200;
    /// Owner execute
    pub const S_IXUSR: u16 = 0o100;
    /// Group read
    pub const S_IRGRP: u16 = 0o040;
    /// Group write
    pub const S_IWGRP: u16 = 0o020;
    /// Group execute
    pub const S_IXGRP: u16 = 0o010;
    /// Others read
    pub const S_IROTH: u16 = 0o004;
    /// Others write
    pub const S_IWOTH: u16 = 0o002;
    /// Others execute
    pub const S_IXOTH: u16 = 0o001;
}
