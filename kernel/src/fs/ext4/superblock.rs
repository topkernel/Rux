//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 superblock and disk structure definitions

use core::mem;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4SuperBlockOnDisk {
    /// inode count
    pub s_inodes_count: u32,
    /// block count
    pub s_blocks_count: u32,
    /// reserved block count
    pub s_r_blocks_count: u32,
    /// free block count
    pub s_free_blocks_count: u32,
    /// free inode count
    pub s_free_inodes_count: u32,
    /// first data block
    pub s_first_data_block: u32,
    /// block size (log2)
    pub s_log_block_size: u32,
    /// fragment size (log2)
    pub s_log_frag_size: u32,
    /// blocks per group
    pub s_blocks_per_group: u32,
    /// fragments per group
    pub s_frags_per_group: u32,
    /// inodes per group
    pub s_inodes_per_group: u32,
    /// mount time
    pub s_mtime: u32,
    /// write time
    pub s_wtime: u32,
    /// mount count
    pub s_mnt_count: u16,
    /// max mount count
    pub s_max_mnt_count: i16,
    /// magic number (0xEF53)
    pub s_magic: u16,
    /// state
    pub s_state: u16,
    /// error handling
    pub s_errors: u16,
    /// minor version
    pub s_minor_rev_level: u16,
    /// last check time
    pub s_lastcheck: u32,
    /// check interval
    pub s_checkinterval: u32,
    /// creator OS
    pub s_creator_os: u32,
    /// version number
    pub s_rev_level: u32,
    /// reserved UID
    pub s_def_resuid: u16,
    /// reserved GID
    pub s_def_resgid: u16,
    /// first non-reserved inode
    pub s_first_ino: u32,
    /// inode size
    pub s_inode_size: u16,
    /// block group number
    pub s_block_group_nr: u16,
    /// feature compatibility flags
    pub s_feature_compat: u32,
    /// feature incompatibility flags
    pub s_feature_incompat: u32,
    /// read-only compatibility feature flags
    pub s_feature_ro_compat: u32,
    /// UUID
    pub s_uuid: [u8; 16],
    /// volume name
    pub s_volume_name: [u8; 16],
    /// last mounted directory
    pub s_last_mounted: [u8; 64],
    /// algorithm bitmap
    pub s_algorithm_usage_bitmap: u32,
    /// preallocation inode count
    pub s_prealloc_blocks: u8,
    /// preallocation directory count
    pub s_prealloc_dir_blocks: u8,
    /// reserved GDT blocks
    pub s_reserved_gdt_blocks: u16,
    /// journal UUID
    pub s_journal_uuid: [u8; 16],
    /// journal inode number
    pub s_journal_inum: u32,
    /// journal device
    pub s_journal_dev: u32,
    /// last orphan inode position
    pub s_last_orphan: u32,
    /// hash seed
    pub s_hash_seed: [u32; 4],
    /// default hash version
    pub s_def_hash_version: u8,
    /// journal backup type
    pub s_jnl_backup_type: u8,
    /// descriptor size
    pub s_desc_size: u16,
    /// default mount options
    pub s_default_mount_opts: u32,
    /// first meta block group
    pub s_first_meta_bg: u32,
    /// filesystem creation time
    pub s_mkfs_time: u32,
    /// journal backup blocks
    pub s_jnl_blocks: [u32; 17],
    /// blocks count below 4KB
    pub s_blocks_count_hi: u32,
    /// reserved blocks count below 4KB
    pub s_r_blocks_count_hi: u32,
    /// free blocks count below 4KB
    pub s_free_blocks_count_hi: u32,
    /// minimum extra inode size
    pub s_min_extra_isize: u16,
    /// desired extra inode size
    pub s_want_extra_isize: u16,
    /// flags
    pub s_flags: u32,
    /// RAID stride
    pub s_raid_stride: u16,
    /// RAID stripe width
    pub s_raid_stripe_width: u32,
    /// journal data block group
    pub s_log_groups_per_flex: u8,
    /// checksum type
    pub s_checksum_type: u8,
    /// repair time
    pub s_encryption_level: u8,
    /// reserved pads
    pub s_reserved_pad: u8,
    /// KB written block count
    pub s_kbytes_written: u64,
    /// snapshot inode number
    pub s_snapshot_inum: u32,
    /// snapshot ID
    pub s_snapshot_id: u32,
    /// snapshot reserved blocks
    pub s_snapshot_r_blocks_count: u64,
    /// snapshot list
    pub s_snapshot_list: u32,
    /// error bitmap location
    pub s_error_count: u32,
    /// error first time
    pub s_first_error_time: u32,
    /// error first inode
    pub s_first_error_ino: u32,
    /// error first block
    pub s_first_error_block: u64,
    /// error first function
    pub s_first_error_func: [u8; 32],
    /// error first line
    pub s_first_error_line: u32,
    /// error last time
    pub s_last_error_time: u32,
    /// error last inode
    pub s_last_error_ino: u32,
    /// error last block
    pub s_last_error_block: u64,
    /// error last function
    pub s_last_error_func: [u8; 32],
    /// error last line
    pub s_last_error_line: u32,
    /// mount options
    pub s_mount_opts: u64,
    /// user quota inode
    pub s_usr_quota_inum: u32,
    /// group quota inode
    pub s_grp_quota_inum: u32,
    /// missing checksum count
    pub s_overhead_clusters: u32,
    /// backup superblock
    pub s_backup_bgs: [u32; 2],
    /// encryption algorithm
    pub s_encrypt_algos: [u8; 4],
    /// encryption key
    pub s_encrypt_pw_salt: [u8; 16],
    /// lninks location
    pub s_lpf_ino: u32,
    /// project quota inode
    pub s_prj_quota_inum: u32,
    /// checksum seed
    pub s_checksum_seed: u32,
    /// features
    pub s_wtime_hi: u32,
    /// inode depth
    pub s_inode_bitmap_high: u64,
    /// inode depth
    pub s_inode_table_high: u64,
    /// reserved
    pub s_reserved: [u32; 98],
}

