//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buddy System (伙伴系统) 内存分配器
//!
//! 改进版本：元数据与用户数据分开存储，避免 BlockHeader 被覆盖

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE_SIZE: usize = 4096;

const MAX_ORDER: usize = 20;

const MIN_ORDER: usize = 0;

const HEAP_START: usize = 0x80A0_0000;

// 堆大小 - 16MB（原始大小）
// 注意：帧缓冲区会从堆中分配，约4MB (1280x800x4)
const HEAP_SIZE: usize = 16 * 1024 * 1024;  // 16MB

/// 最大页数（用于元数据数组大小）
const MAX_PAGES: usize = HEAP_SIZE / PAGE_SIZE;  // 4096 页

/// 空链表标记（使用超出范围的值）
const EMPTY_LIST: usize = MAX_PAGES + 1;

/// 块元数据（分开存储，不与用户数据混用）
#[repr(C)]
#[derive(Clone, Copy)]
struct BlockMeta {
    /// 块的大小等级 (2^order * PAGE_SIZE)
    order: u8,
    /// 是否空闲
    free: u8,
    /// 前驱索引（在元数据数组中的索引，0 表示空）
    prev: u16,
    /// 后继索引（在元数据数组中的索引，0 表示空）
    next: u16,
}

impl BlockMeta {
    const fn new() -> Self {
        Self {
            order: 0,
            free: 0,
            prev: 0,
            next: 0,
        }
    }
}

/// 元数据数组包装器（使用 UnsafeCell 实现内部可变性）
struct MetaArray {
    data: UnsafeCell<[BlockMeta; MAX_PAGES]>,
}

unsafe impl Send for MetaArray {}
unsafe impl Sync for MetaArray {}

impl MetaArray {
    const fn new() -> Self {
        Self {
            data: UnsafeCell::new([const { BlockMeta::new() }; MAX_PAGES]),
        }
    }

    /// 获取元数据引用（安全：只在单线程环境下使用）
    fn get(&self, idx: usize) -> &BlockMeta {
        unsafe { &(*self.data.get())[idx] }
    }

    /// 获取元数据可变引用（安全：只在单线程环境下使用）
    fn get_mut(&self, idx: usize) -> &mut BlockMeta {
        unsafe { &mut (*self.data.get())[idx] }
    }
}

pub struct BuddyAllocator {
    /// 堆的起始地址（用户数据区域）
    heap_start: AtomicUsize,
    /// 堆的结束地址
    heap_end: AtomicUsize,
    /// 空闲块链表 (每个 order 一个链表，存储页索引)
    free_lists: [AtomicUsize; MAX_ORDER + 1],
    /// 是否已初始化
    initialized: AtomicUsize,
    /// 元数据区域（存储每个页的元数据）
    meta: MetaArray,
}

unsafe impl Send for BuddyAllocator {}
unsafe impl Sync for BuddyAllocator {}

impl BuddyAllocator {
    pub const fn new() -> Self {
        Self {
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            free_lists: [const { AtomicUsize::new(0) }; MAX_ORDER + 1],
            initialized: AtomicUsize::new(0),
            meta: MetaArray::new(),
        }
    }

