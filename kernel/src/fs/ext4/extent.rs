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

    /// Get the length (number of blocks), masking the initialized flag
    /// (bit 15). ext4 on-disk encoding (fio-2x lane, 2026-11-17): an
    /// extent of EXACTLY 32768 blocks is stored as ee_len == 0x8000 —
    /// the masked length reads 0. Any file with a >=128 MiB contiguous
    /// run (dd/mkfs-created test images, big rootfs files) decoded as a
    /// zero-length "unwritten" extent: reads zero-filled real data and
    /// writes fell into the materialize/rebuild storm. 0x8000 exactly =
    /// WRITTEN 32768; > 0x8000 = unwritten (len = ee_len - 0x8000).
    pub fn length(&self) -> u16 {
        let l = self.ee_len & 0x7FFF;
        if l == 0 {
            0x8000
        } else {
            l
        }
    }

    /// ext4 encoding discipline: unwritten means ee_len STRICTLY above
    /// the 0x8000 max-written marker (see `length`).
    pub fn is_unwritten(&self) -> bool {
        self.ee_len > EXT4_EXT_UNWRITTEN
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

/// Byte view of the root node's i_block array (INV-EXT-VIEW: same
/// 60-byte allocation, transmuted view in one place).
#[inline]
pub fn i_block_bytes(i_block: &[u32; 15]) -> &[u8] {
    // SAFETY: [u32; 15] and [u8; 60] are the same size and alignment
    // (4-byte aligned >= u8); the slice covers exactly the array.
    unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) }
}

/// Mutable twin of [`i_block_bytes`].
#[inline]
pub fn i_block_bytes_mut(i_block: &mut [u32; 15]) -> &mut [u8] {
    // SAFETY: as above; exclusive borrow.
    unsafe { core::slice::from_raw_parts_mut(i_block.as_mut_ptr() as *mut u8, 60) }
}

