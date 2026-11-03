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

use core::sync::atomic::Ordering;

use super::page_desc::{PageFlag, pfn_to_page_mut};
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

// ==================== SP-7: per-page swap skip-reason census ====================
//
// The round-6 exhaustion re-adjudication left one thread: with 237k
// anonymous pages on LRU_INACTIVE_ANON and swap active, only ~611 swap
// writes completed per boot. The reference kernel under the same load
// swaps aggressively. This census counts, per candidate page examined
// by reclaim_anonymous_pages, EXACTLY which filter rejected it (or
// which terminal path it took) — the skip reason names the fix.
//
// Cost: one relaxed fetch_add per filter branch on the reclaim path
// (only exercised under memory pressure), read by the OOM forensic
// dump and a 1 Hz delta heartbeat. Zero output on standard boots.

/// The census counter set. All monotonically increasing.
#[derive(Default)]
pub struct VscanCensus {
    /// reclaim_anonymous_pages invocations.
    pub calls: u64,
    /// Sum of requested nr_to_scan (priority-derived scan budget).
    pub want: u64,
    /// Sum of candidates lru_collect_cold actually returned.
    pub got: u64,
    /// Candidates whose descriptor pointer was null.
    pub null_page: u64,
    /// !is_anonymous.
    pub not_anon: u64,
    /// !SwapBacked.
    pub not_sb: u64,
    /// !is_mapped (mapcount at or below bias).
    pub unmapped: u64,
    /// refcount != 1 — split by observed value 2 / 3 / >=4.
    pub rc2: u64,
    pub rc3: u64,
    pub rc4p: u64,
    /// Last observed non-1 refcount (context for the buckets).
    pub last_rc: u32,
    /// is_reserved || is_locked.
    pub rsv_lock: u64,
    /// Referenced set → cleared + rotated back to the collect end.
    pub rot_ref: u64,
    /// swap_alloc_slot returned None (break — swap full).
    pub slot_none: u64,
    /// Post-slot-allocation TOCTOU recheck rejected the page.
    pub toctou: u64,
    /// try_to_unmap replaced zero PTEs (page not found in any page table).
    pub unmap0: u64,
    /// SP-8 split of unmap0: mapcount STILL >= bias+1 after the failed
    /// walk (the page really is mapped somewhere the slots did not
    /// reach — the missed-walk face: stale/degraded slots, VMA-type
    /// miss). These pages stay on the LRU and are re-skipped each scan.
    pub u0_live: u64,
    /// SP-8 split of unmap0: mapcount already back at/below bias by the
    /// arm (a teardown raced between the filter and the walk — the page
    /// is on its way out; benign transient, self-correcting).
    pub u0_dead: u64,
    /// SP-8 split of unmap0: RmapOverflow flag set (the 4-slot table
    /// degraded to single-value recording — the known architectural
    /// limit; expected to carry the live-skip residue after the
    /// FileBacked-COW VMA-filter fix).
    pub u0_ovf: u64,
    /// Partial unmap (slot leaked).
    pub partial: u64,
    /// swap_write_page returned Err.
    pub w_err: u64,
    /// swap_write_page returned Ok (single- and multi-mapping arms).
    pub w_ok: u64,
    /// put_page after swap-out did NOT reach 0 (slot leaked).
    pub put_nz: u64,
    /// Pages actually freed via the swap-out path.
    pub freed: u64,
}

macro_rules! vscan_ctr {
    ($name:ident) => {
        static $name: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
    };
}

vscan_ctr!(VS_CALLS);
vscan_ctr!(VS_WANT);
vscan_ctr!(VS_GOT);
vscan_ctr!(VS_NULLP);
vscan_ctr!(VS_NOTANON);
vscan_ctr!(VS_NOTSB);
vscan_ctr!(VS_UNMAPPED);
vscan_ctr!(VS_RC2);
vscan_ctr!(VS_RC3);
vscan_ctr!(VS_RC4P);
static VS_LAST_RC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
vscan_ctr!(VS_RSVLOCK);
vscan_ctr!(VS_ROTREF);
vscan_ctr!(VS_SLOTNONE);
vscan_ctr!(VS_TOCTOU);
vscan_ctr!(VS_UNMAP0);
vscan_ctr!(VS_U0LIVE);
vscan_ctr!(VS_U0DEAD);
vscan_ctr!(VS_U0OVF);
vscan_ctr!(VS_PARTIAL);
vscan_ctr!(VS_WERR);
vscan_ctr!(VS_WOK);
vscan_ctr!(VS_PUTNZ);
vscan_ctr!(VS_FREED);

