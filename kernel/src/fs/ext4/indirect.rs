//! ext4 间接块处理
//!
//! 完全遵循 Linux 内核的 ext4 间接块实现
//! 参考: fs/ext4/indirect.c, fs/ext4/inode.c

use crate::errno;
use crate::fs::bio;

/// 每个块可以存储的块号数量（假设块大小 4096，块号 4 字节）
pub const POINTERS_PER_BLOCK: usize = 1024;

/// ext4 块映射层
///
/// 用于追踪多级间接块的访问路径
#[derive(Debug, Clone)]
struct BlockMappingLayer {
    /// 层级索引（0=直接块，1=单级间接，2=二级间接，3=三级间接）
    level: u32,
    /// 块中的偏移量
    offset: usize,
    /// 指向的块号
    block: u64,
    /// 间接块本身的块号（用于分配）
    indirect_block: u64,
}

/// ext4 间接块迭代器
///
/// 用于遍历文件的所有数据块（包括间接块）
pub struct Ext4BlockIterator {
    /// 当前块索引
    current_block: u64,
    /// 总块数
    total_blocks: u64,
}

impl Ext4BlockIterator {
    /// 创建新的块迭代器
    pub fn new(total_blocks: u64) -> Self {
        Self {
            current_block: 0,
            total_blocks,
        }
    }

    /// 获取下一个块的映射信息
    ///
    /// 返回 (层级, 层内偏移)
    pub fn next_mapping(&mut self) -> Option<(u32, usize)> {
        if self.current_block >= self.total_blocks {
            return None;
        }

        let block = self.current_block;
        self.current_block += 1;

        // 直接块（0-11）
        if block < 12 {
            return Some((0, block as usize));
        }

        // 单级间接块（12 - 1035）
        let indirect = block - 12;
        if indirect < POINTERS_PER_BLOCK as u64 {
            return Some((1, indirect as usize));
        }

        // 二级间接块（1036 - 1048603）
        let double = indirect - POINTERS_PER_BLOCK as u64;
        if double < (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK) as u64 {
            let first = double as usize / POINTERS_PER_BLOCK;
            let second = double as usize % POINTERS_PER_BLOCK;
            // 返回 (2, (first, second)) 但我们需要分开处理
            return Some((2, double as usize));
        }

        // 三级间接块
        let triple = double - (POINTERS_PER_BLOCK * POINTERS_PER_BLOCK) as u64;
        Some((3, triple as usize))
    }
}

/// 从 inode 的 block 数组中获取指定块号的数据块
///
/// # 参数
/// - `fs`: ext4 文件系统
/// - `block_array`: inode 的 i_block 数组（15个元素）
/// - `block_index`: 文件中的块索引（从0开始）
///
/// # 返回
/// 成功返回物理块号，失败返回错误码
///
/// # 块布局
/// - i_block[0-11]: 直接块（块 0-11）
/// - i_block[12]: 单级间接块（块 12-1035）
/// - i_block[13]: 二级间接块（块 1036-1048603）
/// - i_block[14]: 三级间接块（块 1048603+）
pub fn ext4_get_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    block_array: &[u32; 15],
    block_index: u64,
) -> Result<u64, i32> {
    let block_size = fs.block_size as u64;

    // 直接块（0-11）
    if block_index < 12 {
        let block_num = block_array[block_index as usize];
        if block_num == 0 {
            return Ok(0);  // 稀疏文件，块未分配
        }
        return Ok(block_num as u64);
    }

    // 单级间接块
    let indirect_offset = block_index - 12;
    let pointers_per_block = block_size / 4;

    if indirect_offset < pointers_per_block {
        // 单级间接块在 i_block[12]
        let indirect_block = block_array[12];
        if indirect_block == 0 {
            return Ok(0);  // 未分配
        }
        return read_indirect_block(fs, indirect_block as u64, indirect_offset as usize);
    }

    // 二级间接块
    let double_offset = indirect_offset - pointers_per_block;
    let double_pointers = pointers_per_block * pointers_per_block;

    if double_offset < double_pointers {
        // 二级间接块在 i_block[13]
        let double_block = block_array[13];
        if double_block == 0 {
            return Ok(0);
        }

        // 第一级：获取单级间接块号
        let first_index = (double_offset / pointers_per_block) as usize;
        let indirect_block = read_indirect_block(fs, double_block as u64, first_index)?;

        if indirect_block == 0 {
            return Ok(0);
        }

        // 第二级：获取数据块号
        let second_index = (double_offset % pointers_per_block) as usize;
        return read_indirect_block(fs, indirect_block, second_index);
    }

    // 三级间接块
    let triple_offset = double_offset - double_pointers;

    // 三级间接块在 i_block[14]
    let triple_block = block_array[14];
    if triple_block == 0 {
        return Ok(0);
    }

    // 第一级：获取二级间接块号
    let first_index = (triple_offset / double_pointers) as usize;
    let double_block = read_indirect_block(fs, triple_block as u64, first_index)?;

    if double_block == 0 {
        return Ok(0);
    }

    // 第二级：获取单级间接块号
    let remaining = triple_offset % double_pointers;
    let second_index = (remaining / pointers_per_block) as usize;
    let indirect_block = read_indirect_block(fs, double_block, second_index)?;

    if indirect_block == 0 {
        return Ok(0);
    }

    // 第三级：获取数据块号
    let third_index = (remaining % pointers_per_block) as usize;
    read_indirect_block(fs, indirect_block, third_index)
}

