//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buddy System Memory Allocator
//!
//! Improved version: Metadata and user data are stored separately to prevent BlockHeader corruption

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE_SIZE: usize = 4096;

// Use config value for max order
const MAX_ORDER: usize = crate::config::BUDDY_MAX_ORDER;

const MIN_ORDER: usize = 0;

const HEAP_START: usize = 0x80A0_0000;

// Heap size - read from configuration file
// Note: Frame buffer is allocated from the heap, approximately 4MB (1280x800x4)
const HEAP_SIZE: usize = crate::config::KERNEL_HEAP_SIZE;

/// Maximum number of pages (for metadata array size)
const MAX_PAGES: usize = HEAP_SIZE / PAGE_SIZE;  // 4096 pages

/// Empty list marker (use out-of-range value)
const EMPTY_LIST: usize = MAX_PAGES + 1;

/// Block metadata (stored separately, not mixed with user data)
#[repr(C)]
#[derive(Clone, Copy)]
struct BlockMeta {
    /// Block size order (2^order * PAGE_SIZE)
    order: u8,
    /// Whether the block is free
    free: u8,
    /// Previous index (index in metadata array, 0 means null)
    prev: u16,
    /// Next index (index in metadata array, 0 means null)
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

/// Metadata array wrapper (uses UnsafeCell for interior mutability)
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

    /// Get metadata reference (safe: only used in single-threaded context)
    fn get(&self, idx: usize) -> &BlockMeta {
        unsafe { &(*self.data.get())[idx] }
    }

    /// Get mutable metadata reference (safe: only used in single-threaded context)
    fn get_mut(&self, idx: usize) -> &mut BlockMeta {
        if idx >= MAX_PAGES {
            // Index out of bounds, return first element (safe fallback)
            return unsafe { &mut (*self.data.get())[0] };
        }
        unsafe { &mut (*self.data.get())[idx] }
    }
}

pub struct BuddyAllocator {
    /// Magic number (for corruption detection)
    magic: AtomicUsize,
    /// Heap start address (user data area)
    heap_start: AtomicUsize,
    /// Heap end address
    heap_end: AtomicUsize,
    /// Free block lists (one list per order, stores page indices)
    free_lists: [AtomicUsize; MAX_ORDER + 1],
    /// Whether initialized
    initialized: AtomicUsize,
    /// Metadata area (stores metadata for each page)
    meta: MetaArray,
}

unsafe impl Send for BuddyAllocator {}
unsafe impl Sync for BuddyAllocator {}

impl BuddyAllocator {
    pub const fn new() -> Self {
        Self {
            magic: AtomicUsize::new(0xDEADBEEF),
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            free_lists: [const { AtomicUsize::new(0) }; MAX_ORDER + 1],
            initialized: AtomicUsize::new(0),
            meta: MetaArray::new(),
        }
    }

    /// Check if magic number is valid
    fn check_magic(&self) -> bool {
        self.magic.load(Ordering::Acquire) == 0xDEADBEEF
    }

