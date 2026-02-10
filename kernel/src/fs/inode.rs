//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 索引节点 (Inode) 管理
//!
//! 完全遵循 Linux 内核的 inode 设计 (fs/inode.c, include/linux/fs.h)
//!
//! 核心概念：
//! - `struct inode`: 索引节点，表示文件系统中的一个对象
//! - `struct super_block`: 超级块，表示一个文件系统
//! - `struct inode_operations`: inode 操作函数指针

use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::buffer::FileBuffer;

/// Inode 编号类型
pub type Ino = u64;

/// Inode 模式 (文件类型和权限)
///
/// 对应 Linux 的 i_mode 字段 (include/linux/fs.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct InodeMode(u32);

impl InodeMode {
    /// 文件类型掩码
    pub const S_IFMT: u32 = 0o0170000;

    /// 常规文件
    pub const S_IFREG: u32 = 0o0100000;
    /// 目录
    pub const S_IFDIR: u32 = 0o0040000;
    /// 字符设备
    pub const S_IFCHR: u32 = 0o0020000;
    /// 块设备
    pub const S_IFBLK: u32 = 0o0060000;
    /// FIFO (命名管道)
    pub const S_IFIFO: u32 = 0o0010000;
    /// 符号链接
    pub const S_IFLNK: u32 = 0o0120000;
    /// Socket
    pub const S_IFSOCK: u32 = 0o0140000;

    /// 权限位
    pub const S_IRWXU: u32 = 0o0700;  // 用户权限
    pub const S_IRUSR: u32 = 0o0400;  // 用户读
    pub const S_IWUSR: u32 = 0o0200;  // 用户写
    pub const S_IXUSR: u32 = 0o0100;  // 用户执行
    pub const S_IRWXG: u32 = 0o0070;  // 组权限
    pub const S_IRGRP: u32 = 0o0040;  // 组读
    pub const S_IWGRP: u32 = 0o0020;  // 组写
    pub const S_IXGRP: u32 = 0o0010;  // 组执行
    pub const S_IRWXO: u32 = 0o0007;  // 其他权限
    pub const S_IROTH: u32 = 0o0004;  // 其他读
    pub const S_IWOTH: u32 = 0o0002;  // 其他写
    pub const S_IXOTH: u32 = 0o0001;  // 其他执行

    pub fn new(mode: u32) -> Self {
        Self(mode)
    }

    pub fn is_regular_file(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFREG
    }

    pub fn is_directory(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFDIR
    }

    pub fn is_char_device(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFCHR
    }

    pub fn is_block_device(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFBLK
    }

    pub fn is_fifo(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFIFO
    }

    pub fn is_symlink(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFLNK
    }

