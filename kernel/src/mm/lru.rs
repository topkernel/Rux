//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! LRU List Management
//!
//! Singly-linked LRU lists for page reclamation. Pages are linked by PFN
//! via the dedicated `lru_next` field in the Page descriptor. The tail
//! of each list is the least-recently-used end; kswapd scans from here.
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

    // New page becomes the new tail (no next)
    page.set_lru_next(LRU_NONE);

    let tail = node.lru_tails[lru_type].load(core::sync::atomic::Ordering::Relaxed);

    if tail != LRU_NONE {
        // Link old tail → new page
        // SAFETY: tail is a valid PFN from lru_tails; lru_lock is held.
        unsafe {
            let tail_page = pfn_to_page_mut(tail);
            if !tail_page.is_null() {
                (*tail_page).set_lru_next(pfn);
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
/// Scans all lists to find the page (tracking the previous node for
/// singly-linked unlink), then removes it.
pub fn lru_del_page(page: &Page) {
    let node = match first_online_node_mut() {
        Some(n) => n,
        None => return,
    };

    if !page.test_flag(PageFlag::Lru) {
        return;
    }

    let pfn = page_to_pfn(page as *const Page);

    let _guard = node.lru_lock.lock();

    // Scan all lists to find the page and its predecessor
    let mut found_list = None;
    let mut prev_pfn = LRU_NONE;

    for lru in 0..NR_LRU_LISTS {
        let mut cur = node.lru_heads[lru].load(core::sync::atomic::Ordering::Relaxed);
        prev_pfn = LRU_NONE;

        while cur != LRU_NONE {
            if cur == pfn {
                found_list = Some(lru);
                break;
            }
            prev_pfn = cur;
            cur = {
                let p = pfn_to_page_mut(cur);
                if p.is_null() { break; }
                // SAFETY: cur is a valid PFN from lru_heads/chain; lru_lock is held.
                unsafe { (*p).lru_next() }
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

    // Get the page's next pointer
    let next_pfn = page.lru_next();

    // Unlink: prev → next (or update head if prev is none)
    if prev_pfn != LRU_NONE {
        // SAFETY: prev_pfn is a valid PFN found during list scan; lru_lock is held.
        unsafe {
            let prev_page = pfn_to_page_mut(prev_pfn);
            if !prev_page.is_null() {
                (*prev_page).set_lru_next(next_pfn);
            }
        }
    } else {
        node.lru_heads[lru].store(next_pfn, core::sync::atomic::Ordering::Relaxed);
    }

    // Update tail if page was tail
    if next_pfn == LRU_NONE {
        node.lru_tails[lru].store(prev_pfn, core::sync::atomic::Ordering::Relaxed);
    }

    // Clear LRU pointer in page descriptor
    page.set_lru_next(LRU_NONE);

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

/// Get the tail PFN (LRU end) of a specific LRU list.
/// Returns 0 if the list is empty.
pub fn lru_tail(lru_type: usize) -> usize {
    let node = match first_online_node_mut() {
        Some(n) => n,
        None => return 0,
    };
    node.lru_tails[lru_type].load(core::sync::atomic::Ordering::Acquire)
}
