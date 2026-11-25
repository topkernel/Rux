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

#![forbid(unsafe_code)]

extern crate alloc;
use alloc::vec::Vec;

use rux::mm::{types::Page, types::PageFlag, hotpath::page_to_pfn};
use rux::mm::types::NR_LRU_LISTS;

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
fn unlink_locked(
    node: &rux::mm::types::PglistData,
    page: &Page,
    lru: usize,
) {
    let prev = page.lru_prev();
    let next = page.lru_next();

    if prev != LRU_NONE {
        if let Some(prev_page) = rux::mm::r#unsafe::page_of(prev) {
            prev_page.set_lru_next(next);
        }
    } else {
        // Page was the head
        node.lru_heads[lru].store(next, core::sync::atomic::Ordering::Relaxed);
    }

    if next != LRU_NONE {
        if let Some(next_page) = rux::mm::r#unsafe::page_of(next) {
            next_page.set_lru_prev(prev);
        }
    } else {
        // Page was the tail
        node.lru_tails[lru].store(prev, core::sync::atomic::Ordering::Relaxed);
    }

    page.set_lru_next(LRU_NONE);
    page.set_lru_prev(LRU_NONE);
    // FLOOR AT ZERO (pcachewedge lane, 2026-11-15): a plain fetch_sub
    // wraps on underflow (a double unlink of a corrupted descriptor),
    // and the wrapped ~usize::MAX class size later panicked
    // /proc/meminfo's debug-build multiply (meminfo.rs:45). A size
    // counter never legitimately goes below zero — clamp it.
    {
        use core::sync::atomic::Ordering::Relaxed;
        let mut cur = node.lru_sizes[lru].load(Relaxed);
        loop {
            if cur == 0 {
                break;
            }
            match node.lru_sizes[lru].compare_exchange_weak(
                cur,
                cur - 1,
                core::sync::atomic::Ordering::AcqRel,
                Relaxed,
            ) {
                Ok(_) => break,
                Err(now) => cur = now,
            }
        }
    }
}

/// O(1) link of `page` at the TAIL of LRU list `lru` — caller holds
/// `lru_lock`.
///
/// # Safety
/// `page` must not currently be linked on any list.
fn link_tail_locked(
    node: &rux::mm::types::PglistData,
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
        if let Some(tail_page) = rux::mm::r#unsafe::page_of(tail) {
            tail_page.set_lru_next(pfn);
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

/// O(1) link of `page` at the HEAD of LRU list `lru` — caller holds
/// `lru_lock`.
///
/// Head is the young end in the two-tier file-LRU policy (see
/// `page_add_file_lru_young`): pages drift toward the tail as newer pages
/// arrive, so the tail stays the oldest end reclaim consumes.
///
/// # Safety
/// `page` must not currently be linked on any list.
fn link_head_locked(
    node: &rux::mm::types::PglistData,
    page: &Page,
    lru: usize,
) {
    let pfn = page_to_pfn(page as *const Page);

    // New page becomes the new head (no prev)
    page.set_lru_next(LRU_NONE);
    page.set_lru_prev(LRU_NONE);
    page.set_lru_list(lru);

    let head = node.lru_heads[lru].load(core::sync::atomic::Ordering::Relaxed);

    if head != LRU_NONE {
        // Link old head → new page
        if let Some(head_page) = rux::mm::r#unsafe::page_of(head) {
            head_page.set_lru_prev(pfn);
        }
        page.set_lru_next(head);
        node.lru_heads[lru].store(pfn, core::sync::atomic::Ordering::Relaxed);
    } else {
        // List was empty — new page is both head and tail
        node.lru_heads[lru].store(pfn, core::sync::atomic::Ordering::Relaxed);
        node.lru_tails[lru].store(pfn, core::sync::atomic::Ordering::Relaxed);
    }

    node.lru_sizes[lru].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Clear the on-page LRU membership bits (the Lru flag and list index) —
/// small helper shared by the unlink-then-relink policy paths.
fn page_clear_lru_bits(page: &Page) {
    page.clear_flag(PageFlag::Lru);
    page.clear_lru_list();
}

/// Add a page to the *tail* of the specified LRU list.
///
/// The tail is the least-recently-used end; kswapd scans from here.
pub fn lru_add_page(page: &Page, lru_type: usize) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };

    let _guard = node.lru_lock.lock();

    // Page is not linked (checked via the Lru flag) and the lock is
    // held for the whole link.
    if !page.test_flag(PageFlag::Lru) {
        link_tail_locked(node, page, lru_type);
        page.set_flag(PageFlag::Lru);
    }
}

/// Remove a page from its LRU list — O(1) via lru_prev (review 4.11).
pub fn lru_del_page(page: &Page) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match rux::mm::r#unsafe::first_node_mut() {
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

    // Page carries the Lru flag and a valid list index; lock held.
    unlink_locked(node, page, lru);

    page.clear_flag(PageFlag::Lru);
    page.clear_lru_list();
}

