//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! LRU List Management
//!
//! DOUBLY-linked LRU lists for page reclamation (review 4.11). Pages are
//! linked by PFN via the dedicated `lru_next`/`lru_prev` fields in the Page
//! descriptor; the owning list index is recorded in the page's flag bits
//! (LRU_LIST_SHIFT). The tail of each list is the least-recently-used end;
//! kswapd scans from here.
//!
//! With both links, del/move are O(1) — the previous singly-linked scheme
//! walked the whole list to find a page's predecessor on every reclaim
//! (O(n²) across a scan pass).
//!
//! PFN 0 is used as the sentinel for "no page" (valid PFNs start at
//! MIN_PFN which is >> 0 on RISC-V).

extern crate alloc;
use alloc::vec::Vec;

use super::page_desc::{Page, PageFlag, pfn_to_page_mut, page_to_pfn};
use super::pglist::{first_online_node_mut, NR_LRU_LISTS};

/// Sentinel PFN value meaning "no page" (end of list).
const LRU_NONE: usize = 0;

// ==================== Core LRU operations ====================

/// O(1) unlink of `page` from LRU list `lru` — caller holds `lru_lock`.
///
/// Takes `&PglistData` (all LRU fields are atomics — interior mutability);
/// the guard from `node.lru_lock.lock()` keeps an immutable borrow alive.
///
/// # Safety
/// `page` must be linked on list `lru` of `node`'s LRU arrays.
unsafe fn unlink_locked(
    node: &super::pglist::PglistData,
    page: &Page,
    lru: usize,
) {
    let prev = page.lru_prev();
    let next = page.lru_next();

    if prev != LRU_NONE {
        let prev_page = pfn_to_page_mut(prev);
        if !prev_page.is_null() {
            (*prev_page).set_lru_next(next);
        }
    } else {
        // Page was the head
        node.lru_heads[lru].store(next, core::sync::atomic::Ordering::Relaxed);
    }

    if next != LRU_NONE {
        let next_page = pfn_to_page_mut(next);
        if !next_page.is_null() {
            (*next_page).set_lru_prev(prev);
        }
    } else {
        // Page was the tail
        node.lru_tails[lru].store(prev, core::sync::atomic::Ordering::Relaxed);
    }

    page.set_lru_next(LRU_NONE);
    page.set_lru_prev(LRU_NONE);
    node.lru_sizes[lru].fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
}

/// O(1) link of `page` at the TAIL of LRU list `lru` — caller holds
/// `lru_lock`.
///
/// # Safety
/// `page` must not currently be linked on any list.
unsafe fn link_tail_locked(
    node: &super::pglist::PglistData,
    page: &Page,
    lru: usize,
) {
    let pfn = page_to_pfn(page as *const Page);

    // New page becomes the new tail (no next)
    page.set_lru_next(LRU_NONE);
    page.set_lru_prev(LRU_NONE);
    page.set_lru_list(lru);

    let tail = node.lru_tails[lru].load(core::sync::atomic::Ordering::Relaxed);

    if tail != LRU_NONE {
        // Link old tail → new page
        let tail_page = pfn_to_page_mut(tail);
        if !tail_page.is_null() {
            (*tail_page).set_lru_next(pfn);
        }
        page.set_lru_prev(tail);
        node.lru_tails[lru].store(pfn, core::sync::atomic::Ordering::Relaxed);
    } else {
        // List was empty — new page is both head and tail
        node.lru_heads[lru].store(pfn, core::sync::atomic::Ordering::Relaxed);
        node.lru_tails[lru].store(pfn, core::sync::atomic::Ordering::Relaxed);
    }

    node.lru_sizes[lru].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Add a page to the *tail* of the specified LRU list.
///
/// The tail is the least-recently-used end; kswapd scans from here.
pub fn lru_add_page(page: &Page, lru_type: usize) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match unsafe { first_online_node_mut() } {
        Some(n) => n,
        None => return,
    };

    let _guard = node.lru_lock.lock();

    // SAFETY: page is not linked (checked via Lru flag below) and the lock
    // is held for the whole link.
    unsafe {
        if !page.test_flag(PageFlag::Lru) {
            link_tail_locked(node, page, lru_type);
            page.set_flag(PageFlag::Lru);
        }
    }
}

