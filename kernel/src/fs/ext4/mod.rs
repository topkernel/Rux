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
pub mod csum;

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
    /// ENCRYPT is a fs CAPABILITY flag: its presence only means the fs
    /// supports per-file encryption — unencrypted files (all of them in
    /// the OH images) read and write identically. Linux mounts such a
    /// filesystem without complaint; refusing it with ENODEV blocked the
    /// stock OH userdata partition entirely (mount[11] -19, /data dead,
    /// the whole OH service mkdir storm failing with err=2/5).
    pub const KNOWN_INCOMPAT: u32 = incompat::FILETYPE | incompat::RECOVER
        | incompat::EXTENTS | incompat::B64BIT | incompat::FLEX_BG
        | incompat::ENCRYPT;

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
        | incompat::INLINE_DATA;

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
    /// metadata_csum (ro_compat 0x400): crc32c seed + verify on the read
    /// path. Set once during init(), read by every inode load.
    pub metadata_csum: bool,
    /// crc32c seed (crc32c(~0, s_uuid), or s_checksum_seed when present).
    pub csum_seed: u32,
    /// Mount is read-only. metadata_csum images only mount read-only —
    /// the write path does not maintain checksums yet.
    pub read_only: bool,
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
            metadata_csum: false,
            csum_seed: 0,
            read_only: false,
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
                // metadata_csum read+write contract (OH Phase 2): the read
                // path verifies { superblock, group descriptor, inode }
                // checksums; the write path now re-seals exactly those
                // three classes (inode in write_inode/write_inode_disk,
                // group descriptors in update_group_desc_free_{blocks,
                // inodes}, superblock in update_superblock_free_{blocks,
                // inodes}). A metadata_csum filesystem may therefore mount
                // read-write and stay self-consistent for THIS kernel's
                // reads.
                //
                // Scope note (bring-up): bitmap csums (bg_*_bitmap_csum in
                // the descriptors), extent-tree-block csums and directory
                // tail csums are still NOT maintained on write — the read
                // path never verifies them either, so runtime behavior is
                // unaffected, but e2fsck on a real device (writes that
                // persist past -snapshot) will flag those classes. This
                // is the deliberate price of unblocking the OH /data
                // lifecycle: without rw /data, init's early mkdir batch
                // fails EROFS and the whole /data/service tree (access
                // tokens, databases, render_service dirs) never exists.
                if ro_compat & features::ro_compat::METADATA_CSUM != 0 && !self.read_only {
                    crate::pr_warn!(
                        "ext4: metadata_csum rw mount enabled (write path seals sb/gdt/inode csums; bitmap/extent/dir-tail classes not maintained — bring-up scope)"
                    );
                }
                if ro_compat & features::ro_compat::BIGALLOC != 0 {
                    crate::pr_err!("ext4: bigalloc not supported — refusing mount");
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }

                // metadata_csum read-path setup: compute the fs seed and
                // verify the superblock checksum before trusting anything
                // else in it.
                if ro_compat & features::ro_compat::METADATA_CSUM != 0 {
                    self.metadata_csum = true;
                    self.csum_seed = csum::csum_seed(
                        &ext4_sb.s_uuid,
                        ext4_sb.s_checksum_seed,
                    );
                    let sb_raw = &sb_data[1024..1024 + 1024];
                    if !csum::superblock_csum_ok(sb_raw) {
                        crate::pr_err!("ext4: superblock checksum invalid — refusing mount");
                        bio::brelse(sb_bh);
                        return Err(errno::Errno::IOError.as_neg_i32()); // EIO
                    }
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

                // metadata_csum: verify each group descriptor against its
                // stored crc32c before trusting the geometry it carries
                // (a corrupt descriptor would send inode/block reads to
                // arbitrary disk locations).
                if self.metadata_csum
                    && !csum::group_desc_csum_ok(
                        self.csum_seed,
                        i as u32,
                        &gd_data[gd_offset..gd_offset + desc_size],
                    )
                {
                    crate::pr_err!(
                        "ext4: group {} descriptor checksum invalid — refusing mount",
                        i
                    );
                    bio::brelse(gd_bh);
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::IOError.as_neg_i32()); // EIO
                }

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
        crate::dfx::fsw::note_read_inode();
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
        // lo|hi<<32 like the inode.rs twins (review EXT4-64BIT): the
        // inode-table block number is a 64-bit value on a 64BIT fs — the
        // lo-only read sent every inode of a >2^32-block filesystem to the
        // wrong disk location.
        let inode_table_start =
            (gd.bg_inode_table_lo as u64) | ((gd.bg_inode_table_hi as u64) << 32);
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

        // Bounds check (read-side twin of the write paths' R20-FS2 clamp,
        // review EXT4-M1 refined): the parse is clamped to the ON-DISK
        // slot size (s_inode_size). Requiring the full 172-byte struct
        // made the LAST slot of every inode-table block read EIO on a
        // valid s_inode_size=128 filesystem — every lookup of such an
        // inode surfaced as ENOENT (the i128 boot: /bin/sh "not found").
        // All fields from_disk consumes live below offset 128, so a
        // clamped copy is complete for every consumer.
        let copy_len = core::cmp::min(
            core::mem::size_of::<inode::Ext4InodeOnDisk>(),
            self.inode_size as usize,
        );
        if inode_offset + copy_len > data.len() {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        // metadata_csum: verify the inode's crc32c over the FULL
        // inode_size bytes before parsing (Linux ext4_inode_csum_verify —
        // a mismatched inode reads as EIO).
        if self.metadata_csum {
            let full = (inode_offset + self.inode_size as usize).min(data.len());
            if !csum::inode_csum_ok(
                self.csum_seed,
                ino,
                &data[inode_offset..full],
            ) {
                bio::brelse(bh);
                return Err(errno::Errno::IOError.as_neg_i32()); // EIO
            }
        }

        // Parse inode (slot-clamped local copy; the struct tail stays
        // zeroed when s_inode_size < sizeof(Ext4InodeOnDisk)).
        let mut on_disk = inode::Ext4InodeOnDisk::default();
        {
            // SAFETY: copy_len bytes at data+inode_offset are within the
            // bounds-checked buffer; the destination is a fresh local.
            let dst = unsafe {
                core::slice::from_raw_parts_mut(
                    &mut on_disk as *mut _ as *mut u8,
                    copy_len,
                )
            };
            dst.copy_from_slice(&data[inode_offset..inode_offset + copy_len]);
        }
        let result = inode::Ext4Inode::from_disk(&on_disk, ino);

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
            crate::dfx::fsw::note_dir_lookup(blocks.len());

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
            crate::dfx::fsw::note_list_dir();

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
    mount_ext4_flags(device, false)
}

