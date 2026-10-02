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
        // Deep trees: report through the generic walker — it resolves the
        // leaf; unwritten in deep trees is not distinguished (accepted
        // approximation; our own allocator only builds root trees).
        let b = find_block_in_extent_tree(fs, i_block, logical_block, 0)?;
        return Ok((b, false));
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
pub fn ext4_ext_materialize_block(
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
pub fn ext4_ext_insert_written(
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
                    bio::brelse(bh);
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
