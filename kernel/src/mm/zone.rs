//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Zone Allocator Infrastructure
//!
//! This module implements Linux-style memory zones for physical page management.
//! Zones are used to group pages with similar characteristics or constraints.

extern crate alloc;

use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use alloc::string::ToString;
use spin::Mutex;

use super::PAGE_SIZE;
use super::page_desc::{pfn_to_page, pfn_to_page_mut};

// ==================== Zone Type Definitions ====================

/// Zone types (Linux-compatible)
///
/// On RISC-V, we typically only need ZONE_NORMAL since there are no
/// DMA constraints like on x86. However, we define all zones for
/// completeness and future compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum ZoneType {
    /// Zone for DMA-constrained devices (0-16MB on x86)
    /// Not typically needed on RISC-V, but included for compatibility
    ZoneDma = 0,

    /// Zone for 32-bit DMA devices (0-4GB on 64-bit systems)
    /// May be needed for some RISC-V SoCs with 32-bit DMA limitations
    ZoneDma32 = 1,

    /// Normal zone for all general-purpose memory
    /// This is the primary zone used for most allocations on RISC-V
    ZoneNormal = 2,

    /// Zone for movable pages (can be migrated/compacted)
    /// Used for memory hotplug and compaction
    ZoneMovable = 3,

    /// Number of zone types
    ZoneCount = 4,
}

impl ZoneType {
    /// Get zone name as string
    pub fn name(&self) -> &'static str {
        match self {
            ZoneType::ZoneDma => "DMA",
            ZoneType::ZoneDma32 => "DMA32",
            ZoneType::ZoneNormal => "Normal",
            ZoneType::ZoneMovable => "Movable",
            ZoneType::ZoneCount => "Invalid",
        }
    }

    /// Get zone index
    pub fn index(&self) -> usize {
        *self as usize
    }
}

// ==================== Free Area (Buddy System) ====================

/// Maximum order for buddy allocator (2^MAX_ORDER pages = 4MB with 4KB pages)
pub const MAX_ORDER: usize = 10;

/// Null pointer for free list
pub const FREE_LIST_NULL: usize = usize::MAX;

/// Free area for one order level
pub struct FreeArea {
    /// Head of free list (stores PFN of first free block)
    free_list: AtomicUsize,
    /// Number of free blocks at this order
    nr_free: AtomicUsize,
}

impl FreeArea {
    pub const fn new() -> Self {
        Self {
            free_list: AtomicUsize::new(FREE_LIST_NULL),
            nr_free: AtomicUsize::new(0),
        }
    }

    /// Check if free list is empty
    pub fn is_empty(&self) -> bool {
        self.free_list.load(Ordering::Acquire) == FREE_LIST_NULL
    }

    /// Get number of free blocks
    pub fn nr_free(&self) -> usize {
        self.nr_free.load(Ordering::Acquire)
    }

    /// Increment free count
    pub fn inc_free(&self) {
        self.nr_free.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement free count
    pub fn dec_free(&self) {
        self.nr_free.fetch_sub(1, Ordering::Relaxed);
    }
}

// ==================== Zone Structure ====================

/// Memory zone
///
/// Represents a contiguous range of physical memory pages with
/// similar characteristics. Each zone has its own buddy allocator.
pub struct Zone {
    /// Zone type
    zone_type: ZoneType,

    /// Zone ID (index in node's zone array)
    zone_id: usize,

    /// Node ID (NUMA node this zone belongs to)
    node_id: usize,

    /// Start page frame number (PFN)
    zone_start_pfn: AtomicUsize,

    /// End page frame number (exclusive)
    zone_end_pfn: AtomicUsize,

    /// Total number of pages in the zone
    spanned_pages: AtomicUsize,

    /// Number of present pages (excluding holes)
    present_pages: AtomicUsize,

    /// Number of managed pages (available for allocation)
    managed_pages: AtomicUsize,

    /// Number of currently free pages
    free_pages: AtomicUsize,

    /// Free areas for each order (buddy allocator)
    free_area: [FreeArea; MAX_ORDER + 1],

    /// Zone lock for buddy operations
    lock: Mutex<()>,

