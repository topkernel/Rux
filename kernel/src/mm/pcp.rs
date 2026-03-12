//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Per-CPU Pages (PCP) - Per-CPU Page Cache
//!
//! Reduces lock contention on global page allocator, improving multi-core performance.
//!
//! # Design
//! - Each CPU maintains its own page cache
//! - Allocation prioritizes local cache (lock-free)
//! - When local cache is empty, batch acquire from global allocator
//! - When local cache is full, batch release to global allocator
//!
//! # Migration Types (MigrateType)
//! - Unmovable: Cannot be moved (pages used by kernel)
//! - Movable: Can be moved (userspace pages, can be migrated)
//! - Reclaimable: Can be reclaimed (can be swapped out)

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::config::MAX_CPUS;
use super::page::{PhysFrame, PAGE_SIZE, alloc_frame, dealloc_frame};
use super::page_desc::{pfn_to_page_mut, PageFlag};

/// Number of migration types
pub const MIGRATE_TYPES: usize = 3;

/// Maximum page list length for each migration type - from config
pub const PCP_HIGH: usize = crate::config::PCP_HIGH;
pub const PCP_LOW: usize = crate::config::PCP_LOW;
pub const PCP_BATCH: usize = crate::config::PCP_BATCH;

/// Migration type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum MigrateType {
    /// Unmovable
    Unmovable = 0,
    /// Movable
    Movable = 1,
    /// Reclaimable
    Reclaimable = 2,
}

/// Per-CPU page cache
///
/// Local page cache maintained by each CPU
#[repr(C)]
pub struct PerCpuPages {
    /// Page lists for each migration type
    /// Stores physical page numbers (PPN), 0 means empty
    lists: [usize; MIGRATE_TYPES],
    /// Page count for each migration type
    counts: [usize; MIGRATE_TYPES],
    /// High watermark (return pages when exceeded)
    high: usize,
    /// Batch operation count
    batch: usize,
    /// Initialization flag
    initialized: bool,
}

impl PerCpuPages {
    /// Create uninitialized PerCpuPages
    pub const fn new() -> Self {
        Self {
            lists: [0; MIGRATE_TYPES],
            counts: [0; MIGRATE_TYPES],
            high: PCP_HIGH,
            batch: PCP_BATCH,
            initialized: false,
        }
    }

    /// Initialize PerCpuPages
    pub fn init(&mut self) {
        self.lists = [0; MIGRATE_TYPES];
        self.counts = [0; MIGRATE_TYPES];
        self.high = PCP_HIGH;
        self.batch = PCP_BATCH;
        self.initialized = true;
    }

    /// Allocate a page from specified migration type
    pub fn alloc(&mut self, migratetype: MigrateType) -> Option<PhysFrame> {
        let mt = migratetype as usize;

        // Check if local cache has pages
        if self.counts[mt] > 0 {
            // Take a page from list head
            let pfn = self.lists[mt];
            if pfn == 0 {
                return None;
            }

            // Get next page
            let next = self.get_next_free(pfn);
            self.lists[mt] = next;
            self.counts[mt] -= 1;

            // Clear page's free list pointer
            self.clear_next_free(pfn);

            return Some(PhysFrame::new(pfn));
        }

        // Local cache is empty, batch acquire from global allocator
        self.refill(migratetype)?;

        // Try allocation again
        self.alloc(migratetype)
    }

    /// Free a page to local cache
    pub fn free(&mut self, frame: PhysFrame, migratetype: MigrateType) {
        let mt = migratetype as usize;
        let pfn = frame.number;

        // Add page to list head
        self.set_next_free(pfn, self.lists[mt]);
        self.lists[mt] = pfn;
        self.counts[mt] += 1;

        // Check if high watermark is exceeded
        if self.counts[mt] >= self.high {
            // Batch release to global allocator
            self.drain(migratetype);
        }
    }

    /// Batch acquire pages from global allocator
    fn refill(&mut self, migratetype: MigrateType) -> Option<()> {
        let mt = migratetype as usize;
        let batch = self.batch;

        for _ in 0..batch {
            match alloc_frame() {
                Some(frame) => {
                    let pfn = frame.number;
                    // Add to list head
                    self.set_next_free(pfn, self.lists[mt]);
                    self.lists[mt] = pfn;
                    self.counts[mt] += 1;
                }
                None => break,  // Global allocator has no available pages
            }
        }

        if self.counts[mt] > 0 {
            Some(())
        } else {
            None
        }
    }

    /// Batch release pages to global allocator
    fn drain(&mut self, migratetype: MigrateType) {
        let mt = migratetype as usize;
        let batch = self.batch;

        // Keep low watermark pages
        while self.counts[mt] > PCP_LOW && self.counts[mt] > batch {
            // Take batch pages from list head
            for _ in 0..batch {
                let pfn = self.lists[mt];
                if pfn == 0 {
                    break;
                }

                let next = self.get_next_free(pfn);
                self.lists[mt] = next;
                self.counts[mt] -= 1;

                // Clear free list pointer
                self.clear_next_free(pfn);

                // Return to global allocator
                dealloc_frame(PhysFrame::new(pfn));
            }
        }
    }

    /// Get page's next free page pointer
    fn get_next_free(&self, pfn: usize) -> usize {
        let page = super::page_desc::pfn_to_page(pfn);
        if page.is_null() {
            return 0;
        }
        unsafe { (*page).next_free() }
    }

