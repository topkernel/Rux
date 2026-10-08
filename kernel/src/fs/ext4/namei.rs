//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 inode operations (mkdir, create, unlink, rmdir)
//!
//! ext4 directory entry operations

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use core::mem::size_of;

use crate::errno;
use crate::fs::bio::{self, BufferHead, BufferState};
use crate::fs::ext4::superblock::Ext4GroupDesc;
use crate::fs::ext4::inode::{Ext4Inode, Ext4InodeOnDisk};
use crate::fs::ext4::dir::file_type;
use crate::fs::ext4::allocator::BlockAllocator;

use super::Ext4FileSystem;

// ============================================================================
// Current transaction handle (single-core, no concurrency)
// ============================================================================

/// Per-task journal handle (R8-M3, NEW2 mechanism 3): the old GLOBAL slot
/// stored a pointer to a stack-local handle — on SMP one CPU clobbered
/// another's pointer, and jbd2_journal_dirty_metadata then raced (or
/// dereferenced a dead stack frame after the owner returned). The handle
/// now lives in Task, exactly like Linux task_struct::journal_info.
///
/// SAFETY discipline: set/clear bracket the owning syscall on the SAME
/// task; readers run in the same task's syscall context.
pub(crate) unsafe fn set_current_handle(handle: *mut crate::fs::jbd2::Handle) {
    if let Some(task) = crate::sched::current() {
        (*task).journal_handle.set(handle);
    }
}

/// Clear the current journal handle
pub(crate) unsafe fn clear_current_handle() {
    if let Some(task) = crate::sched::current() {
        (*task).journal_handle.set(core::ptr::null_mut());
    }
}

/// Get the current journal handle, if any
pub(crate) unsafe fn get_current_handle() -> Option<*mut crate::fs::jbd2::Handle> {
    let task = crate::sched::current()?;
    let ptr = (*task).journal_handle.get();
    if ptr.is_null() { None } else { Some(ptr) }
}

// ============================================================================
// Constants
// ============================================================================

/// Maximum link count for directories
pub const EXT4_LINK_MAX: u16 = 65000;

/// Inode mode bits
pub const S_IFMT: u16 = 0o170000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFREG: u16 = 0o100000;
pub const S_IFLNK: u16 = 0o120000;

/// Permission bits
pub const S_IRWXU: u16 = 0o0700;
pub const S_IRWXG: u16 = 0o0070;
pub const S_IRWXO: u16 = 0o0007;

// ============================================================================
// Helper functions for block I/O
// ============================================================================

/// Read a block into a Vec<u8>
// SAFETY: device pointer is valid; bio::bread returns valid bh or error
unsafe fn read_block_to_vec(device: *const crate::drivers::blkdev::GenDisk, blocknr: u64, block_size: usize) -> Result<Vec<u8>, i32> {
    let bh = bio::bread(device, blocknr).ok_or(errno::Errno::IOError.as_neg_i32())?;
    let data = (*bh).b_data.clone();
    bio::brelse(bh);
    Ok(data)
}

/// Write a Vec<u8> to a block
// SAFETY: device pointer is valid; bio functions handle buffer lifecycle
unsafe fn write_block_from_vec(device: *const crate::drivers::blkdev::GenDisk, blocknr: u64, data: &[u8]) -> Result<(), i32> {
    let bh = bio::bread(device, blocknr).ok_or(errno::Errno::IOError.as_neg_i32())?;

    // Get mutable reference to buffer head
    let bh_ref = &mut *bh;

    // Copy data to buffer
    let buf_len = bh_ref.b_data.len().min(data.len());
    bh_ref.b_data[0..buf_len].copy_from_slice(&data[0..buf_len]);

    // Mark dirty and sync
    bh_ref.set_state_bit(BufferState::BH_Dirty);

    // If a journal handle is active, register this buffer
    if let Some(handle) = get_current_handle() {
        let _ = crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh);
    }

    let sync_res = bio::sync_dirty_buffer(bh);
    bio::brelse(bh);
    sync_res?;

    Ok(())
}

// ============================================================================
// Inode allocation
// ============================================================================

/// Find a suitable block group for new inode
///
/// Uses Orlov's allocator for directories to spread them across groups.
pub fn find_group_orlov(fs: &Ext4FileSystem, _parent: u32, is_dir: bool) -> Result<u32, i32> {
    let group_count = fs.group_count;
    let _inodes_per_group = fs.inodes_per_group;

    // Simple implementation: find first group with free inodes
    for group in 0..group_count {
        let free_inodes = get_group_free_inodes(fs, group)?;
        if free_inodes > 0 {
            return Ok(group);
        }
    }

    Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
}

/// Get free inode count for a group
pub fn get_group_free_inodes(fs: &Ext4FileSystem, group: u32) -> Result<u32, i32> {
    let group_descs = fs.group_descs.lock();
    if group as usize >= group_descs.len() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    Ok(group_descs[group as usize].bg_free_inodes_count_lo as u32)
}

