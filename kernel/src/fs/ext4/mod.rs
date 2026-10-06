//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 filesystem
//!
//!
//! Core concepts:
//! - `struct ext4_super_block`: ext4 superblock
//! - `struct ext4_inode`: ext4 inode
//! - `struct ext4_group_desc`: block group descriptor
//! - `struct ext4_dir_entry`: directory entry
//!
//! Reference: Documentation/filesystems/ext4/

pub mod superblock;
pub mod inode;
pub mod dir;
pub mod file;
pub mod allocator;
pub mod indirect;
pub mod extent;
pub mod namei;
pub mod journal;

use crate::sync::spinlock::Spinlock;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use crate::errno;
use crate::drivers::blkdev;
use crate::fs::bio;
use crate::fs::superblock::{FileSystemType, FsContext, SuperBlock};

pub const EXT4_SUPER_MAGIC: u16 = 0xEF53;

/// ext4 on-disk feature flags we can safely operate on (review 5.5 high:
/// 挂载不查 feature_incompat/ro_compat — bigalloc/metadata_csum were
/// silently accepted and then mis-handled, corrupting the image).
pub mod features {
    /// s_feature_incompat bits
    pub mod incompat {
        pub const COMPRESSION: u32 = 0x0001;
        pub const FILETYPE: u32 = 0x0002;
        pub const RECOVER: u32 = 0x0004;
        pub const JOURNAL_DEV: u32 = 0x0008;
        pub const META_BG: u32 = 0x0010;
        pub const EXTENTS: u32 = 0x0040;
        pub const B64BIT: u32 = 0x0080;
        pub const MMP: u32 = 0x0100;
        pub const FLEX_BG: u32 = 0x0200;
        pub const EA_INODE: u32 = 0x0400;
        pub const DIRDATA: u32 = 0x1000;
        pub const CSUM_SEED: u32 = 0x2000;
        pub const LARGEDIR: u32 = 0x4000;
        pub const INLINE_DATA: u32 = 0x8000;
        pub const ENCRYPT: u32 = 0x10000;
    }

    /// s_feature_ro_compat bits
    pub mod ro_compat {
        pub const SPARSE_SUPER: u32 = 0x0001;
        pub const LARGE_FILE: u32 = 0x0002;
        pub const BTREE_DIR: u32 = 0x0004;
        pub const HUGE_FILE: u32 = 0x0008;
        pub const GDT_CSUM: u32 = 0x0010;
        pub const DIR_NLINK: u32 = 0x0020;
        pub const EXTRA_ISIZE: u32 = 0x0040;
        pub const SNAPSHOT: u32 = 0x0080;
        pub const QUOTA: u32 = 0x0100;
        pub const BIGALLOC: u32 = 0x0200;
        pub const METADATA_CSUM: u32 = 0x0400;
        pub const REPLICA: u32 = 0x0800;
    }

    /// Incompatible features this implementation UNDERSTANDS. Any other
    /// incompat bit means on-disk structures we would misparse — refuse
    /// the mount (Linux does the same for unknown incompat features).
    pub const KNOWN_INCOMPAT: u32 = incompat::FILETYPE | incompat::RECOVER
        | incompat::EXTENTS | incompat::B64BIT | incompat::FLEX_BG;

    /// Read-only-compat features that are safe for our (write-through,
    /// no-checksum) metadata paths. Everything else — notably BIGALLOC
    /// (cluster-bitmap geometry) and METADATA_CSUM (checksummed metadata)
    /// — would be silently damaged by our writes: refuse the mount
    /// outright rather than corrupt.
    pub const KNOWN_RO_COMPAT: u32 = ro_compat::SPARSE_SUPER | ro_compat::LARGE_FILE
        | ro_compat::HUGE_FILE | ro_compat::DIR_NLINK | ro_compat::EXTRA_ISIZE;

    /// Incompatible features we know about but cannot handle correctly —
    /// folded into the "unknown" test: either way the mount is refused.
    pub const REJECTED_INCOMPAT: u32 = incompat::COMPRESSION | incompat::JOURNAL_DEV
        | incompat::META_BG | incompat::MMP | incompat::EA_INODE
        | incompat::DIRDATA | incompat::CSUM_SEED | incompat::LARGEDIR
        | incompat::INLINE_DATA | incompat::ENCRYPT;

    /// Per-directory flag: directory is htree-indexed (EXT4_INDEX_FL).
    /// Our directory code is linear-scan; READING an indexed directory
    /// still works (entries live in the linear blocks; block 0 holds the
    /// index nodes behind a spanning ".." rec_len), but WRITING would
    /// leave the index inconsistent — writers must refuse (EOPNOTSUPP).
    pub const EXT4_INDEX_FL: u32 = 0x1000;
}

pub struct Ext4FileSystem {
    /// Block device
    pub device: *const blkdev::GenDisk,
    /// Superblock information
    pub sb_info: Option<Box<superblock::Ext4SuperBlockInfo>>,
    /// Block group descriptor table (Mutex-protected for safe concurrent access)
    pub group_descs: Spinlock<Vec<Box<superblock::Ext4GroupDesc>>>,
    /// Block size
    pub block_size: u32,
    /// Block size bits
    pub block_size_bits: u8,
    /// Group descriptor size (32 or 64 bytes depending on 64-bit feature)
    pub desc_size: u16,
    /// Inode size
    pub inode_size: u16,
    /// Blocks per group
    pub blocks_per_group: u32,
    /// Inodes per group
    pub inodes_per_group: u32,
    /// Number of block groups
    pub group_count: u32,
    /// Total blocks
    pub total_blocks: u64,
    /// Total inodes
    pub total_inodes: u32,
    /// Journal inode number (typically 8)
    pub journal_ino: u32,
    /// JBD2 journal (initialized during mount)
    pub journal: Option<alloc::sync::Arc<crate::fs::jbd2::Journal>>,
    /// Block preallocation state (for mballoc)
    pub prealloc: Spinlock<Option<crate::fs::ext4::allocator::PreallocState>>,
}

// SAFETY: Ext4FileSystem is only accessed via &self methods that use bio layer
// locking; device is a raw pointer but only used for I/O through the bio cache.
unsafe impl Send for Ext4FileSystem {}
// SAFETY: all mutable access goes through bio::bread/brelse which uses internal
// locking; device pointer is read-only after initialization.
unsafe impl Sync for Ext4FileSystem {}

impl Ext4FileSystem {
    /// Create new ext4 filesystem instance
    pub fn new(device: *const blkdev::GenDisk) -> Self {
        Self {
            device,
            sb_info: None,
            group_descs: Spinlock::new(Vec::new()),
            block_size: 4096,
            block_size_bits: 12,
            desc_size: 32,  // Default, will be updated from superblock
            inode_size: 256,
            blocks_per_group: 0,
            inodes_per_group: 0,
            group_count: 0,
            total_blocks: 0,
            total_inodes: 0,
            journal_ino: 0,
            journal: None,
            prealloc: Spinlock::new(None),
        }
    }

    /// Get group descriptor (read-only)
    pub fn get_group_desc(&self, group: usize) -> Option<superblock::Ext4GroupDesc> {
        let descs = self.group_descs.lock();
        descs.get(group).map(|b| **b)
    }

    /// Get mutable access to group descriptor free blocks count
    pub fn dec_group_free_blocks(&self, group: usize) {
        let mut descs = self.group_descs.lock();
        if group < descs.len() && descs[group].bg_free_blocks_count_lo > 0 {
            descs[group].bg_free_blocks_count_lo -= 1;
        }
    }

    /// Increment group free blocks count
    pub fn inc_group_free_blocks(&self, group: usize) {
        let mut descs = self.group_descs.lock();
        if group < descs.len() {
            descs[group].bg_free_blocks_count_lo = descs[group].bg_free_blocks_count_lo.saturating_add(1);
        }
    }

    /// Get number of group descriptors
    pub fn group_descs_len(&self) -> usize {
        self.group_descs.lock().len()
    }

    /// Initialize ext4 filesystem
    ///
    /// Read superblock and block group descriptors
    pub fn init(&mut self) -> Result<(), i32> {
        // SAFETY: self.device is a valid GenDisk pointer set during filesystem creation;
        // bio::bread reads a block and returns a valid BufferHead, and the superblock
        // is at a known fixed offset (1024 bytes into block 0).
        unsafe {
            // Read superblock
            // ext4 superblock is at byte offset 1024
            // - For 1KB blocks: superblock at start of block 1
            // - For 2KB+ blocks: superblock at offset 1024 within block 0
            // Since we use 4KB block cache, read block 0 and access offset 1024
            let sb_bh = bio::bread(self.device, 0)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let sb_data = &(*sb_bh).b_data;
            // Superblock is at 1024 byte offset within block
            let ext4_sb = &*(sb_data.as_ptr().add(1024) as *const superblock::Ext4SuperBlockOnDisk);

            // Verify magic number
            if ext4_sb.s_magic != EXT4_SUPER_MAGIC {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }

            // ------------------------------------------------------------------
            // Feature negotiation (review 5.5 high). Before trusting any
            // on-disk structure, verify the feature sets are within what
            // this implementation handles. Unknown/unsupported features
            // fail the mount with a loud, specific message instead of a
            // silent mis-parse that corrupts the image.
            // ------------------------------------------------------------------
            {
                let incompat = ext4_sb.s_feature_incompat;
                let ro_compat = ext4_sb.s_feature_ro_compat;

                let unknown_incompat = incompat & !(features::KNOWN_INCOMPAT | features::REJECTED_INCOMPAT);
                if unknown_incompat != 0 {
                    crate::pr_err!("ext4: unknown incompat features {:#x} — refusing mount", unknown_incompat);
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }
                let rejected = incompat & features::REJECTED_INCOMPAT;
                if rejected != 0 {
                    crate::pr_err!("ext4: unsupported incompat features {:#x} (inline_data/meta_bg/encrypt/...) — refusing mount", rejected);
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }

                let unknown_ro = ro_compat
                    & !(features::KNOWN_RO_COMPAT
                        | features::ro_compat::BIGALLOC | features::ro_compat::METADATA_CSUM
                        | features::ro_compat::REPLICA | features::ro_compat::QUOTA
                        | features::ro_compat::SNAPSHOT | features::ro_compat::GDT_CSUM
                        | features::ro_compat::BTREE_DIR);
                if unknown_ro != 0 {
                    crate::pr_err!("ext4: unknown ro_compat features {:#x} — refusing mount", unknown_ro);
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }
                // We mount read-write and do not maintain ANY metadata
                // checksums or cluster geometry — accepting these features
                // would write checksum-invalid or cluster-misaligned
                // metadata. Reject explicitly (review: metadata_csum 直接拒绝挂载).
                if ro_compat & features::ro_compat::METADATA_CSUM != 0 {
                    crate::pr_err!("ext4: metadata_csum not supported — refusing mount");
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }
                if ro_compat & features::ro_compat::BIGALLOC != 0 {
                    crate::pr_err!("ext4: bigalloc not supported — refusing mount");
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }
            }

            // Parse superblock
            if ext4_sb.s_log_block_size >= 32 {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }
            let block_size = 1024 << ext4_sb.s_log_block_size;
            let block_size_bits = (12 + ext4_sb.s_log_block_size) as u8;

            // Validate on-disk geometry before using any of it in divisions
            // or allocations: a corrupted or malicious superblock must fail
            // the mount with EIO, never panic (division by zero) or exhaust
            // the kernel heap with a bogus group table.
            //
            // The bio buffer cache is fixed at 4096-byte blocks, so only
            // 4096-byte filesystems can be mounted correctly; other legal
            // ext4 geometries would read/write the wrong disk locations.
            if block_size != 4096 {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }
            let blocks_per_group = ext4_sb.s_blocks_per_group;
            let inodes_per_group = ext4_sb.s_inodes_per_group;
            if blocks_per_group == 0
                || blocks_per_group as u64 > 8 * block_size as u64
                || inodes_per_group == 0
                || ext4_sb.s_inode_size < 128
                || ext4_sb.s_inode_size as u32 > block_size
            {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }

            // Compute 64-bit block count: if INCOMPAT_64BIT is set, use hi+lo; otherwise lo only
            let is_64bit = (ext4_sb.s_feature_incompat & 0x80) != 0;
            let total_blocks = if is_64bit {
                (ext4_sb.s_blocks_count as u64) | ((ext4_sb.s_blocks_count_hi as u64) << 32)
            } else {
                ext4_sb.s_blocks_count as u64
            };

            // Bound the block count by the actual device size so a forged
            // s_blocks_count cannot drive a huge descriptor allocation.
            let device_blocks = (*self.device).get_capacity()
                .saturating_mul(512) / block_size as u64;
            if total_blocks == 0
                || (device_blocks > 0 && total_blocks > device_blocks)
                || (device_blocks == 0 && total_blocks > u32::MAX as u64)
            {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }

            let total_inodes = ext4_sb.s_inodes_count;
            let group_count = ((total_blocks as u64) + (blocks_per_group as u64) - 1) /
                (blocks_per_group as u64);

            // Get descriptor size - use actual size from superblock if 64-bit feature is enabled
            // Default is 32 bytes, but with 64-bit feature it's 64 bytes
            let desc_size = if ext4_sb.s_desc_size < 32 { 32 } else { ext4_sb.s_desc_size as usize };
            if desc_size > block_size as usize {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }

            // Read block group descriptor table
            // Block group descriptor table starts at block (block_size / 1024) + 1
            let gd_start_block = if block_size == 1024 { 2 } else { 1 };
            let gds_per_block = block_size as usize / desc_size;

            let mut group_descs = Vec::new();

            for i in 0..group_count {
                let gd_block = gd_start_block + (i as usize / gds_per_block) as u32;
                let gd_index = i as usize % gds_per_block;

                let gd_bh = bio::bread(self.device, gd_block as u64)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let gd_data = &(*gd_bh).b_data;
                // Use actual descriptor size for offset calculation
                let gd_offset = gd_index * desc_size;

                // Read descriptor safely based on desc_size.
                // If desc_size < size_of::<Ext4GroupDesc>(), only read what's
                // available — high 64-bit fields default to zero.
                let mut gd = superblock::Ext4GroupDesc::default();
                let copy_len = desc_size.min(core::mem::size_of::<superblock::Ext4GroupDesc>());
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        gd_data.as_ptr().add(gd_offset),
                        &mut gd as *mut superblock::Ext4GroupDesc as *mut u8,
                        copy_len,
                    );
                }

                group_descs.push(Box::new(gd));
                bio::brelse(gd_bh);
            }

