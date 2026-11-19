//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Page List Data (pglist_data) - NUMA Node Management
//!
//! This module implements the pglist_data structure which represents
//! a NUMA node's memory. On UMA systems, there is a single node.

extern crate alloc;

use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;

use super::zone::{Zone, ZoneType, ZoneStats, MAX_ORDER};
use super::PAGE_SIZE;

// ==================== Pglist Data (NUMA Node) ====================

/// Maximum number of zones per node
pub const MAX_NR_ZONES: usize = ZoneType::ZoneCount as usize;

/// Maximum number of NUMA nodes
pub const MAX_NUMNODES: usize = 1;  // UMA system, single node

// ==================== LRU Constants ====================

/// LRU list type indices (following mm/vmscan.c enum lru_list).
pub const LRU_INACTIVE_ANON: usize = 0;
pub const LRU_ACTIVE_ANON: usize = 1;
pub const LRU_INACTIVE_FILE: usize = 2;
pub const LRU_ACTIVE_FILE: usize = 3;
pub const LRU_UNEVICTABLE: usize = 4;
/// Number of LRU lists.
pub const NR_LRU_LISTS: usize = 5;

/// DEF_PRIORITY — starting priority for kswapd reclaim loop.
pub const DEF_PRIORITY: i32 = 12;

/// Page list data structure (represents a NUMA node)
///
/// On UMA systems (like our current RISC-V QEMU), there is only one node
/// containing all zones. On NUMA systems, each node has its own memory
/// and possibly its own zones.
pub struct PglistData {
    /// Node ID
    node_id: usize,

    /// Zones in this node
    zones: [Option<Zone>; MAX_NR_ZONES],

    /// Number of zones in this node
    nr_zones: AtomicUsize,

    /// Node start PFN
    node_start_pfn: AtomicUsize,

    /// Node spanned pages (total pages including holes)
    node_spanned_pages: AtomicUsize,

    /// Node present pages (excluding holes)
    node_present_pages: AtomicUsize,

    /// Total reserved pages
    total_reserved_pages: AtomicUsize,

    /// Initialized flag
    initialized: AtomicBool,

    // ---- LRU list infrastructure ----

    /// LRU list heads: PFN of the first page (most-recently used end).
    /// 0 means the list is empty.  Protected by `lru_lock`.
    pub(crate) lru_heads: [AtomicUsize; NR_LRU_LISTS],

    /// LRU list tails: PFN of the last page (least-recently used end,
    /// where kswapd scans from).  0 means the list is empty.
    pub(crate) lru_tails: [AtomicUsize; NR_LRU_LISTS],

    /// Number of pages on each LRU list.
    pub(crate) lru_sizes: [AtomicUsize; NR_LRU_LISTS],

    /// Spinlock protecting all LRU list mutations.
    pub(crate) lru_lock: Spinlock<()>,
}

