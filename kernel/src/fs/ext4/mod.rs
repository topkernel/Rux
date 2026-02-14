//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 文件系统
//!
//! 完全遵循 Linux 内核的 ext4 实现 (fs/ext4/, include/linux/ext4*)
//!
//! 核心概念：
//! - `struct ext4_super_block`: ext4 超级块
//! - `struct ext4_inode`: ext4 索引节点
//! - `struct ext4_group_desc`: 块组描述符
//! - `struct ext4_dir_entry`: 目录项
//!
//! 参考：Documentation/filesystems/ext4/

pub mod superblock;
pub mod inode;
pub mod dir;
pub mod file;
pub mod allocator;
pub mod indirect;
pub mod extent;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::errno;
use crate::drivers::blkdev;
use crate::fs::bio;
use crate::fs::superblock::{FileSystemType, FsContext, SuperBlock};

pub const EXT4_SUPER_MAGIC: u16 = 0xEF53;

pub struct Ext4FileSystem {
    /// 块设备
    pub device: *const blkdev::GenDisk,
    /// 超级块信息
    pub sb_info: Option<Box<superblock::Ext4SuperBlockInfo>>,
    /// 块组描述符表
    pub group_descs: Vec<Box<superblock::Ext4GroupDesc>>,
    /// 块大小
    pub block_size: u32,
    /// 块大小位数
    pub block_size_bits: u8,
    /// inode 大小
    pub inode_size: u16,
    /// 每组块数
    pub blocks_per_group: u32,
    /// 每组 inode 数
    pub inodes_per_group: u32,
    /// 块组数量
    pub group_count: u32,
    /// 总块数
    pub total_blocks: u64,
    /// 总 inode 数
    pub total_inodes: u32,
}

unsafe impl Send for Ext4FileSystem {}
unsafe impl Sync for Ext4FileSystem {}

impl Ext4FileSystem {
    /// 创建新的 ext4 文件系统实例
    pub fn new(device: *const blkdev::GenDisk) -> Self {
        Self {
            device,
            sb_info: None,
            group_descs: Vec::new(),
            block_size: 4096,
            block_size_bits: 12,
            inode_size: 256,
            blocks_per_group: 0,
            inodes_per_group: 0,
            group_count: 0,
            total_blocks: 0,
            total_inodes: 0,
        }
    }