            bio::brelse(sb_bh);

            // Update filesystem information
            self.sb_info = Some(Box::new(superblock::Ext4SuperBlockInfo {
                s_inodes_count: ext4_sb.s_inodes_count,
                s_blocks_count: if is_64bit {
                    (ext4_sb.s_blocks_count as u64) | ((ext4_sb.s_blocks_count_hi as u64) << 32)
                } else {
                    ext4_sb.s_blocks_count as u64
                },
                s_r_blocks_count: if is_64bit {
                    (ext4_sb.s_r_blocks_count as u64) | ((ext4_sb.s_r_blocks_count_hi as u64) << 32)
                } else {
                    ext4_sb.s_r_blocks_count as u64
                },
                s_free_blocks_count: if is_64bit {
                    (ext4_sb.s_free_blocks_count as u64) | ((ext4_sb.s_free_blocks_count_hi as u64) << 32)
                } else {
                    ext4_sb.s_free_blocks_count as u64
                },
                s_free_inodes_count: ext4_sb.s_free_inodes_count,
                s_first_data_block: ext4_sb.s_first_data_block,
                s_log_block_size: ext4_sb.s_log_block_size,
                s_blocks_per_group: ext4_sb.s_blocks_per_group,
                s_inodes_per_group: ext4_sb.s_inodes_per_group,
                s_journal_inum: ext4_sb.s_journal_inum,
            }));

            self.block_size = block_size;
            self.block_size_bits = block_size_bits;
            self.desc_size = desc_size as u16;
            self.inode_size = ext4_sb.s_inode_size;
            self.blocks_per_group = blocks_per_group;
            self.inodes_per_group = inodes_per_group;
            self.group_count = group_count as u32;
            self.total_blocks = total_blocks as u64;
            self.total_inodes = total_inodes;
            self.journal_ino = ext4_sb.s_journal_inum;
            *self.group_descs.lock() = group_descs;

