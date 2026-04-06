//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! LRU List Management
//!
//! Doubly-linked LRU lists for page reclamation, following mm/vmscan.c
//! and mm/swap.c in the kernel.  Pages are linked by PFN; when PG_lru
//! is set the Page.mapping and Page.index fields store prev/next PFN.
//!
//! PFN 0 is used as the sentinel for "no page" (valid PFNs start at
//! MIN_PFN which is >> 0 on RISC-V).

use crate::sync::spinlock::Spinlock;
use super::page_desc::{Page, PageFlag, pfn_to_page_mut, page_to_pfn};
use super::pglist::{
    first_online_node_mut, LRU_INACTIVE_ANON, LRU_ACTIVE_ANON,
    LRU_INACTIVE_FILE, LRU_ACTIVE_FILE, LRU_UNEVICTABLE, NR_LRU_LISTS,
};
use super::PAGE_SIZE;

// ==================== LRU list encoding ====================

/// Sentinel PFN value meaning "no page" (end of list).
const LRU_NONE: usize = 0;

// ==================== Core LRU operations ====================

/// Add a page to the *tail* of the specified LRU list.
///
/// The tail is the least-recently-used end; kswapd scans from here.
pub fn lru_add_page(page: &Page, lru_type: usize) {
    let node = match first_online_node_mut() {
        Some(n) => n,
        None => return,
    };

    let pfn = page_to_pfn(page as *const Page);

    let _guard = node.lru_lock.lock();

    let tail = node.lru_tails[lru_type].load(core::sync::atomic::Ordering::Relaxed);

    // Set LRU pointers in the page descriptor:
    //   mapping (→ lru_prev) = current tail PFN
    //   index  (→ lru_next)  = LRU_NONE
    let prev_val = if tail != LRU_NONE { tail } else { LRU_NONE };
    page.set_mapping(prev_val as *mut core::ffi::c_void);
    page.set_index(LRU_NONE);

    if tail != LRU_NONE {
        // Link old tail → new page (set old tail's next = pfn)
        unsafe {
            let tail_page = pfn_to_page_mut(tail);
            if !tail_page.is_null() {
                (*tail_page).set_index(pfn);
            }
        }
        node.lru_tails[lru_type].store(pfn, core::sync::atomic::Ordering::Relaxed);
    } else {
        // List was empty — new page is both head and tail
        node.lru_heads[lru_type].store(pfn, core::sync::atomic::Ordering::Relaxed);
        node.lru_tails[lru_type].store(pfn, core::sync::atomic::Ordering::Relaxed);
    }

    page.set_flag(PageFlag::Lru);
    node.lru_sizes[lru_type].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Remove a page from its LRU list.
///
/// The page must have PG_lru set.
pub fn lru_del_page(page: &Page) {
    let node = match first_online_node_mut() {
        Some(n) => n,
        None => return,
    };

    let pfn = page_to_pfn(page as *const Page);

    // Read current LRU pointers (set while PG_lru is active)
    let prev_pfn = page.mapping() as usize;
    let next_pfn = page.index();

    let _guard = node.lru_lock.lock();

    // Determine which list this page is on by scanning.
    let mut found_list = None;
    for lru in 0..NR_LRU_LISTS {
        let mut cur = node.lru_heads[lru].load(core::sync::atomic::Ordering::Relaxed);
        while cur != LRU_NONE {
            if cur == pfn {
                found_list = Some(lru);
                break;
            }
            cur = {
                let p = pfn_to_page_mut(cur);
                if p.is_null() { break; }
                unsafe { (*p).index() }
            };
        }
        if found_list.is_some() {
            break;
        }
    }

    let lru = match found_list {
        Some(l) => l,
        None => return,
    };

    // Unlink from doubly-linked list
    if prev_pfn != LRU_NONE {
        unsafe {
            let prev_page = pfn_to_page_mut(prev_pfn);
            if !prev_page.is_null() {
                (*prev_page).set_index(next_pfn);
            }
        }
    } else {
        node.lru_heads[lru].store(next_pfn, core::sync::atomic::Ordering::Relaxed);
    }

    if next_pfn != LRU_NONE {
        unsafe {
            let next_page = pfn_to_page_mut(next_pfn);
            if !next_page.is_null() {
                (*next_page).set_mapping(prev_pfn as *mut core::ffi::c_void);
            }
        }
    } else {
        node.lru_tails[lru].store(prev_pfn, core::sync::atomic::Ordering::Relaxed);
    }

    // Clear LRU pointers in the page descriptor
    page.set_mapping(core::ptr::null_mut());
    page.set_index(0);

    page.clear_flag(PageFlag::Lru);
    node.lru_sizes[lru].fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
}

/// Move a page to the tail of a (possibly different) LRU list.
pub fn lru_move_to_tail(page: &Page, new_lru: usize) {
    if page.test_flag(PageFlag::Lru) {
        lru_del_page(page);
    }
    lru_add_page(page, new_lru);
}

/// Move a page from an active LRU list to its inactive counterpart.
pub fn lru_deactivate(page: &Page) {
    if !page.test_flag(PageFlag::Lru) || page.test_flag(PageFlag::Unevictable) {
        return;
    }

    let target = if page.test_flag(PageFlag::Active) {
        page.clear_flag(PageFlag::Active);
        if page.test_flag(PageFlag::Anonymous) {
            LRU_INACTIVE_ANON
        } else {
            LRU_INACTIVE_FILE
        }
    } else {
        return;
    };

    lru_move_to_tail(page, target);
}

/// Move a page from an inactive LRU list to its active counterpart.
pub fn lru_activate(page: &Page) {
    if !page.test_flag(PageFlag::Lru) || page.test_flag(PageFlag::Unevictable) {
        return;
    }

    let target = if !page.test_flag(PageFlag::Active) {
        page.set_flag(PageFlag::Active);
        if page.test_flag(PageFlag::Anonymous) {
            LRU_ACTIVE_ANON
        } else {
            LRU_ACTIVE_FILE
        }
    } else {
        return;
    };

    lru_move_to_tail(page, target);
}

/// Mark a page as recently referenced (set PG_referenced).
pub fn lru_note_refs(page: &Page) {
    page.set_flag(PageFlag::Referenced);
}

/// Check whether a page has been recently referenced.
pub fn page_referenced(page: &Page) -> bool {
    page.test_flag(PageFlag::Referenced)
}

/// Check whether a page is evictable (can be reclaimed).
pub fn page_evictable(page: &Page) -> bool {
    if page.test_flag(PageFlag::Unevictable) {
        return false;
    }
    if page.refcount() > 1 {
        return false;
    }
    true
}

/// Get the total number of pages across all LRU lists.
pub fn lru_page_total() -> usize {
    match first_online_node_mut() {
        Some(node) => {
            let mut total = 0usize;
            for lru in 0..NR_LRU_LISTS {
                total += node.lru_sizes[lru].load(core::sync::atomic::Ordering::Relaxed);
            }
            total
        }
        None => 0,
    }
}

/// Add an anonymous page to LRU_INACTIVE_ANON on first mapping.
pub fn page_add_anon_lru(page: &Page) {
    lru_add_page(page, LRU_INACTIVE_ANON);
}

/// Add a file-backed page to LRU_INACTIVE_FILE on first mapping.
pub fn page_add_file_lru(page: &Page) {
    lru_add_page(page, LRU_INACTIVE_FILE);
}

/// Remove a page from its LRU list when the last mapping is removed.
pub fn page_remove_lru(page: &Page) {
    lru_del_page(page);
}
