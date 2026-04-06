//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Virtual Memory Scanning / Page Reclaim Engine
//!
//! Follows mm/vmscan.c: priority-based reclaim that scans LRU lists
//! and frees clean, unmapped pages.  This module is invoked by both
//! kswapd (background) and direct reclaim (allocation-time).
//!
//! Reclaim sources:
//! 1. Page cache shrinker — evicts clean, unreferenced page cache pages
//! 2. Anonymous page scan — scans page descriptors for mapped anonymous
//!    pages, calls try_to_unmap(), frees successfully unmapped pages

extern crate alloc;
use alloc::vec::Vec;

use core::sync::atomic::Ordering;

use super::page_desc::{PageFlag, PageType, pfn_to_page_mut, MIN_PFN, MAX_PAGES};
use super::page_alloc::free_page;
use super::zone::{ZoneType, WMARK_LOW};
use super::pglist::{
    first_online_node_mut, LRU_INACTIVE_ANON, LRU_INACTIVE_FILE,
    NR_LRU_LISTS, DEF_PRIORITY,
};
use super::pfn_to_phys;
use super::lru;
use super::swap;

// ==================== Scan Control ====================

/// Reclaim scan state, following `struct scan_control` in mm/vmscan.c.
pub struct ScanControl {
    /// Target number of pages to free.
    pub nr_to_reclaim: usize,
    /// Total pages examined.
    pub nr_scanned: usize,
    /// Pages actually freed.
    pub nr_reclaimed: usize,
    /// Whether we are allowed to unmap pages from PTEs.
    pub may_unmap: bool,
    /// Current priority (DEF_PRIORITY=12 down to 1).
    pub priority: i32,
    /// Allocation order we are reclaiming for.
    pub order: i32,
}

// ==================== Public API ====================

/// Top-level reclaim entry point.
///
/// Runs a priority-loop following `balance_pgdat()` in mm/vmscan.c.
/// Returns the number of pages reclaimed.
pub fn balance_pgdat(order: i32) -> usize {
    let mut sc = ScanControl {
        nr_to_reclaim: 1 << order.max(0) as usize,
        nr_scanned: 0,
        nr_reclaimed: 0,
        may_unmap: true,
        priority: DEF_PRIORITY,
        order,
    };

    let mut priority = DEF_PRIORITY;
    while priority >= 1 && sc.nr_reclaimed < sc.nr_to_reclaim {
        sc.priority = priority;
        shrink_node(&mut sc);
        priority -= 1;
    }

    sc.nr_reclaimed
}

/// Direct reclaim: try to free enough pages for an allocation of `order`.
///
/// Called from the page allocator when the zone is below WMARK_LOW.
/// Returns the number of pages reclaimed.
pub fn try_to_free_pages(order: i32) -> usize {
    balance_pgdat(order)
}

// ==================== Internal: shrink_* ====================

/// Per-node reclaim: iterate zones that are below watermark.
fn shrink_node(sc: &mut ScanControl) {
    let node = match first_online_node_mut() {
        Some(n) => n,
        None => return,
    };

    for zone_type in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
        if let Some(zone) = node.zone(zone_type) {
            if zone.is_initialized() && !zone.watermark_ok(sc.order as usize, WMARK_LOW) {
                shrink_lruvec(&node, sc);
            }
        }
    }
}

/// Per-lruvec reclaim: determine scan counts and dispatch.
fn shrink_lruvec(node: &super::pglist::PglistData, sc: &mut ScanControl) {
    // Scan inactive anon list
    let scan_anon = nr_to_scan(LRU_INACTIVE_ANON, node, sc);
    if scan_anon > 0 {
        shrink_inactive_list(LRU_INACTIVE_ANON, scan_anon, node, sc);
    }

    // Scan inactive file list
    let scan_file = nr_to_scan(LRU_INACTIVE_FILE, node, sc);
    if scan_file > 0 {
        shrink_inactive_list(LRU_INACTIVE_FILE, scan_file, node, sc);
    }

    // TODO: shrink_active_list for referenced page rotation
}

/// Calculate the number of pages to scan from an LRU list.
///
/// Following `get_scan_count()` in mm/vmscan.c:
///   scan = size >> (priority - 2)
fn nr_to_scan(lru: usize, node: &super::pglist::PglistData, sc: &ScanControl) -> usize {
    let size = node.lru_sizes[lru].load(Ordering::Relaxed);
    if size == 0 {
        return 0;
    }
    // At DEF_PRIORITY (12): scan size/1024
    // At priority 1: scan size/2
    let shift = (sc.priority as usize).saturating_sub(2);
    let scan = size >> shift;
    if scan == 0 {
        // Ensure at least a small scan at low priorities
        if sc.priority <= 3 {
            size.min(32)
        } else {
            0
        }
    } else {
        scan
    }
}

