//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Page Frame Management

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

    /// Get physical page number (PPN)
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
    free_list: AtomicUsize,  // Free list head (stores physical page numbers)
    total_frames: usize,
    use_page_desc: AtomicUsize, // Whether to use Page descriptors
}

// Use usize::MAX to represent null pointer in free list
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

    /// Enable Page descriptor support
    pub fn enable_page_desc(&self) {
        self.use_page_desc.store(1, Ordering::SeqCst);
    }

    pub fn allocate(&self) -> Option<PhysFrame> {
        // 1. First try to allocate from free list
        loop {
            let head = self.free_list.load(Ordering::Acquire);
            if head == FREE_LIST_NULL {
                break;  // Free list is empty, use bump allocator
            }

            // Read next frame pointer
            let next = if self.use_page_desc.load(Ordering::Acquire) == 1 {
                // Use Page::next_free field to store free list
                let page = super::page_desc::pfn_to_page(head);
                if page.is_null() {
                    break;
                }
                unsafe { (*page).next_free() }
            } else {
                // Old way: store in first 8 bytes of page
                unsafe {
                    let virt_addr = head * PAGE_SIZE;
                    *(virt_addr as *const usize)
                }
            };

            // Try CAS to update free list head
            match self.free_list.compare_exchange_weak(
                head,
                next,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // Allocation successful, update Page reference count
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
                Err(_) => continue,  // CAS failed, retry
            }
        }

        // 2. Free list is empty, use bump allocator
        let frame = self.next_free.fetch_add(1, Ordering::SeqCst);
        if frame < self.total_frames {
            // Update Page reference count
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

        // RISC-V QEMU virt: physical memory starts at 0x80000000
        // Frames below this address cannot be accessed, ignore them
        if frame_num < PHYS_MEMORY_BASE_FRAME {
            return;
        }

        // Add frame to free list head
        loop {
            let head = self.free_list.load(Ordering::Acquire);

            // Write next pointer to freed page
            if self.use_page_desc.load(Ordering::Acquire) == 1 {
                // Use Page::next_free field to store free list
                let page = super::page_desc::pfn_to_page_mut(frame_num);
                if !page.is_null() {
                    unsafe {
                        // Reset Page state
                        (*page).set_refcount(0);
                        (*page).reset_mapcount();
                        (*page).clear_flag(super::page_desc::PageFlag::Referenced);
                        (*page).clear_flag(super::page_desc::PageFlag::Dirty);
                        // Set free list pointer
                        (*page).set_next_free(head);
                    }
                }
            } else {
                // Old way: store in first 8 bytes of page
                unsafe {
                    let virt_addr = frame_num * PAGE_SIZE;
                    *(virt_addr as *mut usize) = head;
                }
            }

            // Try CAS to update free list head
            match self.free_list.compare_exchange_weak(
                head,
                frame_num,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => return,  // Successfully freed
                Err(_) => continue,  // CAS failed, retry
            }
        }
    }
}

static FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::new(TOTAL_FRAMES);

pub fn init_frame_allocator(start_frame: PhysFrameNr) {
    FRAME_ALLOCATOR.init(start_frame);
}

/// Initialize page descriptor support
///
/// Must be called after init_frame_allocator
pub fn init_page_descriptors(start_frame: PhysFrameNr, nr_pages: usize) {
    // Initialize page descriptor array
    super::page_desc::init_mem_map(start_frame, nr_pages);

    // Enable allocator's page descriptor support
    FRAME_ALLOCATOR.enable_page_desc();
}

pub fn alloc_frame() -> Option<PhysFrame> {
    FRAME_ALLOCATOR.allocate()
}

pub fn dealloc_frame(frame: PhysFrame) {
    FRAME_ALLOCATOR.deallocate(frame)
}

/// Physical page frame allocator statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    /// Total frame count
    pub total_frames: usize,
    /// Allocated frame count
    pub allocated_frames: usize,
    /// Free frame count
    pub free_frames: usize,
    /// Total physical memory (bytes)
    pub total_bytes: usize,
    /// Allocated physical memory (bytes)
    pub allocated_bytes: usize,
    /// Free physical memory (bytes)
    pub free_bytes: usize,
}

/// Get physical page frame allocator statistics
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

/// Get Page descriptor for a frame
pub fn frame_to_page(frame: PhysFrame) -> *const super::page_desc::Page {
    super::page_desc::frame_to_page(frame)
}

/// Get mutable Page descriptor for a frame
pub fn frame_to_page_mut(frame: PhysFrame) -> *mut super::page_desc::Page {
    super::page_desc::frame_to_page_mut(frame)
}

// Physical memory constants
const PHYS_MEMORY_BASE: usize = 0x80000000;  // QEMU virt: physical memory start address
const PHYS_MEMORY_BASE_FRAME: PhysFrameNr = PHYS_MEMORY_BASE / PAGE_SIZE;  // 0x80000

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
const PHYS_MEMORY_SIZE: usize = crate::config::PHYS_MEMORY_SIZE;

// Total frame count needs to include base address offset, because frame numbers directly correspond to physical addresses
const TOTAL_FRAMES: usize = PHYS_MEMORY_BASE_FRAME + PHYS_MEMORY_SIZE / PAGE_SIZE;