    /// Set page's next free page pointer
    fn set_next_free(&self, pfn: usize, next: usize) {
        let page = super::page_desc::pfn_to_page_mut(pfn);
        if page.is_null() {
            return;
        }
        unsafe {
            (*page).set_next_free(next);
        }
    }

    /// Clear page's free list pointer
    fn clear_next_free(&self, pfn: usize) {
        let page = super::page_desc::pfn_to_page_mut(pfn);
        if page.is_null() {
            return;
        }
        unsafe {
            (*page).set_next_free(0);
        }
    }

    /// Get page count statistics
    pub fn count(&self, migratetype: MigrateType) -> usize {
        self.counts[migratetype as usize]
    }

    /// Get total page count
    pub fn total_count(&self) -> usize {
        self.counts.iter().sum()
    }
}

/// Global Per-CPU Pages array
///
/// Uses static array to store each CPU's page cache
static mut PER_CPU_PAGES: [PerCpuPages; MAX_CPUS] = [
    PerCpuPages::new(),
    PerCpuPages::new(),
    PerCpuPages::new(),
    PerCpuPages::new(),
];

/// Initialize Per-CPU Pages
///
/// Called when each CPU starts
pub fn init_percpu_pages(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    unsafe {
        PER_CPU_PAGES[cpu_id].init();
    }
}

/// Get current CPU's Per-CPU Pages
///
/// # Safety
/// Caller must ensure cpu_id is valid
fn this_cpu_pcp() -> Option<&'static mut PerCpuPages> {
    let cpu_id = crate::arch::cpu_id() as usize;
    if cpu_id >= MAX_CPUS {
        return None;
    }

    unsafe {
        if !PER_CPU_PAGES[cpu_id].initialized {
            return None;
        }
        Some(&mut PER_CPU_PAGES[cpu_id])
    }
}

/// Allocate a page from Per-CPU cache
///
/// Prioritize allocation from local CPU cache (lock-free)
/// Fall back to global allocator on failure
pub fn alloc_page_pcp(migratetype: MigrateType) -> Option<PhysFrame> {
    // Try to allocate from Per-CPU cache
    if let Some(pcp) = this_cpu_pcp() {
        if let Some(frame) = pcp.alloc(migratetype) {
            return Some(frame);
        }
    }

    // Fall back to global allocator
    alloc_frame()
}

/// Free a page to Per-CPU cache
///
/// Prioritize freeing to local CPU cache (lock-free)
/// Fall back to global allocator on failure
pub fn free_page_pcp(frame: PhysFrame, migratetype: MigrateType) {
    // Try to free to Per-CPU cache
    if let Some(pcp) = this_cpu_pcp() {
        pcp.free(frame, migratetype);
        return;
    }

    // Fall back to global allocator
    dealloc_frame(frame);
}

/// Get Per-CPU cache statistics
pub fn pcp_stats() -> PcpStats {
    let mut stats = PcpStats::default();

    unsafe {
        for cpu_id in 0..MAX_CPUS {
            if PER_CPU_PAGES[cpu_id].initialized {
                stats.cpu_stats[cpu_id].initialized = true;
                for mt in 0..MIGRATE_TYPES {
                    stats.cpu_stats[cpu_id].counts[mt] = PER_CPU_PAGES[cpu_id].counts[mt];
                }
            }
        }
    }

    stats
}

/// Single CPU's Per-CPU cache statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuPcpStats {
    pub initialized: bool,
    pub counts: [usize; MIGRATE_TYPES],
}

/// Global Per-CPU cache statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct PcpStats {
    pub cpu_stats: [CpuPcpStats; MAX_CPUS],
}

/// Determine migration type from allocation flags
pub fn gfp_to_migratetype(gfp_flags: u32) -> MigrateType {
    // Simplified implementation: default to Movable
    // Full implementation should determine based on GFP flags
    if gfp_flags & GFP_KERNEL != 0 {
        MigrateType::Unmovable
    } else {
        MigrateType::Movable
    }
}

/// GFP flags (Get Free Pages)
pub const GFP_KERNEL: u32 = 0x01;      // Kernel allocation (unmovable)
pub const GFP_USER: u32 = 0x02;        // User allocation (movable)
pub const GFP_ATOMIC: u32 = 0x04;      // Atomic allocation (cannot sleep)
pub const GFP_HIGHUSER: u32 = 0x08;    // High user memory
pub const GFP_DMA: u32 = 0x10;         // DMA memory
pub const GFP_NOWAIT: u32 = 0x20;      // No waiting

/// Convenience function: allocate kernel page
pub fn alloc_kernel_page() -> Option<PhysFrame> {
    alloc_page_pcp(MigrateType::Unmovable)
}

/// Convenience function: allocate user page
pub fn alloc_user_page() -> Option<PhysFrame> {
    alloc_page_pcp(MigrateType::Movable)
}

/// Convenience function: free kernel page
pub fn free_kernel_page(frame: PhysFrame) {
    free_page_pcp(frame, MigrateType::Unmovable);
}

/// Convenience function: free user page
pub fn free_user_page(frame: PhysFrame) {
    free_page_pcp(frame, MigrateType::Movable);
}