impl Default for Ext4SuperBlockOnDisk {
    fn default() -> Self {
        // SAFETY: Ext4SuperBlockOnDisk is #[repr(C)] with all integer fields;
        // zeroed memory is a valid representation.
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4SuperBlockInfo {
    /// total inode count
    pub s_inodes_count: u32,
    /// total block count
    pub s_blocks_count: u64,
    /// total reserved block count
    pub s_r_blocks_count: u64,
    /// total free block count
    pub s_free_blocks_count: u64,
    /// total free inode count
    pub s_free_inodes_count: u32,
    /// first data block
    pub s_first_data_block: u32,
    /// block size (log2)
    pub s_log_block_size: u32,
    /// blocks per group
    pub s_blocks_per_group: u32,
    /// inodes per group
    pub s_inodes_per_group: u32,
    /// journal inode number
    pub s_journal_inum: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4GroupDesc {
    /// block bitmap block number
    pub bg_block_bitmap: u32,
    /// inode bitmap block number
    pub bg_inode_bitmap: u32,
    /// inode table start block number
    pub bg_inode_table: u32,
    /// free block count
    pub bg_free_blocks_count: u16,
    /// free inode count
    pub bg_free_inodes_count: u16,
    /// used directory count
    pub bg_used_dirs_count: u16,
    /// flags
    pub bg_flags: u16,
    /// exclude bitmap snapshot
    pub bg_exclude_bitmap_lo: u32,
    /// block bitmap checksum
    pub bg_block_bitmap_csum_lo: u16,
    /// inode bitmap checksum
    pub bg_inode_bitmap_csum_lo: u16,
    /// itable unused
    pub bg_itable_unused_lo: u16,
    /// checksum
    pub bg_checksum: u16,
}

impl Default for Ext4GroupDesc {
    fn default() -> Self {
        // SAFETY: Ext4GroupDesc is #[repr(C)] with all integer fields;
        // zeroed memory is a valid representation.
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
pub struct Ext4FsState {
    /// feature compatibility flags
    pub feature_compat: u32,
    /// feature incompatibility flags
    pub feature_incompat: u32,
    /// read-only compatibility feature flags
    pub feature_ro_compat: u32,
    /// inode size
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

    /// Check if 64-bit is supported
    pub fn has_64bit(&self) -> bool {
        (self.feature_incompat & 0x80) != 0  // INCOMPAT_64BIT
    }

    /// Check if extents are supported
    pub fn has_extents(&self) -> bool {
        (self.feature_incompat & 0x40) != 0  // INCOMPAT_EXTENTS
    }

    /// Check if flex block groups are supported
    pub fn has_flex_bg(&self) -> bool {
        (self.feature_incompat & 0x200) != 0  // INCOMPAT_FLEX_BG
    }
}

impl Default for Ext4FsState {
    fn default() -> Self {
        Self::new()
    }
}
