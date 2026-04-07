//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Physical Page Buddy Allocator
//!
//! This module implements a unified buddy system for physical page allocation.
//! It provides APIs like __get_free_pages() and free_pages().

extern crate alloc;

use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use crate::sync::spinlock::Spinlock;

use super::PAGE_SIZE;
use super::zone::{Zone, ZoneType, GfpFlags, MAX_ORDER, FREE_LIST_NULL, pfn_to_phys, phys_to_pfn, WMARK_LOW};
use super::page_desc::{Page, PageFlag, pfn_to_page, pfn_to_page_mut};
use super::pglist::{first_online_node_mut, node_data_mut, init_node_data};
use super::memblock::memblock_is_reserved;

// ==================== Page Allocation API ====================

/// Allocation statistics for debugging
static ZONE_ALLOCS: AtomicUsize = AtomicUsize::new(0);
static LEGACY_ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// Allocate 2^order contiguous physical pages
///
/// # Arguments
/// - `gfp_flags`: GFP flags controlling allocation behavior
/// - `order`: Order of allocation (2^order pages)
///
/// # Returns
/// - Physical address of the first page, or 0 if allocation fails
pub fn alloc_pages(gfp_flags: GfpFlags, order: usize) -> usize {
    if order > MAX_ORDER {
        return 0;
    }

    // Try to allocate from the Zone system first
    if let Some(node) = first_online_node_mut() {
        let zone_type = gfp_flags.zone_type();
        if let Some(zone) = node.zone_mut(zone_type) {
            if zone.is_initialized() {
                if let Some(pfn) = zone.alloc_pages(order) {
                    ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                    // Update page descriptor
                    let page = pfn_to_page_mut(pfn);
                    if !page.is_null() {
                        unsafe {
                            (*page).set_refcount(1);
                            (*page).set_order(order as u8);
                            (*page).set_flag(PageFlag::Referenced);
                        }
                    }
                    return pfn_to_phys(pfn);
                }
                // Zone allocator failed — wake kswapd if below low watermark
                if !zone.watermark_ok(order, WMARK_LOW) {
                    super::kswapd::wakeup_kswapd(order as i32);
                }

                // High-order allocation failed: try compaction to reduce fragmentation
                if order > 0 {
                    let cr = unsafe { super::compact::compact_zone(zone as *mut Zone, order) };
                    if matches!(cr, super::compact::CompactResult::Success) {
                        if let Some(pfn) = zone.alloc_pages(order) {
                            ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                            let page = pfn_to_page_mut(pfn);
                            if !page.is_null() {
                                unsafe {
                                    (*page).set_refcount(1);
                                    (*page).set_order(order as u8);
                                    (*page).set_flag(PageFlag::Referenced);
                                }
                            }
                            return pfn_to_phys(pfn);
                        }
                    }
                }

                return 0;
            }
        }
    }

    // Zone system not initialized yet, use memblock
    // This should only happen during early boot before zone is set up
    LEGACY_ALLOCS.fetch_add(1, Ordering::Relaxed);
    super::memblock::memblock_phys_alloc().unwrap_or(0)
}

/// Allocate a single page
pub fn alloc_page(gfp_flags: GfpFlags) -> usize {
    alloc_pages(gfp_flags, 0)
}

/// Allocate a page and zero it
pub fn get_zeroed_page(gfp_flags: GfpFlags) -> usize {
    let addr = alloc_page(gfp_flags);
    if addr != 0 {
        // Zero the page
        unsafe {
            let ptr = addr as *mut u8;
            for i in 0..PAGE_SIZE {
                *ptr.add(i) = 0;
            }
        }
    }
    addr
}

/// Free contiguous physical pages
///
/// # Arguments
/// - `addr`: Physical address of the first page
/// - `order`: Order of the allocation
pub fn free_pages(addr: usize, order: usize) {
    if addr == 0 {
        return;
    }

    let pfn = phys_to_pfn(addr);

    // Update page descriptor
    let page = pfn_to_page(pfn);
    if !page.is_null() {
        unsafe {
            (*page).set_refcount(0);
            (*page).clear_flag(PageFlag::Referenced);
        }
    }

    // Try to free to the Zone system first
    if let Some(node) = first_online_node_mut() {
        // Try each zone type to find which one contains this PFN
        for zone_type in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
            if let Some(zone) = node.zone_mut(zone_type) {
                if zone.is_initialized() {
                    let start_pfn = zone.start_pfn();
                    let end_pfn = zone.end_pfn();
                    if pfn >= start_pfn && pfn < end_pfn {
                        zone.free_pages(pfn, order);
                        return;
                    }
                }
            }
        }
    }

    // Page doesn't belong to any zone - this shouldn't happen in normal operation
    // Pages allocated via memblock during early boot are not tracked and don't need freeing
}

