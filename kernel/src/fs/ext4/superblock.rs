//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 超级块和磁盘结构定义
//!
//! 完全遵循 Linux 内核的 ext4 超级块定义
//! 参考: fs/ext4/ext4.h, include/linux/ext4_fs.h

use core::mem;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4SuperBlockOnDisk {
    /// inode 数量
    pub s_inodes_count: u32,
    /// 块数量
    pub s_blocks_count: u32,
    /// 保留块数量
    pub s_r_blocks_count: u32,
    /// 空闲块数量
    pub s_free_blocks_count: u32,
    /// 空闲 inode 数量
    pub s_free_inodes_count: u32,
    /// 第一个数据块
    pub s_first_data_block: u32,
    /// 块大小（log2）
    pub s_log_block_size: u32,
    /// 片大小（log2）
    pub s_log_frag_size: u32,
    /// 每组块数
    pub s_blocks_per_group: u32,
    /// 每组片数
    pub s_frags_per_group: u32,
    /// 每组 inode 数
    pub s_inodes_per_group: u32,
    /// 挂载时间
    pub s_mtime: u32,
    /// 写入时间
    pub s_wtime: u32,
    /// 挂载次数
    pub s_mnt_count: u16,
    /// 最大挂载次数
    pub s_max_mnt_count: i16,
    /// 魔数（0xEF53）
    pub s_magic: u16,
    /// 状态
    pub s_state: u16,
    /// 错误处理
    pub s_errors: u16,
    /// 次版本
    pub s_minor_rev_level: u16,
    /// 最后检查时间
    pub s_lastcheck: u32,
    /// 检查间隔
    pub s_checkinterval: u32,
    /// 创建者 OS
    pub s_creator_os: u32,
    /// 版本号
    pub s_rev_level: u32,
    /// 保留的 UID
    pub s_def_resuid: u16,
    /// 保留的 GID
    pub s_def_resgid: u16,
    /// 第一个非保留 inode
    pub s_first_ino: u32,
    /// inode 大小
    pub s_inode_size: u16,
    /// 块组数量
    pub s_block_group_nr: u16,
    /// 特性兼容标志
    pub s_feature_compat: u32,
    /// 特性不兼容标志
    pub s_feature_incompat: u32,
    /// 只读兼容特性标志
    pub s_feature_ro_compat: u32,
    /// UUID
    pub s_uuid: [u8; 16],
    /// 卷名
    pub s_volume_name: [u8; 16],
    /// 最后挂载目录
    pub s_last_mounted: [u8; 64],
    /// 算法位图
    pub s_algorithm_usage_bitmap: u32,
    /// 预分配 inode 数
    pub s_prealloc_blocks: u8,
    /// 预分配目录数
    pub s_prealloc_dir_blocks: u8,
    /// 保留的 GDT 块
    pub s_reserved_gdt_blocks: u16,
    /// 日志 UUID
    pub s_journal_uuid: [u8; 16],
    /// 日志 inode 号
    pub s_journal_inum: u32,
    /// 日志设备
    pub s_journal_dev: u32,
    /// 最后 orphan inode 位置
    pub s_last_orphan: u32,
    /// 哈希种子
    pub s_hash_seed: [u32; 4],
    /// 默认哈希版本
    pub s_def_hash_version: u8,
    /// 日志备份类型
    pub s_jnl_backup_type: u8,
    /// 描述符大小
    pub s_desc_size: u16,
    /// 默认挂载选项
    pub s_default_mount_opts: u32,
    /// 第一 meta 块组
    pub s_first_meta_bg: u32,
    /// 文件系统创建时间
    pub s_mkfs_time: u32,
    /// 日志备份块
    pub s_jnl_blocks: [u32; 17],
    /// 4KB 以下的块数
    pub s_blocks_count_hi: u32,
    /// 4KB 以下的保留块数
    pub s_r_blocks_count_hi: u32,
    /// 4KB 以下的空闲块数
    pub s_free_blocks_count_hi: u32,
    /// 最少 extra inode 大小
    pub s_min_extra_isize: u16,
    /// 想要 extra inode 大小
    pub s_want_extra_isize: u16,
    /// 标志
    pub s_flags: u32,
    /// RAID stride
    pub s_raid_stride: u16,
    /// RAID stripe width
    pub s_raid_stripe_width: u32,
    /// 日志数据块组
    pub s_log_groups_per_flex: u8,
    /// 校验类型
    pub s_checksum_type: u8,
    /// 修复时间
    pub s_encryption_level: u8,
    /// 保留的 pads
    pub s_reserved_pad: u8,
    /// KB 使用的块数
    pub s_kbytes_written: u64,
    /// 快照 inode 号
    pub s_snapshot_inum: u32,
    /// 快照 ID
    pub s_snapshot_id: u32,
    /// 快照保留块
    pub s_snapshot_r_blocks_count: u64,
    /// 快照列表
    pub s_snapshot_list: u32,
    /// 错误位图位置
    pub s_error_count: u32,
    /// 错误首次时间
    pub s_first_error_time: u32,
    /// 错误首次 inode
    pub s_first_error_ino: u32,
    /// 错误首次块
    pub s_first_error_block: u64,
    /// 错误首次函数
    pub s_first_error_func: [u8; 32],
    /// 错误首次行
    pub s_first_error_line: u32,
    /// 错误最后时间
    pub s_last_error_time: u32,
    /// 错误最后 inode
    pub s_last_error_ino: u32,
    /// 错误最后块
    pub s_last_error_block: u64,
    /// 错误最后函数
    pub s_last_error_func: [u8; 32],
    /// 错误最后行
    pub s_last_error_line: u32,
    /// 挂载选项
    pub s_mount_opts: u64,
    /// 用户 quota inode
    pub s_usr_quota_inum: u32,
    /// 组 quota inode
    pub s_grp_quota_inum: u32,
    /// 缺失校验和计数
    pub s_overhead_clusters: u32,
    /// 备份超级块
    pub s_backup_bgs: [u32; 2],
    /// 加密算法
    pub s_encrypt_algos: [u8; 4],
    /// 加密密钥
    pub s_encrypt_pw_salt: [u8; 16],
    /// lninks 位置
    pub s_lpf_ino: u32,
    /// 项目 quota inode
    pub s_prj_quota_inum: u32,
    /// 校验和种子
    pub s_checksum_seed: u32,
    /// 特性
    pub s_wtime_hi: u32,
    /// inode 深度
    pub s_inode_bitmap_high: u64,
    /// inode 深度
    pub s_inode_table_high: u64,
    /// 保留
    pub s_reserved: [u32; 98],
}