    /// Initialize the allocator
    pub fn init(&self) {
        if self.initialized.load(Ordering::Acquire) != 0 {
            return;
        }

        if self.initialized.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            // Set magic number
            self.magic.store(0xDEADBEEF, Ordering::Release);
            self.heap_start.store(HEAP_START, Ordering::Release);
            self.heap_end.store(HEAP_START + HEAP_SIZE, Ordering::Release);

            // Initialize all free lists to empty
            for i in 0..=MAX_ORDER {
                self.free_lists[i].store(EMPTY_LIST, Ordering::Release);
            }

            // Calculate maximum order
            let max_order = self.heap_size_to_order(HEAP_SIZE);

            // Add entire heap as a single large block to corresponding order's free list
            // Page index 0 corresponds to HEAP_START
            self.init_block(0, max_order, false);
            self.add_to_free_list(0, max_order);
        }
    }

    /// Initialize block metadata
    fn init_block(&self, page_idx: usize, order: usize, free: bool) {
        let meta = self.meta.get_mut(page_idx);
        meta.order = order as u8;
        meta.free = if free { 1 } else { 0 };
        meta.prev = 0;
        meta.next = 0;
    }

    /// Add block to free list
    fn add_to_free_list(&self, page_idx: usize, order: usize) {
        // Boundary check
        if order > MAX_ORDER {
            return;  // Cannot handle blocks exceeding maximum order
        }

        {
            let meta = self.meta.get_mut(page_idx);
            meta.order = order as u8;
            meta.free = 1;
        }

        // Get current free list head
        let list_head = self.free_lists[order].load(Ordering::Acquire);

        // Insert block at list head
        if list_head != EMPTY_LIST && list_head < MAX_PAGES {
            self.meta.get_mut(list_head).prev = page_idx as u16;
        }
        {
            let meta = self.meta.get_mut(page_idx);
            meta.next = if list_head == EMPTY_LIST { 0xFFFF } else { list_head as u16 };
            meta.prev = 0xFFFF;  // 0xFFFF means null
        }

        // Update list head
        self.free_lists[order].store(page_idx, Ordering::Release);
    }

    /// Remove block from free list
    fn remove_from_free_list(&self, page_idx: usize, order: usize) {
        // Boundary check
        if order > MAX_ORDER {
            return;  // Cannot handle blocks exceeding maximum order
        }

        let prev_idx = self.meta.get(page_idx).prev as usize;
        let next_idx = self.meta.get(page_idx).next as usize;

        if prev_idx != 0xFFFF && prev_idx < MAX_PAGES {
            self.meta.get_mut(prev_idx).next = next_idx as u16;
        } else {
            // This is the list head, update global list head
            let new_head = if next_idx == 0xFFFF { EMPTY_LIST } else { next_idx };
            self.free_lists[order].store(new_head, Ordering::Release);
        }

        if next_idx != 0xFFFF && next_idx < MAX_PAGES {
            self.meta.get_mut(next_idx).prev = prev_idx as u16;
        }

        self.meta.get_mut(page_idx).free = 0;
    }

    /// Calculate order for heap size (O(1) bit manipulation)
    fn heap_size_to_order(&self, size: usize) -> usize {
        if size <= PAGE_SIZE {
            return 0;
        }
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
        if order > MAX_ORDER { MAX_ORDER } else { order }
    }

    /// Convert size to order (O(1) bit manipulation)
    fn size_to_order(&self, size: usize) -> usize {
        if size <= PAGE_SIZE {
            return 0;
        }
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
        if order > MAX_ORDER { MAX_ORDER } else { order }
    }

    /// Get buddy page index for a block
    fn get_buddy_idx(&self, page_idx: usize, order: usize) -> usize {
        let block_size_pages = 1usize << order;  // Number of pages in block
        page_idx ^ block_size_pages
    }

    /// Convert page index to address
    fn page_idx_to_addr(&self, page_idx: usize) -> usize {
        HEAP_START + page_idx * PAGE_SIZE
    }

    /// Convert address to page index
    fn addr_to_page_idx(&self, addr: usize) -> usize {
        (addr - HEAP_START) / PAGE_SIZE
    }

    /// Allocate memory
    fn alloc_blocks(&self, order: usize) -> *mut u8 {
        // Search starting from specified order
        for mut current_order in order..=MAX_ORDER {
            let list_head = self.free_lists[current_order].load(Ordering::Acquire);

            if list_head != EMPTY_LIST && list_head < MAX_PAGES {
                // Found free block
                self.remove_from_free_list(list_head, current_order);

                // Split block if needed
                let mut page_idx = list_head;
                while current_order > order {
                    let block_size_pages = 1usize << current_order;
                    let buddy_idx = page_idx + (block_size_pages / 2);

                    // Initialize buddy block and add to free list
                    self.init_block(buddy_idx, current_order - 1, true);
                    self.add_to_free_list(buddy_idx, current_order - 1);

                    // Update current block to first half
                    self.init_block(page_idx, current_order - 1, false);
                    current_order -= 1;
                }

                // Ensure final block is marked as allocated
                self.init_block(page_idx, order, false);

                let addr = self.page_idx_to_addr(page_idx);
                return addr as *mut u8;
            }
        }

        // Not enough memory
        core::ptr::null_mut()
    }

    /// Free memory
    unsafe fn free_blocks(&self, ptr: *mut u8, order: usize) {
        let addr = ptr as usize;
        let mut page_idx = self.addr_to_page_idx(addr);
        let mut current_order = order;

        loop {
            // Boundary check: order cannot exceed MAX_ORDER
            if current_order > MAX_ORDER {
                // Exceeds maximum order, add directly to MAX_ORDER list
                self.add_to_free_list(page_idx, MAX_ORDER);
                break;
            }

            let buddy_idx = self.get_buddy_idx(page_idx, current_order);

            // Check if buddy is in valid range
            if buddy_idx >= MAX_PAGES {
                // Buddy out of range, cannot merge
                self.add_to_free_list(page_idx, current_order);
                break;
            }

            // Check if buddy is free and size matches
            let buddy_meta = self.meta.get(buddy_idx);
            if buddy_meta.free == 0 || buddy_meta.order != current_order as u8 {
                // Buddy not free or size doesn't match, cannot merge
                self.add_to_free_list(page_idx, current_order);
                break;
            }

            // Buddy is free, remove from list
            self.remove_from_free_list(buddy_idx, current_order);

            // Merge: select smaller index as base address
            if page_idx > buddy_idx {
                page_idx = buddy_idx;
            }

            current_order += 1;
        }
    }
}

