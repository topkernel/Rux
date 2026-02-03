//! 页帧管理

use core::sync::atomic::{AtomicUsize, Ordering};

/// 页大小 (4KB)
pub const PAGE_SIZE: usize = 4096;

/// 页大小对齐掩码
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

/// 物理页号类型
pub type PhysFrameNr = usize;

/// 虚拟页号类型
pub type VirtPageNr = usize;

/// 物理地址
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub usize);

/// 虚拟地址
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

/// 物理页帧
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

/// 虚拟页
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

/// 页帧分配器 - 简单的栈分配器
pub struct FrameAllocator {
    next_free: AtomicUsize,
    total_frames: usize,
}

impl FrameAllocator {
    pub const fn new(total_frames: usize) -> Self {
        Self {
            next_free: AtomicUsize::new(0),
            total_frames,
        }
    }

    pub fn init(&self, start_frame: PhysFrameNr) {
        self.next_free.store(start_frame, Ordering::SeqCst);
    }

    pub fn allocate(&self) -> Option<PhysFrame> {
        let frame = self.next_free.fetch_add(1, Ordering::SeqCst);
        if frame < self.total_frames {
            Some(PhysFrame::new(frame))
        } else {
            self.next_free.fetch_sub(1, Ordering::SeqCst);
            None
        }
    }

    pub fn deallocate(&self, _frame: PhysFrame) {
        // 简单实现：暂不处理释放
        // 实际需要更复杂的空闲链表管理
    }
}

/// 全局帧分配器
static FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::new(PHYS_MEMORY_SIZE / PAGE_SIZE);

pub fn init_frame_allocator(start_frame: PhysFrameNr) {
    FRAME_ALLOCATOR.init(start_frame);
}

pub fn alloc_frame() -> Option<PhysFrame> {
    FRAME_ALLOCATOR.allocate()
}

pub fn dealloc_frame(frame: PhysFrame) {
    FRAME_ALLOCATOR.deallocate(frame)
}

// 物理内存大小常量
const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB
