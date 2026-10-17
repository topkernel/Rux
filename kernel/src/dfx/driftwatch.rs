//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Zero-put / free-refusal attribution counters (fake-OOM freelist-drift
//! family, dfx=ledger).
//!
//! The MEMLEDGER census proved the drift mechanism — pages reach
//! refcount 0 without ever re-entering the buddy — but a post-hoc census
//! cannot say WHICH release site orphaned them. This module counts, per
//! call site, every `put_page()` that lands the count on 0 (the moment a
//! frame becomes free-able) and every `free_pages()` refusal branch (the
//! paths that drop a free on the floor). The ledger prints the vector;
//! drift attribution is then arithmetic:
//!
//!   sum(ZERO_PUT[site]) - freed_pages - refused_pages = leaked at site
//!
//! The counters are always-on `Relaxed` atomics (one increment per
//! zero-put — rare — and one per refused free); no locks, no printk, safe
//! from any context including the zone-lock-held free path.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Release sites that can land a refcount on 0. Keep in sync with
/// SITE_NAMES and the `driftwatch::note_zero_put(SITE_*)` call sites.
pub const SITES: usize = 16;

pub const S_TEARDOWN_RV: usize = 0; // riscv64 release_user_leaf
pub const S_TEARDOWN_X86_L1: usize = 1; // x86_64 free_user_page_tables PTE leaf
pub const S_TEARDOWN_X86_L2: usize = 2; // x86_64 2MB leaf
pub const S_TEARDOWN_X86_L3: usize = 3; // x86_64 1GB leaf
pub const S_COW_RV: usize = 4; // riscv64 handle_cow_fault old-page put
pub const S_COW_X86: usize = 5; // x86_64 COW old-page put
pub const S_UNMAP_X86: usize = 6; // x86_64 unmap_pages leaf put
pub const S_UNMAP_RV: usize = 7; // riscv64 unmap/teardown aux sites
pub const S_VMSCAN_SWAP: usize = 8; // vmscan swap-out final put
pub const S_SHM_DROP: usize = 9; // sysv_shm ShmPages::drop
pub const S_ASHMEM_DROP: usize = 10; // ashmem AshmemPages::drop
pub const S_BINDER_DROP: usize = 11; // binder BinderAlloc::drop
pub const S_BINDER_ZAP: usize = 12; // binder reserve-window anon zap
pub const S_IOURING_DROP: usize = 13; // io_uring ring region drop
pub const S_PAGECACHE: usize = 14; // page-cache release paths (unused: cache
//                                  frames free via free_page with rc reset
//                                  by the zone, not via put_page)
pub const S_OTHER: usize = 15; // any site not yet tagged

pub const SITE_NAMES: [&str; SITES] = [
    "teardown-rv",
    "teardown-x86-pte",
    "teardown-x86-2m",
    "teardown-x86-1g",
    "cow-rv",
    "cow-x86",
    "unmap-x86",
    "unmap-rv",
    "vmscan-swap",
    "shm-drop",
    "ashmem-drop",
    "binder-drop",
    "binder-zap",
    "iouring-drop",
    "pagecache",
    "other",
];

/// put_page() transitions to 0, per site.
pub static ZERO_PUT: [AtomicUsize; SITES] = [const { AtomicUsize::new(0) }; SITES];

/// free_pages() outcomes (page_alloc::free_pages):
/// 0 = returned to a zone, 1 = refused still-MAPPED, 2 = WILD addr,
/// 3 = refused boot-reserved, 4 = refused text-stamp foreign,
/// 5 = no zone matched (fell off the end).
pub static FREE_OK: AtomicUsize = AtomicUsize::new(0);
pub static FREE_REFUSED_MAPPED: AtomicUsize = AtomicUsize::new(0);
pub static FREE_DROPPED_WILD: AtomicUsize = AtomicUsize::new(0);
pub static FREE_REFUSED_RESERVED: AtomicUsize = AtomicUsize::new(0);
pub static FREE_REFUSED_TEXT: AtomicUsize = AtomicUsize::new(0);
pub static FREE_NO_ZONE: AtomicUsize = AtomicUsize::new(0);
/// free_pages() calls with addr == 0 (legal no-op).
pub static FREE_NULL: AtomicUsize = AtomicUsize::new(0);
/// put_page() underflow refusals (rc already 0) — the inverse drift
/// signal (a page freed twice through the refcount side).
pub static PUT_UNDERFLOW: AtomicUsize = AtomicUsize::new(0);

#[inline]
pub fn note_zero_put(site: usize) {
    ZERO_PUT[site.min(SITES - 1)].fetch_add(1, Ordering::Relaxed);
}

/// One line per nonzero counter (printk path — ledger context only).
pub fn report() {
    for (i, c) in ZERO_PUT.iter().enumerate() {
        let v = c.load(Ordering::Relaxed);
        if v != 0 {
            crate::println!("DRIFTWATCH zero_put[{}] {}={}", i, SITE_NAMES[i], v);
        }
    }
    let v = FREE_OK.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_ok={}", v);
    }
    let v = FREE_REFUSED_MAPPED.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_refused_mapped={}", v);
    }
    let v = FREE_DROPPED_WILD.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_dropped_wild={}", v);
    }
    let v = FREE_REFUSED_RESERVED.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_refused_reserved={}", v);
    }
    let v = FREE_REFUSED_TEXT.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_refused_text={}", v);
    }
    let v = FREE_NO_ZONE.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_no_zone={}", v);
    }
    let v = FREE_NULL.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_null={}", v);
    }
    let v = PUT_UNDERFLOW.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH put_underflow={}", v);
    }
}
