//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 file operations

use crate::errno;
use crate::fs::bio;
use crate::fs::ext4::indirect;
use crate::fs::file::{File, FileOps};
use crate::fs::inode::Inode;
use crate::fs::io_completion::IoCompletion;
use crate::fs::page_cache;
use crate::fs::readahead::{ReadAheadState, MAX_READAHEAD_BLOCKS};

pub fn ext4_file_read(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &mut [u8],
) -> Result<usize, i32> {
    let file_size = inode.get_size();

    if offset >= file_size {
        return Ok(0);  // EOF
    }

    let available = file_size - offset;
    let to_read = core::cmp::min(buf.len() as u64, available) as usize;

    let blocks = inode.get_data_blocks(fs)?;
    let block_size = fs.block_size as usize;

    let mut total_read = 0;
    let mut current_offset = offset as usize;
    let mut buf_offset = 0;

    while total_read < to_read {
        let block_index = current_offset / block_size;
        let block_offset = current_offset % block_size;

        if block_index >= blocks.len() {
            break;
        }

        unsafe {
            let bh = bio::bread(fs.device, blocks[block_index])
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &(*bh).b_data;
            let remaining = to_read - total_read;
            let available_in_block = block_size - block_offset;
            let read_in_block = core::cmp::min(remaining, available_in_block);

            buf[buf_offset..buf_offset + read_in_block]
                .copy_from_slice(&data[block_offset..block_offset + read_in_block]);

            total_read += read_in_block;
            buf_offset += read_in_block;
            current_offset += read_in_block;

            bio::brelse(bh);
        }
    }

    Ok(total_read)
}

/// Read file data with page cache and read-ahead.
///
/// Uses `get_data_block(index)` for single-block resolution (instead of
/// resolving the entire block map) and caches pages in the global page cache.
fn ext4_file_read_cached(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &mut [u8],
    ra_state: &mut ReadAheadState,
) -> Result<usize, i32> {
    let file_size = inode.get_size();
    if offset >= file_size {
        return Ok(0);
    }

    let available = file_size - offset;
    let to_read = core::cmp::min(buf.len() as u64, available) as usize;
    let block_size = fs.block_size as u64;
    let block_size_usize = fs.block_size as usize;
    let cache = page_cache::get_page_cache();
    let ino = inode.ino;

    let mut total_read = 0;
    let mut current_offset = offset;
    let mut buf_offset = 0;

    while total_read < to_read {
        let page_index = current_offset / block_size;
        let page_offset = (current_offset % block_size) as usize;

        // Check page cache
        if let Some(page_data) = cache.get(ino, page_index) {
            // Cache hit: copy from cached page
            unsafe {
                let remaining = to_read - total_read;
                let available_in_page = block_size_usize - page_offset;
                let copy_len = core::cmp::min(remaining, available_in_page);
                core::ptr::copy_nonoverlapping(
                    page_data.add(page_offset),
                    buf.as_mut_ptr().add(buf_offset),
                    copy_len,
                );
                total_read += copy_len;
                buf_offset += copy_len;
                current_offset += copy_len as u64;
            }
            cache.put(ino, page_index);
        } else {
            // Cache miss: resolve single block and read from disk
            let block_nr = inode.get_data_block(fs, page_index)?;
            if block_nr == 0 {
                // Sparse file: zero-fill
                let remaining = to_read - total_read;
                let available_in_page = block_size_usize - page_offset;
                let zero_len = core::cmp::min(remaining, available_in_page);
                for i in 0..zero_len {
                    buf[buf_offset + i] = 0;
                }
                total_read += zero_len;
                buf_offset += zero_len;
                current_offset += zero_len as u64;
                continue;
            }

            unsafe {
                let bh = bio::bread(fs.device, block_nr)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let remaining = to_read - total_read;
                let available_in_page = block_size_usize - page_offset;
                let copy_len = core::cmp::min(remaining, available_in_page);

                buf[buf_offset..buf_offset + copy_len]
                    .copy_from_slice(&data[page_offset..page_offset + copy_len]);

                // Insert full page into page cache
                cache.insert(ino, page_index, block_nr, &data);

                total_read += copy_len;
                buf_offset += copy_len;
                current_offset += copy_len as u64;

                bio::brelse(bh);
            }
        }
    }

    // Update read-ahead state and issue prefetch if needed
    let (should_ra, ra_start, ra_count) = ra_state.on_read(offset, total_read as u64);
    if should_ra {
        let file_pages = (file_size + block_size - 1) / block_size;

        // Async batch submit: submit all read-ahead I/Os, then wait once.
        let max_ra = MAX_READAHEAD_BLOCKS as usize;
        let mut completions: [IoCompletion; 4] = core::array::from_fn(|_| IoCompletion::new());
        let mut bh_ptrs = [core::ptr::null_mut::<bio::BufferHead>(); 4];
        let mut count = 0usize;

        for i in 0..ra_count {
            if count >= max_ra { break; }
            let idx = ra_start + i as u64;
            if idx >= file_pages { break; }

            // Skip if already cached
            if cache.get(ino, idx).is_some() {
                cache.put(ino, idx);
                continue;
            }

            // Resolve block number
            if let Ok(block_nr) = inode.get_data_block(fs, idx) {
                if block_nr != 0 {
                    if let Some(bh) = bio::bread_async(fs.device, block_nr, &completions[count]) {
                        bh_ptrs[count] = bh;
                        count += 1;
                    }
                }
            }
        }

        // Wait for all async I/Os to complete
        if count > 0 {
            for i in 0..count {
                bio::bread_wait(bh_ptrs[i], &completions[i]);
            }
            // Insert completed pages into page cache
            for i in 0..count {
                unsafe {
                    let data = &(*bh_ptrs[i]).b_data;
                    cache.insert(ino, ra_start + i as u64,
                        (*bh_ptrs[i]).b_blocknr, data);
                    bio::brelse(bh_ptrs[i]);
                }
            }
        }
    }

    Ok(total_read)
}

