// ==================== Per-CPU pagesets (pcp) ====================
//
// Order-0 frame caching per CPU, mirroring the reference kernel's
// zone->per_cpu_pageset: on the alloc/free fast path a frame moves
// between the pcp array and the caller with NO zone-lock acquisition;
// the zone lock is paid once per PCP_BATCH frames (refill/drain).
//
// Why this exists (perf review 2026-10-31): the wave-6 cleanup removed
// an earlier never-wired pcp module, leaving every order-0 alloc/free
// a full round trip on the (effectively global — one zone) zone lock,
// with the page-descriptor init loop of alloc_pages_locked inside it.
// Page faults, COW, page-cache fills and mm teardown all serialize on
// that single lock. This module re-introduces the pageset with a
// design that fits Rux's invariants, where the removed one did not:
//
// INV-PCP-1 (three-state frame lifecycle). Every order-0 managed frame
//   is in exactly one of: buddy freelist (OnFreelist, rc==0), pcp cache
//   (OnPcp, rc==0), or in use (neither bit, rc>=1). The pcp paths
//   maintain this by construction and refuse (leak + taint) any free
//   that would violate it.
//
// INV-PCP-2 (counting). Zone.free_pages counts ALL free frames — buddy
//   lists AND pcp caches (the reference-kernel semantics, and what
//   watermark checks want). pcp entry adds 1, pcp exit subtracts 1;
//   refill/drain round trips through the zone net to zero on the pcp
//   side, so the zone lock paths' existing +/- stay untouched.
//
// INV-PCP-3 (cpu locality = safety). A pcp row is touched only by its
//   own CPU with interrupts disabled; nested entry (interrupt handler
//   allocating/freeing while the interrupted context is mid-pcp) is
//   SAFE by design — same-CPU nesting is serial under irq-off, so
//   unlike CpuLocal this module has no in_use refusal and no taint on
//   nesting. The only atomicity needed is the row's own count, which
//   the irq-off window makes exclusive.
//
// SAFETY (static rows): rows are addressed only by cpu_id() with IRQs
// disabled (same-CPU exclusivity above); cross-CPU access happens only
// via the count peeks, which read a single usize through a raw pointer
// — never a &mut. No `&'static mut` escapes this module (the audit
// failure that killed the wave-6 module).

use super::page_desc::{pfn_to_page_mut, PageFlag};
use super::zone::Zone;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Frames moved to/from the zone per refill/drain round trip.
pub const PCP_BATCH: usize = 16;
/// Row high-water mark: at this many cached frames a drain returns
/// PCP_BATCH frames to the buddy.
const PCP_HIGH: usize = 2 * PCP_BATCH;

#[repr(C, align(64))]
struct PcpPageset {
    /// Cached order-0 PFNs. Slots [0..count) are live.
    list: [usize; PCP_HIGH],
    count: usize,
}

impl PcpPageset {
    const fn new() -> Self {
        Self {
            list: [0; PCP_HIGH],
            count: 0,
        }
    }
}

/// Storage row. `UnsafeCell` + manual `Sync`: cross-CPU access never
/// forms a reference (counts are peeked through raw pointers only);
/// mutable access is confined to the owning CPU inside its irq-off
/// window (INV-PCP-3), which serializes nested entry on the same CPU.
#[repr(C, align(64))]
struct PcpRow(UnsafeCell<PcpPageset>);
unsafe impl Sync for PcpRow {}

static PCP_ROWS: [PcpRow; MAX_CPUS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ROW: PcpRow = PcpRow(UnsafeCell::new(PcpPageset::new()));
    [ROW; MAX_CPUS]
};

use crate::config::MAX_CPUS;

// Observability: fast-path hit/miss and drain/refill round trips.
pub static PCP_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PCP_MISSES: AtomicUsize = AtomicUsize::new(0);
pub static PCP_REFILLS: AtomicUsize = AtomicUsize::new(0);
pub static PCP_DRAINS: AtomicUsize = AtomicUsize::new(0);
/// Double-free attempts against a pcp-cached frame (INV-PCP-1 tripwire).
pub static PCP_DOUBLE_FREE: AtomicUsize = AtomicUsize::new(0);

/// Run `f` with exclusive access to this CPU's row (IRQs off).
fn with_row<R>(f: impl FnOnce(&mut PcpPageset) -> R) -> Option<R> {
    // SAFETY: irq-off window grants same-CPU exclusivity (INV-PCP-3);
    // nested entry is serial and safe, so no in_use guard is needed.
    let flags = crate::arch::cpu::save_and_disable_irq();
    let c = crate::arch::cpu_id() as usize;
    let out = if c < MAX_CPUS {
        // SAFETY: UnsafeCell::get() hands out the raw row pointer; the
        // &mut derived from it is exclusive because only this CPU can
        // touch the row and interrupts are disabled (INV-PCP-3).
        let row = unsafe { &*core::ptr::addr_of!(PCP_ROWS[c]) }.0.get();
        Some(f(unsafe { &mut *row }))
    } else {
        None
    };
    crate::arch::cpu::restore_irq(flags);
    out
}

