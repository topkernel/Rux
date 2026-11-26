//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
// SPDX-License-Identifier: MIT OR Apache-2.0
//! mmforensics: deep frame-tracking capture at double-tracking events.
//!
//! The pcachewedge lane (2026-11-15) contained the RAW-FREE-OF-LIVE-TABLE
//! cascade with two refuse-and-leak guards (cache-side live-PT release
//! refusal, buddy-vs-pcp double-link refusal), but 1-2 residual echo
//! events per boot remained whose decode required naming the STALE TREE
//! POINTER's origin: which structure still references a frame that
//! re-entered circulation, and when that reference was created.
//!
//! This module is the capture side of `rux:mmforensics (retired debug=mmforensics)` (existing gated
//! family — no new switch). At a double-tracking event it dumps the
//! frame's FULL tracking state:
//!
//!   1. page-descriptor state (refcount/mapcount/order/flags/type/
//!      mapping/index — the buddy + page-cache view),
//!   2. PT-ledger bit + boot set,
//!   3. the frame's incarnation history from the forensic rings
//!      (PT_STAMP_RING — now with the stamping tree's ROOT (§6 residual:
//!      "PT_STAMP_RING has no root field"), PT_FREE_RING, ZTRACE_ALLOC,
//!      ZTRACE_FREE, PTEI_RING data-page installs),
//!   4. per-CPU pageset rows naming the pfn,
//!   5. and the decisive one: a bounded scan of EVERY live task's page
//!      table tree for links pointing at the pfn — the reverse map the
//!      kernel otherwise lacks. A frame referenced by two trees (or twice
//!      by one) is double-tracked; the tree that is NOT the stamping root
//!      holds the stale pointer.
//!
//! Callers (all task-context, no zone/pcp/page-cache locks held at the
//! call site): the RAW-FREE watchpoint in `mm::zone::Zone::free_pages`,
//! the live-PT release refusal in `fs::page_cache::release_page_frame`,
//! the PTLEDGER DOUBLE-FREE refusal and the FUT-STALE-TREE census in the
//! arch teardown walks. The capture itself takes NO locks except the
//! pid-hash bucket locks inside `for_each_task_try` (non-blocking try
//! variant: a bucket held at snapshot time is skipped and reported).
//! Reading foreign trees is racy-by-design forensics: every link is
//! bounds-checked (`phys_valid` + RAM top) before descent and every
//! walk carries a hard entry budget.

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Per-boot cap on captures: the events occur <=2/boot; four captures
/// carry the evidence, more would only flood the serial console.
static CAPTURES: AtomicUsize = AtomicUsize::new(0);

// ==================== ref-transition ring (early-put hunt, 2026-11-15) ====================
//
// The mmforensics capture answers "what does the frame look like NOW";
// the ORIGINAL-producer hunt (why a PCACHE mapping's descriptor
// reference is consumed before unmap) additionally needs WHO performed
// every refcount transition on the frame in the past. Every mm-side
// put on a mapped leaf runs through `Page::put_page_site` (driftwatch
// attribution); installs and fork copies run through `Page::get_page`.
// This ring records BOTH for page-cache-typed frames only (the other
// get/put users — shm/ashmem/binder/io_uring/perf — hold their own
// frames), gated by the mmforensics switch so standard boots pay one
// relaxed load per transition. The capture's ring replay then prints
// the get/put sequence with sites: a put whose `old` shows the mapping
// reference already missing names the early consumer directly.

// RefRingEntry — the definition moved to debug/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use super::types::{RefRingEntry};

// 8192 entries (256 KiB of statics): a fammm boot's PageCache mapping
// transitions can push far past 1024 entries between the early put and
// the capture that replays them — the deeper ring keeps the window.
const REF_RING_LEN: usize = 8192;
pub static REF_RING: [RefRingEntry; REF_RING_LEN] =
    [const {
        RefRingEntry {
            ppn: AtomicU64::new(0),
            kind_site: AtomicU64::new(0),
            old: AtomicI32::new(0),
            mc: AtomicI32::new(0),
            pid: AtomicU32::new(0),
        }
    }; REF_RING_LEN];