pub fn ext4_file_write(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &[u8],
) -> Result<usize, i32> {
    let block_size = fs.block_size as u64;
    let to_write = buf.len() as u64;

    // Calculate required block count
    let end_offset = offset + to_write;
    let needed_blocks = (end_offset + block_size - 1) / block_size;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let sectors_per_block = (fs.block_size / 512) as u64;

    // If new blocks are needed, allocate them
    if needed_blocks > current_blocks {
        allocate_blocks_for_file(fs, inode, needed_blocks)?;
    }

    // Write data
    let mut total_written = 0;
    let mut current_offset = offset;
    let mut buf_offset = 0;

    while total_written < to_write as usize {
        let block_index = current_offset / block_size;
        let block_offset = (current_offset % block_size) as usize;

        // Get data block number (supports indirect blocks)
        let block_num = match inode.get_data_block(fs, block_index) {
            Ok(0) => {
                // Block not allocated, need to allocate a new one for writing
                let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                let new_block = match allocator.alloc_block() {
                    Ok(b) => b,
                    Err(e) => return Err(e),
                };

                // Zero the new block
                unsafe {
                    let bh = bio::bread(fs.device, new_block)
                        .ok_or(errno::Errno::IOError.as_neg_i32())?;

                    for byte in (*bh).b_data.iter_mut() {
                        *byte = 0;
                    }
                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    bio::sync_dirty_buffer(bh)?;
                    bio::brelse(bh);
                }

                // Update inode block pointer
                if block_index < 12 {
                    inode.block[block_index as usize] = new_block as u32;
                } else {
                    // Handle indirect blocks
                    allocate_indirect_block(fs, inode, block_index, new_block, &allocator)?;
                }
                inode.blocks += sectors_per_block;

                new_block
            }
            Ok(b) => b,
            Err(e) => return Err(e),
        };

        unsafe {
            let bh = bio::bread(fs.device, block_num)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            let remaining = to_write as usize - total_written;
            let available_in_block = block_size as usize - block_offset;
            let write_in_block = core::cmp::min(remaining, available_in_block);

            // Write data to block
            data[block_offset..block_offset + write_in_block]
                .copy_from_slice(&buf[buf_offset..buf_offset + write_in_block]);

            // Mark as dirty
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;
            bio::brelse(bh);

            total_written += write_in_block;
            buf_offset += write_in_block;
            current_offset += write_in_block as u64;
        }
    }

    // Update file size
    if end_offset > inode.get_size() {
        inode.set_size(end_offset);
    }

    // Update inode timestamp
    let cycles = crate::drivers::intc::clint::read_time();
    let sec = (cycles / 10_000_000) as u32;
    inode.mtime = sec;
    inode.ctime = sec;

    Ok(total_written)
}