/// Allocate one order-0 frame, pcp first.
///
/// `zone` is the caller's resolved ZoneNormal (from alloc_pages_inner).
/// On pcp miss this refills the row with one zone-lock batch and hands
/// out the first frame; on refill failure (zone exhausted) returns None
/// so the caller falls through to the slow path (kswapd / direct
/// reclaim) unchanged.
pub fn alloc_order0(zone: &Zone) -> Option<usize> {
    let got = with_row(|row| {
        if row.count > 0 {
            row.count -= 1;
            let pfn = row.list[row.count];
            row.list[row.count] = 0;
            Some(pfn)
        } else {
            None
        }
    });
    if let Some(Some(pfn)) = got {
        PCP_HITS.fetch_add(1, Ordering::Relaxed);
        // INV-PCP-2: this frame was counted free while cached.
        zone.free_pages_sub(1);
        to_in_use(pfn);
        return Some(pfn);
    }
    PCP_MISSES.fetch_add(1, Ordering::Relaxed);

    // Refill: one zone-lock acquisition initializes PCP_BATCH frames
    // (alloc_pages_locked semantics — the MM-H2 invariant holds because
    // init stays inside the zone lock).
    let mut batch = [0usize; PCP_BATCH];
    let n = zone.alloc_pages_order0_batch(PCP_BATCH, &mut batch);
    if n == 0 {
        return None;
    }
    PCP_REFILLS.fetch_add(1, Ordering::Relaxed);
    let pfn = batch[0];
    // Overflow list: frames that no longer fit (an interrupt-context
    // free_order0 storm between the miss window and this refill window
    // can leave count>0 — up to PCP_HIGH after the spill fixup). Rows
    // that fit go to the row; the rest return to the zone directly,
    // keeping every list index < PCP_HIGH (the wfG panic class).
    let mut overflow = [0usize; PCP_BATCH];
    let mut n_overflow = 0usize;
    let keep = n - 1;
    with_row(|row| {
        for i in 0..keep {
            let p = batch[1 + i];
            if row.count < PCP_HIGH {
                row.list[row.count] = p;
                row.count += 1;
                to_cached(p);
            } else {
                overflow[n_overflow] = p;
                n_overflow += 1;
            }
        }
    });
    if n > 1 {
        // INV-PCP-2: n-1 frames are back to "free" (cached) state; the
        // handed-out frame was already decremented by alloc_pages_locked.
        zone.free_pages_add(n - 1);
    }
    if n_overflow > 0 {
        // The overflow frames were counted free above; return them to
        // the buddy now (they were never cached in a row).
        zone.free_pages_sub(n_overflow);
        for i in 0..n_overflow {
            let pg = pfn_to_page_mut(overflow[i]);
            if !pg.is_null() {
                unsafe {
                    (*pg).set_refcount(1);
                    (*pg).clear_flag(PageFlag::OnPcp);
                }
            }
            zone.free_pages(overflow[i], 0);
        }
    }
    to_in_use(pfn);
    Some(pfn)
}