    pub fn is_socket(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFSOCK
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Inode 操作函数指针表
///
/// 对应 Linux 的 struct inode_operations (include/linux/fs.h)
#[repr(C)]
pub struct INodeOps {
    /// 创建新节点
    pub mkdir: Option<unsafe fn(&mut Inode, &[u8]) -> i32>,
    /// 查找节点
    pub lookup: Option<unsafe fn(&mut Inode, &[u8]) -> Option<*mut Inode>>,
    /// 创建链接
    pub link: Option<unsafe fn(&mut Inode, &mut Inode, &[u8]) -> i32>,
    /// 删除链接
    pub unlink: Option<unsafe fn(&mut Inode, &[u8]) -> i32>,
    /// 创建符号链接
    pub symlink: Option<unsafe fn(&mut Inode, &[u8], &[u8]) -> i32>,
    /// 创建目录
    pub mkdir2: Option<unsafe fn(&mut Inode, &[u8], InodeMode) -> i32>,
    /// 删除目录
    pub rmdir: Option<unsafe fn(&mut Inode, &[u8]) -> i32>,
    /// 重命名
    pub rename: Option<unsafe fn(&mut Inode, &mut Inode, &[u8], &[u8]) -> i32>,
    /// 读取链接
    pub readlink: Option<unsafe fn(&mut Inode, &mut [u8]) -> isize>,
}

/// Inode 状态
///
/// 对应 Linux 的 i_state (include/linux/fs.h)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum InodeState {
    /// 新分配的 inode
    INew = 0,
    /// Inode 已存在
    IExisting = 1,
    /// Inode 正在删除
    IDying = 2,
}

/// 索引节点
///
/// 对应 Linux 的 struct inode (include/linux/fs.h)
#[repr(C)]
pub struct Inode {
    /// Inode 编号
    pub ino: Ino,
    /// Inode 模式 (文件类型和权限)
    pub mode: InodeMode,
    /// 文件大小
    pub size: AtomicU64,
    /// 设备号
    pub rdev: u64,
    /// Inode 状态
    pub state: Mutex<InodeState>,
    /// Inode 操作
    pub ops: Option<&'static INodeOps>,
    /// 私有数据
    pub private_data: Option<*mut u8>,
    /// 文件数据（常规文件使用）
    pub data: Mutex<Option<FileBuffer>>,
    /// 引用计数
    ref_count: AtomicU64,
}

unsafe impl Send for Inode {}
unsafe impl Sync for Inode {}

impl Inode {
    /// 创建新的 inode
    pub fn new(ino: Ino, mode: InodeMode) -> Self {
        Self {
            ino,
            mode,
            size: AtomicU64::new(0),
            rdev: 0,
            state: Mutex::new(InodeState::INew),
            ops: None,
            private_data: None,
            data: Mutex::new(None),
            ref_count: AtomicU64::new(1),
        }
    }

    /// 读取文件数据
    pub fn read_data(&self, offset: usize, buf: &mut [u8]) -> usize {
        if let Some(ref data) = *self.data.lock() {
            data.read(offset, buf)
        } else {
            0
        }
    }

    /// 写入文件数据
    pub fn write_data(&self, offset: usize, buf: &[u8]) -> usize {
        let mut data_guard = self.data.lock();
        if data_guard.is_none() {
            *data_guard = Some(FileBuffer::new());
        }
        if let Some(ref mut data) = *data_guard {
            let written = data.write(offset, buf);
            // 更新文件大小
            let new_size = data.len() as u64;
            self.size.store(new_size, Ordering::Release);
            written
        } else {
            0
        }
    }

    /// 从字节数据加载文件内容
    pub fn load_from_bytes(&self, bytes: &[u8]) {
        let mut data_guard = self.data.lock();
        *data_guard = Some(FileBuffer::from_bytes(bytes));
        self.size.store(bytes.len() as u64, Ordering::Release);
    }

    /// 设置 inode 操作
    pub fn set_ops(&mut self, ops: &'static INodeOps) {
        self.ops = Some(ops);
    }

    /// 设置私有数据
    pub fn set_private_data(&mut self, data: *mut u8) {
        self.private_data = Some(data);
    }

    /// 获取文件大小
    pub fn get_size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    /// 设置文件大小
    pub fn set_size(&self, size: u64) {
        self.size.store(size, Ordering::Release);
    }

    /// 增加引用计数
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn dec_ref(&self) -> u64 {
        self.ref_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 获取引用计数
    pub fn get_ref(&self) -> u64 {
        self.ref_count.load(Ordering::Acquire)
    }
}

/// 创建字符设备 inode
pub fn make_char_inode(ino: Ino, rdev: u64) -> Inode {
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFCHR | 0o666));
    inode.rdev = rdev;
    inode
}

/// 创建常规文件 inode
pub fn make_reg_inode(ino: Ino, size: u64) -> Inode {
    let inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o666));
    inode.set_size(size);
    inode
}

/// 创建带数据的常规文件 inode
pub fn make_reg_inode_with_data(ino: Ino, data: &[u8]) -> Inode {
    let inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o666));
    inode.load_from_bytes(data);
    inode
}

/// 创建目录 inode
pub fn make_dir_inode(ino: Ino) -> Inode {
    Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o755))
}

