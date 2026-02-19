//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Slab 分配器
//!
//! 用于小对象的高效内存分配，减少 buddy allocator 的碎片。
//!
//! 参考：
//! - https://www.kernel.org/doc/html/latest/core-api/memory-allocation.html
//!
//! # 设计
//! - SlabCache: 管理特定大小对象的缓存
//! - Slab: 包含多个相同大小对象的内存页
//! - kmalloc/kfree: 公共分配接口
//!
//! # 支持的对象大小
//! 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 字节

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// 页大小
const PAGE_SIZE: usize = 4096;

/// 最小对象大小（8 字节）
const MIN_OBJECT_SIZE: usize = 8;

/// 最大对象大小（一个页面）
const MAX_OBJECT_SIZE: usize = PAGE_SIZE;

/// Slab 缓存数量
const NUM_CACHES: usize = 10;

/// 对象大小数组
const OBJECT_SIZES: [usize; NUM_CACHES] = [
    8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
];

/// Slab 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlabState {
    /// 完全空闲
    Free,
    /// 部分使用
    Partial,
    /// 完全使用
    Full,
}

/// Slab 头部（存储在每个 slab 页的开头）
#[repr(C)]
struct SlabHeader {
    /// 所属缓存索引
    cache_idx: u8,
    /// 对象大小
    object_size: u16,
    /// 对象总数
    total_objects: u16,
    /// 空闲对象数
    free_objects: u16,
    /// 第一个空闲对象的索引
    free_index: u16,
    /// 下一个 slab 的页索引（在 slab_pages 数组中）
    next: u16,
    /// 上一个 slab 的页索引
    prev: u16,
}

impl SlabHeader {
    const fn new() -> Self {
        Self {
            cache_idx: 0,
            object_size: 0,
            total_objects: 0,
            free_objects: 0,
            free_index: 0,
            next: 0,
            prev: 0,
        }
    }
}

/// Slab 缓存
pub struct SlabCache {
    /// 对象大小
    object_size: usize,
    /// 每个 slab 可容纳的对象数
    objects_per_slab: usize,
    /// 空闲 slab 链表头（页索引）
    free_list: u16,
    /// 部分使用 slab 链表头
    partial_list: u16,
    /// 完全使用 slab 链表头
    full_list: u16,
    /// 统计：分配次数
    alloc_count: AtomicUsize,
    /// 统计：释放次数
    free_count: AtomicUsize,
}

impl SlabCache {
    /// 创建新的 Slab 缓存
    pub const fn new(object_size: usize) -> Self {
        // 计算每个 slab 可容纳的对象数
        // 保留头部空间
        let header_size = core::mem::size_of::<SlabHeader>();
        let usable_size = PAGE_SIZE - header_size;
        let objects_per_slab = usable_size / object_size;

        Self {
            object_size,
            objects_per_slab,
            free_list: 0,
            partial_list: 0,
            full_list: 0,
            alloc_count: AtomicUsize::new(0),
            free_count: AtomicUsize::new(0),
        }
    }