/// 读取间接块中的指定索引的块号
///
/// # 参数
/// - `fs`: ext4 文件系统
/// - `indirect_block`: 间接块的物理块号
/// - `index`: 块号索引
///
/// # 返回
/// 成功返回数据块号，失败返回错误码
pub fn read_indirect_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    indirect_block: u64,
    index: usize,
) -> Result<u64, i32> {
    unsafe {
        let bh = bio::bread(fs.device, indirect_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &(*bh).b_data;
        let block_numbers = reinterpret_slice::<u32>(data);

        if index >= block_numbers.len() {
            bio::brelse(bh);
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        let block_num = block_numbers[index] as u64;

        bio::brelse(bh);
        Ok(block_num)
    }
}

/// 将块号写入间接块的指定索引
///
/// # 参数
/// - `fs`: ext4 文件系统
/// - `indirect_block`: 间接块的物理块号
/// - `index`: 块号索引
/// - `block_num`: 要写入的数据块号
///
/// # 返回
/// 成功返回 Ok(())，失败返回错误码
pub fn write_indirect_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    indirect_block: u64,
    index: usize,
    block_num: u32,
) -> Result<(), i32> {
    unsafe {
        let bh = bio::bread(fs.device, indirect_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = &mut (*bh).b_data;
        let block_numbers = reinterpret_slice_mut::<u32>(data);

        if index >= block_numbers.len() {
            bio::brelse(bh);
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        block_numbers[index] = block_num;

        (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
        bio::sync_dirty_buffer(bh)?;
        bio::brelse(bh);
        Ok(())
    }
}

/// 获取文件最大支持的大小（基于块大小）
///
/// # 参数
/// - `block_size`: 块大小（字节）
///
/// # 返回
/// 最大文件大小（字节）
pub fn max_file_size(block_size: u64) -> u64 {
    let pointers_per_block = block_size / 4;

    // 直接块
    let direct = 12 * block_size;

    // 单级间接块
    let single = pointers_per_block * block_size;

    // 二级间接块
    let double = pointers_per_block * pointers_per_block * block_size;

    // 三级间接块
    let triple = pointers_per_block * pointers_per_block * pointers_per_block * block_size;

    direct + single + double + triple
}

/// 检查文件大小是否需要间接块
///
/// # 参数
/// - `size`: 文件大小（字节）
/// - `block_size`: 块大小（字节）
///
/// # 返回
/// 返回需要的间接块级别：
/// - 0: 只需要直接块
/// - 1: 需要单级间接块
/// - 2: 需要二级间接块
/// - 3: 需要三级间接块
pub fn get_indirect_level(size: u64, block_size: u64) -> u32 {
    let blocks = (size + block_size - 1) / block_size;

    if blocks <= 12 {
        return 0;
    }

    let pointers_per_block = block_size / 4;

    if blocks <= 12 + pointers_per_block {
        return 1;
    }

    let double_pointers = pointers_per_block * pointers_per_block;

    if blocks <= 12 + pointers_per_block + double_pointers {
        return 2;
    }

    3
}

/// 重新解释切片为 u32 类型
///
/// # Safety
/// 调用者必须确保：
/// - 数据对齐正确（u32 需要 4 字节对齐）
/// - 数据长度足够
unsafe fn reinterpret_slice<T>(data: &[u8]) -> &[T] {
    core::slice::from_raw_parts(
        data.as_ptr() as *const T,
        data.len() / core::mem::size_of::<T>(),
    )
}

/// 重新解释可变切片为 u32 类型
///
/// # Safety
/// 调用者必须确保：
/// - 数据对齐正确（u32 需要 4 字节对齐）
/// - 数据长度足够
unsafe fn reinterpret_slice_mut<T>(data: &mut [u8]) -> &mut [T] {
    core::slice::from_raw_parts_mut(
        data.as_ptr() as *mut T,
        data.len() / core::mem::size_of::<T>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_file_size() {
        let block_size = 4096u64;
        let max_size = max_file_size(block_size);

        // 应该能支持超过 4TB 的文件
        assert!(max_size > 4_000_000_000_000);
    }

    #[test]
    fn test_indirect_level() {
        let block_size = 4096u64;

        // 小文件：只用直接块
        assert_eq!(get_indirect_level(48 * 1024, block_size), 0);

        // 中等文件：需要单级间接块
        assert_eq!(get_indirect_level(100 * 1024, block_size), 1);
        assert_eq!(get_indirect_level(4 * 1024 * 1024, block_size), 1);

        // 大文件：需要二级间接块
        assert_eq!(get_indirect_level(5 * 1024 * 1024, block_size), 2);
        assert_eq!(get_indirect_level(4 * 1024 * 1024 * 1024, block_size), 2);

        // 超大文件：需要三级间接块
        assert_eq!(get_indirect_level(5 * 1024 * 1024 * 1024u64, block_size), 3);
    }

    #[test]
    fn test_block_iterator() {
        let mut iter = Ext4BlockIterator::new(20);

        // 前 12 个应该是直接块
        for i in 0..12 {
            let (level, offset) = iter.next_mapping().unwrap();
            assert_eq!(level, 0);
            assert_eq!(offset, i as usize);
        }

        // 接下来 8 个应该是单级间接块
        for i in 0..8 {
            let (level, offset) = iter.next_mapping().unwrap();
            assert_eq!(level, 1);
            assert_eq!(offset, i as usize);
        }

        // 第 21 个应该返回 None
        assert!(iter.next_mapping().is_none());
    }
}
