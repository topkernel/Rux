//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Memory Statistics
//!
//! Provides memory statistics functionality similar to /proc/meminfo to track system memory usage.
//!
//! # Statistics Content
//! - Physical memory usage (Frame Allocator)
//! - Heap memory usage (Buddy Allocator)
//! - Slab allocator usage
//! - Per-CPU Pages cache
//! - Page descriptor status

use super::pglist::first_online_node;
use super::buddy_allocator::buddy_stats;
use super::slab::slab_stats;
use super::pcp::pcp_stats;
use super::page_desc::page_desc_stats;
use super::PAGE_SIZE;
use crate::config::PHYS_MEMORY_SIZE;

/// Memory statistics info (similar to /proc/meminfo)
#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
    // ========== Physical Memory ==========
    /// Total physical memory (bytes)
    pub mem_total: usize,
    /// Free physical memory (bytes)
    pub mem_free: usize,
    /// Available physical memory (bytes) = mem_free + reclaimable memory
    pub mem_available: usize,
    /// Used physical memory (bytes)
    pub mem_used: usize,

    // ========== Heap Memory (Buddy Allocator) ==========
    /// Heap total size (bytes)
    pub heap_total: usize,
    /// Heap used (bytes)
    pub heap_used: usize,
    /// Heap free (bytes)
    pub heap_free: usize,

    // ========== Slab Allocator ==========
    /// Slab page count
    pub slab_pages: usize,
    /// Slab allocation count
    pub slab_allocs: usize,
    /// Slab free count
    pub slab_frees: usize,

    // ========== Per-CPU Pages ==========
    /// PCP page count for each CPU
    pub pcp_pages: [usize; 4],

    // ========== Page Descriptor Statistics ==========
    /// Free page count
    pub pages_free: usize,
    /// In-use page count
    pub pages_used: usize,
    /// Reserved page count
    pub pages_reserved: usize,
    /// Mapped page count
    pub pages_mapped: usize,
    /// Dirty page count
    pub pages_dirty: usize,
    /// COW page count
    pub pages_cow: usize,
    /// Anonymous page count
    pub pages_anon: usize,
}

impl Default for MemoryInfo {
    fn default() -> Self {
        Self {
            mem_total: 0,
            mem_free: 0,
            mem_available: 0,
            mem_used: 0,
            heap_total: 0,
            heap_used: 0,
            heap_free: 0,
            slab_pages: 0,
            slab_allocs: 0,
            slab_frees: 0,
            pcp_pages: [0; 4],
            pages_free: 0,
            pages_used: 0,
            pages_reserved: 0,
            pages_mapped: 0,
            pages_dirty: 0,
            pages_cow: 0,
            pages_anon: 0,
        }
    }
}

impl MemoryInfo {
    /// Format as human-readable string
    pub fn format(&self) -> MemoryInfoFormatter {
        MemoryInfoFormatter { info: self }
    }
}

/// Memory info formatter
pub struct MemoryInfoFormatter<'a> {
    info: &'a MemoryInfo,
}

impl<'a> core::fmt::Display for MemoryInfoFormatter<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "Memory Info:")?;
        writeln!(f, "  MemTotal:       {:>10} kB ({} MB)", self.info.mem_total / 1024, self.info.mem_total / 1024 / 1024)?;
        writeln!(f, "  MemFree:        {:>10} kB ({} MB)", self.info.mem_free / 1024, self.info.mem_free / 1024 / 1024)?;
        writeln!(f, "  MemAvailable:   {:>10} kB ({} MB)", self.info.mem_available / 1024, self.info.mem_available / 1024 / 1024)?;
        writeln!(f, "  MemUsed:        {:>10} kB ({} MB)", self.info.mem_used / 1024, self.info.mem_used / 1024 / 1024)?;
        writeln!(f)?;
        writeln!(f, "  HeapTotal:      {:>10} kB ({} MB)", self.info.heap_total / 1024, self.info.heap_total / 1024 / 1024)?;
        writeln!(f, "  HeapUsed:       {:>10} kB ({} MB)", self.info.heap_used / 1024, self.info.heap_used / 1024 / 1024)?;
        writeln!(f, "  HeapFree:       {:>10} kB ({} MB)", self.info.heap_free / 1024, self.info.heap_free / 1024 / 1024)?;
        writeln!(f)?;
        writeln!(f, "  SlabPages:      {:>10} pages", self.info.slab_pages)?;
        writeln!(f, "  SlabAllocs:     {:>10}", self.info.slab_allocs)?;
        writeln!(f, "  SlabFrees:      {:>10}", self.info.slab_frees)?;
        writeln!(f)?;
        writeln!(f, "  PCP Pages:      CPU0={} CPU1={} CPU2={} CPU3={}",
            self.info.pcp_pages[0], self.info.pcp_pages[1],
            self.info.pcp_pages[2], self.info.pcp_pages[3])?;
        writeln!(f)?;
        writeln!(f, "  PagesFree:      {:>10}", self.info.pages_free)?;
        writeln!(f, "  PagesUsed:      {:>10}", self.info.pages_used)?;
        writeln!(f, "  PagesReserved:  {:>10}", self.info.pages_reserved)?;
        writeln!(f, "  PagesMapped:    {:>10}", self.info.pages_mapped)?;
        writeln!(f, "  PagesDirty:     {:>10}", self.info.pages_dirty)?;
        writeln!(f, "  PagesCOW:       {:>10}", self.info.pages_cow)?;
        writeln!(f, "  PagesAnon:      {:>10}", self.info.pages_anon)
    }
}