/// Free a single page
pub fn free_page(addr: usize) {
    free_pages(addr, 0);
}

// ==================== Page Helper Functions ====================

/// Get the page descriptor for an address
pub fn virt_to_page(addr: usize) -> *mut Page {
    // For identity-mapped kernel addresses
    let phys = addr;  // Identity mapping
    let pfn = phys / PAGE_SIZE;
    pfn_to_page_mut(pfn)
}

/// Get page frame number from address
pub fn virt_to_pfn(addr: usize) -> usize {
    addr / PAGE_SIZE
}

/// Get physical address from page descriptor
pub fn page_to_phys(page: &Page) -> usize {
    super::page_desc::page_to_pfn(page) * PAGE_SIZE
}

/// Get virtual address from page descriptor (identity mapped)
pub fn page_to_virt(page: &Page) -> usize {
    page_to_phys(page)
}

// ==================== Buddy Allocator Implementation ====================

/// Buddy allocator for a memory region
pub struct BuddyAllocator {
    /// Start PFN
    start_pfn: AtomicUsize,

    /// End PFN (exclusive)
    end_pfn: AtomicUsize,

    /// Free lists for each order
    free_lists: [AtomicUsize; MAX_ORDER + 1],

    /// Number of free pages per order
    free_counts: [AtomicUsize; MAX_ORDER + 1],

    /// Total free pages
    total_free: AtomicUsize,

    /// Lock for buddy operations
    lock: Spinlock<()>,

    /// Initialized flag
    initialized: AtomicBool,
}