    /// 初始化分配器
    pub fn init(&self) {
        if self.initialized.load(Ordering::Acquire) != 0 {
            return;
        }

        if self.initialized.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.heap_start.store(HEAP_START, Ordering::Release);
            self.heap_end.store(HEAP_START + HEAP_SIZE, Ordering::Release);

            // 初始化所有空闲链表为空
            for i in 0..=MAX_ORDER {
                self.free_lists[i].store(EMPTY_LIST, Ordering::Release);
            }

            // 计算最大 order
            let max_order = self.heap_size_to_order(HEAP_SIZE);

            // 将整个堆作为一个大块添加到对应 order 的空闲链表
            // 页索引 0 对应 HEAP_START
            self.init_block(0, max_order, false);
            self.add_to_free_list(0, max_order);
        }
    }

    /// 初始化块元数据
    fn init_block(&self, page_idx: usize, order: usize, free: bool) {
        let meta = self.meta.get_mut(page_idx);
        meta.order = order as u8;
        meta.free = if free { 1 } else { 0 };
        meta.prev = 0;
        meta.next = 0;
    }

    /// 将块添加到空闲链表
    fn add_to_free_list(&self, page_idx: usize, order: usize) {
        {
            let meta = self.meta.get_mut(page_idx);
            meta.order = order as u8;
            meta.free = 1;
        }

        // 获取当前空闲链表头
        let list_head = self.free_lists[order].load(Ordering::Acquire);

        // 将块插入链表头部
        if list_head != EMPTY_LIST && list_head < MAX_PAGES {
            self.meta.get_mut(list_head).prev = page_idx as u16;
        }
        {
            let meta = self.meta.get_mut(page_idx);
            meta.next = if list_head == EMPTY_LIST { 0xFFFF } else { list_head as u16 };
            meta.prev = 0xFFFF;  // 0xFFFF 表示空
        }

        // 更新链表头
        self.free_lists[order].store(page_idx, Ordering::Release);
    }

    /// 从空闲链表移除块
    fn remove_from_free_list(&self, page_idx: usize, order: usize) {
        let prev_idx = self.meta.get(page_idx).prev as usize;
        let next_idx = self.meta.get(page_idx).next as usize;

        if prev_idx != 0xFFFF && prev_idx < MAX_PAGES {
            self.meta.get_mut(prev_idx).next = next_idx as u16;
        } else {
            // 这是链表头，更新全局链表头
            let new_head = if next_idx == 0xFFFF { EMPTY_LIST } else { next_idx };
            self.free_lists[order].store(new_head, Ordering::Release);
        }

        if next_idx != 0xFFFF && next_idx < MAX_PAGES {
            self.meta.get_mut(next_idx).prev = prev_idx as u16;
        }

        self.meta.get_mut(page_idx).free = 0;
    }

    /// 计算堆大小对应的 order
    fn heap_size_to_order(&self, size: usize) -> usize {
        let mut order = 0;
        let mut block_size = PAGE_SIZE;
        while block_size < size {
            block_size *= 2;
            order += 1;
        }
        order
    }

    /// 将大小转换为 order
    fn size_to_order(&self, size: usize) -> usize {
        let mut order = 0;
        let mut block_size = PAGE_SIZE;
        while block_size < size {
            block_size *= 2;
            order += 1;
        }
        order
    }

    /// 获取块的伙伴页索引
    fn get_buddy_idx(&self, page_idx: usize, order: usize) -> usize {
        let block_size_pages = 1usize << order;  // 块包含的页数
        page_idx ^ block_size_pages
    }

    /// 页索引转换为地址
    fn page_idx_to_addr(&self, page_idx: usize) -> usize {
        HEAP_START + page_idx * PAGE_SIZE
    }

    /// 地址转换为页索引
    fn addr_to_page_idx(&self, addr: usize) -> usize {
        (addr - HEAP_START) / PAGE_SIZE
    }

    /// 分配内存
    fn alloc_blocks(&self, order: usize) -> *mut u8 {
        // 从指定 order 开始查找
        for mut current_order in order..=MAX_ORDER {
            let list_head = self.free_lists[current_order].load(Ordering::Acquire);

            if list_head != EMPTY_LIST && list_head < MAX_PAGES {
                // 找到空闲块
                self.remove_from_free_list(list_head, current_order);

                // 如果需要，分割块
                let mut page_idx = list_head;
                while current_order > order {
                    let block_size_pages = 1usize << current_order;
                    let buddy_idx = page_idx + (block_size_pages / 2);

                    // 初始化伙伴块并加入空闲链表
                    self.init_block(buddy_idx, current_order - 1, true);
                    self.add_to_free_list(buddy_idx, current_order - 1);

                    // 更新当前块为前半部分
                    self.init_block(page_idx, current_order - 1, false);
                    current_order -= 1;
                }

                // 返回用户可用地址（直接返回块开始地址，元数据分开存储）
                return self.page_idx_to_addr(page_idx) as *mut u8;
            }
        }

        // 没有足够内存
        core::ptr::null_mut()
    }

    /// 释放内存
    unsafe fn free_blocks(&self, ptr: *mut u8, order: usize) {
        let addr = ptr as usize;
        let mut page_idx = self.addr_to_page_idx(addr);
        let mut current_order = order;

        loop {
            let buddy_idx = self.get_buddy_idx(page_idx, current_order);

            // 检查伙伴是否在有效范围内
            if buddy_idx >= MAX_PAGES {
                // 伙伴超出范围，无法合并
                self.add_to_free_list(page_idx, current_order);
                break;
            }

            // 检查伙伴是否空闲且大小匹配
            let buddy_meta = self.meta.get(buddy_idx);
            if buddy_meta.free == 0 || buddy_meta.order != current_order as u8 {
                // 伙伴不空闲或大小不匹配，无法合并
                self.add_to_free_list(page_idx, current_order);
                break;
            }

            // 伙伴空闲，从链表中移除
            self.remove_from_free_list(buddy_idx, current_order);

            // 合并：选择索引较小的作为基址
            if page_idx > buddy_idx {
                page_idx = buddy_idx;
            }

            current_order += 1;
        }
    }
}