pub static REF_CUR: AtomicUsize = AtomicUsize::new(0);

/// Record a get/put transition on a page-cache-typed descriptor.
/// `kind`: 0 = get_page, 1 = put (site-attributed). No-op unless
/// `rux:mmforensics (retired debug=mmforensics)` is armed; takes no locks (safe under PTE locks).
#[inline]
pub fn note_ref(page: *const crate::mm::types::Page, kind: u8, site: u8, old: i32) {
    if !crate::trace::EVENT_MMFORENSICS.armed() {
        return;
    }
    // SAFETY: the caller passes a valid page descriptor it is operating on.
    let p = unsafe { &*page };
    use crate::mm::types::PageType;
    if p.page_type() != PageType::PageCache {
        return;
    }
    let ppn = crate::mm::hotpath::page_to_pfn(page) as u64;
    let idx = REF_CUR.fetch_add(1, Ordering::Relaxed) % REF_RING_LEN;
    let e = &REF_RING[idx];
    e.ppn.store(ppn, Ordering::Relaxed);
    e.kind_site.store(((kind as u64) << 8) | site as u64, Ordering::Relaxed);
    e.old.store(old, Ordering::Relaxed);
    e.mc.store(p.mapcount(), Ordering::Relaxed);
    e.pid.store(crate::process::hotpath::current_pid(), Ordering::Relaxed);
}

// TreeHit — the definition moved to debug/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use super::types::{TreeHit};

/// Current task's page-table root (0 when the context has no user mm).
fn current_root_ppn() -> u64 {
    crate::sched::hotpath::current()
        .and_then(|t| t.address_space_arc())
        .map(|mm| mm.root_ppn())
        .unwrap_or(0)
}