    /// Zone initialized flag
    initialized: AtomicBool,
}

// Static array of free areas
const fn new_free_areas() -> [FreeArea; MAX_ORDER + 1] {
    let mut arr: [FreeArea; MAX_ORDER + 1] = [
        FreeArea::new(), FreeArea::new(), FreeArea::new(), FreeArea::new(),
        FreeArea::new(), FreeArea::new(), FreeArea::new(), FreeArea::new(),
        FreeArea::new(), FreeArea::new(), FreeArea::new(),
    ];
    arr
}

impl Zone {
    /// Create a new uninitialized zone
    pub const fn new(zone_type: ZoneType, zone_id: usize, node_id: usize) -> Self {
        Self {
            zone_type,
            zone_id,
            node_id,
            zone_start_pfn: AtomicUsize::new(0),
            zone_end_pfn: AtomicUsize::new(0),
            spanned_pages: AtomicUsize::new(0),
            present_pages: AtomicUsize::new(0),
            managed_pages: AtomicUsize::new(0),
            free_pages: AtomicUsize::new(0),
            free_area: new_free_areas(),
            lock: Mutex::new(()),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialize the zone
    ///
    /// # Arguments
    /// - `start_pfn`: Start page frame number
    /// - `end_pfn`: End page frame number (exclusive)
    pub fn init(&self, start_pfn: usize, end_pfn: usize) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        let _guard = self.lock.lock();

        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        let spanned = end_pfn.saturating_sub(start_pfn);

        self.zone_start_pfn.store(start_pfn, Ordering::Release);
        self.zone_end_pfn.store(end_pfn, Ordering::Release);
        self.spanned_pages.store(spanned, Ordering::Release);
        self.present_pages.store(spanned, Ordering::Release);
        self.managed_pages.store(spanned, Ordering::Release);
        // Don't set free_pages here - it will be updated as pages are added

        self.initialized.store(true, Ordering::Release);
    }

    /// Check if zone is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Get zone type
    pub fn zone_type(&self) -> ZoneType {
        self.zone_type
    }

    /// Get zone ID
    pub fn zone_id(&self) -> usize {
        self.zone_id
    }

    /// Get node ID
    pub fn node_id(&self) -> usize {
        self.node_id
    }

    /// Get start PFN
    pub fn start_pfn(&self) -> usize {
        self.zone_start_pfn.load(Ordering::Acquire)
    }

    /// Get end PFN (exclusive)
    pub fn end_pfn(&self) -> usize {
        self.zone_end_pfn.load(Ordering::Acquire)
    }

    /// Get spanned pages
    pub fn spanned_pages(&self) -> usize {
        self.spanned_pages.load(Ordering::Acquire)
    }

    /// Get present pages
    pub fn present_pages(&self) -> usize {
        self.present_pages.load(Ordering::Acquire)
    }

    /// Get managed pages
    pub fn managed_pages(&self) -> usize {
        self.managed_pages.load(Ordering::Acquire)
    }

    /// Get free pages count
    pub fn nr_free(&self) -> usize {
        self.free_pages.load(Ordering::Acquire)
    }

    /// Get free pages at a specific order
    pub fn free_pages_order(&self, order: usize) -> usize {
        if order > MAX_ORDER {
            return 0;
        }
        self.free_area[order].nr_free()
    }

    /// Allocate pages from this zone
    ///
    /// # Arguments
    /// - `order`: Order of allocation (2^order pages)
    ///
    /// # Returns
    /// - PFN of allocated block, or None if allocation fails
    pub fn alloc_pages(&self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        let _guard = self.lock.lock();

        // Find a free block at this order or higher
        for current_order in order..=MAX_ORDER {
            let head = self.free_area[current_order].free_list.load(Ordering::Acquire);
            if head != FREE_LIST_NULL {
                // Found a block, remove it and split if needed
                return self.alloc_from_order(current_order, order);
            }
        }

        None
    }

    /// Allocate a block from a specific order level
    fn alloc_from_order(&self, current_order: usize, target_order: usize) -> Option<usize> {
        let head = self.free_area[current_order].free_list.load(Ordering::Acquire);
        if head == FREE_LIST_NULL {
            return None;
        }

        // Remove from free list
        self.remove_from_free_list(head, current_order);

        let mut pfn = head;
        let mut order = current_order;

        // Split blocks until we reach target order
        while order > target_order {
            order -= 1;
            let buddy_pfn = pfn + (1usize << order);

            // Add buddy to free list
            self.add_to_free_list(buddy_pfn, order);
        }

        // Update free pages count
        self.free_pages.fetch_sub(1usize << target_order, Ordering::Relaxed);

        // Update page descriptor
        let page = pfn_to_page_mut(pfn);
        if !page.is_null() {
            unsafe {
                (*page).set_refcount(1);
                (*page).set_order(target_order as u8);
            }
        }

        Some(pfn)
    }

    /// Free pages to this zone
    ///
    /// # Arguments
    /// - `pfn`: Page frame number of the block to free
    /// - `order`: Order of the block
    pub fn free_pages(&self, pfn: usize, order: usize) {
        if order > MAX_ORDER {
            return;
        }

        // Check if pfn is within zone range
        let start = self.zone_start_pfn.load(Ordering::Acquire);
        let end = self.zone_end_pfn.load(Ordering::Acquire);
        if pfn < start || pfn >= end {
            return;
        }

        let _guard = self.lock.lock();

        // Update page descriptor
        let page = pfn_to_page_mut(pfn);
        if !page.is_null() {
            unsafe {
                (*page).set_refcount(0);
            }
        }

        let mut current_pfn = pfn;
        let mut current_order = order;

        // Try to merge with buddy
        while current_order < MAX_ORDER {
            let buddy_pfn = self.find_buddy(current_pfn, current_order);

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

        // Update free pages count
        self.free_pages.fetch_add(1usize << order, Ordering::Relaxed);
    }

    /// Find buddy PFN
    fn find_buddy(&self, pfn: usize, order: usize) -> usize {
        pfn ^ (1usize << order)
    }

    /// Check if buddy is free
    fn is_buddy_free(&self, buddy_pfn: usize, order: usize) -> bool {
        // Check if buddy is in zone range
        let start = self.zone_start_pfn.load(Ordering::Acquire);
        let end = self.zone_end_pfn.load(Ordering::Acquire);
        if buddy_pfn < start || buddy_pfn >= end {
            return false;
        }

        // Check if buddy is in the free list by walking the list
        let mut current = self.free_area[order].free_list.load(Ordering::Acquire);
        while current != FREE_LIST_NULL {
            if current == buddy_pfn {
                return true;
            }

            // Get next from Page descriptor
            let page = pfn_to_page(current);
            if page.is_null() {
                break;
            }
            current = unsafe { (*page).next_free() };
        }

        false
    }

    /// Add block to free list
    fn add_to_free_list(&self, pfn: usize, order: usize) {
        let head = self.free_area[order].free_list.load(Ordering::Acquire);

        // Update Page descriptor's free list pointers
        let page = pfn_to_page_mut(pfn);
        if !page.is_null() {
            unsafe {
                (*page).set_next_free(head);
                (*page).set_order(order as u8);
            }
        }

        // Set new head
        self.free_area[order].free_list.store(pfn, Ordering::Release);
        self.free_area[order].inc_free();
    }

    /// Remove block from free list
    fn remove_from_free_list(&self, pfn: usize, order: usize) {
        let head = self.free_area[order].free_list.load(Ordering::Acquire);

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
            self.free_area[order].free_list.store(next, Ordering::Release);
        } else {
            // Need to walk the list to find and remove
            let mut prev = head;
            loop {
                if prev == FREE_LIST_NULL {
                    break;
                }

                let prev_page = pfn_to_page(prev);
                if prev_page.is_null() {
                    break;
                }

                let next = unsafe { (*prev_page).next_free() };
                if next == pfn {
                    // Found it, update prev's next pointer
                    let target_page = pfn_to_page(pfn);
                    let new_next = if target_page.is_null() {
                        FREE_LIST_NULL
                    } else {
                        unsafe { (*target_page).next_free() }
                    };
                    unsafe { (*prev_page).set_next_free(new_next); }
                    break;
                }
                prev = next;
            }
        }

        self.free_area[order].dec_free();
    }

    /// Get zone statistics
    pub fn stats(&self) -> ZoneStats {
        ZoneStats {
            zone_type: self.zone_type,
            zone_id: self.zone_id,
            node_id: self.node_id,
            start_pfn: self.start_pfn(),
            end_pfn: self.end_pfn(),
            spanned_pages: self.spanned_pages(),
            present_pages: self.present_pages(),
            managed_pages: self.managed_pages(),
            free_pages: self.nr_free(),
            free_blocks: [
                self.free_area[0].nr_free(),
                self.free_area[1].nr_free(),
                self.free_area[2].nr_free(),
                self.free_area[3].nr_free(),
                self.free_area[4].nr_free(),
                self.free_area[5].nr_free(),
                self.free_area[6].nr_free(),
                self.free_area[7].nr_free(),
                self.free_area[8].nr_free(),
                self.free_area[9].nr_free(),
                self.free_area[10].nr_free(),
            ],
        }
    }
}

/// Zone statistics
#[derive(Debug, Clone, Copy)]
pub struct ZoneStats {
    pub zone_type: ZoneType,
    pub zone_id: usize,
    pub node_id: usize,
    pub start_pfn: usize,
    pub end_pfn: usize,
    pub spanned_pages: usize,
    pub present_pages: usize,
    pub managed_pages: usize,
    pub free_pages: usize,
    pub free_blocks: [usize; MAX_ORDER + 1],
}

// ==================== GFP Flags ====================

/// GFP (Get Free Pages) flags
///
/// These flags control allocation behavior and zone selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfpFlags(pub u32);

impl GfpFlags {
    /// Normal kernel allocation
    pub const GFP_KERNEL: GfpFlags = GfpFlags(0x01);