/// Allocate a new inode
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir` - Parent directory inode number
/// * `mode` - Mode for new inode
/// * `name` - Name for new entry
///
/// # Returns
/// * Ok((inode_number, inode)) on success
/// * Err(i32) on failure
pub fn ext4_new_inode(
    fs: &Ext4FileSystem,
    dir: u32,
    mode: u16,
    _name: &[u8],
) -> Result<(u32, Ext4InodeOnDisk), i32> {
    // Find suitable group
    let is_dir = (mode & S_IFMT) == S_IFDIR;
    let group = find_group_orlov(fs, dir, is_dir)?;

    // Get group descriptor
    let inode_bitmap_block = {
        let group_descs = fs.group_descs.lock();
        group_descs[group as usize].bg_inode_bitmap_lo
    };
    let bitmap_data = unsafe {
        read_block_to_vec(fs.device, inode_bitmap_block as u64, fs.block_size as usize)?
    };

    // Find free inode in bitmap
    let inodes_per_group = fs.inodes_per_group as usize;
    let mut free_ino_in_group: usize = 0;
    let mut found = false;

    for byte_idx in 0..bitmap_data.len() {
        let byte = bitmap_data[byte_idx];
        if byte != 0xff {
            // Find first zero bit
            for bit in 0..8 {
                if (byte & (1 << bit)) == 0 {
                    free_ino_in_group = byte_idx * 8 + bit;
                    if free_ino_in_group < inodes_per_group {
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
    }

    if !found || free_ino_in_group >= inodes_per_group {
        return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
    }

    // Calculate global inode number
    let ino = group * fs.inodes_per_group + free_ino_in_group as u32 + 1;

    // Mark inode as used in bitmap
    mark_inode_used(fs, group, free_ino_in_group, &bitmap_data, inode_bitmap_block as u64)?;

    // Create new inode
    let mut inode = Ext4InodeOnDisk::default();
    inode.i_mode = mode;
    inode.i_links_count = 1;
    // Inherit uid/gid from current process credentials.
    // Linux inode_init_owner() assigns current_fsuid()/current_fsgid() —
    // the EFFECTIVE filesystem ids, not the real ones. seteuid(nobody)
    // changes fsuid only; using the real uid made every file created by a
    // seteuid'd LTP test root-owned, so the creator then failed its own
    // open(O_CREAT|O_RDWR) with EACCES against the other-permission bits
    // (chmod03/utimes01 TBROK, and root-owned stale files that broke
    // tst_rmdir cleanup for later tests).
    let (uid, gid) = if let Some(task) = crate::sched::current() {
        // SAFETY: task is a valid reference from sched::current(); cred() is a simple field accessor.
        let cred = unsafe { (*task).cred() };
        (cred.fsuid as u16, cred.fsgid as u16)
    } else {
        (0u16, 0u16)
    };
    inode.i_uid = uid;
    inode.i_gid = gid;
    inode.i_size = 0;
    inode.i_blocks = 0;
    // New regular files and directories are EXTENT-based (as in Linux
    // ext4_new_inode): EXT4_EXTENTS_FL plus an initialized EMPTY extent
    // header in i_block. Without the flag, fresh files took the indirect
    // path where every bulk allocation materializes block-by-block — a
    // 300MB fallocate (LTP tst_acquire_device's scratch device) wedged
    // the system; extent files get the unwritten-extent fast path (see
    // preallocate_unwritten_extents). Symlinks keep i_block for the
    // inline fast-target — they stay indirect.
    inode.i_flags = 0;
    if (mode & S_IFMT) == S_IFREG || (mode & S_IFMT) == S_IFDIR {
        inode.i_flags |= 0x80000; // EXT4_EXTENTS_FL
        use super::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};
        let hdr = Ext4ExtentHeader {
            eh_magic: EXT4_EXT_MAGIC,
            eh_entries: 0,
            eh_max: 4, // (60 - 12) / 12 root inline entries
            eh_depth: 0,
            eh_generation: 0,
        };
        // SAFETY: i_block is a [u32; 15] = 60 bytes; a 12-byte header at
        // its start is in-bounds.
        unsafe {
            *(inode.i_block.as_mut_ptr() as *mut Ext4ExtentHeader) = hdr;
        }
    }
    // Fresh inodes are born "now" (wall-clock epoch seconds —
    // drivers/rtc::wall_secs), as in Linux's ext4_new_inode.
    let now = crate::drivers::rtc::wall_secs() as u32;
    inode.i_atime = now;
    inode.i_mtime = now;
    inode.i_ctime = now;

    // Update group descriptor
    update_group_descriptor_inodes(fs, group, -1)?;

    // Update superblock
    update_superblock_free_inodes(fs, -1)?;

    Ok((ino, inode))
}

/// Mark inode as used in bitmap
fn mark_inode_used(
    fs: &Ext4FileSystem,
    _group: u32,
    ino_in_group: usize,
    bitmap_data: &[u8],
    bitmap_block: u64,
) -> Result<(), i32> {
    let byte_idx = ino_in_group / 8;
    let bit_idx = ino_in_group % 8;

    // Create new bitmap with bit set
    let mut new_bitmap = bitmap_data.to_vec();
    new_bitmap[byte_idx] |= 1 << bit_idx;

    // Write bitmap back
    // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
    unsafe {
        write_block_from_vec(fs.device, bitmap_block, &new_bitmap)?;
    }

    Ok(())
}

/// Update group descriptor free inode count
fn update_group_descriptor_inodes(fs: &Ext4FileSystem, group: u32, delta: i32) -> Result<(), i32> {
    {
        let mut group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        if delta < 0 {
            group_descs[group as usize].bg_free_inodes_count_lo =
                group_descs[group as usize].bg_free_inodes_count_lo.saturating_sub((-delta) as u16);
        } else {
            group_descs[group as usize].bg_free_inodes_count_lo =
                group_descs[group as usize].bg_free_inodes_count_lo.saturating_add(delta as u16);
        }
    }

    // Write group descriptor to disk
    write_group_descriptor(fs, group)?;

    Ok(())
}

/// Update superblock free inode count
fn update_superblock_free_inodes(fs: &Ext4FileSystem, delta: i32) -> Result<(), i32> {
    // Update in-memory sb_info
    let sb_info_ptr = fs.sb_info.as_ref().map(|x| x.as_ref() as *const super::superblock::Ext4SuperBlockInfo);
    if let Some(sb_info_ptr) = sb_info_ptr {
        // SAFETY: sb_info is valid once filesystem is initialized
        unsafe {
            let sb_info = &mut *(sb_info_ptr as *mut super::superblock::Ext4SuperBlockInfo);
            if delta < 0 {
                sb_info.s_free_inodes_count =
                    sb_info.s_free_inodes_count.saturating_sub((-delta) as u32);
            } else {
                sb_info.s_free_inodes_count =
                    sb_info.s_free_inodes_count.saturating_add(delta as u32);
            }
        }
    }

    // Write to on-disk superblock
    // s_free_inodes_count is at offset 16 within the superblock
    // SAFETY: fs.device is a valid GenDisk pointer for the mounted ext4 filesystem; bio::bread returns valid BufferHeads and block numbers are bounds-checked.
    unsafe {
        let sb_block = if fs.block_size == 1024 { 1u64 } else { 0u64 };
        let bh = bio::bread(fs.device, sb_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &mut (*bh).b_data;
        let sb_start = if fs.block_size == 1024 { 0usize } else { 1024usize };
        let ptr = data.as_mut_ptr().add(sb_start + 16) as *mut u32;

        let current = ptr.read_volatile();
        ptr.write_volatile((current as i32 + delta) as u32);

        (*bh).set_state_bit(BufferState::BH_Dirty);
        let sync_res = bio::sync_dirty_buffer(bh);
        bio::brelse(bh);
        sync_res?;
    }

    Ok(())
}

/// Write group descriptor to disk
fn write_group_descriptor(fs: &Ext4FileSystem, group: u32) -> Result<(), i32> {
    let gd = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }
        *group_descs[group as usize]
    };

    // Calculate descriptor table location
    let desc_per_block = fs.block_size / fs.desc_size as u32;
    let desc_block = fs.sb_info.as_ref()
        .map(|sb| sb.s_first_data_block + 1 + group / desc_per_block)
        .unwrap_or(1);
    let desc_offset = (group % desc_per_block) as usize;

    // Read descriptor block
    // SAFETY: device is valid; block numbers come from superblock/group descriptor geometry.
    let mut block_data = unsafe {
        read_block_to_vec(fs.device, desc_block as u64, fs.block_size as usize)?
    };

    // Write descriptor (only the low 32 bytes that Ext4GroupDesc covers;
    // the high 32 bytes in the 64-bit on-disk descriptor are preserved from
    // the block_data we just read).
    let gd_ptr: *const Ext4GroupDesc = &gd;
    // SAFETY: gd is a stack-local Ext4GroupDesc; reinterpreting as bytes is safe for #[repr(C)].
    let gd_bytes = unsafe {
        core::slice::from_raw_parts(
            gd_ptr as *const u8,
            core::mem::size_of::<Ext4GroupDesc>()
        )
    };
    let offset = desc_offset * fs.desc_size as usize;
    // Write at most desc_size bytes: descriptors sit at desc_size stride,
    // so writing the full 64-byte struct over a 32-byte descriptor table
    // clobbered the NEXT group's descriptor — and the last descriptor in
    // the block overflowed the slice outright (review EXT4-H4).
    let write_len = core::cmp::min(fs.desc_size as usize, gd_bytes.len());
    if offset + write_len > block_data.len() {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    block_data[offset..offset + write_len].copy_from_slice(&gd_bytes[..write_len]);

    // Write back
    // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
    unsafe {
        write_block_from_vec(fs.device, desc_block as u64, &block_data)?;
    }

    Ok(())
}

// ============================================================================
// Directory entry operations
// ============================================================================

/// Get block number from directory inode, supporting both extents and direct blocks
fn get_dir_block_nr(fs: &Ext4FileSystem, dir: &Ext4InodeOnDisk, block_idx: u64) -> Result<u64, i32> {
    // Check if using extents
    if (dir.i_flags & 0x80000) != 0 {
        // Use extent tree
        super::extent::ext4_ext_get_block(fs, &dir.i_block, block_idx)
    } else {
        // Use direct/indirect blocks
        if block_idx < 12 {
            Ok(dir.i_block[block_idx as usize] as u64)
        } else {
            // TODO: Handle indirect blocks
            Err(errno::Errno::InvalidArgument.as_neg_i32())
        }
    }
}

/// Add entry to directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Directory inode number
/// * `name` - Entry name
/// * `new_ino` - New entry's inode number
/// * `file_type` - File type (1=file, 2=dir, etc.)
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_add_entry(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    new_ino: u32,
    file_type: u8,
) -> Result<(), i32> {
    // Read directory inode
    let dir = super::inode::read_inode(fs, dir_ino)?;

    // htree-indexed directory (EXT4_INDEX_FL): inserting without updating
    // the dx index leaves real Linux unable to see the entry (and our own
    // linear scan would fight the spanning rec_len of block 0). Refuse the
    // write — reads still work via the linear scan fallback (review 5.5).
    if dir.i_flags & super::features::EXT4_INDEX_FL != 0 {
        return Err(-(crate::syscall::errno::EOPNOTSUPP as i32));
    }

    // Read directory data blocks
    let block_size = fs.block_size as usize;
    let dir_size = dir.i_size as usize;

    // Calculate number of blocks
    let num_blocks = if block_size > 0 {
        (dir_size + block_size - 1) / block_size
    } else {
        0
    };

    // Iterate through directory blocks looking for space
    for block_idx in 0..num_blocks as u64 {
        let block_nr = match get_dir_block_nr(fs, &dir, block_idx) {
            Ok(nr) => nr,
            Err(_) => {
                continue;
            }
        };

        if block_nr == 0 {
            continue;
        }

        let block_data = unsafe {
            let bh = bio::bread(fs.device, block_nr)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let data = (*bh).b_data.clone();
            bio::brelse(bh);
            data
        };

        // Try to find space in this block
        if let Some(offset) = find_entry_space(&block_data, name.len(), block_size) {
            // Found space, create entry
            let mut new_block = block_data.clone();
            add_entry_to_block(&mut new_block, offset, name, new_ino, file_type, block_size);

            // Write block back — ORDERED: everything this entry publishes
            // (the new inode's table block, bitmaps, data blocks) is
            // persisted BEFORE the entry block itself.
            // SAFETY: same contract as write_block_from_vec.
            unsafe {
                write_entry_block_ordered(fs, block_nr, &new_block)?;
            }

            return Ok(());
        }
    }

    // No space in existing blocks, allocate new block
    // No space in existing blocks, allocate new block
    let allocator = BlockAllocator::new(fs);
    let goal_group = (dir_ino / fs.inodes_per_group).min(fs.group_count - 1);
    let new_block_nr = allocator.alloc_block(goal_group)?;

    // Create new block with entry
    let mut new_block = alloc::vec![0u8; block_size];
    create_initial_entry(&mut new_block, name, new_ino, file_type, block_size);

    // Write new block — ORDERED (see the in-place path above): the freshly
    // allocated block's bitmap bit persists before the entry that names it.
    // SAFETY: same contract as write_block_from_vec.
    unsafe {
        write_entry_block_ordered(fs, new_block_nr, &new_block)?;
    }

    // Update directory inode to reference new block
    add_block_to_inode(fs, dir_ino, &dir, new_block_nr)?;

    Ok(())
}

/// Find space for new entry in directory block
///
/// Checks both free space within existing entries and deleted entries
/// (inode==0) that can be reused.
fn find_entry_space(block_data: &[u8], name_len: usize, block_size: usize) -> Option<usize> {
    let mut offset = 0;
    let required_len = ((8 + name_len + 3) & !3) as u16; // Align to 4 bytes

    while offset + 8 <= block_size {
        let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);

        if rec_len == 0 || rec_len < 8 {
            break;
        }

        // Check if this is a deleted/unused entry (inode == 0)
        let ino = u32::from_le_bytes([
            block_data[offset],
            block_data[offset + 1],
            block_data[offset + 2],
            block_data[offset + 3],
        ]);

        if ino == 0 {
            // Deleted entry — can reuse if large enough
            if rec_len >= required_len {
                return Some(offset);
            }
        } else {
            let name_len_entry = block_data[offset + 6] as usize;
            let used_len = ((8 + name_len_entry + 3) & !3) as u16;

            // Check if there's space in this entry
            if rec_len >= used_len + required_len {
                return Some(offset);
            }
        }

        offset += rec_len as usize;
    }

    None
}

/// Add entry to directory block at given offset
fn add_entry_to_block(
    block_data: &mut [u8],
    offset: usize,
    name: &[u8],
    ino: u32,
    file_type: u8,
    _block_size: usize,
) {
    let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);
    let existing_name_len = block_data[offset + 6] as usize;
    let used_len = ((8 + existing_name_len + 3) & !3) as u16;

    // Safety: ensure rec_len > used_len before subtracting
    if rec_len <= used_len {
        return;
    }

    // Calculate new entry length
    let new_rec_len = rec_len - used_len;

    // Update existing entry's record length
    let used_bytes = used_len.to_le_bytes();
    block_data[offset + 4] = used_bytes[0];
    block_data[offset + 5] = used_bytes[1];

    // Create new entry after existing
    let new_offset = offset + used_len as usize;

    // Write inode number
    let ino_bytes = ino.to_le_bytes();
    block_data[new_offset..new_offset + 4].copy_from_slice(&ino_bytes);

    // Write record length
    let new_rec_bytes = new_rec_len.to_le_bytes();
    block_data[new_offset + 4] = new_rec_bytes[0];
    block_data[new_offset + 5] = new_rec_bytes[1];

    // Write name length
    block_data[new_offset + 6] = name.len() as u8;

    // Write file type
    block_data[new_offset + 7] = file_type;

    // Write name
    block_data[new_offset + 8..new_offset + 8 + name.len()].copy_from_slice(name);
}

/// Crash-safe ordering for directory-entry writes (deferred form).
///
/// ROOT CAUSE CONTEXT (the "ghost empty file" after non-clean shutdown):
/// while a journal handle is active, bio::sync_dirty_buffer defers EVERY
/// write into the buffer cache. Op-level durability points (hash-order
/// sync_buffers, cache eviction, the commit fast path) then drain those
/// buffers in orders unrelated to the operation's logic, and a crash
/// mid-drain could persist a parent directory's entry block — the block
/// that PUBLISHES a freshly created inode — before that inode's table
/// block, bitmaps, or data. After the reboot the path resolves to the
/// inode slot's STALE contents (the ghost).
///
/// The original fix (ae288274) ran a synchronous barrier on EVERY entry
/// publication: flush the transaction's registered buffers, flush every
/// other dirty buffer of the device, then write and sync the entry block
/// last. Crash-safe, but 4-8 synchronous virtio round trips per
/// creat/mkdir/link/unlink — any workload that creates or removes a few
/// hundred files (LTP creat05/fork09 open 1021 in setup) blew the 30s
/// per-test wall clock just crawling through the I/O.
///
/// This deferred form keeps the SAME ordering guarantee without the
/// per-op I/O: the create-side operation brackets itself with a PRE
/// capture window (begin_pre_capture right after journal_start), so
/// every block IT dirties is recorded on the handle, and at the entry
/// write that list becomes the publication's `pre` set — an exact
/// snapshot of the operation's own metadata at O(own blocks) cost (the
/// intermediate version snapshotted the WHOLE device dirty set: correct
/// as a superset, but O(cache) per op with O(N)-growing pre sets across
/// un-drained create loops, which still pushed creat05 over the cap).
/// The entry block is written into the buffer cache as usual. Every
/// durability point (fsync/sync, the forced jbd2 commit, eviction of an
/// order-constrained buffer) first drains the queue in append order,
/// pre blocks before entry blocks before post blocks (see the design
/// comment in bio.rs). A crash between drains leaves either "no entry"
/// (operation never persisted) or "entry + fully initialized inode".
///
/// Without an active journal handle nothing is deferred (every metadata
/// write already syncs itself in logical order), so no publication is
/// created — the capture window, if one was armed, is simply discarded.
unsafe fn defer_entry_publication(fs: &Ext4FileSystem, entry_blocknr: u64) -> u64 {
    // SAFETY: get_current_handle is task-local and null-safe.
    let handle_ptr = match unsafe { get_current_handle() } {
        Some(p) => p,
        None => return 0,
    };
    // SAFETY: the handle lives on this task's stack for the enclosing
    // ext4_* operation; take_pre_capture mutates capture state owned by
    // this task (us) and returns exactly the blocks this operation
    // dirtied since the window was armed.
    let pre: alloc::vec::Vec<u64> = unsafe { (*handle_ptr).take_pre_capture() };
    bio::publication_defer(fs.device, &pre, entry_blocknr)
}

/// Write a directory-entry block with crash-safe ordering (deferred):
/// snapshot the publication dependencies, write the block into the
/// buffer cache, and record the publication. Persistence happens in
/// order at the next drain point.
///
/// SAFETY: same contract as write_block_from_vec (valid device, blocknr).
unsafe fn write_entry_block_ordered(
    fs: &Ext4FileSystem,
    blocknr: u64,
    data: &[u8],
) -> Result<(), i32> {
    // SAFETY: fs is the enclosing filesystem; blocknr is the block about
    // to be written.
    unsafe { defer_entry_publication(fs, blocknr) };
    // SAFETY: fs.device is a valid GenDisk pointer; write_block_from_vec
    // handles the buffer lifecycle. Under a journal handle its sync is
    // deferred (the entry block stays dirty until a drain point); without
    // one it syncs inline — last write of the operation either way.
    unsafe { write_block_from_vec(fs.device, blocknr, data) }
}

/// Create initial entry in empty block
fn create_initial_entry(
    block_data: &mut [u8],
    name: &[u8],
    ino: u32,
    file_type: u8,
    block_size: usize,
) {
    let _entry_len = ((8 + name.len() + 3) & !3) as u16;
    let rec_len = block_size as u16;

    // Write inode number
    let ino_bytes = ino.to_le_bytes();
    block_data[0..4].copy_from_slice(&ino_bytes);

    // Write record length (entire block)
    let rec_bytes = rec_len.to_le_bytes();
    block_data[4] = rec_bytes[0];
    block_data[5] = rec_bytes[1];

    // Write name length
    block_data[6] = name.len() as u8;

    // Write file type
    block_data[7] = file_type;

    // Write name
    block_data[8..8 + name.len()].copy_from_slice(name);
}

/// Add block to inode's block list
fn add_block_to_inode(
    fs: &Ext4FileSystem,
    ino: u32,
    inode: &Ext4InodeOnDisk,
    block_nr: u64,
) -> Result<(), i32> {
    let mut new_inode = *inode;
    let block_size = fs.block_size;

    // Check if using extents
    if (new_inode.i_flags & 0x80000) != 0 {
        return add_block_to_inode_extent(fs, ino, &mut new_inode, block_nr, block_size);
    }

    // Direct/indirect block mode: find free slot in i_block array
    for i in 0..12 {
        if new_inode.i_block[i] == 0 {
            new_inode.i_block[i] = block_nr as u32;
            new_inode.i_size += block_size;
            new_inode.i_blocks += (block_size / 512) as u32;

            super::inode::write_inode_disk(fs, ino, &new_inode)?;
            return Ok(());
        }
    }

    // Need to use indirect blocks - for now, return error
    Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32())
}

/// Add block to an extent-based inode by extending the extent tree.
///
/// Appends through the multi-level tree engine (ext4_ext_append): merge
/// with the last extent when physically contiguous, else a new entry —
/// in the inline root while it fits, in external nodes beyond. The old
/// implementation was capped at the 4 inline entries and returned a
/// bogus ENOSPC for the 5th fragment.
fn add_block_to_inode_extent(
    fs: &Ext4FileSystem,
    ino: u32,
    inode: &mut Ext4InodeOnDisk,
    block_nr: u64,
    block_size: u32,
) -> Result<(), i32> {
    // Calculate which logical block this new block will be
    let current_blocks = inode.i_size / block_size;
    let logical_block = current_blocks;

    let ext = crate::fs::ext4::extent::RawExtent {
        ee_block: logical_block as u32,
        phys: block_nr,
        len: 1,
        unwritten: false,
    };
    let mut meta_sectors: u64 = 0;
    crate::fs::ext4::extent::ext4_ext_append(fs, &mut inode.i_block, &ext, &mut meta_sectors)?;

    inode.i_size += block_size;
    inode.i_blocks += (block_size / 512) as u32 + meta_sectors as u32;

    super::inode::write_inode_disk(fs, ino, inode)?;
    Ok(())
}

// ============================================================================
// mkdir implementation
// ============================================================================

/// Create a new directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - New directory name
/// * `mode` - Mode for new directory
///
/// # Returns
/// * Ok(new_inode_number) on success
/// * Err(i32) on failure
pub fn ext4_mkdir(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    // Wrap in journal transaction if journal is available
    if fs.journal.is_some() {
        return ext4_mkdir_inner(fs, dir_ino, name, mode);
    }
    ext4_mkdir_no_journal(fs, dir_ino, name, mode)
}

fn ext4_mkdir_no_journal(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    // Check name length (NAME_MAX; Linux returns ENAMETOOLONG — review 5.5:
    // name_len u8 截断无检查, the u8 field would silently truncate)
    if name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // Check if parent link count would overflow
    let parent_inode = super::inode::read_inode(fs, dir_ino)?;
    if parent_inode.i_links_count >= EXT4_LINK_MAX {
        return Err(errno::Errno::TooManyLinks.as_neg_i32());
    }

    // Allocate new inode
    let dir_mode = mode & !S_IFMT | S_IFDIR;
    let (new_ino, mut new_inode) = ext4_new_inode(fs, dir_ino, dir_mode, name)?;

    // Allocate block for directory entries
    let allocator = BlockAllocator::new(fs);
    let goal_group = (dir_ino / fs.inodes_per_group).min(fs.group_count - 1);
    let block_nr = allocator.alloc_block(goal_group)?;

    // Initialize directory with "." and ".."
    let block_size = fs.block_size as usize;
    let mut block_data = alloc::vec![0u8; block_size];

    // Create "." entry at offset 0: rec_len=12, name=".\0\0"
    let dot_entry = create_dot_entry(new_ino, 12u16);
    block_data[0..8].copy_from_slice(&dot_entry);
    block_data[8..11].copy_from_slice(b".\0\0");

    // Create ".." entry at offset 12: rec_len=block_size-12, name="..\0"
    let dotdot_offset = 12usize;
    let dotdot_rec_len = (block_size - dotdot_offset) as u16;
    let dotdot_entry = create_dotdot_entry(dir_ino, dotdot_rec_len);
    block_data[dotdot_offset..dotdot_offset + 8].copy_from_slice(&dotdot_entry);
    block_data[dotdot_offset + 8..dotdot_offset + 11].copy_from_slice(b"..\0");

    // Write directory block
    // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
    unsafe {
        write_block_from_vec(fs.device, block_nr, &block_data)?;
    }

    // Update new inode's block mapping. New inodes are EXTENT-based (see
    // ext4_new_inode): the first block must become a WRITTEN extent entry
    // in i_block — the old `i_block[0] = block_nr` indirect-style store
    // overwrote the extent HEADER with the block number, corrupting the
    // tree before the directory was ever read.
    if (new_inode.i_flags & 0x80000) != 0 {
        use super::extent::{Ext4ExtentHeader, Ext4Extent, EXT4_EXT_MAGIC};
        // SAFETY: i_block is 60 bytes — a 12-byte header plus one
        // 12-byte extent entry fits with room to spare.
        unsafe {
            let hdr = new_inode.i_block.as_mut_ptr() as *mut Ext4ExtentHeader;
            (*hdr).eh_magic = EXT4_EXT_MAGIC;
            (*hdr).eh_entries = 1;
            (*hdr).eh_max = 4;
            (*hdr).eh_depth = 0;
            (*hdr).eh_generation = 0;
            let e = (new_inode.i_block.as_mut_ptr() as *mut u8)
                .add(core::mem::size_of::<Ext4ExtentHeader>())
                as *mut Ext4Extent;
            (*e).ee_block = 0;
            (*e).ee_len = 1; // written
            (*e).ee_start_hi = (block_nr >> 32) as u16;
            (*e).ee_start_lo = block_nr as u32;
        }
    } else {
        new_inode.i_block[0] = block_nr as u32;
    }
    new_inode.i_size = block_size as u32;
    new_inode.i_blocks = (block_size / 512) as u32;
    new_inode.i_links_count = 2; // "." and parent's entry

    // Write new inode
    super::inode::write_inode_disk(fs, new_ino, &new_inode)?;

    // Add entry to parent directory
    ext4_add_entry(fs, dir_ino, name, new_ino, file_type::EXT4_FT_DIR)?;

    // Update parent link count. RE-READ the parent: ext4_add_entry above may
    // have grown the directory (new block, i_size/i_block updates written
    // via write_inode_disk inside add_block_to_inode). Writing the snapshot
    // taken at function entry rolled those updates back and silently
    // unlinked the new entry from the inode (EXT4-H7 class; rename got this
    // fix, mkdir had not).
    let mut parent = super::inode::read_inode(fs, dir_ino)?;
    parent.i_links_count += 1;
    let sec = crate::drivers::rtc::wall_secs() as u32;
    parent.i_mtime = sec;
    parent.i_ctime = sec;
    super::inode::write_inode_disk(fs, dir_ino, &parent)?;

    // Best-effort full-cache flush for durability. NOT fatal to the
    // syscall: sync_buffers drains EVERY device's dirty buffers, and a
    // foreign device's writeback error (e.g. a loop whose backing-file
    // write failed) must not fail this mkdir — Linux reports such errors
    // via fsync/sync, never through the creating syscall (the r8 mkdtemp
    // ENXIO storm: mkdir inherited a dead loop's -6 from here).
    if let Err(e) = bio::sync_buffers() {
        crate::pr_warn!("ext4: post-mkdir buffer sync failed (errno {})", e);
    }

    Ok(new_ino)
}

fn ext4_mkdir_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    let mut handle = super::journal::ext4_journal_start(fs, 12)?;
    // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
    unsafe { set_current_handle(&mut handle); }
    // PRE-capture window: record this operation's own metadata dirtied
    // before the entry publication (see defer_entry_publication).
    handle.begin_pre_capture(crate::fs::bio::dev_key_of(fs.device));

    let result = ext4_mkdir_no_journal(fs, dir_ino, name, mode);

    // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
    unsafe { clear_current_handle(); }
    super::journal::ext4_journal_stop(&mut handle)?;
    result
}

/// Create "." entry
fn create_dot_entry(ino: u32, rec_len: u16) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0..4].copy_from_slice(&ino.to_le_bytes());
    entry[4..6].copy_from_slice(&rec_len.to_le_bytes());
    entry[6] = 1; // name_len = 1
    entry[7] = file_type::EXT4_FT_DIR;
    entry
}

/// Create ".." entry
fn create_dotdot_entry(ino: u32, rec_len: u16) -> [u8; 8] {
    let mut entry = [0u8; 8];
    entry[0..4].copy_from_slice(&ino.to_le_bytes());
    entry[4..6].copy_from_slice(&rec_len.to_le_bytes());
    entry[6] = 2; // name_len = 2
    entry[7] = file_type::EXT4_FT_DIR;
    entry
}

// ============================================================================
// create implementation
// ============================================================================

/// Create a new regular file
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - New file name
/// * `mode` - Mode for new file
///
/// # Returns
/// * Ok(new_inode_number) on success
/// * Err(i32) on failure
pub fn ext4_create(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 8)?;
        // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
        unsafe { set_current_handle(&mut handle); }
        // PRE-capture window (see defer_entry_publication).
        handle.begin_pre_capture(crate::fs::bio::dev_key_of(fs.device));
        let result = ext4_create_inner(fs, dir_ino, name, mode);
        // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_create_inner(fs, dir_ino, name, mode)
}

fn ext4_create_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    mode: u16,
) -> Result<u32, i32> {
    // Check name length (NAME_MAX; Linux returns ENAMETOOLONG — review 5.5:
    // name_len u8 截断无检查, the u8 field would silently truncate)
    if name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // Allocate new inode
    let file_mode = mode & !S_IFMT | S_IFREG;
    let (new_ino, new_inode) = ext4_new_inode(fs, dir_ino, file_mode, name)?;

    // Write new inode (empty file)
    super::inode::write_inode_disk(fs, new_ino, &new_inode)?;

    // Add entry to parent directory
    ext4_add_entry(fs, dir_ino, name, new_ino, file_type::EXT4_FT_REG_FILE)?;

    // Parent directory timestamps (review 5.5)
    touch_parent_dir(fs, dir_ino);

    Ok(new_ino)
}

// ============================================================================
// symlink implementation
// ============================================================================

/// Create a symbolic link
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Link name
/// * `target` - Symlink target path
///
/// # Returns
/// * Ok(inode number) on success
/// * Err(i32) on failure
pub fn ext4_symlink(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    target: &[u8],
) -> Result<u32, i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 8)?;
        // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
        unsafe { set_current_handle(&mut handle); }
        // PRE-capture window (see defer_entry_publication).
        handle.begin_pre_capture(crate::fs::bio::dev_key_of(fs.device));
        let result = ext4_symlink_inner(fs, dir_ino, name, target);
        // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_symlink_inner(fs, dir_ino, name, target)
}

fn ext4_symlink_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
    target: &[u8],
) -> Result<u32, i32> {
    // Check name length (NAME_MAX; Linux returns ENAMETOOLONG — review 5.5:
    // name_len u8 截断无检查, the u8 field would silently truncate)
    if name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // Allocate new inode with S_IFLNK mode
    let (new_ino, mut new_inode) = ext4_new_inode(fs, dir_ino, S_IFLNK | 0o777, name)?;

    if target.len() <= 60 {
        // Fast symlink: target stored inline in i_block array (60 bytes = 15 * 4)
        // SAFETY: src and dst point to valid inode block data within the BufferHead; sizes are derived from the ext4 inode layout.
        unsafe {
            let block_ptr = new_inode.i_block.as_mut_ptr() as *mut u8;
            core::ptr::copy_nonoverlapping(target.as_ptr(), block_ptr, target.len());
        }
        new_inode.i_size = target.len() as u32;
    } else {
        // Slow symlink: target stored in data block
        let mut allocator = BlockAllocator::new(fs);
        let goal_group = (dir_ino / fs.inodes_per_group).min(fs.group_count - 1);
        let blocknr = allocator.alloc_block(goal_group)? as u32;

        // Write target path to data block
        let mut block_data = alloc::vec![0u8; fs.block_size as usize];
        block_data[..target.len()].copy_from_slice(target);
        // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
        unsafe { write_block_from_vec(fs.device, blocknr as u64, &block_data)?; }

        new_inode.i_block[0] = blocknr;
        new_inode.i_size = target.len() as u32;
        // i_blocks counts 512-byte sectors
        new_inode.i_blocks += (fs.block_size as u32) / 512;
    }

    // Write inode to disk
    super::inode::write_inode_disk(fs, new_ino, &new_inode)?;

    // Add directory entry
    ext4_add_entry(fs, dir_ino, name, new_ino, file_type::EXT4_FT_SYMLINK)?;

    // Parent directory timestamps (review 5.5)
    touch_parent_dir(fs, dir_ino);

    Ok(new_ino)
}

// ============================================================================
// link implementation
// ============================================================================

/// Create a hard link
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `target_ino` - Target inode number to link to
/// * `name` - New link name
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_link(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    target_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 6)?;
        // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
        unsafe { set_current_handle(&mut handle); }
        // PRE-capture window (see defer_entry_publication).
        handle.begin_pre_capture(crate::fs::bio::dev_key_of(fs.device));
        let result = ext4_link_inner(fs, dir_ino, target_ino, name);
        // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_link_inner(fs, dir_ino, target_ino, name)
}

fn ext4_link_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    target_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    // Validate name (NAME_MAX; ENAMETOOLONG per Linux)
    if name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // Read target inode
    let mut target_inode = super::inode::read_inode(fs, target_ino)?;

    // Cannot hard link to directories
    if (target_inode.i_mode & S_IFMT) == S_IFDIR {
        return Err(errno::Errno::IsADirectory.as_neg_i32());
    }

    // Check link count limit
    if target_inode.i_links_count >= EXT4_LINK_MAX {
        return Err(errno::Errno::TooManyLinks.as_neg_i32());
    }

    // Check if name already exists in parent directory
    let dir_inode = super::inode::read_inode(fs, dir_ino)?;
    if find_dir_entry(fs, &dir_inode, name).is_ok() {
        return Err(errno::Errno::FileExists.as_neg_i32());
    }

    // Increment link count
    target_inode.i_links_count += 1;

    // Update timestamp
    let sec = crate::drivers::rtc::wall_secs() as u32;
    target_inode.i_ctime = sec;

    // Write updated inode back
    super::inode::write_inode_disk(fs, target_ino, &target_inode)?;

    // Add directory entry with the TARGET's real file type — a hard link
    // to a symlink/device used to be recorded as a regular file
    // (review 5.5 sibling: rename 保留原 file_type).
    ext4_add_entry(fs, dir_ino, name, target_ino, file_type_from_mode(target_inode.i_mode))?;

    // Parent directory timestamps (review 5.5: 父目录时间戳更新补全)
    touch_parent_dir(fs, dir_ino);

    Ok(())
}

// ============================================================================
// unlink implementation
// ============================================================================

/// Delete a directory entry
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Entry name to delete
///
/// # Returns
/// * Ok(deleted_inode_number) on success
/// * Err(i32) on failure
pub fn ext4_delete_entry(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<u32, i32> {
    // Read parent directory inode
    let dir_inode = super::inode::read_inode(fs, dir_ino)?;

    // htree-indexed directory: entry removal must also update the dx tree;
    // refusing is the only correct option for a linear-only writer
    // (review 5.5: indexed 目录的创建/删除/改名返回 ENOTSUP).
    if dir_inode.i_flags & super::features::EXT4_INDEX_FL != 0 {
        return Err(-(crate::syscall::errno::EOPNOTSUPP as i32));
    }

    // Find the entry
    let (block_nr, offset, entry_ino) = find_dir_entry(fs, &dir_inode, name)?;

    // Read the block
    let block_size = fs.block_size as usize;
    let mut block_data = unsafe {
        read_block_to_vec(fs.device, block_nr, block_size)?
    };

    // Get current entry's record length
    let rec_len = u16::from_le_bytes([block_data[offset + 4], block_data[offset + 5]]);

    // Find previous entry
    let prev_offset = find_prev_entry(&block_data, offset, block_size);

    if prev_offset != offset {
        // Merge with previous entry
        let prev_rec_len = u16::from_le_bytes([
            block_data[prev_offset + 4],
            block_data[prev_offset + 5],
        ]);

        let new_rec_len = prev_rec_len + rec_len;
        let new_rec_bytes = new_rec_len.to_le_bytes();
        block_data[prev_offset + 4] = new_rec_bytes[0];
        block_data[prev_offset + 5] = new_rec_bytes[1];
    }

    // Clear the entry (set inode to 0)
    block_data[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());

    // Write block back
    // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
    unsafe {
        write_block_from_vec(fs.device, block_nr, &block_data)?;
    }

    Ok(entry_ino)
}

/// Find directory entry
fn find_dir_entry(
    fs: &Ext4FileSystem,
    dir_inode: &Ext4InodeOnDisk,
    name: &[u8],
) -> Result<(u64, usize, u32), i32> {
    let block_size = fs.block_size as usize;
    let dir_size = dir_inode.i_size as usize;
    let num_blocks = if block_size > 0 {
        (dir_size + block_size - 1) / block_size
    } else {
        0
    };

    for block_idx in 0..num_blocks as u64 {
        let block_nr = get_dir_block_nr(fs, dir_inode, block_idx)?;

        if block_nr == 0 {
            continue;
        }

        let block_data = unsafe {
            read_block_to_vec(fs.device, block_nr, block_size)?
        };

        // Search for entry in this block
        let mut offset = 0;
        while offset + 8 <= block_size {
            let rec_len = u16::from_le_bytes([
                block_data[offset + 4],
                block_data[offset + 5],
            ]);

            if rec_len == 0 {
                break;
            }

            let ino = u32::from_le_bytes([
                block_data[offset],
                block_data[offset + 1],
                block_data[offset + 2],
                block_data[offset + 3],
            ]);

            if ino == 0 {
                offset += rec_len as usize;
                continue;
            }

            let entry_name_len = block_data[offset + 6] as usize;

            // Compare name
            if entry_name_len == name.len() && offset + 8 + entry_name_len <= block_data.len() {
                let entry_name = &block_data[offset + 8..offset + 8 + entry_name_len];
                if entry_name == name {
                    return Ok((block_nr, offset, ino));
                }
            }

            offset += rec_len as usize;
        }
    }

    Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
}

/// Update a directory's mtime/ctime (wall-clock epoch seconds — see the
/// ext4_setattr timestamp note).
fn touch_parent_dir(fs: &Ext4FileSystem, dir_ino: u32) {
    if let Ok(mut dir) = super::inode::read_inode(fs, dir_ino) {
        let sec = crate::drivers::rtc::wall_secs() as u32;
        dir.i_mtime = sec;
        dir.i_ctime = sec;
        let _ = super::inode::write_inode_disk(fs, dir_ino, &dir);
    }
}

/// Map an inode mode to the ext4 directory-entry file type.
fn file_type_from_mode(mode: u16) -> u8 {
    match mode & S_IFMT {
        S_IFDIR => file_type::EXT4_FT_DIR,
        S_IFREG => file_type::EXT4_FT_REG_FILE,
        0o020000 => file_type::EXT4_FT_CHRDEV,
        0o060000 => file_type::EXT4_FT_BLKDEV,
        0o010000 => file_type::EXT4_FT_FIFO,
        0o140000 => file_type::EXT4_FT_SOCK,
        S_IFLNK => file_type::EXT4_FT_SYMLINK,
        _ => file_type::EXT4_FT_UNKNOWN,
    }
}

/// Walk the ".." chain starting at `dir_ino` and report whether
/// `ancestor_ino` appears (bounded walk: corrupt trees cannot loop us).
fn is_descendant_of(fs: &Ext4FileSystem, dir_ino: u32, ancestor_ino: u32) -> bool {
    let mut current = dir_ino;
    for _ in 0..64 {
        if current == ancestor_ino {
            return true;
        }
        let dir_inode = match super::inode::read_inode(fs, current) {
            Ok(i) => i,
            Err(_) => return false,
        };
        // Read the ".." entry from the first block.
        let block_nr = match get_dir_block_nr(fs, &dir_inode, 0) {
            Ok(b) if b != 0 => b,
            _ => return false,
        };
        let block_data = unsafe {
            match read_block_to_vec(fs.device, block_nr, fs.block_size as usize) {
                Ok(d) => d,
                Err(_) => return false,
            }
        };
        if block_data.len() < 24 {
            return false;
        }
        // "." at offset 0; ".." follows at dot_rec_len.
        let dot_rec_len =
            u16::from_le_bytes([block_data[4], block_data[5]]) as usize;
        if dot_rec_len < 12 || dot_rec_len + 12 > block_data.len() {
            return false;
        }
        let parent = u32::from_le_bytes([
            block_data[dot_rec_len],
            block_data[dot_rec_len + 1],
            block_data[dot_rec_len + 2],
            block_data[dot_rec_len + 3],
        ]);
        if parent == 0 || parent == current {
            return false; // filesystem root
        }
        current = parent;
    }
    false
}

/// Find previous entry in directory block
fn find_prev_entry(block_data: &[u8], target_offset: usize, block_size: usize) -> usize {
    let mut offset = 0;

    while offset + 8 <= block_size {
        let rec_len = u16::from_le_bytes([
            block_data[offset + 4],
            block_data[offset + 5],
        ]) as usize;

        if rec_len == 0 {
            break;
        }

        if offset + rec_len == target_offset {
            return offset;
        }

        offset += rec_len;
    }

    target_offset // No previous entry found
}

/// Unlink a file
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Entry name to unlink
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_unlink(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 8)?;
        // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_unlink_inner(fs, dir_ino, name);
        // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_unlink_inner(fs, dir_ino, name)
}

fn ext4_unlink_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    // Check name (NAME_MAX; ENAMETOOLONG per Linux)
    if name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // unlink(2) on a directory must fail EISDIR BEFORE the directory entry
    // is removed (review 5.5: unlink 目录无 EISDIR — the old code tore the
    // directory down and only then noticed).
    let mut dir_block_hint: Option<u64> = None;
    {
        let dir_inode = super::inode::read_inode(fs, dir_ino)?;
        let (blk, _, entry_ino) = find_dir_entry(fs, &dir_inode, name)?;
        let target = super::inode::read_inode(fs, entry_ino)?;
        if target.is_dir() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }
        dir_block_hint = Some(blk);
    }

    // Delete directory entry
    let entry_ino = ext4_delete_entry(fs, dir_ino, name)?;

    // CRASH-SAFETY (delete side, deferred): the entry REMOVAL must be
    // persisted BEFORE any of the frees below (dead inode, data blocks,
    // inode bitmap) — a crash that lands the bitmap free first leaves a
    // stale entry naming an inode the allocator can hand out again (the
    // cross-link twin of the create-side ghost). Record the removal as a
    // publication and capture every buffer the frees dirty into its
    // `post` set; durability points drain pre -> entry -> post in order.
    // No-op without an active journal handle (nothing is deferred then,
    // so the frees' inline syncs already follow the removal's).
    let _post_capture = PostCaptureGuard::new(fs, dir_block_hint);

    // Read the unlinked inode
    let mut inode = super::inode::read_inode(fs, entry_ino)?;

    // Decrement link count
    if inode.i_links_count > 0 {
        inode.i_links_count -= 1;
    }

    // If link count is 0, free data blocks and inode
    if inode.i_links_count == 0 {
        inode.i_dtime = 1; // TODO: get current time

        // R20-FS1 (F5 order, same fix rename got in r17): persist the dead
        // inode FIRST (dtime set), then free blocks and the inode number.
        // The old order wrote the inode back AFTER free_inode — writing a
        // recycled inode slot / resurrecting freed-block pointers whenever
        // the number got reallocated in the window.
        super::inode::write_inode_disk(fs, entry_ino, &inode)?;
        free_inode_blocks(fs, &inode)?;
        free_inode(fs, entry_ino)?;
        return Ok(());
    }

    // Write inode back (link count decrement for surviving links)
    super::inode::write_inode_disk(fs, entry_ino, &inode)?;

    Ok(())
}

/// Delete-side publication bracket: records the directory block whose
/// entry was removed as an ordered publication and captures the buffers
/// dirtied by the subsequent frees into its `post` set. Drop disarms on
/// every exit path, so the capture window never leaks into unrelated
/// writes.
struct PostCaptureGuard {
    armed: bool,
}

impl PostCaptureGuard {
    fn new(fs: &Ext4FileSystem, dir_block: Option<u64>) -> Self {
        if let Some(blk) = dir_block {
            // SAFETY: get_current_handle is task-local and null-safe.
            if unsafe { get_current_handle() }.is_some() {
                let id = bio::publication_defer_entry_only(fs.device, blk);
                // SAFETY: the handle is alive for this syscall and mutable
                // from its owning task (us).
                unsafe {
                    if let Some(h) = get_current_handle() {
                        let (mj, mi) = bio::dev_key_of(fs.device);
                        (*h).begin_post_capture((mj, mi), id);
                    }
                }
                return PostCaptureGuard { armed: true };
            }
        }
        PostCaptureGuard { armed: false }
    }

    fn disarm(&mut self) {
        if self.armed {
            self.armed = false;
            // SAFETY: handle still alive (same syscall).
            unsafe {
                if let Some(h) = get_current_handle() {
                    (*h).end_post_capture();
                }
            }
        }
    }
}

impl Drop for PostCaptureGuard {
    fn drop(&mut self) {
        self.disarm();
    }
}

// ============================================================================
// rmdir implementation
// ============================================================================

/// Remove an empty directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `dir_ino` - Parent directory inode number
/// * `name` - Directory name to remove
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_rmdir(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 10)?;
        // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
        unsafe { set_current_handle(&mut handle); }
        let result = ext4_rmdir_inner(fs, dir_ino, name);
        // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_rmdir_inner(fs, dir_ino, name)
}

fn ext4_rmdir_inner(
    fs: &Ext4FileSystem,
    dir_ino: u32,
    name: &[u8],
) -> Result<(), i32> {
    // Check name (NAME_MAX; ENAMETOOLONG per Linux)
    if name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // Find the directory entry first
    let parent_inode = super::inode::read_inode(fs, dir_ino)?;
    let (parent_dir_block, _, target_ino) = find_dir_entry(fs, &parent_inode, name)?;

    // Read target directory inode
    let target_inode = super::inode::read_inode(fs, target_ino)?;

    // Verify it's a directory
    if (target_inode.i_mode & S_IFMT) != S_IFDIR {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Check if directory is empty (only "." and "..")
    if !is_dir_empty(fs, &target_inode)? {
        return Err(errno::Errno::DirectoryNotEmpty.as_neg_i32());
    }

    // Delete directory entry from parent
    ext4_delete_entry(fs, dir_ino, name)?;

    // CRASH-SAFETY (delete side, deferred): the entry removal must be
    // persisted BEFORE the inode/bitmap/block frees below (see
    // ext4_unlink_inner).
    let _post_capture = PostCaptureGuard::new(fs, Some(parent_dir_block));

    // Update parent link count
    let mut parent = parent_inode;
    if parent.i_links_count > 0 {
        parent.i_links_count -= 1;
    }
    super::inode::write_inode_disk(fs, dir_ino, &parent)?;

    // Free the target inode
    let mut target = target_inode;
    target.i_links_count = 0;
    target.i_dtime = 1; // TODO: get current time
    super::inode::write_inode_disk(fs, target_ino, &target)?;

    // Free inode in bitmap
    free_inode(fs, target_ino)?;

    // Free data blocks
    free_inode_blocks(fs, &target)?;

    Ok(())
}

/// Check if directory is empty
fn is_dir_empty(fs: &Ext4FileSystem, inode: &Ext4InodeOnDisk) -> Result<bool, i32> {
    let block_size = fs.block_size as usize;
    let dir_size = inode.i_size as usize;

    if dir_size == 0 {
        return Ok(true);
    }

    let num_blocks = (dir_size + block_size - 1) / block_size;

    // Counted ACROSS blocks: with the counter reset per block, a multi-
    // block directory with two live entries per block passed as "empty"
    // and rmdir deleted it (review EXT4-H5).
    let mut entry_count = 0;

    // Iterate ALL directory blocks, not just the first
    for block_idx in 0..num_blocks {
        let block_nr = get_dir_block_nr(fs, inode, block_idx as u64)?;
        if block_nr == 0 {
            continue;
        }

        let block_data = unsafe {
            read_block_to_vec(fs.device, block_nr, block_size)?
        };

        let mut offset = 0;

        while offset + 8 <= block_size {
            let rec_len = u16::from_le_bytes([
                block_data[offset + 4],
                block_data[offset + 5],
            ]);

            if rec_len == 0 {
                break;
            }

            let ino = u32::from_le_bytes([
                block_data[offset],
                block_data[offset + 1],
                block_data[offset + 2],
                block_data[offset + 3],
            ]);

            if ino == 0 {
                offset += rec_len as usize;
                continue;
            }

            entry_count += 1;

            // More than 2 entries means not empty (".", "..", and others)
            if entry_count > 2 {
                return Ok(false);
            }

            offset += rec_len as usize;
        }
    }

    Ok(true)
}

/// Free an inode in the bitmap
fn free_inode(fs: &Ext4FileSystem, ino: u32) -> Result<(), i32> {
    // Drop every cache keyed by this inode BEFORE the number can be
    // reallocated (review EXT4-H8 + icache variant):
    // - page cache: a reader that cached pages of the dying file would
    //   serve them to whatever file reuses this inode number;
    // - VFS icache: path_lookup resurrects cached VFS Inodes, and the
    //   stale sb block map made the new file read/write through the OLD
    //   file's blocks (reproduced via `ln -s x; rm x` + file readback).
    crate::fs::page_cache::get_page_cache().invalidate_inode(fs as *const Ext4FileSystem as u64, ino as u64);
    crate::fs::inode::icache_remove(ino as u64, fs as *const Ext4FileSystem as u64);

    let inodes_per_group = fs.inodes_per_group;
    let group = (ino - 1) / inodes_per_group;
    let ino_in_group = (ino - 1) % inodes_per_group;

    // Get group descriptor
    let bitmap_block = {
        let group_descs = fs.group_descs.lock();
        if group as usize >= group_descs.len() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }
        group_descs[group as usize].bg_inode_bitmap_lo
    };

    // Read bitmap
    let bitmap_data = unsafe {
        read_block_to_vec(fs.device, bitmap_block as u64, fs.block_size as usize)?
    };

    // Clear bit
    let byte_idx = ino_in_group as usize / 8;
    let bit_idx = ino_in_group as usize % 8;

    let mut new_bitmap = bitmap_data.to_vec();
    new_bitmap[byte_idx] &= !(1 << bit_idx);

    // Write bitmap back
    // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
    unsafe {
        write_block_from_vec(fs.device, bitmap_block as u64, &new_bitmap)?;
    }

    // Update group descriptor
    update_group_descriptor_inodes(fs, group, 1)?;

    // Update superblock
    update_superblock_free_inodes(fs, 1)?;

    Ok(())
}

/// Free an indirect block and all data blocks it references (recursive for multi-level)
///
/// # Arguments
/// * `fs` - Filesystem
/// * `allocator` - Block allocator
/// * `blocknr` - Block number of the indirect block
/// * `depth` - Indirection depth (1=single, 2=double, 3=triple)
pub(crate) fn free_indirect_block(
    fs: &Ext4FileSystem,
    allocator: &BlockAllocator,
    blocknr: u32,
    depth: u32,
) -> Result<(), i32> {
    let ptrs_per_block = (fs.block_size as usize) / 4;

    // SAFETY: device is valid; blocknr is a valid indirect block number from the inode.
    let data = unsafe {
        read_block_to_vec(fs.device, blocknr as u64, fs.block_size as usize)?
    };

    // SAFETY: data is block_size bytes; ptrs_per_block = block_size/4 fits exactly.
    let pointers: &[u32] = unsafe {
        core::slice::from_raw_parts(data.as_ptr() as *const u32, ptrs_per_block)
    };

    for &ptr in pointers {
        if ptr == 0 { continue; }
        if depth > 1 {
            free_indirect_block(fs, allocator, ptr, depth - 1)?;
        } else {
            revoke_freed_block(fs, ptr as u64);
            allocator.free_block(ptr as u64)?;
        }
    }

    // Free the indirect block itself
    allocator.free_block(blocknr as u64)?;
    Ok(())
}

/// Revoke a freed block through the CURRENT journal handle (if any).
///
/// Review 5.6 (revoke 最小实现): before a block returns to the free pool,
/// record a jbd2 revoke so recovery cannot replay an OLDER journal entry
/// for that block number over whatever file eventually reallocates it.
/// Outside a transaction (no handle) this is a no-op — the same
/// write-through property that makes our commits immediately checkpointed
/// also means an unjournaled free has no log record to suppress.
fn revoke_freed_block(fs: &Ext4FileSystem, block: u64) {
    if fs.journal.is_none() {
        return;
    }
    // SAFETY: reading the current task's journal handle slot is task-local;
    // the handle lives on this task's stack for the duration of the
    // enclosing ext4_* operation (set/clear bracket it).
    if let Some(handle_ptr) = unsafe { get_current_handle() } {
        // SAFETY: same stack-lifetime contract as above.
        unsafe {
            let _ = crate::fs::jbd2::jbd2_journal_revoke(&mut *handle_ptr, block, None);
        }
    }
}

/// Free all blocks associated with an inode
fn free_inode_blocks(fs: &Ext4FileSystem, inode: &Ext4InodeOnDisk) -> Result<(), i32> {
    let allocator = BlockAllocator::new(fs);

    // Fast symlinks store the target STRING inside i_block — there are no
    // data blocks to free. Freeing the string bytes as block numbers
    // cleared arbitrary bitmap bits (or aborted with an error, leaking the
    // inode) — review EXT4-C3.
    if inode.is_symlink() && inode.i_size <= 60 {
        return Ok(());
    }

    // Check if using extents
    if (inode.i_flags & 0x80000) != 0 {
        // Free blocks referenced by extent entries — ANY tree depth: the
        // tree engine gathers every leaf extent (external nodes included;
        // the old walker only handled the 4-entry inline root and silently
        // LEAKED every deep-tree file) and the index/leaf metadata blocks
        // are freed afterwards.
        let header = unsafe {
            &*(inode.i_block.as_ptr() as *const super::extent::Ext4ExtentHeader)
        };
        if header.eh_magic == super::extent::EXT4_EXT_MAGIC {
            let exts = super::extent::ext4_ext_gather(fs, &inode.i_block)?;
            for e in &exts {
                // Run-free (one bitmap pass for the whole contiguous
                // extent): a fallocate-preallocated scratch file spans
                // 76800 blocks — per-block frees are 4 synchronous I/Os
                // each and effectively hang unlink (LTP tst_rmdir of the
                // device image timed out and leaked the space, driving
                // the whole filesystem into ENOSPC).
                revoke_freed_block(fs, e.phys);
                allocator.free_block_run(e.phys, e.len as u64)?;
            }
            let mut freed_meta: u64 = 0;
            super::extent::ext4_ext_free_index_blocks(fs, &inode.i_block, &mut freed_meta)?;
        }
        return Ok(());
    }

    // Direct/indirect block mode: free direct blocks
    for i in 0..12 {
        if inode.i_block[i] != 0 {
            revoke_freed_block(fs, inode.i_block[i] as u64);
            allocator.free_block(inode.i_block[i] as u64)?;
        }
    }

    // Free single indirect block
    if inode.i_block[12] != 0 {
        free_indirect_block(fs, &allocator, inode.i_block[12], 1)?;
    }
    // Free double indirect block
    if inode.i_block[13] != 0 {
        free_indirect_block(fs, &allocator, inode.i_block[13], 2)?;
    }
    // Free triple indirect block
    if inode.i_block[14] != 0 {
        free_indirect_block(fs, &allocator, inode.i_block[14], 3)?;
    }

    Ok(())
}

// ============================================================================
// rename implementation
// ============================================================================

/// Rename a file or directory
///
/// # Arguments
/// * `fs` - Filesystem
/// * `old_dir_ino` - Old parent directory inode number
/// * `old_name` - Old entry name
/// * `new_dir_ino` - New parent directory inode number
/// * `new_name` - New entry name
///
/// # Returns
/// * Ok(()) on success
/// * Err(i32) on failure
pub fn ext4_rename(
    fs: &Ext4FileSystem,
    old_dir_ino: u32,
    old_name: &[u8],
    new_dir_ino: u32,
    new_name: &[u8],
) -> Result<(), i32> {
    if fs.journal.is_some() {
        let mut handle = super::journal::ext4_journal_start(fs, 16)?;
        // SAFETY: handle is a local variable from ext4_journal_start; set_current_handle stores it in a thread-local for jbd2 metadata journaling during this operation.
        unsafe { set_current_handle(&mut handle); }
        // PRE-capture window for the new-name publication (see
        // defer_entry_publication). A replace of an existing target
        // separately arms the POST window (PostCaptureGuard) after this
        // pre record has been published — the two never overlap.
        handle.begin_pre_capture(crate::fs::bio::dev_key_of(fs.device));
        let result = ext4_rename_inner(fs, old_dir_ino, old_name, new_dir_ino, new_name);
        // SAFETY: clear_current_handle resets the thread-local journal handle to None; no other references to handle exist after this point.
        unsafe { clear_current_handle(); }
        super::journal::ext4_journal_stop(&mut handle)?;
        return result;
    }
    ext4_rename_inner(fs, old_dir_ino, old_name, new_dir_ino, new_name)
}

fn ext4_rename_inner(
    fs: &Ext4FileSystem,
    old_dir_ino: u32,
    old_name: &[u8],
    new_dir_ino: u32,
    new_name: &[u8],
) -> Result<(), i32> {
    // Validate names (NAME_MAX; ENAMETOOLONG per Linux)
    if old_name.is_empty() || new_name.is_empty() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    if old_name.len() > 255 || new_name.len() > 255 {
        return Err(-(crate::syscall::errno::ENAMETOOLONG as i32));
    }

    // Read parent directory inodes
    let old_dir_inode = super::inode::read_inode(fs, old_dir_ino)?;
    let new_dir_inode = super::inode::read_inode(fs, new_dir_ino)?;

    // Find old entry
    let (_, _, old_ino) = find_dir_entry(fs, &old_dir_inode, old_name)?;

    // Read the inode being renamed
    let old_inode = super::inode::read_inode(fs, old_ino)?;
    let old_is_dir = (old_inode.i_mode & S_IFMT) == S_IFDIR;

    // Determine file type for the new directory entry from the INODE MODE,
    // not the old binary dir-or-file guess — symlinks, devices and fifos
    // must keep their d_type (review 5.5: rename 保留原 file_type).
    let new_file_type = file_type_from_mode(old_inode.i_mode);

    // Renaming a directory into itself or its own subdirectory would
    // create a ".." cycle (review 5.5: rename 环检查错 — the old check only
    // compared names within the same directory). Walk the new parent's
    // ancestor chain; if it reaches the renamed directory, refuse.
    if old_is_dir && old_dir_ino != new_dir_ino {
        if new_dir_ino == old_ino || is_descendant_of(fs, new_dir_ino, old_ino) {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }
    }

    // Check if new name already exists
    let target_exists = find_dir_entry(fs, &new_dir_inode, new_name).ok();

    if let Some((target_dir_block, _, target_ino)) = target_exists {
        // Cannot rename to self
        if target_ino == old_ino {
            return Ok(());
        }

        let target_inode = super::inode::read_inode(fs, target_ino)?;
        let target_is_dir = (target_inode.i_mode & S_IFMT) == S_IFDIR;

        // Type checks
        if old_is_dir && !target_is_dir {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }
        if !old_is_dir && target_is_dir {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }
        if target_is_dir && !is_dir_empty(fs, &target_inode)? {
            return Err(errno::Errno::DirectoryNotEmpty.as_neg_i32());
        }

        // Delete existing target entry
        ext4_delete_entry(fs, new_dir_ino, new_name)?;

        // CRASH-SAFETY (delete side, rename-replace, deferred): the target
        // entry's removal must be persisted BEFORE the inode/bitmap/block
        // frees below — same rationale as ext4_unlink_inner (a stale entry
        // naming a freed, reallocatable inode is the create-side ghost's
        // twin).
        let _post_capture = PostCaptureGuard::new(fs, Some(target_dir_block));

        // Clean up the replaced inode
        let mut target_mut = target_inode;
        if target_is_dir {
            // Decrement new parent's link count (was incremented by mkdir).
            // RE-READ first: ext4_delete_entry above may have updated the
            // on-disk parent (size/blocks); writing the stale snapshot
            // would roll that back (review EXT4-H7).
            let mut new_parent = super::inode::read_inode(fs, new_dir_ino)?;
            if new_parent.i_links_count > 0 {
                new_parent.i_links_count -= 1;
            }
            super::inode::write_inode_disk(fs, new_dir_ino, &new_parent)?;

            // Free target directory
            target_mut.i_links_count = 0;
            target_mut.i_dtime = 1;
            super::inode::write_inode_disk(fs, target_ino, &target_mut)?;
            free_inode(fs, target_ino)?;
            free_inode_blocks(fs, &target_mut)?;
        } else {
            // Decrement link count of replaced file
            if target_mut.i_links_count > 0 {
                target_mut.i_links_count -= 1;
            }
            if target_mut.i_links_count == 0 {
                target_mut.i_dtime = 1;
                // R14-8 (F5, order fixed r17): persist the dead inode FIRST
                // (dtime set), then free blocks and the inode number — the
                // old order wrote the inode back AFTER freeing it (writing
                // a recycled inode / resurrecting freed-block pointers).
            }
            super::inode::write_inode_disk(fs, target_ino, &target_mut)?;
            if target_mut.i_links_count == 0 {
                free_inode_blocks(fs, &target_mut)?;
                free_inode(fs, target_ino)?;
            }
        }
    }

    // Prevent renaming a directory into its own subdirectory
    if old_is_dir && old_dir_ino == new_dir_ino && old_name == new_name {
        return Ok(());
    }

    // Add new directory entry
    ext4_add_entry(fs, new_dir_ino, new_name, old_ino, new_file_type)?;

    // Delete old directory entry
    ext4_delete_entry(fs, old_dir_ino, old_name)?;

    // Update timestamp on renamed inode
    let sec = crate::drivers::rtc::wall_secs() as u32;
    let mut renamed_inode = old_inode;
    renamed_inode.i_ctime = sec;
    renamed_inode.i_mtime = sec;
    super::inode::write_inode_disk(fs, old_ino, &renamed_inode)?;

    // If renaming a directory, update parent link counts and ".." entry.
    // Both parents are RE-READ from disk here: ext4_add_entry /
    // ext4_delete_entry above may have grown the directories (new block,
    // i_size/i_block updates via write_inode_disk). Writing the snapshots
    // taken at function entry rolled those updates back — the entry in
    // the freshly allocated block got unlinked from the inode and the
    // rename silently lost the file (review EXT4-H7).
    if old_is_dir {
        let mut old_parent = super::inode::read_inode(fs, old_dir_ino)?;
        let mut new_parent = if old_dir_ino == new_dir_ino {
            old_parent
        } else {
            super::inode::read_inode(fs, new_dir_ino)?
        };

        if old_dir_ino != new_dir_ino {
            // Decrement old parent's link count
            if old_parent.i_links_count > 0 {
                old_parent.i_links_count -= 1;
            }
            // Increment new parent's link count
            new_parent.i_links_count += 1;

            // Update ".." entry in the renamed directory to point to new parent
            update_dotdot(fs, old_ino, new_dir_ino)?;
        }

        // Update timestamps on parent directories
        old_parent.i_ctime = sec;
        old_parent.i_mtime = sec;
        new_parent.i_ctime = sec;
        new_parent.i_mtime = sec;

        super::inode::write_inode_disk(fs, old_dir_ino, &old_parent)?;
        if old_dir_ino != new_dir_ino {
            super::inode::write_inode_disk(fs, new_dir_ino, &new_parent)?;
        }
    } else {
        // Update timestamps on parent directories for file rename
        let mut old_parent = super::inode::read_inode(fs, old_dir_ino)?;
        old_parent.i_ctime = sec;
        old_parent.i_mtime = sec;
        super::inode::write_inode_disk(fs, old_dir_ino, &old_parent)?;

        if old_dir_ino != new_dir_ino {
            let mut new_parent = super::inode::read_inode(fs, new_dir_ino)?;
            new_parent.i_ctime = sec;
            new_parent.i_mtime = sec;
            super::inode::write_inode_disk(fs, new_dir_ino, &new_parent)?;
        }
    }

    Ok(())
}

/// Update the ".." entry of a directory to point to a new parent
fn update_dotdot(fs: &Ext4FileSystem, dir_ino: u32, new_parent_ino: u32) -> Result<(), i32> {
    let dir_inode = super::inode::read_inode(fs, dir_ino)?;
    let block_size = fs.block_size as usize;

    // ".." is always the first entry in the first block
    let block_nr = get_dir_block_nr(fs, &dir_inode, 0)?;
    if block_nr == 0 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    let mut block_data = unsafe {
        read_block_to_vec(fs.device, block_nr, block_size)?
    };

    // First entry is ".", second is ".."
    // Skip "." entry (at offset 0)
    let dot_rec_len = u16::from_le_bytes([block_data[4], block_data[5]]);
    let dotdot_offset = dot_rec_len as usize;

    // Verify this is ".." entry
    if block_data.len() < dotdot_offset + 8 {
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    // Update inode number of ".." entry
    let new_parent_bytes = new_parent_ino.to_le_bytes();
    block_data[dotdot_offset] = new_parent_bytes[0];
    block_data[dotdot_offset + 1] = new_parent_bytes[1];
    block_data[dotdot_offset + 2] = new_parent_bytes[2];
    block_data[dotdot_offset + 3] = new_parent_bytes[3];

    // Write block back
    // SAFETY: fs.device is a valid GenDisk pointer; block numbers come from block group descriptors or inode metadata; bio::bread returns valid BufferHeads.
    unsafe {
        write_block_from_vec(fs.device, block_nr, &block_data)?;
    }

    Ok(())
}