/// Replay every forensic ring naming `ppn`, newest first. The rings are
/// per-arch statics with identical names/shapes on x86_64 and riscv64
/// (PT_STAMP_RING / PT_FREE_RING / PTEI_RING under
/// `arch::mm::mmu_init`, ZTRACE_ALLOC / ZTRACE_FREE under `mm::zone`),
/// so this compiles once for both. Each replay is capped (the first
/// matches carry the history; more would flood the console).
#[cfg(any(feature = "x86_64", feature = "riscv64"))]
fn ring_history(ppn: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    const MAX_PER_RING: usize = 6;

    // Table stamps (alloc side; root recorded when mmforensics armed).
    let cur = crate::arch::mm::mmu_init::PT_STAMP_CURSOR.load(Relaxed);
    let ring = &crate::arch::mm::mmu_init::PT_STAMP_RING;
    let mut n = 0;
    for k in 0..ring.len() {
        let idx = (cur + ring.len() - 1 - k) % ring.len();
        if ring[idx].ppn.load(Relaxed) == ppn {
            crate::pr_err!(
                "  hist stamp[age {}]: ppn={:#x} by pid={} root={:#x}",
                k,
                ppn,
                ring[idx].pid.load(Relaxed),
                ring[idx].root.load(Relaxed)
            );
            n += 1;
            if n >= MAX_PER_RING {
                break;
            }
        }
    }
    if n == 0 {
        crate::pr_err!("  hist stamp: no ring entry names this ppn");
    }

    // Table frees (take side: pid, teardown site, freeing root).
    let cur = crate::arch::mm::mmu_init::PT_FREE_CURSOR.load(Relaxed);
    let ring = &crate::arch::mm::mmu_init::PT_FREE_RING;
    let mut n = 0;
    for k in 0..ring.len() {
        let idx = (cur + ring.len() - 1 - k) % ring.len();
        if ring[idx].ppn.load(Relaxed) == ppn {
            crate::pr_err!(
                "  hist free[age {}]: ppn={:#x} by pid={} site={} root={:#x}",
                k,
                ppn,
                ring[idx].pid.load(Relaxed),
                ring[idx].site.load(Relaxed) as u8 as char,
                ring[idx].root.load(Relaxed)
            );
            n += 1;
            if n >= MAX_PER_RING {
                break;
            }
        }
    }
    if n == 0 {
        crate::pr_err!("  hist free: no ring entry names this ppn");
    }

    // Zone allocator ledger (raw pfn<<4|order entries; no pid).
    for (ring, cur, name) in [
        (
            &crate::mm::hotpath::ZTRACE_ALLOC,
            &crate::mm::hotpath::ZALLOC_CUR,
            "zalloc",
        ),
        (
            &crate::mm::hotpath::ZTRACE_FREE,
            &crate::mm::hotpath::ZFREE_CUR,
            "zfree",
        ),
    ] {
        let c = cur.load(Relaxed);
        let mut n = 0;
        for k in 0..ring.len() {
            let idx = (c + ring.len() - 1 - k) % ring.len();
            let v = ring[idx].load(Relaxed);
            if v != 0 && (v >> 4) == (ppn as u64) {
                crate::pr_err!(
                    "  hist {}[age {}]: pfn={:#x} order={}",
                    name,
                    k,
                    v >> 4,
                    v & 0xF
                );
                n += 1;
                if n >= 2 {
                    break;
                }
            }
        }
    }

    // User PTE installs naming this ppn as DATA (root, va) — records
    // whether the frame also lived as a mapped data page.
    let cur = crate::arch::mm::mmu_init::PTEI_CUR.load(Relaxed);
    let ring = &crate::arch::mm::mmu_init::PTEI_RING;
    let mut n = 0;
    for k in 0..ring.len() {
        let idx = (cur + ring.len() - 1 - k) % ring.len();
        if ring[idx].ppn.load(Relaxed) == ppn {
            crate::pr_err!(
                "  hist pte-install[age {}]: ppn={:#x} root={:#x} va={:#x}",
                k,
                ppn,
                ring[idx].root.load(Relaxed),
                ring[idx].va.load(Relaxed)
            );
            n += 1;
            if n >= MAX_PER_RING {
                break;
            }
        }
    }

    // Refcount transitions (early-put hunt): the get/put sequence with
    // driftwatch sites. Newest first; the full relevant window matters,
    // so this replay prints more matches than the others.
    let site_name = |s: u64| -> &'static str {
        match s {
            0 => "TEARDOWN_RV",
            1 => "TEARDOWN_X86_L1",
            2 => "TEARDOWN_X86_L2",
            3 => "TEARDOWN_X86_L3",
            4 => "COW_RV",
            5 => "COW_X86",
            6 => "UNMAP_X86",
            7 => "UNMAP_RV",
            8 => "VMSCAN_SWAP",
            9 => "SHM_DROP",
            10 => "ASHMEM_DROP",
            11 => "BINDER_DROP",
            12 => "BINDER_ZAP",
            13 => "IOURING_DROP",
            14 => "PAGECACHE",
            _ => "OTHER",
        }
    };
    let cur = REF_CUR.load(Relaxed);
    let mut n = 0;
    for k in 0..REF_RING_LEN {
        let idx = (cur + REF_RING_LEN - 1 - k) % REF_RING_LEN;
        if REF_RING[idx].ppn.load(Relaxed) == ppn {
            let ks = REF_RING[idx].kind_site.load(Relaxed);
            let (kind, site) = ((ks >> 8) as u8, (ks & 0xFF) as u64);
            crate::pr_err!(
                "  hist ref[age {}]: ppn={:#x} {} site={} old={} mc={} pid={}",
                k,
                ppn,
                if kind == 0 { "GET" } else { "PUT" },
                // GETs carry no driftwatch site (they are installs/fork
                // copies); only PUTs are site-attributed.
                if kind == 0 { "install" } else { site_name(site) },
                REF_RING[idx].old.load(Relaxed),
                REF_RING[idx].mc.load(Relaxed),
                REF_RING[idx].pid.load(Relaxed)
            );
            n += 1;
            if n >= 16 {
                break;
            }
        }
    }
    if n == 0 {
        crate::pr_err!("  hist ref: no recorded transition names this ppn");
    }
}