impl Default for Ext4SuperBlockOnDisk {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4SuperBlockInfo {
    /// inode 总数
    pub s_inodes_count: u32,
    /// 块总数
    pub s_blocks_count: u64,
    /// 保留块总数
    pub s_r_blocks_count: u64,
    /// 空闲块总数
    pub s_free_blocks_count: u64,
    /// 空闲 inode 总数
    pub s_free_inodes_count: u32,
    /// 第一个数据块
    pub s_first_data_block: u32,
    /// 块大小（log2）
    pub s_log_block_size: u32,
    /// 每组块数
    pub s_blocks_per_group: u32,
    /// 每组 inode 数
    pub s_inodes_per_group: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4GroupDesc {
    /// 块位图块号
    pub bg_block_bitmap: u32,
    /// inode 位图块号
    pub bg_inode_bitmap: u32,
    /// inode 表起始块号
    pub bg_inode_table: u32,
    /// 空闲块数
    pub bg_free_blocks_count: u16,
    /// 空闲 inode 数
    pub bg_free_inodes_count: u16,
    /// 已用目录数
    pub bg_used_dirs_count: u16,
    /// 标志
    pub bg_flags: u16,
    /// 排除 bitmap 快照
    pub bg_exclude_bitmap_lo: u32,
    /// 块位图校验和
    pub bg_block_bitmap_csum_lo: u16,
    /// inode 位图校验和
    pub bg_inode_bitmap_csum_lo: u16,
    /// itable 未使用
    pub bg_itable_unused_lo: u16,
    /// 校验和
    pub bg_checksum: u16,
}

impl Default for Ext4GroupDesc {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
pub struct Ext4FsState {
    /// 特性兼容标志
    pub feature_compat: u32,
    /// 特性不兼容标志
    pub feature_incompat: u32,
    /// 只读兼容特性标志
    pub feature_ro_compat: u32,
    /// inode 大小
    pub inode_size: u16,
}

impl Ext4FsState {
    pub fn new() -> Self {
        Self {
            feature_compat: 0,
            feature_incompat: 0,
            feature_ro_compat: 0,
            inode_size: 256,
        }
    }

    /// 检查是否支持 64 位
    pub fn has_64bit(&self) -> bool {
        (self.feature_incompat & 0x80) != 0  // INCOMPAT_64BIT
    }

    /// 检查是否支持扩展
    pub fn has_extents(&self) -> bool {
        (self.feature_incompat & 0x40) != 0  // INCOMPAT_EXTENTS
    }

    /// 检查是否支持 flex 块组
    pub fn has_flex_bg(&self) -> bool {
        (self.feature_incompat & 0x200) != 0  // INCOMPAT_FLEX_BG
    }
}

impl Default for Ext4FsState {
    fn default() -> Self {
        Self::new()
    }
}
