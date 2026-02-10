//! ext4 文件操作
//!
//! 完全遵循 Linux 内核的 ext4 文件操作实现
//! 参考: fs/ext4/file.c

use crate::errno;
use crate::fs::bio;

/// ext4 文件读取
///
/// 对应 Linux 的 ext4_file_read_iter (fs/ext4/file.c)
///
/// # 参数
/// - `fs`: ext4 文件系统实例
/// - `inode`: 文件 inode
/// - `offset`: 读取偏移
/// - `buf`: 输出缓冲区
/// - `count`: 要读取的字节数
///
/// # 返回
/// 成功返回读取的字节数，失败返回错误码
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

/// ext4 文件写入
///
/// 对应 Linux 的 ext4_file_write_iter (fs/ext4/file.c)
///
/// # 参数
/// - `fs`: ext4 文件系统实例
/// - `inode`: 文件 inode（需要可变引用）
/// - `offset`: 写入偏移
/// - `buf`: 输入缓冲区
///
/// # 返回
/// 成功返回写入的字节数，失败返回错误码
///
/// 实现说明：
/// - 支持扩展文件（分配新块）
/// - 支持追加写入
/// - 只支持直接块（12个块），不支持间接块
pub fn ext4_file_write(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &[u8],
) -> Result<usize, i32> {
    let block_size = fs.block_size as u64;
    let to_write = buf.len() as u64;

    // 计算需要的块数
    let end_offset = offset + to_write;
    let needed_blocks = (end_offset + block_size - 1) / block_size;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;

    // 如果需要新块，进行分配
    if needed_blocks > current_blocks && needed_blocks <= 12 {
        let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);

        // 分配新块
        for i in current_blocks..needed_blocks {
            match allocator.alloc_block() {
                Ok(block_num) => {
                    // 更新 inode 的块指针
                    inode.block[i as usize] = block_num as u32;

                    // 清零新分配的块
                    unsafe {
                        let bh = bio::bread(fs.device, block_num)
                            .ok_or(errno::Errno::IOError.as_neg_i32())?;

                        // 清零整个块
                        for byte in (*bh).b_data.iter_mut() {
                            *byte = 0;
                        }

                        (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                        bio::sync_dirty_buffer(bh)?;
                        bio::brelse(bh);
                    }
                }
                Err(e) => {
                    // 分配失败，回滚已分配的块
                    for j in current_blocks..i {
                        if inode.block[j as usize] != 0 {
                            let _ = allocator.free_block(inode.block[j as usize] as u64);
                            inode.block[j as usize] = 0;
                        }
                    }
                    return Err(e);
                }
            }
        }
    } else if needed_blocks > 12 {
        // 超过直接块数量，暂不支持
        return Err(errno::Errno::FileTooLarge.as_neg_i32());
    }

    // 写入数据
    let mut total_written = 0;
    let mut current_offset = offset;
    let mut buf_offset = 0;

    while total_written < to_write as usize {
        let block_index = current_offset / block_size;
        let block_offset = (current_offset % block_size) as usize;

        if block_index >= 12 {
            break;  // 超过直接块数量
        }

        let block_num = inode.block[block_index as usize] as u64;
        if block_num == 0 {
            return Err(errno::Errno::IOError.as_neg_i32());
        }

        unsafe {
            let bh = bio::bread(fs.device, block_num)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            let remaining = to_write as usize - total_written;
            let available_in_block = block_size as usize - block_offset;
            let write_in_block = core::cmp::min(remaining, available_in_block);

            // 写入数据到块
            data[block_offset..block_offset + write_in_block]
                .copy_from_slice(&buf[buf_offset..buf_offset + write_in_block]);

            // 标记为脏
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::sync_dirty_buffer(bh)?;
            bio::brelse(bh);

            total_written += write_in_block;
            buf_offset += write_in_block;
            current_offset += write_in_block as u64;
        }
    }

    // 更新文件大小
    if end_offset > inode.get_size() {
        inode.set_size(end_offset);
    }

    // TODO: 更新 inode 时间戳
    // TODO: 同步 inode 到磁盘

    Ok(total_written)
}

/// ext4 文件定位
///
/// 对应 Linux 的 ext4_llseek (fs/ext4/file.c)
///
/// # 参数
/// - `inode`: 文件 inode
/// - `offset`: 偏移量
/// - `whence`: 定位方式（0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END）
///
/// # 返回
/// 成功返回新的文件位置，失败返回错误码
pub fn ext4_file_lseek(
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: isize,
    whence: i32,
) -> Result<isize, i32> {
    let file_size = inode.get_size() as isize;

    let new_pos = match whence {
        0 => offset,              // SEEK_SET
        1 => {
            // TODO: 需要跟踪当前文件位置
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

/// ext4 同步文件
///
/// 将文件的所有脏缓冲区同步到磁盘
///
/// # 参数
/// - `fs`: ext4 文件系统实例
/// - `inode`: 文件 inode
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
pub fn ext4_sync_file(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
) -> Result<(), i32> {
    // 同步文件的所有数据块
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
