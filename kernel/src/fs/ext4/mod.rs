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

// (residue rows: ext4_kill_sb rides the fs-type raw ABI
// (INV-SBTYPE-1, superblock.rs); read_file_internal and
// create_vfs_inode_in ride INV-EXT4-GLOBALFS/SBRES-1 provenance;
// ext4_instance_from_id rides the registry's liveness authority.)
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
pub mod htree;

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

/// The filesystem's dev_t for the ext4 trace events: (major << 20) |
/// first_minor — the MKDEV packing the reference's MAJOR()/MINOR()
/// printk expansion decodes (the block events' dev column convention).
#[inline]
pub(crate) fn dev_id(fs: &Ext4FileSystem) -> u32 {
    (fs.device.major << 20) | fs.device.first_minor
}

/// INV-EXT-VIEW twin for the on-disk superblock: the single cast point,
/// bounds-guarded. The 4 KiB block-0 read always covers 1024 + the struct.
fn superblock_on_disk(disk: &[u8]) -> &superblock::Ext4SuperBlockOnDisk {
    let view = &disk[1024..];
    assert!(
        view.len() >= core::mem::size_of::<superblock::Ext4SuperBlockOnDisk>(),
        "block-0 read too small for the on-disk superblock"
    );
    // The shared header-view edge (INH-BUF-3 shape): view covers at
    // least the struct; #[repr(C)] integer fields, unaligned-safe loads.
    crate::net::buffer::hdr_view::<superblock::Ext4SuperBlockOnDisk>(view).unwrap_or_else(|| unreachable!())
}

pub struct Ext4FileSystem {
    /// Block device (typed handle; INV-BLKDEV-1 on blkdev::get_disk)
    pub device: &'static blkdev::GenDisk,
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
    /// HTREE name-hash seed (s_hash_seed, sb offset 0xEC): dx_probe feeds
    /// it to the half-md4/TEA name hash. Populated at mount.
    pub hash_seed: [u32; 4],
    /// s_flags unsigned-hash flag (0x0002): dx_probe bumps the SIGNED
    /// hash base version by +3 when set.
    pub hash_unsigned: bool,
    /// Directory data-block-list memo (22nd, miss-path lever): keyed
    /// (ino, i_size). A directory's block LIST only changes when its size
    /// crosses a block boundary (entry insert into existing slack patches
    /// rec_len in place — the list is unchanged), so (ino, size) is a
    /// sound validity key. Serves lookup()/list_dir() without the
    /// per-block extent walk train. Bounded, replace-oldest.
    pub dir_blocks_memo: Spinlock<alloc::vec::Vec<(u32, u64, Vec<u64>)>>,
    /// Mount is read-only. metadata_csum images only mount read-only —
    /// the write path does not maintain checksums yet.
    pub read_only: bool,
}

