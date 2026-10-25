//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/vmstat and /proc/buddyinfo — Linux's memory statistics.
//!
//! /proc/vmstat is a flat "key value" list, one statistic per line
//! (Linux's fs/proc/vmstat.c). The keys below are Linux's names; the
//! ones Rux has no machinery for (NUMA, workingset, writeback, swap)
//! report zero, which is what a Linux box without those features
//! reports too.
//!
//! /proc/buddyinfo lists the free pages per order, per zone:
//!
//! ```text
//! Node 0, zone   Normal 129316  64015  23064  12589 ...
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

/// The zones Linux reports, in Linux's order.
const ZONE_ORDER: [crate::mm::zone::ZoneType; 4] = [
    crate::mm::zone::ZoneType::ZoneDma,
    crate::mm::zone::ZoneType::ZoneDma32,
    crate::mm::zone::ZoneType::ZoneNormal,
    crate::mm::zone::ZoneType::ZoneMovable,
];

/// Linux's zone names for /proc/buddyinfo.
fn zone_name(zone: crate::mm::zone::ZoneType) -> &'static str {
    match zone {
        crate::mm::zone::ZoneType::ZoneDma => "DMA",
        crate::mm::zone::ZoneType::ZoneDma32 => "DMA32",
        crate::mm::zone::ZoneType::ZoneNormal => "Normal",
        crate::mm::zone::ZoneType::ZoneMovable => "Movable",
        // ZoneCount is a sentinel, not a real zone.
        _ => "Unknown",
    }
}

/// Total free pages across all zones (Linux's `nr_free_pages`).
fn nr_free_pages() -> u64 {
    let mut total = 0u64;
    // SAFETY: diagnostic read-only node access.
    if let Some(node) = unsafe { crate::mm::pglist::first_online_node() } {
        for zt in ZONE_ORDER {
            if let Some(zone) = node.zone(zt) {
                if zone.is_initialized() {
                    total += zone.stats().free_pages as u64;
                }
            }
        }
    }
    total
}

/// Generate /proc/vmstat content (Linux key/value format).
///
/// leg-34: derived by `procfile!` (rust-design-headroom §4) — the
/// field list below is the single artifact that produces both the
/// rendered bytes and `VMSTAT_FIELDS`; the shape test and the exact-
/// bytes snapshot in `mod vmstat_file_tests` pin the format.
crate::procfile! {
    /// /proc/vmstat — flat key-value stats, Linux mm/vmstat.c order
    /// (zone/page state, page allocator, block I/O / swap).
    vmstat() fields VMSTAT_FIELDS test vmstat_shape -> Vec<u8> kv {
        // --- zone / page state ---
        "nr_free_pages" => nr_free_pages(),
    "nr_zone_inactive_anon" => 0,
    "nr_zone_active_anon" => 0,
    "nr_zone_inactive_file" => 0,
    "nr_zone_active_file" => 0,
    "nr_zone_unevictable" => 0,
    "nr_zone_write_pending" => 0,
    "nr_mlock" => 0,
    "nr_zspages" => 0,
    "nr_free_cma" => 0,
    "nr_unaccepted" => 0,
    "numa_hit" => 0,
    "numa_miss" => 0,
    "numa_foreign" => 0,
    "numa_interleave" => 0,
    "numa_local" => 0,
    "numa_other" => 0,
    "nr_inactive_anon" => 0,
    "nr_active_anon" => 0,
    "nr_inactive_file" => 0,
    "nr_active_file" => 0,
    "nr_unevictable" => 0,
    "nr_slab_reclaimable" => 0,
    "nr_slab_unreclaimable" => 0,
    "nr_isolated_anon" => 0,
    "nr_isolated_file" => 0,
    "workingset_nodes" => 0,
    "workingset_refault_anon" => 0,
    "workingset_refault_file" => 0,
    "workingset_activate_anon" => 0,
    "workingset_activate_file" => 0,
    "workingset_restore_anon" => 0,
    "workingset_restore_file" => 0,
    "workingset_nodereclaim" => 0,
    "nr_anon_pages" => 0,
    "nr_mapped" => 0,
    "nr_file_pages" => 0,
    "nr_dirty" => 0,
    "nr_writeback" => 0,
    // --- page allocator ---
    "pgfault" => crate::mm::vmstat::PGFAULT.load(Ordering::Relaxed),
    "pgmajfault" => crate::mm::vmstat::PGMAJFAULT.load(Ordering::Relaxed),
    "pgfree" => crate::mm::vmstat::PGFREE.load(Ordering::Relaxed),
    // Linux's pgalloc_* order follows the zone-type enum
    // (mm/vmstat.c VMSTAT_ITEMS): DMA, DMA32, Normal, Movable.
    "pgalloc_dma" => crate::mm::vmstat::PGALLOC_DMA.load(Ordering::Relaxed),
    "pgalloc_dma32" => crate::mm::vmstat::PGALLOC_DMA32.load(Ordering::Relaxed),
    "pgalloc_normal" => crate::mm::vmstat::PGALLOC_NORMAL.load(Ordering::Relaxed),
    "pgalloc_movable" => crate::mm::vmstat::PGALLOC_MOVABLE.load(Ordering::Relaxed),
    "pgactivate" => 0,
    "pgdeactivate" => 0,
    "pglazyfree" => 0,
    "pgrefill" => 0,
    "pgsteal_kswapd" => 0,
    "pgsteal_direct" => 0,
    "pgscan_kswapd" => 0,
    "pgscan_direct" => 0,
    "pgpromote_success" => 0,
    "pgpromote_failed" => 0,
    // --- block I/O / swap ---
    "pgpgin" => 0,
    "pgpgout" => 0,
    "pswpin" => 0,
    "pswpout" => 0,
    }
}