unsafe impl GlobalAlloc for BuddyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if self.initialized.load(Ordering::Acquire) == 0 {
            return core::ptr::null_mut();
        }

        let size = layout.size();
        let align = layout.align();

        let order = self.size_to_order(size.max(align));
        self.alloc_blocks(order)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if self.initialized.load(Ordering::Acquire) == 0 {
            return;
        }

        let size = layout.size();
        let align = layout.align();

        let ptr_addr = ptr as usize;
        let heap_start = self.heap_start.load(Ordering::Acquire);
        let heap_end = self.heap_end.load(Ordering::Acquire);

        if ptr_addr < heap_start || ptr_addr >= heap_end {
            return;
        }

        let order = self.size_to_order(size.max(align));

        if order > MAX_ORDER {
            return;
        }

        self.free_blocks(ptr, order);
    }
}

#[global_allocator]
pub static HEAP_ALLOCATOR: BuddyAllocator = BuddyAllocator::new();

pub fn init_heap() {
    HEAP_ALLOCATOR.init();
}

/// Buddy 分配器统计信息
#[derive(Debug, Clone, Copy, Default)]
pub struct BuddyStats {
    /// 堆起始地址
    pub heap_start: usize,
    /// 堆结束地址
    pub heap_end: usize,
    /// 堆总大小（字节）
    pub heap_size: usize,
    /// 已使用大小（字节）
    pub used_bytes: usize,
    /// 空闲大小（字节）
    pub free_bytes: usize,
    /// 各 order 的空闲块数量
    pub free_blocks: [usize; MAX_ORDER + 1],
    /// 总分配次数
    pub alloc_count: usize,
    /// 总释放次数
    pub free_count: usize,
}

/// 获取 Buddy 分配器统计信息
pub fn buddy_stats() -> BuddyStats {
    let mut stats = BuddyStats::default();

    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return stats;
    }

    stats.heap_start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
    stats.heap_end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);
    stats.heap_size = stats.heap_end - stats.heap_start;

    // 统计各 order 的空闲块数量
    let mut total_free_pages = 0usize;
    for order in 0..=MAX_ORDER {
        let mut count = 0usize;
        let mut page_idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);

        while page_idx != EMPTY_LIST && page_idx < MAX_PAGES {
            count += 1;
            total_free_pages += 1usize << order;
            let next = HEAP_ALLOCATOR.meta.get(page_idx).next as usize;
            if next == 0xFFFF || next >= MAX_PAGES {
                break;
            }
            page_idx = next;
        }
        stats.free_blocks[order] = count;
    }

    stats.free_bytes = total_free_pages * PAGE_SIZE;
    stats.used_bytes = stats.heap_size - stats.free_bytes;

    stats
}