/// Parse extent header from i_block array
pub fn get_extent_header(i_block: &[u32; 15]) -> &Ext4ExtentHeader {
    // i_block is 60 bytes; the header is 12 — node_header's size guard
    // is structurally satisfied (INV-EXT-VIEW).
    node_header(i_block_bytes(i_block))
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
    ino: u32,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<(u64, bool), i32> {
    let header = get_extent_header(i_block);

    // ext4:ext4_ext_map_blocks_enter — the extent-mapping funnel's
    // request (one block per call at this funnel; flags 0: Rux passes
    // no EXT4_GET_BLOCKS_* mask).
    if crate::trace::EVENT_EXT4_EXT_MAP_BLOCKS_ENTER.armed() {
        crate::trace::trace_ext4_ext_map_blocks_enter(
            super::dev_id(fs),
            ino as u64,
            logical_block as u32,
            1,
            0,
        );
    }
    let r = ext4_ext_get_block_ex_inner(fs, i_block, logical_block);
    // ext4:ext4_ext_map_blocks_exit — the verdict: pblk/mflags/ret in
    // the reference's ext4_map_blocks return shape (1 mapped / 0 hole /
    // negative errno; unwritten extents map M|U with the real pblk).
    if crate::trace::EVENT_EXT4_EXT_MAP_BLOCKS_EXIT.armed() {
        let (pblk, len, mflags, ret) = match &r {
            Ok((b, unwritten)) => {
                if *b != 0 {
                    (*b, 1u32, if *unwritten { 0x810 } else { 0x10 }, 1i32)
                } else {
                    (0, 0, 0, 0)
                }
            }
            Err(e) => (0, 0, 0, *e),
        };
        crate::trace::trace_ext4_ext_map_blocks_exit(
            super::dev_id(fs),
            ino as u64,
            pblk,
            logical_block as u32,
            len,
            mflags,
            0,
            ret,
        );
    }
    r
}

/// Maximal mapped run covering `logical_block` (paired-test item 1,
/// 2026-11-17): returns `(physical start block, run length in blocks,
/// unwritten)`. The window `[logical_block, logical_block + len)` maps
/// one-to-one onto `[phys, phys + len)` — a whole extent resolved with
/// ONE tree walk, replacing the per-block `ext4_ext_get_block_ex` loop
/// that paid 1-2 cached breads per 4 KiB block on depth>=1 files (512
/// per 1 MiB direct read). Holes return `(0, tail_to_next_extent,
/// false)`; unwritten extents return the run with `unwritten = true`
/// (readers zero-fill, writers materialize per block).
pub fn ext4_ext_get_run(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<(u64, u64, bool), i32> {
    ext4_ext_get_run_impl(fs, ino, i_block, logical_block, false)
}

/// WRITE-side run resolver (fio-delalloc lane, 2026-11-18): identical walk
/// to `ext4_ext_get_run`, but an UNWRITTENT covering extent reports its
/// REAL physical run instead of phys 0 — the direct-write fast arm targets
/// unwritten extents directly (the reference kernel's DIO submits bios
/// over unwritten extents; the unwritten->written state conversion runs at
/// the write's completion, `ext4_ext_convert_unwritten`). READ callers
/// must keep using `ext4_ext_get_run` (unwritten reads zero-fill).
pub fn ext4_ext_get_run_write(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
    i_block: &[u32; 15],
    logical_block: u64,
) -> Result<(u64, u64, bool), i32> {
    ext4_ext_get_run_impl(fs, ino, i_block, logical_block, true)
}

fn ext4_ext_get_run_impl(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
    i_block: &[u32; 15],
    logical_block: u64,
    for_write: bool,
) -> Result<(u64, u64, bool), i32> {
    let _ = ino;
    let header = get_extent_header(i_block);
    if header.eh_magic != EXT4_EXT_MAGIC || header.eh_entries > header.eh_max {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Root-node leaves only for depth 0; deep trees walk down.
    if header.eh_depth != 0 {
        // INV-EXT-VIEW: idx slice bounded by ROOT_NODE_CAP.
        let idxs = node_indices(
            i_block_bytes(i_block),
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
            return Ok((0, 0, false));
        }
        return find_run_in_external_extent(fs, child, logical_block, 0, for_write);
    }

    let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>())
        / core::mem::size_of::<Ext4Extent>();
    if header.eh_entries as usize > max_entries {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // INV-EXT-VIEW: eh_entries validated against max_entries above.
    let entries = node_extents(i_block_bytes(i_block), header.eh_entries as usize);
    Ok(run_from_entries(entries, logical_block, for_write))
}

/// Binary search a leaf `entries` slice (sorted by ee_block) for the
/// entry covering `logical_block`. Returns the covering entry's index, or
/// the INVERSE index (bit 31 set, values are small) of the first entry
/// PAST `logical_block` (the hole-to-next-extent distance), or
/// u32::MAX when nothing lies ahead. Replaces the linear scans that made
/// every lookup of a fragmented tree O(entries) — the fsw br=25k-class
/// CPU burn on depth>=1 files (wrfsync lane, 2026-11-17).
#[inline]
fn search_extent_entry(entries: &[Ext4Extent], logical_block: u64) -> u32 {
    let mut lo = 0usize;
    let mut hi = entries.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if entries[mid].ee_block as u64 <= logical_block {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // lo = first entry with ee_block > logical_block; lo-1 = candidate.
    if lo > 0 {
        let i = lo - 1;
        let start = entries[i].ee_block as u64;
        if logical_block < start + entries[i].length() as u64 {
            return i as u32;
        }
    }
    if lo < entries.len() {
        0x8000_0000 | lo as u32 // hole: index of the next entry, marked
    } else {
        u32::MAX // hole: nothing ahead
    }
}

/// Scan leaf `entries` for the extent covering `logical_block`; return
/// the maximal run from that point. Holes report the distance to the
/// next extent's start (length 0 means "nothing known ahead" — callers
/// re-resolve per block there).
#[inline]
fn run_from_entries(entries: &[Ext4Extent], logical_block: u64, for_write: bool) -> (u64, u64, bool) {
    let r = search_extent_entry(entries, logical_block);
    if r != u32::MAX && r & 0x8000_0000 == 0 {
        let ext = &entries[r as usize];
        let start = ext.ee_block as u64;
        let offset = logical_block - start;
        let unwritten = ext.is_unwritten();
        // READS of unwritten blocks zero-fill (callers treat phys 0 as a
        // hole-shaped stretch); the WRITE-side resolver (`for_write`) needs
        // the REAL physical run so a direct write can target unwritten
        // extents — the reference kernel's DIO shape (bios over unwritten
        // extents; state conversion at completion).
        let phys = if unwritten && !for_write { 0 } else { ext.start_block() + offset };
        let len = ext.length() as u64 - offset;
        return (phys, len, unwritten);
    }
    if r == u32::MAX {
        (0, 0, false)
    } else {
        let next = (r & 0x7FFF_FFFF) as usize;
        let next_start = entries[next].ee_block as u64;
        (0, next_start.saturating_sub(logical_block), false)
    }
}

/// Deep-tree run resolver: one walk per level, whole-extent verdict.
/// `block_num` must be a valid extent-tree block of this fs (walker
/// discipline); the on-disk views go through the INV-EXT-VIEW accessors.
fn find_run_in_external_extent(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_num: u64,
    logical_block: u64,
    depth: u32,
    for_write: bool,
) -> Result<(u64, u64, bool), i32> {
    if depth > 5 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // SAFETY: bh is pinned by bread; b_data is a block-sized slice.
    let bh = unsafe {
        bio::bread(fs.device, block_num).ok_or(errno::Errno::IOError.as_neg_i32())?
    };
    let data = unsafe { &(*bh).b_data };
    {
        let header = node_header(data);
        if header.eh_magic != EXT4_EXT_MAGIC || header.eh_entries > header.eh_max {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let max_slots = (fs.block_size as usize - core::mem::size_of::<Ext4ExtentHeader>())
            / core::mem::size_of::<Ext4Extent>();
        if header.eh_max as usize > max_slots {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let result = if header.eh_depth == 0 {
            let entries = node_extents(data, header.eh_entries as usize);
            Ok(run_from_entries(entries, logical_block, for_write))
        } else {
            let indices = node_indices(data, header.eh_entries as usize);
            let mut child_block = 0u64;
            for idx in indices {
                if logical_block >= idx.ei_block as u64 {
                    child_block = idx.leaf_block();
                } else {
                    break;
                }
            }
            if child_block == 0 {
                Ok((0, 0, false))
            } else {
                find_run_in_external_extent(fs, child_block, logical_block, depth + 1, for_write)
            }
        };
        bio::brelse(bh);
        result
    }
}

fn ext4_ext_get_block_ex_inner(
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
        // INV-EXT-VIEW: idx slice bounded by ROOT_NODE_CAP.
        let idxs = node_indices(
            i_block_bytes(i_block),
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
        // SAFETY: child is a valid extent-tree block (walker discipline).
        return unsafe { find_block_in_external_extent_ex(fs, child, logical_block, 0) };
    }

    let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>())
        / core::mem::size_of::<Ext4Extent>();
    if header.eh_entries as usize > max_entries {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    // INV-EXT-VIEW: eh_entries validated against max_entries above.
    let entries = node_extents(i_block_bytes(i_block), header.eh_entries as usize);

    for ext in entries {
        let start = ext.ee_block as u64;
        let end = start + ext.length() as u64;
        if logical_block >= start && logical_block < end {
            let offset = logical_block - start;
            let unwritten = ext.is_unwritten();
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
        // INV-EXT-VIEW: count validated above.
        let entries = node_extents(i_block_bytes(i_block), n_entries as usize);
        for (i, ext) in entries.iter().enumerate() {
            let start = ext.ee_block as u64;
            let end = start + ext.length() as u64;
            if logical_block >= start && logical_block < end {
                if !ext.is_unwritten() {
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

    // Work on a mutable entry view for the split (INV-EXT-VIEW: idx was
    // found within the validated entry count).
    let (ee_block, ee_len, phys) = {
        let entries = node_extents_mut(i_block_bytes_mut(i_block), n_entries as usize);
        let e = &entries[idx];
        (e.ee_block, e.length(), e.start_block())
    };

    let prefix = (logical_block - ee_block as u64) as u16; // blocks before
    let suffix = ee_len - prefix - 1; // blocks after
    let needed_extra = (prefix > 0) as u16 + (suffix > 0) as u16;
    if n_entries + needed_extra > eh_max.min(max_entries as u16) {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Rebuild the entry list with the split pieces (single memmove-sized
    // shift; root arrays are at most 4 entries; INV-EXT-VIEW).
    {
        let entries = node_extents_mut(i_block_bytes_mut(i_block), max_entries);
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
        // Update the entry count (INV-EXT-VIEW).
        {
            let hdr = node_header_mut(i_block_bytes_mut(i_block));
            hdr.eh_entries = n_entries + needed_extra;
            n_entries = hdr.eh_entries;
        }
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

    // Entry array after the 12-byte header in the 60-byte i_block;
    // count validated above (INV-EXT-VIEW).
    {
        let entries = node_extents_mut(i_block_bytes_mut(i_block), max_entries as usize);

        // Try merging with an adjacent entry first.
        for i in 0..n_entries as usize {
            let e = &mut entries[i];
            let start = e.ee_block as u64;
            let end = start + e.length() as u64;
            let e_unwritten = e.is_unwritten();
            // u15 headroom (append_node's discipline, review EXT4-MERGE):
            // ee_len's bit 15 is the unwritten flag, so a written extent
            // can cover at most 0x7FFF blocks. `ee_len += len` without
            // the check wraps a max-length extent into a ZERO-length
            // unwritten one (silent mapping loss). Guard failed = no
            // merge, fall through to a separate entry.
            let fits = e.length() as u64 + len as u64 <= 0x7FFE;
            // Extend a preceding written extent.
            if !e_unwritten && fits && end == ee_block && e.start_block() + e.length() as u64 == phys {
                e.ee_len += len;
                return Ok(());
            }
            // Extend a following written extent (ours ends where it starts).
            if !e_unwritten && fits && start == ee_block + len as u64 && phys + len as u64 == e.start_block() {
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
        let hdr = node_header_mut(i_block_bytes_mut(i_block));
        n_entries += 1;
        hdr.eh_entries = n_entries;
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

    let r = find_block_in_extent_tree(fs, i_block, logical_block, 0);
    // rux:exthunt (retired dfx=exthunt): a read-side miss on a DEEP tree (depth >= 1) — the
    // reader is about to serve a hole for a file that grew past its
    // root. Print the root view so a stale cached root (old depth /
    // fewer entries) is distinguishable from a broken index block.
    if header.eh_depth >= 1
        && (r.is_err() || *r.as_ref().unwrap_or(&0) == 0)
        && crate::trace::EVENT_EXTHUNT.armed()
    {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static MISSES: AtomicUsize = AtomicUsize::new(0);
        if MISSES.fetch_add(1, Ordering::Relaxed) < 60 {
            crate::pr_info!(
                "EXT-HUNT-MISS logical={} root[magic={:x} depth={} entries={} max={}] -> {:?}",
                logical_block,
                header.eh_magic,
                header.eh_depth,
                header.eh_entries,
                header.eh_max,
                r
            );
        }
    }
    r
}

/// rux:exthunt (retired dfx=exthunt) read-side companion to EXT-HUNT-MISS: call wherever a read
/// path concludes "hole" (mapping 0) and is about to zero-fill — the page
/// cache batch fill, the synchronous fallback, and the O_DIRECT reader all
/// map through the CACHED inode's root, NOT through ext4_ext_get_block,
/// so EXT-HUNT-MISS alone cannot see the reader's verdict. Prints the
/// reader's root view so a stale/reverted cached root is distinguishable
/// from a legitimately sparse file: a file that already grew past its
/// root (see EXT-HUNT-GROW) should never again report a shallow root.
pub fn exthunt_read_hole(ino: u32, i_block: &[u32; 15], logical_block: u64) {
    if !crate::trace::EVENT_EXTHUNT.armed() {
        return;
    }
    use core::sync::atomic::{AtomicUsize, Ordering};
    // Hole verdicts are reads too: register the ino so the following
    // writes (the C2 write-after-hole shape) are captured.
    hunt_watch(ino);
    static HOLES: AtomicUsize = AtomicUsize::new(0);
    if HOLES.fetch_add(1, Ordering::Relaxed) >= 80 {
        return;
    }
    crate::trace::trace_exthunt(0, ino, logical_block, 0);
    let header = get_extent_header(i_block);
    crate::pr_info!(
        "EXT-HUNT-READHOLE pid={} ino={} logical={} root[magic={:x} depth={} entries={} max={}]",
        crate::process::current_pid(),
        ino,
        logical_block,
        header.eh_magic,
        header.eh_depth,
        header.eh_entries,
        header.eh_max
    );
}

// ---- rux:exthunt (retired dfx=exthunt) write/read adjudication probes (ltp-fix-b leg-10) ----
//
// The ftest04/zzprobe4 all-zeros family needs the write-side vs read-side
// (logical -> phys) story on ONE boot: which physical block each side
// touched and which bytes the reader actually served. Static audit of the
// write path (buffer-cache deferred-dirty under the journal handle),
// read paths (page-cache fill through the block_cached coherence guard),
// fsync, and eviction found every link individually coherent — so the
// divergence point must be observed live:
//
// - WRPUT: every data-block copy in ext4_file_write (the writer's truth).
// - RDSERVE / RDBATCH / RDOD / RDHIT: the reader's resolutions — sync
//   fallback, batch-fill submit, O_DIRECT, and page-cache frame hit —
//   with the first two bytes each actually served.
//
// Read prints are filtered to inodes seen in a recent WRPUT (the watch
// ring) so boot-time I/O cannot eat the caps before the probe file's
// story is captured. OFF by default, like every exthunt probe.

const HUNT_CAP_WR: usize = 500;
const HUNT_CAP_RD: usize = 500;
const HUNT_WATCH: usize = 8;
// leg-11: raised so a full 5-file zzprobe4 pass (every chunk's data
// block + read prints incl. the O_DIRECT phase) fits before the caps.
const HUNT_CAP_WR2: usize = 3000;
const HUNT_CAP_RD2: usize = 3000;

static HUNT_WR_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static HUNT_RD_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static HUNT_WATCH_RING: [core::sync::atomic::AtomicU32; HUNT_WATCH] =
    [const { core::sync::atomic::AtomicU32::new(0) }; HUNT_WATCH];
static HUNT_WATCH_NEXT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn hunt_on() -> bool {
    crate::trace::EVENT_EXTHUNT.armed()
}

fn hunt_watch(ino: u32) {
    use core::sync::atomic::Ordering;
    let i = HUNT_WATCH_NEXT.fetch_add(1, Ordering::Relaxed) % HUNT_WATCH;
    HUNT_WATCH_RING[i].store(ino, Ordering::Relaxed);
}

fn hunt_watched(ino: u64) -> bool {
    use core::sync::atomic::Ordering;
    HUNT_WATCH_RING
        .iter()
        .any(|w| w.load(Ordering::Relaxed) as u64 == ino)
}

/// Writer truth: ext4_file_write copied `pat` into `phys` for `logical`.
/// `buf_ptr` is the destination buffer's b_data pointer — the read side
/// prints the served entry's pointer, so a mismatch proves two live
/// buffers for one key even when the dup counter never fired.
/// Only prints for watched inos (inos seen in a recent read) so kernel
/// append-only files (kmsg) cannot flood the console into a feedback
/// loop with printk persistence.
pub fn exthunt_wr_put(ino: u32, logical: u64, phys: u64, pat: u8, buf_ptr: *const u8) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    // Feed the phys watch ring unconditionally (watch-gated ino check
    // below only gates the print): the block-cache tracers key on phys.
    hunt_phys_watch(phys);
    if !hunt_watched(ino as u64) {
        return;
    }
    if HUNT_WR_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_WR2 {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-WRPUT pid={} ino={} logical={} phys={} pat={:02x} buf={:p}",
        crate::process::current_pid(),
        ino,
        logical,
        phys,
        pat,
        buf_ptr
    );
}

/// Reader truth: synchronous single-block serve — `b0 b1` are the first
/// two bytes the buffer actually handed out.
pub fn exthunt_rd_serve(ino: u64, logical: u64, phys: u64, b0: u8, b1: u8) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    hunt_watch(ino as u32);
    if HUNT_RD_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_RD2 {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-RDSERVE pid={} ino={} logical={} phys={} b0={:02x}{:02x}",
        crate::process::current_pid(),
        ino,
        logical,
        phys,
        b0,
        b1
    );
}

/// Reader truth: batch fill submitted a coalesced run starting at
/// `logical` -> `phys` (bread_async_multi; single-block runs delegate to
/// the cache-aware bread_async).
pub fn exthunt_rd_batch(ino: u64, logical: u64, phys: u64, nblocks: u32) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    hunt_watch(ino as u32);
    if HUNT_RD_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_RD2 {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-RDBATCH pid={} ino={} logical={} phys={} n={}",
        crate::process::current_pid(),
        ino,
        logical,
        phys,
        nblocks
    );
}

/// Reader truth: O_DIRECT serve — first two bytes off the block.
pub fn exthunt_rd_od(ino: u64, logical: u64, phys: u64, b0: u8, b1: u8, buf_ptr: *const u8) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    hunt_watch(ino as u32);
    if HUNT_RD_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_RD2 {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-RDOD pid={} ino={} logical={} phys={} b0={:02x}{:02x} buf={:p}",
        crate::process::current_pid(),
        ino,
        logical,
        phys,
        b0,
        b1,
        buf_ptr
    );
}

/// Reader truth: page-cache frame hit — first two bytes of the served
/// frame (a poisoned frame shows zeros here long after any disk/block
/// state was repaired).
pub fn exthunt_rd_hit(ino: u64, logical: u64, b0: u8, b1: u8) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    hunt_watch(ino as u32);
    if HUNT_RD_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_RD2 {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-RDHIT pid={} ino={} logical={} b0={:02x}{:02x}",
        crate::process::current_pid(),
        ino,
        logical,
        b0,
        b1
    );
}

// ---- rux:exthunt (retired dfx=exthunt) block-cache eviction/fabrication tracers (leg-11) ----
//
// The last open gap between EXT-HUNT-WRPUT pat=XX and EXT-HUNT-RDHIT
// b0=0000 on identical (logical, phys): a read DMA that races an
// unhashed evicting dirty victim. evict_one() unhooks the victim from
// the hash (evicting=true) BEFORE its Phase-4 sync writes the dirty
// data to disk; every cache lookup (bread_async Phase 1, get() Phase 1,
// block_cached) skips evicting entries, so in that window the block
// looks absent and a fresh entry is fabricated on top of STALE disk
// bytes — the guard cannot see an UNHASHED victim. These tracers close
// the gap on one boot:
//
//   EVICT   blk=P dirty=1 — a watched block left the hash (data now
//            lives only in the victim's buffer)
//   EVICTED blk=P rc=0     — the eviction's sync landed (disk truth)
//   FAB     blk=P skip=1   — a cache miss fabricated a fresh entry;
//            skip=1 means Phase 1 walked past an evicting same-key
//            entry (the miss was eviction-caused)
//   BCBLIND blk=P          — block_cached() returned false for a block
//            whose evicting same-key entry sits in the chain (the
//            coherence guard's blind spot)
//
// The phys watch ring is fed by WRPUT (the writer's physical blocks);
// FAB also prints unwatched blocks when skip=1 (the rare adjudicating
// event), all under a print cap. OFF by default, like every exthunt.

const HUNT_CAP_EV: usize = 400;
const HUNT_PHYS: usize = 2048;

static HUNT_EV_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static HUNT_FAB_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static HUNT_PHYS_RING: [core::sync::atomic::AtomicU64; HUNT_PHYS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; HUNT_PHYS];
static HUNT_PHYS_NEXT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn hunt_phys_watch(phys: u64) {
    use core::sync::atomic::Ordering;
    let i = HUNT_PHYS_NEXT.fetch_add(1, Ordering::Relaxed) % HUNT_PHYS;
    HUNT_PHYS_RING[i].store(phys, Ordering::Relaxed);
}

/// Is this physical block in the WRPUT-fed watch set? Called from the
/// block-cache layer (bio.rs), which has no inode context.
pub fn exthunt_phys_watched(blocknr: u64) -> bool {
    use core::sync::atomic::Ordering;
    HUNT_PHYS_RING
        .iter()
        .any(|w| w.load(Ordering::Relaxed) == blocknr)
}

/// An eviction unhashed a watched block; `dirty` is the victim's state
/// at the sync point (dirty data exists only in that victim).
pub fn exthunt_evict_unhash(blocknr: u64, dirty: bool) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    if HUNT_EV_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_EV {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-EVICT pid={} blk={} dirty={}",
        crate::process::current_pid(),
        blocknr,
        dirty as u8
    );
}

/// The eviction's Phase-4 sync returned `rc` for a watched block — the
/// disk-truth point a racing fabrication could have missed.
pub fn exthunt_evict_done(blocknr: u64, rc: i32) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    if HUNT_EV_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_EV {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-EVICTED pid={} blk={} rc={}",
        crate::process::current_pid(),
        blocknr,
        rc
    );
}

/// A cache miss fabricated a fresh entry (Phase 2). `skipped_evicting`
/// records whether the Phase-1 walk stepped over an evicting entry with
/// the SAME key — the miss was eviction-caused, and the fabricated DMA
/// may be reading disk the victim's sync has not updated yet.
pub fn exthunt_fab(blocknr: u64, skipped_evicting: bool, sync_path: bool) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    if !skipped_evicting && !exthunt_phys_watched(blocknr) {
        return;
    }
    if HUNT_FAB_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_EV {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-FAB pid={} blk={} skip={} path={}",
        crate::process::current_pid(),
        blocknr,
        skipped_evicting as u8,
        if sync_path { "sync" } else { "async" }
    );
}

/// block_cached() was about to return false while an evicting same-key
/// entry sits in the chain — the coherence guard's blind spot that lets
/// the multi-block direct DMA bypass a dirty victim mid-eviction.
pub fn exthunt_bc_blind(blocknr: u64) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    if HUNT_EV_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_EV {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-BCBLIND pid={} blk={}",
        crate::process::current_pid(),
        blocknr
    );
}

/// Block-cache hit truth (bread_async Phase 1): the entry's buffer
/// pointer and first two bytes at the moment of the serve. The WRPUT
/// print carries the writer's buffer pointer — a mismatch on the same
/// key proves two live buffers where the dup counter saw none.
pub fn exthunt_rxbhit(blocknr: u64, buf_ptr: *const u8, b0: u8, b1: u8) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    if !exthunt_phys_watched(blocknr) {
        return;
    }
    if HUNT_FAB_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_EV {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-RXBHIT pid={} blk={} b0={:02x}{:02x} buf={:p}",
        crate::process::current_pid(),
        blocknr,
        b0,
        b1,
        buf_ptr
    );
}

/// Extent-path zeroing truth: the write path is about to zero a block
/// via getblk_zero — `kind` is "mat" (unwritten materialization) or
/// "hole" (hole-fill allocation). Printed for watched inos; a hit here
/// AFTER a WRPUT for the same phys is the zero-overwrite smoking gun.
pub fn exthunt_zzero(kind: &str, ino: u32, logical: u64, phys: u64) {
    use core::sync::atomic::Ordering;
    if !hunt_on() {
        return;
    }
    hunt_phys_watch(phys);
    if !hunt_watched(ino as u64) {
        return;
    }
    if HUNT_EV_N.fetch_add(1, Ordering::Relaxed) >= HUNT_CAP_EV {
        return;
    }
    crate::pr_info!(
        "EXT-HUNT-ZZERO-{} pid={} ino={} logical={} phys={}",
        kind,
        crate::process::current_pid(),
        ino,
        logical,
        phys
    );
}

/// Find logical block in extent tree
fn find_block_in_extent_tree(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u32; 15],
    logical_block: u64,
    depth: u32,
) -> Result<u64, i32> {
    // INV-EXT-VIEW: i_block is 60 bytes (header 12 + entries).
    let header = node_header(i_block_bytes(data));

    if header.eh_depth == 0 {
        // Leaf node: search for extent in i_block array
        // Validate eh_entries: each entry is 12 bytes; after the 12-byte header,
        // at most (60 - 12) / 12 = 4 entries fit in i_block.
        let max_entries = (60 - core::mem::size_of::<Ext4ExtentHeader>()) / core::mem::size_of::<Ext4Extent>();
        if header.eh_entries as usize > max_entries {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let entries = node_extents(i_block_bytes(data), header.eh_entries as usize);

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
        // INV-EXT-VIEW: eh_entries validated above against max_entries.
        let indices = node_indices(i_block_bytes(data), header.eh_entries as usize);

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
        if header.eh_magic != EXT4_EXT_MAGIC
            || header.eh_entries > header.eh_max
        {
            // rux:exthunt (retired dfx=exthunt): the walk below the root just failed — the
            // reader will turn this Err into a hole (zeros). Dump the
            // block header + first bytes so zeros / foreign data / a
            // reverted header are distinguishable.
            if crate::trace::EVENT_EXTHUNT.armed() {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static BADS: AtomicUsize = AtomicUsize::new(0);
                if BADS.fetch_add(1, Ordering::Relaxed) < 40 {
                    let mut hex = [0u8; 48];
                    let mut n = 0usize;
                    for b in data.iter().take(16) {
                        let hi = b >> 4;
                        let lo = b & 0xf;
                        hex[n] = if hi < 10 { b'0' + hi } else { b'a' + hi - 10 };
                        hex[n + 1] = if lo < 10 { b'0' + lo } else { b'a' + lo - 10 };
                        n += 2;
                    }
                    crate::pr_info!(
                        "EXT-HUNT-IDXBAD block={} hdr[magic={:x} depth={} entries={} max={}] first16={}",
                        block_num,
                        header.eh_magic,
                        header.eh_depth,
                        header.eh_entries,
                        header.eh_max,
                        core::str::from_utf8(&hex[..n]).unwrap_or("??")
                    );
                }
            }
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let max_slots = (fs.block_size as usize - core::mem::size_of::<Ext4ExtentHeader>())
            / core::mem::size_of::<Ext4Extent>();
        if header.eh_max as usize > max_slots {
            bio::brelse(bh);
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let result = if header.eh_depth == 0 {
            let entries = node_extents(data, header.eh_entries as usize);
            // Binary search (entries sorted by ee_block): O(log n) per
            // lookup instead of the linear scan (wrfsync lane).
            let r = search_extent_entry(entries, logical_block);
            if r != u32::MAX && r & 0x8000_0000 == 0 {
                let ext = &entries[r as usize];
                let offset = logical_block - ext.ee_block as u64;
                Ok((ext.start_block() + offset, ext.is_unwritten()))
            } else {
                Ok((0u64, false))
            }
        } else {
            let indices = node_indices(data, header.eh_entries as usize);
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
        let header = node_header(data);

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
            let entries = node_extents(data, header.eh_entries as usize);

            for ext in entries {
                let start = ext.ee_block as u64;
                let end = start + ext.length() as u64;

                if logical_block >= start && logical_block < end {
                    let offset = logical_block - start;
                    let unwritten = ext.is_unwritten();
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
            let indices = node_indices(data, header.eh_entries as usize);

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
            unwritten: e.is_unwritten(),
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

// Header/entries accessors over a raw node buffer (root 60B or 4KB node).
//
// INV-EXT-VIEW (unsafe-slim round 2): these are the typed on-disk views
// the whole file routes through — each accessor bounds-guards its cast.
// Callers validate eh_entries against the node capacity beforehand (the
// tree walkers' standing checks); the guards only fire where the
// previous open-coded casts read out of bounds (UB, corrupt-disk class).
// The header accessors assert the 12-byte minimum: every caller passes
// an i_block (60 B) or a full block, and a too-small buffer was UB
// before, not a value path.

fn node_header(data: &[u8]) -> &Ext4ExtentHeader {
    assert!(
        data.len() >= core::mem::size_of::<Ext4ExtentHeader>(),
        "extent node buffer smaller than its header"
    );
    // SAFETY: repr(C) on-disk layout; size asserted above.
    unsafe { &*(data.as_ptr() as *const Ext4ExtentHeader) }
}

fn node_header_mut(data: &mut [u8]) -> &mut Ext4ExtentHeader {
    assert!(
        data.len() >= core::mem::size_of::<Ext4ExtentHeader>(),
        "extent node buffer smaller than its header"
    );
    // SAFETY: repr(C) on-disk layout; size asserted above.
    unsafe { &mut *(data.as_mut_ptr() as *mut Ext4ExtentHeader) }
}

fn node_extents(data: &[u8], count: usize) -> &[Ext4Extent] {
    let need =
        core::mem::size_of::<Ext4ExtentHeader>() + count * core::mem::size_of::<Ext4Extent>();
    if count == 0 || data.len() < need {
        return &[];
    }
    // SAFETY: repr(C) layout; bounds checked above.
    unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4Extent,
            count,
        )
    }
}

fn node_extents_mut(data: &mut [u8], count: usize) -> &mut [Ext4Extent] {
    let need =
        core::mem::size_of::<Ext4ExtentHeader>() + count * core::mem::size_of::<Ext4Extent>();
    if count == 0 || data.len() < need {
        return &mut [];
    }
    // SAFETY: repr(C) layout; bounds checked above.
    unsafe {
        core::slice::from_raw_parts_mut(
            data.as_mut_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *mut Ext4Extent,
            count,
        )
    }
}

fn node_indices(data: &[u8], count: usize) -> &[Ext4ExtentIdx] {
    let need =
        core::mem::size_of::<Ext4ExtentHeader>() + count * core::mem::size_of::<Ext4ExtentIdx>();
    if count == 0 || data.len() < need {
        return &[];
    }
    // SAFETY: repr(C) layout; bounds checked above.
    unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *const Ext4ExtentIdx,
            count,
        )
    }
}

fn node_indices_mut(data: &mut [u8], count: usize) -> &mut [Ext4ExtentIdx] {
    let need =
        core::mem::size_of::<Ext4ExtentHeader>() + count * core::mem::size_of::<Ext4ExtentIdx>();
    if count == 0 || data.len() < need {
        return &mut [];
    }
    // SAFETY: repr(C) layout; bounds checked above.
    unsafe {
        core::slice::from_raw_parts_mut(
            data.as_mut_ptr().add(core::mem::size_of::<Ext4ExtentHeader>()) as *mut Ext4ExtentIdx,
            count,
        )
    }
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

/// Read one external node into a fresh buffer. `blk` is a valid
/// filesystem block number on fs.device.
fn read_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    blk: u64,
) -> Result<alloc::vec::Vec<u8>, i32> {
    // SAFETY: bh pinned by bread; b_data is a block-sized slice.
    let bh = bio::bread(fs.device, blk).ok_or(errno::Errno::IOError.as_neg_i32())?;
    let data = unsafe { (*bh).b_data.to_vec() };
    bio::brelse(bh);
    Ok(data)
}

/// Write one external node back to disk (dirty + sync). When the fs has
/// metadata_csum, the extent-tree tail checksum is (re)computed first:
/// Linux's __ext4_ext_check rejects any non-root extent block whose
/// ext4_extent_tail.et_checksum does not verify ("extent tree corrupted",
/// -EFSBADCRC), which killed the reference kernel's boot on a
/// Rux-populated userdata (why-slow lane, wsref1: SaInit0 EIO storm).
/// Coverage block[0..len-4] equals Linux's EXT4_EXTENT_TAIL_OFFSET(eh)
/// because external_node_cap derives eh_max the same way ((block_size-12)
/// /12 entries => tail lands at 12+eh_max*12 == block_size-4). The seed
/// is the per-inode chain (sb csum seed -> ino -> i_generation), supplied
/// by the caller via `csum_seed`. The READ side stays permissive (no
/// verify): images written by pre-fix kernels carry zero tails and must
/// keep working on Rux.
/// `blk` is a valid filesystem block; data.len() == fs.block_size.
fn write_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    blk: u64,
    data: &[u8],
    csum_seed: u32,
) -> Result<(), i32> {
    // Family b (ext4-corrupt lane): register the node buffer with the
    // current jbd2 handle so it persists at this operation's commit
    // TOGETHER with the root (inode table), the allocation bitmaps and
    // the group descriptors. The old deferred-only path left freshly
    // written tree nodes dirty-in-cache until an out-of-band drain —
    // while the bitmap registration made their BITS durable at every
    // op's commit — so a hard kill left roots pointing at zeroed
    // blocks (g1 cook: 18 inodes with bad-magic-0 nodes; e2fsck
    // "invalid extent node"). Linux journals extent tree blocks the
    // same way (they are metadata).
    if fs.metadata_csum && data.len() >= 4 {
        let mut sealed = alloc::vec::Vec::from(data);
        crate::fs::ext4::csum::extent_block_csum_set(csum_seed, &mut sealed);
        let bh = bio::bread(fs.device, blk).ok_or(errno::Errno::IOError.as_neg_i32())?;
        // SAFETY: bh pinned by bread; whole-block copy into b_data.
        unsafe {
            (*bh).b_data.copy_from_slice(&sealed);
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            if let Some(handle) = crate::fs::ext4::namei::get_current_handle() {
                let _ = crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh);
            }
        }
        let sync_res = bio::sync_dirty_buffer(bh);
        bio::brelse(bh);
        return sync_res;
    }
    let bh = bio::bread(fs.device, blk).ok_or(errno::Errno::IOError.as_neg_i32())?;
    // SAFETY: bh pinned by bread; whole-block copy + dirty mark.
    unsafe {
        (*bh).b_data.copy_from_slice(data);
        (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
        // Family b: journal registration (see the sealed branch above).
        if let Some(handle) = crate::fs::ext4::namei::get_current_handle() {
            let _ = crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh);
        }
    }
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
    csum_seed: u32,
) -> Result<Option<SplitIdx>, i32> {
    if depth == 0 {
        let n = node_header(data).eh_entries as usize;
        if n > cap {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // Merge with the last entry (same state, logical+physical
        // contiguity, u15 headroom).
        if n > 0 {
            // n <= cap entries fit in the buffer per the caller's
            // contract (root 60B/4, external block_size/cap).
            let last = &mut node_extents_mut(data, cap)[n - 1];
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
            // slot n < cap is within the buffer contract.
            let slot = &mut node_extents_mut(data, cap)[n];
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
        write_node(fs, blk, &nd, csum_seed)?;
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
        match append_node(fs, &mut child, external_node_cap(fs), depth - 1, e, meta_sectors, csum_seed)? {
            None => {
                // SAFETY: last_child valid.
                write_node(fs, last_child, &child, csum_seed)?;
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
                    write_node(fs, blk, &nd, csum_seed)?;
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
    csum_seed: u32,
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

    match append_node(fs, &mut root, ROOT_NODE_CAP, depth, e, meta_sectors, csum_seed)? {
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
            write_node(fs, down, &down_data, csum_seed)?;
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
            // rux:exthunt (retired dfx=exthunt): the tree just grew one level — the event the
            // ftest04/zzprobe4 all-zeros family correlates with.
            if crate::trace::EVENT_EXTHUNT.armed() {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static GROWS: AtomicUsize = AtomicUsize::new(0);
                if GROWS.fetch_add(1, Ordering::Relaxed) < 20 {
                    crate::pr_info!(
                        "EXT-HUNT-GROW new_depth={} down={} split_child={} split_ei_block={}",
                        new_depth,
                        down,
                        split.child,
                        split.ei_block
                    );
                }
            }
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
    let root = i_block_bytes(i_block);
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
fn gather_node(
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

/// R108 (fi-fix-verify, crash-ordering): collect the tree's metadata
/// block numbers WITHOUT freeing them. The truncate/punch paths must not
/// clear an old tree's bitmap bits while the on-disk root still points
/// at that tree — the free is deferred until the rebuilt root is durable
/// (see ext4_ext_rebuild_deferred).
pub fn ext4_ext_collect_index_blocks(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &[u32; 15],
    deferred: &mut alloc::vec::Vec<u64>,
) -> Result<(), i32> {
    let root = i_block_bytes(i_block);
    let h = node_header(root);
    if h.eh_magic != EXT4_EXT_MAGIC || h.eh_depth == 0 {
        return Ok(());
    }
    // SAFETY: root node access bounded by ROOT_NODE_CAP.
    collect_index_node(fs, root, ROOT_NODE_CAP, h.eh_depth, deferred)
}

/// Collecting twin of free_index_node (same walk, no allocator free).
/// SAFETY: data holds cap 12-byte entries after the header.
fn collect_index_node(
    fs: &crate::fs::ext4::Ext4FileSystem,
    data: &[u8],
    cap: usize,
    depth: u16,
    deferred: &mut alloc::vec::Vec<u64>,
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
            collect_index_node(fs, &child, external_node_cap(fs), ch.eh_depth, deferred)?;
        }
        deferred.push(child_blk);
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
    let root = i_block_bytes(i_block);
    let h = node_header(root);
    if h.eh_magic != EXT4_EXT_MAGIC || h.eh_depth == 0 {
        return Ok(());
    }
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    free_index_node(fs, root, ROOT_NODE_CAP, h.eh_depth, &allocator, freed_sectors)
}

fn free_index_node(
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
    csum_seed: u32,
) -> Result<i64, i32> {
    let mut freed: u64 = 0;
    ext4_ext_free_index_blocks(fs, i_block, &mut freed)?;
    init_node_header(
        // SAFETY: i_block is 60 bytes; header write is in-bounds.
        i_block_bytes_mut(i_block),
        0,
        ROOT_NODE_CAP as u16,
    );
    let mut added: u64 = 0;
    for e in exts {
        ext4_ext_append(fs, i_block, e, &mut added, csum_seed)?;
    }
    Ok(added as i64 - freed as i64)
}

/// R108 (fi-fix-verify, crash-ordering): deferred-meta twin of
/// ext4_ext_rebuild for the truncate/punch paths. The OLD tree's
/// metadata blocks are COLLECTED into `deferred_meta` instead of freed:
/// clearing their bitmap bits while the on-disk root still points at the
/// old tree is the same re-free poison window as the data-block free
/// (crash → stale root walks reallocated tree blocks → garbage map →
/// re-free of live blocks). The caller frees the collected blocks only
/// after the rebuilt root is durably published (write_inode +
/// sync_inode_table_block). The `freed` accounting keeps counting them —
/// if the caller's drain is skipped (sync failure), i_blocks merely
/// undercounts a leaked tree (advisory field, benign).
pub fn ext4_ext_rebuild_deferred(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    exts: &[RawExtent],
    deferred_meta: &mut alloc::vec::Vec<u64>,
    csum_seed: u32,
) -> Result<i64, i32> {
    let mut freed: u64 = 0;
    ext4_ext_collect_index_blocks(fs, i_block, deferred_meta)?;
    // Count the deferred blocks as freed for the i_blocks delta (the
    // caller drains them right after the root goes durable).
    freed = deferred_meta.len() as u64 * (fs.block_size / 512) as u64;
    init_node_header(
        // SAFETY: i_block is 60 bytes; header write is in-bounds.
        i_block_bytes_mut(i_block),
        0,
        ROOT_NODE_CAP as u16,
    );
    let mut added: u64 = 0;
    for e in exts {
        ext4_ext_append(fs, i_block, e, &mut added, csum_seed)?;
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
    let root = i_block_bytes(i_block);
    let h = node_header(root);
    if h.eh_magic != EXT4_EXT_MAGIC || h.eh_depth == 0 {
        return Ok(0);
    }
    let n = h.eh_entries as usize;
    if n > ROOT_NODE_CAP {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    if n == 0 {
        // Empty index root (corrupt/stale tree): the fallback below would
        // index idxs[0] of an empty slice and PANIC — report no leaf
        // instead (the caller treats 0 as a hole).
        return Ok(0);
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

/// Push `e` onto `out`, merging with the previous entry when the two are
/// logically+physically contiguous with the SAME unwritten state (the
/// append_node merge rule; u15 headroom per EXT4-MERGE).
#[inline]
fn push_merge(out: &mut alloc::vec::Vec<RawExtent>, e: RawExtent) {
    if let Some(l) = out.last_mut() {
        if l.unwritten == e.unwritten
            && l.end() == e.ee_block as u64
            && l.phys + l.len as u64 == e.phys
            && l.len as u64 + e.len as u64 <= 0x7FFE
        {
            l.len += e.len;
            return;
        }
    }
    out.push(e);
}

/// Splice a WRITTEN conversion of the logical range `[start, start+len)`
/// through a flat sorted extent list, merging neighbors. Returns None when
/// no unwritten extent overlapped the range (a no-op call). Takes an
/// iterator so no-op calls (the per-write common case on already-written
/// ranges) allocate nothing.
fn splice_converted<I: Iterator<Item = RawExtent>>(
    entries: I,
    start: u64,
    len: u64,
) -> Option<alloc::vec::Vec<RawExtent>> {
    let end = start.saturating_add(len);
    let mut out: alloc::vec::Vec<RawExtent> = alloc::vec::Vec::new();
    let mut changed = false;
    for e in entries {
        let e_end = e.end();
        if !e.unwritten || e_end <= start || e.ee_block as u64 >= end {
            push_merge(&mut out, e);
            continue;
        }
        changed = true;
        // Overlap [ov_s, ov_e) turns WRITTEN; unwritten prefix/suffix
        // survive around it.
        let ov_s = start.max(e.ee_block as u64);
        let ov_e = end.min(e_end);
        let e_start = e.ee_block as u64;
        if ov_s > e_start {
            push_merge(
                &mut out,
                RawExtent {
                    ee_block: e.ee_block,
                    phys: e.phys,
                    len: (ov_s - e_start) as u16,
                    unwritten: true,
                },
            );
        }
        push_merge(
            &mut out,
            RawExtent {
                ee_block: ov_s as u32,
                phys: e.phys + (ov_s - e_start),
                len: (ov_e - ov_s) as u16,
                unwritten: false,
            },
        );
        if ov_e < e_end {
            push_merge(
                &mut out,
                RawExtent {
                    ee_block: ov_e as u32,
                    phys: e.phys + (ov_e - e_start),
                    len: (e_end - ov_e) as u16,
                    unwritten: true,
                },
            );
        }
    }
    if changed {
        Some(out)
    } else {
        None
    }
}

/// Bulk unwritten->written conversion for a logical block range (the
/// reference kernel's write shape: Linux submits the write's bios over
/// unwritten extents directly and converts the extent STATE in bulk —
/// ext4_ext_convert_unwritten — one tree edit per write, never per block).
///
/// The range's covered unwritten extents are spliced into (unwritten
/// prefix, WRITTEN overlap, unwritten suffix) with neighbor merging in the
/// covering node; the common single-extent case is a node-local edit
/// (inline root: no tree I/O at all — the root lives in the inode; external
/// leaf: one checksummed write_node). Ranges crossing node boundaries or
/// overflowing a node's capacity fall back to gather/splice/rebuild. A
/// no-op call (nothing unwritten overlapping) costs only the covering
/// node's scan — no I/O, no edit.
///
/// Crash ordering note: the conversion happens BEFORE the data blocks are
/// written, exactly like the per-block materialize path it replaces (a
/// crash between the two exposes the preallocated block's stale bytes
/// instead of zeros — the documented NJ write-back stance; e2fsck's
/// unwritten-vs-bitmap pass covers it the same way it covers
/// ext4_ext_materialize_block).
///
/// `meta_delta` accumulates the SIGNED 512-byte-sector delta of tree
/// metadata (a rebuild may free more than it adds) — same contract as
/// ext4_ext_materialize_block.
pub fn ext4_ext_convert_unwritten(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    start: u64,
    len: u64,
    meta_delta: &mut i64,
    csum_seed: u32,
) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    let end = start.saturating_add(len);
    let root = i_block_bytes(i_block);
    let rh = node_header(root);
    if rh.eh_magic != EXT4_EXT_MAGIC {
        return Ok(());
    }
    if rh.eh_depth == 0 {
        let n = rh.eh_entries as usize;
        if n > ROOT_NODE_CAP {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: n bounded by ROOT_NODE_CAP (root 60-byte contract).
        let entries: &[Ext4Extent] = unsafe { node_extents(root, n) };
        let Some(spliced) =
            splice_converted(entries.iter().map(|e| RawExtent::from_disk(e)), start, len)
        else {
            return Ok(());
        };
        if spliced.len() > ROOT_NODE_CAP {
            // Root cannot hold the splice (fragmented legacy tree): full
            // rebuild from the merged flat list.
            let delta = ext4_ext_rebuild(fs, i_block, &spliced, csum_seed)?;
            *meta_delta += delta;
            return Ok(());
        }
        // Node-local root splice.
        let mut node = root.to_vec();
        let h = node_header_mut(&mut node);
        h.eh_entries = spliced.len() as u16;
        // SAFETY: spliced.len() <= ROOT_NODE_CAP fits the 60-byte root.
        unsafe {
            node_extents_mut(&mut node, ROOT_NODE_CAP)[..spliced.len()]
                .copy_from_slice(&spliced.iter().map(|e| e.to_disk()).collect::<alloc::vec::Vec<_>>());
            core::ptr::copy_nonoverlapping(node.as_ptr(), i_block.as_mut_ptr() as *mut u8, 60);
        }
        return Ok(());
    }
    // Depth >= 1: LEAF-LOCAL splices. The write's range usually covers a
    // handful of leaves; each covering leaf is read once, spliced under
    // its own capacity and written back with one checksum. The whole-tree
    // gather/splice/rebuild runs only when a leaf overflows (fragmented
    // legacy tree) — a per-write GATHER here was O(total-entries) on the
    // random-overwrite fsync row (mid-run ~2k entries → 80 us per 4 KiB
    // flushed page; the no-op path below is one cached leaf read + scan).
    let mut lb = start;
    let mut overflow = false;
    while lb < end {
        // SAFETY: i_block is the inode's extent root (60 bytes).
        let leaf_blk = find_covering_leaf_blk(fs, i_block, lb)?;
        if leaf_blk == 0 {
            // Left of the first leaf's key — no coverage, no unwritten.
            break;
        }
        // SAFETY: leaf_blk from the tree index walk.
        let mut node = unsafe { read_node(fs, leaf_blk)? };
        let cap = external_node_cap(fs);
        let lh = node_header(&node);
        let n = lh.eh_entries as usize;
        if lh.eh_magic != EXT4_EXT_MAGIC || n > cap {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        // SAFETY: n bounded by cap.
        let entries: &[Ext4Extent] = unsafe { node_extents(&node, n) };
        let leaf_end = entries
            .last()
            .map(|e| RawExtent::from_disk(e).end())
            .unwrap_or(lb);
        if leaf_end <= lb {
            break; // degenerate/empty leaf
        }
        let range_end = end.min(leaf_end);
        match splice_converted(entries.iter().map(|e| RawExtent::from_disk(e)), lb, range_end - lb) {
            None => {
                // No unwritten overlap in this leaf — advance.
                lb = leaf_end;
            }
            Some(spliced) => {
                if spliced.len() > cap {
                    overflow = true;
                    break;
                }
                let h = node_header_mut(&mut node);
                h.eh_entries = spliced.len() as u16;
                // SAFETY: spliced.len() <= cap fits the node buffer.
                unsafe {
                    let disk: alloc::vec::Vec<_> =
                        spliced.iter().map(|e| e.to_disk()).collect();
                    node_extents_mut(&mut node, cap)[..spliced.len()].copy_from_slice(&disk);
                }
                // SAFETY: leaf_blk from the tree walk.
                unsafe { write_node(fs, leaf_blk, &node, csum_seed)? };
                lb = leaf_end;
            }
        }
    }
    if overflow {
        // A leaf cannot hold its splice: whole-tree rebuild from the
        // merged flat list (this also collapses a fragmented legacy
        // tree back toward the inline root).
        let all = ext4_ext_gather(fs, i_block)?;
        let Some(spliced) = splice_converted(all.into_iter(), start, len) else {
            return Ok(());
        };
        let delta = ext4_ext_rebuild(fs, i_block, &spliced, csum_seed)?;
        *meta_delta += delta;
    }
    Ok(())
}

/// Write-time conversion for unwritten extents (any tree depth): mark the
/// single logical block WRITTEN, splitting its extent into up to three
/// entries. The covering node (inline root OR external leaf) is spliced
/// in place when it has room; a FULL node falls back to a whole-tree
/// gather/splice/rebuild (the old inline-only path returned EIO for a
/// 4-entry root — writes into fragmented preallocations failed).
/// Returns the physical block. `meta_delta` accumulates the SIGNED
/// 512-byte-sector delta of tree metadata (a rebuild can FREE more than
/// it adds); the caller applies it to i_blocks with saturation.
pub fn ext4_ext_materialize_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    logical_block: u64,
    meta_delta: &mut i64,
    csum_seed: u32,
) -> Result<Option<u64>, i32> {
    // Resolve the covering node: the inline root (depth 0) or the
    // external leaf reached through the index walk.
    let root = i_block_bytes(i_block);
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
            if !e.is_unwritten() {
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
                unsafe { write_node(fs, blk, &node, csum_seed)? };
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
        let delta = ext4_ext_rebuild(fs, i_block, &all, csum_seed)?;
        // SIGNED accumulation: a shrinking rebuild has delta < 0. The
        // old u64 form folded a negative delta into ~u64::MAX and the
        // caller's `i_blocks += hm` then overflow-panicked the kernel
        // (dio_read on a fragmented ext4: "attempt to add with
        // overflow" at ext4/file.rs).
        *meta_delta += delta;
        Ok(Some(raw.phys + prefix as u64))
    }
}

/// Insert a WRITTEN extent (any tree depth): inline root keeps the sorted
/// fast path; deep trees insert into the covering leaf (rebuild fallback).
/// `meta_delta` accumulates the SIGNED metadata-sector delta (see
/// ext4_ext_materialize_block).
pub fn ext4_ext_insert_written(
    fs: &crate::fs::ext4::Ext4FileSystem,
    i_block: &mut [u32; 15],
    ee_block: u64,
    phys: u64,
    len: u16,
    meta_delta: &mut i64,
    csum_seed: u32,
) -> Result<(), i32> {
    {
        let root = i_block_bytes(i_block);
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
            let delta = ext4_ext_rebuild(fs, i_block, &all, csum_seed)?;
            // SIGNED accumulation — see the rebuild-fallback note in
            // ext4_ext_materialize_block above.
            *meta_delta += delta;
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
    // Merge with the last entry when contiguous. Guard like append_node
    // (review EXT4-MERGE): (a) the unwritten states must MATCH — merging a
    // written insert into a trailing UNWRITTEN extent cleared bit 15 of
    // ee_len, marking the whole never-materialized preallocation WRITTEN
    // (reads of it then served disk garbage instead of zeros; Linux's
    // ext4_can_extents_be_merged rejects mixed states); (b) the u15
    // headroom — `& 0x7FFF` on an overflowing sum silently truncated the
    // mapping instead of refusing the merge. Guard failed = separate entry
    // via the sorted insert below.
    if let Some(last) = entries.last() {
        let last_raw = RawExtent::from_disk(last);
        if !last_raw.unwritten
            && last_raw.len as u64 + len as u64 <= 0x7FFE
            && last_raw.end() == ee_block
            && last_raw.phys + last_raw.len as u64 == phys
        {
            let mut ne = entries.clone();
            let l = ne.last_mut().unwrap();
            l.ee_len = last_raw.len + len;
            let h = node_header_mut(&mut leaf);
            h.eh_entries = ne.len() as u16;
            // SAFETY: ne.len() <= cap.
            unsafe { node_extents_mut(&mut leaf, cap)[..ne.len()].copy_from_slice(&ne) };
            // SAFETY: leaf_blk valid.
            unsafe { write_node(fs, leaf_blk, &leaf, csum_seed)? };
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
        unsafe { write_node(fs, leaf_blk, &leaf, csum_seed)? };
        Ok(())
    } else {
        // Leaf full: whole-tree rebuild with the entry inserted sorted.
        let mut all = ext4_ext_gather(fs, i_block)?;
        let pos = all
            .iter()
            .position(|e| e.ee_block as u64 > ee_block)
            .unwrap_or(all.len());
        all.insert(pos, RawExtent { ee_block: ee_block as u32, phys, len, unwritten: false });
        let delta = ext4_ext_rebuild(fs, i_block, &all, csum_seed)?;
        // SIGNED accumulation — see the rebuild-fallback note in
        // ext4_ext_materialize_block above.
        *meta_delta += delta;
        Ok(())
    }
}