/// 创建 FIFO inode
pub fn make_fifo_inode(ino: Ino) -> Inode {
    Inode::new(ino, InodeMode::new(InodeMode::S_IFIFO | 0o666))
}

// ============================================================================
// Inode 缓存 (inode cache)
// ============================================================================

/// Inode 缓存大小
const ICACHE_SIZE: usize = 256;

/// Inode 缓存统计信息
#[derive(Debug)]
pub struct InodeCacheStats {
    /// 缓存命中次数
    pub hits: AtomicU64,
    /// 缓存未命中次数
    pub misses: AtomicU64,
    /// 淘汰次数
    pub evictions: AtomicU64,
}

impl InodeCacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            (hits as f64) / (total as f64)
        }
    }
}

/// 哈希表桶
struct InodeHashBucket {
    /// inode 指针
    inode: Option<Arc<Inode>>,
    /// inode 编号（用于快速比较）
    ino: Ino,
    /// LRU 时间戳（用于淘汰）
    access_time: AtomicU64,
}

impl Clone for InodeHashBucket {
    fn clone(&self) -> Self {
        Self {
            inode: self.inode.clone(),
            ino: self.ino,
            access_time: AtomicU64::new(self.access_time.load(Ordering::Relaxed)),
        }
    }
}

/// Inode 哈希表
struct InodeCache {
    /// 哈希表
    buckets: [InodeHashBucket; ICACHE_SIZE],
    /// 缓存中的条目数量
    count: usize,
    /// 全局时间戳（用于 LRU）
    global_time: AtomicU64,
    /// 统计信息
    stats: InodeCacheStats,
}

unsafe impl Send for InodeCache {}
unsafe impl Sync for InodeCache {}

/// 全局 Inode 缓存
static ICACHE: spin::Mutex<Option<InodeCache>> = spin::Mutex::new(None);

/// 初始化 Inode 缓存
fn icache_init() {
    let mut cache = ICACHE.lock();
    if cache.is_some() {
        return;  // 已经初始化
    }

    // 创建空桶数组
    let buckets: [InodeHashBucket; ICACHE_SIZE] = core::array::from_fn(|_| InodeHashBucket {
        inode: None,
        ino: 0,
        access_time: AtomicU64::new(0),
    });

    *cache = Some(InodeCache {
        buckets,
        count: 0,
        global_time: AtomicU64::new(1),
        stats: InodeCacheStats::new(),
    });
}

/// 计算哈希值
///
/// 使用简单的 FNV-1a 哈希算法
fn inode_hash(ino: Ino) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis

    // 混合 inode 编号
    hash ^= ino;
    hash = hash.wrapping_mul(0x100000001b3);

    hash
}

/// 在 Inode 缓存中查找
///
/// 对应 Linux 内核的 ilookup() (fs/inode.c)
pub fn icache_lookup(ino: Ino) -> Option<Arc<Inode>> {
    // 确保缓存已初始化
    icache_init();

    let mut cache = ICACHE.lock();
    let cache_inner = cache.as_mut()?;

    // 计算哈希值
    let hash = inode_hash(ino);
    let index = (hash as usize) % ICACHE_SIZE;

    // 查找匹配的条目
    let bucket = &cache_inner.buckets[index];

    if let Some(ref inode) = bucket.inode {
        // 比较 inode 编号
        if bucket.ino == ino {
            // 更新访问时间（用于 LRU）
            let current_time = cache_inner.global_time.fetch_add(1, Ordering::Relaxed);
            bucket.access_time.store(current_time, Ordering::Relaxed);

            // 记录命中
            cache_inner.stats.record_hit();

            return Some(inode.clone());
        }
    }

    // 记录未命中
    cache_inner.stats.record_miss();

    None
}

