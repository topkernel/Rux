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
pub fn generate() -> Vec<u8> {
    use crate::mm::vmstat;

    let mut out = String::new();
    let free = nr_free_pages();

    // --- zone / page state -------------------------------------------
    out.push_str(&format!("nr_free_pages {}\n", free));
    out.push_str("nr_zone_inactive_anon 0\n");
    out.push_str("nr_zone_active_anon 0\n");
    out.push_str("nr_zone_inactive_file 0\n");
    out.push_str("nr_zone_active_file 0\n");
    out.push_str("nr_zone_unevictable 0\n");
    out.push_str("nr_zone_write_pending 0\n");
    out.push_str("nr_mlock 0\n");
    out.push_str("nr_zspages 0\n");
    out.push_str("nr_free_cma 0\n");
    out.push_str("nr_unaccepted 0\n");
    out.push_str("numa_hit 0\n");
    out.push_str("numa_miss 0\n");
    out.push_str("numa_foreign 0\n");
    out.push_str("numa_interleave 0\n");
    out.push_str("numa_local 0\n");
    out.push_str("numa_other 0\n");
    out.push_str("nr_inactive_anon 0\n");
    out.push_str("nr_active_anon 0\n");
    out.push_str("nr_inactive_file 0\n");
    out.push_str("nr_active_file 0\n");
    out.push_str("nr_unevictable 0\n");
    out.push_str("nr_slab_reclaimable 0\n");
    out.push_str("nr_slab_unreclaimable 0\n");
    out.push_str("nr_isolated_anon 0\n");
    out.push_str("nr_isolated_file 0\n");
    out.push_str("workingset_nodes 0\n");
    out.push_str("workingset_refault_anon 0\n");
    out.push_str("workingset_refault_file 0\n");
    out.push_str("workingset_activate_anon 0\n");
    out.push_str("workingset_activate_file 0\n");
    out.push_str("workingset_restore_anon 0\n");
    out.push_str("workingset_restore_file 0\n");
    out.push_str("workingset_nodereclaim 0\n");
    out.push_str("nr_anon_pages 0\n");
    out.push_str("nr_mapped 0\n");
    out.push_str("nr_file_pages 0\n");
    out.push_str("nr_dirty 0\n");
    out.push_str("nr_writeback 0\n");

    // --- page allocator ----------------------------------------------
    out.push_str(&format!(
        "pgfault {}\n",
        vmstat::PGFAULT.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "pgmajfault {}\n",
        vmstat::PGMAJFAULT.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "pgfree {}\n",
        vmstat::PGFREE.load(Ordering::Relaxed)
    ));
    // Linux's pgalloc_* order follows the zone-type enum
    // (mm/vmstat.c VMSTAT_ITEMS): DMA, DMA32, Normal, Movable.
    out.push_str(&format!(
        "pgalloc_dma {}\n",
        vmstat::PGALLOC_DMA.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "pgalloc_dma32 {}\n",
        vmstat::PGALLOC_DMA32.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "pgalloc_normal {}\n",
        vmstat::PGALLOC_NORMAL.load(Ordering::Relaxed)
    ));
    out.push_str(&format!(
        "pgalloc_movable {}\n",
        vmstat::PGALLOC_MOVABLE.load(Ordering::Relaxed)
    ));
    out.push_str("pgactivate 0\n");
    out.push_str("pgdeactivate 0\n");
    out.push_str("pglazyfree 0\n");
    out.push_str("pgrefill 0\n");
    out.push_str("pgsteal_kswapd 0\n");
    out.push_str("pgsteal_direct 0\n");
    out.push_str("pgscan_kswapd 0\n");
    out.push_str("pgscan_direct 0\n");
    out.push_str("pgpromote_success 0\n");
    out.push_str("pgpromote_failed 0\n");

    // --- block I/O / swap ---------------------------------------------
    out.push_str("pgpgin 0\n");
    out.push_str("pgpgout 0\n");
    out.push_str("pswpin 0\n");
    out.push_str("pswpout 0\n");

    out.into_bytes()
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