/// Mount with explicit read-only-ness: a metadata_csum filesystem only
/// mounts read-only (the write path does not maintain checksums yet);
/// the read path verifies superblock/group/inode crc32c (csum.rs).
pub fn mount_ext4_flags(
    device: *const blkdev::GenDisk,
    read_only: bool,
) -> Result<(), i32> {
    use core::sync::atomic::Ordering;

    if device.is_null() {
        return Err(-22); // EINVAL
    }

    // Create ext4 filesystem instance
    let mut fs = Box::new(Ext4FileSystem::new(device));
    fs.read_only = read_only;

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
                file::allocate_blocks_for_file(fs, &mut ext4_inode, needed_blocks, current_blocks)
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
    file::allocate_blocks_for_file(fs, &mut parent_inode, new_block_index + 1, new_block_index)?;

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
unsafe fn ext4_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    let fs_ptr = dir.private_data.ok_or(errno::Errno::IOError)?;
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from parent's private_data
    let parent_ext4_inode = dir.sb.ok_or(errno::Errno::NotADirectory)?;
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

    // st_dev = the filesystem instance's id (Linux s_dev): OH's
    // switch_root stats "/" and "/usr" and SKIPS the root switch when
    // they match — with st_dev 0 on both, the ramdisk root was never
    // replaced and exec("/bin/init") failed ENOENT.
    stat.st_dev = inode.fs_id;
    stat.st_ino = inode.ino;
    stat.st_mode = ext4_inode.mode as u32;  // u16 -> u32
    stat.st_size = ext4_inode.get_size() as i64;
    stat.st_nlink = ext4_inode.links_count as u32;
    stat.st_uid = ext4_inode.uid as u32;
    stat.st_gid = ext4_inode.gid as u32;
    // Device nodes: rdev is encoded in the on-disk i_block[0]
    // (ext4_iget: new_decode_dev(le32_to_cpu(raw_inode->i_block[0]))).
    stat.st_rdev = if ext4_inode.mode & 0o170000 == 0o020000
        || ext4_inode.mode & 0o170000 == 0o060000
    {
        let enc = ext4_inode.block[0];
        // Linux new_decode_dev() then MKDEV(): inverse of new_encode_dev
        let minor = (enc & 0xff) | ((enc >> 12) & 0xfff00);
        let major = (enc >> 8) & 0xfff;
        ((minor & 0xff) | (major << 8) | ((minor & !0xffu32) << 12)) as u64
    } else {
        0
    };
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

    // Session 25 (MAP_SHARED writeback): on a size change, flush dirty
    // page-cache frames BEFORE the big lock and the new size commit. The
    // flush takes EXT4_BIG_LOCK internally (ext4_write_at_core), so it
    // cannot run inside this function's own critical section. Flush-then-
    // cut: a dirty page straddling the new EOF writes its full content
    // first, then the cut removes the tail (Linux writeback-then-
    // truncate). A GROW used to skip this while the success tail still
    // called invalidate_inode on the WHOLE cache — dirty frames below the
    // old size (unflushed MAP_SHARED stores) were freed by the
    // invalidate, silently dropping the stores. Flush for every
    // ATTR_SIZE: the flush iterates dirty entries only, so a no-dirty
    // call costs nothing. Stores racing after the flush are discarded by
    // the invalidate below (POSIX: stores at truncate are undefined).
    if attr == setattr_attr::ATTR_SIZE {
        crate::fs::ext4::file::ext4_flush_dirty_inode(inode, 0, u64::MAX);
    }

    // ext4 has no internal concurrency protection on the block
    // allocator / inode writer (review 5.5 high: EXT4_BIG_LOCK 仅覆盖
    // namei): take the big lock for the whole setattr — truncate frees
    // blocks (bitmap RMW) and rewrites the inode.
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();

    // Every setattr flavor mutates on-disk state; a read-only mount
    // answers EROFS (Linux notify_change/mnt_want_write; sc21). atime
    // updates from reads call this too, but both callers discard the
    // result, so read paths on ro images are unaffected.
    let fs = match get_ext4_fs_rw_from_inode(inode) {
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
                            match indirect::ext4_get_block(fs, &ext4_inode.block, i as u64) {
                                Ok(block_num) if block_num != 0 => {
                                    let _ = allocator.free_block(block_num);
                                    // Clear the freed block's indirect slot: a
                                    // stale pointer outlives the free, and after
                                    // a later grow the hole range would resolve
                                    // to the old physical blocks — which the
                                    // allocator may have reused — serving
                                    // foreign bytes for reads and writing
                                    // through dead mappings (the reference
                                    // kernel clears each pointer as it frees,
                                    // ext4_free_branches).
                                    let _ = indirect::clear_indirect_entry(
                                        fs, &ext4_inode.block, i as u64,
                                    );
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

/// Deferred-delete queue: (filesystem, ino) pairs reaped by the next
/// ext4 entry point. ext4_destroy_inode runs in arbitrary Drop contexts
/// (close_fd, scope ends inside unrelated syscalls) where taking
/// EXT4_BIG_LOCK or issuing block I/O directly could invert lock order;
/// destroying only QUEUES the reap, and the wrappers below drain the
/// queue at their start under their own lock discipline.
static PENDING_REAPS: crate::sync::spinlock::Spinlock<
    alloc::vec::Vec<(&'static Ext4FileSystem, u32)>,
> = crate::sync::spinlock::Spinlock::new(alloc::vec::Vec::new());

/// Drain the deferred-delete queue. Called from ext4 entry points that
/// already serialize on EXT4_BIG_LOCK (unlink/create/mkdir wrappers);
/// each item is reaped under the big lock, one at a time.
fn flush_pending_reaps() {
    loop {
        let item = {
            let mut q = PENDING_REAPS.lock();
            match q.pop() {
                Some(it) => it,
                None => return,
            }
        };
        let _guard = EXT4_BIG_LOCK.lock_fair();
        let _ = namei::ext4_reap_unlinked(item.0, item.1);
    }
}

/// Ext4 destroy_inode: reclaim the Box<Ext4Inode> stored in inode.sb and
/// queue the POSIX deferred deletion.
///
/// The Drop that lands here is the last VFS reference — for an unlinked
/// file that means the last descriptor just closed (unlink keeps the
/// number/blocks until then; see ext4_unlink_inner). The reap itself is
/// deferred to the next ext4 entry point (PENDING_REAPS note above).
unsafe fn ext4_destroy_inode(inode: &mut crate::fs::inode::Inode) {
    if let Some(ptr) = inode.sb {
        // ptr was created by Box::into_raw(Box::new(ext4_inode.clone()))
        let _ = alloc::boxed::Box::from_raw(ptr as *mut inode::Ext4Inode);
        inode.sb = None;
    }
    if let Ok(fs) = get_ext4_fs_from_inode(inode) {
        // SAFETY: read_inode only touches the block layer under the big
        // lock taken below; queueing avoids I/O in this Drop context.
        unsafe {
            if let Ok(disk) = inode::read_inode(fs, inode.ino as u32) {
                if disk.i_links_count == 0 && disk.i_dtime == 0 {
                    let mut q = PENDING_REAPS.lock();
                    q.push((fs, inode.ino as u32));
                }
            }
        }
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
///
/// R84 (b139 fairness): wake-one alone is NOT a handoff. The woken
/// waiter must first be scheduled, and in that window any FRESH
/// fast-path caller (`!st.locked` → take) barged past it. Under KVM's
/// I/O concurrency every .so page fault re-enters ext4 through the page
/// cache, so the bargers never stopped: oh-b139 parked bootanimation /
/// sa_main / render_service for 10+ s (47 EXT4-BIGLOCK-STALL prints,
/// holder cycling at held=0j) while 10+ services sat UNINTERRUPTIBLE —
/// pure fairness starvation (nr_running=6 with idle CPU headroom), not
/// a deadlock. TCG boots flowed only because lower concurrency means
/// less barging. Fixed with Linux-mutex-HANDOFF semantics:
///  - `waiters` counts parked tasks plus unspent wake tokens;
///  - fresh SLEEPABLE callers are gated (`try_acquire_or_recurse_fair`):
///    a free lock is taken only when `waiters == 0`, so they queue;
///  - the woken waiter spends its token as ownership
///    (`try_acquire_wake_token`): unconditional take of the free lock,
///    count dropped in the same critical section;
///  - non-sleeping spinners (no task ctx / preempt_count != 0) stay
///    UNGATED — they cannot queue, so the queue can never serve them;
///  - a waiter parked longer than ANTI_STARVE_JIFFIES may take a
///    free lock at the registration re-check even with other waiters
///    queued: bounded-latency self-heal so the gate itself can never
///    strand a free lock behind a stuck count.
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

/// R77 (wedge hunt): is the recorded big-lock owner still a live task?
/// (pid-hash lookup; the stall tripwire only fires while the owner lives —
/// a DEAD owner means the lock leaked with its holder.)
fn locked_owner_alive_wait(owner: &u32) -> bool {
    if *owner == 0 {
        return false;
    }
    let t = crate::process::pid_hash::pid_hash_lookup_pinned(*owner);
    if t.is_null() {
        return false;
    }
    let alive = unsafe { !(*t).state().is_dead() };
    crate::process::task::Task::task_put(t);
    alive
}

struct Ext4BigState {
    locked: bool,
    owner: u32,
    depth: u32,
    /// R77 (wedge hunt): jiffies stamp of the outermost acquire — with the
    /// waiters' tripwire below it names a holder that never releases (a
    /// leaked lock after a synchronous kill inside the critical section
    /// parks every later ext4 user in D-state forever: the full3/inst-full1
    /// wedge shape).
    acquired_at: u64,
    /// R84 (b139 fairness): tasks parked on `wait` PLUS tasks holding an
    /// unspent release() wake token. A task enters the count when it
    /// first registers on the queue and leaves it in the same critical
    /// section in which it takes the lock (token spend or sole-waiter
    /// take). While nonzero, the fair fast path refuses fresh sleepable
    /// acquirers — see try_acquire_or_recurse_fair.
    waiters: u32,
    /// R85 (b148 silent crawl): orphaned-handoff epoch. release() hands
    /// the lock to ONE waiter via wake_up_one; when the queue is empty
    /// at that moment (every counted waiter is mid-loop between
    /// finish_wait and its next prepare_to_wait — the failed-token-spend
    /// re-register path) the wake is dropped and the lock sits FREE
    /// behind a nonzero `waiters` with nobody left to wake it. Each such
    /// drop bumps this epoch: a waiter whose registration snapshot of
    /// the epoch predates the bump may take the free lock at its
    /// re-check, so an orphaned handoff is recovered by the FIRST
    /// mid-loop waiter to re-register instead of stranding everyone
    /// until an incidental foreign wake.
    handoff_epoch: u64,
}

impl Ext4BigLock {
    /// R84 (b139 fairness): a waiter older than this may take a FREE
    /// lock at the registration re-check even with other waiters
    /// queued — the self-heal that bounds any residual gate pathology
    /// (stale count, token-holder races beyond the analysis) at a few
    /// seconds of extra wait instead of an unbounded wedge.
    const ANTI_STARVE_JIFFIES: u64 = crate::config::KERNEL_HZ as u64 * 5;

    /// R84: stall tripwire threshold on the WAITER's own queue age
    /// (first registration to now). Normal parking lasts milliseconds;
    /// 3 s queued means the handoff is not reaching this waiter.
    const STALL_WAITER_JIFFIES: u64 = crate::config::KERNEL_HZ as u64 * 3;

    const fn new() -> Self {
        Self {
            state: crate::sync::spinlock::Spinlock::new(Ext4BigState {
                locked: false,
                owner: 0,
                depth: 0,
                acquired_at: 0,
                waiters: 0,
                handoff_epoch: 0,
            }),
            wait: crate::process::wait::WaitQueueHead::new(),
        }
    }

    /// Try to take the lock / recurse into it, UNGATED: a free lock goes
    /// to whoever asks. Caller must NOT hold the state spinlock.
    ///
    /// R84 (b139 fairness): this is no longer the general acquire. It
    /// survives for exactly the callers the queue cannot serve:
    ///  - the non-sleeping spinner in lock_fair (no task context or
    ///    preempt_count != 0 — it cannot register on the waitqueue, so
    ///    gating it would livelock it behind a queue that never drains
    ///    under load; its section is preempt/IRQ-constrained and short);
    ///  - any path that must never be rejected for recursion reasons.
    /// Fresh sleepable callers use try_acquire_or_recurse_fair; the
    /// woken waiter uses try_acquire_wake_token.
    fn try_acquire_or_recurse(&'static self, pid: u32) -> Option<Ext4BigGuard> {
        let mut st = self.state.lock();
        if !st.locked {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::drivers::timer::get_jiffies();
            Some(Ext4BigGuard { lock: self })
        } else if st.owner == pid {
            st.depth += 1;
            Some(Ext4BigGuard { lock: self })
        } else {
            None
        }
    }

    /// R84 (b139 fairness): the acquire for fresh sleepable callers.
    /// Identical to try_acquire_or_recurse except a FREE lock is only
    /// taken when no waiter is parked and no wake token is outstanding
    /// (`waiters == 0`) — everyone else must queue. This is what stops
    /// the barging: before it, any task on another CPU could take the
    /// just-released lock during the scheduling window of the waiter
    /// that release() had woken, and under KVM's .so-page-fault churn
    /// the bargers never stopped (47 STALL prints, waiters aged 10+ s
    /// while holders cycled at held=0j).
    fn try_acquire_or_recurse_fair(&'static self, pid: u32) -> Option<Ext4BigGuard> {
        let mut st = self.state.lock();
        if !st.locked && st.waiters == 0 {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::drivers::timer::get_jiffies();
            Some(Ext4BigGuard { lock: self })
        } else if st.owner == pid {
            st.depth += 1;
            Some(Ext4BigGuard { lock: self })
        } else {
            None
        }
    }

    /// R84 (b139 fairness): spend the release() handoff token. The woken
    /// waiter takes a FREE lock unconditionally and leaves the waiter
    /// count in the same critical section — the count keeps covering the
    /// token holder for the whole wake-to-take window, which is what
    /// holds fresh callers at the gate until ownership has actually
    /// changed hands (Linux mutex HANDOFF, in miniature). A held lock or
    /// a lost race against another token holder returns None: the caller
    /// re-parks and keeps its count. Recursion is honored without
    /// touching the count (a counted task cannot be the owner — it has
    /// not taken the lock yet — so that branch is unreachable defensive
    /// code; the anti-starvation override heals any hypothetical stale
    /// count it would leave behind).
    fn try_acquire_wake_token(&'static self, pid: u32) -> Option<Ext4BigGuard> {
        let mut st = self.state.lock();
        if st.owner == pid {
            st.depth += 1;
            Some(Ext4BigGuard { lock: self })
        } else if !st.locked {
            if st.waiters > 0 {
                st.waiters -= 1;
            }
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::drivers::timer::get_jiffies();
            Some(Ext4BigGuard { lock: self })
        } else {
            None
        }
    }

    /// Acquire (sleep on contention, recursive per task).
    pub fn lock_fair(&'static self) -> Ext4BigGuard {
        let pid = crate::process::current_pid();
        crate::dfx::fsw::note_biglock();

        // Adaptive: a short spin first covers the common case of a holder
        // that is on-CPU and about to release (boot-time short sections
        // never touch the waitqueue at all). R84: gated so the spin
        // cannot barge past parked waiters either.
        for _ in 0..4 {
            if let Some(g) = self.try_acquire_or_recurse_fair(pid) {
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
        crate::dfx::fsw::note_biglock_contend();
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

        // R83b: baseline header snapshot for the waiter self-watch below.
        let mut wait_snap = crate::dfx::r83hunt::WatchSnap {
            ksp: 0, user_sp: 0, preempt: 0, pick: 0, claim: 0, flags: 0,
        };
        let mut wait_snap_set = false;

        // R84 (b139 fairness) waiter bookkeeping:
        //  counted      <=> st.waiters already includes us (we are on the
        //                   queue or hold an unspent wake token); the count
        //                   is entered once at first registration and left
        //                   in the same critical section as a take.
        //  woken_retry  <=> schedule() just returned: the release() that
        //                   woke us spent its one handoff token on us.
        //  first_park   <=> jiffies of our FIRST registration; re-parks
        //                   keep it so the anti-starvation override below
        //                   measures true queue age.
        //  timer_id     <=> R85: anti-starve deadline wake armed for the
        //                   CURRENT park (0 = none). A parked waiter has
        //                   no waker of its own when a handoff is dropped
        //                   (empty-queue release) or its token is lost —
        //                   without this timer the ANTI_STARVE override
        //                   below can never fire, because it is only
        //                   evaluated on a loop iteration and iterating
        //                   requires a wake. That was the b148 shape: a
        //                   free lock, dozens of UNINTERRUPTIBLE waiters,
        //                   zero STALL prints, progress only on incidental
        //                   foreign wakes.
        let mut counted = false;
        let mut woken_retry = false;
        let mut first_park = 0u64;
        let mut timer_id = 0u64;

        loop {
            if woken_retry {
                // Handoff: the free lock is ours if it is still free.
                // Losing it means another token holder won the race or a
                // spurious wake — fall through and re-park like any
                // contender (our count stays).
                if let Some(g) = self.try_acquire_wake_token(pid) {
                    counted = false;
                    return g;
                }
                woken_retry = false;
            }
            if let Some(g) = self.try_acquire_or_recurse_fair(pid) {
                return g;
            }
            // R83b (R74 producer hunt): every capture so far had the victim
            // parked in THIS wait when its ti_kernel_sp turned to foreign
            // data. ti_kernel_sp/ti_user_sp have NO legal writer while a
            // task sleeps here (their only writers are the task's own trap
            // exit / exec paths), so any change between two iterations of
            // this loop is the scribbler — reported with before/after
            // words, the full header, and the neighborhood, from inside
            // the exposure window itself.
            {
                let now_snap = crate::dfx::r83hunt::watch_snap(cur as *mut _);
                if !wait_snap_set {
                    wait_snap = now_snap;
                    wait_snap_set = true;
                } else if now_snap.ksp != wait_snap.ksp || now_snap.user_sp != wait_snap.user_sp {
                    crate::dfx::r83hunt::waitwatch_report(
                        cur as *mut _,
                        &wait_snap,
                        &now_snap,
                    );
                    wait_snap = now_snap;
                }
            }
            // Register on the wait queue BEFORE re-checking the state
            // (R7-B6b discipline): any release() that pairs with this
            // attempt finds us queued and hands its wake token to us.
            // EXCLUSIVE + tail insert => FIFO, one wake per release.
            self.wait.prepare_to_wait(cur, true, false);
            // R85: snapshot the orphaned-handoff epoch BEFORE the
            // registration critical section below — a release() that
            // dropped its wake (empty queue) between this read and the
            // re-check shows up as an epoch change, letting us take the
            // free lock it stranded instead of parking past it.
            let my_epoch = {
                let st = self.state.lock();
                st.handoff_epoch
            };
            // R7-B6b re-check, fairness-aware (R84): count ourselves under
            // the state lock — once; re-parks keep the count — then take
            // the free lock ONLY when no OTHER waiter/token exists
            // (`waiters == 1`: us). That covers the release that raced
            // our registration (its wake found an empty queue — nobody
            // else can ever hand the lock out, so we must). With older
            // waiters queued the pending handoff belongs to them and we
            // sleep like everyone else. The anti-starvation override
            // (parked > ANTI_STARVE_JIFFIES and the lock is free) is the
            // self-heal that guarantees the gate can never strand a free
            // lock behind a stuck count: it converts any residual
            // pathology into bounded latency instead of a wedge, and in
            // normal operation it never fires because tokens already
            // flow FIFO. R85 adds the epoch term: a handoff that was
            // DROPPED (empty-queue release bumped the epoch after we
            // registered) belongs to nobody — take the free lock now.
            {
                let now = crate::drivers::timer::get_jiffies();
                let mut st = self.state.lock();
                if !counted {
                    st.waiters += 1;
                    counted = true;
                    first_park = now;
                }
                let take = if st.owner == pid {
                    // Unreachable while parked (a counted waiter has not
                    // taken the lock, so it cannot own it), kept so
                    // recursion is honored on every path.
                    st.waiters -= 1;
                    counted = false;
                    st.depth += 1;
                    true
                } else if !st.locked
                    && (st.waiters == 1
                        || st.handoff_epoch != my_epoch
                        || now.wrapping_sub(first_park)
                            > Self::ANTI_STARVE_JIFFIES)
                {
                    st.waiters -= 1;
                    counted = false;
                    st.locked = true;
                    st.owner = pid;
                    st.depth = 1;
                    st.acquired_at = now;
                    true
                } else {
                    false
                };
                drop(st);
                if take {
                    self.wait.finish_wait(cur);
                    // A concurrent release()'s wake_up_one may have seen
                    // us queued+sleeping and enqueued us between the
                    // registration and the acquire — undo it (the
                    // per-class on_rq guards make it a no-op when we
                    // were not actually enqueued). Mirrors Semaphore::down.
                    crate::sched::dequeue_task(&*cur);
                    if timer_id != 0 && crate::timer::timer_pending(timer_id) {
                        crate::timer::del_timer(timer_id);
                    }
                    return Ext4BigGuard { lock: self };
                }
            }
            // Sleep until release() wakes us (UNINTERRUPTIBLE: ext4 ops
            // are not signal-interruptible and the guard must be carried
            // back out of the critical section).
            //
            // R77 (wedge hunt tripwire): a holder that never releases
            // (killed synchronously inside the critical section — RAII
            // never runs through do_exit) shows up as held growing. R84
            // (b139 fairness): a WAITER parked/retrying for many seconds
            // while holders cycle (held ~0) is the starvation shape the
            // fairness gate exists to kill. Normal parking lasts
            // milliseconds, so the tripwire only prints when EITHER the
            // waiter has been queued 3+ s (fairness starvation) or the
            // holder has held 10+ s (leak) — rate-limited to one line
            // per 10 s either way.
            //
            // R85 (b148): the old tripwire only reported a LIVE owner —
            // the stranded-LOCK-FREE shape (owner=0, waiters stuck,
            // dropped handoff) printed NOTHING while the whole OH cohort
            // crawled, which is exactly how b148 looked (zero STALL
            // prints, 120 s+ D-state waiters, free lock). Report that
            // shape too, same rate limit.
            {
                let now = crate::drivers::timer::get_jiffies();
                let (owner, depth, held, nwaiters, locked) = {
                    let st = self.state.lock();
                    (
                        st.owner,
                        st.depth,
                        now.saturating_sub(st.acquired_at),
                        st.waiters,
                        st.locked,
                    )
                };
                let waited = now.wrapping_sub(first_park);
                let stranded_free = !locked && nwaiters > 0 && waited >= Self::STALL_WAITER_JIFFIES;
                if (waited >= Self::STALL_WAITER_JIFFIES
                    || held >= crate::config::KERNEL_HZ as u64 * 10)
                    && (stranded_free || locked_owner_alive_wait(&owner))
                {
                    static LAST_WAIT_REPORT: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0);
                    let last = LAST_WAIT_REPORT.load(core::sync::atomic::Ordering::Relaxed);
                    if now.wrapping_sub(last) >= crate::config::KERNEL_HZ as u64 * 10
                        && LAST_WAIT_REPORT
                            .compare_exchange(
                                last,
                                now,
                                core::sync::atomic::Ordering::Relaxed,
                                core::sync::atomic::Ordering::Relaxed,
                            )
                            .is_ok()
                    {
                        let cur_pid = crate::process::current_pid();
                        if stranded_free {
                            crate::pr_err!(
                                "EXT4-BIGLOCK-STALL-FREE waiter={} owner=none waiters={} waited={}j (dropped handoff / stuck count)",
                                cur_pid,
                                nwaiters,
                                waited
                            );
                        } else {
                            crate::pr_err!(
                                "EXT4-BIGLOCK-STALL waiter={} holder={} depth={} held={}j waiters={} waited={}j",
                                cur_pid,
                                owner,
                                depth,
                                held,
                                nwaiters,
                                waited
                            );
                        }
                    }
                }
            }
            // R85: arm the anti-starve self-wake. schedule() below blocks
            // until SOMETHING wakes us; when the handoff was dropped (the
            // empty-queue release) or our token is stolen, that someone
            // does not exist — without this timer the override above can
            // never evaluate and a stranded waiter sleeps indefinitely
            // (re-armed on every re-park so a held lock is polled at
            // ANTI_STARVE granularity, not hot-looped). Timer-table full
            // (id 0) degrades to the old wake-dependent behavior.
            if timer_id != 0 {
                crate::timer::del_timer(timer_id);
                timer_id = 0;
            }
            timer_id = crate::timer::add_timer_wakeup(
                crate::drivers::timer::get_jiffies() + Self::ANTI_STARVE_JIFFIES,
                pid,
            );
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            if timer_id != 0 {
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
                timer_id = 0;
            }
            self.wait.finish_wait(cur);
            // Woken: the release() that issued this wake spent its handoff
            // token on us — spend it at the top of the loop. (R85: a timer
            // or foreign wake ALSO lands here with no token; the token
            // spend fails or succeeds on lock state alone, so a spurious
            // retry is harmless.)
            woken_retry = true;
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
        // Hand the lock to one waiter. R84 (b139 fairness): this wake is
        // a HANDOFF token, not a retry hint — the woken waiter takes the
        // free lock unconditionally in lock_fair, and
        // try_acquire_or_recurse_fair keeps fresh callers queued behind
        // it until the token is spent. Still done outside the state
        // spinlock: the waker takes the waitqueue lock, the waiter's
        // finish_wait takes it too — never both at once. Waking an empty
        // queue is a no-op (an outstanding token holder takes the free
        // lock on its own), so this is safe even without waiter
        // accounting.
        let woke = self.wait.wake_up_one();
        if woke == 0 {
            // R85 (b148 silent crawl): the handoff was DROPPED — the
            // queue was empty, so every counted waiter is mid-loop
            // between finish_wait and its next prepare_to_wait and the
            // just-freed lock has no taker: the fresh-caller gate
            // (waiters > 0) bars everyone else. Bump the orphaned-handoff
            // epoch so the first mid-loop waiter to re-register takes the
            // free lock at its re-check instead of parking past it. (No
            // counted waiters at all: the bump is inert — nobody's
            // registration snapshot can predate it.)
            let mut st = self.state.lock();
            if !st.locked && st.waiters > 0 {
                st.handoff_epoch += 1;
            }
        }
    }

    /// R73 exit-path release: force-drop the lock when its owner dies.
    ///
    /// do_exit() does not unwind Rust guards — a task killed while inside
    /// an ext4 critical section (signal during a slow TCG read, OOM kill,
    /// abort) leaks the big lock forever: every later ext4 op then spins
    /// or sleeps on a dead owner (GNOME r72-fire2: the dead owner's pid
    /// was even reused by a new shell while the lock still recorded it;
    /// 4 CPUs wedged in lock_fair for 15+ min while gs-b and Xorg
    /// starved). Called from do_exit only — the dying task can no longer
    /// make forward progress in its critical section, so dropping the
    /// lock is strictly better than stranding the whole filesystem.
    /// Hand out exactly ONE retry token, same as release(): the woken
    /// waiter either takes the lock (and its later release() hands the
    /// next token down the queue) or re-sleeps — a storm of woken
    /// retryers is pointless load on the scheduler.
    pub fn release_if_owner(&'static self, pid: u32) {
        let was_held = {
            let mut st = self.state.lock();
            if st.locked && st.owner == pid {
                if st.depth != 1 {
                    crate::pr_err!(
                        "ext4: big lock force-released at exit: pid {} depth {} (nested sections abandoned)",
                        pid,
                        st.depth
                    );
                }
                st.locked = false;
                st.owner = 0;
                st.depth = 0;
                true
            } else {
                false
            }
        };
        if was_held {
            let woke = self.wait.wake_up_one();
            if woke == 0 {
                // R85: same dropped-handoff recovery as release().
                let mut st = self.state.lock();
                if !st.locked && st.waiters > 0 {
                    st.handoff_epoch += 1;
                }
            }
        }
    }

    /// Diagnostic snapshot (VW forensic instrumentation): (locked, owner,
    /// depth, queued, woken-flagged). Called from the UART dump path (RX
    /// IRQ context) — uses TRY-locks only: a blocking lock here would
    /// self-deadlock against an interrupted holder on this CPU and wedge
    /// the dump itself. queued=usize::MAX means "list lock busy".
    pub fn vw_state(&'static self) -> (bool, u32, u32, usize, usize) {
        let mut locked = false;
        let mut owner = 0u32;
        let mut depth = 0u32;
        if let Some(st) = self.state.try_lock() {
            locked = st.locked;
            owner = st.owner;
            depth = st.depth;
        } else {
            owner = u32::MAX; // state lock busy marker
        }
        let (queued, woken) = if let Some((t, w)) = self.wait.vw_queue_census_try() {
            (t, w)
        } else {
            (usize::MAX, 0)
        };
        (locked, owner, depth, queued, woken)
    }

    /// R85 (b148 hunt): one SBI-console census line for the periodic
    /// taskdump — the b148 wedge was invisible because nothing printed
    /// the biglock's own state (zero STALL prints while every OH service
    /// sat D-parked). Try-locks only (taskdump context may be the
    /// watchdog); busy locks print as '?' fields, never block.
    pub fn dfx_census_line(&'static self) {
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        let (locked, owner, depth, waiters, epoch) = match self.state.try_lock() {
            Some(st) => (st.locked, st.owner, st.depth, st.waiters, st.handoff_epoch),
            None => (false, u32::MAX, 0, u32::MAX, u64::MAX),
        };
        let (queued, woken) = match self.wait.vw_queue_census_try() {
            Some(v) => v,
            None => (usize::MAX, 0),
        };
        taskdump_raw_line(b"EXT4-BIGLOCK-CENSUS locked=");
        taskdump_dec(locked as u64);
        taskdump_raw_line(b" owner=");
        taskdump_dec(owner as u64);
        taskdump_raw_line(b" depth=");
        taskdump_dec(depth as u64);
        taskdump_raw_line(b" waiters=");
        taskdump_dec(waiters as u64);
        taskdump_raw_line(b" epoch=");
        taskdump_dec(epoch);
        taskdump_raw_line(b" queued=");
        taskdump_dec(queued as u64);
        taskdump_raw_line(b" wokenflag=");
        taskdump_dec(woken as u64);
        taskdump_raw_line(b"\n");
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
    mknod: Some(ext4_mknod_wrapper),
    rename: Some(ext4_rename_wrapper),
    readlink: Some(ext4_readlink),
    get_file_ops: Some(ext4_get_file_ops),
    readdir: Some(ext4_readdir),
    readdir_cacheable: true,
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
unsafe fn ext4_iget(parent: &Inode, _name: &[u8], ino: Ino) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
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
unsafe fn ext4_mkdir_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    flush_pending_reaps();
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_rw_from_inode(dir)?;

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
    let fs = match get_ext4_fs_rw_from_inode(dir) {
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
unsafe fn ext4_create_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    flush_pending_reaps();
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_rw_from_inode(dir)?;
    let new_ino = namei::ext4_create(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs as *const Ext4FileSystem as *mut Ext4FileSystem, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_mknod to match VFS signature (device/FIFO nodes).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn ext4_mknod_wrapper(dir: &Inode, name: &[u8], mode: InodeMode, dev: u64) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_rw_from_inode(dir)?;
    let new_ino = namei::ext4_mknod(fs, dir.ino as u32, name, mode.bits() as u16, dev)?;

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
unsafe fn ext4_symlink_wrapper(dir: &Inode, name: &[u8], target: &[u8]) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = get_ext4_fs_rw_from_inode(dir)?;
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
    let fs = match get_ext4_fs_rw_from_inode(dir) {
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
    flush_pending_reaps();
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let fs = match get_ext4_fs_rw_from_inode(dir) {
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
    let fs = match get_ext4_fs_rw_from_inode(old_dir) {
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

/// Mutating-op variant of get_ext4_fs_from_inode: a read-only-mounted ext4
/// refuses writes with EROFS, the way Linux does (may_create checks
/// IS_RDONLY(dir), ext4_new_inode its own sb_rdonly; sc21). The guard must
/// sit at the op WRAPPERS because binds SHARE inodes with the original
/// mount — check_not_readonly only sees the bind's own flags, and without
/// the fs-level refusal a mkdir/create under a bind of the ro /usr or
/// /vendor ext4 ran the full write path (bitmap RMW, superblock counter,
/// journal) and failed deep inside with EIO, mis-reporting the errno
/// (sc20: sandbox prep mkdirs under /mnt/sandbox/system returned -5).
fn get_ext4_fs_rw_from_inode(inode: &Inode) -> Result<&'static Ext4FileSystem, i32> {
    let fs = get_ext4_fs_from_inode(inode)?;
    if fs.read_only {
        return Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32());
    }
    Ok(fs)
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
    // Keep the full S_IALLUGO (0o7777: setuid/setgid/sticky included)
    // for dirs and regs, matching the chmod path (update_mode's
    // v & 0o7777 below). Masking to 0o777 silently dropped S_ISUID on
    // lookup: dbus-daemon's activation check stats its setuid launch
    // helper and refused it ("The permission of the setuid helper is
    // not correct" for every helper-activated system service —
    // org.freedesktop.Accounts included) because stat reported 0754
    // where the image carries 04754 (gdm bring-up gdmr18). Symlinks
    // stay 0777 (Linux reports them so).
    let mode = if ext4_inode.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | (ext4_inode.mode as u32 & 0o7777))
    } else if ext4_inode.is_symlink() {
        InodeMode::new(InodeMode::S_IFLNK | 0o777)
    } else if ext4_inode.is_reg() {
        InodeMode::new(InodeMode::S_IFREG | (ext4_inode.mode as u32 & 0o7777))
    } else {
        InodeMode::new(ext4_inode.mode as u32)
    };

    let mut inode = Inode::new(ino as u64, mode);
    // Set fs_id to the filesystem pointer address for cache uniqueness
    inode.fs_id = fs_ptr as u64;
    inode.uid.store(ext4_inode.uid as u32, core::sync::atomic::Ordering::Relaxed);
    inode.gid.store(ext4_inode.gid as u32, core::sync::atomic::Ordering::Relaxed);
    inode.size.store(ext4_inode.size, core::sync::atomic::Ordering::Relaxed);
    // Seed the chattr view: on-disk i_flags shares bit values with the
    // FS_*_FL UAPI words (EXT4_IMMUTABLE_FL == FS_IMMUTABLE_FL == 0x10,
    // ...). Mask to the user-visible set so internal bits (EXTENTS etc.)
    // never leak through FS_IOC_GETFLAGS.
    inode.ioc_flags.store(
        ext4_inode.flags & crate::fs::inode::FS_FL_USER_VISIBLE,
        core::sync::atomic::Ordering::Relaxed,
    );
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
    read_only: bool,
) -> Result<(alloc::sync::Arc<Inode>, bool), i32> {
    if disk.is_null() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    let _ext4_guard = EXT4_BIG_LOCK.lock_fair();
    let mut fs = alloc::boxed::Box::new(Ext4FileSystem::new(disk));
    fs.read_only = read_only;
    // Superblock/group/inode-table bootstrap; a non-ext image (missing
    // magic — e.g. unformatted loop) fails here.
    fs.init()?;
    // Dirty-journal replay for rw mounts (OH userdata lineage): those
    // images boot with s_start != 0 (an unclean shutdown froze mid-
    // transaction). Building allocator state on the half-applied result
    // and then writing would corrupt the live metadata. ext2 scratch
    // images and clean journals take the no-op path inside init_journal
    // (s_start == 0 skips recovery); read-only mounts skip entirely —
    // replay WRITES, and ro is how system/vendor/sys_prod mount.
    //
    // After the one-shot replay the journal handle is DROPPED:
    // subsequent metadata writes keep this driver's direct-write
    // discipline (buffer-cache dirty + inline sync), which the
    // allocator's F4 notes and the journal-revocation comments are
    // built around. The next mount sees the clean journal superblock
    // the replay wrote (write_clean_sb: s_start = 0), so this stays
    // idempotent across boots.
    if !fs.read_only {
        if let Err(e) = fs.init_journal() {
            crate::pr_debug!("ext4: loop-instance journal init failed: {}", e);
        }
        fs.journal = None;
    }
    let fs_ptr = alloc::boxed::Box::into_raw(fs) as *mut Ext4FileSystem;
    // SAFETY: fs_ptr is a leaked, valid instance pointer.
    let fs_ref = unsafe { &*fs_ptr };
    match fs_ref.read_inode(2) {
        Ok(ext4_inode) => Ok((create_vfs_inode_in(fs_ptr, 2, &ext4_inode), fs_ref.read_only)),
        Err(_) => {
            // Not a filesystem root (bad magic / inode 2): fail the mount
            // and reclaim the instance.
            // SAFETY: reclaiming the just-leaked box.
            unsafe { drop(alloc::boxed::Box::from_raw(fs_ptr)) };
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }
}
