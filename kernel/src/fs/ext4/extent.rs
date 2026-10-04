//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 extent tree support

use crate::errno;
use crate::fs::bio;

/// Extent header magic number
pub const EXT4_EXT_MAGIC: u16 = 0xF30A;

/// Extent header (in i_block or external block)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4ExtentHeader {
    /// Magic number (0xF30A)
    pub eh_magic: u16,
    /// Number of valid entries
    pub eh_entries: u16,
    /// Maximum number of entries that could follow
    pub eh_max: u16,
    /// Depth of extent tree (0 = leaf)
    pub eh_depth: u16,
    /// Generation number
    pub eh_generation: u32,
}

/// Extent entry (leaf node)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4Extent {
    /// First logical block covered by this extent
    pub ee_block: u32,
    /// Number of blocks covered by this extent
    pub ee_len: u16,
    /// High 16 bits of physical block
    pub ee_start_hi: u16,
    /// Low 32 bits of physical block
    pub ee_start_lo: u32,
}

impl Ext4Extent {
    /// Get the starting physical block number
    pub fn start_block(&self) -> u64 {
        ((self.ee_start_hi as u64) << 32) | (self.ee_start_lo as u64)
    }

    /// Get the length (number of blocks), masking the initialized flag (bit 15).
    pub fn length(&self) -> u16 {
        (self.ee_len as u32 & 0x7FFF) as u16
    }
}

/// Index entry (internal node)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4ExtentIdx {
    /// Logical block covered by children
    pub ei_block: u32,
    /// Low 32 bits of child block
    pub ei_leaf_lo: u32,
    /// High 16 bits of child block
    pub ei_leaf_hi: u16,
    /// Reserved
    pub ei_unused: u16,
}

impl Ext4ExtentIdx {
    /// Get the child block number
    pub fn leaf_block(&self) -> u64 {
        ((self.ei_leaf_hi as u64) << 32) | (self.ei_leaf_lo as u64)
    }
}

/// Parse extent header from i_block array
pub fn get_extent_header(i_block: &[u32; 15]) -> &Ext4ExtentHeader {
    // SAFETY: i_block is a &[u32; 15] (60 bytes), which is larger than
    // Ext4ExtentHeader (12 bytes), so the cast is within bounds of the allocation.
    unsafe {
        &*(i_block.as_ptr() as *const Ext4ExtentHeader)
    }
}

/// Unwritten-extent flag (bit 15 of ee_len): the blocks are allocated in
/// the bitmap but their contents have NOT been materialized — reads must
/// be zero-filled without touching disk. fallocate(2) preallocation
/// creates these (Linux semantics); the first write converts them.
pub const EXT4_EXT_UNWRITTEN: u16 = 0x8000;