impl PglistData {
    /// Create a new uninitialized pglist_data
    pub const fn new(node_id: usize) -> Self {
        Self {
            node_id,
            zones: [None, None, None, None],
            nr_zones: AtomicUsize::new(0),
            node_start_pfn: AtomicUsize::new(0),
            node_spanned_pages: AtomicUsize::new(0),
            node_present_pages: AtomicUsize::new(0),
            total_reserved_pages: AtomicUsize::new(0),
            initialized: AtomicBool::new(false),
            lru_heads: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_LRU_LISTS]
            },
            lru_tails: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_LRU_LISTS]
            },
            lru_sizes: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_LRU_LISTS]
            },
            lru_lock: Spinlock::new(()),
        }
    }

    /// Initialize the node with memory regions
    ///
    /// # Arguments
    /// - `start_pfn`: Start page frame number
    /// - `spanned_pages`: Total pages including holes
    /// - `present_pages`: Present pages (excluding holes)
    pub fn init(&mut self, start_pfn: usize, spanned_pages: usize, present_pages: usize) {
        self.node_start_pfn.store(start_pfn, Ordering::Release);
        self.node_spanned_pages.store(spanned_pages, Ordering::Release);
        self.node_present_pages.store(present_pages, Ordering::Release);
        self.initialized.store(true, Ordering::Release);
    }

    /// Add a zone to this node
    pub fn add_zone(&mut self, zone_type: ZoneType, zone: Zone) {
        let idx = zone_type as usize;
        if idx < MAX_NR_ZONES {
            self.zones[idx] = Some(zone);
            self.nr_zones.fetch_add(1, Ordering::Release);
        }
    }

    /// Get zone by type
    pub fn zone(&self, zone_type: ZoneType) -> Option<&Zone> {
        self.zones[zone_type as usize].as_ref()
    }

    /// Get mutable zone by type
    pub fn zone_mut(&mut self, zone_type: ZoneType) -> Option<&mut Zone> {
        self.zones[zone_type as usize].as_mut()
    }

    /// Get zone by index
    pub fn zone_at(&self, idx: usize) -> Option<&Zone> {
        if idx < MAX_NR_ZONES {
            self.zones[idx].as_ref()
        } else {
            None
        }
    }

    /// Get node ID
    pub fn node_id(&self) -> usize {
        self.node_id
    }

    /// Get number of zones
    pub fn nr_zones(&self) -> usize {
        self.nr_zones.load(Ordering::Acquire)
    }

    /// Get node start PFN
    pub fn start_pfn(&self) -> usize {
        self.node_start_pfn.load(Ordering::Acquire)
    }

    /// Get node spanned pages
    pub fn spanned_pages(&self) -> usize {
        self.node_spanned_pages.load(Ordering::Acquire)
    }

    /// Get node present pages
    pub fn present_pages(&self) -> usize {
        self.node_present_pages.load(Ordering::Acquire)
    }

    /// Get total free pages across all zones
    pub fn free_pages(&self) -> usize {
        let mut total = 0;
        for zone_opt in &self.zones {
            if let Some(zone) = zone_opt {
                if zone.is_initialized() {
                    total += zone.nr_free();
                }
            }
        }
        total
    }

    /// Check if node is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Get node statistics
    pub fn stats(&self) -> NodeStats {
        let mut zone_stats = Vec::new();
        for zone_opt in &self.zones {
            if let Some(zone) = zone_opt {
                if zone.is_initialized() {
                    zone_stats.push(zone.stats());
                }
            }
        }

        NodeStats {
            node_id: self.node_id,
            start_pfn: self.start_pfn(),
            spanned_pages: self.spanned_pages(),
            present_pages: self.present_pages(),
            total_free_pages: self.free_pages(),
            zones: zone_stats,
        }
    }
}

/// Node statistics
#[derive(Debug, Clone)]
pub struct NodeStats {
    pub node_id: usize,
    pub start_pfn: usize,
    pub spanned_pages: usize,
    pub present_pages: usize,
    pub total_free_pages: usize,
    pub zones: Vec<ZoneStats>,
}

// ==================== Global Node Management ====================

/// Global node array (static storage)
/// For UMA systems, we only have node 0
static mut NODE_DATA: [Option<PglistData>; MAX_NUMNODES] = [None];

/// Node count
static NODE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Initialize node data
///
/// # Safety
/// Must be called only during early boot, before other CPUs are online
pub unsafe fn init_node_data() {
    // SAFETY: called only during early boot (single-threaded), before any other
    // access to NODE_DATA.
    NODE_DATA[0] = Some(PglistData::new(0));
    NODE_COUNT.store(1, Ordering::Release);
}

/// Get node by ID
///
/// # Safety
/// Returns reference to static data
pub fn node_data(node_id: usize) -> Option<&'static PglistData> {
    if node_id >= MAX_NUMNODES {
        return None;
    }
    // SAFETY: node_id is bounds-checked above; NODE_DATA is initialized by
    // init_node_data() before any call here.
    unsafe { NODE_DATA[node_id].as_ref() }
}