/// Read the whole census (the OOM dump prints it verbatim).
pub fn vscan_census() -> VscanCensus {
    use core::sync::atomic::Ordering::Relaxed;
    VscanCensus {
        calls: VS_CALLS.load(Relaxed),
        want: VS_WANT.load(Relaxed),
        got: VS_GOT.load(Relaxed),
        null_page: VS_NULLP.load(Relaxed),
        not_anon: VS_NOTANON.load(Relaxed),
        not_sb: VS_NOTSB.load(Relaxed),
        unmapped: VS_UNMAPPED.load(Relaxed),
        rc2: VS_RC2.load(Relaxed),
        rc3: VS_RC3.load(Relaxed),
        rc4p: VS_RC4P.load(Relaxed),
        last_rc: VS_LAST_RC.load(Relaxed),
        rsv_lock: VS_RSVLOCK.load(Relaxed),
        rot_ref: VS_ROTREF.load(Relaxed),
        slot_none: VS_SLOTNONE.load(Relaxed),
        toctou: VS_TOCTOU.load(Relaxed),
        unmap0: VS_UNMAP0.load(Relaxed),
        u0_live: VS_U0LIVE.load(Relaxed),
        u0_dead: VS_U0DEAD.load(Relaxed),
        u0_ovf: VS_U0OVF.load(Relaxed),
        partial: VS_PARTIAL.load(Relaxed),
        w_err: VS_WERR.load(Relaxed),
        w_ok: VS_WOK.load(Relaxed),
        put_nz: VS_PUTNZ.load(Relaxed),
        freed: VS_FREED.load(Relaxed),
    }
}

/// One compact census line (the OOM dump and the heartbeat share it).
pub fn vscan_census_line() -> alloc::string::String {
    use alloc::format;
    let c = vscan_census();
    format!(
        "VSCAN-CENSUS calls={} want={} got={} nullp={} notanon={} notsb={} unmapped={} rc(2/3/4+ last)={}/{}/{} {} rsvlock={} rotref={} slotnone={} toctou={} unmap0={} u0(live/dead/ovf)={}/{}/{} partial={} wok={} werr={} putnz={} freed={}",
        c.calls, c.want, c.got, c.null_page, c.not_anon, c.not_sb,
        c.unmapped, c.rc2, c.rc3, c.rc4p, c.last_rc, c.rsv_lock,
        c.rot_ref, c.slot_none, c.toctou, c.unmap0, c.u0_live,
        c.u0_dead, c.u0_ovf, c.partial,
        c.w_ok, c.w_err, c.put_nz, c.freed
    )
}

/// SP-7 heartbeat: print the census line once per second while new
/// anon-scan activity accrues (self-gated — the line is silent unless
/// reclaim_anonymous_pages ran since the last heartbeat). Sits next to
/// the SP-5 reclaim-gate census on the 1 Hz timer heartbeat; standard
/// boots (no anon pressure) never print.
pub fn vscan_heartbeat() {
    use core::sync::atomic::Ordering::Relaxed;
    const VS_HB_BUDGET: u64 = 300;
    static LAST_CALLS: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    static PRINTS: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let calls = VS_CALLS.load(Relaxed);
    if calls == LAST_CALLS.load(Relaxed) {
        return; // no anon scan activity since the last heartbeat
    }
    LAST_CALLS.store(calls, Relaxed);
    if PRINTS.fetch_add(1, Relaxed) >= VS_HB_BUDGET {
        return; // bounded: an hours-long storm cannot flood the console
    }
    crate::pr_info!("{}", vscan_census_line());
}


// ==================== Public API ====================

