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
/// - `count`: 要写入的字节数
///
/// # 返回
/// 成功返回写入的字节数，失败返回错误码
///
/// 注意：当前实现为只读，不支持写入
pub fn ext4_file_write(
    _fs: &crate::fs::ext4::Ext4FileSystem,
    _inode: &mut crate::fs::ext4::inode::Ext4Inode,
    _offset: u64,
    _buf: &[u8],
) -> Result<usize, i32> {
    // TODO: 实现写入功能
    // 需要：
    // 1. 分配新的数据块
    // 2. 更新 inode 的块指针
    // 3. 写入数据到块
    // 4. 更新 inode 的时间戳
    // 5. 同步 inode 到磁盘

    Err(errno::Errno::ReadOnlyFileSystem.as_neg_i32())
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