/// Get complete memory statistics
pub fn get_memory_info() -> MemoryInfo {
    let mut info = MemoryInfo::default();

    // Physical memory statistics from zone allocator
    if let Some(node) = first_online_node() {
        let free_pages = node.free_pages();
        info.mem_free = free_pages * PAGE_SIZE;
        info.mem_available = free_pages * PAGE_SIZE; // Simplified: equals free memory

        // Total physical memory from config
        info.mem_total = PHYS_MEMORY_SIZE;
        info.mem_used = info.mem_total.saturating_sub(info.mem_free);
    } else {
        // Fallback: use config value
        info.mem_total = PHYS_MEMORY_SIZE;
        info.mem_free = 0;
        info.mem_used = PHYS_MEMORY_SIZE;
        info.mem_available = 0;
    }

    // Heap memory statistics
    let buddy_stats = buddy_stats();
    info.heap_total = buddy_stats.heap_size;
    info.heap_used = buddy_stats.used_bytes;
    info.heap_free = buddy_stats.free_bytes;

    // Slab statistics
    let slab_stats = slab_stats();
    info.slab_pages = slab_stats.total_pages;
    info.slab_allocs = slab_stats.cache_stats.iter().map(|c| c.alloc_count).sum();
    info.slab_frees = slab_stats.cache_stats.iter().map(|c| c.free_count).sum();

    // Per-CPU Pages statistics
    let pcp_stats = pcp_stats();
    for (i, cpu_stat) in pcp_stats.cpu_stats.iter().enumerate() {
        if i < 4 && cpu_stat.initialized {
            info.pcp_pages[i] = cpu_stat.counts.iter().sum();
        }
    }

    // Page descriptor statistics
    let page_stats = page_desc_stats();
    info.pages_free = page_stats.free_pages;
    info.pages_used = page_stats.used_pages;
    info.pages_reserved = page_stats.reserved_pages;
    info.pages_mapped = page_stats.mapped_pages;
    info.pages_dirty = page_stats.dirty_pages;
    info.pages_cow = page_stats.cow_pages;
    info.pages_anon = page_stats.anonymous_pages;

    info
}

/// Print memory statistics
pub fn print_memory_info() {
    let info = get_memory_info();
    crate::println!("{}", info.format());
}

/// Memory usage summary (for quick checking)
#[derive(Debug, Clone, Copy, Default)]
pub struct MemorySummary {
    /// Total physical memory (MB)
    pub total_mb: usize,
    /// Used physical memory (MB)
    pub used_mb: usize,
    /// Free physical memory (MB)
    pub free_mb: usize,
    /// Heap usage percentage
    pub heap_usage_percent: usize,
}

/// Get memory usage summary
pub fn get_memory_summary() -> MemorySummary {
    let info = get_memory_info();

    let total_mb = info.mem_total / 1024 / 1024;
    let used_mb = info.mem_used / 1024 / 1024;
    let free_mb = info.mem_free / 1024 / 1024;

    let heap_usage_percent = if info.heap_total > 0 {
        info.heap_used * 100 / info.heap_total
    } else {
        0
    };

    MemorySummary {
        total_mb,
        used_mb,
        free_mb,
        heap_usage_percent,
    }
}

/// Check if memory is low (for OOM warning)
pub fn is_memory_low() -> bool {
    let info = get_memory_info();

    // If free memory is less than 5% of total, consider memory low
    if info.mem_total > 0 {
        info.mem_free * 100 / info.mem_total < 5
    } else {
        false
    }
}

/// Check if OOM should be triggered
pub fn should_trigger_oom() -> bool {
    let info = get_memory_info();

    // If free memory is less than 1% of total, trigger OOM
    if info.mem_total > 0 {
        info.mem_free * 100 / info.mem_total < 1
    } else {
        false
    }
}