pub fn allocate_blocks_for_file(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
) -> Result<(), i32> {
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    let block_size = fs.block_size as u64;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let sectors_per_block = (fs.block_size / 512) as u64;

    // Check if file uses extents
    if inode.has_extent() {
        return allocate_blocks_with_extents(fs, inode, needed_blocks, current_blocks, &allocator);
    }

    // Allocate new blocks (indirect block mode)
    for i in current_blocks..needed_blocks {
        match allocator.alloc_block() {
            Ok(data_block) => {
                // Zero newly allocated data block
                unsafe {
                    let bh = bio::bread(fs.device, data_block)
                        .ok_or(errno::Errno::IOError.as_neg_i32())?;

                    for byte in (*bh).b_data.iter_mut() {
                        *byte = 0;
                    }

                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    bio::sync_dirty_buffer(bh)?;
                    bio::brelse(bh);
                }

                // Decide how to store block number based on block index
                let block_index = i;

                if block_index < 12 {
                    // Direct block
                    inode.block[block_index as usize] = data_block as u32;
                } else {
                    // Indirect block
                    allocate_indirect_block(fs, inode, block_index, data_block, &allocator)?;
                }
                inode.blocks += sectors_per_block;
            }
            Err(e) => {
                // Allocation failed, rollback allocated blocks
                // TODO: Implement complete rollback
                return Err(e);
            }
        }
    }

    Ok(())
}

