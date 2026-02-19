//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 目录项 (Dentry) 管理
//!
//!
//! 核心概念：
//! - `struct dentry`: 目录项，表示目录中的一个条目
//! - `dcache`: 目录项缓存，加速路径查找
//! - `LRU`: 最近最少使用淘汰策略

use alloc::sync::Arc;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::inode::Inode;

/// Dentry 状态标志
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DentryFlags(u32);

impl DentryFlags {
    /// 目录项未连接到 dcache
    pub const DCACHE_UNHASHED: u32 = 0x00000001;
    /// 目录项已连接到 dcache
    pub const DCACHE_HASHED: u32 = 0x00000002;
    /// 目录项正在使用中
    pub const DCACHE_REFERENCED: u32 = 0x00000010;
    /// 目录项已删除
    pub const DCACHE_DENTRY_KILL: u32 = 0x00000040;

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_hashed(&self) -> bool {
        (self.0 & Self::DCACHE_HASHED) != 0
    }

    pub fn is_unhashed(&self) -> bool {
        (self.0 & Self::DCACHE_UNHASHED) != 0
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Dentry 状态
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DentryState {
    /// Dentry 未使用
    DUnhashed,
    /// Dentry 已使用
    DHashed,
    /// Dentry 正在删除
    DKill,
}

/// 目录项
///
#[repr(C)]
pub struct Dentry {
    /// dentry 名称
    pub name: Mutex<String>,
    /// 父目录项
    pub parent: Mutex<Option<Arc<Dentry>>>,
    /// 关联的 inode
    pub inode: Mutex<Option<Arc<Inode>>>,
    /// dentry 状态
    pub state: Mutex<DentryState>,
    /// dentry 标志
    pub flags: Mutex<DentryFlags>,
    /// 引用计数
    ref_count: AtomicU64,
}

unsafe impl Send for Dentry {}
unsafe impl Sync for Dentry {}

impl Dentry {
    /// 创建新的 dentry
    pub fn new(name: String) -> Self {
        Self {
            name: Mutex::new(name),
            parent: Mutex::new(None),
            inode: Mutex::new(None),
            state: Mutex::new(DentryState::DUnhashed),
            flags: Mutex::new(DentryFlags::new(DentryFlags::DCACHE_UNHASHED)),
            ref_count: AtomicU64::new(1),
        }
    }

    /// 设置父目录项
    pub fn set_parent(&self, parent: Arc<Dentry>) {
        *self.parent.lock() = Some(parent);
    }

    /// 设置 inode
    pub fn set_inode(&self, inode: Arc<Inode>) {
        *self.inode.lock() = Some(inode);
    }

    /// 获取 inode
    pub fn get_inode(&self) -> Option<Arc<Inode>> {
        // Arc 已经实现了 Clone trait (标准库)
        // 暂时返回 None，需要实现实际的 inode 关联逻辑
        None
    }

    /// 获取名称
    pub fn get_name(&self) -> String {
        self.name.lock().clone()
    }

    /// 设置为已哈希状态
    pub fn set_hashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_HASHED);
        *self.state.lock() = DentryState::DHashed;
    }

    /// 设置为未哈希状态
    pub fn set_unhashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_UNHASHED);
        *self.state.lock() = DentryState::DUnhashed;
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

/// 创建根目录项
pub fn make_root_dentry() -> Option<Arc<Dentry>> {
    let dentry = Arc::new(Dentry::new("/".to_owned()));
    // Note: Arc returns &T when dereferenced
    // For now, we'll return the Arc directly - the caller can call set_hashed if needed
    Some(dentry)
}

// ============================================================================
// Dentry 缓存 (dcache)
// ============================================================================

/// Dentry 缓存大小
const DCACHE_SIZE: usize = 256;

/// Dentry 缓存统计信息
#[derive(Debug)]
pub struct DentryCacheStats {
    /// 缓存命中次数
    pub hits: AtomicU64,
    /// 缓存未命中次数
    pub misses: AtomicU64,
    /// 淘汰次数
    pub evictions: AtomicU64,
}

