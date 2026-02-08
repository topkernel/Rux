//! Buddy System (伙伴系统) 内存分配器
//!
//! 对应 Linux 的 page allocation (mm/page_alloc.c)
//!
//! 特点：
//! - 支持内存释放
//! - 伙伴合并机制
//! - O(log n) 分配/释放复杂度
//!
//! 算法原理：
//! - 内存按 2^n * PAGE_SIZE 划分为块
//! - 每个块有唯一的伙伴 (buddy)
//! - 释放时与伙伴合并，减少碎片

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// 页面大小 (4KB)
const PAGE_SIZE: usize = 4096;

/// 最大 order (2^20 * 4KB = 4GB)
const MAX_ORDER: usize = 20;

/// 最小 order (2^0 * 4KB = 4KB)
const MIN_ORDER: usize = 0;

/// 堆的起始地址 (0x80A0_0000，内核之后)
const HEAP_START: usize = 0x80A0_0000;

/// 堆的大小 (16MB)
const HEAP_SIZE: usize = 16 * 1024 * 1024;

/// 块头元数据
#[repr(C)]
struct BlockHeader {
    /// 块的大小等级 (2^order * PAGE_SIZE)
    order: u32,
    /// 是否空闲
    free: u32,
    /// 前驱指针 (双向链表)
    prev: usize,
    /// 后继指针 (双向链表)
    next: usize,
}

impl BlockHeader {
    const fn new(order: usize) -> Self {
        Self {
            order: order as u32,
            free: 1,
            prev: 0,
            next: 0,
        }
    }
}

/// Buddy System 分配器
pub struct BuddyAllocator {
    /// 堆的起始地址
    heap_start: AtomicUsize,
    /// 堆的结束地址
    heap_end: AtomicUsize,
    /// 空闲块链表 (每个 order 一个链表)
    /// free_lists[order] 指向该 order 的第一个空闲块
    free_lists: [AtomicUsize; MAX_ORDER + 1],
    /// 是否已初始化
    initialized: AtomicUsize,
}

unsafe impl Send for BuddyAllocator {}
unsafe impl Sync for BuddyAllocator {}

impl BuddyAllocator {
    pub const fn new() -> Self {
        const INIT: usize = 0;
        Self {
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            free_lists: [const { AtomicUsize::new(INIT) }; MAX_ORDER + 1],
            initialized: AtomicUsize::new(0),
        }
    }