/// Move a page to the tail of a (possibly different) LRU list — O(1).
///
/// Acquires the LRU lock once for both the unlink and the relink.
pub fn lru_move_to_tail(page: &Page, new_lru: usize) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };

    let _guard = node.lru_lock.lock();

    // Unlink only when the page really is linked; lock held.
    if page.test_flag(PageFlag::Lru) {
        let lru = page.lru_list();
        if lru < NR_LRU_LISTS {
            unlink_locked(node, page, lru);
            page.clear_flag(PageFlag::Lru);
            page.clear_lru_list();
        }
    }
    link_tail_locked(node, page, new_lru);
    page.set_flag(PageFlag::Lru);
}

/// Move a page to the head of a (possibly different) LRU list — the young
/// end in the two-tier file-LRU policy (see `page_add_file_lru_young`).
///
/// Acquires the LRU lock once for both the unlink and the relink.
pub fn lru_move_to_head(page: &Page, new_lru: usize) {
    // SAFETY: called with lru_lock held — exclusive node access.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };

    let _guard = node.lru_lock.lock();
    // Unlink only when the page really is linked; lock held.
    if page.test_flag(PageFlag::Lru) {
        let lru = page.lru_list();
        if lru < NR_LRU_LISTS {
            unlink_locked(node, page, lru);
            page_clear_lru_bits(page);
        }
    }
    link_head_locked(node, page, new_lru);
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
            rux::mm::types::LRU_INACTIVE_ANON
        } else {
            rux::mm::types::LRU_INACTIVE_FILE
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
            rux::mm::types::LRU_ACTIVE_ANON
        } else {
            rux::mm::types::LRU_ACTIVE_FILE
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
    match rux::mm::r#unsafe::first_node_mut() {
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

/// Add an anonymous page to LRU_INACTIVE_ANON on first mapping — at the
/// HEAD (young end).
///
/// SP-7 (stress-port r7): the reference kernel adds newly faulted pages
/// at the HEAD of the inactive list (lru_cache_add → list_add) and
/// isolates reclaim candidates from the TAIL, so the tail stays the
/// oldest end. Rux linked anon pages at the TAIL — the SAME end
/// `lru_collect_cold` consumes — so under an anon fault storm the scan
/// window was permanently occupied by freshly-faulted pages while the
/// genuinely-old pages could never reach it (measured: 11 reclaim
/// invocations examined 3,589 of 245,515 anon-LRU pages in the
/// exhaustion boot; the SP-7 census banked the distribution). The file
/// side already uses this shape (`page_add_file_lru_young` →
/// link_head_locked, whose doc spells out head-young/tail-old).
pub fn page_add_anon_lru(page: &Page) {
    // SAFETY: same node-access discipline as lru_add_page.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };
    let _guard = node.lru_lock.lock();
    // Page is not linked (Lru flag checked) and the lock is held.
    if !page.test_flag(PageFlag::Lru) {
        link_head_locked(node, page, rux::mm::types::LRU_INACTIVE_ANON);
        page.set_flag(PageFlag::Lru);
    }
}

/// Add a file-backed page to LRU_INACTIVE_FILE on first mapping.
pub fn page_add_file_lru(page: &Page) {
    lru_add_page(page, rux::mm::types::LRU_INACTIVE_FILE);
}

/// Link a batch of pages to LRU_INACTIVE_FILE under ONE lru_lock
/// (bulk companion of `page_add_file_lru` for page-cache batch inserts,
/// where per-page lock/unlock dominated the insert cost under TCG).
pub fn page_add_file_lru_batch(pages: &[*mut Page]) {
    // SAFETY: same node-access discipline as lru_add_page.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };
    let _guard = node.lru_lock.lock();
    for page_ptr in pages {
        if page_ptr.is_null() {
            continue;
        }
        // Caller owns these frames (fresh allocations); the lock is
        // held for the whole batch, and the Lru-flag check keeps double
        // links impossible — same contract as lru_add_page
        // (INV-MM-2 owned-frame arm).
        if let Some(page) = rux::mm::r#unsafe::page_ref_of(
            rux::mm::hotpath::page_to_pfn(*page_ptr)
        ) {
            if !page.test_flag(PageFlag::Lru) {
                link_tail_locked(node, page, rux::mm::types::LRU_INACTIVE_FILE);
                page.set_flag(PageFlag::Lru);
            }
        }
    }
}

// ==================== Two-tier file-LRU policy (page cache) ====================
//
// The functions below implement the aging direction the page cache relies
// on. They are ADDITIVE: the legacy tail-link helpers above stay unchanged
// for their existing callers (rmap, anon reclaim).
//
// Direction contract (matching Linux's head=young, tail=old):
//   - Freshly cached file pages enter at the HEAD (young end).
//   - Reclaim scans and demotion consume from the TAIL (old end).
// The pre-two-tier code linked new pages at the TAIL — the same end
// `evict_one` scanned from — so every eviction examined the NEWEST page
// first (an anti-LRU: concurrent fillers evicted each other's freshest
// pages, the OH .so-load thrash family).