/// Map a logical block to (physical block, unwritten flag).
///
/// Returns `Ok((0, false))` for a hole (nothing allocated). Unwritten
/// blocks ARE allocated — the physical block is returned along with
/// `unwritten == true` so callers can zero-fill instead of reading
/// garbage from a never-materialized disk block.
pub fn ext4_ext_get_block_ex(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<(u64, bool), i32> {
    let header = get_extent_header(i_block);

    if header.eh_magic != EXT4_EXT_MAGIC {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    if header.eh_entries > header.eh_max {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Root-node leaves only (same scope limit as ext4_ext_get_block for
    // depth > 0 trees created by foreign tools).
    if header.eh_depth != 0 {
        // Deep trees: resolve through the external walker, preserving the
        // unwritten flag (deep trees hold fallocate preallocations now).
        // SAFETY: i_block is 60 bytes; the index entries stay in-bounds.
        unsafe {
            let idxs = core::slice::from_raw_parts(
                (i_block.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>())
                    as *const Ext4ExtentIdx,
                core::cmp::min(header.eh_entries as usize, ROOT_NODE_CAP),
            );
            let mut child = 0u64;
            for idx in idxs {
                if logical_block >= idx.ei_block as u64 {
                    child = idx.leaf_block();
                } else {
                    break;
                }
            }
            if child == 0 {
                return Ok((0, false));
            }
            return find_block_in_external_extent_ex(fs, child, logical_block, 0);
        }
    }

    let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>())
        / core::mem::size_of::<Ext4Extent>();
    if header.eh_entries as usize > max_entries {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // SAFETY: entries follow the 12-byte header inside the 60-byte i_block;
    // eh_entries is validated against max_entries above.
    let entries = unsafe {
        core::slice::from_raw_parts(
            (i_block.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>())
                as *const Ext4Extent,
            header.eh_entries as usize
        )
    };

    for ext in entries {
        let start = ext.ee_block as u64;
        let end = start + ext.length() as u64;
        if logical_block >= start && logical_block < end {
            let offset = logical_block - start;
            let unwritten = ext.ee_len & EXT4_EXT_UNWRITTEN != 0;
            return Ok((ext.start_block() + offset, unwritten));
        }
    }
    Ok((0, false))
}

/// Write-time conversion for unwritten extents: mark the single logical
/// block WRITTEN, splitting its extent into up to three entries
/// (unwritten prefix / one written block / unwritten suffix) inside the
/// root node. Returns the physical block (which the caller must then
/// materialize — zero it — before any read-modify-write).
///
/// Returns `Ok(None)` when the logical block has no extent (hole) and
/// `Err(IOError)` for deep trees or when the split would overflow the
/// four root inline entries.
fn materialize_block_inline(
    i_block: &mut [u32; 15],
    logical_block: u64,
) -> Result<Option<u64>, i32> {
    // Copy out the scalars first — the mutable rewrite below reborrows
    // i_block, which the live &Ext4ExtentHeader borrow would conflict with.
    let (magic, mut n_entries, eh_max, depth): (u16, u16, u16, u16) = {
        let header = get_extent_header(i_block);
        (header.eh_magic, header.eh_entries, header.eh_max, header.eh_depth)
    };
    if magic != EXT4_EXT_MAGIC || depth != 0 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>())
        / core::mem::size_of::<Ext4Extent>();
    if n_entries as usize > max_entries {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Find the covering entry index.
    let mut hit: Option<usize> = None;
    {
        // SAFETY: entries live inside the 60-byte i_block after the
        // 12-byte header; count validated above.
        let entries = unsafe {
            core::slice::from_raw_parts(
                (i_block.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>())
                    as *const Ext4Extent,
                n_entries as usize
            )
        };
        for (i, ext) in entries.iter().enumerate() {
            let start = ext.ee_block as u64;
            let end = start + ext.length() as u64;
            if logical_block >= start && logical_block < end {
                if ext.ee_len & EXT4_EXT_UNWRITTEN == 0 {
                    // Already written — nothing to convert.
                    return Ok(Some(ext.start_block() + (logical_block - start)));
                }
                hit = Some(i);
                break;
            }
        }
    }
    let Some(idx) = hit else {
        return Ok(None); // hole
    };

    // Work on a byte view of the entry array for the split.
    // SAFETY: i_block is 60 bytes; header 12 bytes; entry idx*12 within
    // the remaining 48 bytes (validated via eh_entries above).
    let (ee_block, ee_len, phys) = unsafe {
        let e = ((i_block.as_mut_ptr() as *mut u8)
            .add(core::mem::size_of::<Ext4ExtentHeader>() + idx * core::mem::size_of::<Ext4Extent>())
            ) as *mut Ext4Extent;
        ((*e).ee_block, (*e).length(), (*e).start_block())
    };

    let prefix = (logical_block - ee_block as u64) as u16; // blocks before
    let suffix = ee_len - prefix - 1; // blocks after
    let needed_extra = (prefix > 0) as u16 + (suffix > 0) as u16;
    if n_entries + needed_extra > eh_max.min(max_entries as u16) {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Rebuild the entry list with the split pieces (single memmove-sized
    // shift; root arrays are at most 4 entries).
    // SAFETY: building on the in-place array; total entries <= 4*12+12=60.
    unsafe {
        let base = (i_block.as_mut_ptr() as *mut u8)
            .add(core::mem::size_of::<Ext4ExtentHeader>());
        let entries = core::slice::from_raw_parts_mut(base as *mut Ext4Extent, max_entries);
        // Shift the tail right by needed_extra slots.
        let tail_len = n_entries as usize - idx - 1;
        for i in (idx + 1..idx + 1 + tail_len).rev() {
            entries[i + needed_extra as usize] = entries[i];
        }
        let mut slot = idx;
        if prefix > 0 {
            entries[slot] = Ext4Extent {
                ee_block,
                ee_len: prefix | EXT4_EXT_UNWRITTEN,
                ee_start_hi: (phys >> 32) as u16,
                ee_start_lo: phys as u32,
            };
            slot += 1;
        }
        entries[slot] = Ext4Extent {
            ee_block: (ee_block as u64 + prefix as u64) as u32,
            ee_len: 1, // written
            ee_start_hi: ((phys + prefix as u64) >> 32) as u16,
            ee_start_lo: (phys + prefix as u64) as u32,
        };
        slot += 1;
        if suffix > 0 {
            entries[slot] = Ext4Extent {
                ee_block: (ee_block as u64 + prefix as u64 + 1) as u32,
                ee_len: suffix | EXT4_EXT_UNWRITTEN,
                ee_start_hi: ((phys + prefix as u64 + 1) >> 32) as u16,
                ee_start_lo: (phys + prefix as u64 + 1) as u32,
            };
        }
        // Update the entry count.
        let hdr = (i_block.as_mut_ptr() as *mut u8) as *mut Ext4ExtentHeader;
        (*hdr).eh_entries = n_entries + needed_extra;
        n_entries = (*hdr).eh_entries;
    }

    Ok(Some(phys + prefix as u64))
}

/// Insert a WRITTEN extent entry covering [ee_block, ee_block+len) at
/// physical `phys`, keeping the root array sorted by ee_block, merging
/// with a logically+physically contiguous neighbor when possible.
///
/// Used by the write path when a write lands in a HOLE of an extent file
/// (ftruncate-grow then write): a block is allocated and mapped here.
/// Returns Err when the four root inline slots are exhausted.
fn insert_written_inline(
    i_block: &mut [u32; 15],
    ee_block: u64,
    phys: u64,
    len: u16,
) -> Result<(), i32> {
    let (magic, mut n_entries, eh_max, depth) = {
        let h = get_extent_header(i_block);
        (h.eh_magic, h.eh_entries, h.eh_max, h.eh_depth)
    };
    if magic != EXT4_EXT_MAGIC || depth != 0 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    let max_entries = ((60 - core::mem::size_of::<Ext4ExtentHeader>())
        / core::mem::size_of::<Ext4Extent>()) as u16;
    if n_entries > eh_max.min(max_entries) {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // SAFETY: entry array sits after the 12-byte header in the 60-byte
    // i_block; the count was validated above.
    unsafe {
        let base = (i_block.as_mut_ptr() as *mut u8)
            .add(core::mem::size_of::<Ext4ExtentHeader>());
        let entries = core::slice::from_raw_parts_mut(base as *mut Ext4Extent, max_entries as usize);

        // Try merging with an adjacent entry first.
        for i in 0..n_entries as usize {
            let e = &mut entries[i];
            let start = e.ee_block as u64;
            let end = start + e.length() as u64;
            let e_unwritten = e.ee_len & EXT4_EXT_UNWRITTEN != 0;
            // Extend a preceding written extent.
            if !e_unwritten && end == ee_block && e.start_block() + e.length() as u64 == phys {
                e.ee_len += len;
                return Ok(());
            }
            // Extend a following written extent (ours ends where it starts).
            if !e_unwritten && start == ee_block + len as u64 && phys + len as u64 == e.start_block() {
                e.ee_block = ee_block as u32;
                e.ee_start_hi = (phys >> 32) as u16;
                e.ee_start_lo = phys as u32;
                e.ee_len += len;
                return Ok(());
            }
        }

        // New entry: find the insertion slot (sorted by ee_block).
        if n_entries >= eh_max.min(max_entries) {
            return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
        }
        let mut slot = n_entries as usize;
        for i in 0..n_entries as usize {
            if (entries[i].ee_block as u64) > ee_block {
                slot = i;
                break;
            }
        }
        // Shift the tail right one slot.
        for i in (slot..n_entries as usize).rev() {
            entries[i + 1] = entries[i];
        }
        entries[slot] = Ext4Extent {
            ee_block: ee_block as u32,
            ee_len: len,
            ee_start_hi: (phys >> 32) as u16,
            ee_start_lo: phys as u32,
        };
        let hdr = (i_block.as_mut_ptr() as *mut u8) as *mut Ext4ExtentHeader;
        n_entries += 1;
        (*hdr).eh_entries = n_entries;
    }
    Ok(())
}

/// Find physical block corresponding to logical block (using extent)
///
/// # Parameters
/// - `fs`: ext4 filesystem
/// - `i_block`: inode's i_block array
/// - `logical_block`: logical block number to find
///
/// # Returns
/// Physical block number, returns 0 if not found
pub fn ext4_ext_get_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<u64, i32> {
    let header = get_extent_header(i_block);

    // Verify magic
    if header.eh_magic != EXT4_EXT_MAGIC {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Validate eh_entries against eh_max
    if header.eh_entries > header.eh_max {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    find_block_in_extent_tree(fs, i_block, logical_block, 0)
}

/// Find logical block in extent tree
fn find_block_in_extent_tree(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u32; 15],
    logical_block: u64,
    depth: u32,
) -> Result<u64, i32> {
    // SAFETY: data is a &[u32; 15] (60 bytes), larger than ExtentExtentHeader (12 bytes).
    let header = unsafe { &*(data.as_ptr() as *const Ext4ExtentHeader) };

    if header.eh_depth == 0 {
        // Leaf node: search for extent in i_block array
        // Validate eh_entries: each entry is 12 bytes; after the 12-byte header,
        // at most (60 - 12) / 12 = 4 entries fit in i_block.
        let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>()) / core::mem::size_of::<Ext4Extent>();
        if header.eh_entries as usize > max_entries {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: offset by header size (12 bytes) stays within the 60-byte i_block array;
        // eh_entries was validated above against max_entries.
        let entries = unsafe {
            core::slice::from_raw_parts(
                (data.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
                header.eh_entries as usize
            )
        };

        for ext in entries {
            let start = ext.ee_block as u64;
            let end = start + ext.length() as u64;

            if logical_block >= start && logical_block < end {
                // Found! Calculate offset
                let offset = logical_block - start;
                return Ok(ext.start_block() + offset);
            }
        }

        // Not found
        Ok(0)
    } else {
        // Internal node: read index entries and recurse
        // Validate eh_entries: index entries are 12 bytes each, exactly like
        // leaf extents, so at most (60 - 12) / 12 = 4 fit in i_block. eh_max
        // comes from disk too and cannot be trusted on its own.
        let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>())
            / core::mem::size_of::<Ext4ExtentIdx>();
        if header.eh_entries as usize > max_entries {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: offset by header size (12 bytes) stays within the 60-byte i_block array;
        // eh_entries was validated above against max_entries.
        let indices = unsafe {
            core::slice::from_raw_parts(
                (data.as_ptr() as *const u8).add(core::mem::size_of::<Ext4ExtentHeader>())
                    as *const Ext4ExtentIdx,
                header.eh_entries as usize
            )
        };

        let mut child_block = 0u64;
        for idx in indices {
            if logical_block >= idx.ei_block as u64 {
                child_block = idx.leaf_block();
            } else {
                break;
            }
        }

        if child_block == 0 {
            return Ok(0);
        }

        find_block_in_external_extent(fs, child_block, logical_block, depth + 1)
    }
}

/// Like find_block_in_external_extent but also reports the UNWRITTEN
/// flag of the covering extent (deep trees can hold fallocate
/// preallocations — reads of those must zero-fill, not hit disk).
fn find_block_in_external_extent_ex(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_num: u64,
    logical_block: u64,
    depth: u32,
) -> Result<(u64, bool), i32> {
    if depth > 5 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // SAFETY: bio::bread returns a valid buffer_head; casts below stay
    // within the block-sized b_data.
    unsafe {
        let bh = bio::bread(fs.device, block_num)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;
        let data = &(*bh).b_data;
        let header = &*(data.as_ptr() as *const Ext4ExtentHeader);
        if header.eh_magic != EXT4_EXT_MAGIC {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let max_slots = (fs.block_size as usize - core::mem::size_of::<Ext4ExtentHeader>())
            / core::mem::size_of::<Ext4Extent>();
        if header.eh_entries > header.eh_max || header.eh_max as usize > max_slots {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let result = if header.eh_depth == 0 {
            let entries = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
                header.eh_entries as usize,
            );
            let mut hit = Ok((0u64, false));
            for ext in entries {
                let start = ext.ee_block as u64;
                let end = start + ext.length() as u64;
                if logical_block >= start && logical_block < end {
                    let offset = logical_block - start;
                    let unwritten = ext.ee_len & EXT4_EXT_UNWRITTEN != 0;
                    hit = Ok((ext.start_block() + offset, unwritten));
                    break;
                }
            }
            hit
        } else {
            let indices = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx,
                header.eh_entries as usize,
            );
            let mut child_block = 0u64;
            for idx in indices {
                if logical_block >= idx.ei_block as u64 {
                    child_block = idx.leaf_block();
                } else {
                    break;
                }
            }
            if child_block == 0 {
                Ok((0, false))
            } else {
                find_block_in_external_extent_ex(fs, child_block, logical_block, depth + 1)
            }
        };
        bio::brelse(bh);
        result
    }
}

/// Read extent from external block and find logical block
fn find_block_in_external_extent(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_num: u64,
    logical_block: u64,
    depth: u32,
) -> Result<u64, i32> {
    // ext4 extent trees are at most 5 levels deep (EXT4_MAX_EXTENT_DEPTH);
    // a deeper tree means on-disk corruption or a block cycle.
    if depth > 5 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // SAFETY: bio::bread returns a valid buffer_head whose b_data is a properly
    // aligned block-sized byte slice; the subsequent casts to Ext4ExtentHeader,
    // Ext4Extent, and Ext4ExtentIdx are within this buffer and eh_entries is
    // validated against the on-disk header.
    unsafe {
        let bh = bio::bread(fs.device, block_num)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &(*bh).b_data;
        let header = &*(data.as_ptr() as *const Ext4ExtentHeader);

        if header.eh_magic != EXT4_EXT_MAGIC {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        // Validate eh_entries against eh_max, and eh_max itself against the
        // actual buffer capacity ((block_size - 12) / 12 slots): both fields
        // come from disk and may be forged on a corrupted image.
        let max_slots = (fs.block_size as usize - core::mem::size_of::<Ext4ExtentHeader>())
            / core::mem::size_of::<Ext4Extent>();
        if header.eh_entries > header.eh_max
            || header.eh_max as usize > max_slots
        {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        if header.eh_depth == 0 {
            // Leaf node
            let entries = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
                header.eh_entries as usize
            );

            for ext in entries {
                let start = ext.ee_block as u64;
                let end = start + ext.length() as u64;

                if logical_block >= start && logical_block < end {
                    let offset = logical_block - start;
                    let unwritten = ext.ee_len & EXT4_EXT_UNWRITTEN != 0;
                    bio::brelse(bh);
                    // Ok(phys) loses the unwritten flag; callers that need
                    // it use find_block_external_ex below.
                    let _ = unwritten;
                    return Ok(ext.start_block() + offset);
                }
            }

            bio::brelse(bh);
            Ok(0)
        } else {
            // Internal node: recursive search
            let indices = core::slice::from_raw_parts(
                data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx,
                header.eh_entries as usize
            );

            // Binary search for appropriate index
            let mut child_block = 0;
            for idx in indices {
                if logical_block >= idx.ei_block as u64 {
                    child_block = idx.leaf_block();
                } else {
                    break;
                }
            }

            bio::brelse(bh);

            if child_block == 0 {
                return Ok(0);
            }

            // Recursive search
            find_block_in_external_extent(fs, child_block, logical_block, depth + 1)
        }
    }
}

// ============================================================================
// Multi-level extent tree (external index/leaf nodes)
//
// The root node lives in inode.i_block (60 bytes = 4 entries max). Real
// Linux ext4 grows the tree into external 4KB nodes as soon as the root
// overflows. Before this engine existed, every extent writer refused at 4
// entries with ENOSPC — on a fragmented filesystem a large fallocate
// (LTP tst_acquire_device preallocates a 300MB scratch image) failed
// MIDWAY, leaving a partial file LTP never cleans; a few such files drove
// the whole filesystem into ENOSPC and every later mkdir/mknod/write in
// the sweep chunk failed (the r3 mkdir04/mkdir05/mkdirat01/mknod02/
// mknodat01/lseek07 regression family).
//
// All writers append at the LOGICAL END (files grow forward), so the
// engine implements the ext4 rightmost-split strategy: a full node splits
// off a new rightmost sibling instead of rebalancing the whole tree.
// ============================================================================

/// A materialized extent entry (unwritten flag carried separately from
/// the u15 length field).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawExtent {
    pub ee_block: u32,
    pub phys: u64,
    pub len: u16,
    pub unwritten: bool,
}

impl RawExtent {
    pub fn end(&self) -> u64 {
        self.ee_block as u64 + self.len as u64
    }

    fn from_disk(e: &Ext4Extent) -> Self {
        RawExtent {
            ee_block: e.ee_block,
            phys: e.start_block(),
            len: e.length(),
            unwritten: e.ee_len & EXT4_EXT_UNWRITTEN != 0,
        }
    }

    fn to_disk(&self) -> Ext4Extent {
        Ext4Extent {
            ee_block: self.ee_block,
            ee_len: if self.unwritten { self.len | EXT4_EXT_UNWRITTEN } else { self.len },
            ee_start_hi: (self.phys >> 32) as u16,
            ee_start_lo: self.phys as u32,
        }
    }
}

/// Root-node capacity: (60 - 12) / 12 inline entries.
const ROOT_NODE_CAP: usize = 4;

/// A child-pointer split propagated to the parent after a node split.
struct SplitIdx {
    ei_block: u32,
    child: u64,
}

/// External-node capacity for this filesystem's block size.
fn external_node_cap(fs: &crate::fs::ext4::Ext4FileSystem) -> usize {
    ((fs.block_size as usize - core::mem::size_of::<Ext4ExtentHeader>())
        / core::mem::size_of::<Ext4Extent>()) as usize
}

/// Header/entries accessors over a raw node buffer (root 60B or 4KB node).
/// SAFETY requirement: `data.len() >= 12 + 12 * cap.
fn node_header(data: &[u8]) -> &Ext4ExtentHeader {
    unsafe { &*(data.as_ptr() as *const Ext4ExtentHeader) }
}

fn node_header_mut(data: &mut [u8]) -> &mut Ext4ExtentHeader {
    unsafe { &mut *(data.as_mut_ptr() as *mut Ext4ExtentHeader) }
}

/// SAFETY: caller guarantees the buffer holds `count` 12-byte entries
/// after the 12-byte header (root: 60 bytes; external: block_size).
unsafe fn node_extents(data: &[u8], count: usize) -> &[Ext4Extent] {
    core::slice::from_raw_parts(
        data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
        count,
    )
}

unsafe fn node_extents_mut(data: &mut [u8], count: usize) -> &mut [Ext4Extent] {
    core::slice::from_raw_parts_mut(
        data.as_mut_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *mut Ext4Extent,
        count,
    )
}

unsafe fn node_indices(data: &[u8], count: usize) -> &[Ext4ExtentIdx] {
    core::slice::from_raw_parts(
        data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx,
        count,
    )
}

unsafe fn node_indices_mut(data: &mut [u8], count: usize) -> &mut [Ext4ExtentIdx] {
    core::slice::from_raw_parts_mut(
        data.as_mut_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *mut Ext4ExtentIdx,
        count,
    )
}

/// Allocate one metadata (index/leaf) block, zero it, and account the
/// sectors into `meta_sectors`.
fn alloc_meta_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    near_phys: u64,
    meta_sectors: &mut u64,
) -> Result<u64, i32> {
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    let goal = if fs.blocks_per_group > 0 {
        ((near_phys / fs.blocks_per_group as u64) as u32).min(fs.group_count.saturating_sub(1))
    } else {
        0
    };
    let blk = allocator.alloc_block(goal)?;
    *meta_sectors += (fs.block_size / 512) as u64;
    // SAFETY: fs.device is valid; blk is freshly allocated.
    unsafe {
        let bh = bio::bread(fs.device, blk).ok_or(errno::Errno::IOError.as_neg_i32())?;
        for byte in (*bh).b_data.iter_mut() {
            *byte = 0;
        }
        (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
        let sync_res = bio::sync_dirty_buffer(bh);
        bio::brelse(bh);
        sync_res?;
    }
    Ok(blk)
}

/// Read one external node into a fresh buffer.
/// SAFETY: blk is a valid filesystem block number on fs.device.
unsafe fn read_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    blk: u64,
) -> Result<alloc::vec::Vec<u8>, i32> {
    let bh = bio::bread(fs.device, blk).ok_or(errno::Errno::IOError.as_neg_i32())?;
    let data = (*bh).b_data.to_vec();
    bio::brelse(bh);
    Ok(data)
}

/// Write one external node back to disk (dirty + sync).
/// SAFETY: blk is a valid filesystem block; data.len() == fs.block_size.
unsafe fn write_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    blk: u64,
    data: &[u8],
) -> Result<(), i32> {
    let bh = bio::bread(fs.device, blk).ok_or(errno::Errno::IOError.as_neg_i32())?;
    (*bh).b_data.copy_from_slice(data);
    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    let sync_res = bio::sync_dirty_buffer(bh);
    bio::brelse(bh);
    sync_res
}

/// Initialize an empty node header in `data` (already zeroed).
fn init_node_header(data: &mut [u8], depth: u16, cap: u16) {
    let h = node_header_mut(data);
    h.eh_magic = EXT4_EXT_MAGIC;
    h.eh_entries = 0;
    h.eh_max = cap;
    h.eh_depth = depth;
    h.eh_generation = 0;
}

/// Recursive append into the subtree rooted at the node held in `data`.
///
/// `depth == 0` treats the node as a leaf, `depth >= 1` as an index.
/// Returns `Ok(Some(split))` when a new rightmost sibling was created and
/// the PARENT must insert `split`; `Ok(None)` when the entry was placed
/// (data mutated in place — caller persists external nodes itself).
fn append_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &mut [u8],
    cap: usize,
    depth: u16,
    e: &RawExtent,
    meta_sectors: &mut u64,
) -> Result<Option<SplitIdx>, i32> {
    if depth == 0 {
        let n = node_header(data).eh_entries as usize;
        if n > cap {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // Merge with the last entry (same state, logical+physical
        // contiguity, u15 headroom).
        if n > 0 {
            // SAFETY: n <= cap entries fit in the buffer per the caller's
            // contract (root 60B/4, external block_size/cap).
            let last = unsafe { &mut node_extents_mut(data, cap)[n - 1] };
            let last_raw = RawExtent::from_disk(last);
            if last_raw.end() == e.ee_block as u64
                && last_raw.phys + last_raw.len as u64 == e.phys
                && last_raw.unwritten == e.unwritten
                && last_raw.len as u64 + e.len as u64 <= 0x7FFE
            {
                last.ee_len = (last_raw.len + e.len) | if e.unwritten { EXT4_EXT_UNWRITTEN } else { 0 };
                return Ok(None);
            }
        }
        if n < cap {
            // SAFETY: slot n < cap is within the buffer contract.
            let slot = unsafe { &mut node_extents_mut(data, cap)[n] };
            *slot = e.to_disk();
            node_header_mut(data).eh_entries = (n + 1) as u16;
            return Ok(None);
        }
        // Leaf full: new rightmost sibling leaf holding just `e`.
        let blk = alloc_meta_block(fs, e.phys, meta_sectors)?;
        let mut nd = alloc::vec![0u8; fs.block_size as usize];
        // The sibling is a FULL-SIZE external node: its eh_max must be the
        // external capacity, NOT this node's `cap`. When the split fired
        // on the inline root (cap=4), the sibling went to disk with
        // eh_max=4; later appends descend with external_node_cap and grew
        // the entry count past the stored max (entries=5 > max=4), and
        // every subsequent read of the node failed validation with EIO —
        // writes into fallocate-preallocated files past the first tree
        // promotion died (LTP mkfs-on-loop, the access04/acct01 chain).
        let ext_cap = external_node_cap(fs);
        init_node_header(&mut nd, 0, ext_cap as u16);
        // SAFETY: fresh block-size buffer with ext_cap entries fitting.
        unsafe { node_extents_mut(&mut nd, ext_cap)[0] = e.to_disk() };
        node_header_mut(&mut nd).eh_entries = 1;
        // SAFETY: blk was just allocated (valid fs block).
        unsafe { write_node(fs, blk, &nd)? };
        Ok(Some(SplitIdx { ei_block: e.ee_block, child: blk }))
    } else {
        let n = node_header(data).eh_entries as usize;
        if n == 0 || n > cap {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: index entries fit per the buffer contract.
        let (last_child, _last_ei) = unsafe {
            let idxs = node_indices(data, cap);
            (idxs[n - 1].leaf_block(), idxs[n - 1].ei_block)
        };
        // SAFETY: last_child is a valid fs block read from the index node.
        let mut child = unsafe { read_node(fs, last_child)? };
        match append_node(fs, &mut child, external_node_cap(fs), depth - 1, e, meta_sectors)? {
            None => {
                // SAFETY: last_child valid.
                unsafe { write_node(fs, last_child, &child)? };
                Ok(None)
            }
            Some(split) => {
                if n < cap {
                    // SAFETY: slot n < cap within contract.
                    unsafe {
                        let slot = &mut node_indices_mut(data, cap)[n];
                        slot.ei_block = split.ei_block;
                        slot.ei_leaf_lo = split.child as u32;
                        slot.ei_leaf_hi = (split.child >> 32) as u16;
                        slot.ei_unused = 0;
                    }
                    node_header_mut(data).eh_entries = (n + 1) as u16;
                    Ok(None)
                } else {
                    // Index full: new rightmost sibling index node with
                    // just the split child. Same eh_max discipline as the
                    // leaf split above: the sibling is a full-size external
                    // node even when THIS node is the 4-slot inline root.
                    let blk = alloc_meta_block(fs, split.child, meta_sectors)?;
                    let mut nd = alloc::vec![0u8; fs.block_size as usize];
                    let ext_cap = external_node_cap(fs);
                    init_node_header(&mut nd, depth, ext_cap as u16);
                    // SAFETY: fresh block-size buffer, ext_cap entries fit.
                    unsafe {
                        let slot = &mut node_indices_mut(&mut nd, ext_cap)[0];
                        slot.ei_block = split.ei_block;
                        slot.ei_leaf_lo = split.child as u32;
                        slot.ei_leaf_hi = (split.child >> 32) as u16;
                        slot.ei_unused = 0;
                    }
                    node_header_mut(&mut nd).eh_entries = 1;
                    // SAFETY: blk just allocated.
                    unsafe { write_node(fs, blk, &nd)? };
                    Ok(Some(SplitIdx { ei_block: split.ei_block, child: blk }))
                }
            }
        }
    }
}

/// Append one extent at the logical end of the tree rooted in `i_block`,
/// growing the tree into external nodes (depth increases) as needed.
///
/// `meta_sectors` accumulates the 512-byte sectors of index/leaf metadata
/// blocks allocated (the caller adds them to i_blocks).
pub fn ext4_ext_append(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    e: &RawExtent,
    meta_sectors: &mut u64,
) -> Result<(), i32> {
    // Work on a byte image of the 60-byte root.
    let mut root = alloc::vec![0u8; 60];
    root.copy_from_slice(unsafe {
        core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60)
    });
    let depth = node_header(&root).eh_depth;
    if node_header(&root).eh_magic != EXT4_EXT_MAGIC {
        init_node_header(&mut root, 0, ROOT_NODE_CAP as u16);
    }

    match append_node(fs, &mut root, ROOT_NODE_CAP, depth, e, meta_sectors)? {
        None => {}
        Some(split) => {
            // The root itself overflowed. Promote: move the current root
            // content down into a fresh node and install a new root above
            // it, then place the split child.
            let old_depth = depth;
            let old_cap = ROOT_NODE_CAP as u16;
            let down = alloc_meta_block(fs, split.child, meta_sectors)?;
            let mut down_data = alloc::vec![0u8; fs.block_size as usize];
            down_data[..60].copy_from_slice(&root);
            // Fix eh_max for the larger node (entries stay valid — same
            // layout, more slots).
            node_header_mut(&mut down_data).eh_max = external_node_cap(fs) as u16;
            // SAFETY: down just allocated.
            unsafe { write_node(fs, down, &down_data)? };
            let new_depth = old_depth + 1;
            init_node_header(&mut root, new_depth, old_cap);
            // SAFETY: root 60B holds 4 index entries per contract.
            unsafe {
                let first_ei = if old_depth == 0 {
                    // Downgraded leaf: first index key = first extent's ee_block.
                    let n = node_header(&down_data).eh_entries as usize;
                    if n == 0 { e.ee_block } else { node_extents(&down_data, external_node_cap(fs))[0].ee_block }
                } else {
                    let n = node_header(&down_data).eh_entries as usize;
                    if n == 0 { e.ee_block } else { node_indices(&down_data, external_node_cap(fs))[0].ei_block }
                };
                let slot = &mut node_indices_mut(&mut root, ROOT_NODE_CAP)[0];
                slot.ei_block = first_ei;
                slot.ei_leaf_lo = down as u32;
                slot.ei_leaf_hi = (down >> 32) as u16;
                slot.ei_unused = 0;
                let slot2 = &mut node_indices_mut(&mut root, ROOT_NODE_CAP)[1];
                slot2.ei_block = split.ei_block;
                slot2.ei_leaf_lo = split.child as u32;
                slot2.ei_leaf_hi = (split.child >> 32) as u16;
                slot2.ei_unused = 0;
            }
            node_header_mut(&mut root).eh_entries = 2;
        }
    }

    // Copy the root image back into i_block.
    // SAFETY: same-size copy (60 bytes) into the inode field.
    unsafe {
        core::ptr::copy_nonoverlapping(
            root.as_ptr(),
            i_block.as_mut_ptr() as *mut u8,
            60,
        );
    }
    Ok(())
}

/// Gather every leaf extent of the tree in logical order.
pub fn ext4_ext_gather(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
) -> Result<alloc::vec::Vec<RawExtent>, i32> {
    let mut out = alloc::vec::Vec::new();
    let root = unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) };
    let h = node_header(root);
    if h.eh_magic != EXT4_EXT_MAGIC {
        return Ok(out);
    }
    // SAFETY: root node access bounded by ROOT_NODE_CAP (60 bytes).
    unsafe {
        gather_node(fs, root, ROOT_NODE_CAP, h.eh_depth, &mut out)?;
    }
    Ok(out)
}

/// SAFETY: data holds cap 12-byte entries after the header.
unsafe fn gather_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u8],
    cap: usize,
    depth: u16,
    out: &mut alloc::vec::Vec<RawExtent>,
) -> Result<(), i32> {
    let h = node_header(data);
    let n = h.eh_entries as usize;
    if n > cap {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    if depth == 0 {
        for e in node_extents(data, n) {
            out.push(RawExtent::from_disk(e));
        }
        return Ok(());
    }
    for idx in node_indices(data, n) {
        let child = read_node(fs, idx.leaf_block())?;
        let ch = node_header(&child);
        if ch.eh_magic != EXT4_EXT_MAGIC {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        gather_node(fs, &child, external_node_cap(fs), ch.eh_depth, out)?;
    }
    Ok(())
}

/// Free every index/leaf METADATA block of the tree (not the data
/// extents). `freed_sectors` accumulates their 512-byte sectors.
pub fn ext4_ext_free_index_blocks(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    freed_sectors: &mut u64,
) -> Result<(), i32> {
    let root = unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) };
    let h = node_header(root);
    if h.eh_magic != EXT4_EXT_MAGIC || h.eh_depth == 0 {
        return Ok(());
    }
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    // SAFETY: root node access bounded by ROOT_NODE_CAP.
    unsafe { free_index_node(fs, root, ROOT_NODE_CAP, h.eh_depth, &allocator, freed_sectors) }
}

/// SAFETY: data holds cap 12-byte entries after the header.
unsafe fn free_index_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u8],
    cap: usize,
    depth: u16,
    allocator: &crate::fs::ext4::allocator::BlockAllocator,
    freed_sectors: &mut u64,
) -> Result<(), i32> {
    let h = node_header(data);
    let n = h.eh_entries as usize;
    if n > cap {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    for idx in node_indices(data, n) {
        let child_blk = idx.leaf_block();
        if depth >= 2 {
            let child = read_node(fs, child_blk)?;
            let ch = node_header(&child);
            if ch.eh_magic != EXT4_EXT_MAGIC {
                return Err(errno::Errno::IOError.as_neg_i32());
            }
            free_index_node(fs, &child, external_node_cap(fs), ch.eh_depth, allocator, freed_sectors)?;
        }
        allocator.free_block(child_blk)?;
        *freed_sectors += (fs.block_size / 512) as u64;
    }
    Ok(())
}

/// Rebuild the whole tree from `exts` (must be sorted by ee_block):
/// frees the old metadata blocks, resets the root, and appends each extent.
/// Returns the metadata sector delta (added - freed) for i_blocks.
pub fn ext4_ext_rebuild(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    exts: &[RawExtent],
) -> Result<i64, i32> {
    let mut freed: u64 = 0;
    ext4_ext_free_index_blocks(fs, i_block, &mut freed)?;
    init_node_header(
        // SAFETY: i_block is 60 bytes; header write is in-bounds.
        unsafe { core::slice::from_raw_parts_mut(i_block.as_mut_ptr() as *mut u8, 60) },
        0,
        ROOT_NODE_CAP as u16,
    );
    let mut added: u64 = 0;
    for e in exts {
        ext4_ext_append(fs, i_block, e, &mut added)?;
    }
    Ok(added as i64 - freed as i64)
}

/// Descend to the leaf node whose key range covers `logical_block`.
/// Returns the leaf block number (0 when the tree has no covering child).
/// SAFETY: i_block is 60 bytes.
fn find_covering_leaf_blk(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<u64, i32> {
    let root = unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) };
    let h = node_header(root);
    if h.eh_magic != EXT4_EXT_MAGIC || h.eh_depth == 0 {
        return Ok(0);
    }
    let n = h.eh_entries as usize;
    if n > ROOT_NODE_CAP {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // SAFETY: root holds up to ROOT_NODE_CAP index entries.
    let idxs = unsafe { node_indices(root, n) };
    let mut child = 0u64;
    for idx in idxs {
        if logical_block >= idx.ei_block as u64 {
            child = idx.leaf_block();
        } else {
            break;
        }
    }
    if child == 0 {
        // Left of the first key — use the first leaf.
        child = idxs[0].leaf_block();
    }
    // Descend intermediate index nodes.
    let mut depth = h.eh_depth;
    while depth > 1 {
        // SAFETY: child is a valid fs block read from an index node.
        let data = unsafe { read_node(fs, child)? };
        let ch = node_header(&data);
        if ch.eh_magic != EXT4_EXT_MAGIC || ch.eh_depth != depth - 1 {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let cn = ch.eh_entries as usize;
        if cn > external_node_cap(fs) {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: cn bounded by capacity.
        let cidxs = unsafe { node_indices(&data, cn) };
        let mut next = cidxs[0].leaf_block();
        for idx in cidxs {
            if logical_block >= idx.ei_block as u64 {
                next = idx.leaf_block();
            } else {
                break;
            }
        }
        child = next;
        depth -= 1;
    }
    Ok(child)
}

/// Write-time conversion for unwritten extents (any tree depth): mark the
/// single logical block WRITTEN, splitting its extent into up to three
/// entries. The covering node (inline root OR external leaf) is spliced
/// in place when it has room; a FULL node falls back to a whole-tree
/// gather/splice/rebuild (the old inline-only path returned EIO for a
/// 4-entry root — writes into fragmented preallocations failed).
/// Returns the physical block.
pub fn ext4_ext_materialize_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    logical_block: u64,
    meta_sectors: &mut u64,
) -> Result<Option<u64>, i32> {
    // Resolve the covering node: the inline root (depth 0) or the
    // external leaf reached through the index walk.
    let root = unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) };
    let rh = node_header(root);
    if rh.eh_magic != EXT4_EXT_MAGIC {
        return Ok(None);
    }
    let (mut node, node_blk, cap): (alloc::vec::Vec<u8>, Option<u64>, usize) = if rh.eh_depth == 0 {
        (root.to_vec(), None, ROOT_NODE_CAP)
    } else {
        let blk = find_covering_leaf_blk(fs, i_block, logical_block)?;
        if blk == 0 {
            return Ok(None); // hole (left of the first leaf's key)
        }
        // SAFETY: blk comes from the tree index walk.
        (unsafe { read_node(fs, blk)? }, Some(blk), external_node_cap(fs))
    };
    let lh = node_header(&node);
    let n = lh.eh_entries as usize;
    if lh.eh_magic != EXT4_EXT_MAGIC || n > cap {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // SAFETY: n bounded by cap per the node contract.
    let entries = unsafe { node_extents(&node, n) }.to_vec();
    let mut hit: Option<usize> = None;
    for (i, e) in entries.iter().enumerate() {
        let start = e.ee_block as u64;
        let end = start + e.length() as u64;
        if logical_block >= start && logical_block < end {
            if e.ee_len & EXT4_EXT_UNWRITTEN == 0 {
                return Ok(Some(e.start_block() + (logical_block - start)));
            }
            hit = Some(i);
            break;
        }
    }
    let Some(idx) = hit else {
        return Ok(None); // hole in this leaf
    };
    let raw = RawExtent::from_disk(&entries[idx]);
    let prefix = (logical_block - raw.ee_block as u64) as u16;
    let suffix = raw.len - prefix - 1;

    let mut pieces: alloc::vec::Vec<Ext4Extent> = alloc::vec::Vec::new();
    if prefix > 0 {
        pieces.push(RawExtent { ee_block: raw.ee_block, phys: raw.phys, len: prefix, unwritten: true }.to_disk());
    }
    pieces.push(RawExtent { ee_block: raw.ee_block + prefix as u32, phys: raw.phys + prefix as u64, len: 1, unwritten: false }.to_disk());
    if suffix > 0 {
        pieces.push(RawExtent { ee_block: raw.ee_block + prefix as u32 + 1, phys: raw.phys + prefix as u64 + 1, len: suffix, unwritten: true }.to_disk());
    }

    if n + pieces.len() - 1 <= cap {
        // Node-local splice.
        let mut new_entries: alloc::vec::Vec<Ext4Extent> = alloc::vec::Vec::with_capacity(n + 2);
        new_entries.extend_from_slice(&entries[..idx]);
        new_entries.extend_from_slice(&pieces);
        new_entries.extend_from_slice(&entries[idx + 1..]);
        let h = node_header_mut(&mut node);
        h.eh_entries = new_entries.len() as u16;
        // SAFETY: new_entries.len() <= cap fits the node buffer.
        unsafe { node_extents_mut(&mut node, cap)[..new_entries.len()].copy_from_slice(&new_entries) };
        match node_blk {
            Some(blk) => {
                // SAFETY: blk from the tree walk.
                unsafe { write_node(fs, blk, &node)? };
            }
            None => {
                // Root: copy the 60-byte image back.
                // SAFETY: same-size copy into i_block.
                unsafe {
                    core::ptr::copy_nonoverlapping(node.as_ptr(), i_block.as_mut_ptr() as *mut u8, 60);
                }
            }
        }
        Ok(Some(raw.phys + prefix as u64))
    } else {
        // Node full: whole-tree rebuild with the split applied.
        let mut all = ext4_ext_gather(fs, i_block)?;
        let mut spliced = false;
        for (i, e) in all.iter().enumerate() {
            if e.ee_block == raw.ee_block && e.phys == raw.phys && e.len == raw.len {
                let mut repl: alloc::vec::Vec<RawExtent> = alloc::vec::Vec::new();
                if prefix > 0 {
                    repl.push(RawExtent { ee_block: raw.ee_block, phys: raw.phys, len: prefix, unwritten: true });
                }
                repl.push(RawExtent { ee_block: raw.ee_block + prefix as u32, phys: raw.phys + prefix as u64, len: 1, unwritten: false });
                if suffix > 0 {
                    repl.push(RawExtent { ee_block: raw.ee_block + prefix as u32 + 1, phys: raw.phys + prefix as u64 + 1, len: suffix, unwritten: true });
                }
                all.splice(i..i + 1, repl);
                spliced = true;
                break;
            }
        }
        if !spliced {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let delta = ext4_ext_rebuild(fs, i_block, &all)?;
        *meta_sectors = (*meta_sectors as i64 + delta) as u64;
        Ok(Some(raw.phys + prefix as u64))
    }
}

/// Insert a WRITTEN extent (any tree depth): inline root keeps the sorted
/// fast path; deep trees insert into the covering leaf (rebuild fallback).
pub fn ext4_ext_insert_written(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    ee_block: u64,
    phys: u64,
    len: u16,
    meta_sectors: &mut u64,
) -> Result<(), i32> {
    {
        let root = unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) };
        let h = node_header(root);
        if h.eh_magic != EXT4_EXT_MAGIC {
            return Ok(());
        }
        if h.eh_depth == 0 {
            match insert_written_inline(i_block, ee_block, phys, len) {
                Ok(()) => return Ok(()),
                // Full inline root: fall through to the rebuild path
                // below (gather every extent, insert sorted, rebuild —
                // writing into a hole of a 4-fragment file used to
                // EIO here).
                Err(_) => {}
            }
            let mut all = ext4_ext_gather(fs, i_block)?;
            let pos = all
                .iter()
                .position(|e| e.ee_block as u64 > ee_block)
                .unwrap_or(all.len());
            all.insert(pos, RawExtent { ee_block: ee_block as u32, phys, len, unwritten: false });
            let delta = ext4_ext_rebuild(fs, i_block, &all)?;
            *meta_sectors = (*meta_sectors as i64 + delta) as u64;
            return Ok(());
        }
    }
    let leaf_blk = find_covering_leaf_blk(fs, i_block, ee_block)?;
    if leaf_blk == 0 {
        return insert_written_inline(i_block, ee_block, phys, len);
    }
    // SAFETY: leaf_blk from the tree walk.
    let mut leaf = unsafe { read_node(fs, leaf_blk)? };
    let cap = external_node_cap(fs);
    let lh = node_header(&leaf);
    let n = lh.eh_entries as usize;
    if lh.eh_magic != EXT4_EXT_MAGIC || n > cap {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // SAFETY: n bounded by cap.
    let entries = unsafe { node_extents(&leaf, n) }.to_vec();
    // Merge with the last entry when contiguous.
    if let Some(last) = entries.last() {
        let last_raw = RawExtent::from_disk(last);
        if last_raw.end() == ee_block && last_raw.phys + last_raw.len as u64 == phys {
            let mut ne = entries.clone();
            let l = ne.last_mut().unwrap();
            l.ee_len = (last_raw.len + len) & 0x7FFF;
            let h = node_header_mut(&mut leaf);
            h.eh_entries = ne.len() as u16;
            // SAFETY: ne.len() <= cap.
            unsafe { node_extents_mut(&mut leaf, cap)[..ne.len()].copy_from_slice(&ne) };
            // SAFETY: leaf_blk valid.
            unsafe { write_node(fs, leaf_blk, &leaf)? };
            return Ok(());
        }
    }
    if n < cap {
        // Sorted insert into the leaf.
        let mut new_entries = entries.clone();
        let pos = new_entries
            .iter()
            .position(|e| e.ee_block as u64 > ee_block)
            .unwrap_or(new_entries.len());
        new_entries.insert(
            pos,
            RawExtent { ee_block: ee_block as u32, phys, len, unwritten: false }.to_disk(),
        );
        let h = node_header_mut(&mut leaf);
        h.eh_entries = new_entries.len() as u16;
        // SAFETY: new_entries.len() <= cap.
        unsafe { node_extents_mut(&mut leaf, cap)[..new_entries.len()].copy_from_slice(&new_entries) };
        // SAFETY: leaf_blk valid.
        unsafe { write_node(fs, leaf_blk, &leaf)? };
        Ok(())
    } else {
        // Leaf full: whole-tree rebuild with the entry inserted sorted.
        let mut all = ext4_ext_gather(fs, i_block)?;
        let pos = all
            .iter()
            .position(|e| e.ee_block as u64 > ee_block)
            .unwrap_or(all.len());
        all.insert(pos, RawExtent { ee_block: ee_block as u32, phys, len, unwritten: false });
        let delta = ext4_ext_rebuild(fs, i_block, &all)?;
        *meta_sectors = (*meta_sectors as i64 + delta) as u64;
        Ok(())
    }
}