    /// 初始化分配器
    pub fn init(&self) {
        // 使用 CAS 确保只初始化一次
        if self.initialized.load(Ordering::Acquire) != 0 {
            return;
        }

        if self.initialized.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.heap_start.store(HEAP_START, Ordering::Release);
            self.heap_end.store(HEAP_START + HEAP_SIZE, Ordering::Release);

            // 初始化所有空闲链表为空
            for i in 0..=MAX_ORDER {
                self.free_lists[i].store(0, Ordering::Release);
            }

            // 计算最大 order
            let max_order = self.heap_size_to_order(HEAP_SIZE);

            // 将整个堆作为一个大块添加到对应 order 的空闲链表
            let block_ptr = HEAP_START as *mut BlockHeader;
            self.init_block(block_ptr, max_order, false);  // 先初始化为已分配状态
            self.add_to_free_list(block_ptr, max_order);    // add_to_free_list 会设置 free=1
        }
    }

    /// 初始化块元数据
    fn init_block(&self, block_ptr: *mut BlockHeader, order: usize, free: bool) {
        unsafe {
            (*block_ptr).order = order as u32;
            (*block_ptr).free = if free { 1 } else { 0 };
            (*block_ptr).prev = 0;
            (*block_ptr).next = 0;
        }
    }

    /// 将块添加到空闲链表
    fn add_to_free_list(&self, block_ptr: *mut BlockHeader, order: usize) {
        unsafe {
            // 设置块的 order
            (*block_ptr).order = order as u32;
            (*block_ptr).free = 1;

            // 获取当前空闲链表头
            let list_head = self.free_lists[order].load(Ordering::Acquire) as *mut BlockHeader;

            // 将块插入链表头部
            if !list_head.is_null() {
                (*list_head).prev = block_ptr as usize;
            }
            (*block_ptr).next = list_head as usize;
            (*block_ptr).prev = 0;

            // 更新链表头
            self.free_lists[order].store(block_ptr as usize, Ordering::Release);
        }
    }

    /// 从空闲链表移除块
    fn remove_from_free_list(&self, block_ptr: *mut BlockHeader, order: usize) {
        unsafe {
            let prev = (*block_ptr).prev as *mut BlockHeader;
            let next = (*block_ptr).next as *mut BlockHeader;

            if !prev.is_null() {
                (*prev).next = next as usize;
            } else {
                // 这是链表头，更新全局链表头
                self.free_lists[order].store(next as usize, Ordering::Release);
            }

            if !next.is_null() {
                (*next).prev = prev as usize;
            }

            (*block_ptr).free = 0;
        }
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

    /// 获取块的伙伴地址
    fn get_buddy(&self, block_ptr: usize, order: usize) -> usize {
        let block_size = PAGE_SIZE << order;
        let offset = block_ptr - self.heap_start.load(Ordering::Acquire);
        let buddy_offset = offset ^ block_size;
        self.heap_start.load(Ordering::Acquire) + buddy_offset
    }

    /// 分配内存
    fn alloc_blocks(&self, order: usize) -> *mut u8 {
        // 从指定 order 开始查找
        for mut current_order in order..=MAX_ORDER {
            let list_head = self.free_lists[current_order].load(Ordering::Acquire) as *mut BlockHeader;

            if !list_head.is_null() {
                // 找到空闲块
                self.remove_from_free_list(list_head, current_order);

                // 如果需要，分割块
                let block_ptr = list_head as usize;
                while current_order > order {
                    let block_size = PAGE_SIZE << current_order;
                    let buddy_ptr = block_ptr + (block_size / 2);

                    // 初始化伙伴块并加入空闲链表
                    self.init_block(buddy_ptr as *mut BlockHeader, current_order - 1, true);
                    self.add_to_free_list(buddy_ptr as *mut BlockHeader, current_order - 1);

                    // 更新当前块为前半部分（地址较小的块）
                    // block_ptr 保持不变，始终指向前半部分
                    self.init_block(block_ptr as *mut BlockHeader, current_order - 1, false);
                    current_order -= 1;
                }

                return block_ptr as *mut u8;
            }
        }

        // 没有足够内存
        core::ptr::null_mut()
    }

    /// 释放内存
    unsafe fn free_blocks(&self, block_ptr: *mut u8, order: usize) {
        let mut current_ptr = block_ptr as usize;
        let mut current_order = order;

        loop {
            let buddy_ptr = self.get_buddy(current_ptr, current_order);
            let buddy = buddy_ptr as *mut BlockHeader;

            // 检查伙伴是否在堆范围内（关键修复：防止访问超出堆边界的地址）
            let heap_start = self.heap_start.load(Ordering::Acquire);
            let heap_end = self.heap_end.load(Ordering::Acquire);

            if buddy_ptr < heap_start || buddy_ptr >= heap_end {
                // 伙伴超出堆范围，无法合并
                self.add_to_free_list(current_ptr as *mut BlockHeader, current_order);
                break;
            }

            // 检查伙伴是否空闲
            if buddy.is_null() || (*buddy).free == 0 || (*buddy).order != current_order as u32 {
                // 伙伴不空闲或大小不匹配，无法合并
                self.add_to_free_list(current_ptr as *mut BlockHeader, current_order);
                break;
            }

            // 伙伴空闲，从链表中移除
            self.remove_from_free_list(buddy, current_order);

            // 合并：选择地址较小的作为基址
            if current_ptr > buddy_ptr {
                current_ptr = buddy_ptr;
            }

            current_order += 1;
        }
    }
}

unsafe impl GlobalAlloc for BuddyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // 确保已初始化
        if self.initialized.load(Ordering::Acquire) == 0 {
            return core::ptr::null_mut();
        }

        let size = layout.size();
        let align = layout.align();

        // 计算需要的 order（考虑对齐要求）
        let mut order = self.size_to_order(size.max(align));

        // 分配块
        let block_ptr = self.alloc_blocks(order);

        if block_ptr.is_null() {
            // OOM
            return core::ptr::null_mut();
        }

        // Buddy 系统的块已经按照 2^n * PAGE_SIZE 对齐，所以返回的地址已经满足对齐要求
        // 不需要额外的偏移，这样 dealloc 可以正确还原块地址
        block_ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // 确保已初始化
        if self.initialized.load(Ordering::Acquire) == 0 {
            return;
        }

        let size = layout.size();
        let align = layout.align();

        // 验证指针是否在堆范围内
        let heap_start = self.heap_start.load(Ordering::Acquire);
        let heap_end = self.heap_end.load(Ordering::Acquire);
        let ptr_addr = ptr as usize;

        if ptr_addr < heap_start || ptr_addr >= heap_end {
            // 指针不在堆范围内，直接返回（可能是静态分配的）
            return;
        }

        // 计算需要的 order（必须与 alloc 中的计算一致）
        let order = self.size_to_order(size.max(align));

        // 验证 order 是否有效
        if order > MAX_ORDER {
            // order 太大，可能有问题，直接返回
            return;
        }

        // 释放块
        self.free_blocks(ptr, order);
    }
}

/// 全局 Buddy 分配器
#[global_allocator]
pub static HEAP_ALLOCATOR: BuddyAllocator = BuddyAllocator::new();

/// 初始化堆
pub fn init_heap() {
    HEAP_ALLOCATOR.init();
}