impl DentryCacheStats {
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
struct DentryHashBucket {
    /// dentry 指针
    dentry: Option<Arc<Dentry>>,
    /// 哈希键（用于快速比较）
    key: u64,
    /// LRU 时间戳（用于淘汰）
    access_time: AtomicU64,
}

impl Clone for DentryHashBucket {
    fn clone(&self) -> Self {
        Self {
            dentry: self.dentry.clone(),
            key: self.key,
            access_time: AtomicU64::new(self.access_time.load(Ordering::Relaxed)),
        }
    }
}

/// Dentry 哈希表
struct DentryCache {
    /// 哈希表
    buckets: [DentryHashBucket; DCACHE_SIZE],
    /// 缓存中的条目数量
    count: usize,
    /// 全局时间戳（用于 LRU）
    global_time: AtomicU64,
    /// 统计信息
    stats: DentryCacheStats,
}

unsafe impl Send for DentryCache {}
unsafe impl Sync for DentryCache {}

/// 全局 Dentry 缓存
static DCACHE: spin::Mutex<Option<DentryCache>> = spin::Mutex::new(None);

/// 初始化 Dentry 缓存
fn dcache_init() {
    let mut cache = DCACHE.lock();
    if cache.is_some() {
        return;  // 已经初始化
    }

    // 创建空桶数组
    let buckets: [DentryHashBucket; DCACHE_SIZE] = core::array::from_fn(|_| DentryHashBucket {
        dentry: None,
        key: 0,
        access_time: AtomicU64::new(0),
    });

    *cache = Some(DentryCache {
        buckets,
        count: 0,
        global_time: AtomicU64::new(1),
        stats: DentryCacheStats::new(),
    });
}

/// 计算哈希值
///
/// 使用简单的 FNV-1a 哈希算法
fn dentry_hash(name: &str, parent_ino: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis

    // 混合父 inode 编号
    hash ^= parent_ino;
    hash = hash.wrapping_mul(0x100000001b3);

    // 混合名称
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
}

/// 在 Dentry 缓存中查找
///
pub fn dcache_lookup(name: &str, parent_ino: u64) -> Option<Arc<Dentry>> {
    // 确保缓存已初始化
    dcache_init();

    let mut cache = DCACHE.lock();
    let cache_inner = cache.as_mut()?;

    // 计算哈希值
    let hash = dentry_hash(name, parent_ino);
    let index = (hash as usize) % DCACHE_SIZE;

    // 查找匹配的条目
    let bucket = &cache_inner.buckets[index];

    if let Some(ref dentry) = bucket.dentry {
        // 比较哈希键
        if bucket.key == hash {
            // 比较名称
            if dentry.name.lock().as_str() == name {
                // 更新访问时间（用于 LRU）
                let current_time = cache_inner.global_time.fetch_add(1, Ordering::Relaxed);
                bucket.access_time.store(current_time, Ordering::Relaxed);

                // 记录命中
                cache_inner.stats.record_hit();

                return Some(dentry.clone());
            }
        }
    }

    // 记录未命中
    cache_inner.stats.record_miss();

    None
}

/// 将 Dentry 添加到缓存
///
pub fn dcache_add(dentry: Arc<Dentry>, parent_ino: u64) {
    // 确保缓存已初始化
    dcache_init();

    // 计算哈希值（在缓存锁外进行）
    let name = dentry.name.lock();
    let name_str = name.clone();
    drop(name);  // 释放 name 锁
    let hash = dentry_hash(&name_str, parent_ino);

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    let index = (hash as usize) % DCACHE_SIZE;

    // 检查是否已存在
    if let Some(ref _existing) = inner.buckets[index].dentry {
        if inner.buckets[index].key == hash {
            return;  // 已经在缓存中
        }

        // 使用 LRU 策略：查找并淘汰最久未使用的条目
        dcache_evict_lru(inner);
    }

    // 获取当前时间戳
    let current_time = inner.global_time.fetch_add(1, Ordering::Relaxed);

    // 添加到缓存
    inner.buckets[index] = DentryHashBucket {
        dentry: Some(dentry.clone()),
        key: hash,
        access_time: AtomicU64::new(current_time),
    };
    inner.count += 1;

    // 标记为已哈希（在缓存锁外进行）
    drop(cache);  // 释放缓存锁
    dentry.set_hashed();
}

/// LRU 淘汰策略：淘汰最久未使用的条目
///
fn dcache_evict_lru(cache: &mut DentryCache) {
    // 查找最久未使用的条目（最小访问时间）
    let mut lru_index = 0;
    let mut lru_time = u64::MAX;
    let mut found = false;

    for (i, bucket) in cache.buckets.iter().enumerate() {
        if bucket.dentry.is_some() {
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
        if let Some(ref dentry) = cache.buckets[lru_index].dentry {
            // 标记为未哈希
            dentry.set_unhashed();
        }

        cache.buckets[lru_index].dentry = None;
        cache.buckets[lru_index].key = 0;
        cache.buckets[lru_index].access_time.store(0, Ordering::Relaxed);
        cache.count -= 1;

        // 记录淘汰
        cache.stats.record_eviction();
    }
}

/// 从 Dentry 缓存中删除
///
pub fn dcache_remove(name: &str, parent_ino: u64) {
    // 确保缓存已初始化
    dcache_init();

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    // 计算哈希值
    let hash = dentry_hash(name, parent_ino);
    let index = (hash as usize) % DCACHE_SIZE;

    // 删除条目
    if let Some(ref dentry) = inner.buckets[index].dentry {
        if inner.buckets[index].key == hash {
            // 标记为未哈希
            dentry.set_unhashed();

            // 从缓存中移除
            inner.buckets[index].dentry = None;
            inner.buckets[index].key = 0;
            inner.buckets[index].access_time.store(0, Ordering::Relaxed);
            inner.count -= 1;
        }
    }
}

/// 获取缓存统计信息
pub fn dcache_stats() -> (usize, usize) {
    // 确保缓存已初始化
    dcache_init();

    let cache = DCACHE.lock();
    let cache_inner = cache.as_ref().expect("dcache not initialized");

    (cache_inner.count, DCACHE_SIZE)
}

/// 获取详细的缓存统计信息
pub fn dcache_stats_detailed() -> (u64, u64, u64, f64) {
    // 确保缓存已初始化
    dcache_init();

    let cache = DCACHE.lock();
    let cache_inner = cache.as_ref().expect("dcache not initialized");

    (
        cache_inner.stats.hits.load(Ordering::Relaxed),
        cache_inner.stats.misses.load(Ordering::Relaxed),
        cache_inner.stats.evictions.load(Ordering::Relaxed),
        cache_inner.stats.get_hit_rate(),
    )
}

/// 清空 Dentry 缓存
///
pub fn dcache_flush() {
    // 确保缓存已初始化
    dcache_init();

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    // 清空所有桶
    for bucket in inner.buckets.iter_mut() {
        if let Some(ref dentry) = bucket.dentry {
            // 标记为未哈希
            dentry.set_unhashed();
        }
        bucket.dentry = None;
        bucket.key = 0;
        bucket.access_time.store(0, Ordering::Relaxed);
    }

    inner.count = 0;
}
