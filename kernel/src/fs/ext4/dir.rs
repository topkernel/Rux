//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 directory operations

use alloc::vec::Vec;

use crate::errno;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Ext4DirEntry {
    /// inode number
    pub inode: u32,
    /// record length
    pub rec_len: u16,
    /// name length
    pub name_len: u8,
    /// file type
    pub file_type: u8,
    /// filename
    pub name: [u8; 255],
}

impl Ext4DirEntry {
    /// Create directory entry from byte data
    ///
    /// # Safety
    /// bytes must contain at least 8 bytes
    pub unsafe fn from_bytes(bytes: &[u8], block_size: usize) -> Self {
        let inode = u32::from_le_bytes(*(bytes[0..4].as_ptr() as *const [u8; 4]));
        let rec_len = u16::from_le_bytes(*(bytes[4..6].as_ptr() as *const [u8; 2]));
        let name_len = bytes[6];
        let file_type = bytes[7];

        let mut name = [0u8; 255];
        if name_len as usize + 8 <= block_size {
            name[..name_len as usize].copy_from_slice(&bytes[8..8 + name_len as usize]);
        }

        Self {
            inode,
            rec_len,
            name_len,
            file_type,
            name,
        }
    }

    /// Get filename
    pub fn get_name(&self) -> &str {
        unsafe {
            core::str::from_utf8_unchecked(&self.name[..self.name_len as usize])
        }
    }

    /// Check if directory
    pub fn is_dir(&self) -> bool {
        self.file_type == 2
    }

    /// Check if regular file
    pub fn is_reg(&self) -> bool {
        self.file_type == 1
    }

    /// Check if symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type == 7
    }
}

pub mod file_type {
    /// Unknown
    pub const EXT4_FT_UNKNOWN: u8 = 0;
    /// Regular file
    pub const EXT4_FT_REG_FILE: u8 = 1;
    /// Directory
    pub const EXT4_FT_DIR: u8 = 2;
    /// Character device
    pub const EXT4_FT_CHRDEV: u8 = 3;
    /// Block device
    pub const EXT4_FT_BLKDEV: u8 = 4;
    /// FIFO
    pub const EXT4_FT_FIFO: u8 = 5;
    /// Socket
    pub const EXT4_FT_SOCK: u8 = 6;
    /// Symbolic link
    pub const EXT4_FT_SYMLINK: u8 = 7;
}

pub struct Ext4DirIterator {
    /// Block data
    data: Vec<u8>,
    /// Block size
    block_size: usize,
    /// Current offset
    offset: usize,
}

impl Ext4DirIterator {
    /// Create new directory iterator
    pub fn new(data: Vec<u8>, block_size: usize) -> Self {
        Self {
            data,
            block_size,
            offset: 0,
        }
    }
}

impl Iterator for Ext4DirIterator {
    type Item = Ext4DirEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }

        unsafe {
            let entry = Ext4DirEntry::from_bytes(&self.data[self.offset..], self.block_size);
            self.offset += entry.rec_len as usize;

            if entry.inode == 0 {
                // Skip deleted entries
                self.next()
            } else {
                Some(entry)
            }
        }
    }
}

pub fn ext4_find_entry(dir_data: &[u8], block_size: usize, name: &str) -> Result<Ext4DirEntry, i32> {
    let iter = Ext4DirIterator::new(dir_data.to_vec(), block_size);

    for entry in iter {
        if entry.get_name() == name {
            return Ok(entry);
        }
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
}
