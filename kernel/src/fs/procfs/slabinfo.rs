//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/slabinfo — the slab-cache inventory (the reference kernel's
//! mm/slab_common.c slabinfo_show; header and row bytes are 5.15's).
//!
//! The reference file, one row per kmem_cache:
//!
//! ```text
//! slabinfo - version: 2.1
//! # name            <active_objs> <num_objs> <objsize> <objperslab> <pagesperslab> : tunables <limit> <batchcount> <sharedfactor> : slabdata <active_slabs> <num_slabs> <sharedavail>
//! kmalloc-8              868      912      8    510      1 : tunables    0    0    0 : slabdata      2      2      0
//! ```
//!
//! Row geometry (mm/slab_common.c cache_show): `%-17s %6lu %6lu %6u
//! %4u %4d`, then ` : tunables %4u %4u %4u`, then ` : slabdata %6lu
//! %6lu %6lu`. SLUB reports the tunables as zeros (get_slabinfo in
//! mm/slub.c never writes limit/batchcount/shared over the memset) and
//! active_slabs == num_slabs; sharedavail is the memset zero.
//!
//! Rux mapping — every number is a live counter, nothing is invented:
//!
//! - Rux serves kmalloc through TWO packing layers: the fixed-region
//!   caches (mm/slab.rs, 16 MiB front, classes 8..4096) and the
//!   buddy-backed classes (mm/small_alloc.rs, elastic backstore for the
//!   traffic that overflows the region, classes 8..1024). Both hold
//!   real objects of the same kmalloc-<S> bin family, so one row per
//!   size class carries the SUM of the two layers.
//! - active_objs: live objects, per-class allocs-frees from each
//!   layer's own monotone counters. mm/slab.rs bumps them under the
//!   cache lock (exact); small_alloc bumps them in the locked bodies
//!   only, so magazine residency skews the estimate by at most
//!   MAX_CPUS*8*MAG_DEPTH entries (the census prints the same numbers).
//! - num_objs: capacity — live slab pages × objects per page, summed
//!   over the two layers. The two layers' per-page object counts are
//!   identical for every class ((4096 - 16)/S and (4096 - max(16,S))/S
//!   agree under integer division), so one objperslab per row is exact.
//! - objsize: the class size S. objperslab: (PAGE_SIZE-16)/S.
//!   pagesperslab: 1 — every slab page is an order-0 page.
//! - The 4096 class has no row: mm/slab.rs's 4096 cache computes
//!   objects_per_slab == 0 (the 16-byte header leaves no room) and
//!   serves nothing, and layouts that large bypass both layers for
//!   whole buddy pages. A row of all zeros would describe a cache that
//!   does not exist; the 8..2048 rows are the caches that do.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The header line pair, byte-for-byte the reference 5.15
/// print_slabinfo_header (non-DEBUG_SLAB): the version banner, then the
/// column header (the leading "# name" + 12 spaces matches the row's
/// `%-17s ` name field).
const HEADER: &str = "slabinfo - version: 2.1\n\
# name            <active_objs> <num_objs> <objsize> <objperslab> <pagesperslab> : \
tunables <limit> <batchcount> <sharedfactor> : \
slabdata <active_slabs> <num_slabs> <sharedavail>\n";

/// One row in the reference's exact geometry.
///
/// The tunables triple is always `0 0 0` — SLUB's answer too — and
/// active_slabs == num_slabs == the cache's slab-page count, sharedavail
/// 0. Rux has no per-cache tunables; printing the reference's SLUB
/// zeros is the truthful rendering of "no tunables".
fn slabinfo_row(
    name: &str,
    active_objs: u64,
    num_objs: u64,
    objsize: u64,
    objperslab: u64,
) -> String {
    format!(
        "{:<17} {:>6} {:>6} {:>6} {:>4} {:>4} : tunables {:>4} {:>4} {:>4} : slabdata {:>6} {:>6} {:>6}\n",
        name,
        active_objs,
        num_objs,
        objsize,
        objperslab,
        1, // pagesperslab: order-0 slab pages
        0,
        0,
        0, // tunables: limit, batchcount, sharedfactor
        num_objs / objperslab.max(1), // active_slabs
        num_objs / objperslab.max(1), // num_slabs
        0, // sharedavail
    )
}

/// Generate /proc/slabinfo content: one row per live kmalloc size class.
pub fn generate() -> Vec<u8> {
    let mut out = String::from(HEADER);

    // The two layers, per class index (both order their classes small to
    // large; small_alloc stops at 1024).
    let region = crate::mm::slab::slab_stats();
    let buddy = crate::mm::small_alloc::slabinfo_counts();

    for (i, size) in crate::mm::slab::OBJECT_SIZES.iter().enumerate() {
        let rc = &region.cache_stats[i];
        // classes above 1024 have no small_alloc counterpart
        let buddy_idx = if *size <= crate::mm::small_alloc::SMALL_MAX {
            i // identical class order below 1024
        } else {
            usize::MAX
        };
        let (sa_allocs, sa_frees, sa_pages) = if buddy_idx != usize::MAX {
            buddy[buddy_idx]
        } else {
            (0, 0, 0)
        };

        let objperslab = rc.objects_per_slab as u64;
        if objperslab == 0 {
            // The 4096 cache: header + one 4096 object does not fit a
            // page, the cache serves nothing, no row (see module doc).
            continue;
        }

        let active_objs = (rc.alloc_count as u64).saturating_sub(rc.free_count as u64)
            + sa_allocs.saturating_sub(sa_frees);
        let slabs = rc.pages as u64 + sa_pages;
        let num_objs = slabs * objperslab;

        out.push_str(&slabinfo_row(
            &format!("kmalloc-{}", size),
            active_objs,
            num_objs,
            *size as u64,
            objperslab,
        ));
    }
    out.into_bytes()
}

/// Shape pin for /proc/slabinfo: the row's exact BYTES against the
/// reference geometry, so a width or column drift fails here, not in a
/// user's parser. Callable body for the in-kernel unit-test suite.
#[cfg(any(test, feature = "unit-test"))]
pub mod slabinfo_file_tests {
    pub fn snapshot_check() {
        let got = super::slabinfo_row("kmalloc-96", 868, 912, 96, 42);
        // The reference row: name padded to 17, the five counts in
        // 6/6/6/4/4 columns, tunables 0 0 0 in 4-wide columns, slabdata
        // in 6-wide columns.
        assert_eq!(
            got,
            "kmalloc-96           868    912     96   42    1 : tunables    0    0    0 : slabdata     21     21      0\n",
            "/proc/slabinfo row bytes drifted from the reference layout"
        );
    }

    #[cfg(test)]
    #[test]
    fn slabinfo_snapshot() {
        snapshot_check();
    }
}
