//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 file operations

use crate::errno;
use crate::fs::bio;
use crate::fs::ext4::indirect;

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
                // Sparse file, block not allocated, skip
                let remaining = to_write as usize - total_written;
                let skip_to_block_end = block_size as usize - block_offset;
                let skip = core::cmp::min(remaining, skip_to_block_end);
                total_written += skip;
                buf_offset += skip;
                current_offset += skip as u64;
                continue;
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

    // TODO: Update inode timestamp
    // TODO: Sync inode to disk

    Ok(total_written)
}

fn allocate_blocks_for_file(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
) -> Result<(), i32> {
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    let block_size = fs.block_size as u64;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;

    // Allocate new blocks
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

fn allocate_indirect_block(
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
