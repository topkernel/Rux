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
}

// 使用 usize::MAX 表示空闲链表的空指针
const FREE_LIST_NULL: usize = usize::MAX;

impl FrameAllocator {
    pub const fn new(total_frames: usize) -> Self {
        Self {
            next_free: AtomicUsize::new(0),
            free_list: AtomicUsize::new(FREE_LIST_NULL),
            total_frames,
        }
    }

    pub fn init(&self, start_frame: PhysFrameNr) {
        self.next_free.store(start_frame, Ordering::SeqCst);
    }

    pub fn allocate(&self) -> Option<PhysFrame> {
        // 1. 首先尝试从空闲链表中分配
        loop {
            let head = self.free_list.load(Ordering::Acquire);
            if head == FREE_LIST_NULL {
                break;  // 空闲链表为空，使用 bump allocator
            }

            // 读取下一帧的指针（存储在释放页面的前 8 字节）
            // RISC-V: 在 QEMU virt 平台上，物理地址可以直接作为虚拟地址访问
            // （内核区域的恒等映射，参见 arch/riscv64/mm.rs::virt_to_phys）
            let next = unsafe {
                let virt_addr = head * PAGE_SIZE;
                *(virt_addr as *const usize)
            };

            // 尝试 CAS 更新空闲链表头
            match self.free_list.compare_exchange_weak(
                head,
                next,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(PhysFrame::new(head)),  // 成功从空闲链表分配
                Err(_) => continue,  // CAS 失败，重试
            }
        }

        // 2. 空闲链表为空，使用 bump 分配器
        let frame = self.next_free.fetch_add(1, Ordering::SeqCst);
        if frame < self.total_frames {
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

            // 将 next 指针写入释放页面的前 8 字节
            // RISC-V: 在 QEMU virt 平台上，物理地址可以直接作为虚拟地址访问
            // （内核区域的恒等映射，参见 arch/riscv64/mm.rs::virt_to_phys）
            unsafe {
                let virt_addr = frame_num * PAGE_SIZE;
                *(virt_addr as *mut usize) = head;
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

pub fn alloc_frame() -> Option<PhysFrame> {
    FRAME_ALLOCATOR.allocate()
}

pub fn dealloc_frame(frame: PhysFrame) {
    FRAME_ALLOCATOR.deallocate(frame)
}

// 物理内存常量
const PHYS_MEMORY_BASE: usize = 0x80000000;  // QEMU virt: 物理内存起始地址
const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB
const PHYS_MEMORY_BASE_FRAME: PhysFrameNr = PHYS_MEMORY_BASE / PAGE_SIZE;  // 0x80000
