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
//! Current implementation uses a shrinker-based approach for page cache
//! pages (like Linux's `shrink_slab`).  LRU list scanning for mapped
//! pages (anonymous / file-backed) is deferred until try_to_unmap()
//! walks page tables and flushes TLB entries.

extern crate alloc;
use alloc::vec::Vec;

use core::sync::atomic::Ordering;

use super::page_desc::{PageFlag, PageType, pfn_to_page_mut};
use super::page_alloc::free_page;
use super::zone::{ZoneType, WMARK_LOW};
use super::pglist::{
    first_online_node_mut, LRU_INACTIVE_ANON, LRU_INACTIVE_FILE,
    NR_LRU_LISTS, DEF_PRIORITY,
};
use super::pfn_to_phys;
use super::lru;

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
/// Current implementation delegates to the page cache shrinker (analogous
/// to Linux's shrinker API).  This reclaims clean, unreferenced page cache
/// pages without needing to walk LRU lists or call try_to_unmap.
///
/// TODO: Add LRU list scanning for mapped pages once try_to_unmap is implemented.
fn shrink_inactive_list(
    _lru: usize,
    nr_to_scan: usize,
    _node: &super::pglist::PglistData,
    sc: &mut ScanControl,
) {
    // Reclaim page cache pages via the shrinker interface
    let reclaimed = crate::fs::page_cache::get_page_cache().shrink(nr_to_scan);
    sc.nr_reclaimed += reclaimed;
    sc.nr_scanned += nr_to_scan;
}