    /// 从缓存分配一个对象
    pub fn alloc(&mut self, slab_pages: &mut SlabPages) -> *mut u8 {
        // 优先从 partial 链表分配
        if self.partial_list != 0 {
            let ptr = self.alloc_from_slab(self.partial_list, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        // 如果 partial 为空，从 free 链表分配
        if self.free_list != 0 {
            let slab_idx = self.free_list;
            // 将 slab 从 free 移到 partial
            self.free_list = slab_pages.get_next(slab_idx);
            if self.free_list != 0 {
                slab_pages.set_prev(self.free_list, 0);
            }
            slab_pages.set_prev(slab_idx, 0);
            slab_pages.set_next(slab_idx, 0);
            self.partial_list = slab_idx;

            let ptr = self.alloc_from_slab(slab_idx, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        // 没有可用的 slab，需要创建新的
        if let Some(slab_idx) = self.create_slab(slab_pages) {
            self.partial_list = slab_idx;
            let ptr = self.alloc_from_slab(slab_idx, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        core::ptr::null_mut()
    }

    /// 从指定 slab 分配对象
    fn alloc_from_slab(&mut self, slab_idx: u16, slab_pages: &mut SlabPages) -> *mut u8 {
        let header = slab_pages.get_header_mut(slab_idx);

        if header.free_objects == 0 {
            return core::ptr::null_mut();
        }

        // 获取空闲对象索引
        let obj_idx = header.free_index;

        // 计算对象地址
        let header_size = core::mem::size_of::<SlabHeader>();
        let obj_offset = header_size + obj_idx as usize * self.object_size;
        let page_addr = slab_pages.get_page_addr(slab_idx);
        let obj_ptr = (page_addr + obj_offset) as *mut u8;

        // 更新头部信息
        // 读取下一个空闲索引（存储在对象内存中）
        let next_free = unsafe {
            if self.object_size >= 2 {
                *(obj_ptr as *const u16)
            } else {
                obj_idx + 1
            }
        };

        header.free_index = next_free;
        header.free_objects -= 1;

        // 检查 slab 是否变满
        if header.free_objects == 0 {
            // 从 partial 移到 full
            self.move_slab_to_full(slab_idx, slab_pages);
        }

        obj_ptr
    }

    /// 释放对象到缓存
    pub fn free(&mut self, ptr: *mut u8, slab_pages: &mut SlabPages) -> bool {
        // 找到对象所在的 slab
        let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
        let slab_idx = match slab_pages.find_slab_by_addr(page_addr) {
            Some(idx) => idx,
            None => return false,
        };

        let header = slab_pages.get_header_mut(slab_idx);

        // 验证缓存索引
        if header.cache_idx as usize >= NUM_CACHES {
            return false;
        }

        // 计算对象索引
        let header_size = core::mem::size_of::<SlabHeader>();
        let obj_offset = ptr as usize - page_addr - header_size;
        let obj_idx = (obj_offset / self.object_size) as u16;

        // 将对象索引写入对象内存（作为空闲链表）
        unsafe {
            if self.object_size >= 2 {
                *(ptr as *mut u16) = header.free_index;
            }
        }

        header.free_index = obj_idx;
        header.free_objects += 1;

        self.free_count.fetch_add(1, Ordering::Relaxed);

        // 检查 slab 状态变化
        let was_full = header.free_objects == 1;
        let is_empty = header.free_objects == header.total_objects;

        if was_full {
            // 从 full 移到 partial
            self.move_slab_from_full(slab_idx, slab_pages);
        } else if is_empty {
            // 从 partial 移到 free（可选：释放 slab）
            // 暂时保留在 partial，避免频繁创建/销毁
        }

        true
    }

    /// 创建新的 slab
    fn create_slab(&mut self, slab_pages: &mut SlabPages) -> Option<u16> {
        // 从 buddy allocator 分配一页
        let page = slab_pages.alloc_page()?;

        // 初始化 slab 头部
        let header = slab_pages.get_header_mut(page);
        header.cache_idx = 0; // 将在返回后设置
        header.object_size = self.object_size as u16;
        header.total_objects = self.objects_per_slab as u16;
        header.free_objects = self.objects_per_slab as u16;
        header.free_index = 0;
        header.next = 0;
        header.prev = 0;

        // 初始化空闲链表（每个对象存储下一个空闲索引）
        let header_size = core::mem::size_of::<SlabHeader>();
        let page_addr = slab_pages.get_page_addr(page);

        for i in 0..self.objects_per_slab - 1 {
            let obj_offset = header_size + i * self.object_size;
            let obj_ptr = (page_addr + obj_offset) as *mut u16;
            unsafe {
                *obj_ptr = (i + 1) as u16;
            }
        }

        // 最后一个对象的 next 为 0xFFFF（链表结束标记）
        if self.objects_per_slab > 0 {
            let last_offset = header_size + (self.objects_per_slab - 1) * self.object_size;
            let last_ptr = (page_addr + last_offset) as *mut u16;
            unsafe {
                *last_ptr = 0xFFFF;
            }
        }

        Some(page)
    }

    /// 将 slab 从 partial 移到 full
    fn move_slab_to_full(&mut self, slab_idx: u16, slab_pages: &mut SlabPages) {
        // 从 partial 链表移除
        let next = slab_pages.get_next(slab_idx);
        let prev = slab_pages.get_prev(slab_idx);

        if prev == 0 {
            self.partial_list = next;
        } else {
            slab_pages.set_next(prev, next);
        }

        if next != 0 {
            slab_pages.set_prev(next, prev);
        }

        // 添加到 full 链表头部
        slab_pages.set_next(slab_idx, self.full_list);
        slab_pages.set_prev(slab_idx, 0);
        if self.full_list != 0 {
            slab_pages.set_prev(self.full_list, slab_idx);
        }
        self.full_list = slab_idx;
    }

    /// 将 slab 从 full 移到 partial
    fn move_slab_from_full(&mut self, slab_idx: u16, slab_pages: &mut SlabPages) {
        // 从 full 链表移除
        let next = slab_pages.get_next(slab_idx);
        let prev = slab_pages.get_prev(slab_idx);

        if prev == 0 {
            self.full_list = next;
        } else {
            slab_pages.set_next(prev, next);
        }

        if next != 0 {
            slab_pages.set_prev(next, prev);
        }

        // 添加到 partial 链表头部
        slab_pages.set_next(slab_idx, self.partial_list);
        slab_pages.set_prev(slab_idx, 0);
        if self.partial_list != 0 {
            slab_pages.set_prev(self.partial_list, slab_idx);
        }
        self.partial_list = slab_idx;
    }
}

/// Slab 页管理
pub struct SlabPages {
    /// Slab 页基地址
    base_addr: usize,
    /// 已分配的页数
    allocated_pages: AtomicUsize,
    /// 最大页数
    max_pages: usize,
}

impl SlabPages {
    pub const fn new(base_addr: usize, max_pages: usize) -> Self {
        Self {
            base_addr,
            allocated_pages: AtomicUsize::new(0),
            max_pages,
        }
    }

    /// 分配一个新页
    fn alloc_page(&self) -> Option<u16> {
        let idx = self.allocated_pages.fetch_add(1, Ordering::AcqRel);
        if idx >= self.max_pages {
            self.allocated_pages.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some((idx + 1) as u16) // 使用 1-based 索引，0 表示空
    }

    /// 获取页地址
    fn get_page_addr(&self, idx: u16) -> usize {
        self.base_addr + (idx as usize - 1) * PAGE_SIZE
    }

    /// 获取 slab 头部
    fn get_header_mut(&self, idx: u16) -> &mut SlabHeader {
        let addr = self.get_page_addr(idx);
        unsafe { &mut *(addr as *mut SlabHeader) }
    }

    /// 获取下一个 slab
    fn get_next(&self, idx: u16) -> u16 {
        self.get_header_mut(idx).next
    }

    /// 设置下一个 slab
    fn set_next(&self, idx: u16, next: u16) {
        self.get_header_mut(idx).next = next;
    }

    /// 获取上一个 slab
    fn get_prev(&self, idx: u16) -> u16 {
        self.get_header_mut(idx).prev
    }

    /// 设置上一个 slab
    fn set_prev(&self, idx: u16, prev: u16) {
        self.get_header_mut(idx).prev = prev;
    }

    /// 根据地址查找 slab
    fn find_slab_by_addr(&self, addr: usize) -> Option<u16> {
        if addr < self.base_addr {
            return None;
        }
        let offset = addr - self.base_addr;
        if offset >= self.max_pages * PAGE_SIZE {
            return None;
        }
        let idx = (offset / PAGE_SIZE) as u16 + 1;
        if idx as usize > self.allocated_pages.load(Ordering::Acquire) {
            return None;
        }
        Some(idx)
    }
}

/// Slab 分配器全局状态
pub struct SlabAllocator {
    /// Slab 缓存数组
    caches: [Mutex<SlabCache>; NUM_CACHES],
    /// Slab 页管理
    pages: SlabPages,
    /// 是否已初始化
    initialized: AtomicUsize,
}

/// 静态 Slab 分配器实例
static mut SLAB_ALLOCATOR: SlabAllocator = SlabAllocator {
    caches: [
        Mutex::new(SlabCache::new(8)),
        Mutex::new(SlabCache::new(16)),
        Mutex::new(SlabCache::new(32)),
        Mutex::new(SlabCache::new(64)),
        Mutex::new(SlabCache::new(128)),
        Mutex::new(SlabCache::new(256)),
        Mutex::new(SlabCache::new(512)),
        Mutex::new(SlabCache::new(1024)),
        Mutex::new(SlabCache::new(2048)),
        Mutex::new(SlabCache::new(4096)),
    ],
    pages: SlabPages::new(0, 0),
    initialized: AtomicUsize::new(0),
};

impl SlabAllocator {
    /// 初始化 Slab 分配器
    pub fn init(base_addr: usize, max_pages: usize) {
        unsafe {
            SLAB_ALLOCATOR.pages = SlabPages::new(base_addr, max_pages);
            SLAB_ALLOCATOR.initialized.store(1, Ordering::Release);
        }
    }

    /// 检查是否已初始化
    fn is_initialized() -> bool {
        unsafe { SLAB_ALLOCATOR.initialized.load(Ordering::Acquire) == 1 }
    }

    /// 查找合适的缓存索引
    fn find_cache_index(size: usize) -> Option<usize> {
        if size == 0 || size > MAX_OBJECT_SIZE {
            return None;
        }

        for (i, &obj_size) in OBJECT_SIZES.iter().enumerate() {
            if size <= obj_size {
                return Some(i);
            }
        }
        None
    }
}

/// 分配内存
///
/// # 参数
/// - `size`: 请求的内存大小
///
/// # 返回
/// 成功返回内存指针，失败返回 null
pub fn kmalloc(size: usize) -> *mut u8 {
    if !SlabAllocator::is_initialized() {
        return core::ptr::null_mut();
    }

    // 查找合适的缓存
    let cache_idx = match SlabAllocator::find_cache_index(size) {
        Some(idx) => idx,
        None => return core::ptr::null_mut(),
    };

    unsafe {
        let mut cache = SLAB_ALLOCATOR.caches[cache_idx].lock();
        cache.alloc(&mut SLAB_ALLOCATOR.pages)
    }
}

/// 释放内存
///
/// # 参数
/// - `ptr`: 要释放的内存指针
pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() || !SlabAllocator::is_initialized() {
        return;
    }

    unsafe {
        // 尝试在每个缓存中释放
        for i in 0..NUM_CACHES {
            let mut cache = SLAB_ALLOCATOR.caches[i].lock();
            if cache.free(ptr, &mut SLAB_ALLOCATOR.pages) {
                return;
            }
        }
    }
}

/// 分配并清零内存
///
/// # 参数
/// - `size`: 请求的内存大小
///
/// # 返回
/// 成功返回清零的内存指针，失败返回 null
pub fn kzalloc(size: usize) -> *mut u8 {
    let ptr = kmalloc(size);
    if !ptr.is_null() {
        unsafe {
            core::ptr::write_bytes(ptr, 0, size);
        }
    }
    ptr
}

/// 初始化 Slab 分配器
///
/// # 参数
/// - `base_addr`: Slab 内存区域起始地址
/// - `size`: Slab 内存区域大小
pub fn init_slab(base_addr: usize, size: usize) {
    let max_pages = size / PAGE_SIZE;
    SlabAllocator::init(base_addr, max_pages);
}

/// 获取 Slab 统计信息
pub fn slab_stats() -> SlabStats {
    let mut stats = SlabStats::default();

    if !SlabAllocator::is_initialized() {
        return stats;
    }

    unsafe {
        for i in 0..NUM_CACHES {
            let cache = SLAB_ALLOCATOR.caches[i].lock();
            stats.cache_stats[i] = CacheStats {
                object_size: cache.object_size,
                alloc_count: cache.alloc_count.load(Ordering::Relaxed),
                free_count: cache.free_count.load(Ordering::Relaxed),
            };
        }
        stats.total_pages = SLAB_ALLOCATOR.pages.allocated_pages.load(Ordering::Relaxed);
    }

    stats
}

/// 缓存统计信息
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub object_size: usize,
    pub alloc_count: usize,
    pub free_count: usize,
}

/// Slab 统计信息
#[derive(Debug, Clone, Copy, Default)]
pub struct SlabStats {
    pub cache_stats: [CacheStats; NUM_CACHES],
    pub total_pages: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_cache_index() {
        assert_eq!(SlabAllocator::find_cache_index(1), Some(0));
        assert_eq!(SlabAllocator::find_cache_index(8), Some(0));
        assert_eq!(SlabAllocator::find_cache_index(9), Some(1));
        assert_eq!(SlabAllocator::find_cache_index(16), Some(1));
        assert_eq!(SlabAllocator::find_cache_index(100), Some(6));
        assert_eq!(SlabAllocator::find_cache_index(4096), Some(9));
        assert_eq!(SlabAllocator::find_cache_index(4097), None);
        assert_eq!(SlabAllocator::find_cache_index(0), None);
    }
}