unsafe impl GlobalAlloc for BuddyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Check magic number and initialization state
        if self.magic.load(Ordering::Acquire) != 0xDEADBEEF
            || self.initialized.load(Ordering::Acquire) == 0
            || self.heap_start.load(Ordering::Acquire) == 0 {
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

        // Check if order exceeds MAX_ORDER
        if order > MAX_ORDER {
            return;
        }

        self.free_blocks(ptr, order);
    }
}

/// Global allocator (Buddy System)
/// Note: This is the only allocator instance, used for kernel heap allocation and #[global_allocator]
#[global_allocator]
pub static GLOBAL_ALLOCATOR: BuddyAllocator = BuddyAllocator::new();

/// Compatibility alias
pub use GLOBAL_ALLOCATOR as HEAP_ALLOCATOR;

pub fn init_heap() {
    GLOBAL_ALLOCATOR.init();
}

/// Buddy allocator statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct BuddyStats {
    /// Heap start address
    pub heap_start: usize,
    /// Heap end address
    pub heap_end: usize,
    /// Heap total size (bytes)
    pub heap_size: usize,
    /// Used size (bytes)
    pub used_bytes: usize,
    /// Free size (bytes)
    pub free_bytes: usize,
    /// Number of free blocks per order
    pub free_blocks: [usize; MAX_ORDER + 1],
    /// Total allocation count
    pub alloc_count: usize,
    /// Total free count
    pub free_count: usize,
}

/// Get Buddy allocator statistics
pub fn buddy_stats() -> BuddyStats {
    let mut stats = BuddyStats::default();

    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return stats;
    }

    stats.heap_start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
    stats.heap_end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);
    stats.heap_size = stats.heap_end - stats.heap_start;

    // Count free blocks per order
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

/// Combined allocator - prioritizes Slab for small objects, falls back to Buddy for large objects
///
/// This design can reduce memory fragmentation and improve small object allocation efficiency
pub struct CombinedAllocator;

unsafe impl GlobalAlloc for CombinedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();

        // Small objects (<= 4096 bytes) try Slab allocator
        if size <= 4096 && crate::mm::slab::is_slab_initialized() {
            let ptr = crate::mm::kmalloc(size);
            if !ptr.is_null() {
                return ptr;
            }
            // Slab allocation failed, fall back to Buddy
        }

        // Large objects or Slab failure uses Buddy allocator
        HEAP_ALLOCATOR.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }

        let ptr_addr = ptr as usize;
        let heap_start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
        let heap_end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);

        // Check if pointer is in Slab area
        // Slab area is after the heap
        let slab_start = heap_end;
        let slab_end = slab_start + 4 * 1024 * 1024; // 4MB slab

        if ptr_addr >= slab_start && ptr_addr < slab_end {
            // In Slab area, use kfree
            crate::mm::kfree(ptr);
        } else if ptr_addr >= heap_start && ptr_addr < heap_end {
            // In Buddy heap area
            HEAP_ALLOCATOR.dealloc(ptr, layout);
        }
        // Pointers in other regions are ignored
    }
}