impl Ext4FileSystem {
    /// Create new ext4 filesystem instance
    pub fn new(device: &'static blkdev::GenDisk) -> Self {
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
            hash_seed: [0; 4],
            hash_unsigned: false,
            dir_blocks_memo: Spinlock::new(Vec::new()),
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

    /// Estimated free blocks of the whole fs: the sum of the in-memory
    /// group-descriptor free counts (falloc-wedge lane, 2026-11-16).
    /// The delayed-allocation write path defers block allocation to
    /// writeback, so a full filesystem kept ACCEPTING writes — pages
    /// piled up dirty, their flushes failed -ENOSPC forever, and an
    /// ENOSPC fill (LTP tst_fill_fs) never terminated (the fallocate05
    /// timeout face; FLUSH-FAIL wrote=-28 census, run/f05h). The
    /// reference kernel's eager-allocation fses surface ENOSPC at
    /// write(); the write path uses this estimate for the same contract
    /// (extending write on a zero-free fs fails before dirtying).
    /// ESTIMATE: lo-word only (64BIT fses carry hi words — a group with
    /// free % 65536 == 0 undercounts; the consumers only test ==0, and
    /// the allocator's own u16 counters saturate the same way), and it
    /// may lag concurrent allocation by the group_descs lock hold — a
    /// false PASS costs a flush-time ENOSPC (the delayed latitude), a
    /// false FAIL needs every group at 0 while blocks exist.
    pub fn est_free_blocks(&self) -> u64 {
        let descs = self.group_descs.lock();
        descs
            .iter()
            .map(|d| d.bg_free_blocks_count_lo as u64)
            .sum()
    }

    /// Initialize ext4 filesystem
    ///
    /// Read superblock and block group descriptors
    pub fn init(&mut self) -> Result<(), i32> {
        // SAFETY: self.device is a valid GenDisk pointer set during filesystem creation;
        // bio::bread reads a block and returns a valid BufferHead, and the superblock
        // is at a known fixed offset (1024 bytes into block 0).
        {
            // Read superblock
            // ext4 superblock is at byte offset 1024
            // - For 1KB blocks: superblock at start of block 1
            // - For 2KB+ blocks: superblock at offset 1024 within block 0
            // Since we use 4KB block cache, read block 0 and access offset 1024
            let sb_bh = bio::bread(self.device, 0)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let sb_data = bio::bh_data(&sb_bh);
            // Superblock is at 1024 byte offset within block
            // (unsafe-slim r3) typed view via superblock_on_disk below
            // (bounds-guarded; same INV-EXT-VIEW discipline as extent.rs).
            let ext4_sb = superblock_on_disk(sb_data);

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
                        | features::ro_compat::BTREE_DIR
                        // 0x10000 (e4defrag/shared_blocks): read-compatible
                        // (noble-desktop rootfs mkfs sets it; 2026-11-13)
                        | 0x10000);
                if unknown_ro != 0 {
                    crate::pr_err!("ext4: unknown ro_compat features {:#x} — refusing mount", unknown_ro);
                    bio::brelse(sb_bh);
                    return Err(errno::Errno::NoSuchDevice.as_neg_i32()); // ENODEV
                }
                // We mount read-write and maintain ALL metadata checksum
                // classes (csum.rs: superblock/group-descriptor/inode are
                // re-sealed by the *_csum_set wiring; bitmap csums,
                // directory tails and extent tails are sealed at their
                // write sites — oh-svc 2026-10-31). metadata_csum mounts
                // READ-WRITE with full read-path verification (csum.rs —
                // the OH system/vendor images are mkfs'd with
                // metadata_csum); bigalloc stays rejected outright.
                //
                // History (OH bring-up): this used to DOWNGRADE every rw
                // metadata_csum mount to read-only because the write path
                // did not maintain checksums. OH's fstab mounts /data rw;
                // the downgrade made that mount fail (EIO from the
                // geometry guard upstream) and the entire first-boot
                // /data cascade — native token file, persist params,
                // el1/el2 service dirs — ran against an unmounted /data
                // on every boot.
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

                let gd_data = bio::bh_data(&gd_bh);
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
                {
                    // (unsafe-slim r3) slice-indexed source; the one raw
                    // copy moves copy_len bytes into the packed struct.
                    let src_bytes = &gd_data[gd_offset..gd_offset + copy_len];
                    // The packed on-disk group descriptor as a value
                    // (CopyBytes owns the byte-image read).
                    if let Some(g) = crate::safety::bytes::from_bytes::<superblock::Ext4GroupDesc>(src_bytes) {
                        gd = g;
                    }
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
            // HTREE hash parameters (dx_probe's inputs): s_hash_seed at
            // sb offset 0xEC and the s_flags unsigned-hash bit 0x0002.
            self.hash_seed = ext4_sb.s_hash_seed;
            self.hash_unsigned = ext4_sb.s_flags & 0x0002 != 0;
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

        // Degenerate-geometry short-circuit (msync panic backstop twin
        // of inode_table_block_of): an instance whose init() never ran
        // has inodes_per_group == 0 and would panic on the divide
        // below. EIO, not a panic — init() rejects such superblocks,
        // so this never fires on a real filesystem.
        if self.inodes_per_group == 0 {
            return Err(errno::Errno::IOError.as_neg_i32());
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

        let data = bio::bh_data(&bh);

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
        // The on-disk inode as a value (CopyBytes owns the read; the
        // struct tail stays zeroed when copy_len < sizeof).
        let mut on_disk = inode::Ext4InodeOnDisk::default();
        {
            let src = &data[inode_offset..inode_offset + copy_len];
            let dst = crate::safety::bytes::bytes_of_mut(&mut on_disk);
            dst[..copy_len].copy_from_slice(src);
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
    /// Directory data-block-list memo helpers (22nd). Key (ino, size);
    /// bounded (32 entries, replace-oldest).
    fn dir_blocks_cached(&self, dir: &inode::Ext4Inode) -> Option<Vec<u64>> {
        let memo = self.dir_blocks_memo.lock();
        for (ino, size, blocks) in memo.iter() {
            if *ino == dir.ino && *size == dir.size {
                return Some(blocks.clone());
            }
        }
        None
    }

    fn dir_blocks_store(&self, dir: &inode::Ext4Inode, blocks: Vec<u64>) {
        const DIR_BLOCKS_MEMO_CAP: usize = 32;
        let mut memo = self.dir_blocks_memo.lock();
        for (ino, size, stored) in memo.iter_mut() {
            if *ino == dir.ino && *size == dir.size {
                *stored = blocks;
                return;
            }
        }
        if memo.len() >= DIR_BLOCKS_MEMO_CAP {
            memo.remove(0);
        }
        memo.push((dir.ino, dir.size, blocks));
    }

    /// Drop every memo entry for an inode (23rd soundness fix). The
    /// (ino, size) key alone does not protect against inode REUSE: a
    /// deleted directory's ino reallocated to a new directory of equal
    /// size would be served the old block list. Every ext4 entry
    /// mutation purges the parent here (add/delete entry sites in
    /// namei.rs, under ENTRY_MUTEX) and namei's free_inode purges the
    /// freed inode itself, so a stale list can never be served.
    pub fn dir_blocks_purge(&self, ino: u32) {
        let mut memo = self.dir_blocks_memo.lock();
        memo.retain(|(i, _, _)| *i != ino);
    }

    pub fn lookup(&self, dir: &inode::Ext4Inode, name: &str) -> Result<dir::Ext4DirEntry, i32> {
        // HTREE fast path (22nd, miss-path lever): indexed directories
        // resolve through the dx tree in 2-3 block reads instead of the
        // full linear scan. The port (htree.rs) follows the reference's
        // dx_probe/ext4_dx_find_entry bit-for-bit and returns None on any
        // anomaly (not indexed / corrupt index / unsupported hash), in
        // which case the linear scan below runs — the reference's
        // ERR_BAD_DX_DIR fallback contract. Our writer refuses indexed
        // dirs, so the index can never go stale through us.
        match htree::dx_find_entry(self, dir, name) {
            Some(r) => return r,
            None => {}
        }
        // SAFETY: self.device is a valid GenDisk pointer; dir.get_data_blocks returns
        // valid block numbers; bio::bread returns a valid BufferHead for each block.
        // Traverse directory's data blocks — through the block-list
        // memo when warm (22nd): the per-block extent walk train
        // (Vec + one extent lookup per block) ran on EVERY lookup;
        // big dirs paid it per miss.
        let blocks = match self.dir_blocks_cached(dir) {
            Some(b) => b,
            None => {
                let b = dir.get_data_blocks(self)?;
                self.dir_blocks_store(dir, b.clone());
                b
            }
        };
        crate::dfx::fsw::note_dir_lookup(blocks.len());

        // Pathology probe (oh-svc cook1/cook2 wedge): a directory scan
        // whose block list is absurdly large means a corrupt inode
        // (i_size/extent garbage) turned lookup into a multi-minute
        // bread+parse loop that monopolizes the CPU under the bucket
        // locks. Name the inode before we enter the loop so the boot
        // log pinpoints which directory went bad. A real directory of
        // 1024+ blocks (4MiB of dirents) never exists in these images.
        if blocks.len() > 1024 {
            crate::pr_err!(
                "ext4: PATHOLOGICAL DIR LOOKUP dir ino={} size={} blocks={} blocks_field={} name={:?}",
                dir.ino,
                dir.size,
                blocks.len(),
                dir.blocks,
                name
            );
        }

        for block in blocks.iter() {
            if *block == 0 {
                continue;
            }
            let bh = bio::bread(self.device, *block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = bio::bh_data(&bh);
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
                    // Non-UTF8 (corrupt/binary) name: SKIP the entry but
                    // still advance — a bare `continue` here retried the
                    // SAME offset forever and livelocked the walker (the
                    // OH userdata carries such entries; two vCPUs spun
                    // in from_bytes with no serial output).
                    Err(_) => {
                        offset += entry.rec_len as usize;
                        continue;
                    }
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
        let mut entries = Vec::new();

        // Traverse directory's data blocks (block-list memo, 22nd —
        // see lookup()).
        let blocks = match self.dir_blocks_cached(dir) {
            Some(b) => b,
            None => {
                let b = dir.get_data_blocks(self)?;
                self.dir_blocks_store(dir, b.clone());
                b
            }
        };
        crate::dfx::fsw::note_list_dir();
        // Pathology probe — see the twin in lookup().
        if blocks.len() > 1024 {
            crate::pr_err!(
                "ext4: PATHOLOGICAL DIR LIST dir ino={} size={} blocks={} blocks_field={}",
                dir.ino,
                dir.size,
                blocks.len(),
                dir.blocks
            );
        }

        for block in blocks.iter() {
            if *block == 0 {
                continue;
            }
            let bh = bio::bread(self.device, *block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = bio::bh_data(&bh);
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
                    // Non-UTF8 name: skip but ADVANCE (see lookup — a
                    // bare continue livelocked on the same offset).
                    Err(_) => {
                        offset += entry.rec_len as usize;
                        continue;
                    }
                };
                if name != "." && name != ".." {
                    // (launcher-errno lane) Degrade, never panic: a
                    // huge directory (the V0 vehicle's Linux-written
                    // trees carry 100K+ entry dirs) must not reach
                    // handle_alloc_error on the whole-dir Vec — the
                    // le5 V0 boot died exactly there (ALLOCTHROW
                    // 4 MiB inside ext4_readdir, kernel panic at
                    // t=40 s). On reservation failure return the
                    // PARTIAL list; the caller's pos-based walk
                    // serves what was collected.
                    if entries.try_reserve_exact(1).is_err() {
                        crate::pr_err!(
                            "ext4: list_dir partial: ino={} entries collected={} (heap budget)",
                            dir.ino,
                            entries.len()
                        );
                        return Ok(entries);
                    }
                    entries.push(entry.clone());
                }

                offset += entry.rec_len as usize;
            }

            bio::brelse(bh);
        }

        Ok(entries)
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
        if size <= 60 && inode.blocks == 0 {
            let all = crate::safety::bytes::bytes_of(&inode.block);
            return Ok(String::from_utf8_lossy(&all[..size]).into_owned());
        }

        // Slow symlink: target stored in data blocks
        let blocks = inode.get_data_blocks(self)?;
        let mut target = String::new();

        for block in blocks {
            // SAFETY: self.device is a valid GenDisk pointer; block numbers come from
            // get_data_blocks; bio::bread returns a valid BufferHead.
            {
                let bh = bio::bread(self.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;
                let data = bio::bh_data(&bh);

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
fn ext4_mount(fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    crate::pr_info!("ext4: mounting...");

    // Get source device
    let _source = fc.source.ok_or(-2_i32)?;  // ENOENT

    // TODO: Get block device from source
    // Simplified implementation: assume device is already registered
    // Need to implement device name to device mapping

    // This legacy extern mount entry never resolved a device (the old
    // body fabricated Ext4FileSystem::new(null), which could not pass
    // init — the real mounts go through mount_ext4_flags /
    // mount_loop_instance with a typed &'static GenDisk handle).
    // Refuse instead of fabricating a device-less instance.
    Err(-6) // ENXIO
}

/// Kill an ext4 superblock during unmount. Called from VFS umount path.
// SAFETY: sb is a valid SuperBlock pointer from a previous ext4_mount call;
// s_fs_info contains a valid Ext4FileSystem pointer from Box::into_raw.
fn ext4_kill_sb(sb: *mut SuperBlock) {
    // SAFETY: sb is a valid SuperBlock from a previous ext4_mount call;
    // s_fs_info carries the Ext4FileSystem Box::into_raw pointer. The
    // unmount path is the sole remaining user (mount-installed globals
    // are torn down first).
    unsafe {
        if let Some(fs_info) = (*sb).s_fs_info {
            // Unregister BEFORE the free (the writeback resolver's
            // liveness authority).
            ext4_instance_unregister(fs_info as *mut Ext4FileSystem);
            let _fs = Box::from_raw(fs_info as *mut Ext4FileSystem);
        }
        let _sb = Box::from_raw(sb);
    }
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
pub fn read_file(device: &'static blkdev::GenDisk, path: &str) -> Option<Vec<u8>> {
    read_file_internal(device, path, 0)
}

/// Internal implementation, supports recursion depth limit to prevent circular symbolic links
fn read_file_internal(device: &'static blkdev::GenDisk, path: &str, depth: u32) -> Option<Vec<u8>> {
    use alloc::vec::Vec;

    // Prevent circular symbolic links, max recursion depth 8
    if depth > 8 {
        return None;
    }

    // SAFETY: device is a valid GenDisk pointer from the block device layer;
    // Box::into_raw leaks the Box to make a static-lifetime pointer stored in
    // GLOBAL_EXT4_FS; this is called once during mount and cleaned up on unmount.
    {
        // The boot init loader runs before the VFS mount — and the fresh
        // instance's uninitialized extent walk mis-mapped the noble rootfs's
        // indexed directory extents (/sbin lookup ENOENTed while the
        // post-mount VFS resolved). Route through the MOUNTED instance
        // (whose lookup_path walk the VFS uses) when one exists; the fresh
        // fallback stays for the pre-mount initramfs case.
        let g = GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire);
        if !g.is_null() {
            let fs_ref: &Ext4FileSystem = unsafe { &*g };
            return match fs_ref.lookup_path(path) {
                Ok((_ino, inode)) => {
                    let size = inode.get_size() as usize;
                    let mut buffer = alloc::vec![0u8; size];
                    match inode.read_data(fs_ref, 0, &mut buffer) {
                        Ok(n) if n == size => Some(buffer),
                        _ => None,
                    }
                }
                Err(_) => None,
            };
        }
        // Create ext4 filesystem instance (pre-mount fallback)
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
    if size <= 60 && inode.blocks == 0 {
        // Read directly from block array
        let block_data = crate::safety::bytes::bytes_of(&inode.block);
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

// ============================================================================
// Live-instance registry (writeback inode resolution)
// ============================================================================
//
// The page cache identifies a file by `(fs_id, ino)`, and for ext4-backed
// entries `fs_id` IS the `Ext4FileSystem` instance pointer (that is the
// identity `ext4_buffered_write` and `writeback_populate_range` use). The
// writeback path therefore has to turn that pointer back into a usable
// instance — and it must NOT trust it blindly: `unmount_ext4` FREES the
// instance box, so a page-cache entry that outlived its filesystem would
// be a use-after-free.
//
// This registry is the liveness authority: an instance is registered when
// it is mounted and unregistered BEFORE its box is freed.

const EXT4_INSTANCE_SLOTS: usize = 8;
static EXT4_INSTANCES: [core::sync::atomic::AtomicPtr<Ext4FileSystem>; EXT4_INSTANCE_SLOTS] =
    [const { core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()) }; EXT4_INSTANCE_SLOTS];

/// Register a live ext4 instance (called right after the instance is
/// created and its pointer published).
pub fn ext4_instance_register(fs_ptr: *mut Ext4FileSystem) {
    use core::sync::atomic::Ordering;
    if fs_ptr.is_null() {
        return;
    }
    for slot in EXT4_INSTANCES.iter() {
        if slot.load(Ordering::Acquire).is_null() {
            slot.store(fs_ptr, Ordering::Release);
            return;
        }
    }
    // Registry full: the instance stays usable through the pointer paths
    // (the mount table keeps it alive); the writeback resolver will simply
    // refuse to materialize an inode for it rather than risk a stale
    // dereference.
    crate::pr_warn!("ext4: instance registry full ({} slots)", EXT4_INSTANCE_SLOTS);
}

/// Unregister an instance BEFORE its box is freed.
pub fn ext4_instance_unregister(fs_ptr: *mut Ext4FileSystem) {
    use core::sync::atomic::Ordering;
    for slot in EXT4_INSTANCES.iter() {
        if slot.load(Ordering::Acquire) == fs_ptr {
            slot.store(core::ptr::null_mut(), Ordering::Release);
            return;
        }
    }
}

/// Resolve a page-cache `fs_id` (the instance pointer) to a LIVE instance.
///
/// Returns None for an id that is not a registered instance — a stale
/// page-cache entry must never dereference a freed filesystem.
pub fn ext4_instance_from_id(fs_id: u64) -> Option<&'static Ext4FileSystem> {
    use core::sync::atomic::Ordering;
    if fs_id == 0 {
        return None;
    }
    for slot in EXT4_INSTANCES.iter() {
        let p = slot.load(Ordering::Acquire);
        if !p.is_null() && p as u64 == fs_id {
            // SAFETY: the registry entry is published only for instances
            // whose Box was leaked (static lifetime) and is cleared before
            // any free, so the pointer stays valid while it is listed.
            return Some(unsafe { &*p });
        }
    }
    None
}

/// Resolve the VFS inode a page-cache writeback target refers to.
///
/// THE sync(2) DATA-LOSS FIX (data-integrity lane, 2026-11-11): the old
/// contract was "resolve through the icache; a miss means skip". The
/// icache is a BOUNDED, DIRECT-MAPPED cache (1024 single-slot buckets): an
/// inode evicted from it — or overwritten by a hash collision — was silently
/// never flushed, and its dirty frames were lost at the next crash. The
/// measured victim: an 8 MB file whose 2048 dirty pages were COLLECTED by
/// the sync-time scan and then SKIPPED ("miss = skip"), so neither the data
/// nor the size update ever reached the device.
///
/// Resolution order:
/// 1. the icache (the live VFS inode — its cached size is the writer's
///    published EOF), and
/// 2. otherwise materialize a fresh VFS inode from the ON-DISK inode.
///    `create_vfs_inode_in` lifts that copy's size to the page cache's
///    dirty extent, so the flush still sees the true EOF and does not
///    discard the frames as "past EOF".
///
/// The writeback path must never depend on a cache for correctness: losing
/// an inode from a cache may cost performance, never data.
pub fn resolve_writeback_inode(
    fs_id: u64,
    ino: u64,
) -> Option<alloc::sync::Arc<crate::fs::inode::Inode>> {
    if let Some(inode) = crate::fs::inode::icache_lookup(ino, fs_id) {
        return Some(inode);
    }
    let fs = ext4_instance_from_id(fs_id)?;
    // `fs` is a live registered instance (the registry is the liveness
    // authority); read_inode takes a shared reference.
    let ext4_inode = fs.read_inode(ino as u32).ok()?;
    // The materialized copy is NOT inserted into the icache: it is a
    // writeback-scoped view, and publishing it would fight the icache's
    // own eviction policy for slots.
    Some(create_vfs_inode_in(fs, ino as u32, &ext4_inode))
}

/// Persist the inode-table blocks of the given writeback targets.
///
/// The ordered-data discipline: the DATA blocks of a flushed file must
/// reach the device BEFORE the inode that describes them, otherwise a
/// crash in between leaves a file whose size says 8 MB but whose blocks
/// are still unwritten. `sync(2)` therefore syncs the buffer cache (data)
/// first and only then these metadata blocks — the same discipline
/// `ext4_sync_file_inner` applies for fsync(2).
///
/// Returns the number of inode-table blocks that were persisted.
pub fn sync_inode_tables(targets: &[(u64, u64)]) -> usize {
    let mut n = 0usize;
    for &(fs_id, ino) in targets {
        let Some(fs) = ext4_instance_from_id(fs_id) else {
            continue;
        };
        let Some(tbl_blk) = inode::inode_table_block_of(fs, ino as u32) else {
            continue;
        };
        // INV-BIO-BHREF: bread pins until brelse; the dirty query rides
        // the safe accessor.
        if let Some(bh) = crate::fs::bio::bread(fs.device, tbl_blk) {
            let dirty = crate::fs::bio::bh_is_dirty(&bh);
            let res = if dirty {
                crate::fs::bio::sync_dirty_buffer(bh)
            } else {
                Ok(())
            };
            crate::fs::bio::brelse(bh);
            if res.is_ok() && dirty {
                n += 1;
            }
        }
    }
    n
}

/// Persist the DATA blocks of the given writeback targets (ordered data).
///
/// The plain buffer-cache drain (`bio::sync_buffers`) writes every dirty
/// buffer in HASH ORDER: the inode-table block (the file's new size) can
/// reach the device BEFORE the data blocks it describes. A crash in
/// between leaves a file that CLAIMS 8 MB of data the device never
/// received — measured in the fiE3 crash-window boot: the on-disk inode
/// had the full size and the full 2048-block extent map, but only ~35 %
/// of the data blocks were on the device (md5 mismatch).
///
/// The ordered discipline (Linux's ordered data mode): the data blocks
/// first, the metadata last. `sync(2)` therefore syncs these data
/// blocks BEFORE the buffer-cache drain, so whenever the inode-table
/// block reaches the device the data it describes is already durable.
///
/// Returns the number of data blocks that were persisted.
pub fn sync_flushed_data(targets: &[(u64, u64)]) -> usize {
    let mut n = 0usize;
    for &(fs_id, ino) in targets {
        let Some(fs) = ext4_instance_from_id(fs_id) else {
            continue;
        };
        let Ok(ext4_inode) = fs.read_inode(ino as u32) else {
            continue;
        };
        // The block map as the flush left it (the buffer-cached inode
        // table entry carries the extent map the flush wrote).
        let Ok(blocks) = ext4_inode.get_data_blocks(fs) else {
            continue;
        };
        for block in blocks {
            if block == 0 {
                continue; // hole
            }
            // DURABILITY-BY-ABSENCE (same rationale as ext4_sync_file's
            // data loop): a block whose buffer left the cache was synced
            // at its eviction, so the ordered-data obligation for it is
            // already met — re-reading it paid one synchronous 4 KiB read
            // per absent block (the sync(2) wall after a large flush).
            if { crate::fs::bio::block_absent_durable(fs.device, block) } {
                continue;
            }
            // INV-BIO-BHREF: as sync_inode_tables.
            if let Some(bh) = crate::fs::bio::bread(fs.device, block) {
                let dirty = crate::fs::bio::bh_is_dirty(&bh);
                if dirty {
                    let _ = crate::fs::bio::sync_dirty_buffer(bh);
                    n += 1;
                }
                crate::fs::bio::brelse(bh);
            }
        }
    }
    n
}

/// Mount ext4 filesystem
///
/// # Parameters
/// - `device`: Block device pointer
///
/// # Returns
/// - `Ok(())`: Mount successful
/// - `Err(code)`: Mount failed
pub fn mount_ext4(device: &'static blkdev::GenDisk) -> Result<(), i32> {
    mount_ext4_flags(device, false)
}

/// Mount with explicit read-only-ness: a metadata_csum filesystem only
/// mounts read-only (the write path does not maintain checksums yet);
/// the read path verifies superblock/group/inode crc32c (csum.rs).
pub fn mount_ext4_flags(
    device: &'static blkdev::GenDisk,
    read_only: bool,
) -> Result<(), i32> {
    use core::sync::atomic::Ordering;

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
    // The writeback path resolves (fs_id, ino) through this registry.
    ext4_instance_register(fs_ptr);

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
            let cwd_bytes = { (*current).get_cwd() };
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

    // 0. Delayed allocation (2026-11): buffered write(2) leaves data only
    // in dirty page-cache frames — drain them into the ext4 write path
    // BEFORE the buffer-cache sync, or reboot/halt would drop every
    // un-fsynced write (write-through used to make this a no-op need).
    // The drain returns the (fs, inode) targets it flushed — their data
    // blocks sync first and their inode-table blocks last (ordered data,
    // see sys_sync).
    let flushed_targets = crate::fs::page_cache::get_page_cache().flush_all_dirty();

    // 1. The flushed files' DATA blocks (ordered data: data before the
    // metadata that describes it — see sync_flushed_data).
    let _ = sync_flushed_data(&flushed_targets);

    // 1a. All dirty data/metadata buffers.
    crate::fs::bio::sync_buffers()?;

    // 1b. The flushed files' inode-table blocks (their new size) — AFTER
    // the data (ordered data), the same discipline fsync/sync(2) use.
    let _ = sync_inode_tables(&flushed_targets);

    // 2. Superblock clean marking (block 0, byte offset 1024 + 58).
    // No ext4 root — nothing to mark.
    let device = match mounted_ext4_fs() {
        Some(fs) => fs.device,
        None => return Ok(()),
    };
    if let Some(bh) = crate::fs::bio::bread(device, 0) {
        // s_state offset within the on-disk superblock: 13 u32 fields
        // (52) + s_mnt_count(2) + s_max_mnt_count(2) + s_magic(2) = 58.
        const SB_OFF: usize = 1024;
        const S_STATE_OFF: usize = SB_OFF + 58;
        const EXT4_VALID_FS: u16 = 1;
        // INV-BIO-BHREF: the s_state store goes through the exclusive
        // data view (BufferHead::write's body is this slice store).
        crate::fs::bio::bh_data_mut(&bh)[S_STATE_OFF..S_STATE_OFF + 2]
            .copy_from_slice(&EXT4_VALID_FS.to_le_bytes());
        let r = crate::fs::bio::sync_dirty_buffer(bh);
        crate::fs::bio::brelse(bh);
        r?;
    }

    // 3. Disk-side volatile cache.
    crate::drivers::virtio::flush_pci_blk()
}

/// Create a VFS inode for the ext4 root directory (inode 2).
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
    let fallback = || {
        let mut inode = Inode::new(2, InodeMode::new(InodeMode::S_IFDIR | 0o755));
        inode.ops = Some(&EXT4_INODE_OPS);
        alloc::sync::Arc::new(inode)
    };
    match mounted_ext4_fs() {
        Some(fs) => match fs.read_inode(2) {
            Ok(ext4_inode) => create_vfs_inode(2, &ext4_inode),
            // Fallback: shouldn't happen at mount time.
            Err(_) => fallback(),
        },
        None => fallback(),
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
        // Unregister BEFORE the free: a page-cache entry that outlived this
        // filesystem must not dereference the freed box.
        ext4_instance_unregister(fs_ptr);
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
    // Parse path to absolute path
    let abs_path = resolve_path(path);

    let fs = mounted_ext4_fs()?;

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

    // GLOBAL_EXT4_FS stores the mount's instance (INV-EXT4-GLOBALFS);
    // resolve through the audited edge.
    let fs = mounted_ext4_fs().ok_or(errno::Errno::IOError.as_neg_i32())?;

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
        generation: 0,
    };

    // Initialize extent header in i_block
    let header = extent::get_extent_header_mut(&mut new_inode.block);
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
    Ok(create_vfs_inode_in(fs, new_ino, &new_inode))
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
    // The instance pointer comes from the caller's inode private_data
    // (fs_hint, INV-EXT4-PRIVFS provenance: create_vfs_inode installed
    // it) or the boot GLOBAL (INV-EXT4-GLOBALFS) — both valid for the
    // mount; resolved through the framework edges.
    let fs: &Ext4FileSystem = if let Some(hint) = fs_hint {
        crate::fs::edges::static_ref_unchecked(hint as *mut Ext4FileSystem)
    } else {
        mounted_ext4_fs().ok_or(errno::Errno::IOError.as_neg_i32())?
    };

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

    let _ext4_guard = EXT4_BIG_LOCK.lock();

    // PUNCH-PATH DATA SYNC (falloc-wedge lane, 2026-11-16): a punch
    // on a file whose buffered writes are still unflushed must first
    // WRITE THOSE PAGES BACK — the reference ext4_punch_hole starts
    // with filemap_write_and_wait_range(). Without it, the on-disk
    // inode still carried the pre-write size (0 for a fresh file)
    // and no extents, so the punch clamped to an empty range and
    // returned SUCCESS as a no-op while the page cache kept serving
    // the original bytes (LTP fallocate06's clear-check and the
    // punchprobe: every hole byte read back nonzero,
    // run/shell-punch3.log). Flushing materializes the data blocks
    // and the on-disk size the punch then operates on. Callers hold
    // no loop io claim (syscall context), so the writeback engine's
    // re-entry discipline is satisfied; WRITEBACK_GUARD serializes
    // against concurrent flush engines.
    if mode & FALLOC_FL_PUNCH_HOLE != 0 {
        let fs_id = fs as *const Ext4FileSystem as u64;
        if let Some(vfs_ino) = crate::fs::ext4::resolve_writeback_inode(fs_id, ino as u64) {
            crate::fs::ext4::file::ext4_flush_dirty_inode(
                &vfs_ino,
                0,
                u64::MAX,
            );
        }
    }

    let mut ext4_inode = fs.read_inode(ino)?;
    if !ext4_inode.is_reg() {
        return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32()); // EBADR-ish
    }
    // falloc-wedge debug (bounded): name the punch's starting state.
    if mode & FALLOC_FL_PUNCH_HOLE != 0 {
        static PDBG: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if PDBG.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 4 {
            crate::pr_info!(
                "PUNCH-DBG ino={} off={} len={} disk_size={} extent={} i_blocks={}",
                ino,
                offset,
                len,
                ext4_inode.size,
                ext4_inode.has_extent(),
                ext4_inode.blocks
            );
        }
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
        // ALIGNED INTERIOR + ZEROED EDGES (falloc-wedge round 3,
        // 2026-11-16): only blocks FULLY covered by the range are
        // freed; partial edge blocks are zeroed IN PLACE, allocation
        // kept — the reference ext4_punch_hole shape (non-bigalloc).
        // The first cut floored/ceiled the range to block
        // granularity, so a MISALIGNED punch clobbered the bytes
        // just outside the range (LTP fallocate06's clear-check:
        // memcmp of the prefix before the punched range failed,
        // run/shell-f06c.log).
        let interior_first = (offset + block_size - 1) / block_size;
        let interior_last_excl = file_end / block_size;
        let punch_first = interior_first;
        let punch_last = interior_last_excl; // exclusive
        // Edge byte ranges to zero in place (outside the interior).
        let head_zero = (offset, core::cmp::min(file_end, interior_first * block_size));
        let tail_zero = (
            core::cmp::max(offset, interior_last_excl * block_size),
            file_end,
        );
        // Zero the partial edge blocks IN PLACE (byte-sliced,
        // allocation kept). Runs BEFORE the interior rebuild — the
        // map is still the pre-punch one, so phys resolution is
        // exact for both extent and indirect files. Missing blocks
        // (holes) need nothing — they already read as zeros.
        {
            // phys of one file block, from the pre-punch map.
            let resolve = |blk: u64| -> Option<u64> {
                if ext4_inode.has_extent() {
                    let exts = crate::fs::ext4::extent::ext4_ext_gather(
                        fs,
                        &ext4_inode.block,
                    )
                    .ok()?;
                    exts.iter()
                        .find(|e| {
                            blk >= e.ee_block as u64 && blk < e.ee_block as u64 + e.len as u64
                        })
                        .map(|e| e.phys + (blk - e.ee_block as u64))
                } else if blk < 12 {
                    let b = ext4_inode.block[blk as usize];
                    if b != 0 { Some(b as u64) } else { None }
                } else {
                    let ind = ext4_inode.block[12] as u64;
                    if ind == 0 { return None; }
                    let bh = crate::fs::bio::bread(fs.device, ind)?;
                    let o = ((blk - 12) * 4) as usize;
                    let d = bio::bh_data(&bh);
                    let b = u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
                    crate::fs::bio::brelse(bh);
                    if b != 0 { Some(b as u64) } else { None }
                }
            };
            for &(zfrom, zto) in &[head_zero, tail_zero] {
                let mut off = zfrom;
                while off < zto {
                    let blk = off / block_size;
                    let in_blk = off % block_size;
                    let n = core::cmp::min(zto - off, block_size - in_blk);
                    if let Some(phys) = resolve(blk) {
                        if phys != 0 {
                            if let Some(bh) = crate::fs::bio::bread(fs.device, phys) {
                                let d = bio::bh_data_mut(&bh);
                                d[(in_blk as usize)..(in_blk + n) as usize].fill(0);
                                bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                                let _ = crate::fs::bio::sync_dirty_buffer(bh);
                                crate::fs::bio::brelse(bh);
                            }
                        }
                    }
                    off += n;
                }
            }
        }

        // R108: punch frees collected by the extent branch, drained
        // after the tail write_inode_disk publishes the punched map.
        let mut r108_punch_drain: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
        let mut r108_punch_meta: alloc::vec::Vec<u64> = alloc::vec::Vec::new();

        if ext4_inode.has_extent() {
            // Any-depth punch via gather/classify/rebuild (the old
            // inline-root-only walker refused deep trees and could
            // not represent more than 4 surviving fragments).
            let hdr = extent::get_extent_header(&ext4_inode.block);
            if hdr.eh_magic == EXT4_EXT_MAGIC {
                // R108 (crash-ordering, see ext4_setattr): the punch
                // frees are DEFERRED until the punched map is durably
                // published (write_inode_disk at the tail of this path
                // syncs) — freeing against the still-stale on-disk map
                // is the re-free poison window.
                let mut r108_punch_runs: alloc::vec::Vec<(u64, u64)> =
                    alloc::vec::Vec::new();
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
                        // frees are untenable for big extents). R108:
                        // freed after the punched map is durable.
                        r108_punch_runs.push((phys, ext_len));
                        freed_sectors += ext_len * (block_size / 512);
                    } else if ext_first < punch_last && ext_last > punch_first {
                        // Partially covered (falloc-wedge lane,
                        // 2026-11-16): SPLIT at the punch boundaries and
                        // free the covered middle — the reference
                        // kernel's ext4_ext_remove_space shape. The old
                        // arm zeroed the covered blocks in place and
                        // KEPT the allocation, so punching any proper
                        // prefix of an extent returned NO space (LTP
                        // fallocate05's dealloc step: punch [0,64) of a
                        // 256-block extent freed nothing and the
                        // follow-up write ENOSPC'd, run/shell-f05.log).
                        let z_from = punch_first.max(ext_first);
                        let z_to = punch_last.min(ext_last);
                        // Left survivor [ext_first, z_from): same
                        // phys origin, unchanged unwritten flag (a
                        // kept preallocated range must stay unwritten
                        // — masked lengths turn never-materialized
                        // blocks into "written" and reads would fetch
                        // disk garbage instead of zeros).
                        if z_from > ext_first {
                            kept.push(crate::fs::ext4::extent::RawExtent {
                                ee_block: ext_first as u32,
                                phys,
                                len: (z_from - ext_first) as u16,
                                unwritten: e.unwritten,
                            });
                        }
                        // Right survivor [z_to, ext_last): phys shifted
                        // past the freed middle.
                        if ext_last > z_to {
                            kept.push(crate::fs::ext4::extent::RawExtent {
                                ee_block: z_to as u32,
                                phys: phys + (z_to - ext_first),
                                len: (ext_last - z_to) as u16,
                                unwritten: e.unwritten,
                            });
                        }
                        // Covered middle: FREED (written and unwritten
                        // alike — a punched preallocated range
                        // deallocates, Linux semantics). R108: freed
                        // after the punched map is durable.
                        r108_punch_runs.push((
                            phys + (z_from - ext_first),
                            z_to - z_from,
                        ));
                        freed_sectors += (z_to - z_from) * (block_size / 512);
                    } else {
                        kept.push(*e);
                    }
                }
                let csum_seed = crate::fs::ext4::csum::dir_csum_seed(
                    fs.csum_seed,
                    ext4_inode.ino,
                    ext4_inode.generation,
                );
                let meta_delta = crate::fs::ext4::extent::ext4_ext_rebuild_deferred(
                    fs, &mut ext4_inode.block, &kept, &mut r108_punch_meta, csum_seed,
                )?;
                // R108: the punched map (rebuilt above) is published
                // durably by the write_inode_disk at the tail of this
                // path; only then do the collected runs free. The
                // frees are stashed here and drained at the tail.
                r108_punch_drain = r108_punch_runs;
                ext4_inode.blocks = ext4_inode
                    .blocks
                    .saturating_sub(freed_sectors)
                    .saturating_add_signed(meta_delta);
            }
        } else {
            // Indirect-block files (falloc-wedge lane, 2026-11-16):
            // FREE the interior blocks instead of zeroing them in
            // place — the old arm returned no space, so a punch on an
            // indirect file (ext2/ext3 mounts) never made room (LTP
            // fallocate05's dealloc step, run/shell-f05.log). Direct
            // pointers of the ALIGNED INTERIOR are freed and cleared
            // (partial edge blocks were zeroed above, allocation
            // kept); the singly-indirect range frees its data blocks
            // and zeroes the matching entries (the indirect block
            // itself survives while any entry remains — bounded
            // one-block conservatism; unlink/truncate reclaim it).
            // Deeper indirection keeps nothing to free here (edge
            // zeroing above already covered its bytes; interior
            // blocks past the singly range are left allocated —
            // bounded, noted). All bitmap frees ride the R108 drain
            // below (map durable first, frees after).
            if interior_first >= interior_last_excl {
                // Whole range inside (at most) the two edge blocks —
                // nothing to free, the edge zeroing did everything.
            } else {
            let first = interior_first as usize;
            let last = (interior_last_excl - 1) as usize;
            let mut freed_indirect_blocks: u64 = 0;
            for i in first..=last.min(11) {
                let b = ext4_inode.block[i];
                if b != 0 {
                    r108_punch_drain.push((b as u64, 1));
                    freed_indirect_blocks += 1;
                    ext4_inode.block[i] = 0;
                }
            }
            if last >= 12 && ext4_inode.block[12] != 0 {
                let ind_blk = ext4_inode.block[12] as u64;
                let ents_per_blk = (fs.block_size / 4) as usize;
                let ent_first = first.saturating_sub(12);
                let ent_last_excl = core::cmp::min(last.saturating_sub(12) + 1, ents_per_blk);
                if ent_first < ent_last_excl {
                    if let Some(bh) = crate::fs::bio::bread(fs.device, ind_blk) {
                        let data = bio::bh_data_mut(&bh);
                        for ei in ent_first..ent_last_excl {
                            let o = ei * 4;
                            let b = u32::from_le_bytes([
                                data[o], data[o + 1], data[o + 2], data[o + 3],
                            ]);
                            if b != 0 {
                                r108_punch_drain.push((b as u64, 1));
                                freed_indirect_blocks += 1;
                                data[o..o + 4].fill(0);
                            }
                        }
                        bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                        let _ = crate::fs::bio::sync_dirty_buffer(bh);
                        crate::fs::bio::brelse(bh);
                    }
                }
            }
            if freed_indirect_blocks > 0 {
                ext4_inode.blocks = ext4_inode
                    .blocks
                    .saturating_sub(freed_indirect_blocks * (block_size / 512));
            }
            }
        }

        // PAGE-CACHE DISCIPLINE (falloc-wedge root cause, 2026-11-16):
        // kill ONLY the punched range's cached state — the reference
        // ext4_punch_hole runs truncate_pagecache_range(offset, end),
        // never a whole-inode invalidation. The whole-inode
        // invalidate_inode that ran here dropped STILL-DIRTY
        // delayed-allocation pages of the whole file — outside the
        // punched range those frames are the only copy of
        // acknowledged writes (LTP fallocate05/06 data loss;
        // punchprobe3 caught it with NO flush involved). Pages FULLY
        // covered by [offset, file_end) die; partial EDGE pages keep
        // their frames (they hold live bytes outside the range) with
        // the punched slice zeroed in-frame — the same discipline
        // tmpfs punch_hole uses. The pre-sync above flushed the dirty
        // set, so the dropped pages are clean; a page still dirty
        // after a failed flush inside the punched range dies with the
        // punch — its data is deleted by definition.
        {
            let cache = crate::fs::page_cache::get_page_cache();
            let fs_id = fs as *const Ext4FileSystem as u64;
            let ps = crate::mm::PAGE_SIZE as u64;
            let first_full = offset.div_ceil(ps);
            let last_full = file_end / ps; // exclusive
            if last_full > first_full {
                cache.invalidate_inode_span(fs_id, ino as u64, first_full, last_full);
            }
            // Zero the punched byte range in surviving live frames
            // (partial edges, plus pinned/mapped pages the span
            // invalidate left as marked): readers must see zeros.
            const ZCHUNK: usize = 65536;
            let total = (file_end - offset) as usize;
            if total > 0 {
                let zeros = alloc::vec![0u8; ZCHUNK.min(total)];
                let mut off = offset;
                while off < file_end {
                    let n = ((file_end - off) as usize).min(zeros.len());
                    cache.writeback_update_range_stale(fs_id, ino as u64, off, &zeros[..n]);
                    off += n as u64;
                }
            }
        }

        let sec = crate::drivers::rtc::wall_secs() as u32;
        ext4_inode.mtime = sec;
        ext4_inode.ctime = sec;
        // write_inode_disk expects the on-disk layout; convert.
        let on_disk = ext4_inode.to_on_disk();
        inode::write_inode_disk(fs, ino, &on_disk)?;
        // R108: the punched map is now DURABLE (write_inode_disk
        // syncs) — drain the deferred punch frees (data runs and the
        // replaced tree's metadata blocks). A failure above (the `?`
        // on write_inode_disk) skips the frees: leaked blocks (bits
        // set, unclaimed) instead of re-free poison.
        if !r108_punch_drain.is_empty() || !r108_punch_meta.is_empty() {
            let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
            for (phys, len) in &r108_punch_drain {
                let _ = allocator.free_block_run(*phys, *len);
            }
            for blk in &r108_punch_meta {
                let _ = allocator.free_block(*blk);
            }
        }
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
        return Err(-(crate::errno::constants::EOPNOTSUPP as i32));
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
        {
            let bh = bio::bread(fs.device, *block_num)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let data = bio::bh_data_mut(&bh);

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
                    // metadata_csum: refresh the block's dir tail.
                    super::ext4::namei::dir_block_seal(fs, parent.ino, data);
                    bio::bh_set_state_bit(&bh, bio::BufferState::BH_Dirty);
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

                // metadata_csum: refresh the block's dir tail.
                super::ext4::namei::dir_block_seal(fs, parent.ino, data);
                bio::bh_set_state_bit(&bh, bio::BufferState::BH_Dirty);
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
    {
        let bh = bio::bread(fs.device, new_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        // Zero the block
        for byte in bio::bh_data_mut(&bh).iter_mut() {
            *byte = 0;
        }

        // Create a single entry spanning the whole block
        // Since we don't have checksum, use full block size
        let data = bio::bh_data_mut(&bh);
        create_dir_entry(data, ino, name, file_type, block_size as u16);

        // metadata_csum: refresh the block's dir tail.
        super::ext4::namei::dir_block_seal(fs, parent_ino, data);
        bio::bh_set_state_bit(&bh, bio::BufferState::BH_Dirty);
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

/// Ext4 inode lookup operation (pub: the path walk's negative-memo gate
/// compares this fn pointer — only filesystems whose content changes
/// exclusively through VFS mutations may memo ENOENT results; dynamic
/// filesystems are excluded).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
pub fn ext4_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    let fs = get_ext4_fs_from_inode(dir).map_err(|e| errno::Errno::from_neg_i32(e))?;
    let parent_inode = sb_inode_of(dir).map_err(|_| errno::Errno::NotADirectory)?;

    // Convert name to str
    let name_str = core::str::from_utf8(name).map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    // Lookup in directory
    let entry = fs.lookup(parent_inode, name_str)?;
    Ok(entry.inode as Ino)
}

/// Ext4 getattr operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_getattr(inode: &Inode, stat: &mut Stat) -> i32 {
    if get_ext4_fs_from_inode(inode).is_err() {
        return errno::Errno::NoSuchFileOrDirectory.as_neg_i32();
    }
    // Get ext4 inode from sb field (we store it there)
    let ext4_inode = match sb_inode_of(inode) {
        Ok(v) => v,
        Err(e) => return e,
    };

    // st_dev = the filesystem instance's id. Fallback only: the central
    // stat exits (vfs_stat / file_stat / stat_file_by_path*) overwrite
    // this with the instance's anonymous device number
    // (inode::stat_dev_user — Linux get_anon_bdev semantics); OH's
    // switch_root "/" vs "/usr" comparison works either way — distinct
    // instances report distinct devs.
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
fn ext4_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let fs = match get_ext4_fs_from_inode(inode) {
        Ok(fs) => fs,
        Err(_) => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    // Get ext4 inode from sb field
    let ext4_inode = match sb_inode_of(inode) {
        Ok(v) => v,
        Err(_) => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };

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
        let block_data = crate::safety::bytes::bytes_of(&ext4_inode.block);
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
fn ext4_setattr(inode: &Inode, attr: u32, arg1: u64, arg2: u64) -> i32 {
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
    let _ext4_guard = EXT4_BIG_LOCK.lock();

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

    // R108 (fi-fix-verify, P0 crash-window data integrity): the truncate
    // path DEFERS its bitmap frees until the shrunk map is durably
    // published. The old order — free_block_run's synchronous BITMAP
    // write first, the inode-table write riding the lazy buffer
    // writeback (write_inode does not sync without a journal handle) —
    // left a crash window in which the image carried FREE bits + a
    // STALE map still claiming them. On the next boot the allocator
    // handed those blocks to a new owner and the stale map's next
    // truncate re-freed LIVE blocks: two file pages, one disk block
    // (the fvpop→fvset→fv1/2/3 poison chain — every crash-window boot
    // re-manufactured the taint it then suffered from; measured
    // 2026-11-12). The vectors are filled by the ATTR_SIZE truncate
    // classification below and drained in the write-back success arm
    // AFTER sync_inode_table_block; a crash in between leaves the
    // benign leak direction (map shrunk, bits still set).
    let mut r108_drop_runs: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    let mut r108_drop_blocks: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    // Old extent-TREE metadata blocks replaced by the rebuild (same
    // deferred-free rule: their bits clear only after the rebuilt root
    // is durable — a stale root pointing at freed tree blocks walks
    // garbage on the next boot).
    let mut r108_drop_meta: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    // Family b sibling: set when the extent branch reconciled i_blocks
    // (data + tree nodes); the post-branch blanket assignment must not
    // clobber it back to data-only.
    let mut ext4_blocks_reconciled = false;

    // SP-2 (stress-port r1, 2026-11-02): metadata-only setattr (chmod/
    // chown/utime/atime) must NOT write back the stale disk size. The
    // disk-read Ext4Inode lags pending writeback (write() updates the
    // VFS size immediately; the disk inode is synced by writeback), so
    // a chmod on a freshly-written file persisted i_size=0 — combined
    // with the (formerly unconditional) page-cache invalidate below it
    // dropped the unflushed data and read-after-chmod returned EOF
    // (renameco: chmod sequence 3/300 fails with n=0; rename alone
    // clean). The live VFS size is authoritative for every non-SIZE
    // flavor (see FIX8 below for the same authority argument).
    if attr != setattr_attr::ATTR_SIZE {
        let live = inode.size.load(core::sync::atomic::Ordering::Acquire);
        ext4_inode.set_size(live);
    }

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
                // Truncate: free blocks beyond new_size. The frees are
                // R108-deferred (see r108_drop_runs at the top of this
                // function); classification only.
                let block_size = fs.block_size as u64;
                let new_blocks = (new_size + block_size - 1) / block_size;
                let old_blocks = (ext4_inode.get_size() + block_size - 1) / block_size;

                // Delalloc reservations (fio-delalloc): the cut tail's
                // pending-writeback reservations are void — release every
                // reserved index at or past the new EOF.
                crate::fs::ext4::file::da_release_from(fs, ext4_inode.ino, new_blocks);

                if ext4_inode.has_extent() {
                    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};
                    // Any-depth shrink via gather/classify/rebuild: extents
                    // fully beyond the new EOF are freed, a straddling one
                    // keeps its head (unwritten flag preserved), and the
                    // tree is rebuilt from the survivors (the old code only
                    // handled the 4-entry inline root and refused deep
                    // trees).
                    let hdr = extent::get_extent_header(&ext4_inode.block);
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
                                // R108: freed after the shrunk map is
                                // durable (see r108_drop_runs above).
                                r108_drop_runs.push((phys_start, ext_len));
                                freed_sectors += ext_len * (block_size / 512);
                            } else if logical_start + ext_len > new_blocks {
                                // Straddles: shrink to the new EOF, free the tail.
                                let keep_len = new_blocks - logical_start;
                                // R108: deferred free (see r108_drop_runs).
                                r108_drop_runs.push((
                                    phys_start + keep_len,
                                    ext_len - keep_len,
                                ));
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
                        let csum_seed = crate::fs::ext4::csum::dir_csum_seed(
                            fs.csum_seed,
                            ext4_inode.ino,
                            ext4_inode.generation,
                        );
                        let meta_delta = match crate::fs::ext4::extent::ext4_ext_rebuild_deferred(
                            fs, &mut ext4_inode.block, &kept, &mut r108_drop_meta, csum_seed,
                        ) {
                            Ok(d) => d,
                            Err(e) => return e,
                        };
                        ext4_inode.blocks = ext4_inode
                            .blocks
                            .saturating_sub(freed_sectors)
                            .saturating_add_signed(meta_delta);
                        // i_blocks keeps the extent-tree blocks (family b
                        // sibling, ext4-corrupt lane): the blanket
                        // `blocks = new_blocks * spb` assignment after the
                        // branch used to clobber the reconciled value to
                        // DATA-ONLY sectors, dropping every kept tree
                        // node from i_blocks — e2fsck pass 1 recomputes
                        // i_blocks from the walked tree and reported
                        // "i_blocks N, should be N+16" for every
                        // truncated deep-tree file (g2: inodes 456/465,
                        // 2008 vs 2024 = exactly their 2 tree nodes).
                        ext4_blocks_reconciled = true;
                    }
                } else {
                    // Free indirect blocks beyond new size
                    for i in new_blocks as usize..old_blocks as usize {
                        if i < 12 {
                            if ext4_inode.block[i] != 0 {
                                // R108: deferred free (see r108_drop_blocks).
                                r108_drop_blocks.push(ext4_inode.block[i] as u64);
                                ext4_inode.block[i] = 0;
                            }
                        } else {
                            match indirect::ext4_get_block(fs, &ext4_inode.block, i as u64) {
                                Ok(block_num) if block_num != 0 => {
                                    // R108: deferred free (see
                                    // r108_drop_blocks) — the data block's
                                    // bit clears only after the root
                                    // pointers are durable.
                                    r108_drop_blocks.push(block_num);
                                    // Clear the freed block's indirect slot: a
                                    // stale pointer outlives the free, and after
                                    // a later grow the hole range would resolve
                                    // to the old physical blocks — which the
                                    // allocator may have reused — serving
                                    // foreign bytes for reads and writing
                                    // through dead mappings (the reference
                                    // kernel clears each pointer as it frees,
                                    // ext4_free_branches). clear_indirect_entry
                                    // syncs the slot inline — the slot clear is
                                    // already durable before the deferred free.
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
                        // R108: deferred free (see r108_drop_blocks).
                        r108_drop_blocks.push(ext4_inode.block[12] as u64);
                        ext4_inode.block[12] = 0;
                    }
                    if new_blocks < 12 + (block_size / 4) as u64 && ext4_inode.block[13] != 0 {
                        // R108: deferred free (see r108_drop_blocks).
                        r108_drop_blocks.push(ext4_inode.block[13] as u64);
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
                        // R108: deferred free (see r108_drop_blocks).
                        r108_drop_blocks.push(ext4_inode.block[14] as u64);
                        ext4_inode.block[14] = 0;
                    }
                }
                // Indirect-mode files: the block count is exactly the data
                // blocks (indirect metadata is not counted in i_blocks by
                // this implementation's grow paths — see the note in
                // file.rs allocate_indirect_block). Extent-mode files keep
                // the reconciled value (data + surviving tree nodes) set
                // in the extent branch above.
                if !ext4_blocks_reconciled {
                    ext4_inode.blocks = (new_blocks * (block_size / 512)) as u64;
                }

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
                                // INV-BIO-BHREF: bh is a one-block buffer
                                // from bread; the tail range is in-bounds
                                // and the buffer is released after sync.
                                let data = crate::fs::bio::bh_data_mut(&bh);
                                data[tail_start..block_size as usize].fill(0);
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
            // Invalidate page cache ONLY on a size change (truncate/
            // extend). SP-2 (stress-port r1): this used to run for EVERY
            // setattr flavor — a chmod/chown/utime/atime dropped the
            // inode's DIRTY unflushed pages (silent data loss: renameco
            // read back EOF after chmod on a freshly-written file; the
            // reference kernel keeps the page cache across metadata-only
            // attribute changes) and needlessly killed the buffer-cache
            // hit rate on the atime path.
            if attr == setattr_attr::ATTR_SIZE {
                crate::fs::page_cache::get_page_cache().invalidate_inode(
                    fs as *const Ext4FileSystem as u64,
                    inode.ino,
                );
            }
            // R108: the truncating ATTR_SIZE path publishes the shrunk
            // map HERE. write_inode marked the inode-table block dirty
            // WITHOUT syncing (no journal handle); sync it now so the
            // shrunk map is durable BEFORE the deferred bitmap frees.
            // On sync failure the frees are skipped entirely — a leaked
            // run (bits stay set, map no longer claims them) is benign,
            // while freeing against an unpublishable map is exactly the
            // re-free poison window this ordering exists to close.
            if attr == setattr_attr::ATTR_SIZE
                && (!r108_drop_runs.is_empty()
                    || !r108_drop_blocks.is_empty()
                    || !r108_drop_meta.is_empty())
            {
                if inode::sync_inode_table_block(fs, ext4_ino).is_ok() {
                    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                    for (phys, len) in &r108_drop_runs {
                        let _ = allocator.free_block_run(*phys, *len);
                    }
                    for blk in &r108_drop_blocks {
                        let _ = allocator.free_block(*blk);
                    }
                    for blk in &r108_drop_meta {
                        let _ = allocator.free_block(*blk);
                    }
                } else {
                    static R108_SKIP: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    if R108_SKIP.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 10 {
                        crate::pr_err!(
                            "R108-FREE-SKIPPED ino={} runs={} blocks={} meta={} (map not durable; leaked, not aliased)",
                            ext4_ino,
                            r108_drop_runs.len(),
                            r108_drop_blocks.len(),
                            r108_drop_meta.len()
                        );
                    }
                }
            }
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
        let _guard = EXT4_BIG_LOCK.lock();
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
fn ext4_destroy_inode(inode: &mut crate::fs::inode::Inode) {
    if let Some(ptr) = inode.sb {
        // ptr was created by Box::into_raw(Box::new(ext4_inode.clone()))
        // SAFETY: sb holds the Box leaked by into_raw in create_vfs_inode
        // (INV-EXT4-SBRES-1); this Drop is the single reclaim.
        let _ = crate::fs::edges::box_take(ptr as *mut inode::Ext4Inode);
        inode.sb = None;
    }
    if let Ok(fs) = get_ext4_fs_from_inode(inode) {
        // SAFETY: read_inode only touches the block layer under the big
        // lock taken below; queueing avoids I/O in this Drop context.
        {
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
/// The primitive is `crate::sync::SleepMutex` (kernel/src/sync/mutex.rs)
/// — the reference-recipe sleeping mutex for locks held across blocking
/// I/O. History: a semaphore-backed mutex attempt (R7-A2) lost wakeups
/// and was reverted; the pure-spin acquire could wedge the whole machine
/// when the holder slept in block I/O while another CPU spun
/// preempt-disabled on it (LTP-F10, the LTP r2 WEDGE family); the
/// waitqueue slowpath (R7-B6b/R8-6/R32-F10) fixed that; R84 added the
/// fairness gate/handoff (b139 starvation); R85 the orphaned-handoff
/// epoch (b148 silent crawl). The sync-realign lane (2026-11) moved the
/// whole algorithm into sync/mutex.rs unchanged so the tree has ONE
/// mutex primitive and no yielding spinlock anywhere.
///
/// Task-recursive: a loop device backed by a file on ANOTHER ext4
/// instance re-enters ext4 from inside the block layer (ext4#2's create
/// writes metadata through /dev/loopN into ext4#1, which takes the same
/// lock). Nested acquire by the SAME task is legal; the outermost guard
/// releases. do_exit() force-drops via release_if_owner (R73) since
/// RAII never runs through do_exit.
pub static EXT4_BIG_LOCK: crate::sync::SleepMutex =
    crate::sync::SleepMutex::new_rt("EXT4-BIGLOCK", true);

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
    // Incremental getdents64 (getdents lane): when present, the VFS
    // readdir path uses this op exclusively — no whole-dir Vec, no
    // DirentCache snapshot. ext4_readdir above stays as the Vec-returning
    // fallback for whole-listing callers.
    readdir_fill: Some(ext4_readdir_fill),
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
fn ext4_iget(parent: &Inode, _name: &[u8], ino: Ino) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    let fs = get_ext4_fs_from_inode(parent).map_err(errno::Errno::from_neg_i32)?;

    // Read child inode from disk
    let ext4_inode = fs.read_inode(ino as u32)
        .map_err(|_| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let vfs_inode = create_vfs_inode_in(fs, ino as u32, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// open_by_handle_at support: materialize the VFS inode for an
/// arbitrary on-disk inode number of THIS filesystem (the reference's
/// export_ops fh_to_dentry -> ext4_iget). The `base` inode only pins
/// the Ext4FileSystem instance.
///
/// SAFETY: `base` must be an ext4 inode (EXT4_INODE_OPS).
pub fn iget_by_ino(
    base: &Inode,
    ino: u64,
) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    ext4_iget(base, b"", ino)
}

/// Wrapper for ext4_mkdir to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_mkdir_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    flush_pending_reaps();
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    let fs = get_ext4_fs_rw_from_inode(dir)?;

    // Call namei's ext4_mkdir
    let new_ino = namei::ext4_mkdir(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    // Read the new inode and convert to in-memory format
    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Update the parent directory's cached Ext4Inode after a directory modification.
/// This ensures subsequent lookups see the latest block pointers and size.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn refresh_parent_dir_cache(dir: &Inode, fs: &Ext4FileSystem) {
    refresh_inode_cache(dir, fs);
}

/// Refresh the Ext4Inode cached in inode.sb after a disk write.
/// This ensures cached data (size, blocks, timestamps) stays in sync.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn refresh_inode_cache(inode: &Inode, fs: &Ext4FileSystem) {
    if let Ok(cached) = sb_inode_of_mut(inode) {
        if let Ok(disk_inode) = inode::read_inode(fs, inode.ino as u32) {
            let mut updated = inode::Ext4Inode::from_disk(&disk_inode, inode.ino as u32);
            // Delayed-allocation guard (compat-r3): buffered writes publish
            // the sb-box size AHEAD of the disk inode; this refresh carries
            // the DISK-FRESH state and used to regress the box back to the
            // lagging on-disk size (chmod after a buffered write → every
            // later stat served st_size=0: LTP stat01/fstat02). Hold the
            // published dirty-tail EOF while dirty frames still live beyond
            // the disk size — the same watermark the icache revival guard
            // uses (create_vfs_inode_in); writeback's refresh lands with
            // the exact final size once the frames are gone.
            if updated.is_reg() {
                let fs_id = fs as *const Ext4FileSystem as u64;
                if let Some(tail_eof) = crate::fs::page_cache::get_page_cache()
                    .dirty_tail_eof(fs_id, inode.ino as u64)
                {
                    if tail_eof > updated.size {
                        updated.size = tail_eof;
                    }
                }
            }
            *cached = updated;
        }
    }
}

/// Wrapper for ext4_rmdir to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_rmdir_wrapper(dir: &Inode, name: &[u8]) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock();
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
fn ext4_create_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    flush_pending_reaps();
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    let fs = get_ext4_fs_rw_from_inode(dir)?;
    let new_ino = namei::ext4_create(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_mknod to match VFS signature (device/FIFO nodes).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_mknod_wrapper(dir: &Inode, name: &[u8], mode: InodeMode, dev: u64) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    let fs = get_ext4_fs_rw_from_inode(dir)?;
    let new_ino = namei::ext4_mknod(fs, dir.ino as u32, name, mode.bits() as u16, dev)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_symlink to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_symlink_wrapper(dir: &Inode, name: &[u8], target: &[u8]) -> Result<alloc::sync::Arc<Inode>, errno::Errno> {
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    let fs = get_ext4_fs_rw_from_inode(dir)?;
    let new_ino = namei::ext4_symlink(fs, dir.ino as u32, name, target)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode_in(fs, new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_link to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_link_wrapper(dir: &Inode, name: &[u8], target: &Inode) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock();
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
            if let Ok(cached) = sb_inode_of_mut(target) {
                if let Ok(disk) = inode::read_inode(fs, target.ino as u32) {
                    let mut fresh = inode::Ext4Inode::from_disk(&disk, target.ino as u32);
                    // Delayed-allocation guard (compat-r3): same discipline
                    // as refresh_inode_cache — a buffered-written target's
                    // published size must survive the disk-fresh reload.
                    if fresh.is_reg() {
                        let fs_id = fs as *const Ext4FileSystem as u64;
                        if let Some(tail_eof) = crate::fs::page_cache::get_page_cache()
                            .dirty_tail_eof(fs_id, target.ino as u64)
                        {
                            if tail_eof > fresh.size {
                                fresh.size = tail_eof;
                            }
                        }
                    }
                    *cached = fresh;
                }
            }
            0
        }
        Err(e) => e,
    }
}

/// Wrapper for ext4_unlink to match VFS signature
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_unlink_wrapper(dir: &Inode, name: &[u8]) -> i32 {
    flush_pending_reaps();
    let _ext4_guard = EXT4_BIG_LOCK.lock();
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
fn ext4_rename_wrapper(old_dir: &Inode, old_name: &[u8], new_dir: &Inode, new_name: &[u8]) -> i32 {
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    let fs = match get_ext4_fs_rw_from_inode(old_dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match namei::ext4_rename(fs, old_dir.ino as u32, old_name, new_dir.ino as u32, new_name) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// INV-EXT4-GLOBALFS (unsafe-slim r3c): the one audited read of the
/// GLOBAL_EXT4_FS singleton — the Box is installed by mount_ext4 and only
/// reclaimed by unmount_ext4, so a non-null Acquire load is live for the
/// 'static mount lifetime. Every global-fs wrapper rides this.
pub fn mounted_ext4_fs() -> Option<&'static Ext4FileSystem> {
    let p = GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire);
    if p.is_null() {
        return None;
    }
    // SAFETY: INV-EXT4-GLOBALFS above.
    Some(unsafe { &*p })
}

/// Get Ext4FileSystem pointer from inode's private_data
fn get_ext4_fs_from_inode(inode: &Inode) -> Result<&'static Ext4FileSystem, i32> {
    let fs_ptr = inode.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    // SAFETY: fs_ptr was stored as a raw pointer to the global
    // Ext4FileSystem during create_vfs_inode; valid for the mount's
    // lifetime (the ONE audited inode->fs resolver).
    Ok(unsafe { &*(fs_ptr as *const Ext4FileSystem) })
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

/// INV-EXT4-SBRES-1 (unsafe-slim L4): `inode.sb`, when `Some(p)`, is a
/// `Box::into_raw(Box::new(ext4_inode.clone()))` installed by
/// create_vfs_inode[_in] and reclaimed by ext4_destroy_inode at the
/// final Arc<Inode> drop — the only take-site. The borrow is tied to
/// the `&Inode` it came from, so it cannot outlive the inode whose
/// Drop reclaims the box. Every read of the cached on-disk inode goes
/// through this helper (or the `_mut` twin below) — the ONE audited
/// resolution per direction.
fn sb_inode_of(inode: &Inode) -> Result<&inode::Ext4Inode, i32> {
    let p = inode.sb.ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    // SAFETY: INV-EXT4-SBRES-1 above.
    Ok(unsafe { &*(p as *const inode::Ext4Inode) })
}

/// The mutating twin of `sb_inode_of` (INV-EXT4-SBRES-1): for
/// cache-refresh stores into the sb box. Callers hold the ext4 big
/// lock (EXT4_BIG_LOCK) or run in single-threaded creation contexts.
fn sb_inode_of_mut(inode: &Inode) -> Result<&mut inode::Ext4Inode, i32> {
    let p = inode.sb.ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    // SAFETY: INV-EXT4-SBRES-1 above; no aliasing shared borrow is live
    // at the store sites that use this (see each caller's lock note).
    Ok(unsafe { &mut *(p as *mut inode::Ext4Inode) })
}

/// Get file operations for ext4 regular files and directories
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
fn ext4_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
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
fn ext4_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    let fs_ptr = get_ext4_fs_from_inode(inode).ok()?;
    let ext4_inode = sb_inode_of(inode).ok()?;

    let ext4_entries = fs_ptr.list_dir(ext4_inode).ok()?;
    let mut entries = alloc::vec::Vec::new();
    for entry in ext4_entries.iter() {
        let name_bytes = &entry.name[..entry.name_len as usize];
        let dt = ext4_dirent_file_type(entry.file_type);
        // (launcher-errno lane) same heap-budget guard as list_dir: the
        // VfsDirEntry stage can also grow past the heap on huge dirs.
        if entries
            .try_reserve_exact(1)
            .is_err()
        {
            crate::pr_err!(
                "ext4: readdir partial: ino={} entries collected={} (heap budget)",
                ext4_inode.ino,
                entries.len()
            );
            break;
        }
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: entry.inode as u64,
            name: name_bytes.to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

/// Map an on-disk ext4 dirent file_type to the getdents DT_* value.
fn ext4_dirent_file_type(ft: u8) -> u8 {
    match ft {
        1 => crate::fs::inode::file_type::DT_REG,
        2 => crate::fs::inode::file_type::DT_DIR,
        3 => crate::fs::inode::file_type::DT_CHR,
        4 => crate::fs::inode::file_type::DT_BLK,
        5 => crate::fs::inode::file_type::DT_FIFO,
        6 => crate::fs::inode::file_type::DT_SOCK,
        7 => crate::fs::inode::file_type::DT_LNK,
        _ => crate::fs::inode::file_type::DT_UNKNOWN,
    }
}

/// Ext4 incremental readdir — the reference kernel's ext4_readdir shape
/// (getdents lane, 2026-11-17). Walks directory blocks ONE at a time
/// starting at byte offset `pos` (the file position) and packs
/// linux_dirent64 records straight into `buf`; returns
/// `(bytes_filled, next_pos)`. No whole-directory materialization, no
/// per-entry heap allocation, no 255-byte name copies — the old path's
/// per-open full list_dir (Vec<Ext4DirEntry> + Vec<VfsDirEntry> + one
/// Vec<u8> name per entry + dot synthesis Vec + snapshot Arc) made a
/// fresh-fd full-dir sweep cost 19x the reference.
///
/// Compatibility notes (kept bit-compatible with the Vec path above):
/// - "." and ".." are REAL on-disk entries in block 0 (both our mkdir
///   and reference mkfs write them); they are emitted from disk with
///   their true inode numbers instead of being synthesized.
/// - inode==0 entries are skipped (deleted slots, metadata_csum tails,
///   htree index fakes). name_len==0 and non-UTF-8 names are skipped —
///   the same corrupt-image tolerance list_dir has carried since the
///   OH userdata lanes (a bare advance, never a livelock).
/// - d_off is the byte offset of the NEXT entry: telldir/seekdir
///   round-trip through lseek with no index translation, and a
///   mutation between calls leaves offsets stable (a deleted entry
///   becomes an inode==0 slot that is stepped over).
/// - Indexed (htree) directories: every entry physically lives in a
///   leaf block, so the linear scan still visits each exactly once;
///   internal dx blocks parse as garbage dirents whose rec_len breaks
///   the block scan (block numbers < 8 fail rec_len>=8; their
///   name_len byte is the block number's high byte, 0 for any image
///   under 16M blocks) — the identical accidental-but-tolerated
///   behavior the list_dir scanner has always had on dx dirs.
// SAFETY: VFS callback contract; the Ext4Inode behind inode.sb outlives
// the call (the open File pins the VFS Inode that owns it).
fn ext4_readdir_fill(
    inode: &Inode,
    pos: u64,
    buf: &mut [u8],
) -> Result<(usize, u64), i32> {
    let fs = get_ext4_fs_from_inode(inode)?;
    let ext4_inode = sb_inode_of(inode)?;

    let dir_size = ext4_inode.size;
    if pos >= dir_size || buf.is_empty() {
        return Ok((0, pos.max(dir_size)));
    }
    let block_size = fs.block_size as u64;

    // Block list through the (ino, size)-keyed memo — same source
    // list_dir uses, so the two readers can never disagree. The clone
    // is one small Vec (<= nblocks u64) per call; the alternative
    // (get_data_block per block) re-walks the extent tree per block.
    let blocks = match fs.dir_blocks_cached(ext4_inode) {
        Some(b) => b,
        None => {
            let b = ext4_inode.get_data_blocks(fs)?;
            fs.dir_blocks_store(ext4_inode, b.clone());
            b
        }
    };
    crate::dfx::fsw::note_list_dir();

    let mut filled = 0usize;
    let mut cur = pos; // byte offset of the next candidate entry
    let mut saw_unfittable = false;

    'outer: while cur < dir_size {
        let bidx = (cur / block_size) as usize;
        let mut off = (cur % block_size) as usize;
        if bidx >= blocks.len() {
            break; // size says more, mapping says no: treat as EOF
        }
        let blk = blocks[bidx];
        if blk == 0 {
            // Sparse/holes: whole-block skip.
            cur = (bidx as u64 + 1) * block_size;
            continue;
        }
        let bh = match bio::bread(fs.device, blk) {
            Some(bh) => bh,
            None => return Err(errno::Errno::IOError.as_neg_i32()),
        };
        // SAFETY: bread returns a valid, pinned BufferHead whose b_data
        // covers one full block; we only read it and brelse before any
        // early exit below.
        let data = bio::bh_data(&bh);
        while off + 8 <= block_size as usize {
            let rec = &data[off..];
            let entry_ino = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            let rec_len = u16::from_le_bytes([rec[4], rec[5]]) as usize;
            let name_len = rec[6] as usize;
            let file_type = rec[7];

            // Corrupt tail: stop this block (ext4_rec_len validity is
            // rec_len >= 8 and entry contained in the block; the
            // >=12B folding rule from the ext4-corrupt lane means a
            // well-formed final entry's rec_len reaches the block end).
            if rec_len < 8 || off + rec_len > block_size as usize {
                break;
            }

            // Skip deleted slots / csum tails / dx fakes; advance past.
            if entry_ino == 0 || name_len == 0 || name_len + 8 > rec_len {
                off += rec_len;
                cur += rec_len as u64;
                continue;
            }
            let name = &rec[8..8 + name_len];
            // Corrupt/binary names: skip but ADVANCE (the historical
            // OH-userdata tolerance; a bare continue livelocked).
            if core::str::from_utf8(name).is_err() {
                off += rec_len;
                cur += rec_len as u64;
                continue;
            }

            // Pack one linux_dirent64 (19B header + name + NUL, padded
            // to 8 — the same record shape the VFS emit loop builds).
            let reclen64 = (19 + name_len + 1 + 7) & !7;
            if filled + reclen64 > buf.len() {
                saw_unfittable = true;
                // Release the pinned block BEFORE leaving the walk
                // (break 'outer skips the brelse below the loop).
                bio::brelse(bh);
                break 'outer;
            }
            let dst = &mut buf[filled..filled + reclen64];
            dst[0..8].copy_from_slice(&(entry_ino as u64).to_le_bytes());
            // d_off = byte offset of the NEXT entry (resume point).
            dst[8..16].copy_from_slice(&((off + rec_len) as u64 + bidx as u64 * block_size).to_le_bytes());
            dst[16..18].copy_from_slice(&(reclen64 as u16).to_le_bytes());
            dst[18] = ext4_dirent_file_type(file_type);
            dst[19..19 + name_len].copy_from_slice(name);
            dst[19 + name_len] = 0; // NUL
            for b in &mut dst[20 + name_len..] {
                *b = 0; // pad (never read by userland, zeroed for ABI)
            }

            filled += reclen64;
            off += rec_len;
            cur += rec_len as u64;
        }
        bio::brelse(bh);
        if off + 8 > block_size as usize && cur % block_size != 0 {
            // Block scan ended mid-block (corrupt tail): advance to the
            // next block so the walk cannot wedge on the same offset.
            cur = (bidx as u64 + 1) * block_size;
        }
    }

    if filled == 0 && saw_unfittable {
        // Reference behavior: a buffer too small for even ONE entry is
        // EINVAL, not a silent Ok(0) (telldir loops would spin).
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if filled == 0 {
        // Only skippable slots remained: consume to EOF and report it.
        return Ok((0, dir_size));
    }
    Ok((filled, cur))
}

/// Create VFS inode from ext4 inode
///
/// This helper function creates a VFS inode structure from an ext4 inode,
/// properly setting up the inode_operations and private data.
pub fn create_vfs_inode(ino: u32, ext4_inode: &inode::Ext4Inode) -> alloc::sync::Arc<Inode> {
    let fs = mounted_ext4_fs().expect("create_vfs_inode: no mounted ext4 instance");
    create_vfs_inode_in(fs, ino, ext4_inode)
}

/// Instance-aware variant: wires the new inode to `fs_ptr` (the creating
/// directory's filesystem — a loop-mounted ext2's children must point at
/// the LOOP instance, not the boot root).
pub fn create_vfs_inode_in(
    fs: &Ext4FileSystem,
    ino: u32,
    ext4_inode: &inode::Ext4Inode,
) -> alloc::sync::Arc<Inode> {
    // The instance's identity word (fs_id / private_data storage): the
    // reference itself is the liveness proof; only its ADDRESS is stored
    // (never dereferenced through those fields — see get_ext4_fs_from_inode
    // for the read-back edge).
    let fs_ptr = fs as *const Ext4FileSystem as *mut Ext4FileSystem;
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

    // Buffered-write revival guard (delayed allocation, 2026-11): dirty
    // page-cache frames of this (fs, ino) can OUTLIVE the VFS inode's
    // icache entry — write(2) deferred their writeback, then the icache
    // evicted the inode. The fresh copy above carries the ON-DISK size,
    // which lags those frames: every read would clamp at the stale size
    // and the written bytes stay invisible until writeback happens to
    // run. Lift the cached size to the EXACT dirty-tail EOF
    // (dirty_tail_eof: the publish-time watermark clamped to the
    // surviving dirty extent — a page-rounded extent would report 4096
    // for a 1000-byte appended file) so reads reach the frames without
    // exposing phantom zeros (the reference kernel keeps a dirty inode
    // pinned in its inode cache until writeback finishes — same
    // contract, different mechanism).
    if inode.mode.is_regular_file() {
        let fs_id = fs_ptr as u64;
        if let Some(tail_eof) = crate::fs::page_cache::get_page_cache()
            .dirty_tail_eof(fs_id, ino as u64)
        {
            if tail_eof > inode.size.load(core::sync::atomic::Ordering::Relaxed) {
                inode.size.store(tail_eof, core::sync::atomic::Ordering::Relaxed);
                // SAFETY: sb was just installed above (Box::into_raw) and
                // the Arc is not shared yet — exclusive plain field write.
                unsafe {
                    (*(inode.sb.unwrap() as *mut inode::Ext4Inode)).size = tail_eof;
                }
            }
        }
    }

    alloc::sync::Arc::new(inode)
}

/// Mount a SECOND ext4/ext2 instance (loop-backed scratch filesystem).
///
/// Builds an independent Ext4FileSystem on `disk` (an mkfs.ext2 image
/// bound to /dev/loopN), reads its root inode, and returns a VFS root
/// Per-device live-instance reuse table (the sget-by-bdev lookup — see
/// the reuse block in `mount_loop_instance`). Rows are published at
/// instance creation and invalidated when the device is re-bound
/// (LOOP_SET_FD / LOOP_CLR_FD), when the on-disk superblock identity
/// changed (a re-mkfs'd device — see `uuid` below), or when the instance
/// is reclaimed at a failed mount. `disk` is the GenDisk address, `fs`
/// the registered instance.
struct LoopInstanceReuse {
    disk: core::sync::atomic::AtomicU64,
    fs: core::sync::atomic::AtomicU64,
    /// On-disk superblock UUID (two LE u64 halves) captured at publish
    /// time. mke2fs writes a FRESH random UUID on every run, so a UUID
    /// mismatch is a reliable "the device image was re-created" signal:
    /// the cached instance's parsed geometry (inode size, group layout,
    /// 64bit feature) then describes a DEAD image and must not be
    /// reused — LTP rename01 iterates ext2/ext3/ext4 variants over one
    /// loop binding, and the ext4 variant's 64bit layout made the
    /// ext2-era instance's read_inode(2) land on a non-directory block
    /// (chdir(mntpoint) ENOTDIR).
    uuid: [core::sync::atomic::AtomicU64; 2],
}

const LOOP_REUSE_SLOTS: usize = 8;

static LOOP_INSTANCE_REUSE: [LoopInstanceReuse; LOOP_REUSE_SLOTS] = [const {
    LoopInstanceReuse {
        disk: core::sync::atomic::AtomicU64::new(0),
        fs: core::sync::atomic::AtomicU64::new(0),
        uuid: [
            core::sync::atomic::AtomicU64::new(0),
            core::sync::atomic::AtomicU64::new(0),
        ],
    }
}; LOOP_REUSE_SLOTS];

/// Read the on-disk superblock UUID of `disk` through the buffer cache
/// (same read init() uses: block 0, byte offset 1024; s_uuid at +0x68).
/// None when the block cannot be read.
fn sb_uuid_of(disk: &'static crate::drivers::blkdev::GenDisk) -> Option<(u64, u64)> {
    // INV-BIO-BHREF: bread on a live GenDisk returns a pinned BufferHead
    // whose b_data covers the full block; the UUID fields are read as
    // little-endian byte slices (equivalent to the old unaligned loads).
    let bh = crate::fs::bio::bread(disk, 0)?;
    let data = crate::fs::bio::bh_data(&bh);
    let lo = u64::from_le_bytes(
        data[1024 + 0x68..1024 + 0x68 + 8].try_into().expect("uuid lo"),
    );
    let hi = u64::from_le_bytes(
        data[1024 + 0x70..1024 + 0x70 + 8].try_into().expect("uuid hi"),
    );
    crate::fs::bio::brelse(bh);
    Some((lo, hi))
}

/// Publish a fresh instance for its device (replacing any stale row for
/// the same disk). Called under EXT4_BIG_LOCK.
fn loop_instance_publish(disk: &'static crate::drivers::blkdev::GenDisk, fs_ptr: *mut Ext4FileSystem) {
    use core::sync::atomic::Ordering;
    let disk_bits = disk as *const _ as u64;
    let fs_bits = fs_ptr as u64;
    let uuid = sb_uuid_of(disk).unwrap_or((0, 0));
    for slot in LOOP_INSTANCE_REUSE.iter() {
        let cur = slot.disk.load(Ordering::Acquire);
        if cur == 0 || cur == disk_bits {
            // Clear the fs word first: a reader that still sees the old
            // disk match must not observe the new fs pointer half-way.
            slot.fs.store(0, Ordering::Release);
            slot.disk.store(disk_bits, Ordering::Release);
            slot.uuid[0].store(uuid.0, Ordering::Release);
            slot.uuid[1].store(uuid.1, Ordering::Release);
            slot.fs.store(fs_bits, Ordering::Release);
            return;
        }
    }
    // Table full: this instance is simply not reusable — correctness
    // unaffected (a fresh mount then reads the device, like pre-reuse).
}

/// Drop the reuse row for a device. The loop driver's re-bind path calls
/// this (takes EXT4_BIG_LOCK); the mount-failure path uses the _locked
/// variant (already under the lock — SleepMutex is not reentrant).
pub fn loop_instance_invalidate(disk: &crate::drivers::blkdev::GenDisk) {
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    loop_instance_invalidate_locked(disk);
}

fn loop_instance_invalidate_locked(disk: &crate::drivers::blkdev::GenDisk) {
    use core::sync::atomic::Ordering;
    let disk_bits = disk as *const _ as u64;
    for slot in LOOP_INSTANCE_REUSE.iter() {
        if slot.disk.load(Ordering::Acquire) == disk_bits {
            // fs first, then the key: a racing mount either sees the row
            // gone (fresh instance) or the old fs with the key still set
            // (instance stays registered — safe either way).
            slot.fs.store(0, Ordering::Release);
            slot.disk.store(0, Ordering::Release);
        }
    }
}

/// wired to THIS instance (private_data / fs_id / cached inode). The
/// boot root keeps GLOBAL_EXT4_FS; all instance-specific operations
/// resolve the filesystem from the inode (get_ext4_fs_from_inode).
/// The instance is intentionally leaked (single mount per boot test
/// lifecycle; umount drops the dentry tree).
pub fn mount_loop_instance(
    disk: &'static blkdev::GenDisk,
    read_only: bool,
) -> Result<(alloc::sync::Arc<Inode>, bool), i32> {
    // SUPERBLOCK REUSE (reference get_tree_bdev/sget shape): a mount of
    // a device that already has a LIVE instance reuses it. Linux's sget
    // searches for an existing superblock by bdev BEFORE reading one —
    // so data written through an fd that outlived a MNT_DETACH umount
    // (the detached mount keeps the sb alive) is visible to the very
    // next mount of the same device: LTP umount2_01 writes 26 bytes
    // through the open fd AFTER umount2(MNT_DETACH), remounts, and
    // reads them back. A fresh instance answered from the on-disk inode
    // (0 bytes — the write never had a flush opportunity between detach
    // and remount) and the test TBROK'd at the read. The reuse key is
    // the GenDisk pointer; LOOP_SET_FD/LOOP_CLR_FD invalidate the row
    // (a re-bound loop is a NEW device image — the next mkfs must start
    // from the disk, never from the stale in-memory instance).
    let _ext4_guard = EXT4_BIG_LOCK.lock();
    {
        use core::sync::atomic::Ordering;
        for slot in LOOP_INSTANCE_REUSE.iter() {
            if slot.disk.load(Ordering::Acquire) as *const blkdev::GenDisk == disk {
                let fs_ptr =
                    slot.fs.load(Ordering::Acquire) as *mut Ext4FileSystem;
                if !fs_ptr.is_null() {
                    // Identity guard: the row is only valid while the
                    // on-disk superblock is the one the instance was
                    // built from (mke2fs rotates the UUID every run —
                    // see LoopInstanceReuse::uuid). A re-mkfs'd device
                    // falls through to a fresh instance.
                    if let Some((lo, hi)) = sb_uuid_of(disk) {
                        if slot.uuid[0].load(Ordering::Acquire) != lo
                            || slot.uuid[1].load(Ordering::Acquire) != hi
                        {
                            loop_instance_invalidate_locked(disk);
                            break;
                        }
                    }
                    // SAFETY: the slot's pointer is a registered instance
                    // (published under EXT4_BIG_LOCK; cleared by the same
                    // lock's invalidation path before any free).
                    let fs_ref = unsafe { &*fs_ptr };
                    // The requested read-only-ness only ever ADDS
                    // restrictions at the VFS mount flags (do_mount
                    // mirrors effective_ro); the instance itself stays
                    // as it was mounted.
                    let root = fs_ref
                        .read_inode(2)
                        .map(|ext4_inode| {
                            create_vfs_inode_in(fs_ref, 2, &ext4_inode)
                        })
                        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;
                    return Ok((root, fs_ref.read_only));
                }
            }
        }
    }

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
    // The writeback path resolves (fs_id, ino) through this registry
    // (fs_id IS the instance pointer for ext4-backed page-cache entries).
    ext4_instance_register(fs_ptr);
    // Publish into the per-device reuse table (see the reuse block above
    // for the sget-by-bdev rationale). Registered first: a reuse hit must
    // never outlive the registry row.
    loop_instance_publish(disk, fs_ptr);
    // fs_ptr is a leaked, valid instance pointer (registry-proven).
    let fs_ref = crate::fs::edges::static_ref_unchecked(fs_ptr);
    match fs_ref.read_inode(2) {
        Ok(ext4_inode) => Ok((create_vfs_inode_in(fs_ref, 2, &ext4_inode), fs_ref.read_only)),
        Err(_) => {
            // Not a filesystem root (bad magic / inode 2): fail the mount
            // and reclaim the instance.
            // Drop the reuse row BEFORE the reclaim (a published row must
            // never name a freed instance), then unregister (same
            // invariant as unmount).
            loop_instance_invalidate_locked(disk);
            ext4_instance_unregister(fs_ptr);
            // SAFETY: reclaiming the just-leaked box.
            unsafe { drop(alloc::boxed::Box::from_raw(fs_ptr)) };
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }
}