/// Deep capture of one frame's tracking state. Rate-limited; a no-op
/// unless `rux:mmforensics (retired debug=mmforensics)` armed the switch.
pub fn capture_frame(pfn: usize, why: &'static str) {
    if !crate::trace::EVENT_MMFORENSICS.armed() {
        return;
    }
    if CAPTURES.fetch_add(1, Ordering::Relaxed) >= 4 {
        return;
    }
    let pid = crate::process::hotpath::current_pid();
    crate::pr_err!(
        "=== MMFORENSICS capture #{}: pfn={:#x} why={} pid={} root={:#x} ===",
        CAPTURES.load(Ordering::Relaxed),
        pfn,
        why,
        pid,
        current_root_ppn()
    );

    // ---- 1. page-descriptor state (buddy + page-cache view) ----
    let page = crate::mm::hotpath::pfn_to_page(pfn);
    if page.is_null() {
        crate::pr_err!("  desc: NO DESCRIPTOR (pfn outside managed range)");
    } else {
        // SAFETY: read-only descriptor access on the diagnostic path.
        unsafe {
            use crate::mm::types::PageFlag;
            let p = &*page;
            let bit = |f: PageFlag, n: &'static str| -> &'static str {
                if p.test_flag(f) {
                    n
                } else {
                    ""
                }
            };
            crate::pr_err!(
                "  desc: refcount={} mapcount={} order={} mapped={} type={} mapping={:#x} index={} flags:[{}{}{}{}{}{}{}{}]",
                p.refcount(),
                p.mapcount(),
                p.order(),
                p.is_mapped(),
                p.page_type() as u32,
                p.mapping() as usize,
                p.index(),
                bit(PageFlag::OnFreelist, "OnFreelist "),
                bit(PageFlag::OnPcp, "OnPcp "),
                bit(PageFlag::Lru, "Lru "),
                bit(PageFlag::Anonymous, "Anon "),
                bit(PageFlag::Dirty, "Dirty "),
                bit(PageFlag::UpToDate, "UpToDate "),
                bit(PageFlag::Reserved, "Reserved "),
                bit(PageFlag::Active, "Active "),
            );
        }
    }

    // ---- 2. PT ledger ----
    let ppn = pfn as u64;
    crate::pr_err!(
        "  ledger: live={} boot={}",
        crate::arch::mm::mmu_init::PtLedger::is_live_table(ppn),
        crate::arch::mm::mmu_init::PT_LEDGER.is_boot(ppn),
    );

    // ---- 3. incarnation history (forensic rings, newest first) ----
    #[cfg(any(feature = "x86_64", feature = "riscv64"))]
    ring_history(ppn);
    #[cfg(feature = "aarch64")]
    crate::pr_err!("  hist: (no forensic rings on aarch64)");

    // ---- 4. per-CPU pageset rows ----
    let mut rows = [0usize; 8];
    let n = crate::mm::hotpath::forensics_scan_rows(pfn, &mut rows);
    if n > 0 {
        crate::pr_err!(
            "  pcp: pfn sits in CPU row(s) {:?} (still pcp-cached!)",
            &rows[..n]
        );
    } else {
        crate::pr_err!("  pcp: not present in any CPU row");
    }

    // ---- 5. bounded reverse scan of every live tree ----
    #[cfg(any(feature = "x86_64", feature = "riscv64"))]
    {
        let mut hits = [
            TreeHit {
                pid: 0,
                root_ppn: 0,
                level: 0,
                idx0: 0,
                idx1: 0,
            };
            16
        ];
        let (nh, tasks, skipped, budget) =
            crate::arch::mm::mmu_init::forensics_scan_trees(ppn, &mut hits);
        for h in hits.iter().take(nh) {
            let kind = match h.level {
                0 => "ROOT frame",
                1..=3 => "upper-table LINK",
                _ => "LEAF (data mapping)",
            };
            crate::pr_err!(
                "  tree-ref: pid={} root={:#x} level={} idx0={} idx1={} ({})",
                h.pid, h.root_ppn, h.level, h.idx0, h.idx1, kind
            );
        }
        crate::pr_err!(
            "  tree-scan: {} reference(s) over {} tasks ({} buckets skipped, {} entries read)",
            nh, tasks, skipped, budget
        );
    }
    #[cfg(feature = "aarch64")]
    crate::pr_err!("  tree-scan: (not implemented on aarch64)");
    crate::pr_err!("=== MMFORENSICS capture end ===");
}