    /// 初始化 ext4 文件系统
    ///
    /// 读取超级块和块组描述符
    pub fn init(&mut self) -> Result<(), i32> {
        unsafe {
            // 读取超级块
            // ext4 超级块位于字节偏移 1024 字节处
            // - 对于 1KB 块：超级块在块 1 的起始位置
            // - 对于 2KB+ 块：超级块在块 0 的偏移 1024 处
            // 由于我们使用 4KB 块缓存，读取块 0 并访问偏移 1024
            let sb_bh = bio::bread(self.device, 0)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let sb_data = &(*sb_bh).b_data;
            // 超级块在块内偏移 1024 字节处
            let ext4_sb = &*(sb_data.as_ptr().add(1024) as *const superblock::Ext4SuperBlockOnDisk);

            // 验证魔数
            if ext4_sb.s_magic != EXT4_SUPER_MAGIC {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }

            // 解析超级块
            let block_size = 1024 << ext4_sb.s_log_block_size;
            let block_size_bits = (12 + ext4_sb.s_log_block_size) as u8;
            let blocks_per_group = ext4_sb.s_blocks_per_group;
            let inodes_per_group = ext4_sb.s_inodes_per_group;
            let total_blocks = ext4_sb.s_blocks_count;
            let total_inodes = ext4_sb.s_inodes_count;
            let group_count = ((total_blocks as u64) + (blocks_per_group as u64) - 1) /
                (blocks_per_group as u64);

            // 读取块组描述符表
            // 块组描述符表从块 (block_size / 1024) + 1 开始
            let gd_start_block = if block_size == 1024 { 2 } else { 1 };
            let gds_per_block = block_size / core::mem::size_of::<superblock::Ext4GroupDesc>() as u32;
            let _gd_blocks = (group_count as u32 + gds_per_block - 1) / gds_per_block;

            let mut group_descs = Vec::new();

            for i in 0..group_count {
                let gd_block = gd_start_block + (i as u32 / gds_per_block);
                let gd_offset = (i as u32 % gds_per_block) as usize;

                let gd_bh = bio::bread(self.device, gd_block as u64)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let gd_data = &(*gd_bh).b_data;
                let gd_ptr = unsafe {
                    &*(gd_data.as_ptr().add(gd_offset * core::mem::size_of::<superblock::Ext4GroupDesc>())
                        as *const superblock::Ext4GroupDesc)
                };

                group_descs.push(Box::new(*gd_ptr));
                bio::brelse(gd_bh);
            }

            bio::brelse(sb_bh);

            // 更新文件系统信息
            self.sb_info = Some(Box::new(superblock::Ext4SuperBlockInfo {
                s_inodes_count: ext4_sb.s_inodes_count,
                s_blocks_count: ext4_sb.s_blocks_count as u64,
                s_r_blocks_count: ext4_sb.s_r_blocks_count as u64,
                s_free_blocks_count: ext4_sb.s_free_blocks_count as u64,
                s_free_inodes_count: ext4_sb.s_free_inodes_count,
                s_first_data_block: ext4_sb.s_first_data_block,
                s_log_block_size: ext4_sb.s_log_block_size,
                s_blocks_per_group: ext4_sb.s_blocks_per_group,
                s_inodes_per_group: ext4_sb.s_inodes_per_group,
            }));

            self.block_size = block_size;
            self.block_size_bits = block_size_bits;
            self.inode_size = ext4_sb.s_inode_size;
            self.blocks_per_group = blocks_per_group;
            self.inodes_per_group = inodes_per_group;
            self.group_count = group_count as u32;
            self.total_blocks = total_blocks as u64;
            self.total_inodes = total_inodes;
            self.group_descs = group_descs;

            Ok(())
        }
    }