/// Allocate blocks for extent-based files
/// Creates a simple inline extent that maps logical blocks to physical blocks
fn allocate_blocks_with_extents(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
    current_blocks: u64,
    allocator: &crate::fs::ext4::allocator::BlockAllocator,
) -> Result<(), i32> {
    use crate::fs::ext4::extent::{Ext4ExtentHeader, Ext4Extent, EXT4_EXT_MAGIC};

    let sectors_per_block = (fs.block_size / 512) as u64;

    // For simplicity, allocate blocks one by one and update/create extent
    for logical_block in current_blocks..needed_blocks {
        let physical_block = allocator.alloc_block()?;

        // Zero the new block
        unsafe {
            let bh = bio::bread(fs.device, physical_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            for byte in (*bh).b_data.iter_mut() {
                *byte = 0;
            }
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;
            bio::brelse(bh);
        }

        // Update extent tree
        // For simple case, we just create/extend an inline extent in i_block
        let header = unsafe {
            &mut *(inode.block.as_mut_ptr() as *mut Ext4ExtentHeader)
        };

        if header.eh_magic != EXT4_EXT_MAGIC {
            // Initialize new extent header
            header.eh_magic = EXT4_EXT_MAGIC;
            header.eh_entries = 0;
            header.eh_max = 4; // Max inline extents
            header.eh_depth = 0;
            header.eh_generation = 0;
        }

        // Get or create extent entry
        let entries = unsafe {
            core::slice::from_raw_parts_mut(
                (inode.block.as_mut_ptr() as *mut u8).add(core::mem::size_of::<Ext4ExtentHeader>()) as *mut Ext4Extent,
                header.eh_max as usize
            )
        };

        // Check if we can extend the last extent or need a new one
        if header.eh_entries > 0 {
            let last_entry = &mut entries[(header.eh_entries - 1) as usize];
            let last_end = last_entry.ee_block as u64 + last_entry.length() as u64;

            if last_end == logical_block && last_entry.length() < 0x8000 {
                // Can extend last extent (contiguous blocks)
                // Check if physical blocks are contiguous
                let expected_physical = last_entry.start_block() + last_entry.length() as u64;
                if physical_block == expected_physical {
                    last_entry.ee_len += 1;
                    inode.blocks += sectors_per_block;
                    continue;
                }
            }
        }

        // Need to add new extent entry
        if header.eh_entries >= header.eh_max {
            // No space for new extent - should not happen for small files
            return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
        }

        let new_entry = &mut entries[header.eh_entries as usize];
        new_entry.ee_block = logical_block as u32;
        new_entry.ee_len = 1;
        new_entry.ee_start_hi = (physical_block >> 32) as u16;
        new_entry.ee_start_lo = physical_block as u32;
        header.eh_entries += 1;
        inode.blocks += sectors_per_block;
    }

    Ok(())
}

pub fn allocate_indirect_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    block_index: u64,
    data_block: u64,
    allocator: &crate::fs::ext4::allocator::BlockAllocator,
) -> Result<(), i32> {
    let block_size = fs.block_size as u64;
    let pointers_per_block = block_size / 4;
    let indirect_offset = block_index - 12;

    if indirect_offset < pointers_per_block {
        // Single indirect block
        if inode.block[12] == 0 {
            // Need to allocate single indirect block
            let indirect_block = allocator.alloc_block()?;
            inode.block[12] = indirect_block as u32;

            // Zero indirect block
            unsafe {
                let bh = bio::bread(fs.device, indirect_block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                for byte in (*bh).b_data.iter_mut() {
                    *byte = 0;
                }

                (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                bio::sync_dirty_buffer(bh)?;
                bio::brelse(bh);
            }
        }

        // Write block number to indirect block
        indirect::write_indirect_block(
            fs,
            inode.block[12] as u64,
            indirect_offset as usize,
            data_block as u32,
        )?;
    } else {
        let double_offset = indirect_offset - pointers_per_block;
        let double_pointers = pointers_per_block * pointers_per_block;

        if double_offset < double_pointers {
            // Double indirect block
            if inode.block[13] == 0 {
                // Need to allocate double indirect block
                let double_block = allocator.alloc_block()?;
                inode.block[13] = double_block as u32;

                // Zero
                unsafe {
                    let bh = bio::bread(fs.device, double_block)
                        .ok_or(errno::Errno::IOError.as_neg_i32())?;

                    for byte in (*bh).b_data.iter_mut() {
                        *byte = 0;
                    }

                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    bio::sync_dirty_buffer(bh)?;
                    bio::brelse(bh);
                }
            }

            // First level index
            let first_index = (double_offset / pointers_per_block) as usize;
            let second_index = (double_offset % pointers_per_block) as usize;

            // Get or allocate single indirect block
            let mut indirect_block = indirect::read_indirect_block(
                fs,
                inode.block[13] as u64,
                first_index,
            )?;

            if indirect_block == 0 {
                // Need to allocate single indirect block
                indirect_block = allocator.alloc_block()?;

                // Zero
                unsafe {
                    let bh = bio::bread(fs.device, indirect_block)
                        .ok_or(errno::Errno::IOError.as_neg_i32())?;

                    for byte in (*bh).b_data.iter_mut() {
                        *byte = 0;
                    }

                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    bio::sync_dirty_buffer(bh)?;
                    bio::brelse(bh);
                }

                // Update double indirect block
                indirect::write_indirect_block(
                    fs,
                    inode.block[13] as u64,
                    first_index,
                    indirect_block as u32,
                )?;
            }

            // Write data block number to single indirect block
            indirect::write_indirect_block(
                fs,
                indirect_block,
                second_index,
                data_block as u32,
            )?;
        } else {
            // Triple indirect block - not supported yet
            return Err(errno::Errno::FileTooLarge.as_neg_i32());
        }
    }

    Ok(())
}

pub fn ext4_file_lseek(
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: isize,
    whence: i32,
) -> Result<isize, i32> {
    let file_size = inode.get_size() as isize;

    let new_pos = match whence {
        0 => offset,              // SEEK_SET
        1 => {
            // TODO: Need to track current file position
            return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
        }
        2 => file_size + offset,   // SEEK_END
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    };

    if new_pos < 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    Ok(new_pos)
}

pub fn ext4_sync_file(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
) -> Result<(), i32> {
    // Sync all data blocks of file
    let blocks = inode.get_data_blocks(fs)?;

    for block in blocks {
        unsafe {
            let bh = bio::bread(fs.device, block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            if (*bh).is_dirty() {
                bio::sync_dirty_buffer(bh)?;
            }

            bio::brelse(bh);
        }
    }

    Ok(())
}

// ============================================================================
// VFS Wrapper Functions
// ============================================================================

/// VFS read wrapper - calls ext4_file_read_cached with page cache and read-ahead
pub fn ext4_file_read_vfs(file: &File, buf: &mut [u8]) -> isize {
    unsafe {
        // Get VFS inode from file
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt {
            Some(i) => i,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };

        // Get ext4 filesystem pointer from inode's private_data
        let fs_ptr = match inode.private_data {
            Some(ptr) => ptr as *const crate::fs::ext4::Ext4FileSystem,
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };
        let fs = &*fs_ptr;

        // Use cached Ext4Inode from inode.sb instead of re-reading from disk
        let ext4_inode = match inode.sb {
            Some(ptr) => &*(ptr as *const super::inode::Ext4Inode),
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };

        // Get current file position
        let offset = file.get_pos() as u64;

        // Get or create read-ahead state from file.private_data
        let ra_state = get_or_create_ra_state(file, fs.block_size as u64);

        // Call cached read function
        match ext4_file_read_cached(fs, ext4_inode, offset, buf, ra_state) {
            Ok(read_bytes) => {
                file.set_pos(offset + read_bytes as u64);
                read_bytes as isize
            }
            Err(e) => e as isize,
        }
    }
}

/// VFS write wrapper - calls ext4_file_write
pub fn ext4_file_write_vfs(file: &File, buf: &[u8]) -> isize {
    unsafe {
        // Get VFS inode from file
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt {
            Some(i) => i,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };

        // Get ext4 filesystem pointer from inode's private_data
        let fs_ptr = match inode.private_data {
            Some(ptr) => ptr as *const crate::fs::ext4::Ext4FileSystem,
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };
        let fs = &*fs_ptr;
        let ext4_ino = inode.ino as u32;

        // Start a journal transaction for data=ordered semantics:
        // data blocks are synced during write, then the inode metadata is
        // committed to the journal with all data already on disk.
        let use_journal = fs.journal.is_some();
        let mut journal_handle = if use_journal {
            match super::journal::ext4_journal_start(fs, 4) {
                Ok(mut h) => {
                    super::namei::set_current_handle(&mut h);
                    Some(h)
                }
                Err(_) => None,
            }
        } else {
            None
        };

        // Read ext4 inode from disk (write needs fresh on-disk data)
        let mut ext4_inode = match fs.read_inode(ext4_ino) {
            Ok(inode) => inode,
            Err(e) => {
                if journal_handle.is_some() {
                    super::namei::clear_current_handle();
                    if let Some(mut h) = journal_handle {
                        let _ = super::journal::ext4_journal_stop(&mut h);
                    }
                }
                return e as isize;
            }
        };

        // Get current file position (O_APPEND: always write at end of file)
        let offset = if file.flags.bits() & crate::fs::file::FileFlags::O_APPEND != 0 {
            ext4_inode.get_size()
        } else {
            file.get_pos() as u64
        };

        // Call internal write function
        let result = match ext4_file_write(fs, &mut ext4_inode, offset, buf) {
            Ok(written_bytes) => {
                // Update cached copy in inode.sb
                if let Some(ptr) = inode.sb {
                    let cached = &mut *(ptr as *mut super::inode::Ext4Inode);
                    cached.block = ext4_inode.block;
                    cached.size = ext4_inode.size;
                    cached.blocks = ext4_inode.blocks;
                    cached.mtime = ext4_inode.mtime;
                    cached.ctime = ext4_inode.ctime;
                }
                // Update cached VFS inode size
                inode.size.store(ext4_inode.get_size(), core::sync::atomic::Ordering::Relaxed);
                // Write back inode to disk (registers with journal if handle active)
                match crate::fs::ext4::inode::write_inode(fs, ext4_ino, &ext4_inode) {
                    Ok(()) => {
                        // Invalidate page cache for this inode after write
                        page_cache::get_page_cache().invalidate_inode(ext4_ino);
                        // Update file position
                        file.set_pos(offset + written_bytes as u64);
                        written_bytes as isize
                    }
                    Err(e) => e as isize,
                }
            }
            Err(e) => e as isize,
        };

        // Stop journal transaction
        if journal_handle.is_some() {
            super::namei::clear_current_handle();
            if let Some(mut h) = journal_handle {
                let _ = super::journal::ext4_journal_stop(&mut h);
            }
        }

        result
    }
}

/// Get or create ReadAheadState stored in file.private_data.
unsafe fn get_or_create_ra_state<'a>(file: &File, block_size: u64) -> &'a mut ReadAheadState {
    let ptr = file.private_data.get();
    if let Some(state_ptr) = *ptr {
        &mut *(state_ptr as *mut ReadAheadState)
    } else {
        let state = alloc::boxed::Box::new(ReadAheadState::new(block_size));
        let state_ptr = alloc::boxed::Box::into_raw(state);
        *ptr = Some(state_ptr as *mut u8);
        &mut *state_ptr
    }
}

/// Close callback — free ReadAheadState from file.private_data.
fn ext4_file_close(file: &File) -> i32 {
    unsafe {
        let ptr = file.private_data.get();
        if let Some(state_ptr) = *ptr {
            let _ = alloc::boxed::Box::from_raw(state_ptr as *mut ReadAheadState);
            *ptr = None;
        }
    }
    0
}

/// Ext4 file operations structure
pub static EXT4_FILE_OPS: FileOps = FileOps {
    read: Some(ext4_file_read_vfs),
    write: Some(ext4_file_write_vfs),
    lseek: Some(reg_file_lseek),
    close: Some(ext4_file_close),
    poll: None,
};

/// Default regular file lseek implementation
fn reg_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    let inode_opt = unsafe { &*file.inode.get() };
    let inode = match inode_opt {
        Some(i) => i,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };

    // Get file size from cached VFS inode (avoids re-reading from disk)
    let file_size = inode.size.load(core::sync::atomic::Ordering::Relaxed) as i64;

    let current_pos = file.get_pos() as i64;
    let new_pos = match whence {
        0 => offset as i64,              // SEEK_SET
        1 => current_pos + offset as i64, // SEEK_CUR
        2 => file_size + offset as i64,   // SEEK_END
        _ => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };

    if new_pos < 0 {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }

    file.set_pos(new_pos as u64);
    new_pos as isize
}