/// Remove a page from its LRU list — O(1) via lru_prev (review 4.11).
pub fn lru_del_page(page: &Page) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match unsafe { first_online_node_mut() } {
        Some(n) => n,
        None => return,
    };

    if !page.test_flag(PageFlag::Lru) {
        return;
    }

    let _guard = node.lru_lock.lock();

    // The list index is recorded on the page — no list walk required.
    let lru = page.lru_list();
    if lru >= NR_LRU_LISTS {
        return;
    }

    // SAFETY: page carries the Lru flag and a valid list index; lock held.
    unsafe {
        unlink_locked(node, page, lru);
    }

    page.clear_flag(PageFlag::Lru);
    page.clear_lru_list();
}

/// Move a page to the tail of a (possibly different) LRU list — O(1).
///
/// Acquires the LRU lock once for both the unlink and the relink.
pub fn lru_move_to_tail(page: &Page, new_lru: usize) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match unsafe { first_online_node_mut() } {
        Some(n) => n,
        None => return,
    };

    let _guard = node.lru_lock.lock();

    // SAFETY: unlink only when the page really is linked; lock held.
    unsafe {
        if page.test_flag(PageFlag::Lru) {
            let lru = page.lru_list();
            if lru < NR_LRU_LISTS {
                unlink_locked(node, page, lru);
                page.clear_flag(PageFlag::Lru);
                page.clear_lru_list();
            }
        }
        link_tail_locked(node, page, new_lru);
    }
    page.set_flag(PageFlag::Lru);
}

/// Move a page from an active LRU list to its inactive counterpart.
pub fn lru_deactivate(page: &Page) {
    if !page.test_flag(PageFlag::Lru) || page.test_flag(PageFlag::Unevictable) {
        return;
    }

    let target = if page.test_flag(PageFlag::Active) {
        page.clear_flag(PageFlag::Active);
        if page.test_flag(PageFlag::Anonymous) {
            super::pglist::LRU_INACTIVE_ANON
        } else {
            super::pglist::LRU_INACTIVE_FILE
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
            super::pglist::LRU_ACTIVE_ANON
        } else {
            super::pglist::LRU_ACTIVE_FILE
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
    // SAFETY: read-only LRU statistics — no concurrent mutation concern.
    match unsafe { first_online_node_mut() } {
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
    lru_add_page(page, super::pglist::LRU_INACTIVE_ANON);
}

/// Add a file-backed page to LRU_INACTIVE_FILE on first mapping.
pub fn page_add_file_lru(page: &Page) {
    lru_add_page(page, super::pglist::LRU_INACTIVE_FILE);
}

/// Remove a page from its LRU list when the last mapping is removed.
pub fn page_remove_lru(page: &Page) {
    lru_del_page(page);
}

/// Get the tail PFN (LRU end) of a specific LRU list.
/// Returns 0 if the list is empty.
pub fn lru_tail(lru_type: usize) -> usize {
    // SAFETY: read-only LRU access — no concurrent mutation concern.
    let node = match unsafe { first_online_node_mut() } {
        Some(n) => n,
        None => return 0,
    };
    node.lru_tails[lru_type].load(core::sync::atomic::Ordering::Acquire)
}

/// Collect up to `nr` candidate PFNs from the COLD (tail) end of the given
/// LRU list, oldest first (review 4.12: reclaim must consume the LRU list
/// instead of scanning every page descriptor in the system).
///
/// The chain is walked under the lru_lock; the returned PFNs are then
/// processed by the caller WITHOUT the lock (processing may itself call
/// back into lru_del_page / lru_move_to_tail). Snapshotting under the lock
/// keeps the walk consistent while allowing each page to be unlinked or
/// rotated independently afterwards.
pub fn lru_collect_cold(lru_type: usize, nr: usize) -> Vec<usize> {
    let mut out = Vec::new();

    // SAFETY: node access is for reading; the lock guards the walk.
    let node = match unsafe { first_online_node_mut() } {
        Some(n) => n,
        None => return out,
    };

    let _guard = node.lru_lock.lock();

    let mut cur = node.lru_tails[lru_type].load(core::sync::atomic::Ordering::Relaxed);
    while cur != LRU_NONE && out.len() < nr {
        out.push(cur);
        // SAFETY: cur comes from the (locked) list chain; read-only access.
        unsafe {
            let p = pfn_to_page_mut(cur);
            if p.is_null() {
                break;
            }
            cur = (*p).lru_prev();
        }
    }

    out
}
