//! ext4 目录操作
//!
//! 完全遵循 Linux 内核的 ext4 目录实现
//! 参考: fs/ext4/dir.c, include/linux/ext4_fs.h

use alloc::vec::Vec;

use crate::errno;

/// ext4 目录项
///
/// 对应 Linux 的 struct ext4_dir_entry (include/linux/ext4_fs.h)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct Ext4DirEntry {
    /// inode 编号
    pub inode: u32,
    /// 记录长度
    pub rec_len: u16,
    /// 名字长度
    pub name_len: u8,
    /// 文件类型
    pub file_type: u8,
    /// 文件名
    pub name: [u8; 255],
}

impl Ext4DirEntry {
    /// 从字节数据创建目录项
    ///
    /// # Safety
    /// bytes 必须至少包含 8 字节
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

    /// 获取文件名
    pub fn get_name(&self) -> &str {
        unsafe {
            core::str::from_utf8_unchecked(&self.name[..self.name_len as usize])
        }
    }

    /// 检查是否是目录
    pub fn is_dir(&self) -> bool {
        self.file_type == 2
    }

    /// 检查是否是常规文件
    pub fn is_reg(&self) -> bool {
        self.file_type == 1
    }

    /// 检查是否是符号链接
    pub fn is_symlink(&self) -> bool {
        self.file_type == 7
    }
}

/// 文件类型定义
///
/// 对应 Linux 的 ext4 文件类型
pub mod file_type {
    /// 未知
    pub const EXT4_FT_UNKNOWN: u8 = 0;
    /// 常规文件
    pub const EXT4_FT_REG_FILE: u8 = 1;
    /// 目录
    pub const EXT4_FT_DIR: u8 = 2;
    /// 字符设备
    pub const EXT4_FT_CHRDEV: u8 = 3;
    /// 块设备
    pub const EXT4_FT_BLKDEV: u8 = 4;
    /// FIFO
    pub const EXT4_FT_FIFO: u8 = 5;
    /// Socket
    pub const EXT4_FT_SOCK: u8 = 6;
    /// 符号链接
    pub const EXT4_FT_SYMLINK: u8 = 7;
}

/// ext4 目录迭代器
///
/// 用于遍历目录中的所有条目
pub struct Ext4DirIterator {
    /// 块数据
    data: Vec<u8>,
    /// 块大小
    block_size: usize,
    /// 当前偏移
    offset: usize,
}

impl Ext4DirIterator {
    /// 创建新的目录迭代器
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
                // 跳过已删除的条目
                self.next()
            } else {
                Some(entry)
            }
        }
    }
}

/// 查找目录项
///
/// 在目录中查找指定名称的条目
///
/// # 参数
/// - `dir_data`: 目录数据
/// - `block_size`: 块大小
/// - `name`: 要查找的名称
///
/// # 返回
/// 成功返回目录项，失败返回错误码
pub fn ext4_find_entry(dir_data: &[u8], block_size: usize, name: &str) -> Result<Ext4DirEntry, i32> {
    let iter = Ext4DirIterator::new(dir_data.to_vec(), block_size);

    for entry in iter {
        if entry.get_name() == name {
            return Ok(entry);
        }
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
}
