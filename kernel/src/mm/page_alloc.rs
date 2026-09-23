//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Physical Page Buddy Allocator
//!
//! This module implements a unified buddy system for physical page allocation.
//! It provides APIs like __get_free_pages() and free_pages().

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};

use super::PAGE_SIZE;
use super::zone::{Zone, ZoneType, GfpFlags, MAX_ORDER, pfn_to_phys, phys_to_pfn, WMARK_LOW};
use super::page_desc::{Page, pfn_to_page, pfn_to_page_mut};
use super::pglist::{first_online_node_mut, node_data_mut, init_node_data};

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
    // SAFETY: early boot or under allocator lock — exclusive node access.
    if let Some(node) = unsafe { first_online_node_mut() } {
        let zone_type = gfp_flags.zone_type();
        if let Some(zone) = node.zone_mut(zone_type) {
            if zone.is_initialized() {
                if let Some(pfn) = zone.alloc_pages(order) {
                    ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                    // Page descriptors (refcount/mapcount/flags/order) were
                    // initialized by zone.alloc_pages INSIDE the zone lock —
                    // doing it here used to leave a lock-free window where
                    // the pages still looked free (review MM-H2).
                    return pfn_to_phys(pfn);
                }
                // Zone allocator failed — wake kswapd if below low watermark
                if !zone.watermark_ok(order, WMARK_LOW) {
                    super::kswapd::wakeup_kswapd(order as i32);
                }

                // High-order allocation failed: try compaction to reduce fragmentation
                if order > 0 {
                    // Convert to raw pointer before passing to compact_zone.
                    // Use raw pointer for the subsequent alloc_pages call too,
                    // avoiding the aliasing UB of re-creating &mut from a raw
                    // pointer while the original &mut binding is still live.
                    let zone_ptr: *mut Zone = zone;
                    // SAFETY: zone_ptr is a valid pointer to an initialized zone.
                    let cr = unsafe { super::compact::compact_zone(zone_ptr, order) };
                    if matches!(cr, super::compact::CompactResult::Success) {
                        // SAFETY: compact_zone does not destroy the zone; the
                        // pointer remains valid for the lifetime of the node.
                        // Using raw pointer method call avoids creating a new &mut.
                        let pfn = unsafe { (*zone_ptr).alloc_pages(order) };
                        if let Some(pfn) = pfn {
                            ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                            // Descriptors initialized under the zone lock.
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
        // SAFETY: addr is a valid physical address just allocated by alloc_page.
        // phys_to_virt converts it to the corresponding virtual address in the
        // kernel linear mapping region, which is safe to write to.
        // Writing PAGE_SIZE bytes is within the allocated page.
        let virt = crate::arch::riscv64::mm::phys_to_virt(
            crate::arch::riscv64::mm::PhysAddr::new(addr as u64),
        );
        unsafe {
            core::ptr::write_bytes(virt.0 as *mut u8, 0, PAGE_SIZE);
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


    // NOTE: descriptor reset (refcount→0 etc.) happens inside zone.free_pages
    // under the zone lock — resetting here, before the lock, used to expose a
    // window where the block looked free and could be double-allocated
    // (review MM-H2).

    // Try to free to the Zone system first
    // SAFETY: exclusive node access — caller must ensure no concurrent mutation.
    if let Some(node) = unsafe { first_online_node_mut() } {
        // Try each zone type to find which one contains this PFN.
        // ZoneMovable included (review 4.7): pages allocated with
        // __GFP_MOVABLE live there and silently leaked when their free
        // fell through this list.
        for zone_type in [
            ZoneType::ZoneNormal,
            ZoneType::ZoneMovable,
            ZoneType::ZoneDma32,
            ZoneType::ZoneDma,
        ] {
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

/// Get the page descriptor for a kernel VIRTUAL address (linear mapping).
///
/// Review 4.x: the old implementation treated the argument as a PHYSICAL
/// address (`addr / PAGE_SIZE`) while every caller-visible name says
/// "virt" — for any kernel-linear VA the PFN came out ~PAGE_OFFSET/PAGE_SIZE
/// too large, yielding a wild descriptor. Now the address is properly
/// translated through virt_to_phys() (identity-mapped legacy region
/// included); addresses outside both fall through to virt_to_phys's
/// warned identity behavior.
pub fn virt_to_page(addr: usize) -> *mut Page {
    pfn_to_page_mut(virt_to_pfn(addr))
}

/// Get page frame number from a kernel virtual address.
pub fn virt_to_pfn(addr: usize) -> usize {
    let phys = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(addr as u64),
    );
    (phys.bits() as usize) / PAGE_SIZE
}

/// Get physical address from page descriptor
pub fn page_to_phys(page: &Page) -> usize {
    super::page_desc::page_to_pfn(page) * PAGE_SIZE
}

/// Get the kernel-linear virtual address of a page described by `page`.
pub fn page_to_virt(page: &Page) -> usize {
    let phys = page_to_phys(page);
    crate::arch::riscv64::mm::phys_to_virt(
        crate::arch::riscv64::mm::PhysAddr::new(phys as u64),
    )
    .bits() as usize
}

// ==================== Dead-code removal note (review 4.7) ====================
// The THIRD buddy implementation that used to live here (struct
// BuddyAllocator + static KERNEL_BUDDY + init_kernel_buddy/buddy_alloc/
// buddy_free/buddy_stats) had ZERO callers: the kernel heap uses
// mm/buddy_allocator.rs (GLOBAL_ALLOCATOR) and physical pages go through
// the zone system in this file. Three coexisting buddy implementations
// tripled the audit surface for the same invariants; this one is deleted.
// The remaining two: zone.rs (Zone::alloc_pages/free_pages) and
// buddy_allocator.rs (#[global_allocator]).
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
    // SAFETY: Called once during early boot before any page allocation.
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
/// # Safety
/// Caller must ensure the returned physical address is used only by the kernel
/// and freed exactly once via `__free_pages` with the correct order.
pub unsafe fn __get_free_pages(gfp_flags: GfpFlags, order: usize) -> usize {
    alloc_pages(gfp_flags, order)
}

/// __get_free_page - Allocate a single page
///
/// # Safety
/// Caller must ensure the returned physical address is used only by the kernel
/// and freed exactly once via `__free_page`.
pub unsafe fn __get_free_page(gfp_flags: GfpFlags) -> usize {
    alloc_page(gfp_flags)
}

/// __get_zeroed_page - Allocate and zero a page
///
/// # Safety
/// Caller must ensure the returned physical address is used only by the kernel
/// and freed exactly once via `__free_page`.
pub unsafe fn __get_zeroed_page(gfp_flags: GfpFlags) -> usize {
    get_zeroed_page(gfp_flags)
}

/// __free_pages - Free pages
///
/// # Safety
/// `addr` must be a physical address previously returned by `__get_free_pages`
/// with the same `order`, and must not have been freed already.
pub unsafe fn __free_pages(addr: usize, order: usize) {
    free_pages(addr, order);
}

/// __free_page - Free a single page
///
/// # Safety
/// `addr` must be a physical address previously returned by `__get_free_page`
/// or `__get_zeroed_page`, and must not have been freed already.
pub unsafe fn __free_page(addr: usize) {
    free_page(addr);
}