/// Scan pages from an inactive LRU list and attempt to reclaim them.
///
/// Following `shrink_inactive_list()` in mm/vmscan.c.
///
/// For inactive file: delegates to the page cache shrinker.
/// For inactive anon: writes pages to swap and unmaps them.
fn shrink_inactive_list(
    lru: usize,
    nr_to_scan: usize,
    _node: &super::pglist::PglistData,
    sc: &mut ScanControl,
) {
    if lru == LRU_INACTIVE_FILE {
        // Reclaim page cache pages via the shrinker interface
        let cache_reclaimed = crate::fs::page_cache::get_page_cache().shrink(nr_to_scan);
        sc.nr_reclaimed += cache_reclaimed;
    } else if lru == LRU_INACTIVE_ANON && sc.may_unmap && swap::nr_active_swap() {
        // Reclaim anonymous pages via swap
        let anon_reclaimed = reclaim_anonymous_pages(nr_to_scan, sc);
        sc.nr_reclaimed += anon_reclaimed;
    }

    sc.nr_scanned += nr_to_scan;
}

/// Scan page descriptors for mapped anonymous pages and swap them out.
///
/// Iterates all page descriptors looking for anonymous, swap-backed, mapped
/// pages with a single mapping (mapcount == 1).  For each candidate:
///   1. Allocate a swap slot
///   2. Write the page to the swap device
///   3. Replace all PTEs with a swap entry (try_to_unmap_with_swap)
///   4. Free the physical page
///
/// The scan is bounded by `nr_to_scan` to limit latency.
fn reclaim_anonymous_pages(nr_to_scan: usize, sc: &mut ScanControl) -> usize {
    use super::rmap::try_to_unmap_with_swap;

    let mut reclaimed = 0usize;

    for i in 0..MAX_PAGES {
        if reclaimed >= nr_to_scan {
            break;
        }

        let pfn = MIN_PFN + i;
        let page = pfn_to_page_mut(pfn);
        if page.is_null() {
            continue;
        }

        unsafe {
            let p = &*page;

            // Skip non-anonymous pages
            if !p.is_anonymous() {
                continue;
            }

            // Must be swap-backed (set by page_add_anon_rmap)
            if !p.test_flag(PageFlag::SwapBacked) {
                continue;
            }

            // Skip unmapped pages
            if !p.is_mapped() {
                continue;
            }

            // Skip reserved/locked pages
            if p.is_reserved() || p.is_locked() {
                continue;
            }

            // Check referenced flag (recently accessed — give it another chance)
            if p.test_flag(PageFlag::Referenced) {
                p.clear_flag(PageFlag::Referenced);
                sc.nr_scanned += 1;
                continue;
            }

            sc.nr_scanned += 1;

            // Allocate a swap slot
            let (swap_type, swap_offset) = match swap::swap_alloc_slot() {
                Some(slot) => slot,
                None => break, // No swap space left
            };

            // Build the swap entry that will be stored in PTEs
            let swap_entry = swap::make_swap_entry(swap_type, swap_offset);

            // Write page contents to the swap device
            let phys = pfn_to_phys(pfn);
            if swap::swap_write_page(swap_type, swap_offset, phys).is_err() {
                // Write failed — free the slot and skip this page
                swap::swap_free_slot(swap_type, swap_offset);
                continue;
            }

            // Replace PTEs with swap entry
            let unmapped = try_to_unmap_with_swap(p, swap_entry);

            if unmapped > 0 && !p.is_mapped() {
                // Successfully swapped out — clean up and free
                p.clear_flag(PageFlag::Anonymous);
                p.clear_flag(PageFlag::SwapBacked);
                p.set_index(0);

                // Remove from LRU
                super::lru::page_remove_lru(p);

                // Drop reference; free if last holder
                let refcount = p.put_page();
                if refcount <= 0 {
                    free_page(phys);
                    reclaimed += 1;
                } else {
                    // Page still referenced (unexpected) — free swap slot
                    swap::swap_free_slot(swap_type, swap_offset);
                }
            } else {
                // Unmap failed or page still mapped — free the swap slot
                swap::swap_free_slot(swap_type, swap_offset);
            }
        }
    }

    reclaimed
}