/// Get mutable node by ID
///
/// # Safety
/// Must be called during initialization only
pub fn node_data_mut(node_id: usize) -> Option<&'static mut PglistData> {
    if node_id >= MAX_NUMNODES {
        return None;
    }
    // SAFETY: node_id is bounds-checked above; caller must ensure exclusive
    // access (called only during init or with appropriate locking).
    unsafe { NODE_DATA[node_id].as_mut() }
}

/// Get first (and only, on UMA) node
pub fn first_online_node() -> Option<&'static PglistData> {
    node_data(0)
}

/// Get first mutable node.
///
/// # Safety
/// Caller must ensure exclusive access to the node.  On a single-node UMA
/// system this means no other thread may hold a mutable or shared reference
/// to the same `PglistData` simultaneously.  Typically this is safe only
/// during early boot or when the caller holds a lock that prevents
/// concurrent page allocation / reclaim on the same node.
pub unsafe fn first_online_node_mut() -> Option<&'static mut PglistData> {
    node_data_mut(0)
}

/// Get node count
pub fn num_online_nodes() -> usize {
    NODE_COUNT.load(Ordering::Acquire)
}

// ==================== Zone Selection ====================

/// Select zone for allocation based on GFP flags
///
/// # Arguments
/// - `gfp_flags`: GFP flags for the allocation
/// - `node`: Node to allocate from
///
/// # Returns
/// - The appropriate zone for this allocation
pub fn select_zone(gfp_flags: super::zone::GfpFlags, node: &PglistData) -> Option<&Zone> {
    let zone_type = gfp_flags.zone_type();
    node.zone(zone_type)
}

/// Select mutable zone for allocation
pub fn select_zone_mut(gfp_flags: super::zone::GfpFlags, node: &mut PglistData) -> Option<&mut Zone> {
    let zone_type = gfp_flags.zone_type();
    node.zone_mut(zone_type)
}

// ==================== Buddyinfo ====================

/// Print buddyinfo
pub fn print_buddyinfo() {
    for node_id in 0..num_online_nodes() {
        if let Some(node) = node_data(node_id) {
            crate::println!("Node {}, zone{}", node_id, "");

            // Print header
            crate::print!("  Order: ");
            for order in 0..=MAX_ORDER {
                crate::print!("{:4} ", order);
            }
            crate::println!("");

            // Print free count per zone
            for zone_type in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
                if let Some(zone) = node.zone(zone_type) {
                    if zone.is_initialized() {
                        crate::print!("  {:6} ", zone_type.name());
                        for order in 0..=MAX_ORDER {
                            crate::print!("{:4} ", zone.free_pages_order(order));
                        }
                        crate::println!("");
                    }
                }
            }
        }
    }
}

/// Print memory layout
pub fn print_zoneinfo() {
    for node_id in 0..num_online_nodes() {
        if let Some(node) = node_data(node_id) {
            crate::println!("Node {}", node_id);
            crate::println!("  spanned pages: {}", node.spanned_pages());
            crate::println!("  present pages: {}", node.present_pages());

            for zone_type in [ZoneType::ZoneDma, ZoneType::ZoneDma32, ZoneType::ZoneNormal, ZoneType::ZoneMovable] {
                if let Some(zone) = node.zone(zone_type) {
                    if zone.is_initialized() {
                        crate::println!("");
                        crate::println!("  zone {}", zone_type.name());
                        crate::println!("    start_pfn:     {:#x}", zone.start_pfn());
                        crate::println!("    spanned:       {}", zone.spanned_pages());
                        crate::println!("    present:       {}", zone.present_pages());
                        crate::println!("    managed:       {}", zone.managed_pages());
                        crate::println!("    free:          {}", zone.nr_free());
                        crate::println!("    buddy pages:");
                        for order in 0..=MAX_ORDER {
                            crate::println!("      order {}: {} blocks",
                                order, zone.free_pages_order(order));
                        }
                    }
                }
            }
        }
    }
}