/// Reclaim batch size (Linux SWAP_CLUSTER_MAX).
///
/// Reclaiming exactly the allocation size (`1 << order` = ONE page for the
/// order-0 fault path) made every allocation under sustained pressure pay
/// a full pass through the priority machinery per page freed: once the
/// page cache legitimately grows to RAM scale (the dynamic page-cache
/// cap), the allocator slowpath's 8 retry rounds moved 8 pages per
/// allocation and the whole system crawled (OH boot: zero SA .so loads in
/// 9 minutes, one CPU saturated in reclaim). Freeing a 32-page batch per
/// pass is Linux's shape and amortizes the scan cost.
const SWAP_CLUSTER_MAX: usize = 32;

/// Top-level reclaim entry point.
///
/// Runs a priority-loop following `balance_pgdat()` in mm/vmscan.c.
/// Returns the number of pages reclaimed.
pub fn balance_pgdat(order: i32) -> usize {
    let mut sc = ScanControl {
        nr_to_reclaim: core::cmp::max(1 << order.max(0) as usize, SWAP_CLUSTER_MAX),
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

    // SP-5 waiter-side progress witness (page_alloc's gate wait): pages
    // freed by ANY reclaimer — kswapd here, or the direct-reclaim gate
    // holder through try_to_free_pages — tell stalled allocations that
    // the system IS making progress, so they keep retrying instead of
    // failing (reference should_reclaim_retry semantics).
    if sc.nr_reclaimed > 0 {
        super::page_alloc::RECLAIM_PROGRESS.fetch_add(sc.nr_reclaimed as u64, core::sync::atomic::Ordering::Relaxed);
    }
    sc.nr_reclaimed
}

/// Direct reclaim: try to free enough pages for an allocation of `order`.
///
/// Called from the page allocator when the zone is below WMARK_LOW.
/// Returns the number of pages reclaimed.
pub fn try_to_free_pages(order: i32) -> usize {
    // Leg-15 pace: cumulative round count + jiffies spent here, read by
    // the schbd heartbeat (dfx=schb). No lock held; note_drct is a
    // counter tick unless the switch is off.
    let j0 = crate::drivers::timer::get_jiffies();
    super::page_alloc::gate_note_phase(super::page_alloc::GATE_PHASE_ROUND);
    let r = balance_pgdat(order);
    // SP-5 census: round progress + pages freed by the gate holder (the
    // stamps are no-ops for kswapd — gate_note_phase checks identity).
    if r > 0 {
        super::page_alloc::gate_note_round_done(r as u64);
    }
    crate::dfx::schb::note_drct(j0);
    r
}

// ==================== Deep pre-OOM drain (stress-fleet s17, leg 49B) ====

/// Counters for the deep pre-OOM drain — the change's own instrument
/// (the merge gate's "add the DFX tool that measures it" duty). They
/// stay zero on every standard boot (the drain runs only on the
/// doomed slowpath, after the 8 steady-state rounds have failed to
/// satisfy the allocation); the storm fleet reads them on the serial
/// OOM-DEEP-PASS line and the census can assert the zero.
pub static DEEP_PASS_RUNS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static DEEP_PASS_FREED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static DEEP_PASS_DEMOTED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Deep final pre-OOM reclaim round (design: perf-report.md 8.10.19;
/// the stress lane's s17 evidence: the kill fired while 175-204 MB of
/// clean file cache sat on the LRUs — the pre-OOM protocol drained at
/// most ~8x32 = 256 pages, two orders of magnitude short).
///
/// WHY this is a separate function and not a bigger SWAP_CLUSTER_MAX:
/// the steady-state bounds (32-page batch, 8 rounds, MAX_SCAN=64 per
/// evict_one, break-on-failure in shrink) are pace/anti-crawl measures
/// for the COMMON slowpath — they stay untouched. All extra depth lives
/// here, on the path where the allocation is about to fail anyway:
///
///   - chunked `shrink_reclaim` (the direct-reclaim variant with dirty
///     writeback — same contract as shrink_inactive_list's caller: task
///     context, IN_DIRECT_RECLAIM held by the caller);
///   - a BULK active-list demotion between starved chunks: the active
///     file list (graduated clean pages — referenced=0, dirty=0 in the
///     storm censuses) is NEVER scanned by shrink_lruvec ("TODO:
///     shrink_active_list"); it is reachable only through the
///     32-per-call demotion bridge, which lands pages at the young end
///     BEHIND the referenced wall. Demoting in bulk here makes the
///     one-list-away pool reachable at batch rate.
///
/// Stops when `nr_to_reclaim` pages are freed, or one full pass frees
/// nothing AND demotes nothing (the pool is genuinely dry — pinning,
/// dirtiness, or references everywhere — and the kill that follows is
/// then correct, not fake). Bounded by `MAX_PASSES` regardless.
///
/// Returns pages freed. Prints one OOM-DEEP-PASS line per invocation
/// (the doomed path already prints forensics; standard boots never run
/// here, so ordinary serial stays clean).
pub fn drain_file_cache_before_oom(nr_to_reclaim: usize) -> usize {
    const CHUNK: usize = 256;
    const DEMOTE_BATCH: usize = 256;
    const MAX_PASSES: usize = 64;

    DEEP_PASS_RUNS.fetch_add(1, Ordering::Relaxed);

    let cache = crate::fs::page_cache::get_page_cache();
    let mut freed_total = 0usize;
    let mut demoted_total = 0usize;
    let mut passes = 0usize;

    while freed_total < nr_to_reclaim && passes < MAX_PASSES {
        passes += 1;
        super::page_alloc::gate_note_deep_pass(0);
        let want = core::cmp::min(CHUNK, nr_to_reclaim - freed_total);
        let freed = cache.shrink_reclaim(want);
        freed_total += freed;
        super::page_alloc::gate_note_deep_pass(freed as u64);
        if freed < want {
            // Chunk starved: the inactive walk hit the referenced wall
            // (evict_one's LRU rounds exhaust MAX_SCAN graduating pages)
            // and the map fallback is O(cache) per single page — bridge
            // the active list in bulk so the next chunk has material.
            demoted_total += lru::lru_demote_active_file_oldest(DEMOTE_BATCH);
        }
        if freed == 0 && demoted_total == 0 {
            // A full pass with zero progress on both axes: nothing
            // clean is reachable. More depth cannot help; the OOM
            // decision that follows is exercising a genuinely dry pool.
            break;
        }
        // freed==0 with demoted>0 keeps looping one more pass (the
        // demotion itself is progress toward evictable pages).
    }

    DEEP_PASS_FREED.fetch_add(freed_total as u64, Ordering::Relaxed);
    DEEP_PASS_DEMOTED.fetch_add(demoted_total as u64, Ordering::Relaxed);
    // Rate-limited report line (10 jiffies = 100 ms at HZ=100): a storm
    // of doomed allocations each run the drain, and unbounded prints
    // would burn serial bandwidth at exactly the moment the machine is
    // dying. The counters above keep the full accounting.
    static LAST_PRINT_J: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST_PRINT_J.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= 10
        && LAST_PRINT_J
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        crate::pr_info!(
            "OOM-DEEP-PASS target={} freed={} demoted={} passes={} runs={} tot_freed={} (deep pre-OOM file-cache drain)",
            nr_to_reclaim,
            freed_total,
            demoted_total,
            passes,
            DEEP_PASS_RUNS.load(Ordering::Relaxed),
            DEEP_PASS_FREED.load(Ordering::Relaxed)
        );
    }
    freed_total
}

// ==================== Internal: shrink_* ====================

/// Per-node reclaim: iterate zones that are below watermark.
fn shrink_node(sc: &mut ScanControl) {
    // SAFETY: called from kswapd/balance_pgdat — exclusive reclaim context.
    let node = match unsafe { first_online_node_mut() } {
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
        // Reclaim page cache pages via the shrinker interface. Direct
        // reclaim only (this path is the page_alloc slowpath, task
        // context, IN_DIRECT_RECLAIM-serialized): shrink_reclaim adds
        // dirty-frame writeback so abandoned unflushed MAP_SHARED pages
        // become evictable instead of a permanent drain. The mmu_init
        // PT-alloc retry must NOT use this variant (per-mm PteLock held).
        super::page_alloc::gate_note_phase(super::page_alloc::GATE_PHASE_SHRINK);
        let cache_reclaimed =
            crate::fs::page_cache::get_page_cache().shrink_reclaim(nr_to_scan);
        sc.nr_reclaimed += cache_reclaimed;
    } else if lru == LRU_INACTIVE_ANON && sc.may_unmap && swap::nr_active_swap() {
        // Reclaim anonymous pages via swap
        super::page_alloc::gate_note_phase(super::page_alloc::GATE_PHASE_ANON);
        let c0 = vscan_census();
        let anon_reclaimed = reclaim_anonymous_pages(nr_to_scan, sc);
        sc.nr_reclaimed += anon_reclaimed;
        // The reference-shaped batch tracepoint (see the declaration's
        // divergence note in kernel/src/trace/mod.rs). Argument evaluation
        // is two census reads (relaxed atomics) — paid only on the anon
        // reclaim path, never on any standard-boot path.
        let c1 = vscan_census();
        let d = |a: u64, b: u64| a.saturating_sub(b) as i64;
        crate::trace::trace_mm_vmscan_lru_shrink_inactive(
            0,
            nr_to_scan as i64,
            anon_reclaimed as i64,
            0,
            0,
            0,
            0,
            d(c1.rot_ref, c0.rot_ref),
            d(c1.unmap0, c0.unmap0)
                + d(c1.partial, c0.partial)
                + d(c1.toctou, c0.toctou),
            sc.priority,
        );
    }

    sc.nr_scanned += nr_to_scan;
}

/// Reclaim anonymous pages: swap them out and free the physical frames.
///
/// Review 4.12: candidates come from the LRU_INACTIVE_ANON LIST (cold/tail
/// end first) instead of a linear scan over every page descriptor in the
/// system.
///
/// For each candidate:
///   1. Allocate a swap slot
///   2. EXCLUSIVELY-OWNED single-mapping pages: replace the PTE with the
///      swap entry FIRST, then write the page to the device. Installing the
///      entry before the write closes the torn-write window — with the old
///      write-first order, user stores landing in the page DURING the block
///      write were silently lost when the PTE was replaced afterwards
///      (review 4.12). A fault in the small [PTE-installed, write-done]
///      window waits on the slot's pending-write mark (swap.rs).
///      Multi-mapping pages keep the legacy write-first order (the
///      swap-in/free-slot race on the remaining PTEs is not handled
///      without a swap cache).
///   3. Free the physical page
fn reclaim_anonymous_pages(nr_to_scan: usize, sc: &mut ScanControl) -> usize {
    use super::rmap::try_to_unmap_with_swap;
    use core::sync::atomic::Ordering::Relaxed;

    let mut reclaimed = 0usize;

    // SP-7 skip-reason census: invocation + collect-vs-want accounting
    // (a want >> got gap means the LRU list itself is short; a large
    // got with a dominant skip bucket names the filter).
    VS_CALLS.fetch_add(1, Relaxed);
    VS_WANT.fetch_add(nr_to_scan as u64, Relaxed);

    let candidates = super::lru::lru_collect_cold(LRU_INACTIVE_ANON, nr_to_scan);
    VS_GOT.fetch_add(candidates.len() as u64, Relaxed);

    for pfn in candidates {
        if reclaimed >= nr_to_scan {
            break;
        }

        let page = pfn_to_page_mut(pfn);
        if page.is_null() {
            VS_NULLP.fetch_add(1, Relaxed);
            continue;
        }

        // SAFETY: page is non-null (checked above); pfn is within [MIN_PFN, MAX_PFN).
        unsafe {
            let p = &*page;

            // Skip non-anonymous pages
            if !p.is_anonymous() {
                VS_NOTANON.fetch_add(1, Relaxed);
                continue;
            }

            // Must be swap-backed (set by page_add_anon_rmap)
            if !p.test_flag(PageFlag::SwapBacked) {
                VS_NOTSB.fetch_add(1, Relaxed);
                continue;
            }

            // Skip unmapped pages
            if !p.is_mapped() {
                VS_UNMAPPED.fetch_add(1, Relaxed);
                continue;
            }

            // Only reclaim EXCLUSIVELY OWNED pages (refcount == 1): the
            // swap-out path replaces PTEs with a swap entry and then drops
            // the page reference — with refcount > 1 (fork COW share) the
            // put_page() never reaches zero and the old code freed the swap
            // slot while the installed PTEs still referenced it (slot UAF,
            // review MM-C3).
            if p.refcount() != 1 {
                // SP-7: bucket the observed value — rc2 is the fork-COW
                // face, rc4+ is a pinning/leak face; last_rc carries the
                // exact value of the most recent reject.
                let rc = p.refcount();
                VS_LAST_RC.store(rc as u32, Relaxed);
                match rc {
                    2 => VS_RC2.fetch_add(1, Relaxed),
                    3 => VS_RC3.fetch_add(1, Relaxed),
                    _ => VS_RC4P.fetch_add(1, Relaxed),
                };
                continue;
            }

            // Skip reserved/locked pages
            if p.is_reserved() || p.is_locked() {
                VS_RSVLOCK.fetch_add(1, Relaxed);
                continue;
            }

            // Check referenced flag (recently accessed — give it another
            // chance at the MRU end instead of just clearing in place).
            // SP-7 (stress-port r7): the MRU end is the HEAD (the same
            // young end `page_add_anon_lru` links at; see link_head_
            // locked's policy doc). The old call moved the page to the
            // TAIL — the COLLECT end — so every rotation fed the page
            // straight back to the next scan (and the measured rotref
            // share of the scan budget was 46%, flags planted by the
            // allocator, not accesses). The reference kernel protects a
            // referenced inactive page at the young end (activate / head
            // of inactive); head-rotation is the single-list form of
            // that protection.
            if p.test_flag(PageFlag::Referenced) {
                p.clear_flag(PageFlag::Referenced);
                super::lru::lru_move_to_head(p, LRU_INACTIVE_ANON);
                VS_ROTREF.fetch_add(1, Relaxed);
                sc.nr_scanned += 1;
                continue;
            }

            sc.nr_scanned += 1;

            // Allocate a swap slot
            let (swap_type, swap_offset) = match swap::swap_alloc_slot() {
                Some(slot) => slot,
                None => {
                    VS_SLOTNONE.fetch_add(1, Relaxed);
                    break; // No swap space left
                }
            };

            // Re-check page state after potential preemption during swap
            // allocation — the page may have been freed or remapped (TOCTOU guard, fixes H32).
            if !p.is_anonymous()
                || !p.test_flag(PageFlag::SwapBacked)
                || !p.is_mapped()
                || p.refcount() != 1
                || p.is_reserved()
                || p.is_locked()
                || p.test_flag(PageFlag::Referenced)
            {
                swap::swap_free_slot(swap_type, swap_offset);
                VS_TOCTOU.fetch_add(1, Relaxed);
                continue;
            }

            // Build the swap entry that will be stored in PTEs
            let swap_entry = swap::make_swap_entry(swap_type, swap_offset);

            // Single mapping (mapcount == 0) → unmap-first (see doc comment).
            // Multiple mappings → legacy write-first order.
            let single_mapping = p.mapcount() == 0;

            let phys = pfn_to_phys(pfn);

            if single_mapping {
                // Install the swap entry in the PTE (page becomes unreachable
                // for user stores), mark the slot write-in-flight, then write.
                // The pending mark MUST precede the PTE install: the window
                // [entry-installed, mark-set] let a racing fault swap-in the
                // slot's PREVIOUS device content (another page's bytes — a
                // fresh anon page arriving dirty with foreign data, the OH
                // leg-22 recycler evidence class). Paths that do not reach
                // the write must clear the mark (swap slot leak arms below).
                swap::swap_mark_pending(swap_type, swap_offset);
                let unmapped = try_to_unmap_with_swap(p, swap_entry);
                if unmapped > 0 && !p.is_mapped() {
                    super::page_alloc::gate_note_phase(
                        super::page_alloc::GATE_PHASE_SWAPW,
                    );
                    let write_ok = swap::swap_write_page(swap_type, swap_offset, phys).is_ok();
                    swap::swap_clear_pending(swap_type, swap_offset);
                    if write_ok {
                        VS_WOK.fetch_add(1, Relaxed);
                    } else {
                        VS_WERR.fetch_add(1, Relaxed);
                    }

                    if !write_ok {
                        // The PTE already holds the entry — nothing to roll
                        // back to. Leak the slot (freeing it would UAF the
                        // swap-in path) and drop the page; the data is lost
                        // to the I/O error either way.
                        crate::pr_warn!(
                            "vmscan: swap write failed (type={}, off={}): slot leaked",
                            swap_type, swap_offset
                        );
                    }

                    p.clear_flag(PageFlag::Anonymous);
                    p.clear_flag(PageFlag::SwapBacked);
                    p.set_index(0);

                    // Remove from LRU
                    super::lru::page_remove_lru(p);

                    // Drop reference; free if last holder
                    let refcount = p.put_page_site(crate::dfx::driftwatch::S_VMSCAN_SWAP);
                    // R14-1 (F10): free ONLY when WE took it to zero.
                    if refcount == 0 {
                        free_page(phys);
                        reclaimed += 1;
                        VS_FREED.fetch_add(1, Relaxed);
                    } else {
                        VS_PUTNZ.fetch_add(1, Relaxed);
                        crate::pr_warn!(
                            "vmscan: swapped-out page refcount={} (slot leaked)",
                            refcount
                        );
                    }
                } else if unmapped == 0 {
                    // No PTE was replaced: the entry was never installed,
                    // so the slot is genuinely unused — clear the pre-set
                    // in-flight mark and free it.
                    swap::swap_clear_pending(swap_type, swap_offset);
                    swap::swap_free_slot(swap_type, swap_offset);
                    VS_UNMAP0.fetch_add(1, Relaxed);
                    // SP-8 split: dead = a teardown raced the filter→walk
                    // window (mapcount already back at/below bias; the
                    // page leaves the LRU on its own — benign). live =
                    // the page is still mapped and the walk missed it
                    // (stale/degraded slots — the missed-walk face; the
                    // page stays LRU-linked and is re-skipped each scan).
                    if p.mapcount() < 0 {
                        VS_U0DEAD.fetch_add(1, Relaxed);
                    } else {
                        VS_U0LIVE.fetch_add(1, Relaxed);
                    }
                    if p.test_flag(PageFlag::RmapOverflow) {
                        VS_U0OVF.fetch_add(1, Relaxed);
                    }
                } else {
                    // Partial unmap: at least one PTE now references the swap
                    // entry — freeing the slot would be a UAF. Leak it (and
                    // clear the in-flight mark: the write never happened, so
                    // a swap-in must not wait on it).
                    swap::swap_clear_pending(swap_type, swap_offset);
                    VS_PARTIAL.fetch_add(1, Relaxed);
                    crate::pr_warn!(
                        "vmscan: partial unmap during swap-out (slot leaked)"
                    );
                }
            } else {
                // Multi-mapping page — legacy order: write first, then unmap.
                super::page_alloc::gate_note_phase(super::page_alloc::GATE_PHASE_SWAPW);
                if swap::swap_write_page(swap_type, swap_offset, phys).is_err() {
                    // Write failed — free the slot and skip this page
                    swap::swap_free_slot(swap_type, swap_offset);
                    VS_WERR.fetch_add(1, Relaxed);
                    continue;
                }
                VS_WOK.fetch_add(1, Relaxed);

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
                    let refcount = p.put_page_site(crate::dfx::driftwatch::S_VMSCAN_SWAP);
                    if refcount == 0 {
                        free_page(phys);
                        reclaimed += 1;
                        VS_FREED.fetch_add(1, Relaxed);
                    } else {
                        VS_PUTNZ.fetch_add(1, Relaxed);
                        crate::pr_warn!(
                            "vmscan: swapped-out page refcount={} (slot leaked)",
                            refcount
                        );
                    }
                } else if unmapped == 0 {
                    swap::swap_free_slot(swap_type, swap_offset);
                    VS_UNMAP0.fetch_add(1, Relaxed);
                    // SP-8 split (same classification as the
                    // single-mapping arm above).
                    if p.mapcount() < 0 {
                        VS_U0DEAD.fetch_add(1, Relaxed);
                    } else {
                        VS_U0LIVE.fetch_add(1, Relaxed);
                    }
                    if p.test_flag(PageFlag::RmapOverflow) {
                        VS_U0OVF.fetch_add(1, Relaxed);
                    }
                } else {
                    VS_PARTIAL.fetch_add(1, Relaxed);
                    crate::pr_warn!(
                        "vmscan: partial unmap during swap-out (slot leaked)"
                    );
                }
            }
        }
    }

    reclaimed
}