impl BuddyAllocator {
    /// Create a new uninitialized buddy allocator
    pub const fn new() -> Self {
        Self {
            start_pfn: AtomicUsize::new(0),
            end_pfn: AtomicUsize::new(0),
            free_lists: [const { AtomicUsize::new(FREE_LIST_NULL) }; MAX_ORDER + 1],
            free_counts: [const { AtomicUsize::new(0) }; MAX_ORDER + 1],
            total_free: AtomicUsize::new(0),
            lock: Spinlock::new(()),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialize the buddy allocator with a memory region
    ///
    /// # Arguments
    /// - `start_pfn`: Start page frame number
    /// - `nr_pages`: Number of pages in the region
    pub fn init(&self, start_pfn: usize, nr_pages: usize) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        let _guard = self.lock.lock();

        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        let end_pfn = start_pfn + nr_pages;
        self.start_pfn.store(start_pfn, Ordering::Release);
        self.end_pfn.store(end_pfn, Ordering::Release);

        // Add all pages to the appropriate free list
        // Find the largest order that fits
        let mut remaining = nr_pages;
        let mut current_pfn = start_pfn;

        while remaining > 0 {
            // Find highest order that fits and is aligned
            let mut order = 0;
            for o in (0..=MAX_ORDER).rev() {
                let block_size = 1usize << o;
                // Check alignment and size
                if current_pfn % block_size == 0 && remaining >= block_size {
                    order = o;
                    break;
                }
            }

            // Add block to free list
            self.add_to_free_list(current_pfn, order);

            current_pfn += 1usize << order;
            remaining -= 1usize << order;
        }

        self.total_free.store(nr_pages, Ordering::Release);
        self.initialized.store(true, Ordering::Release);
    }

    /// Allocate pages
    pub fn alloc(&self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        let _guard = self.lock.lock();

        // Find a free block at this order or higher
        for current_order in order..=MAX_ORDER {
            let head = self.free_lists[current_order].load(Ordering::Acquire);
            if head != FREE_LIST_NULL {
                // Found a block, remove it
                self.remove_from_free_list(head, current_order);

                let mut pfn = head;
                let mut o = current_order;

                // Split block down to target order
                while o > order {
                    o -= 1;
                    let buddy_pfn = pfn + (1usize << o);
                    self.add_to_free_list(buddy_pfn, o);
                }

                // Update free count
                self.total_free.fetch_sub(1usize << order, Ordering::Relaxed);

                return Some(pfn);
            }
        }

        None
    }

    /// Free pages
    pub fn free(&self, pfn: usize, order: usize) {
        if order > MAX_ORDER {
            return;
        }

        // Validate PFN range
        let start = self.start_pfn.load(Ordering::Acquire);
        let end = self.end_pfn.load(Ordering::Acquire);
        if pfn < start || pfn >= end {
            return;
        }

        let _guard = self.lock.lock();

        let mut current_pfn = pfn;
        let mut current_order = order;

        // Try to merge with buddy
        while current_order < MAX_ORDER {
            let buddy_pfn = current_pfn ^ (1usize << current_order);

            // Check if buddy is free
            if !self.is_buddy_free(buddy_pfn, current_order) {
                break;
            }

            // Remove buddy from free list
            self.remove_from_free_list(buddy_pfn, current_order);

            // Merge: take lower address
            current_pfn = current_pfn.min(buddy_pfn);
            current_order += 1;
        }

        // Add merged block to free list
        self.add_to_free_list(current_pfn, current_order);

        // Update free count
        self.total_free.fetch_add(1usize << order, Ordering::Relaxed);
    }

    /// Add block to free list
    fn add_to_free_list(&self, pfn: usize, order: usize) {
        let head = self.free_lists[order].load(Ordering::Acquire);

        // Update Page descriptor's free list pointers
        let page = pfn_to_page_mut(pfn);
        if !page.is_null() {
            unsafe {
                (*page).set_next_free(head);
                (*page).set_order(order as u8);
            }
        }

        // Update head's prev pointer
        if head != FREE_LIST_NULL {
            let head_page = pfn_to_page_mut(head);
            if !head_page.is_null() {
                unsafe {
                    // In a full implementation, we'd set prev pointer here
                }
            }
        }

        // Set new head
        self.free_lists[order].store(pfn, Ordering::Release);
        self.free_counts[order].fetch_add(1, Ordering::Relaxed);
    }

    /// Remove block from free list
    fn remove_from_free_list(&self, pfn: usize, order: usize) {
        let head = self.free_lists[order].load(Ordering::Acquire);

        if head == pfn {
            // Removing head, get next from Page descriptor
            let next = {
                let page = pfn_to_page(pfn);
                if page.is_null() {
                    FREE_LIST_NULL
                } else {
                    unsafe { (*page).next_free() }
                }
            };
            self.free_lists[order].store(next, Ordering::Release);
        }

        self.free_counts[order].fetch_sub(1, Ordering::Relaxed);
    }

    /// Check if buddy is free
    fn is_buddy_free(&self, buddy_pfn: usize, order: usize) -> bool {
        // Check range
        let start = self.start_pfn.load(Ordering::Acquire);
        let end = self.end_pfn.load(Ordering::Acquire);
        if buddy_pfn < start || buddy_pfn >= end {
            return false;
        }

        // Check if buddy is in the free list
        // For simplicity, check if it has the correct order
        let page = pfn_to_page(buddy_pfn);
        if page.is_null() {
            return false;
        }

        unsafe {
            let page_order = (*page).order();
            page_order == order as u8 && (*page).is_free()
        }
    }

    /// Get total free pages
    pub fn nr_free(&self) -> usize {
        self.total_free.load(Ordering::Acquire)
    }

    /// Get free pages at specific order
    pub fn nr_free_order(&self, order: usize) -> usize {
        if order > MAX_ORDER {
            0
        } else {
            self.free_counts[order].load(Ordering::Acquire)
        }
    }

    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

// ==================== Global Allocator ====================

/// Global buddy allocator for kernel pages
static KERNEL_BUDDY: BuddyAllocator = BuddyAllocator::new();

/// Initialize the kernel buddy allocator
pub fn init_kernel_buddy(start_pfn: usize, nr_pages: usize) {
    KERNEL_BUDDY.init(start_pfn, nr_pages);
}

/// Allocate from kernel buddy allocator
pub fn buddy_alloc(order: usize) -> Option<usize> {
    KERNEL_BUDDY.alloc(order)
}

/// Free to kernel buddy allocator
pub fn buddy_free(pfn: usize, order: usize) {
    KERNEL_BUDDY.free(pfn, order);
}

/// Get kernel buddy allocator statistics
pub fn buddy_stats() -> BuddyStats {
    let mut free_blocks = [0; MAX_ORDER + 1];
    for i in 0..=MAX_ORDER {
        free_blocks[i] = KERNEL_BUDDY.nr_free_order(i);
    }

    BuddyStats {
        start_pfn: KERNEL_BUDDY.start_pfn.load(Ordering::Acquire),
        end_pfn: KERNEL_BUDDY.end_pfn.load(Ordering::Acquire),
        total_free: KERNEL_BUDDY.nr_free(),
        free_blocks,
    }
}

/// Buddy allocator statistics
#[derive(Debug, Clone, Copy)]
pub struct BuddyStats {
    pub start_pfn: usize,
    pub end_pfn: usize,
    pub total_free: usize,
    pub free_blocks: [usize; MAX_ORDER + 1],
}

// ==================== Zone System Initialization ====================

/// Initialize the zone system with physical memory
///
/// This replaces the separate user_phys_allocator with a unified zone system.
/// All physical page allocation should go through the zone system after this.
///
/// # Arguments
/// - `phys_start`: Physical memory start address
/// - `phys_size`: Total physical memory size in bytes
/// - `kernel_end`: End of kernel memory (where allocation can start)
pub fn init_zone_system(phys_start: usize, phys_size: usize, kernel_end: usize) {
    // Initialize node data structure
    unsafe {
        init_node_data();
    }

    // Get mutable node
    let node = match node_data_mut(0) {
        Some(n) => n,
        None => {
            crate::println!("page_alloc: Failed to get node 0 for zone initialization");
            return;
        }
    };

    // Validate phys_size against MAX_PAGES
    // This ensures zone allocations stay within vmemmap bounds
    let max_pages = super::page_desc::MAX_PAGES;
    let max_phys_size = max_pages * PAGE_SIZE;
    let effective_phys_size = phys_size.min(max_phys_size);
    if effective_phys_size != phys_size {
        crate::println!("page_alloc: phys_size {}MB exceeds MAX_PAGES limit {}MB, truncating",
            phys_size / (1024 * 1024), max_phys_size / (1024 * 1024));
    }

    // Initialize node with total memory range
    let start_pfn = phys_start / PAGE_SIZE;
    let total_pages = effective_phys_size / PAGE_SIZE;
    node.init(start_pfn, total_pages, total_pages);

    // Create ZONE_NORMAL for all allocatable memory
    // On RISC-V, we don't need DMA zones, but we'll use ZONE_NORMAL
    let alloc_start_pfn = (kernel_end / PAGE_SIZE).max(start_pfn);
    let alloc_end_pfn = start_pfn + total_pages;
    let alloc_start = alloc_start_pfn * PAGE_SIZE;
    let alloc_end = alloc_end_pfn * PAGE_SIZE;

    // Create and initialize ZONE_NORMAL
    let mut zone = Zone::new(ZoneType::ZoneNormal, 0, 0);
    zone.init(alloc_start_pfn, alloc_end_pfn);

    // Add pages to the zone's buddy allocator
    // Use memblock_for_each_free_range to iterate over FREE memory ranges only
    // Free all memblock-reserved pages to the buddy allocator
    let mut total_added = 0usize;

    super::memblock::memblock_for_each_free_range(alloc_start, alloc_end, |free_start, free_end| {
        // Convert to PFNs
        let range_start_pfn = free_start / PAGE_SIZE;
        let range_end_pfn = free_end / PAGE_SIZE;
        let range_pages = range_end_pfn.saturating_sub(range_start_pfn);

        if range_pages == 0 {
            return;
        }

        // Add pages in this free range to the zone
        let mut remaining = range_pages;
        let mut current_pfn = range_start_pfn;

        while remaining > 0 {
            // Find highest order that fits and is aligned
            let mut order = 0;
            for o in (0..=MAX_ORDER).rev() {
                let block_size = 1usize << o;
                if current_pfn % block_size == 0 && remaining >= block_size {
                    order = o;
                    break;
                }
            }

            // Add directly to zone's free list without buddy merging
            // During initialization, we know pages are not in any list yet
            zone.add_to_free_list_init(current_pfn, order);
            total_added += 1usize << order;

            current_pfn += 1usize << order;
            remaining -= 1usize << order;
        }
    });

    // Add zone to node
    node.add_zone(ZoneType::ZoneNormal, zone);

    // Setup per-zone watermarks
    if let Some(zone_ref) = node.zone(ZoneType::ZoneNormal) {
        let total_managed = zone_ref.managed_pages();
        let refs: [&Zone; 1] = [zone_ref];
        super::zone::setup_per_zone_wmarks(&refs, total_managed);
    }
}

// ==================== Page Allocation APIs ====================

/// __get_free_pages - Allocate contiguous pages
///
/// Page allocation API
pub unsafe fn __get_free_pages(gfp_flags: GfpFlags, order: usize) -> usize {
    alloc_pages(gfp_flags, order)
}

/// __get_free_page - Allocate a single page
pub unsafe fn __get_free_page(gfp_flags: GfpFlags) -> usize {
    alloc_page(gfp_flags)
}

/// __get_zeroed_page - Allocate and zero a page
pub unsafe fn __get_zeroed_page(gfp_flags: GfpFlags) -> usize {
    get_zeroed_page(gfp_flags)
}

/// __free_pages - Free pages
pub unsafe fn __free_pages(addr: usize, order: usize) {
    free_pages(addr, order);
}

/// __free_page - Free a single page
pub unsafe fn __free_page(addr: usize) {
    free_page(addr);
}