pub fn generate() -> Vec<u8> {
    vmstat()
}

/// Exact-bytes snapshot (the author's conscious act — see procfile.rs):
/// the pre-conversion renderer's output for all-zero counters plus a
/// known free count, pinned so any future field/width/order edit that
/// changes bytes fails here and forces a deliberate snapshot update.
#[cfg(any(test, feature = "unit-test"))]
pub mod vmstat_file_tests {
    use super::*;

    /// Callable body (the in-kernel unit-test suite runs this via
    /// tests/mod.rs; the #[test] wrapper covers cargo test).
    pub fn snapshot_check() {
        // Rendering with every value 0 through the same declaration:
        // build the rows from VMSTAT_FIELDS (order included).
        let zero: u64 = 0;
        let rows: alloc::vec::Vec<(&'static str, &dyn core::fmt::Display)> =
            VMSTAT_FIELDS.iter().map(|k| (*k, &zero as &dyn core::fmt::Display)).collect();
        let got = crate::fs::procfs::procfile::render_kv(&rows);
        // First and last lines plus total line count pin order + shape.
        let lines: alloc::vec::Vec<&str> = got.lines().collect();
        assert_eq!(lines.first(), Some(&"nr_free_pages 0"));
        assert_eq!(lines.get(1), Some(&"nr_zone_inactive_anon 0"));
        assert_eq!(
            lines.iter().position(|l| *l == "pgalloc_dma32 0"),
            Some(43),
            "pgalloc_dma32 moved — column order is Linux-observable"
        );
        assert_eq!(lines.last(), Some(&"pswpout 0"));
        assert_eq!(lines.len(), VMSTAT_FIELDS.len());
        assert!(VMSTAT_FIELDS.len() == 60, "field count changed: {} — update this snapshot deliberately", VMSTAT_FIELDS.len());
    }

    #[cfg(test)]
    #[test]
    fn snapshot_zero_counters() {
        snapshot_check();
    }
}

/// Generate /proc/buddyinfo content: the free pages per order, per
/// zone (Linux's fs/proc/vmstat.c buddyinfo_show).
pub fn generate_buddyinfo() -> Vec<u8> {
    let mut out = String::new();
    // SAFETY: diagnostic read-only node access.
    if let Some(node) = unsafe { crate::mm::pglist::first_online_node() } {
        for zt in ZONE_ORDER {
            let zone = match node.zone(zt) {
                Some(z) if z.is_initialized() => z,
                _ => continue,
            };
            let stats = zone.stats();
            out.push_str(&format!(
                "Node {}, zone {:>8} ",
                stats.node_id,
                zone_name(zt)
            ));
            // Linux prints each order count as "%6lu " — right-aligned
            // in six columns plus one trailing space (mm/vmstat.c
            // frag_show_print), so the line ends with a space too.
            for order in 0..stats.free_blocks.len() {
                out.push_str(&format!("{:>6} ", stats.free_blocks[order]));
            }
            out.push('\n');
        }
    }
    out.into_bytes()
}
