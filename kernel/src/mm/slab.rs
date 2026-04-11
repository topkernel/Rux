//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Slab Allocator
//!
//! Efficient memory allocation for small objects, reducing buddy allocator fragmentation.
//!
//! # Design
//! - SlabCache: Cache for managing specific size objects
//! - Slab: Memory pages containing multiple same-size objects
//! - kmalloc/kfree: Public allocation interface
//!
//! # Supported Object Sizes
//! 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 bytes

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::spinlock::Spinlock;

/// Page size
const PAGE_SIZE: usize = 4096;

/// Minimum object size (8 bytes)
const MIN_OBJECT_SIZE: usize = 8;

/// Maximum object size (one page)
const MAX_OBJECT_SIZE: usize = PAGE_SIZE;

/// Number of slab caches - from config
const NUM_CACHES: usize = crate::config::SLAB_NUM_CACHES;

/// Object size array
const OBJECT_SIZES: [usize; NUM_CACHES] = [
    8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
];

/// Slab state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlabState {
    /// Completely free
    Free,
    /// Partially used
    Partial,
    /// Completely used
    Full,
}

/// Slab header (stored at the beginning of each slab page)
#[repr(C)]
struct SlabHeader {
    /// Cache index this slab belongs to
    cache_idx: u8,
    /// Object size
    object_size: u16,
    /// Total object count
    total_objects: u16,
    /// Free object count
    free_objects: u16,
    /// First free object index
    free_index: u16,
    /// Next slab page index (in slab_pages array)
    next: u16,
    /// Previous slab page index
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

/// Slab cache
pub struct SlabCache {
    /// Object size
    object_size: usize,
    /// Objects per slab
    objects_per_slab: usize,
    /// Free slab list head (page index)
    free_list: u16,
    /// Partially used slab list head
    partial_list: u16,
    /// Fully used slab list head
    full_list: u16,
    /// Statistics: allocation count
    alloc_count: AtomicUsize,
    /// Statistics: free count
    free_count: AtomicUsize,
}

impl SlabCache {
    /// Create new slab cache
    pub const fn new(object_size: usize) -> Self {
        // Calculate objects per slab
        // Reserve header space
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

    /// Allocate an object from cache
    pub fn alloc(&mut self, slab_pages: &SlabPages) -> *mut u8 {
        // Prioritize allocation from partial list
        if self.partial_list != 0 {
            let ptr = self.alloc_from_slab(self.partial_list, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        // If partial is empty, allocate from free list
        if self.free_list != 0 {
            let slab_idx = self.free_list;
            // Move slab from free to partial
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

        // No available slab, need to create new one
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

    /// Allocate object from specified slab
    fn alloc_from_slab(&mut self, slab_idx: u16, slab_pages: &SlabPages) -> *mut u8 {
        let header = slab_pages.get_header_mut(slab_idx);

        if header.free_objects == 0 {
            return core::ptr::null_mut();
        }

        // Get free object index
        let obj_idx = header.free_index;

        // Calculate object address (checked to prevent usize overflow)
        let header_size = core::mem::size_of::<SlabHeader>();
        let obj_offset = match header_size.checked_add(
            (obj_idx as usize).checked_mul(self.object_size).unwrap_or(usize::MAX)
        ) {
            Some(off) => off,
            None => return core::ptr::null_mut(),
        };
        let page_addr = slab_pages.get_page_addr(slab_idx);
        let obj_ptr = (page_addr + obj_offset) as *mut u8;

        // Update header info
        // Read next free index (stored in object memory)
        // SAFETY: obj_ptr points within a slab page allocated from SlabPages;
        // we hold the cache lock so no concurrent alloc/free on this cache.
        let next_free = unsafe {
            if self.object_size >= 2 {
                *(obj_ptr as *const u16)
            } else {
                obj_idx + 1
            }
        };

        // Validate free-list index: detect corruption by checking the value is
        // either the sentinel (0xFFFF = end-of-list) or a valid object index.
        let max_objects = (PAGE_SIZE - header_size) / self.object_size;
        if next_free != 0xFFFF && next_free as usize >= max_objects {
            // free-list corrupted — treat slab as full to avoid out-of-bounds access
            header.free_objects = 0;
            self.move_slab_to_full(slab_idx, slab_pages);
            return obj_ptr;
        }

        header.free_index = next_free;
        header.free_objects -= 1;

        // Check if slab is now full
        if header.free_objects == 0 {
            // Move from partial to full
            self.move_slab_to_full(slab_idx, slab_pages);
        }

        obj_ptr
    }

    /// Free object to cache
    pub fn free(&mut self, ptr: *mut u8, slab_pages: &SlabPages) -> bool {
        // Find slab containing the object
        let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
        let slab_idx = match slab_pages.find_slab_by_addr(page_addr) {
            Some(idx) => idx,
            None => return false,
        };

        let header = slab_pages.get_header_mut(slab_idx);

        // Validate cache index
        if header.cache_idx as usize >= NUM_CACHES {
            return false;
        }

        // Calculate object index
        let header_size = core::mem::size_of::<SlabHeader>();
        let obj_offset = ptr as usize - page_addr - header_size;
        let obj_idx = (obj_offset / self.object_size) as u16;

        // Write object index to object memory (as free list)
        // SAFETY: ptr points within a slab page; we hold the cache lock.
        unsafe {
            if self.object_size >= 2 {
                *(ptr as *mut u16) = header.free_index;
            }
        }

        header.free_index = obj_idx;
        header.free_objects += 1;

        self.free_count.fetch_add(1, Ordering::Relaxed);

        // Check slab state change
        let was_full = header.free_objects == 1;
        let is_empty = header.free_objects == header.total_objects;

        if was_full {
            // Move from full to partial
            self.move_slab_from_full(slab_idx, slab_pages);
        } else if is_empty {
            // Move from partial to free (optional: release slab)
            // Keep in partial for now to avoid frequent create/destroy
        }

        true
    }

    /// Create new slab
    fn create_slab(&mut self, slab_pages: &SlabPages) -> Option<u16> {
        // Allocate a page from buddy allocator
        let page = slab_pages.alloc_page()?;

        // Initialize slab header
        let header = slab_pages.get_header_mut(page);
        header.cache_idx = 0; // Will be set after return
        header.object_size = self.object_size as u16;
        header.total_objects = self.objects_per_slab as u16;
        header.free_objects = self.objects_per_slab as u16;
        header.free_index = 0;
        header.next = 0;
        header.prev = 0;

        // Initialize free list (each object stores next free index)
        let header_size = core::mem::size_of::<SlabHeader>();
        let page_addr = slab_pages.get_page_addr(page);

        for i in 0..self.objects_per_slab - 1 {
            let obj_offset = header_size + i * self.object_size;
            let obj_ptr = (page_addr + obj_offset) as *mut u16;
            // SAFETY: obj_ptr is within a freshly allocated slab page; no
            // concurrent access — this slab is not yet in any list.
            unsafe {
                *obj_ptr = (i + 1) as u16;
            }
        }

        // Last object's next is 0xFFFF (list end marker)
        if self.objects_per_slab > 0 {
            let last_offset = header_size + (self.objects_per_slab - 1) * self.object_size;
            let last_ptr = (page_addr + last_offset) as *mut u16;
            unsafe {
                *last_ptr = 0xFFFF;
            }
        }

        Some(page)
    }

    /// Move slab from partial to full
    fn move_slab_to_full(&mut self, slab_idx: u16, slab_pages: &SlabPages) {
        // Remove from partial list
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

        // Add to full list head
        slab_pages.set_next(slab_idx, self.full_list);
        slab_pages.set_prev(slab_idx, 0);
        if self.full_list != 0 {
            slab_pages.set_prev(self.full_list, slab_idx);
        }
        self.full_list = slab_idx;
    }

    /// Move slab from full to partial
    fn move_slab_from_full(&mut self, slab_idx: u16, slab_pages: &SlabPages) {
        // Remove from full list
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

        // Add to partial list head
        slab_pages.set_next(slab_idx, self.partial_list);
        slab_pages.set_prev(slab_idx, 0);
        if self.partial_list != 0 {
            slab_pages.set_prev(self.partial_list, slab_idx);
        }
        self.partial_list = slab_idx;
    }
}

/// Slab page management
///
/// All methods take `&self` (not `&mut self`). Interior mutability is
/// achieved through:
///   - `allocated_pages`: AtomicUsize for lock-free page allocation
///   - `get_header_mut()`: returns raw `&mut SlabHeader` via pointer
///     arithmetic — each slab header lives at a unique page address,
///     and concurrent access to *different* pages is safe because the
///     underlying memory regions don't overlap.
///
/// This design allows `SlabPages` to be shared (`&self`) across CPUs
/// without a global lock, while per-cache spinlocks serialize access
/// to each cache's slab lists.
pub struct SlabPages {
    /// Slab page base address (immutable after init)
    base_addr: usize,
    /// Allocated page count (atomic for lock-free concurrent page allocation)
    allocated_pages: AtomicUsize,
    /// Maximum page count (immutable after init)
    max_pages: usize,
}

// Safety: SlabPages uses AtomicUsize for the only concurrently-modified field.
// All other fields are immutable after initialization. get_header_mut() returns
// pointers to non-overlapping page-aligned memory, so concurrent accesses to
// different slabs are safe.
unsafe impl Sync for SlabPages {}

impl SlabPages {
    pub const fn new(base_addr: usize, max_pages: usize) -> Self {
        Self {
            base_addr,
            allocated_pages: AtomicUsize::new(0),
            max_pages,
        }
    }

    /// Get base address
    pub fn base_addr(&self) -> usize {
        self.base_addr
    }

    /// Get max pages
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// Allocate a new page
    fn alloc_page(&self) -> Option<u16> {
        let idx = self.allocated_pages.fetch_add(1, Ordering::AcqRel);
        if idx >= self.max_pages {
            self.allocated_pages.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some((idx + 1) as u16) // Use 1-based index, 0 means null
    }

    /// Get page address
    fn get_page_addr(&self, idx: u16) -> usize {
        self.base_addr + (idx as usize - 1) * PAGE_SIZE
    }

    /// Get slab header
    fn get_header_mut(&self, idx: u16) -> &mut SlabHeader {
        let addr = self.get_page_addr(idx);
        // SAFETY: addr is page-aligned within the slab region; caller holds
        // cache lock ensuring exclusive access to this slab.
        unsafe { &mut *(addr as *mut SlabHeader) }
    }

    /// Get next slab
    fn get_next(&self, idx: u16) -> u16 {
        self.get_header_mut(idx).next
    }

    /// Set next slab
    fn set_next(&self, idx: u16, next: u16) {
        self.get_header_mut(idx).next = next;
    }

    /// Get previous slab
    fn get_prev(&self, idx: u16) -> u16 {
        self.get_header_mut(idx).prev
    }

    /// Set previous slab
    fn set_prev(&self, idx: u16, prev: u16) {
        self.get_header_mut(idx).prev = prev;
    }

    /// Find slab by address
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

/// Slab allocator global state.
///
/// All fields are `Sync`:
///   - `caches`: per-cache `Spinlock` — only one CPU can hold a given cache lock
///   - `pages`: `SlabPages` with `unsafe impl Sync` (atomic page counter, immutable fields)
///   - `initialized`: `AtomicUsize` — lock-free read
///
/// This allows `SLAB_ALLOCATOR` to be a plain `static` (no `mut`), eliminating
/// the `&mut` aliasing UB that existed when multiple CPUs simultaneously called
/// `kmalloc` for different cache indices.
pub struct SlabAllocator {
    /// Slab cache array
    caches: [Spinlock<SlabCache>; NUM_CACHES],
    /// Slab page management
    pages: SlabPages,
    /// Whether initialized
    initialized: AtomicUsize,
}

// Safety: all fields are individually Sync (see struct doc comment above).
unsafe impl Sync for SlabAllocator {}

/// Static Slab allocator instance (no `mut` — safe for concurrent access).
static SLAB_ALLOCATOR: SlabAllocator = SlabAllocator {
    caches: [
        Spinlock::new(SlabCache::new(8)),
        Spinlock::new(SlabCache::new(16)),
        Spinlock::new(SlabCache::new(32)),
        Spinlock::new(SlabCache::new(64)),
        Spinlock::new(SlabCache::new(128)),
        Spinlock::new(SlabCache::new(256)),
        Spinlock::new(SlabCache::new(512)),
        Spinlock::new(SlabCache::new(1024)),
        Spinlock::new(SlabCache::new(2048)),
        Spinlock::new(SlabCache::new(4096)),
    ],
    pages: SlabPages::new(0, 0),
    initialized: AtomicUsize::new(0),
};

impl SlabAllocator {
    /// Initialize Slab allocator.
    ///
    /// # Safety
    /// `init()` must be called exactly once, before any concurrent `kmalloc` /
    /// `kfree` calls.  It writes through a raw pointer to initialize the
    /// `SlabPages` fields that are immutable afterwards.  No other CPU may
    /// access `SLAB_ALLOCATOR` concurrently during this call.
    pub fn init(base_addr: usize, max_pages: usize) {
        // SAFETY: init() is called once during boot before secondary CPUs start
        // and before any kmalloc/kfree call.  We write through a raw pointer to
        // initialize the SlabPages fields that are immutable after this point.
        unsafe {
            let allocator_ptr = &SLAB_ALLOCATOR as *const SlabAllocator as *mut SlabAllocator;
            (*allocator_ptr).pages = SlabPages::new(base_addr, max_pages);
        }
        SLAB_ALLOCATOR.initialized.store(1, Ordering::Release);
    }

    /// Check if initialized
    fn is_initialized() -> bool {
        SLAB_ALLOCATOR.initialized.load(Ordering::Acquire) == 1
    }

    /// Find appropriate cache index
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

/// Allocate memory
///
/// # Arguments
/// - `size`: Requested memory size
///
/// # Returns
/// Memory pointer on success, null on failure
pub fn kmalloc(size: usize) -> *mut u8 {
    if !SlabAllocator::is_initialized() {
        return core::ptr::null_mut();
    }

    // Find appropriate cache
    let cache_idx = match SlabAllocator::find_cache_index(size) {
        Some(idx) => idx,
        None => return core::ptr::null_mut(),
    };

    // Use lock_irqsave: kmalloc can be called from interrupt context,
    // and an interrupt firing while we hold the slab lock would self-deadlock
    // if the handler also allocates memory.
    let mut cache = SLAB_ALLOCATOR.caches[cache_idx].lock_irqsave();
    let ptr = cache.alloc(&SLAB_ALLOCATOR.pages);
    if !ptr.is_null() {
        // Store cache_idx in slab header for O(1) kfree lookup
        let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
        let header = page_addr as *mut SlabHeader;
        // SAFETY: page_addr is page-aligned and within the slab region;
        // we hold the cache lock.
        unsafe {
            (*header).cache_idx = cache_idx as u8;
        }
    }
    ptr
}

/// Free memory
///
/// # Arguments
/// - `ptr`: Memory pointer to free
pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() || !SlabAllocator::is_initialized() {
        return;
    }

    // O(1) lookup: read cache_idx from slab header
    let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
    let base = SLAB_ALLOCATOR.pages.base_addr();
    let max_pages = SLAB_ALLOCATOR.pages.max_pages();
    let slab_end = base + max_pages * PAGE_SIZE;

    // Check if pointer is within slab region
    if page_addr >= base && page_addr < slab_end {
        let header = page_addr as *const SlabHeader;
        // SAFETY: page_addr is within the slab region (checked above); slab
        // header was written by kmalloc and is stable.
        let idx = unsafe { (*header).cache_idx as usize };
        if idx < NUM_CACHES {
            let mut cache = SLAB_ALLOCATOR.caches[idx].lock_irqsave();
            if cache.free(ptr, &SLAB_ALLOCATOR.pages) {
                return;
            }
        }
    }

    // Fallback: linear search for non-slab pointers or corrupted header
    for i in 0..NUM_CACHES {
        let mut cache = SLAB_ALLOCATOR.caches[i].lock_irqsave();
        if cache.free(ptr, &SLAB_ALLOCATOR.pages) {
            return;
        }
    }
}

/// Allocate and zero memory
///
/// # Arguments
/// - `size`: Requested memory size
///
/// # Returns
/// Zeroed memory pointer on success, null on failure
pub fn kzalloc(size: usize) -> *mut u8 {
    let ptr = kmalloc(size);
    if !ptr.is_null() {
        // SAFETY: ptr was returned by kmalloc, size matches the allocation.
        unsafe {
            core::ptr::write_bytes(ptr, 0, size);
        }
    }
    ptr
}

/// Initialize Slab allocator
///
/// # Arguments
/// - `base_addr`: Slab memory region start address
/// - `size`: Slab memory region size
pub fn init_slab(base_addr: usize, size: usize) {
    let max_pages = size / PAGE_SIZE;
    SlabAllocator::init(base_addr, max_pages);
}

/// Check if Slab allocator is initialized
pub fn is_slab_initialized() -> bool {
    SlabAllocator::is_initialized()
}

/// Get Slab statistics
pub fn slab_stats() -> SlabStats {
    let mut stats = SlabStats::default();

    if !SlabAllocator::is_initialized() {
        return stats;
    }

    // Use lock_irqsave: interrupts must be disabled while holding slab locks.
    // A timer interrupt firing mid-lock can invoke kmalloc (which uses
    // lock_irqsave on the same cache), causing a self-deadlock on the
    // current CPU since the IRQ handler spins forever waiting for the
    // lock held by the preempted context.
    for i in 0..NUM_CACHES {
        let mut cache = SLAB_ALLOCATOR.caches[i].lock_irqsave();
        stats.cache_stats[i] = CacheStats {
            object_size: cache.object_size,
            alloc_count: cache.alloc_count.load(Ordering::Relaxed),
            free_count: cache.free_count.load(Ordering::Relaxed),
        };
    }
    stats.total_pages = SLAB_ALLOCATOR.pages.allocated_pages.load(Ordering::Relaxed);

    stats
}

/// Cache statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub object_size: usize,
    pub alloc_count: usize,
    pub free_count: usize,
}

/// Slab statistics
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
