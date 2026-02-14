//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buffer I/O 层 - 块缓存管理
//!
//! 完全遵循 Linux 内核的 buffer I/O 设计 (fs/buffer.c, include/linux/buffer_head.h)
//!
//! 核心概念：
//! - `struct buffer_head`: 缓冲区头，表示一个被缓存的块
//! - 块缓存：缓存磁盘块以提高性能
//! - 哈希表：快速查找已缓存的块

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::drivers::blkdev;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct BufferState(u8);

impl BufferState {
    pub const BH_Uptodate: u8 = 0;  // Buffer contains valid data
    pub const BH_Dirty: u8 = 1;     // Buffer needs to be written to disk
    pub const BH_Lock: u8 = 2;      // Buffer is locked
    pub const BH_Req: u8 = 3;       // Buffer has been requested
    pub const BH_Mapped: u8 = 4;    // Buffer is mapped to a disk block

    pub fn new() -> Self {
        Self(0)
    }

    pub fn set(&mut self, bit: u8) {
        self.0 |= 1 << bit;
    }

    pub fn clear(&mut self, bit: u8) {
        self.0 &= !(1 << bit);
    }

    pub fn test(&self, bit: u8) -> bool {
        (self.0 & (1 << bit)) != 0
    }

    pub fn is_uptodate(&self) -> bool {
        self.test(Self::BH_Uptodate)
    }

    pub fn is_dirty(&self) -> bool {
        self.test(Self::BH_Dirty)
    }

    pub fn is_locked(&self) -> bool {
        self.test(Self::BH_Lock)
    }

    pub fn is_mapped(&self) -> bool {
        self.test(Self::BH_Mapped)
    }
}

pub struct BufferHead {
    /// 块设备
    pub b_device: Option<*const blkdev::GenDisk>,
    /// 块号
    pub b_blocknr: u64,
    /// 块大小
    pub b_size: u32,
    /// 缓冲区状态
    pub b_state: Mutex<BufferState>,
    /// 数据
    pub b_data: Vec<u8>,
    /// 引用计数
    b_count: AtomicU32,
}

unsafe impl Send for BufferHead {}
unsafe impl Sync for BufferHead {}

impl BufferHead {
    /// 创建新的缓冲区头
    pub fn new(blocknr: u64, size: u32) -> Self {
        Self {
            b_device: None,
            b_blocknr: blocknr,
            b_size: size,
            b_state: Mutex::new(BufferState::new()),
            b_data: vec![0u8; size as usize],
            b_count: AtomicU32::new(1),
        }
    }

    /// 设置块设备
    pub fn set_device(&mut self, device: *const blkdev::GenDisk) {
        self.b_device = Some(device);
        let mut state = self.b_state.lock();
        state.set(BufferState::BH_Mapped);
    }

    /// 获取状态
    pub fn get_state(&self) -> BufferState {
        let state = self.b_state.lock();
        *state
    }

    /// 设置状态位
    pub fn set_state_bit(&self, bit: u8) {
        let mut state = self.b_state.lock();
        state.set(bit);
    }

    /// 清除状态位
    pub fn clear_state_bit(&self, bit: u8) {
        let mut state = self.b_state.lock();
        state.clear(bit);
    }

    /// 检查是否是脏
    pub fn is_dirty(&self) -> bool {
        let state = self.b_state.lock();
        state.is_dirty()
    }

    /// 增加引用计数
    pub fn get(&self) {
        self.b_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn put(&self) -> u32 {
        self.b_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 读取数据
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.b_size as usize {
            return 0;
        }
        let available = self.b_size as usize - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.b_data[offset..offset + to_read]);
        to_read
    }

    /// 写入数据
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if offset >= self.b_data.len() {
            return 0;
        }
        let available = self.b_data.len() - offset;
        let to_write = core::cmp::min(buf.len(), available);
        self.b_data[offset..offset + to_write].copy_from_slice(&buf[..to_write]);
        self.set_state_bit(BufferState::BH_Dirty);
        to_write
    }

    /// 同步到磁盘
    pub fn sync(&self) -> Result<(), i32> {
        if !self.is_dirty() {
            return Ok(());
        }

        if let Some(device) = self.b_device {
            blkdev::blkdev_write(
                device,
                self.b_blocknr * (self.b_size as u64 / 512),
                &self.b_data,
            )?;
            self.clear_state_bit(BufferState::BH_Dirty);
            Ok(())
        } else {
            Err(-6)  // ENXIO
        }
    }
}

struct BlockCache {
    /// 缓冲区哈希表
    /// 索引: (设备主设备号, 块号) % 哈希表大小
    buffers: Mutex<Vec<Option<*mut BufferHead>>>,
    /// 哈希表大小（必须是 2 的幂）
    hash_size: usize,
    /// 缓冲区大小
    block_size: u32,
}