    /// 读取 inode
    pub fn read_inode(&self, ino: u32) -> Result<inode::Ext4Inode, i32> {
        unsafe {
            // 计算块组和 inode 表索引
            let group = (ino - 1) / self.inodes_per_group;
            let index = (ino - 1) % self.inodes_per_group;

            if group as usize >= self.group_descs.len() {
                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
            }

            let gd = &self.group_descs[group as usize];

            // 计算 inode 块号
            let inode_table_start = gd.bg_inode_table;
            let inodes_per_block = self.block_size / (self.inode_size as u32);
            let inode_block = inode_table_start + (index / inodes_per_block);
            let inode_offset = ((index % inodes_per_block) * (self.inode_size as u32)) as usize;

            // 读取包含 inode 的块
            let bh = bio::bread(self.device, inode_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &(*bh).b_data;

            // 解析 inode
            let ext4_inode = &*(data.as_ptr().add(inode_offset) as *const inode::Ext4InodeOnDisk);

            let result = inode::Ext4Inode::from_disk(ext4_inode, ino);

            bio::brelse(bh);
            Ok(result)
        }
    }

    /// 获取根 inode
    pub fn get_root_inode(&self) -> Result<inode::Ext4Inode, i32> {
        // ext4 中根 inode 的编号总是 2
        self.read_inode(2)
    }

    /// 查找目录项
    pub fn lookup(&self, dir: &inode::Ext4Inode, name: &str) -> Result<dir::Ext4DirEntry, i32> {
        unsafe {
            // 遍历目录的数据块
            let blocks = dir.get_data_blocks(self)?;
            let _name_bytes = name.as_bytes();

            for block in blocks {
                let bh = bio::bread(self.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    if entry.inode == 0 {
                        offset += entry.rec_len as usize;
                        continue;
                    }

                    let entry_name = core::str::from_utf8_unchecked(&entry.name[..entry.name_len as usize]);

                    if entry_name == name {
                        bio::brelse(bh);
                        return Ok(entry);
                    }

                    offset += entry.rec_len as usize;
                }

                bio::brelse(bh);
            }

            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
    }
}

static EXT4_FS_TYPE: FileSystemType = FileSystemType::new(
    "ext4",
    Some(ext4_mount),
    Some(ext4_kill_sb),
    0,
);

unsafe extern "C" fn ext4_mount(fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    use crate::console::putchar;

    const MSG: &[u8] = b"ext4: mounting...\n";
    for &b in MSG {
        putchar(b);
    }

    // 获取源设备
    let _source = fc.source.ok_or(-2_i32)?;  // ENOENT

    // TODO: 从 source 获取块设备
    // 简化实现：假设设备已经注册
    // 这里需要实现设备名到设备的映射

    // 创建 ext4 文件系统实例
    let mut fs = Box::new(Ext4FileSystem::new(core::ptr::null()));

    // 初始化文件系统
    fs.init()?;

    // 创建 VFS 超级块
    let mut sb = Box::new(SuperBlock::new(fs.block_size as usize, EXT4_SUPER_MAGIC as u32));
    sb.set_type(&EXT4_FS_TYPE);
    sb.set_flags(crate::fs::superblock::SuperBlockFlags::new(
        crate::fs::superblock::SuperBlockFlags::SB_RDONLY,
    ));

    // 设置私有数据
    let fs_ptr = Box::into_raw(fs) as *mut u8;
    sb.set_fs_info(fs_ptr);

    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

unsafe extern "C" fn ext4_kill_sb(sb: *mut SuperBlock) {
    if let Some(fs_info) = (*sb).s_fs_info {
        let _fs = Box::from_raw(fs_info as *mut Ext4FileSystem);
        // Box 会自动释放
    }

    let _sb = Box::from_raw(sb);
    // Box 会自动释放
}

/// 从 ext4 文件系统读取整个文件
///
/// # 参数
/// - `device`: 块设备指针
/// - `path`: 文件路径（绝对路径，如 "/bin/sh"）
///
/// # 返回
/// - `Some(data)`: 文件内容
/// - `None`: 读取失败
pub fn read_file(device: *const blkdev::GenDisk, path: &str) -> Option<Vec<u8>> {
    use alloc::vec::Vec;

    unsafe {
        // 创建 ext4 文件系统实例
        let mut fs = Box::new(Ext4FileSystem::new(device));

        // 初始化文件系统
        if fs.init().is_err() {
            crate::println!("ext4: Failed to initialize filesystem");
            return None;
        }

        // 解析路径
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // 从根 inode 开始
        let mut current_inode = match fs.get_root_inode() {
            Ok(inode) => inode,
            Err(e) => {
                crate::println!("ext4: Failed to get root inode: {}", e);
                return None;
            }
        };

        // 遍历路径
        for part in path_parts.iter() {
            let entry = match fs.lookup(&current_inode, part) {
                Ok(e) => e,
                Err(_) => {
                    crate::println!("ext4: Entry '{}' not found", part);
                    return None;
                }
            };

            // 读取目标 inode
            current_inode = match fs.read_inode(entry.inode) {
                Ok(inode) => inode,
                Err(e) => {
                    crate::println!("ext4: Failed to read inode {}: {}", entry.inode, e);
                    return None;
                }
            };
        }

        // 读取文件内容
        let file_size = current_inode.get_size() as usize;
        if file_size == 0 {
            return Some(Vec::new());
        }

        let mut buffer = Vec::with_capacity(file_size);
        buffer.resize(file_size, 0);

        match file::ext4_file_read(&fs, &current_inode, 0, &mut buffer) {
            Ok(n) => {
                buffer.truncate(n);
                Some(buffer)
            }
            Err(_) => None,
        }
    }
}

pub fn init() {
    use crate::console::putchar;

    const MSG: &[u8] = b"ext4: initializing...\n";
    for &b in MSG {
        putchar(b);
    }

    // 注册文件系统类型
    crate::println!("ext4: Calling register_filesystem...");
    match crate::fs::superblock::register_filesystem(&EXT4_FS_TYPE) {
        Ok(_) => {
            crate::println!("ext4: register_filesystem returned Ok");
        }
        Err(_e) => {
            crate::println!("ext4: register_filesystem returned Err: {:?}", _e);
        }
    }

    crate::println!("ext4: Init continuing...");
}
