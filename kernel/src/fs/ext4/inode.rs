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
