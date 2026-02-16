//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 页帧管理

use core::sync::atomic::{AtomicUsize, Ordering};

pub const PAGE_SIZE: usize = 4096;

pub const PAGE_MASK: usize = PAGE_SIZE - 1;

pub type PhysFrameNr = usize;

pub type VirtPageNr = usize;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub usize);

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

impl PhysAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn frame_number(&self) -> PhysFrameNr {
        self.0 / PAGE_SIZE
    }

    /// 获取物理页号 (PPN)
    pub fn ppn(&self) -> usize {
        self.0 / PAGE_SIZE
    }
}

impl VirtAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn page_number(&self) -> VirtPageNr {
        self.0 / PAGE_SIZE
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PhysFrame {
    pub number: PhysFrameNr,
}

impl PhysFrame {
    pub const fn new(number: PhysFrameNr) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: PhysAddr) -> Self {
        Self::new(addr.frame_number())
    }

    pub fn start_address(&self) -> PhysAddr {
        PhysAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<PhysAddr> {
        let start = self.start_address();
        let end = PhysAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct VirtPage {
    pub number: VirtPageNr,
}

impl VirtPage {
    pub const fn new(number: VirtPageNr) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: VirtAddr) -> Self {
        Self::new(addr.page_number())
    }

    pub fn start_address(&self) -> VirtAddr {
        VirtAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<VirtAddr> {
        let start = self.start_address();
        let end = VirtAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

pub struct FrameAllocator {
    next_free: AtomicUsize,
    free_list: AtomicUsize,  // 空闲链表头（存储物理页号）
    total_frames: usize,
    use_page_desc: AtomicUsize, // 是否使用 Page 描述符
}

// 使用 usize::MAX 表示空闲链表的空指针
const FREE_LIST_NULL: usize = usize::MAX;

impl FrameAllocator {
    pub const fn new(total_frames: usize) -> Self {
        Self {
            next_free: AtomicUsize::new(0),
            free_list: AtomicUsize::new(FREE_LIST_NULL),
            total_frames,
            use_page_desc: AtomicUsize::new(0),
        }
    }

    pub fn init(&self, start_frame: PhysFrameNr) {
        self.next_free.store(start_frame, Ordering::SeqCst);
    }

    /// 启用 Page 描述符支持
    pub fn enable_page_desc(&self) {
        self.use_page_desc.store(1, Ordering::SeqCst);
    }

    pub fn allocate(&self) -> Option<PhysFrame> {
        // 1. 首先尝试从空闲链表中分配
        loop {
            let head = self.free_list.load(Ordering::Acquire);
            if head == FREE_LIST_NULL {
                break;  // 空闲链表为空，使用 bump allocator
            }

            // 读取下一帧的指针
            let next = if self.use_page_desc.load(Ordering::Acquire) == 1 {
                // 使用 Page::next_free 字段存储空闲链表
                let page = super::page_desc::pfn_to_page(head);
                if page.is_null() {
                    break;
                }
                unsafe { (*page).next_free() }
            } else {
                // 旧方式：存储在页面的前 8 字节
                unsafe {
                    let virt_addr = head * PAGE_SIZE;
                    *(virt_addr as *const usize)
                }
            };

            // 尝试 CAS 更新空闲链表头
            match self.free_list.compare_exchange_weak(
                head,
                next,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // 分配成功，更新 Page 引用计数
                    if self.use_page_desc.load(Ordering::Acquire) == 1 {
                        let page = super::page_desc::pfn_to_page_mut(head);
                        if !page.is_null() {
                            unsafe {
                                (*page).set_refcount(1);
                                (*page).set_flag(super::page_desc::PageFlag::Referenced);
                            }
                        }
                    }
                    return Some(PhysFrame::new(head));
                }
                Err(_) => continue,  // CAS 失败，重试
            }
        }

        // 2. 空闲链表为空，使用 bump 分配器
        let frame = self.next_free.fetch_add(1, Ordering::SeqCst);
        if frame < self.total_frames {
            // 更新 Page 引用计数
            if self.use_page_desc.load(Ordering::Acquire) == 1 {
                let page = super::page_desc::pfn_to_page_mut(frame);
                if !page.is_null() {
                    unsafe {
                        (*page).set_refcount(1);
                        (*page).set_flag(super::page_desc::PageFlag::Referenced);
                    }
                }
            }
            Some(PhysFrame::new(frame))
        } else {
            self.next_free.fetch_sub(1, Ordering::SeqCst);
            None
        }
    }

    pub fn deallocate(&self, frame: PhysFrame) {
        let frame_num = frame.number;

        // RISC-V QEMU virt: 物理内存从 0x80000000 开始
        // 低于此地址的帧无法访问，直接忽略
        if frame_num < PHYS_MEMORY_BASE_FRAME {
            return;
        }

        // 将帧添加到空闲链表头部
        loop {
            let head = self.free_list.load(Ordering::Acquire);

            // 将 next 指针写入释放页面
            if self.use_page_desc.load(Ordering::Acquire) == 1 {
                // 使用 Page::next_free 字段存储空闲链表
                let page = super::page_desc::pfn_to_page_mut(frame_num);
                if !page.is_null() {
                    unsafe {
                        // 重置 Page 状态
                        (*page).set_refcount(0);
                        (*page).reset_mapcount();
                        (*page).clear_flag(super::page_desc::PageFlag::Referenced);
                        (*page).clear_flag(super::page_desc::PageFlag::Dirty);
                        // 设置空闲链表指针
                        (*page).set_next_free(head);
                    }
                }
            } else {
                // 旧方式：存储在页面的前 8 字节
                unsafe {
                    let virt_addr = frame_num * PAGE_SIZE;
                    *(virt_addr as *mut usize) = head;
                }
            }

            // 尝试 CAS 更新空闲链表头
            match self.free_list.compare_exchange_weak(
                head,
                frame_num,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => return,  // 成功释放
                Err(_) => continue,  // CAS 失败，重试
            }
        }
    }
}

static FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::new(PHYS_MEMORY_SIZE / PAGE_SIZE);

pub fn init_frame_allocator(start_frame: PhysFrameNr) {
    FRAME_ALLOCATOR.init(start_frame);
}

/// 初始化页描述符支持
///
/// 必须在 init_frame_allocator 之后调用
pub fn init_page_descriptors(start_frame: PhysFrameNr, nr_pages: usize) {
    // 初始化页描述符数组
    super::page_desc::init_mem_map(start_frame, nr_pages);

    // 启用分配器的页描述符支持
    FRAME_ALLOCATOR.enable_page_desc();
}

pub fn alloc_frame() -> Option<PhysFrame> {
    FRAME_ALLOCATOR.allocate()
}

pub fn dealloc_frame(frame: PhysFrame) {
    FRAME_ALLOCATOR.deallocate(frame)
}

/// 物理页帧分配器统计信息
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    /// 总页帧数
    pub total_frames: usize,
    /// 已分配页帧数
    pub allocated_frames: usize,
    /// 空闲页帧数
    pub free_frames: usize,
    /// 总物理内存（字节）
    pub total_bytes: usize,
    /// 已分配物理内存（字节）
    pub allocated_bytes: usize,
    /// 空闲物理内存（字节）
    pub free_bytes: usize,
}

/// 获取物理页帧分配器统计信息
pub fn frame_stats() -> FrameStats {
    let total = FRAME_ALLOCATOR.total_frames;
    let allocated = FRAME_ALLOCATOR.next_free.load(Ordering::Acquire);
    let free = total.saturating_sub(allocated);

    FrameStats {
        total_frames: total,
        allocated_frames: allocated,
        free_frames: free,
        total_bytes: total * PAGE_SIZE,
        allocated_bytes: allocated * PAGE_SIZE,
        free_bytes: free * PAGE_SIZE,
    }
}

/// 获取页帧对应的 Page 描述符
pub fn frame_to_page(frame: PhysFrame) -> *const super::page_desc::Page {
    super::page_desc::frame_to_page(frame)
}

/// 获取页帧对应的可变 Page 描述符
pub fn frame_to_page_mut(frame: PhysFrame) -> *mut super::page_desc::Page {
    super::page_desc::frame_to_page_mut(frame)
}

// 物理内存常量
const PHYS_MEMORY_BASE: usize = 0x80000000;  // QEMU virt: 物理内存起始地址
const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB
const PHYS_MEMORY_BASE_FRAME: PhysFrameNr = PHYS_MEMORY_BASE / PAGE_SIZE;  // 0x80000
