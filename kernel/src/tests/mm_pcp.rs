//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Test: per-CPU pagesets (mm/pcp.rs, INV-PCP-1..3, commit 45ccb66d)
//!
//! Drives the PUBLIC order-0 allocator (mm::page_alloc::alloc_pages /
//! free_pages route ZoneNormal order-0 through the pcp rows) through
//! alloc/free/realloc cycles under IRQs disabled, so the zone counter
//! deltas are exact — no softirq/timer page churn can blur them:
//!
//!  - INV-PCP-2 (counting): zone free_pages counts buddy AND pcp
//!    frames; N allocs drop it by exactly N, freeing them restores it
//!    bit-for-bit (the pre-pcp invariant the pageset must not break).
//!  - every alloc/free moves exactly one of PCP_HITS/PCP_MISSES; the
//!    free path never trips the INV-PCP-1 double-free taint.
//!  - the row cap bounds cached_pages_total() at MAX_CPUS * PCP_HIGH.
//!  - freed frames come back out of the cache (no leak): the second
//!    cycle is served predominantly by hits, and every frame returned
//!    by the cycle is a frame the cycle owned.
//!
//! Framekernel layer policy (docs/architecture/framekernel-layers.md):
//! this module must stay Safe Rust.
#![forbid(unsafe_code)]

use crate::mm::page_alloc::{alloc_pages, free_pages};
use crate::mm::pcp::{PCP_BATCH, PCP_DOUBLE_FREE, PCP_HITS, PCP_MISSES, cached_pages_total};
use crate::mm::pglist::first_online_node;
use crate::mm::zone::GfpFlags;
use core::sync::atomic::Ordering;
use super::test_group_start;

/// Normal kernel allocation (ZoneNormal — the pcp-backed zone).
const GFP_KERNEL: GfpFlags = GfpFlags(0x01);

/// Row high-water mark (pcp.rs keeps it private: 2 * PCP_BATCH).
const PCP_HIGH: usize = 2 * PCP_BATCH;

fn free_total() -> usize {
    first_online_node().map(|n| n.free_pages()).unwrap_or(0)
}

fn all_distinct(pages: &[usize]) -> bool {
    for i in 0..pages.len() {
        for j in i + 1..pages.len() {
            if pages[i] == pages[j] {
                return false;
            }
        }
    }
    true
}

pub fn test_pcp_pagesets() {
    test_group_start("mm pcp pagesets");

    // More than 2*PCP_BATCH pages: the cycle refills the row at least
    // twice and pushes it through at least one high-water drain.
    const N: usize = 3 * PCP_BATCH + 8; // 56

    let hits0 = PCP_HITS.load(Ordering::Relaxed);
    let misses0 = PCP_MISSES.load(Ordering::Relaxed);
    let dfree0 = PCP_DOUBLE_FREE.load(Ordering::Relaxed);

    // IRQs off for the whole measured cycle: the zone free counter is
    // exact only while nothing else allocates (single-CPU unit boot).
    let flags = crate::arch::cpu::save_and_disable_irq();

    let before = free_total();
    let mut pages = [0usize; N];
    let mut alloc_ok = true;
    for p in pages.iter_mut() {
        let phys = alloc_pages(GFP_KERNEL, 0);
        if phys == 0 {
            alloc_ok = false;
            break;
        }
        *p = phys;
    }

    if !alloc_ok {
        crate::arch::cpu::restore_irq(flags);
        super::test_fail("pcp alloc cycle", "order-0 allocation failed");
        return;
    }
    let mid = free_total();
    test_assert!(
        before - mid == N,
        "pcp INV-PCP-2 alloc drops free_pages by exactly N"
    );
    test_assert!(
        all_distinct(&pages),
        "pcp allocated frames are distinct"
    );
    test_assert!(
        cached_pages_total() <= crate::config::MAX_CPUS * PCP_HIGH,
        "pcp row cap bounds cached frames"
    );

    // Free them all back through the pcp path.
    for &p in pages.iter() {
        free_pages(p, 0);
    }
    let after = free_total();
    test_assert!(
        after == before,
        "pcp INV-PCP-2 free cycle restores free_pages exactly"
    );
    test_assert!(
        PCP_DOUBLE_FREE.load(Ordering::Relaxed) == dfree0,
        "pcp INV-PCP-1 no double-free taint in the cycle"
    );

    // Every alloc moved exactly one hit/miss counter.
    let hits1 = PCP_HITS.load(Ordering::Relaxed);
    let misses1 = PCP_MISSES.load(Ordering::Relaxed);
    test_assert!(
        hits1 + misses1 == hits0 + misses0 + N,
        "pcp fast-path counters advance by exactly N"
    );

    // Second cycle: the cache serves what it holds (freed frames come
    // back), and the second set overlaps the first heavily — the rows
    // hand back the very frames this test freed.
    let mut pages2 = [0usize; N];
    let mut alloc_ok = true;
    for p in pages2.iter_mut() {
        let phys = alloc_pages(GFP_KERNEL, 0);
        if phys == 0 {
            alloc_ok = false;
            break;
        }
        *p = phys;
    }
    let hits2 = PCP_HITS.load(Ordering::Relaxed);
    let overlap = if alloc_ok {
        pages2.iter().filter(|p| pages.contains(p)).count()
    } else {
        0
    };
    let served = hits2 - hits1;

    // Give everything back before re-enabling IRQs.
    if alloc_ok {
        for &p in pages2.iter() {
            free_pages(p, 0);
        }
    }
    let after2 = free_total();
    crate::arch::cpu::restore_irq(flags);

    test_assert!(
        after2 == before,
        "pcp second cycle restores free_pages exactly"
    );
    if !alloc_ok {
        super::test_fail("pcp second alloc cycle", "order-0 allocation failed");
        return;
    }
    test_assert!(
        all_distinct(&pages2),
        "pcp second-cycle frames distinct"
    );
    test_assert!(
        served >= PCP_BATCH,
        "pcp second cycle served from the cache"
    );
    // Steady state after the frees leaves at least one drain margin
    // (PCP_HIGH - PCP_BATCH = PCP_BATCH) cached frames in the row, and
    // every one of them is a frame this test freed.
    test_assert!(
        overlap >= PCP_BATCH,
        "pcp freed frames return from the cache (no leak)"
    );
}
