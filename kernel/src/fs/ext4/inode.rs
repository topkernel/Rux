//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 inode 操作
//!
//! 完全遵循 Linux 内核的 ext4 inode 实现
//! 参考: fs/ext4/inode.c, include/linux/ext4_fs.h

use core::mem;
use alloc::vec::Vec;

use crate::errno;
use crate::fs::ext4::superblock;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4InodeOnDisk {
    /// 文件模式（类型和权限）
    pub i_mode: u16,
    /// 用户 ID
    pub i_uid: u16,
    /// 文件大小
    pub i_size: u32,
    /// 最后访问时间
    pub i_atime: u32,
    /// 最后 inode 修改时间
    pub i_ctime: u32,
    /// 最后数据修改时间
    pub i_mtime: u32,
    /// 删除时间
    pub i_dtime: u32,
    /// 组 ID
    pub i_gid: u16,
    /// 链接数
    pub i_links_count: u16,
    /// 块数
    pub i_blocks: u32,
    /// 标志
    pub i_flags: u32,
    /// OS 特定值 1
    pub osd1: u32,
    /// 直接块指针
    pub i_block: [u32; 15],
    /// 生成号
    pub i_generation: u32,
    /// 文件访问控制
    pub i_file_acl: u32,
    /// 文件访问控制（高）
    pub i_file_acl_high: u32,
    /// 目录 ACL
    pub i_dir_acl: u32,
    /// 块地址（高）
    pub i_dir_acl_high: u32,
    /// 碎片地址
    pub i_faddr: u32,
    /// OS 特定值 2
    pub osd2: [u8; 12],
    /// 额外 inode 大小
    pub i_extra_isize: u16,
    /// 校验和
    pub i_checksum: u16,
    /// ctime 扩展
    pub i_ctime_extra: u32,
    /// mtime 扩展
    pub i_mtime_extra: u32,
    /// atime 扩展
    pub i_atime_extra: u32,
    /// crtime（创建时间）
    pub i_crtime: u32,
    /// crtime 扩展
    pub i_crtime_extra: u32,
    /// 项目 ID
    pub i_projid: u32,
    /// 保留
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
    /// inode 编号
    pub ino: u32,
    /// 文件模式
    pub mode: u16,
    /// 用户 ID
    pub uid: u16,
    /// 组 ID
    pub gid: u16,
    /// 文件大小
    pub size: u64,
    /// 块数
    pub blocks: u64,
    /// 链接数
    pub links_count: u16,
    /// 标志
    pub flags: u32,
    /// 直接块指针
    pub block: [u32; 15],
    /// 最后访问时间
    pub atime: u32,
    /// 最后修改时间
    pub mtime: u32,
    /// 创建时间
    pub ctime: u32,
}

impl Ext4Inode {
    /// 从磁盘格式创建
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

    /// 检查是否是目录
    pub fn is_dir(&self) -> bool {
        (self.mode & 0xF000) == 0x4000
    }

    /// 检查是否是常规文件
    pub fn is_reg(&self) -> bool {
        (self.mode & 0xF000) == 0x8000
    }

    /// 检查是否是符号链接
    pub fn is_symlink(&self) -> bool {
        (self.mode & 0xF000) == 0xA000
    }

    /// 检查是否使用 extent
    pub fn has_extent(&self) -> bool {
        (self.flags & 0x80000) != 0
    }

    /// 获取文件大小
    pub fn get_size(&self) -> u64 {
        self.size
    }

    /// 设置文件大小
    pub fn set_size(&mut self, size: u64) {
        self.size = size;
    }

    /// 获取数据块列表
    ///
    /// 支持直接块和间接块（单级、二级、三级）
    pub fn get_data_blocks(&self, fs: &super::super::ext4::Ext4FileSystem) -> Result<Vec<u64>, i32> {
        let mut blocks = Vec::new();

        let remaining_blocks = (self.size + fs.block_size as u64 - 1) / (fs.block_size as u64);

        // 使用间接块模块获取所有数据块
        for i in 0..remaining_blocks {
            match super::indirect::ext4_get_block(fs, &self.block, i) {
                Ok(block_num) => {
                    if block_num != 0 {
                        blocks.push(block_num);
                    } else {
                        // 稀疏文件，块未分配
                        blocks.push(0);
                    }
                }
                Err(e) => return Err(e),
            }
        }

        Ok(blocks)
    }

    /// 获取指定块索引的数据块号
    ///
    /// 支持直接块和间接块
    pub fn get_data_block(&self, fs: &super::super::ext4::Ext4FileSystem, block_index: u64) -> Result<u64, i32> {
        super::indirect::ext4_get_block(fs, &self.block, block_index)
    }

    /// 读取文件数据
    ///
    /// 从指定偏移量读取数据
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
    /// 字符设备
    pub const S_IFCHR: u16 = 0o020000;
    /// 目录
    pub const S_IFDIR: u16 = 0o040000;
    /// 块设备
    pub const S_IFBLK: u16 = 0o060000;
    /// 常规文件
    pub const S_IFREG: u16 = 0o100000;
    /// 符号链接
    pub const S_IFLNK: u16 = 0o120000;
    /// Socket
    pub const S_IFSOCK: u16 = 0o140000;

    /// 文件类型掩码
    pub const S_IFMT: u16 = 0o170000;
}

pub mod perm {
    /// 所有者读
    pub const S_IRUSR: u16 = 0o400;
    /// 所有者写
    pub const S_IWUSR: u16 = 0o200;
    /// 所有者执行
    pub const S_IXUSR: u16 = 0o100;
    /// 组读
    pub const S_IRGRP: u16 = 0o040;
    /// 组写
    pub const S_IWGRP: u16 = 0o020;
    /// 组执行
    pub const S_IXGRP: u16 = 0o010;
    /// 其他读
    pub const S_IROTH: u16 = 0o004;
    /// 其他写
    pub const S_IWOTH: u16 = 0o002;
    /// 其他执行
    pub const S_IXOTH: u16 = 0o001;
}