/// 将 Inode 添加到缓存
///
/// 对应 Linux 内核的 inode_insert5() (fs/inode.c)
pub fn icache_add(inode: Arc<Inode>) {
    // 确保缓存已初始化
    icache_init();

    // 获取 inode 编号（在缓存锁外进行）
    let ino = inode.ino;

    // 计算哈希值
    let hash = inode_hash(ino);

    let mut cache = ICACHE.lock();
    let inner = cache.as_mut().expect("icache not initialized");

    let index = (hash as usize) % ICACHE_SIZE;

    // 检查是否已存在
    if let Some(ref _existing) = inner.buckets[index].inode {
        if inner.buckets[index].ino == ino {
            return;  // 已经在缓存中
        }

        // 使用 LRU 策略：查找并淘汰最久未使用的条目
        icache_evict_lru(inner);
    }

    // 获取当前时间戳
    let current_time = inner.global_time.fetch_add(1, Ordering::Relaxed);

    // 添加到缓存
    inner.buckets[index] = InodeHashBucket {
        inode: Some(inode.clone()),
        ino,
        access_time: AtomicU64::new(current_time),
    };
    inner.count += 1;
}

/// LRU 淘汰策略：淘汰最久未使用的条目
///
/// 对应 Linux 内核的 prune_icache() (fs/inode.c)
fn icache_evict_lru(cache: &mut InodeCache) {
    // 查找最久未使用的条目（最小访问时间）
    let mut lru_index = 0;
    let mut lru_time = u64::MAX;
    let mut found = false;

    for (i, bucket) in cache.buckets.iter().enumerate() {
        if bucket.inode.is_some() {
            let access_time = bucket.access_time.load(Ordering::Relaxed);
            if access_time < lru_time {
                lru_time = access_time;
                lru_index = i;
                found = true;
            }
        }
    }

    // 淘汰 LRU 条目
    if found {
        cache.buckets[lru_index].inode = None;
        cache.buckets[lru_index].ino = 0;
        cache.buckets[lru_index].access_time.store(0, Ordering::Relaxed);
        cache.count -= 1;

        // 记录淘汰
        cache.stats.record_eviction();
    }
}

/// 从 Inode 缓存中删除
///
/// 对应 Linux 内核的 iput() 的缓存清理部分 (fs/inode.c)
pub fn icache_remove(ino: Ino) {
    // 确保缓存已初始化
    icache_init();

    let mut cache = ICACHE.lock();
    let inner = cache.as_mut().expect("icache not initialized");

    // 计算哈希值
    let hash = inode_hash(ino);
    let index = (hash as usize) % ICACHE_SIZE;

    // 删除条目
    if let Some(ref _inode) = inner.buckets[index].inode {
        if inner.buckets[index].ino == ino {
            // 从缓存中移除
            inner.buckets[index].inode = None;
            inner.buckets[index].ino = 0;
            inner.buckets[index].access_time.store(0, Ordering::Relaxed);
            inner.count -= 1;
        }
    }
}

/// 获取缓存统计信息
pub fn icache_stats() -> (usize, usize) {
    // 确保缓存已初始化
    icache_init();

    let cache = ICACHE.lock();
    let cache_inner = cache.as_ref().expect("icache not initialized");

    (cache_inner.count, ICACHE_SIZE)
}

/// 获取详细的缓存统计信息
pub fn icache_stats_detailed() -> (u64, u64, u64, f64) {
    // 确保缓存已初始化
    icache_init();

    let cache = ICACHE.lock();
    let cache_inner = cache.as_ref().expect("icache not initialized");

    (
        cache_inner.stats.hits.load(Ordering::Relaxed),
        cache_inner.stats.misses.load(Ordering::Relaxed),
        cache_inner.stats.evictions.load(Ordering::Relaxed),
        cache_inner.stats.get_hit_rate(),
    )
}

/// 清空 Inode 缓存
///
/// 对应 Linux 内核的 invalidate_inodes() (fs/inode.c)
pub fn icache_flush() {
    // 确保缓存已初始化
    icache_init();

    let mut cache = ICACHE.lock();
    let inner = cache.as_mut().expect("icache not initialized");

    // 清空所有桶
    for bucket in inner.buckets.iter_mut() {
        bucket.inode = None;
        bucket.ino = 0;
        bucket.access_time.store(0, Ordering::Relaxed);
    }

    inner.count = 0;
}