            Ok(())
        }
    }

    /// Read inode
    pub fn read_inode(&self, ino: u32) -> Result<inode::Ext4Inode, i32> {
        if ino == 0 {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }
        if ino > self.total_inodes {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        // Calculate block group and inode table index
        let group = (ino - 1) / self.inodes_per_group;
        let index = (ino - 1) % self.inodes_per_group;

        let gd = {
            let group_descs = self.group_descs.lock();
            if group as usize >= group_descs.len() {
                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
            }
            *group_descs[group as usize]
        };

        // Calculate inode block number. Bounds-check the offset against
        // the block: reading a 172-byte on-disk inode at the last slot of
        // a block must stay inside the buffer (review EXT4-M1).
        let inode_table_start = gd.bg_inode_table_lo as u64;
        let inodes_per_block = self.block_size / (self.inode_size as u32);
        if inodes_per_block == 0 {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let inode_block = inode_table_start + (index / inodes_per_block) as u64;
        let inode_offset = ((index % inodes_per_block) * (self.inode_size as u32)) as usize;

        // Read block containing inode
        let bh = bio::bread(self.device, inode_block as u64)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = unsafe { &(*bh).b_data };

        // Bounds check: reading a 172-byte on-disk inode at this offset
        // must stay inside the 4096-byte block (review EXT4-M1).
        if inode_offset + core::mem::size_of::<inode::Ext4InodeOnDisk>() > data.len() {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        // Parse inode
        let ext4_inode = unsafe {
            &*(data.as_ptr().add(inode_offset) as *const inode::Ext4InodeOnDisk)
        };

        let result = inode::Ext4Inode::from_disk(ext4_inode, ino);

        bio::brelse(bh);
        Ok(result)
    }

    /// Get root inode
    pub fn get_root_inode(&self) -> Result<inode::Ext4Inode, i32> {
        // Root inode number in ext4 is always 2
        self.read_inode(2)
    }

    /// Lookup directory entry
    pub fn lookup(&self, dir: &inode::Ext4Inode, name: &str) -> Result<dir::Ext4DirEntry, i32> {
        // SAFETY: self.device is a valid GenDisk pointer; dir.get_data_blocks returns
        // valid block numbers; bio::bread returns a valid BufferHead for each block.
        unsafe {
            // Traverse directory's data blocks
            let blocks = dir.get_data_blocks(self)?;

            for block in blocks.iter() {
                if *block == 0 {
                    continue;
                }
                let bh = bio::bread(self.device, *block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    // Guard against corrupted directory entries (rec_len == 0)
                    if entry.rec_len == 0 {
                        break;
                    }

                    if entry.inode == 0 {
                        offset += entry.rec_len as usize;
                        continue;
                    }

                    let entry_name = match core::str::from_utf8(&entry.name[..entry.name_len as usize]) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };

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

    /// List directory contents
    ///
    /// # Arguments
    /// - `dir`: Directory inode
    ///
    /// # Returns
    /// List of directory entries
    pub fn list_dir(&self, dir: &inode::Ext4Inode) -> Result<Vec<dir::Ext4DirEntry>, i32> {
        // SAFETY: self.device is a valid GenDisk pointer; dir.get_data_blocks returns
        // valid block numbers; bio::bread returns valid BufferHeads for directory blocks.
        unsafe {
            let mut entries = Vec::new();

            // Traverse directory's data blocks
            let blocks = dir.get_data_blocks(self)?;

            for block in blocks.iter() {
                if *block == 0 {
                    continue;
                }
                let bh = bio::bread(self.device, *block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    // Guard against corrupted directory entries (rec_len == 0)
                    if entry.rec_len == 0 {
                        break;
                    }

                    if entry.inode == 0 {
                        offset += entry.rec_len as usize;
                        continue;
                    }

                    // Skip . and ..
                    let name = match core::str::from_utf8(&entry.name[..entry.name_len as usize]) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if name != "." && name != ".." {
                        entries.push(entry.clone());
                    }

                    offset += entry.rec_len as usize;
                }

                bio::brelse(bh);
            }

            Ok(entries)
        }
    }

    /// Read symbolic link target path
    ///
    /// # Arguments
    /// - `inode`: Symbolic link inode
    ///
    /// # Returns
    /// Symbolic link target path
    fn read_symlink_target(&self, inode: &inode::Ext4Inode) -> Result<String, i32> {
        let size = inode.get_size() as usize;

        // Fast symlink: target stored in block array (<= 60 bytes)
        if size <= 60 {
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    &inode.block[0] as *const _ as *const u8,
                    size
                )
            };
            return Ok(String::from_utf8_lossy(bytes).into_owned());
        }

        // Slow symlink: target stored in data blocks
        let blocks = inode.get_data_blocks(self)?;
        let mut target = String::new();

        for block in blocks {
            // SAFETY: self.device is a valid GenDisk pointer; block numbers come from
            // get_data_blocks; bio::bread returns a valid BufferHead.
            unsafe {
                let bh = bio::bread(self.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;
                let data = &(*bh).b_data;

                let remaining = size - target.len();
                let to_read = core::cmp::min(remaining, self.block_size as usize);

                let bytes = &data[..to_read];
                target.push_str(&String::from_utf8_lossy(bytes));

                bio::brelse(bh);

                if target.len() >= size {
                    break;
                }
            }
        }

        Ok(target)
    }

    /// Lookup inode by path (following symbolic links)
    ///
    /// # Arguments
    /// - `path`: File path (absolute path, e.g. "/bin/sh")
    ///
    /// # Returns
    /// Inode number and inode structure
    pub fn lookup_path(&self, path: &str) -> Result<(u32, inode::Ext4Inode), i32> {
        self.lookup_path_internal(path, 0)
    }

    /// Internal path lookup implementation (with symlink depth limit to prevent loops)
    fn lookup_path_internal(&self, path: &str, symlink_depth: u32) -> Result<(u32, inode::Ext4Inode), i32> {
        // Max symlink depth from config
        const MAX_SYMLINK_DEPTH: u32 = crate::config::EXT4_MAX_SYMLINK_DEPTH as u32;

        if symlink_depth > MAX_SYMLINK_DEPTH {
            return Err(errno::Errno::TooManySymbolicLinks.as_neg_i32());
        }

        // Parse path - filter out empty strings and "." (current directory)
        // Note: ".." handling would require parent tracking, not implemented yet
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();

        // Start from root inode
        let mut current_inode = self.get_root_inode()?;
        let mut current_ino = 2u32; // Root inode number

        // Traverse path
        for (idx, part) in path_parts.iter().enumerate() {
            let entry = self.lookup(&current_inode, *part)?;

            // Read next level inode
            current_ino = entry.inode;
            current_inode = self.read_inode(entry.inode)?;

            // If it's a symbolic link, follow it
            if current_inode.is_symlink() {
                let target = self.read_symlink_target(&current_inode)?;

                // Build remaining path
                let remaining: Vec<&str> = path_parts[idx + 1..].to_vec();

                // Build full target path
                let full_target = if target.starts_with('/') {
                    // Absolute path
                    if remaining.is_empty() {
                        target
                    } else {
                        let mut t = target;
                        for r in remaining {
                            t.push('/');
                            t.push_str(r);
                        }
                        t
                    }
                } else {
                    // Relative path - relative to current directory
                    let mut base_parts: Vec<&str> = path_parts[..idx].to_vec();
                    let target_parts: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();

                    for tp in target_parts {
                        if tp == ".." {
                            base_parts.pop();
                        } else if tp != "." {
                            base_parts.push(tp);
                        }
                    }

                    // Add remaining path
                    for r in remaining {
                        base_parts.push(r);
                    }

                    let mut result = String::new();
                    for p in base_parts {
                        result.push('/');
                        result.push_str(p);
                    }
                    if result.is_empty() {
                        result.push('/');
                    }
                    result
                };

                // Recursively lookup target path
                return self.lookup_path_internal(&full_target, symlink_depth + 1);
            }
        }

        Ok((current_ino, current_inode))
    }
}

static EXT4_FS_TYPE: FileSystemType = FileSystemType::new(
    "ext4",
    Some(ext4_mount),
    Some(ext4_kill_sb),
    0,
);

/// Mount an ext4 filesystem. Called from VFS mount path.
// SAFETY: fc is a valid FsContext reference from the VFS mount call;
/// the returned SuperBlock pointer is valid for the mount lifetime.
unsafe extern "C" fn ext4_mount(fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    crate::pr_info!("ext4: mounting...");

    // Get source device
    let _source = fc.source.ok_or(-2_i32)?;  // ENOENT

    // TODO: Get block device from source
    // Simplified implementation: assume device is already registered
    // Need to implement device name to device mapping

    // Create ext4 filesystem instance
    let mut fs = Box::new(Ext4FileSystem::new(core::ptr::null()));

    // Initialize filesystem
    fs.init()?;

    // Create VFS superblock
    let mut sb = Box::new(SuperBlock::new(fs.block_size as usize, EXT4_SUPER_MAGIC as u32));
    sb.set_type(&EXT4_FS_TYPE);
    sb.set_flags(crate::fs::superblock::SuperBlockFlags::new(
        crate::fs::superblock::SuperBlockFlags::SB_RDONLY,
    ));

    // Set private data
    let fs_ptr = Box::into_raw(fs) as *mut u8;
    sb.set_fs_info(fs_ptr);

    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

/// Kill an ext4 superblock during unmount. Called from VFS umount path.
// SAFETY: sb is a valid SuperBlock pointer from a previous ext4_mount call;
// s_fs_info contains a valid Ext4FileSystem pointer from Box::into_raw.
unsafe extern "C" fn ext4_kill_sb(sb: *mut SuperBlock) {
    if let Some(fs_info) = (*sb).s_fs_info {
        let _fs = Box::from_raw(fs_info as *mut Ext4FileSystem);
        // Box will be automatically freed
    }

    let _sb = Box::from_raw(sb);
    // Box will be automatically freed
}

/// Read entire file from ext4 filesystem (supports symbolic links)
///
/// # Parameters
/// - `device`: Block device pointer
/// - `path`: File path (absolute path, e.g. "/bin/sh")
///
/// # Returns
/// - `Some(data)`: File content
/// - `None`: Read failed
pub fn read_file(device: *const blkdev::GenDisk, path: &str) -> Option<Vec<u8>> {
    read_file_internal(device, path, 0)
}

/// Internal implementation, supports recursion depth limit to prevent circular symbolic links
fn read_file_internal(device: *const blkdev::GenDisk, path: &str, depth: u32) -> Option<Vec<u8>> {
    use alloc::vec::Vec;

    // Prevent circular symbolic links, max recursion depth 8
    if depth > 8 {
        return None;
    }

    // SAFETY: device is a valid GenDisk pointer from the block device layer;
    // Box::into_raw leaks the Box to make a static-lifetime pointer stored in
    // GLOBAL_EXT4_FS; this is called once during mount and cleaned up on unmount.
    unsafe {
        // Create ext4 filesystem instance
        let mut fs = Box::new(Ext4FileSystem::new(device));

        // Initialize filesystem
        if fs.init().is_err() {
            return None;
        }
        // Parse path - filter out empty strings and "." (current directory)
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();

        // Start from root inode
        let mut current_inode = match fs.get_root_inode() {
            Ok(inode) => inode,
            Err(_) => {
                return None;
            }
        };

        // Record current path directory part (for resolving relative path symbolic links)
        let mut current_dir_parts: Vec<&str> = Vec::new();

        // Traverse path
        for part in path_parts.iter() {
            let entry = match fs.lookup(&current_inode, part) {
                Ok(e) => e,
                Err(_) => {
                    return None;
                }
            };

            // Read target inode
            let target_inode = match fs.read_inode(entry.inode) {
                Ok(inode) => inode,
                Err(_) => {
                    return None;
                }
            };

            // Check if it's a symbolic link
            if target_inode.is_symlink() {
                // Read symbolic link target
                let link_target = read_symlink_target(&fs, &target_inode)?;

                // Build the REPLACEMENT path: the symlink target takes the
                // place of THIS component; remaining components must be
                // preserved (the old code replaced the WHOLE path, losing
                // everything after the link: "/bin/dash" with bin->usr/bin
                // resolved to "/usr/bin" instead of "/usr/bin/dash" —
                // returning the DIRECTORY's 8192 bytes as "file data").
                let part_index = path_parts.iter()
                    .position(|p| core::ptr::eq(p, part))
                    .unwrap_or(0);
                let mut resolved_path = if link_target.starts_with('/') {
                    link_target
                } else {
                    // Relative: prepend the walked directory components
                    let mut r = String::from("/");
                    for dir_part in &current_dir_parts {
                        r.push_str(dir_part);
                        r.push('/');
                    }
                    r.push_str(&link_target);
                    r
                };
                for remaining in path_parts.iter().skip(part_index + 1) {
                    resolved_path.push('/');
                    resolved_path.push_str(remaining);
                }

                // Recursively read target file
                return read_file_internal(device, &resolved_path, depth + 1);
            }

            // If it's a directory, update current directory path
            if target_inode.is_dir() {
                current_dir_parts.push(part);
            }

            current_inode = target_inode;
        }

        // Read file content
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

/// Read symbolic link target
///
/// ext4 symbolic link target storage methods:
/// - Short links (<= 60 bytes): stored in inode's block array
/// - Long links: stored in data blocks
fn read_symlink_target(fs: &Ext4FileSystem, inode: &inode::Ext4Inode) -> Option<String> {
    let size = inode.get_size() as usize;
    if size == 0 || size > 4096 {
        return None;
    }

    let mut buffer = alloc::vec![0u8; size];

    // Short symbolic link: data stored in block array (inline data)
    // ext4 short symbolic link threshold is usually 60 bytes
    if size <= 60 && !inode.has_extent() {
        // Read directly from block array
        let block_data = unsafe {
            core::slice::from_raw_parts(inode.block.as_ptr() as *const u8, 60)
        };
        buffer[..size].copy_from_slice(&block_data[..size]);
    } else {
        // Long symbolic link: read from data block
        match inode.read_data(fs, 0, &mut buffer) {
            Ok(n) if n == size => {}
            _ => return None,
        }
    }

    // Convert to string
    String::from_utf8(buffer).ok()
}

pub fn init() {
    // Register filesystem type
    let _ = crate::fs::superblock::register_filesystem(&EXT4_FS_TYPE);
}

/// Global ext4 filesystem instance
pub static GLOBAL_EXT4_FS: core::sync::atomic::AtomicPtr<Ext4FileSystem> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Mount ext4 filesystem
///
/// # Parameters
/// - `device`: Block device pointer
///
/// # Returns
/// - `Ok(())`: Mount successful
/// - `Err(code)`: Mount failed
pub fn mount_ext4(device: *const blkdev::GenDisk) -> Result<(), i32> {
    use core::sync::atomic::Ordering;

    if device.is_null() {
        return Err(-22); // EINVAL
    }

    // Create ext4 filesystem instance
    let mut fs = Box::new(Ext4FileSystem::new(device));

    // Initialize filesystem
    fs.init()?;

    // Initialize journal (gracefully skips if no journal)
    if let Err(e) = fs.init_journal() {
        crate::pr_debug!("ext4: journal init failed: {}", e);
        let _ = e;
    }

    // Save to global variable
    let fs_ptr = Box::into_raw(fs);
    GLOBAL_EXT4_FS.store(fs_ptr, Ordering::Release);

    Ok(())
}

/// Get mounted ext4 filesystem
pub fn get_ext4_fs() -> Option<*mut Ext4FileSystem> {
    use core::sync::atomic::Ordering;
    let ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// List directory contents from mounted ext4
///
/// # Parameters
/// - `path`: Directory path (absolute or relative path, e.g. "/bin" or ".")
///
/// # Returns
/// Resolve path to absolute path
/// Supports relative paths and current working directory
/// Always normalizes the path (handles . and ..)
fn resolve_path(path: &str) -> String {
    // Get absolute path
    let abs_path = if path.starts_with('/') {
        // Already absolute path
        String::from(path)
    } else {
        // Get current working directory
        let cwd = if let Some(current) = crate::sched::current() {
            let cwd_bytes = unsafe { (*current).get_cwd() };
            match core::str::from_utf8(&cwd_bytes) {
                Ok(s) => String::from(s),
                Err(_) => String::from("/"),
            }
        } else {
            String::from("/")
        };

        // Build full path
        let mut full_path = String::new();
        full_path.push_str(&cwd);
        if !cwd.ends_with('/') {
            full_path.push('/');
        }
        full_path.push_str(path);
        full_path
    };

    // Always normalize the path (handles . and ..)
    normalize_path(&abs_path)
}

/// Normalize path (handle . and ..)
fn normalize_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => {
                // Ignore empty parts and current directory
            }
            ".." => {
                // Go up one directory level
                components.pop();
            }
            _ => {
                components.push(part);
            }
        }
    }

    if components.is_empty() {
        String::from("/")
    } else {
        let mut result = String::new();
        for part in components {
            result.push('/');
            result.push_str(part);
        }
        result
    }
}

/// Check if ext4 is mounted
pub fn is_mounted() -> bool {
    use core::sync::atomic::Ordering;
    !GLOBAL_EXT4_FS.load(Ordering::Acquire).is_null()
}

/// P1 shutdown cascade: flush and mark the ext4 root filesystem clean.
///
/// Minimal umount semantics:
/// 1. flush the whole buffer cache (all dirty block buffers hit the disk),
/// 2. rewrite the superblock's s_state to EXT4_VALID_FS (clean) through the
///    same buffer cache and sync that block,
/// 3. persist the device's volatile write cache.
///
/// The filesystem stays mounted in memory (full teardown would need to
/// evict every inode/dentry reference); on the power-off path nothing runs
/// afterwards anyway, and on the restart path the fresh boot re-reads the
/// on-disk superblock and sees a clean unmount.
pub fn ext4_shutdown_sync() -> Result<(), i32> {
    use core::sync::atomic::Ordering;

    // 1. All dirty data/metadata buffers.
    crate::fs::bio::sync_buffers()?;

    // 2. Superblock clean marking (block 0, byte offset 1024 + 58).
    let fs_ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
    if fs_ptr.is_null() {
        return Ok(()); // no ext4 root — nothing to mark
    }
    // SAFETY: fs_ptr was set by mount_ext4 (Box::into_raw) and the fs is
    // still mounted; only the device pointer and block size are read.
    unsafe {
        let device = (*fs_ptr).device;
        if let Some(bh) = crate::fs::bio::bread(device, 0) {
            // s_state offset within the on-disk superblock: 13 u32 fields
            // (52) + s_mnt_count(2) + s_max_mnt_count(2) + s_magic(2) = 58.
            const SB_OFF: usize = 1024;
            const S_STATE_OFF: usize = SB_OFF + 58;
            const EXT4_VALID_FS: u16 = 1;
            (*bh).write(S_STATE_OFF, &EXT4_VALID_FS.to_le_bytes());
            let r = crate::fs::bio::sync_dirty_buffer(bh);
            crate::fs::bio::brelse(bh);
            r?;
        }
    }

    // 3. Disk-side volatile cache.
    crate::drivers::virtio::flush_pci_blk()
}

/// Create a VFS inode for the ext4 root directory (inode 2).
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
    let fs_ptr = GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire);
    if fs_ptr.is_null() {
        // Fallback: shouldn't happen at mount time
        let mut inode = Inode::new(2, InodeMode::new(InodeMode::S_IFDIR | 0o755));
        inode.ops = Some(&EXT4_INODE_OPS);
        return alloc::sync::Arc::new(inode);
    }
    // SAFETY: fs_ptr was loaded from GLOBAL_EXT4_FS with null check above;
    // the pointer is valid for the lifetime of the mount (set in ext4_mount).
    unsafe {
        let fs = &*fs_ptr;
        match fs.read_inode(2) {
            Ok(ext4_inode) => create_vfs_inode(2, &ext4_inode),
            Err(_) => {
                let mut inode = Inode::new(2, InodeMode::new(InodeMode::S_IFDIR | 0o755));
                inode.ops = Some(&EXT4_INODE_OPS);
                alloc::sync::Arc::new(inode)
            }
        }
    }
}

/// Unmount ext4 filesystem
///
/// This sets the global ext4 filesystem pointer to null and frees the
/// Ext4FileSystem structure. After this call, is_mounted() returns false
/// and all ext4 operations will fail.
pub fn unmount_ext4() {
    use core::sync::atomic::Ordering;

    let fs_ptr = GLOBAL_EXT4_FS.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if !fs_ptr.is_null() {
        // SAFETY: global pointer is valid once initialized via mount_ext4
        unsafe {
            let _ = Box::from_raw(fs_ptr);
        }
    }
}

/// Lookup path in ext4 filesystem and return VFS inode
///
/// # Parameters
/// Read file from mounted ext4 filesystem
///
/// # Parameters
/// - `path`: File path (absolute path)
///
/// # Returns
/// - `Some(data)`: File content
/// - `None`: Read failed
pub fn read_file_from_mounted(path: &str) -> Option<alloc::vec::Vec<u8>> {
    use core::sync::atomic::Ordering;

    let fs_ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
    if fs_ptr.is_null() {
        return None;
    }

    // Parse path to absolute path
    let abs_path = resolve_path(path);

    // SAFETY: fs_ptr was loaded from GLOBAL_EXT4_FS with null check above;
    // the pointer is valid for the lifetime of the mount.
    unsafe {
        let fs = &*fs_ptr;

        // Use the global mounted filesystem directly instead of creating
        // a temporary Ext4FileSystem, which avoids unnecessary buffer
        // cache pressure and potential eviction issues.
        let (_, inode) = fs.lookup_path(&abs_path).ok()?;

        let file_size = inode.get_size() as usize;
        if file_size == 0 {
            return Some(alloc::vec::Vec::new());
        }

        let mut buffer = alloc::vec::Vec::with_capacity(file_size);
        buffer.resize(file_size, 0);

        match file::ext4_file_read(fs, &inode, 0, &mut buffer) {
            Ok(n) => {
                buffer.truncate(n);
                Some(buffer)
            }
            Err(_) => None,
        }
    }
}

/// Create a new file on ext4 filesystem
///
/// # Arguments
/// - `path`: Absolute path for the new file
/// - `mode`: File mode (permissions)
///
/// # Returns
/// - `Ok(inode)`: VFS inode of the created file
/// - `Err(errno)`: Error code
pub fn create_file(path: &str, mode: u32) -> Result<alloc::sync::Arc<Inode>, i32> {
    use core::sync::atomic::Ordering;
    use crate::fs::ext4::inode::{Ext4Inode, file_type};
    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};

    // SAFETY: GLOBAL_EXT4_FS stores a pointer set during ext4_mount; null check
    // follows; the pointer is valid for the lifetime of the mount.
    unsafe {
        let fs_ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
        if fs_ptr.is_null() {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let fs = &*fs_ptr;

        // Parse path to get parent directory and filename
        let abs_path = resolve_path(path);
        let (parent_path, filename) = split_path(&abs_path);

        // Lookup parent directory, creating intermediate dirs if needed (mkdir -p)
        let parent_inode = create_parent_dirs(fs, &abs_path, &parent_path)?;

        if !parent_inode.is_dir() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        // Allocate new inode
        let allocator = allocator::InodeAllocator::new(fs);
        let new_ino = allocator.alloc_inode()?;

        // Initialize new inode
        let mut new_inode = Ext4Inode {
            ino: new_ino,
            mode: (file_type::S_IFREG | (mode as u16 & 0o777)) as u16,
            uid: 0,
            gid: 0,
            size: 0,
            blocks: 0,
            links_count: 1,
            flags: 0x80000,  // EXT4_EXTENTS_FL - use extent tree
            block: [0u32; 15],
            atime: 0,
            mtime: 0,
            ctime: 0,
        };

        // Initialize extent header in i_block
        let header = &mut *(new_inode.block.as_mut_ptr() as *mut Ext4ExtentHeader);
        header.eh_magic = EXT4_EXT_MAGIC;
        header.eh_entries = 0;
        header.eh_max = 4;
        header.eh_depth = 0;
        header.eh_generation = 0;

        // Write new inode to disk
        inode::write_inode(fs, new_ino, &new_inode)?;

        // Add directory entry in parent
        add_dir_entry(fs, &parent_inode, filename, new_ino, 1)?;  // 1 = regular file

        // Create VFS inode
        Ok(create_vfs_inode_in(fs as *const Ext4FileSystem as *mut Ext4FileSystem, new_ino, &new_inode))
    }
}

/// fallocate flags (UAPI)
pub const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
pub const FALLOC_FL_PUNCH_HOLE: i32 = 0x02;

/// ext4 fallocate — minimal implementation (review 5.5).
///
/// * KEEP_SIZE (and the default mode): allocate blocks covering
///   [offset, offset+len) WITHOUT changing i_size.
/// * PUNCH_HOLE (must be combined with KEEP_SIZE per Linux): zeroes the
///   range and FREES every root extent that lies entirely inside it
///   (depth-0 trees only). Extents only partially covered are zeroed in
///   place and kept — their release would require extent splitting.
///   Reads over the whole range observe zeroes either way (hole
///   semantics), so the observable behaviour matches; space reclamation
///   is best-effort for partially-covered extents.
pub fn ext4_fallocate(
    ino: u32,
    mode: i32,
    offset: u64,
    len: u64,
) -> Result<(), i32> {
    // Instance-aware wrapper: prefer the caller-resolved filesystem (a
    // loop-mounted ext2 must not touch the boot root's inode tables).
    ext4_fallocate_on(ino, mode, offset, len, None)
}

/// `fs_hint`: the filesystem instance the inode lives on (None = boot
/// root via GLOBAL_EXT4_FS).
pub fn ext4_fallocate_on(
    ino: u32,
    mode: i32,
    offset: u64,
    len: u64,
    fs_hint: Option<*const Ext4FileSystem>,
) -> Result<(), i32> {
    use crate::fs::ext4::extent::{Ext4ExtentHeader, Ext4Extent, EXT4_EXT_MAGIC};

    if len == 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    let end = match offset.checked_add(len) {
        Some(e) => e,
        None => return Err(errno::Errno::FileTooLarge.as_neg_i32()),
    };

    let fs_ptr = fs_hint.unwrap_or_else(|| {
        GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire) as *const Ext4FileSystem
    });
    if fs_ptr.is_null() {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // SAFETY: the instance pointer comes from the caller's inode
    // (private_data) or the boot GLOBAL — both valid for the mount.
    unsafe {
        let fs = &*fs_ptr;

        // EFBIG before touching the allocator (Linux ext4_fallocate checks
        // s_maxbytes first). The extent tree encodes logical blocks in a
        // u32 ee_block, so the largest representable file is 2^32 blocks;
        // a request past that used to fall into the block allocator and
        // drain the whole filesystem run by run before failing ENOSPC —
        // and because a failed preallocation did not persist the inode,
        // every drained block leaked permanently (the LTP r4/r5 "/tmp
        // fills and never recovers" regression: fallocate02's EFBIG
        // subtests alone consumed ~500MB this way).
        let max_file_size = (1u64 << 32) * fs.block_size as u64;
        if offset >= max_file_size || end > max_file_size {
            return Err(errno::Errno::FileTooLarge.as_neg_i32());
        }

        let _ext4_guard = EXT4_BIG_LOCK.lock_fair();

        let mut ext4_inode = fs.read_inode(ino)?;
        if !ext4_inode.is_reg() {
            return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32()); // EBADR-ish
        }

        let block_size = fs.block_size as u64;
        let file_size = ext4_inode.size;

        if mode & FALLOC_FL_PUNCH_HOLE != 0 {
            if mode & FALLOC_FL_KEEP_SIZE == 0 {
                return Err(errno::Errno::InvalidArgument.as_neg_i32());
            }
            // Clamp the punch range to the file size.
            let file_end = file_size.min(end);
            if offset >= file_end {
                return Ok(()); // nothing to punch
            }
            let punch_first = offset / block_size;
            let punch_last = (file_end - offset).div_ceil(block_size) + punch_first; // exclusive

            if ext4_inode.has_extent() {
                // Any-depth punch via gather/classify/rebuild (the old
                // inline-root-only walker refused deep trees and could
                // not represent more than 4 surviving fragments).
                let hdr = &*(ext4_inode.block.as_ptr() as *const Ext4ExtentHeader);
                if hdr.eh_magic == EXT4_EXT_MAGIC {
                    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                    let all = crate::fs::ext4::extent::ext4_ext_gather(fs, &ext4_inode.block)?;
                    let mut kept: alloc::vec::Vec<crate::fs::ext4::extent::RawExtent> =
                        alloc::vec::Vec::new();
                    let mut freed_sectors: u64 = 0;
                    for e in &all {
                        let ext_first = e.ee_block as u64;
                        let ext_len = e.len as u64;
                        let ext_last = ext_first + ext_len;
                        let phys = e.phys;

                        if ext_first >= punch_first && ext_last <= punch_last {
                            // Entirely inside the punch range: free it all
                            // (one bitmap pass for the contiguous run —
                            // see free_inode_blocks for why per-block
                            // frees are untenable for big extents).
                            let _ = allocator.free_block_run(phys, ext_len);
                            freed_sectors += ext_len * (block_size / 512);
                        } else if ext_first < punch_last && ext_last > punch_first {
                            // Partially covered: ZERO the overlapped blocks in
                            // place; allocation kept (documented above).
                            let z_from = punch_first.max(ext_first);
                            let z_to = punch_last.min(ext_last);
                            for b in z_from..z_to {
                                let phys_b = phys + (b - ext_first);
                                if phys_b == 0 {
                                    continue;
                                }
                                if let Some(bh) = crate::fs::bio::bread(fs.device, phys_b) {
                                    for byte in (*bh).b_data.iter_mut() {
                                        *byte = 0;
                                    }
                                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                                    let _ = crate::fs::bio::sync_dirty_buffer(bh);
                                    crate::fs::bio::brelse(bh);
                                }
                            }
                            // Keep the RAW entry — the unwritten flag on a
                            // kept preallocated range must survive (masked
                            // lengths turn never-materialized blocks into
                            // "written" and reads would fetch disk garbage
                            // instead of zeros).
                            kept.push(*e);
                        } else {
                            kept.push(*e);
                        }
                    }
                    let meta_delta = crate::fs::ext4::extent::ext4_ext_rebuild(
                        fs, &mut ext4_inode.block, &kept,
                    )?;
                    ext4_inode.blocks = ext4_inode
                        .blocks
                        .saturating_sub(freed_sectors)
                        .saturating_add_signed(meta_delta);
                }
            } else {
                // Indirect-block files: zero the covered direct blocks in
                // place (allocation retained) — conservative hole semantics.
                let first = (offset / block_size) as usize;
                let last = ((file_end.saturating_sub(1)) / block_size) as usize;
                for i in first..=last.min(11) {
                    let b = ext4_inode.block[i];
                    if b != 0 {
                        if let Some(bh) = crate::fs::bio::bread(fs.device, b as u64) {
                            for byte in (*bh).b_data.iter_mut() {
                                *byte = 0;
                            }
                            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                            let _ = crate::fs::bio::sync_dirty_buffer(bh);
                            crate::fs::bio::brelse(bh);
                        }
                    }
                }
            }

            // Page cache may hold the old contents.
            crate::fs::page_cache::get_page_cache()
                .invalidate_inode(fs as *const Ext4FileSystem as u64, ino as u64);

            let sec = crate::drivers::rtc::wall_secs() as u32;
            ext4_inode.mtime = sec;
            ext4_inode.ctime = sec;
            // write_inode_disk expects the on-disk layout; convert.
            let on_disk = ext4_inode.to_on_disk();
            inode::write_inode_disk(fs, ino, &on_disk)?;
            return Ok(());
        }

        // KEEP_SIZE / default: preallocate blocks up to `end`.
        let needed_blocks = (end + block_size - 1) / block_size;
        let current_blocks = (file_size + block_size - 1) / block_size;
        if needed_blocks > current_blocks {
            // Extent files: UNWRITTEN preallocation — metadata-only, no
            // per-block zeroing (a 300MB posix_fallocate would otherwise
            // issue 76800 buffer reads+writes and wedge the system).
            let alloc_res = if ext4_inode.has_extent() {
                let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                let goal_group = (ext4_inode.ino / fs.inodes_per_group).min(fs.group_count - 1);
                file::preallocate_unwritten_extents(
                    fs, &mut ext4_inode, needed_blocks, current_blocks, &allocator, goal_group,
                )
            } else {
                file::allocate_blocks_for_file(fs, &mut ext4_inode, needed_blocks)
            };
            if let Err(e) = alloc_res {
                // The allocators mark blocks used in the on-disk bitmap as
                // they go, but only the in-memory `ext4_inode` carries the
                // extents claiming them. Persist the partial state BEFORE
                // propagating the error — otherwise every block allocated
                // before the failure (ENOSPC mid-run, bitmap I/O error, ...)
                // is owned by nobody and leaks permanently once the caller
                // deletes the file (the r4/r5 /tmp-exhaustion leak).
                // Linux keeps partial fallocate allocations too; the file
                // stays deletable and truncatable, reclaiming its blocks.
                let sec = crate::drivers::rtc::wall_secs() as u32;
                ext4_inode.mtime = sec;
                ext4_inode.ctime = sec;
                let on_disk = ext4_inode.to_on_disk();
                let _ = inode::write_inode_disk(fs, ino, &on_disk);
                return Err(e);
            }
        }
        // Default mode (no FALLOC_FL_KEEP_SIZE) extends the file: the range
        // is guaranteed allocated, reads as zeros, and i_size grows to
        // cover it (fallocate(2); POSIX posix_fallocate relies on this —
        // LTP fallocate01/03 stat the size after allocating). KEEP_SIZE
        // leaves i_size untouched.
        if mode & FALLOC_FL_KEEP_SIZE == 0 && end > ext4_inode.size {
            ext4_inode.set_size(end);
        }
        {
            let sec = crate::drivers::rtc::wall_secs() as u32;
            ext4_inode.mtime = sec;
            ext4_inode.ctime = sec;
            let on_disk = ext4_inode.to_on_disk();
            inode::write_inode_disk(fs, ino, &on_disk)?;
        }
        Ok(())
    }
}

/// Split path into parent directory and filename
fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    if let Some(last_slash) = trimmed.rfind('/') {
        let parent = if last_slash == 0 { "/" } else { &trimmed[..last_slash] };
        let name = &trimmed[last_slash + 1..];
        (parent, name)
    } else {
        ("/", path)
    }
}

/// Create parent directories as needed (mkdir -p semantics).
///
/// Tries to lookup `parent_path`; if it fails (ENOENT), creates missing
/// intermediate directories one by one using `ext4_mkdir`.
///
/// Returns the parent directory's Ext4Inode.
fn create_parent_dirs(
    fs: &Ext4FileSystem,
    _abs_path: &str,
    parent_path: &str,
) -> Result<inode::Ext4Inode, i32> {
    // Fast path: parent directory exists and is a directory
    if let Ok((_, inode)) = fs.lookup_path(parent_path) {
        if inode.is_dir() {
            return Ok(inode);
        }
        // A file exists where we need a directory — cannot proceed
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Slow path: create intermediate directories one by one.
    // e.g., for "/var/log/kmsg" with parent "/var/log",
    // create "/var" then "/var/log"
    let parts: Vec<&str> = parent_path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current_ino = 2u32; // root inode

    for (i, part) in parts.iter().enumerate() {
        // Build the full path up to this component, e.g., "/var", "/var/log"
        let mut path = alloc::string::String::from("/");
        for (j, p) in parts[..=i].iter().enumerate() {
            if j > 0 {
                path.push('/');
            }
            path.push_str(p);
        }

        match fs.lookup_path(&path) {
            Ok((ino, inode)) => {
                if !inode.is_dir() {
                    return Err(errno::Errno::NotADirectory.as_neg_i32());
                }
                current_ino = ino;
            }
            Err(_) => {
                // Directory doesn't exist, create it
                current_ino = crate::fs::ext4::namei::ext4_mkdir(
                    fs,
                    current_ino,
                    part.as_bytes(),
                    0o755,
                )?;
            }
        }
    }

    // Read the final parent inode
    fs.read_inode(current_ino)
}

/// Add a directory entry
fn add_dir_entry(
    fs: &Ext4FileSystem,
    parent: &inode::Ext4Inode,
    name: &str,
    ino: u32,
    file_type: u8,
) -> Result<(), i32> {
    use crate::fs::bio;

    // htree-indexed directories (EXT4_INDEX_FL / dx_root magic): our
    // directory writer is linear-only — inserting into an indexed
    // directory without updating the dx tree leaves real Linux unable to
    // find the entry. Conservatively refuse the write (review 5.5).
    if parent.flags & features::EXT4_INDEX_FL != 0 {
        return Err(-(crate::syscall::errno::EOPNOTSUPP as i32));
    }

    // Get parent's data blocks
    let blocks = parent.get_data_blocks(fs)?;

    if blocks.is_empty() {
        // Parent has no data blocks, need to allocate one
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    let block_size = fs.block_size as usize;
    let entry_size = ((8 + name.len() as usize + 3) / 4) * 4;

    // Iterate all blocks looking for space
    for block_num in &blocks {
        if *block_num == 0 {
            continue;  // Skip sparse blocks
        }

        // SAFETY: fs.device is a valid GenDisk pointer; *block_num is a non-zero
        // block number from get_data_blocks; bio::bread returns a valid BufferHead.
        unsafe {
            let bh = bio::bread(fs.device, *block_num)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let data = &mut (*bh).b_data;

            // Try to find space in this block
            let mut offset = 0;
            let mut prev_offset = 0usize;
            let mut prev_rec_len = 0usize;

            while offset + 8 < block_size {
                let rec_len = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) as usize;

                if rec_len == 0 {
                    break;
                }

                // Check if this is an unused entry (inode == 0) with enough space
                let existing_ino = u32::from_le_bytes([
                    data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
                ]);

                if existing_ino == 0 && rec_len >= entry_size {
                    // Reuse this entry
                    let entry_data = &mut data[offset..offset + entry_size];
                    create_dir_entry(entry_data, ino, name, file_type, rec_len as u16);
                    (*bh).set_state_bit(bio::BufferState::BH_Dirty);
                    let sync_res = bio::sync_dirty_buffer(bh);
                    bio::brelse(bh);
                    sync_res?;
                    return Ok(());
                }

                prev_offset = offset;
                prev_rec_len = rec_len;
                offset += rec_len;
            }

            // Try to add at end of this block
            if offset + entry_size <= block_size {
                // Update previous entry's rec_len to point to new entry.
                // Only split if prev entry's rec_len actually spans to offset.
                if prev_offset + prev_rec_len == offset && prev_rec_len > 0 {
                    let prev_name_len = data[prev_offset + 6];
                    let prev_actual_size = ((8 + prev_name_len as usize + 3) / 4) * 4;
                    if prev_actual_size >= prev_rec_len {
                        // Previous entry has no spare space; cannot split
                        bio::brelse(bh);
                        continue;
                    }
                    // Validate no overlap: actual size must leave room for new entry
                    if offset + entry_size > prev_offset + prev_rec_len {
                        bio::brelse(bh);
                        continue;
                    }
                    data[prev_offset + 4] = (prev_actual_size & 0xFF) as u8;
                    data[prev_offset + 5] = ((prev_actual_size >> 8) & 0xFF) as u8;
                }

                // Create new entry
                let remaining = block_size - offset;
                let entry_data = &mut data[offset..offset + entry_size];
                create_dir_entry(entry_data, ino, name, file_type, remaining as u16);

                (*bh).set_state_bit(bio::BufferState::BH_Dirty);
                let sync_res = bio::sync_dirty_buffer(bh);
                bio::brelse(bh);
                sync_res?;
                return Ok(());
            }

            bio::brelse(bh);
        }
    }

    // All blocks are full, need to allocate a new block
    append_dir_block(fs, parent, name, ino, file_type, entry_size, block_size)
}

/// Append a new block to directory and add entry
fn append_dir_block(
    fs: &Ext4FileSystem,
    parent: &inode::Ext4Inode,
    name: &str,
    ino: u32,
    file_type: u8,
    entry_size: usize,
    block_size: usize,
) -> Result<(), i32> {
    use crate::fs::bio;

    // Read parent inode from disk first
    let parent_ino = parent.ino;
    let mut parent_inode = fs.read_inode(parent_ino)?;

    // Get current block count
    let current_blocks = (parent_inode.get_size() + block_size as u64 - 1) / block_size as u64;
    let new_block_index = current_blocks;

    // First allocate the block in the inode's extent tree/indirect blocks
    // This will give us the physical block number
    file::allocate_blocks_for_file(fs, &mut parent_inode, new_block_index + 1)?;

    // Now get the actual block number that was allocated
    let new_block = parent_inode.get_data_block(fs, new_block_index)?;

    // Zero the new block and create the directory entry
    // SAFETY: fs.device is a valid GenDisk pointer; new_block is a freshly allocated
    // block number; bio::bread returns a valid BufferHead.
    unsafe {
        let bh = bio::bread(fs.device, new_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        // Zero the block
        for byte in (*bh).b_data.iter_mut() {
            *byte = 0;
        }

        // Create a single entry spanning the whole block
        // Since we don't have checksum, use full block size
        let data = &mut (*bh).b_data;
        create_dir_entry(data, ino, name, file_type, block_size as u16);

        (*bh).set_state_bit(bio::BufferState::BH_Dirty);
        let sync_res = bio::sync_dirty_buffer(bh);
        bio::brelse(bh);
        sync_res?;
    }

    // Update parent directory size
    let new_size = (new_block_index + 1) as u64 * block_size as u64;
    parent_inode.set_size(new_size);

    // Write parent inode back to disk
    inode::write_inode(fs, parent_ino, &parent_inode)?;

    Ok(())
}

/// Create a directory entry in buffer
fn create_dir_entry(data: &mut [u8], ino: u32, name: &str, file_type: u8, rec_len: u16) {
    // inode number (4 bytes)
    data[0..4].copy_from_slice(&ino.to_le_bytes());
    // record length (2 bytes)
    data[4..6].copy_from_slice(&rec_len.to_le_bytes());
    // name length (1 byte)
    data[6] = name.len() as u8;
    // file type (1 byte)
    data[7] = file_type;
    // name (variable)
    data[8..8 + name.len()].copy_from_slice(name.as_bytes());
}

// ============================================================================
// Ext4 Inode Operations
// ============================================================================

use crate::fs::inode::{Inode, InodeMode, INodeOps, Ino};
use crate::fs::Stat;

/// Ext4 inode lookup operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let fs_ptr = dir.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from parent's private_data
    let parent_ext4_inode = dir.sb.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let parent_inode = &*(parent_ext4_inode as *const inode::Ext4Inode);

    // Convert name to str
    let name_str = core::str::from_utf8(name).map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    // Lookup in directory
    let entry = fs.lookup(parent_inode, name_str)?;
    Ok(entry.inode as Ino)
}

/// Ext4 getattr operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_getattr(inode: &Inode, stat: &mut Stat) -> i32 {
    let fs_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let _fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from sb field (we store it there)
    let ext4_inode_ptr = match inode.sb {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let ext4_inode = &*(ext4_inode_ptr as *const inode::Ext4Inode);

    stat.st_ino = inode.ino;
    stat.st_mode = ext4_inode.mode as u32;  // u16 -> u32
    stat.st_size = ext4_inode.get_size() as i64;
    stat.st_nlink = ext4_inode.links_count as u32;
    stat.st_uid = ext4_inode.uid as u32;
    stat.st_gid = ext4_inode.gid as u32;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = ext4_inode.blocks as i64;
    stat.st_atime = ext4_inode.atime as i64;
    stat.st_atime_nsec = 0;
    stat.st_mtime = ext4_inode.mtime as i64;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = ext4_inode.ctime as i64;
    stat.st_ctime_nsec = 0;

    0
}

/// Ext4 readlink operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let fs_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from sb field
    let ext4_inode_ptr = match inode.sb {
        Some(ptr) => ptr,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let ext4_inode = &*(ext4_inode_ptr as *const inode::Ext4Inode);

    if !ext4_inode.is_symlink() {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }

    // Read symlink target
    let size = ext4_inode.get_size() as usize;
    if size == 0 || size > buf.len() {
        return errno::Errno::IOError.as_neg_i32() as isize;
    }

    // Short symlink: data stored inline
    if size <= 60 && !ext4_inode.has_extent() {
        let block_data = core::slice::from_raw_parts(
            ext4_inode.block.as_ptr() as *const u8,
            60
        );
        buf[..size].copy_from_slice(&block_data[..size]);
        size as isize
    } else {
        // Long symlink: read from data blocks
        match ext4_inode.read_data(fs, 0, &mut buf[..size]) {
            Ok(n) if n == size => n as isize,
            _ => errno::Errno::IOError.as_neg_i32() as isize,
        }
    }
}

/// Ext4 setattr implementation
///
/// Handles chmod (ATTR_MODE), chown (ATTR_UID_GID), and ftruncate (ATTR_SIZE)
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_setattr(inode: &Inode, attr: u32, arg1: u64, arg2: u64) -> i32 {
    use crate::fs::inode::setattr_attr;

    // ext4 has no internal concurrency protection on the block
    // allocator / inode writer (review 5.5 high: EXT4_BIG_LOCK 仅覆盖
    // namei): take the big lock for the whole setattr — truncate frees
    // blocks (bitmap RMW) and rewrites the inode.
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();

    let fs = match get_ext4_fs_from_inode(inode) {
        Ok(fs) => fs,
        Err(e) => return e,
    };
    let ext4_ino = inode.ino as u32;

    let mut ext4_inode = match fs.read_inode(ext4_ino) {
        Ok(i) => i,
        Err(e) => return e,
    };

    match attr {
        setattr_attr::ATTR_MODE => {
            // arg1 = new mode. A full mode word (type bits present) RETYPES
            // the inode — that is how AF_UNIX bind(2) and mkfifo(3) create
            // their special nodes (create-regular-then-retype, mirroring
            // the kernel mknod path). A perm-only word (chmod) preserves
            // the existing file type, exactly like Linux chmod.
            let v = arg1 as u32;
            let vtype = v & 0o170000;
            // Keep the full S_IALLUGO (0o7777: setuid/setgid/sticky included).
            // Masking to 0o777 silently dropped S_ISUID/S_ISGID/S_ISVTX, so
            // chmod(2) modes like 01777/04755/02777 never stuck on ext4
            // (LTP chmod01/chmod07, mkdir02 S_ISGID inheritance).
            let new_mode = if vtype != 0 {
                vtype | (v & 0o7777)
            } else {
                ((ext4_inode.mode as u32) & 0o170000) | (v & 0o7777)
            };
            ext4_inode.mode = new_mode as u16;
            // Keep the icache copy coherent: open() and DAC checks read
            // Inode.mode directly while stat() goes through getattr — a
            // stale cache made retyped nodes (mkfifo/socket) and chmod
            // results invisible to the VFS (LTP open06/read03).
            inode.update_cached_mode(crate::fs::inode::InodeMode::new(new_mode));
        }
        setattr_attr::ATTR_UID_GID => {
            // arg1 = uid, arg2 = gid. Keep the VFS-cached owner in sync —
            // open()/chmod/chown DAC and owner checks read Inode.uid/gid
            // directly while stat() goes through getattr; a stale cached
            // owner made a post-chown chmod by the NEW owner fail EPERM
            // (LTP chmod05: setup chowns testdir to nobody, drops to
            // nobody, then chmods its own directory).
            ext4_inode.uid = arg1 as u16;
            ext4_inode.gid = arg2 as u16;
            inode.uid.store(arg1 as u32, core::sync::atomic::Ordering::Relaxed);
            inode.gid.store(arg2 as u32, core::sync::atomic::Ordering::Relaxed);
        }
        setattr_attr::ATTR_ATIME => {
            ext4_inode.atime = arg1 as u32;
        }
        setattr_attr::ATTR_MTIME => {
            ext4_inode.mtime = arg1 as u32;
            ext4_inode.ctime = arg1 as u32;
        }
        setattr_attr::ATTR_SIZE => {
            // arg1 = new size (ftruncate). Upper-bounded to a sane maximum:
            // the extent code can only address 2^32 blocks so anything
            // beyond ~16TB is unreachable; u64::MAX used to flow straight
            // into i_size and wrap later allocations (review EXT4-M3).
            const MAX_FILE_SIZE: u64 = 1 << 42; // 4 TB
            if arg1 > MAX_FILE_SIZE {
                return errno::Errno::FileTooLarge.as_neg_i32();
            }
            let new_size = arg1;
            if new_size < ext4_inode.get_size() {
                // Truncate: free blocks beyond new_size
                let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                let block_size = fs.block_size as u64;
                let new_blocks = (new_size + block_size - 1) / block_size;
                let old_blocks = (ext4_inode.get_size() + block_size - 1) / block_size;

                if ext4_inode.has_extent() {
                    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};
                    // Any-depth shrink via gather/classify/rebuild: extents
                    // fully beyond the new EOF are freed, a straddling one
                    // keeps its head (unwritten flag preserved), and the
                    // tree is rebuilt from the survivors (the old code only
                    // handled the 4-entry inline root and refused deep
                    // trees).
                    let hdr = unsafe {
                        &*(ext4_inode.block.as_ptr() as *const Ext4ExtentHeader)
                    };
                    if hdr.eh_magic == EXT4_EXT_MAGIC {
                        let all = match crate::fs::ext4::extent::ext4_ext_gather(fs, &ext4_inode.block) {
                            Ok(a) => a,
                            Err(e) => return e,
                        };
                        let mut kept: alloc::vec::Vec<crate::fs::ext4::extent::RawExtent> =
                            alloc::vec::Vec::new();
                        let mut freed_sectors: u64 = 0;
                        for e in &all {
                            let logical_start = e.ee_block as u64;
                            let phys_start = e.phys;
                            let ext_len = e.len as u64;

                            if logical_start >= new_blocks {
                                // Entirely beyond the new EOF: drop it.
                                let _ = allocator.free_block_run(phys_start, ext_len);
                                freed_sectors += ext_len * (block_size / 512);
                            } else if logical_start + ext_len > new_blocks {
                                // Straddles: shrink to the new EOF, free the tail.
                                let keep_len = new_blocks - logical_start;
                                let _ = allocator.free_block_run(
                                    phys_start + keep_len,
                                    ext_len - keep_len,
                                );
                                freed_sectors += (ext_len - keep_len) * (block_size / 512);
                                // Preserve the unwritten flag on the kept
                                // head (see the punch-path note above).
                                kept.push(crate::fs::ext4::extent::RawExtent {
                                    ee_block: e.ee_block,
                                    phys: phys_start,
                                    len: keep_len as u16,
                                    unwritten: e.unwritten,
                                });
                            } else {
                                // Fully below the new EOF: keep as-is.
                                kept.push(*e);
                            }
                        }
                        let meta_delta = match crate::fs::ext4::extent::ext4_ext_rebuild(
                            fs, &mut ext4_inode.block, &kept,
                        ) {
                            Ok(d) => d,
                            Err(e) => return e,
                        };
                        ext4_inode.blocks = ext4_inode
                            .blocks
                            .saturating_sub(freed_sectors)
                            .saturating_add_signed(meta_delta);
                    }
                } else {
                    // Free indirect blocks beyond new size
                    for i in new_blocks as usize..old_blocks as usize {
                        if i < 12 {
                            if ext4_inode.block[i] != 0 {
                                let _ = allocator.free_block(ext4_inode.block[i] as u64);
                                ext4_inode.block[i] = 0;
                            }
                        } else {
                            // Indirect blocks - use ext4_get_block to check, then free
                            match indirect::ext4_get_block(fs, &ext4_inode.block, i as u64) {
                                Ok(block_num) if block_num != 0 => {
                                    let _ = allocator.free_block(block_num);
                                }
                                _ => {}
                            }
                        }
                    }
                    // Clean up indirect block pointers if truncating below 12 blocks
                    if new_blocks < 12 && ext4_inode.block[12] != 0 {
                        let _ = allocator.free_block(ext4_inode.block[12] as u64);
                        ext4_inode.block[12] = 0;
                    }
                    if new_blocks < 12 + (block_size / 4) as u64 && ext4_inode.block[13] != 0 {
                        let _ = allocator.free_block(ext4_inode.block[13] as u64);
                        ext4_inode.block[13] = 0;
                    }
                    // Triple-indirect (block[14]) was never freed here —
                    // files > 12+1024 blocks leaked their L2 tree on
                    // truncate below that boundary (review EXT4-M8).
                    // Triple-indirect METADATA block: only free the
                    // tree-pointer itself (data blocks were already freed
                    // by the per-block loop above); guard on truncating
                    // below the triple-indirect boundary so in-use trees
                    // survive (regression round 5, HIGH: double-free).
                    let p = (block_size / 4) as u64;
                    if new_blocks < 12 + p + p * p && ext4_inode.block[14] != 0 {
                        let _ = allocator.free_block(ext4_inode.block[14] as u64);
                        ext4_inode.block[14] = 0;
                    }
                }
                ext4_inode.blocks = (new_blocks * (block_size / 512)) as u64;

                // Partial last block: ZERO the tail of the kept block.
                // POSIX truncate-extend semantics make [old, new) read as
                // zeros — without this the stale bytes of the kept block
                // resurface on the next extend (LTP ftruncate01 extends
                // 256→1024 and reads back 'a's).
                if new_size % block_size != 0 {
                    let last_block = new_size / block_size;
                    if let Ok(block_nr) = ext4_inode.get_data_block(fs, last_block) {
                        if block_nr != 0 {
                            let tail_start = (new_size % block_size) as usize;
                            if let Some(bh) = crate::fs::bio::bread(fs.device, block_nr) {
                                // SAFETY: bh is a one-block buffer from
                                // bread; the tail range is in-bounds and
                                // the buffer is released after the sync.
                                unsafe {
                                    let data = (*bh).b_data.as_mut_ptr();
                                    core::ptr::write_bytes(
                                        data.add(tail_start),
                                        0,
                                        (block_size as usize) - tail_start,
                                    );
                                }
                                let _ = crate::fs::bio::sync_dirty_buffer(bh);
                                crate::fs::bio::brelse(bh);
                            }
                        }
                    }
                }
            }
            ext4_inode.set_size(new_size);
        }
        _ => return errno::Errno::InvalidArgument.as_neg_i32(),
    }

    // Update timestamps: Unix epoch seconds from the wall clock
    // (drivers/rtc::wall_secs — goldfish RTC boot read + settimeofday
    // adjustments); the monotonic boot clock is never stored on disk.
    // ctime changes on every metadata write. mtime is auto-stamped ONLY for
    // size changes — a second setattr call in a utimensat pair (ATIME after
    // MTIME) used to stomp the just-stored explicit mtime back to "now"
    // (LTP utime01/utime02/utime04).
    let sec = crate::drivers::rtc::wall_secs() as u32;
    ext4_inode.ctime = sec;
    if attr == setattr_attr::ATTR_SIZE {
        ext4_inode.mtime = sec;
    }

    // Write back
    match inode::write_inode(fs, ext4_ino, &ext4_inode) {
        Ok(()) => {
            // Refresh cached Ext4Inode so subsequent reads see the new state
            refresh_inode_cache(inode, fs);
            // FIX8 (stale VFS inode.size): ftruncate (ATTR_SIZE) updated the
            // on-disk inode and the sb-cached copy but never the VFS
            // Inode.size — stat() reads the sb copy and looked right, while
            // consumers of inode.size (loop_dev LOOP_SET_FD sizing, lseek
            // SEEK_END, mmap sizing) still saw the OLD size. A tmpfs-style
            // truncate-then-bind (LTP tst_acquire_device via ftruncate,
            // losetup) failed LOOP_SET_FD with EINVAL on a 0 size.
            if attr == setattr_attr::ATTR_SIZE {
                inode.size.store(
                    ext4_inode.get_size(),
                    core::sync::atomic::Ordering::Release,
                );
            }
            // Invalidate page cache after size change (truncate/extend)
            crate::fs::page_cache::get_page_cache().invalidate_inode(fs as *const Ext4FileSystem as u64, inode.ino);
            0
        }
        Err(e) => e,
    }
}

/// Ext4 destroy_inode: reclaim the Box<Ext4Inode> stored in inode.sb.
unsafe fn ext4_destroy_inode(inode: &mut crate::fs::inode::Inode) {
    if let Some(ptr) = inode.sb {
        // ptr was created by Box::into_raw(Box::new(ext4_inode.clone()))
        let _ = alloc::boxed::Box::from_raw(ptr as *mut inode::Ext4Inode);
        inode.sb = None;
    }
}

/// Coarse per-fs serialization for ext4 (review EXT4-H10 / NEW2):
/// SMP is enabled but ext4 has no internal concurrency protection
/// (global journal handle, bitmap RMW, group descriptor lost-update).
/// One CPU in ext4 at a time until per-inode locking exists.
///
/// R7-A2 (reverted): converting this to a semaphore-backed Mutex caused a
/// ~50% boot-to-smoke hang (tasks lost wakeups / vanished from the run
/// queue); the sleeping-Mutex path needs its own audit round before it
/// can carry this lock. The preemption-stall noise it produces as a
/// spinlock (namei I/O sleeps under it after its 256-iteration spin
/// window) remains a documented quality issue, not a correctness one:
/// mutual exclusion holds either way.
///
/// LTP-F10: the pure-spin acquire was NOT merely noisy — it could wedge
/// the whole machine. The holder legitimately sleeps in block I/O while
/// a second task spins in lock_fair on the other CPU; when the holder
/// is woken it is RUNNABLE but the kernel does not preempt a task
/// spinning in kernel mode, so the spinner keeps the CPU forever and
/// the holder never runs to release. Every ext4 op then piles up behind
/// the big lock with all CPUs spinning (silent serial — not even the
/// test-runner's watchdog can run to SIGKILL). Fixed by sleeping on a
/// waitqueue instead of spinning, using the post-R7-B6b/R8-6/R32-F10
/// discipline that R7-A2 predated: register on the queue BEFORE
/// re-checking the state, EXCLUSIVE/FIFO, wake-one on release, and undo
/// a spurious concurrent enqueue on the fast path.
pub static EXT4_BIG_LOCK: Ext4BigLock = Ext4BigLock::new();

/// Task-recursive big lock.
///
/// A loop device backed by a file on ANOTHER ext4 instance re-enters
/// ext4 from inside the block layer: ext4#2's create (holding the big
/// lock) writes metadata through /dev/loopN, and the loop's request_fn
/// calls the backing file's write op on ext4#1 — which takes the same
/// big lock. A plain spinlock self-deadlocks there; reentrancy is keyed
/// on the owning task (nested acquire by the SAME task is legal, the
/// outermost guard releases).
pub struct Ext4BigLock {
    state: crate::sync::spinlock::Spinlock<Ext4BigState>,
    /// Tasks blocked while another task holds the big lock (the holder
    /// may sleep in block I/O for a long time — see the note above).
    wait: crate::process::wait::WaitQueueHead,
}

struct Ext4BigState {
    locked: bool,
    owner: u32,
    depth: u32,
}

impl Ext4BigLock {
    const fn new() -> Self {
        Self {
            state: crate::sync::spinlock::Spinlock::new(Ext4BigState {
                locked: false,
                owner: 0,
                depth: 0,
            }),
            wait: crate::process::wait::WaitQueueHead::new(),
        }
    }

    /// Try to take the lock / recurse into it. Returns the guard on
    /// success. Caller must NOT hold the state spinlock.
    fn try_acquire_or_recurse(&'static self, pid: u32) -> Option<Ext4BigGuard> {
        let mut st = self.state.lock();
        if !st.locked {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            Some(Ext4BigGuard { lock: self })
        } else if st.owner == pid {
            st.depth += 1;
            Some(Ext4BigGuard { lock: self })
        } else {
            None
        }
    }

    /// Acquire (sleep on contention, recursive per task).
    pub fn lock_fair(&'static self) -> Ext4BigGuard {
        let pid = crate::process::current_pid();

        // Adaptive: a short spin first covers the common case of a holder
        // that is on-CPU and about to release (boot-time short sections
        // never touch the waitqueue at all).
        for _ in 0..4 {
            if let Some(g) = self.try_acquire_or_recurse(pid) {
                return g;
            }
            for _ in 0..64 {
                core::hint::spin_loop();
            }
        }

        // Contended for real. If we have no task context (early boot /
        // IRQ) we cannot sleep — degrade to the old pure spin. A task that
        // holds a spinlock (preempt_count != 0) cannot sleep EITHER:
        // __schedule's preempt discipline refuses to switch it out, so the
        // wait-then-schedule loop below degenerates into a hot retry loop
        // whose dequeue_if_enqueued hammers the GRQ lock and starves the
        // other CPUs' timer ticks (the sendmsg02 stall-quiet wedge — see
        // VFS_MUTATION_LOCK's note). Spin instead: interrupts stay enabled,
        // the scheduler is never touched, and whichever CPU runs the holder
        // can still take its ticks and release us.
        let can_sleep = crate::sched::current().is_some()
            && crate::interrupt::preempt::preempt_count() == 0;
        let cur = match crate::sched::current() {
            Some(t) if can_sleep => t,
            _ => {
                loop {
                    if let Some(g) = self.try_acquire_or_recurse(pid) {
                        return g;
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        loop {
            if let Some(g) = self.try_acquire_or_recurse(pid) {
                return g;
            }
            // Register on the wait queue BEFORE re-checking the state
            // (R7-B6b discipline): any release() that pairs with this
            // attempt finds us queued and hands its wake token to us.
            // EXCLUSIVE + tail insert => FIFO, one wake per release.
            self.wait.prepare_to_wait(cur, true, false);
            match self.try_acquire_or_recurse(pid) {
                Some(g) => {
                    self.wait.finish_wait(cur);
                    // A concurrent release()'s wake_up_one may have seen
                    // us queued+sleeping and enqueued us between the
                    // registration and the acquire — undo it (the
                    // per-class on_rq guards make this a no-op when we
                    // were not actually enqueued). Mirrors Semaphore::down.
                    crate::sched::dequeue_task(&*cur);
                    return g;
                }
                None => {}
            }
            // Sleep until release() wakes us (UNINTERRUPTIBLE: ext4 ops
            // are not signal-interruptible and the guard must be carried
            // back out of the critical section).
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();
            self.wait.finish_wait(cur);
            // Woken: loop and retry the acquire (another task may have
            // barged past us — the release that woke us is not a
            // reservation, just a retry token).
        }
    }

    fn release(&'static self) {
        {
            let mut st = self.state.lock();
            if st.depth > 0 {
                st.depth -= 1;
                if st.depth == 0 {
                    st.locked = false;
                    st.owner = 0;
                }
            }
        }
        // Hand the lock to one waiter. Done outside the state spinlock:
        // the waker takes the waitqueue lock, the waiter's finish_wait
        // takes it too — never both at once. Waking an empty queue is a
        // no-op, so this is safe even without waiter accounting.
        self.wait.wake_up_one();
    }
}

/// Guard: releases on drop (outermost only).
pub struct Ext4BigGuard {
    lock: &'static Ext4BigLock,
}

impl Drop for Ext4BigGuard {
    fn drop(&mut self) {
        self.lock.release();
    }
}

/// Ext4 inode operations table
/// Ext4 now supports write operations through namei module
pub static EXT4_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(ext4_lookup),
    create: Some(ext4_create_wrapper),
    link: Some(ext4_link_wrapper),
    unlink: Some(ext4_unlink_wrapper),
    symlink: Some(ext4_symlink_wrapper),
    mkdir: Some(ext4_mkdir_wrapper),
    rmdir: Some(ext4_rmdir_wrapper),
    mknod: None,        // TODO: implement
    rename: Some(ext4_rename_wrapper),
    readlink: Some(ext4_readlink),
    get_file_ops: Some(ext4_get_file_ops),
    readdir: Some(ext4_readdir),
    open: None,
    permission: None,   // Default: allow all
    getattr: Some(ext4_getattr),
    setattr: Some(ext4_setattr),
    iget: Some(ext4_iget),
    destroy_inode: Some(ext4_destroy_inode),
};

/// Ext4 iget: instantiate VFS Inode from (parent, name, ino).
///
/// Reads the child inode from disk using the parent's filesystem pointer.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_iget(parent: &Inode, _name: &[u8], ino: Ino) -> Result<alloc::sync::Arc<Inode>, i32> {
    let fs_ptr = parent.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Read child inode from disk
    let ext4_inode = fs.read_inode(ino as u32)
        .map_err(|_| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let vfs_inode = create_vfs_inode_in(fs as *const Ext4FileSystem as *mut Ext4FileSystem, ino as u32, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_mkdir to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_mkdir_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, i32> {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_from_inode(dir)?;

    // Call namei's ext4_mkdir
    let new_ino = namei::ext4_mkdir(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    // Read the new inode and convert to in-memory format
    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs as *const Ext4FileSystem as *mut Ext4FileSystem, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Update the parent directory's cached Ext4Inode after a directory modification.
/// This ensures subsequent lookups see the latest block pointers and size.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn refresh_parent_dir_cache(dir: &Inode, fs: &Ext4FileSystem) {
    refresh_inode_cache(dir, fs);
}

/// Refresh the Ext4Inode cached in inode.sb after a disk write.
/// This ensures cached data (size, blocks, timestamps) stays in sync.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn refresh_inode_cache(inode: &Inode, fs: &Ext4FileSystem) {
    if let Some(sb_ptr) = inode.sb {
        if let Ok(disk_inode) = inode::read_inode(fs, inode.ino as u32) {
            let updated = inode::Ext4Inode::from_disk(&disk_inode, inode.ino as u32);
            let cached = &mut *(sb_ptr as *mut inode::Ext4Inode);
            *cached = updated;
        }
    }
}

/// Wrapper for ext4_rmdir to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_rmdir_wrapper(dir: &Inode, name: &[u8]) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = match get_ext4_fs_from_inode(dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    let result = match namei::ext4_rmdir(fs, dir.ino as u32, name) {
        Ok(()) => 0,
        Err(e) => e,
    };

    // Refresh the parent's cached Ext4Inode: namei::ext4_rmdir decrements
    // the parent's on-disk links_count, but the VFS-cached copy stayed
    // stale, so stat kept reporting inflated st_nlink for directories
    // whose subdirs were removed (LTP tst_tmpdir rmobj saw nlink>=3 on an
    // EMPTY dir, took the "linked directory" unlink() path and got EISDIR).
    if result == 0 {
        refresh_parent_dir_cache(dir, fs);
    }

    result
}

/// Wrapper for ext4_create to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_create_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, i32> {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_from_inode(dir)?;
    let new_ino = namei::ext4_create(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs as *const Ext4FileSystem as *mut Ext4FileSystem, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_symlink to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_symlink_wrapper(dir: &Inode, name: &[u8], target: &[u8]) -> Result<alloc::sync::Arc<Inode>, i32> {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_from_inode(dir)?;
    let new_ino = namei::ext4_symlink(fs, dir.ino as u32, name, target)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs as *const Ext4FileSystem as *mut Ext4FileSystem, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_link to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_link_wrapper(dir: &Inode, name: &[u8], target: &Inode) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = match get_ext4_fs_from_inode(dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match namei::ext4_link(fs, dir.ino as u32, target.ino as u32, name) {
        Ok(()) => {
            // Refresh the target's CACHED Ext4Inode (VFS inode.sb): fstat
            // and stat read links_count through it, and namei::ext4_link
            // only updates the on-disk inode — the stale cache reported
            // st_nlink == 1 after a successful link (LTP fstat02).
            if let Some(ptr) = target.sb {
                let cached = &mut *(ptr as *mut inode::Ext4Inode);
                if let Ok(disk) = inode::read_inode(fs, target.ino as u32) {
                    *cached = inode::Ext4Inode::from_disk(&disk, target.ino as u32);
                }
            }
            0
        }
        Err(e) => e,
    }
}

/// Wrapper for ext4_unlink to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_unlink_wrapper(dir: &Inode, name: &[u8]) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = match get_ext4_fs_from_inode(dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    let result = match namei::ext4_unlink(fs, dir.ino as u32, name) {
        Ok(()) => 0,
        Err(e) => e,
    };

    // Update parent directory's cached Ext4Inode
    if result == 0 {
        refresh_parent_dir_cache(dir, fs);
    }

    result
}

/// Wrapper for ext4_rename to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_rename_wrapper(old_dir: &Inode, old_name: &[u8], new_dir: &Inode, new_name: &[u8]) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = match get_ext4_fs_from_inode(old_dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match namei::ext4_rename(fs, old_dir.ino as u32, old_name, new_dir.ino as u32, new_name) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Get Ext4FileSystem pointer from inode's private_data
fn get_ext4_fs_from_inode(inode: &Inode) -> Result<&'static Ext4FileSystem, i32> {
    let fs_ptr = inode.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    // SAFETY: fs_ptr was stored as a raw pointer to the global Ext4FileSystem during
    // create_vfs_inode; it is valid for the lifetime of the mount.
    unsafe {
        Ok(&*(fs_ptr as *const Ext4FileSystem))
    }
}

/// Get file operations for ext4 regular files and directories
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&file::EXT4_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// Ext4 readdir: list directory entries via inode.ops
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;

    let fs_ptr = get_ext4_fs_from_inode(inode).ok()?;
    let ext4_inode_ptr = inode.sb?;
    let ext4_inode = &*(ext4_inode_ptr as *const inode::Ext4Inode);

    let ext4_entries = fs_ptr.list_dir(ext4_inode).ok()?;
    let mut entries = alloc::vec::Vec::new();
    for entry in ext4_entries.iter() {
        let name_bytes = &entry.name[..entry.name_len as usize];
        let dt = match entry.file_type {
            1 => file_type::DT_REG,
            2 => file_type::DT_DIR,
            3 => file_type::DT_CHR,
            4 => file_type::DT_BLK,
            5 => file_type::DT_FIFO,
            6 => file_type::DT_SOCK,
            7 => file_type::DT_LNK,
            _ => file_type::DT_UNKNOWN,
        };
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: entry.inode as u64,
            name: name_bytes.to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

/// Create VFS inode from ext4 inode
///
/// This helper function creates a VFS inode structure from an ext4 inode,
/// properly setting up the inode_operations and private data.
pub fn create_vfs_inode(ino: u32, ext4_inode: &inode::Ext4Inode) -> alloc::sync::Arc<Inode> {
    let fs_ptr = GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire);
    create_vfs_inode_in(fs_ptr, ino, ext4_inode)
}

/// Instance-aware variant: wires the new inode to `fs_ptr` (the creating
/// directory's filesystem — a loop-mounted ext2's children must point at
/// the LOOP instance, not the boot root).
pub fn create_vfs_inode_in(
    fs_ptr: *mut Ext4FileSystem,
    ino: u32,
    ext4_inode: &inode::Ext4Inode,
) -> alloc::sync::Arc<Inode> {
    let mode = if ext4_inode.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | (ext4_inode.mode as u32 & 0o777))
    } else if ext4_inode.is_symlink() {
        InodeMode::new(InodeMode::S_IFLNK | 0o777)
    } else if ext4_inode.is_reg() {
        InodeMode::new(InodeMode::S_IFREG | (ext4_inode.mode as u32 & 0o777))
    } else {
        InodeMode::new(ext4_inode.mode as u32)
    };

    let mut inode = Inode::new(ino as u64, mode);
    // Set fs_id to the filesystem pointer address for cache uniqueness
    inode.fs_id = fs_ptr as u64;
    inode.uid.store(ext4_inode.uid as u32, core::sync::atomic::Ordering::Relaxed);
    inode.gid.store(ext4_inode.gid as u32, core::sync::atomic::Ordering::Relaxed);
    inode.size.store(ext4_inode.size, core::sync::atomic::Ordering::Relaxed);
    inode.ops = Some(&EXT4_INODE_OPS);
    inode.private_data = Some(fs_ptr as *mut u8);

    // Cache a copy of the Ext4Inode in sb field (boxed, leaked pointer)
    // This avoids re-reading from disk on every read/write/stat/lseek
    let ext4_copy = alloc::boxed::Box::new(ext4_inode.clone());
    inode.sb = Some(Box::into_raw(ext4_copy) as *mut u8);

    alloc::sync::Arc::new(inode)
}

/// Mount a SECOND ext4/ext2 instance (loop-backed scratch filesystem).
///
/// Builds an independent Ext4FileSystem on `disk` (an mkfs.ext2 image
/// bound to /dev/loopN), reads its root inode, and returns a VFS root
/// wired to THIS instance (private_data / fs_id / cached inode). The
/// boot root keeps GLOBAL_EXT4_FS; all instance-specific operations
/// resolve the filesystem from the inode (get_ext4_fs_from_inode).
/// The instance is intentionally leaked (single mount per boot test
/// lifecycle; umount drops the dentry tree).
pub fn mount_loop_instance(
    disk: *const blkdev::GenDisk,
) -> Result<alloc::sync::Arc<Inode>, i32> {
    if disk.is_null() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let mut fs = alloc::boxed::Box::new(Ext4FileSystem::new(disk));
    // Superblock/group/inode-table bootstrap; a non-ext image (missing
    // magic — e.g. unformatted loop) fails here.
    fs.init()?;
    // No journal on ext2 scratch images; a journaled ext4 image would be
    // replayed by its own mkfs/umount cycle — skip replay on purpose.
    let fs_ptr = alloc::boxed::Box::into_raw(fs) as *mut Ext4FileSystem;
    // SAFETY: fs_ptr is a leaked, valid instance pointer.
    let fs_ref = unsafe { &*fs_ptr };
    match fs_ref.read_inode(2) {
        Ok(ext4_inode) => Ok(create_vfs_inode_in(fs_ptr, 2, &ext4_inode)),
        Err(_) => {
            // Not a filesystem root (bad magic / inode 2): fail the mount
            // and reclaim the instance.
            // SAFETY: reclaiming the just-leaked box.
            unsafe { drop(alloc::boxed::Box::from_raw(fs_ptr)) };
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }
}