    /// User space allocation
    pub const GFP_USER: GfpFlags = GfpFlags(0x02);

    /// High priority allocation (can use atomic reserves)
    pub const GFP_ATOMIC: GfpFlags = GfpFlags(0x04);

    /// DMA compatible allocation
    pub const GFP_DMA: GfpFlags = GfpFlags(0x08);

    /// DMA32 compatible allocation
    pub const GFP_DMA32: GfpFlags = GfpFlags(0x10);

    /// Zero pages on allocation
    pub const __GFP_ZERO: GfpFlags = GfpFlags(0x100);

    /// High memory allocation (not used on RISC-V)
    pub const __GFP_HIGHMEM: GfpFlags = GfpFlags(0x200);

    /// Movable allocation
    pub const __GFP_MOVABLE: GfpFlags = GfpFlags(0x400);

    /// Get zone type for this GFP mask
    pub fn zone_type(&self) -> ZoneType {
        if self.0 & Self::GFP_DMA.0 != 0 {
            ZoneType::ZoneDma
        } else if self.0 & Self::GFP_DMA32.0 != 0 {
            ZoneType::ZoneDma32
        } else if self.0 & Self::__GFP_MOVABLE.0 != 0 {
            ZoneType::ZoneMovable
        } else {
            ZoneType::ZoneNormal
        }
    }
}

// ==================== Helper Functions ====================

/// Convert PFN to physical address
pub fn pfn_to_phys(pfn: usize) -> usize {
    pfn * PAGE_SIZE
}

/// Convert physical address to PFN
pub fn phys_to_pfn(phys: usize) -> usize {
    phys / PAGE_SIZE
}

/// Print zone information
pub fn print_zone_info(zone: &Zone) {
    crate::println!("Zone {} (Node {}):", zone.zone_type().name(), zone.node_id());
    crate::println!("  Start PFN:    {:#x}", zone.start_pfn());
    crate::println!("  End PFN:      {:#x}", zone.end_pfn());
    crate::println!("  Spanned:      {} pages ({} MB)",
        zone.spanned_pages(),
        zone.spanned_pages() * PAGE_SIZE / (1024 * 1024));
    crate::println!("  Present:      {} pages", zone.present_pages());
    crate::println!("  Managed:      {} pages", zone.managed_pages());
    crate::println!("  Free:         {} pages ({} MB)",
        zone.nr_free(),
        zone.nr_free() * PAGE_SIZE / (1024 * 1024));
    crate::println!("  Free blocks per order:");
    for order in 0..=MAX_ORDER {
        let nr = zone.free_pages_order(order);
        if nr > 0 {
            crate::println!("    Order {}: {} blocks ({} pages each)",
                order, nr, 1usize << order);
        }
    }
}