/// Free one order-0 frame into this CPU's pcp row.
///
/// Returns true when the frame was cached (caller is done); false means
/// "not eligible for the pcp" — the caller falls through to the zone
/// path unchanged. A detected double free is leaked + tainted here and
/// NOT passed to the zone (the buddy has no view of pcp frames).
pub fn free_order0(pfn: usize, zone: &Zone) -> bool {
    let page = pfn_to_page_mut(pfn);
    if page.is_null() {
        return false;
    }
    // SAFETY: descriptor transitions below are the pcp ownership
    // transfer for this frame; exclusivity comes from INV-PCP-1: only
    // one of the three states owns the frame, and every state change
    // validates the previous state first.
    unsafe {
        // Textbook in-use state only; anything else goes to the zone
        // slow path whose tripwires own the diagnosis.
        if (*page).refcount() != 1
            || (*page).test_flag(PageFlag::OnFreelist)
            || (*page).order() != 0
        {
            return false;
        }
        if (*page).test_flag(PageFlag::OnPcp) {
            // Double free of a pcp-cached frame. Leak it (stay cached,
            // never hand out twice) and name it, mirroring the buddy's
            // R15-3 stance. No printk from this path (free contexts hold
            // locks; floods wedge the console — sc83 discipline).
            PCP_DOUBLE_FREE.fetch_add(1, Ordering::Relaxed);
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            return true;
        }
        // In use -> cached.
        (*page).set_refcount(0);
        (*page).clear_flag(PageFlag::Referenced);
        (*page).set_flag(PageFlag::OnPcp);
    }
    // INV-PCP-2: the frame is free again (cached).
    zone.free_pages_add(1);

    // Spill handling is ONE irq-off window: detect the high-water mark,
    // copy the drain batch out, and fix `count` up atomically. The old
    // two-window form (detect, then drain in a second with_row) left
    // `count == PCP_HIGH == list.len()` visible with IRQs enabled
    // between the windows; an interrupt-context free_order0 in that gap
    // executed `row.list[row.count] = pfn` with count==32 — the release
    // OH boot wfG panic "index out of bounds: the len is 32 but the
    // index is 32" at 108.6 s. Zone-side accounting (free_pages_sub +
    // the 16 zone.free_pages calls) stays OUTSIDE the window exactly as
    // before — only the 16 usize copies moved inside, and a nested
    // same-CPU entry remains serial (INV-PCP-3).
    //
    // lb250 (2026-11-02, verify roller): the 35336eb1 refill guard can
    // leave count == PCP_HIGH EXACTLY (its stores stop at the bound, so
    // a 31-count row refilled to full parks ON 32 with no drain pending
    // — the intended spill never runs because that store is not a
    // free_order0 entry). This entry store must therefore be bound
    // checked too: a full row spills the OLDEST PCP_BATCH out of the
    // list and sends the INCOMING frame straight to the buddy (its +1
    // accounting and OnPcp transition already happened above, so it
    // joins the drain accounting as +1/-1, not a row member).
    let mut pfns = [0usize; PCP_BATCH];
    let mut do_drain = false;
    let mut spill_direct = false;
    with_row(|row| {
        if row.count < PCP_HIGH {
            row.list[row.count] = pfn;
            row.count += 1;
        } else {
            // Row left full by a capped refill: the incoming frame
            // bypasses the list (spill_direct below).
            spill_direct = true;
        }
        if row.count >= PCP_HIGH {
            let first = row.count - PCP_BATCH;
            for i in 0..PCP_BATCH {
                pfns[i] = row.list[first + i];
            }
            row.count = first;
            do_drain = true;
        }
    });

    if do_drain {
        // Drain PCP_BATCH frames back to the buddy: undo the pcp-side
        // +1 per frame, then let zone.free_pages re-add them (its own
        // merge/list logic untouched).
        PCP_DRAINS.fetch_add(1, Ordering::Relaxed);
        zone.free_pages_sub(PCP_BATCH);
        for p in pfns.iter() {
            // SAFETY: frame was cached (OnPcp) — transition back through
            // the zone's own free path, which relinks and merges it.
            let pg = pfn_to_page_mut(*p);
            if !pg.is_null() {
                unsafe {
                    (*pg).clear_flag(PageFlag::OnPcp);
                }
            }
            zone.free_pages(*p, 0);
        }
    }
    if spill_direct {
        // The incoming frame never entered the row: undo its pcp-side
        // +1 and hand it to the zone directly (same transition shape as
        // the drain loop's frames).
        PCP_DRAINS.fetch_add(1, Ordering::Relaxed);
        zone.free_pages_sub(1);
        let pg = pfn_to_page_mut(pfn);
        if !pg.is_null() {
            unsafe {
                (*pg).clear_flag(PageFlag::OnPcp);
            }
        }
        zone.free_pages(pfn, 0);
    }
    true
}

/// Descriptor transition: cached (OnPcp, rc==0). Caller guarantees the
/// frame is in the "in use" or "just allocated" state.
fn to_cached(pfn: usize) {
    let page = pfn_to_page_mut(pfn);
    if !page.is_null() {
        // SAFETY: refill batch frames come from alloc_pages_locked in
        // the in-use state (rc==1, Referenced); this flips them to the
        // pcp-cached free state.
        unsafe {
            (*page).set_refcount(0);
            (*page).clear_flag(PageFlag::Referenced);
            (*page).set_flag(PageFlag::OnPcp);
        }
    }
}

/// Descriptor transition: in use — bit-identical to the state
/// alloc_pages_locked produces for an order-0 frame.
fn to_in_use(pfn: usize) {
    let page = pfn_to_page_mut(pfn);
    if !page.is_null() {
        // SAFETY: pop from this CPU's row under irq-off; the frame was
        // cached (OnPcp, rc==0).
        unsafe {
            (*page).clear_all_flags();
            (*page).reset_mapcount();
            (*page).set_refcount(1);
            // SP-7 (stress-port r7): no Referenced flag on handout (see
            // the zone.rs alloc-path note — the reference's prep_new_page
            // produces flag-clear pages; the plant made every PCP-served
            // order-0 fault page look hot to reclaim).
            (*page).set_order(0);
        }
    }
}

/// Total frames cached across all pcp rows (meminfo / forensic dumps).
pub fn cached_pages_total() -> usize {
    let mut total = 0usize;
    for c in 0..MAX_CPUS {
        // SAFETY: read-only peek of a single usize through the raw row
        // pointer — no reference is formed; a moment-stale value is
        // acceptable for reporting (the caller is diagnostic).
        let row = unsafe { &*core::ptr::addr_of!(PCP_ROWS[c]) }.0.get();
        total += unsafe { (*row).count };
    }
    total
}

/// Per-CPU cached counts for /proc meminfo's pcp_pages row.
pub fn cached_pages_per_cpu() -> [usize; MAX_CPUS] {
    let mut out = [0usize; MAX_CPUS];
    for c in 0..MAX_CPUS {
        // SAFETY: read-only single-usize peek, see cached_pages_total.
        let row = unsafe { &*core::ptr::addr_of!(PCP_ROWS[c]) }.0.get();
        out[c] = unsafe { (*row).count };
    }
    out
}