/// Link `page` at the HEAD (young end) of LRU_INACTIVE_FILE.
pub fn page_add_file_lru_young(page: &Page) {
    // SAFETY: same node-access discipline as lru_add_page.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };
    let _guard = node.lru_lock.lock();
    // Page is not linked (Lru flag checked) and the lock is held.
    if !page.test_flag(PageFlag::Lru) {
        link_head_locked(node, page, rux::mm::types::LRU_INACTIVE_FILE);
        page.set_flag(PageFlag::Lru);
    }
}

/// Batch version of `page_add_file_lru_young`: link each page at the head
/// of LRU_INACTIVE_FILE under ONE lru_lock. Pages are linked in slice
/// order, each at the head — so the LAST slice element ends up closest to
/// the head (youngest) and the first element is the oldest of the batch.
pub fn page_add_file_lru_young_batch(pages: &[*mut Page]) {
    // SAFETY: same node-access discipline as lru_add_page.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return,
    };
    let _guard = node.lru_lock.lock();
    for page_ptr in pages {
        if page_ptr.is_null() {
            continue;
        }
        // Caller owns these frames (fresh allocations); the Lru flag
        // check keeps double links impossible — same contract as
        // page_add_file_lru_batch (INV-MM-2 owned-frame arm).
        if let Some(page) = rux::mm::r#unsafe::page_ref_of(
            rux::mm::hotpath::page_to_pfn(*page_ptr)
        ) {
            if !page.test_flag(PageFlag::Lru) {
                link_head_locked(node, page, rux::mm::types::LRU_INACTIVE_FILE);
                page.set_flag(PageFlag::Lru);
            }
        }
    }
}

/// Activate a referenced inactive file page onto the HEAD (young end) of
/// LRU_ACTIVE_FILE, clearing nothing — the caller clears Referenced.
///
/// Linking at the young end makes the ACTIVE tail the "oldest active"
/// end, which is where `lru_demote_active_file_oldest` demotes from.
pub fn lru_activate_young(page: &Page) {
    if !page.test_flag(PageFlag::Lru) || page.test_flag(PageFlag::Unevictable) {
        return;
    }
    if !page.test_flag(PageFlag::Active) {
        page.set_flag(PageFlag::Active);
    }
    lru_move_to_head(page, rux::mm::types::LRU_ACTIVE_FILE);
}

/// Demote up to `nr` of the OLDEST pages on LRU_ACTIVE_FILE back onto the
/// HEAD (young end) of LRU_INACTIVE_FILE, clearing Referenced — the
/// second-chance handoff that keeps eviction making progress when every
/// inactive page is pinned or hot.
///
/// Returns the number of pages demoted (0 when the active list is empty
/// or its oldest members are unevictable).
pub fn lru_demote_active_file_oldest(nr: usize) -> usize {
    if nr == 0 {
        return 0;
    }
    // SAFETY: same node-access discipline as lru_collect_cold.
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return 0,
    };
    let _guard = node.lru_lock.lock();
    let mut demoted = 0usize;
    let mut cur = node.lru_tails[rux::mm::types::LRU_ACTIVE_FILE]
        .load(core::sync::atomic::Ordering::Relaxed);
    while cur != LRU_NONE && demoted < nr {
        // cur comes from the (locked) list chain (INV-MM-2 chain arm).
        let p = match rux::mm::r#unsafe::page_ref_of(cur) {
            Some(p) => p,
            None => break,
        };
        let prev = p.lru_prev(); // toward the head = toward younger
        if p.test_flag(PageFlag::Unevictable) {
            cur = prev;
            continue;
        }
        // Page is linked on the ACTIVE list; the lock is held for the
        // unlink+relink (the link/unlink contracts).
        {
            let lru = p.lru_list();
            if lru < NR_LRU_LISTS {
                unlink_locked(node, p, lru);
                page_clear_lru_bits(p);
            }
            p.clear_flag(PageFlag::Referenced);
            p.clear_flag(PageFlag::Active);
            link_head_locked(node, p, rux::mm::types::LRU_INACTIVE_FILE);
            p.set_flag(PageFlag::Lru);
        }
        demoted += 1;
        cur = prev;
    }
    demoted
}

/// Remove a page from its LRU list when the last mapping is removed.
pub fn page_remove_lru(page: &Page) {
    lru_del_page(page);
}

/// Get the tail PFN (LRU end) of a specific LRU list.
/// Returns 0 if the list is empty.
pub fn lru_tail(lru_type: usize) -> usize {
    // SAFETY: read-only LRU access — no concurrent mutation concern.
    let node = match rux::mm::r#unsafe::first_node_mut() {
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
    let node = match rux::mm::r#unsafe::first_node_mut() {
        Some(n) => n,
        None => return out,
    };

    let _guard = node.lru_lock.lock();

    let mut cur = node.lru_tails[lru_type].load(core::sync::atomic::Ordering::Relaxed);
    while cur != LRU_NONE && out.len() < nr {
        out.push(cur);
        // cur comes from the (locked) list chain; read-only access
        // (INV-MM-2 chain arm).
        match rux::mm::r#unsafe::page_ref_of(cur) {
            Some(p) => cur = p.lru_prev(),
            None => break,
        }
    }

    out
}