unsafe impl Send for BlockCache {}
unsafe impl Sync for BlockCache {}

impl BlockCache {
    /// 创建新的块缓存
    fn new(hash_size: usize, block_size: u32) -> Self {
        // 使用裸指针初始化，避免需要 Clone trait
        let mut vec = Vec::with_capacity(hash_size);
        for _ in 0..hash_size {
            vec.push(None);
        }

        Self {
            buffers: Mutex::new(vec),
            hash_size,
            block_size,
        }
    }

    /// 计算哈希索引
    fn hash_index(&self, device_major: u32, blocknr: u64) -> usize {
        // 使用简单的哈希函数
        let hash = (device_major as u64).wrapping_mul(31).wrapping_add(blocknr);
        (hash as usize) & (self.hash_size - 1)
    }

    /// 查找缓冲区
    fn lookup(&self, device_major: u32, blocknr: u64) -> Option<*const BufferHead> {
        let index = self.hash_index(device_major, blocknr);
        let buffers = self.buffers.lock();

        if let Some(bh_ptr) = buffers[index] {
            unsafe {
                let bh = &*bh_ptr;
                if bh.b_blocknr == blocknr {
                    if let Some(device) = bh.b_device {
                        if (*device).major == device_major {
                            return Some(bh_ptr);
                        }
                    }
                }
            }
        }

        None
    }

    /// 获取或创建缓冲区
    fn get(&self, device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
        unsafe {
            let device_major = (*device).major;

            // 首先尝试查找已存在的缓冲区
            if let Some(bh) = self.lookup(device_major, blocknr) {
                let bh_ref = &*bh;
                bh_ref.get();
                return Some(bh as *mut u8 as *mut BufferHead);
            }

            // 创建新缓冲区
            let bh = Box::new(BufferHead::new(blocknr, self.block_size));

            // 从磁盘读取数据
            let mut bh_owned = bh;
            if let Err(_e) = blkdev::blkdev_read(
                device,
                blocknr * (self.block_size as u64 / 512),
                &mut bh_owned.b_data,
            ) {
                return None;
            }

            bh_owned.set_device(device);
            bh_owned.set_state_bit(BufferState::BH_Uptodate);

            // 转换为裸指针并泄漏
            let bh_ptr = Box::leak(bh_owned);

            // 插入到哈希表
            let index = self.hash_index(device_major, blocknr);
            let mut buffers = self.buffers.lock();
            buffers[index] = Some(bh_ptr);

            Some(bh_ptr)
        }
    }

    /// 释放缓冲区
    fn put(&self, _bh: *const BufferHead) {
        // 简化实现：不真正释放
        // 在完整实现中，应该减少引用计数，并在计数为 0 时回收
    }

    /// 同步所有脏缓冲区
    fn sync_all(&self) -> Result<(), i32> {
        let buffers = self.buffers.lock();

        for bh_opt in buffers.iter() {
            if let Some(bh_ptr) = *bh_opt {
                unsafe {
                    let bh = &*bh_ptr;
                    if bh.is_dirty() {
                        bh.sync()?;
                    }
                }
            }
        }

        Ok(())
    }

    /// 释放所有缓冲区
    fn invalidate(&self) {
        let mut buffers = self.buffers.lock();

        for i in 0..buffers.len() {
            if let Some(bh_ptr) = buffers[i] {
                unsafe {
                    // 重新获取所有权并释放
                    let _ = Box::from_raw(bh_ptr);
                }
                buffers[i] = None;
            }
        }
    }
}

// 使用 lazy_static 风格的初始化
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

static CACHE_INIT: AtomicBool = AtomicBool::new(false);
static mut BLOCK_CACHE: Option<BlockCache> = None;

fn get_block_cache() -> &'static BlockCache {
    unsafe {
        if !CACHE_INIT.load(AtomicOrdering::Acquire) {
            // 使用 16 个条目的缓存（64KB）
            BLOCK_CACHE = Some(BlockCache::new(16, 4096));
            CACHE_INIT.store(true, AtomicOrdering::Release);
        }
        BLOCK_CACHE.as_ref().unwrap()
    }
}

pub fn bread(device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
    get_block_cache().get(device, blocknr)
}

pub fn brelse(bh: *const BufferHead) {
    get_block_cache().put(bh)
}

pub fn sync_dirty_buffer(bh: *const BufferHead) -> Result<(), i32> {
    unsafe {
        let bh_ref = &*bh;
        bh_ref.sync()
    }
}

pub fn sync_buffers() -> Result<(), i32> {
    get_block_cache().sync_all()
}

pub fn init() {
    // 缓存会在第一次使用时自动初始化（懒加载模式）
    // 不在这里初始化，避免启动时分配过多内存导致 panic
}
