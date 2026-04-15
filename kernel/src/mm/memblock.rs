//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memblock Early Memory Allocator
//!
//! This module implements a memblock-style early memory allocator for
//! memblock subsystem. It is used during early boot before the buddy allocator is
//! initialized.
//!
//! # Overview
//!
//! Memblock manages memory in terms of regions, with four types:
//! - memory: Available physical memory regions (from device tree)
//! - reserved: Regions that are already in use (kernel, initrd, dtb, etc.)
//! - nomap: Memory regions that should not be mapped (e.g., persistent memory)
//!
//! # Alignment
//!
//! This implementation follows standard memblock design:
//! 1. Memory regions are discovered from device tree (/memory nodes)
//! 2. Reserved regions are added for kernel, initrd, dtb, etc.
//! 3. Frame allocator is initialized from available memory (memory - reserved)

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use super::PAGE_SIZE;

/// Maximum number of memory regions
/// Need enough slots for all reserved regions plus individual page allocations
/// during early boot (device mappings, linear mapping page tables, vmemmap)
const MAX_MEMBLOCK_REGIONS: usize = 128;

/// Memory region flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemBlockFlags(u32);

impl MemBlockFlags {
    /// No special flags
    pub const NONE: MemBlockFlags = MemBlockFlags(0);
    /// Region should not be mapped (MEMBLOCK_NOMAP)
    pub const NOMAP: MemBlockFlags = MemBlockFlags(1 << 0);
    /// Region is mirror of another region
    pub const MIRROR: MemBlockFlags = MemBlockFlags(1 << 1);
}

/// A memory region descriptor
#[derive(Debug, Clone, Copy)]
pub struct MemBlockRegion {
    /// Physical start address
    pub base: usize,
    /// Region size in bytes
    pub size: usize,
    /// Region flags
    pub flags: MemBlockFlags,
    /// NUMA node ID (0 for UMA systems)
    pub nid: u32,
}

impl MemBlockRegion {
    pub const fn new(base: usize, size: usize) -> Self {
        Self {
            base,
            size,
            flags: MemBlockFlags::NONE,
            nid: 0,
        }
    }

    pub const fn with_flags(base: usize, size: usize, flags: MemBlockFlags) -> Self {
        Self {
            base,
            size,
            flags,
            nid: 0,
        }
    }

    /// Get end address (exclusive)
    #[inline]
    pub fn end(&self) -> usize {
        self.base + self.size
    }

    /// Check if region contains address
    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.end()
    }

    /// Get page frame number of base
    #[inline]
    pub fn base_pfn(&self) -> usize {
        self.base / PAGE_SIZE
    }

    /// Get page frame number of end (exclusive)
    #[inline]
    pub fn end_pfn(&self) -> usize {
        (self.base + self.size) / PAGE_SIZE
    }

    /// Get number of pages in region
    #[inline]
    pub fn page_count(&self) -> usize {
        self.size / PAGE_SIZE
    }
}

/// A collection of memory regions
#[derive(Debug)]
pub struct MemBlockType {
    /// Array of regions
    regions: [MemBlockRegion; MAX_MEMBLOCK_REGIONS],
    /// Number of valid regions
    cnt: usize,
    /// Total size of all regions
    total_size: usize,
}

impl MemBlockType {
    pub const fn new() -> Self {
        Self {
            regions: [MemBlockRegion::new(0, 0); MAX_MEMBLOCK_REGIONS],
            cnt: 0,
            total_size: 0,
        }
    }

    /// Add a region
    pub fn add(&mut self, base: usize, size: usize) -> Result<(), ()> {
        if self.cnt >= MAX_MEMBLOCK_REGIONS {
            return Err(());
        }

        // Align region to page boundaries using base+end (not base+size independently)
        let aligned_base = (base + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let aligned_end = (base + size) & !(PAGE_SIZE - 1);

        if aligned_end <= aligned_base {
            return Err(());
        }
        let base = aligned_base;
        let size = aligned_end - aligned_base;

        // Check for overlaps and merge if possible
        for i in 0..self.cnt {
            let region = &mut self.regions[i];

            // Check if new region can be merged with existing one
            if base == region.end() {
                // Extend existing region
                region.size += size;
                self.total_size += size;
                return Ok(());
            } else if base + size == region.base {
                // Prepend to existing region
                region.base = base;
                region.size += size;
                self.total_size += size;
                return Ok(());
            } else if base >= region.base && base < region.end() {
                // Overlapping region, extend if needed
                let new_end = base + size;
                if new_end > region.end() {
                    let extra = new_end - region.end();
                    region.size += extra;
                    self.total_size += extra;
                }
                return Ok(());
            }
        }

        // Add new region
        self.regions[self.cnt] = MemBlockRegion::new(base, size);
        self.cnt += 1;
        self.total_size += size;

        Ok(())
    }

    /// Add a reserved region (with merge support for adjacent regions)
    pub fn add_reserved(&mut self, base: usize, size: usize, flags: MemBlockFlags) -> Result<(), ()> {
        // Check for adjacent or overlapping regions and merge them
        let new_end = base + size;

        for i in 0..self.cnt {
            let region = &mut self.regions[i];
            let region_end = region.base + region.size;

            // Check if this region is adjacent or overlapping
            // Adjacent: new_start == region_end OR new_end == region.base
            // Overlapping: new_start < region_end AND new_end > region.base
            if base <= region_end && new_end >= region.base {
                // Merge: extend the existing region
                let merged_base = base.min(region.base);
                let merged_end = new_end.max(region_end);
                region.base = merged_base;
                region.size = merged_end - merged_base;
                // Note: total_size adjustment is approximate (may overcount overlaps)
                return Ok(());
            }
        }

        // No adjacent region found, add new one
        if self.cnt >= MAX_MEMBLOCK_REGIONS {
            return Err(());
        }

        self.regions[self.cnt] = MemBlockRegion::with_flags(base, size, flags);
        self.cnt += 1;
        self.total_size += size;

        Ok(())
    }

    /// Remove a region by index
    pub fn remove(&mut self, idx: usize) {
        if idx >= self.cnt {
            return;
        }

        self.total_size -= self.regions[idx].size;

        // Shift remaining regions
        for i in idx..self.cnt - 1 {
            self.regions[i] = self.regions[i + 1];
        }
        self.cnt -= 1;
    }

    /// Iterate over regions
    pub fn iter(&self) -> impl Iterator<Item = &MemBlockRegion> {
        self.regions[..self.cnt].iter()
    }

    /// Get region count
    #[inline]
    pub fn len(&self) -> usize {
        self.cnt
    }

    /// Check if empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cnt == 0
    }

    /// Get total size
    #[inline]
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Find region containing address
    pub fn find(&self, addr: usize) -> Option<&MemBlockRegion> {
        self.iter().find(|r| r.contains(addr))
    }

    /// Find first available region (for frame allocator)
    pub fn find_first_available(&self) -> Option<&MemBlockRegion> {
        self.iter().find(|r| r.flags == MemBlockFlags::NONE)
    }
}

/// Global memblock state
pub struct MemBlock {
    /// Available memory regions
    memory: MemBlockType,
    /// Reserved memory regions
    reserved: MemBlockType,
    /// Whether memblock is initialized
    initialized: AtomicBool,
    /// Bottom of available memory (after kernel + reserved)
    bottom: usize,
    /// Top of available memory
    top: usize,
    /// Current allocation pointer
    current: usize,
}

impl MemBlock {
    pub const fn new() -> Self {
        Self {
            memory: MemBlockType::new(),
            reserved: MemBlockType::new(),
            initialized: AtomicBool::new(false),
            bottom: 0,
            top: 0,
            current: 0,
        }
    }

    /// Initialize memblock (can only be called once)
    pub fn init(&mut self) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }
        self.initialized.store(true, Ordering::Release);
    }

    /// Add memory region (from device tree)
    pub fn add_memory(&mut self, base: usize, size: usize) -> Result<(), ()> {
        self.memory.add(base, size)?;

        // Update top
        if base + size > self.top {
            self.top = base + size;
        }

        Ok(())
    }

    /// Reserve a memory region
    pub fn reserve(&mut self, base: usize, size: usize) -> Result<(), ()> {
        self.reserved.add_reserved(base, size, MemBlockFlags::NONE)?;

        // Update bottom if this is the highest reserved region
        if base + size > self.bottom {
            self.bottom = base + size;
        }

        Ok(())
    }

    /// Reserve a memory region with NOMAP flag
    pub fn reserve_nomap(&mut self, base: usize, size: usize) -> Result<(), ()> {
        self.reserved.add_reserved(base, size, MemBlockFlags::NOMAP)
    }

    /// Get available memory regions (memory - reserved)
    /// Returns the first region that can be used for frame allocation
    pub fn get_available_region(&self) -> Option<MemBlockRegion> {
        // Find the highest reserved region end
        let mut highest_reserved_end = 0usize;
        for res_region in self.reserved.iter() {
            if res_region.end() > highest_reserved_end {
                highest_reserved_end = res_region.end();
            }
        }

        // Find the first memory region that starts at or after the highest reserved end
        for mem_region in self.memory.iter() {
            // Check if this memory region contains addressable space after reserved regions
            if mem_region.end() > highest_reserved_end {
                let available_start = highest_reserved_end.max(mem_region.base);
                let available_end = mem_region.end();

                // Align to page boundary
                let aligned_start = (available_start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                let aligned_end = available_end & !(PAGE_SIZE - 1);

                if aligned_end > aligned_start {
                    return Some(MemBlockRegion::new(aligned_start, aligned_end - aligned_start));
                }
            }
        }

        None
    }

    /// Get total memory size
    #[inline]
    pub fn total_memory(&self) -> usize {
        self.memory.total_size()
    }

    /// Get total reserved size
    #[inline]
    pub fn total_reserved(&self) -> usize {
        self.reserved.total_size()
    }

    /// Get available memory size
    pub fn available_memory(&self) -> usize {
        self.memory.total_size().saturating_sub(self.reserved.total_size())
    }

    /// Get memory regions
    #[inline]
    pub fn memory(&self) -> &MemBlockType {
        &self.memory
    }

    /// Get reserved regions
    #[inline]
    pub fn reserved(&self) -> &MemBlockType {
        &self.reserved
    }

    /// Check if address is reserved
    pub fn is_reserved(&self, addr: usize) -> bool {
        self.reserved.find(addr).is_some()
    }

    /// Check if address is in memory
    pub fn is_memory(&self, addr: usize) -> bool {
        self.memory.find(addr).is_some()
    }

    /// Find a free region of given size (for early boot allocations)
    pub fn find_in_range(&self, size: usize, min_addr: usize, max_addr: usize) -> Option<usize> {
        // Align size to page boundary
        let size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for mem_region in self.memory.iter() {
            let region_start = mem_region.base.max(min_addr);
            let region_end = (mem_region.base + mem_region.size).min(max_addr);

            if region_start >= region_end || region_end - region_start < size {
                continue;
            }

            // Check if region is free (not reserved)
            let mut addr = region_start;
            while addr + size <= region_end {
                let mut is_free = true;
                for res_region in self.reserved.iter() {
                    if addr < res_region.end() && addr + size > res_region.base {
                        // Overlaps with reserved region, skip to after it
                        addr = res_region.end();
                        is_free = false;
                        break;
                    }
                }
                if is_free {
                    return Some(addr);
                }
            }
        }

        None
    }

    /// Dump memblock state (for debugging)
    pub fn dump(&self) {
        crate::println!("memblock: memory regions:");
        for (i, region) in self.memory.iter().enumerate() {
            crate::println!("  [{:2}] {:#018x} - {:#018x} ({:?})",
                i, region.base, region.end(), region.size / (1024 * 1024));
        }
        crate::println!("memblock: reserved regions:");
        for (i, region) in self.reserved.iter().enumerate() {
            crate::println!("  [{:2}] {:#018x} - {:#018x} ({:?})",
                i, region.base, region.end(), region.size / (1024 * 1024));
        }
        crate::println!("memblock: total: {:?} MB, reserved: {:?} MB, available: {:?} MB",
            self.total_memory() / (1024 * 1024),
            self.total_reserved() / (1024 * 1024),
            self.available_memory() / (1024 * 1024));
    }

    /// Iterate over free memory ranges (memory - reserved)
    pub fn for_each_free_range<F>(&self, min_addr: usize, max_addr: usize, mut f: F)
    where
        F: FnMut(usize, usize),
    {
        // Collect reserved regions for sorting
        let reserved: Vec<_> = self.reserved.iter().collect();
        let mut reserved_sorted: Vec<_> = reserved.into_iter().collect();
        reserved_sorted.sort_by_key(|r| r.base);

        for mem_region in self.memory.iter() {
            let region_start = mem_region.base.max(min_addr);
            let region_end = (mem_region.base + mem_region.size).min(max_addr);

            if region_start >= region_end {
                continue;
            }

            // Walk through this memory region, skipping reserved parts
            let mut current = region_start;
            for res in &reserved_sorted {
                if res.base >= region_end {
                    break;
                }
                if res.end() <= region_start {
                    continue;
                }

                // Free range before this reserved region
                if current < res.base {
                    let free_start = current;
                    let free_end = res.base.min(region_end);
                    if free_start < free_end {
                        f(free_start, free_end);
                    }
                }

                // Skip past the reserved region
                current = current.max(res.end());
            }

            // Free range after all reserved regions
            if current < region_end {
                f(current, region_end);
            }
        }
    }
}

// Global memblock instance.
//
// MEMBLOCK is accessed from early boot (single-threaded) through the memblock_init /
// memblock_add / memblock_reserve helpers, and later read-only via memblock().
// All mutation must happen before the scheduler starts; after that, only
// `memblock()` (shared ref) is safe to call.
static mut MEMBLOCK: MemBlock = MemBlock::new();

/// Init guard for MEMBLOCK — ensures init() runs exactly once.
static MEMBLOCK_INIT: AtomicBool = AtomicBool::new(false);

/// Initialize memblock
pub fn memblock_init() {
    // Use compare_exchange so that even if two early-boot callers race, only
    // one proceeds with initialization.
    if MEMBLOCK_INIT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        // SAFETY: called only during early boot (single-threaded), before any
        // concurrent access to MEMBLOCK.
        unsafe {
            MEMBLOCK.init();
        }
    }
}

/// Add memory region
pub fn memblock_add(base: usize, size: usize) -> Result<(), ()> {
    // SAFETY: called only during early boot before concurrent access.
    unsafe {
        MEMBLOCK.add_memory(base, size)
    }
}

/// Reserve memory region
pub fn memblock_reserve(base: usize, size: usize) -> Result<(), ()> {
    // SAFETY: called only during early boot before concurrent access.
    unsafe {
        MEMBLOCK.reserve(base, size)
    }
}

/// Reserve memory region with NOMAP
pub fn memblock_reserve_nomap(base: usize, size: usize) -> Result<(), ()> {
    // SAFETY: called only during early boot before concurrent access.
    unsafe {
        MEMBLOCK.reserve_nomap(base, size)
    }
}

/// Get available region for frame allocator
pub fn memblock_get_available_region() -> Option<MemBlockRegion> {
    // SAFETY: read-only access; MEMBLOCK is initialized before this is called.
    unsafe {
        MEMBLOCK.get_available_region()
    }
}

/// Get total memory size
pub fn memblock_total_memory() -> usize {
    // SAFETY: read-only access; MEMBLOCK is initialized before this is called.
    unsafe { MEMBLOCK.total_memory() }
}

/// Get available memory size
pub fn memblock_available_memory() -> usize {
    // SAFETY: read-only access; MEMBLOCK is initialized before this is called.
    unsafe { MEMBLOCK.available_memory() }
}

/// Check if address is reserved
pub fn memblock_is_reserved(addr: usize) -> bool {
    // SAFETY: read-only access; MEMBLOCK is initialized before this is called.
    unsafe { MEMBLOCK.is_reserved(addr) }
}

/// Iterate over free memory ranges (memory - reserved)
pub fn memblock_for_each_free_range<F>(min_addr: usize, max_addr: usize, f: F)
where
    F: FnMut(usize, usize),
{
    // SAFETY: read-only iteration over regions; called during boot or init.
    unsafe { MEMBLOCK.for_each_free_range(min_addr, max_addr, f) }
}

/// Find free region in range
pub fn memblock_find_in_range(size: usize, min_addr: usize, max_addr: usize) -> Option<usize> {
    // SAFETY: read-only query; MEMBLOCK is initialized before this is called.
    unsafe { MEMBLOCK.find_in_range(size, min_addr, max_addr) }
}

/// Dump memblock state
pub fn memblock_dump() {
    // SAFETY: read-only access for diagnostics; MEMBLOCK is initialized.
    unsafe { MEMBLOCK.dump() }
}

/// Get reference to memblock (for reading).
///
/// # Safety
/// Caller must ensure MEMBLOCK has been initialized (memblock_init called).
/// After the scheduler starts this is safe to call from any context because
/// MEMBLOCK is no longer mutated.
pub fn memblock() -> &'static MemBlock {
    // SAFETY: MEMBLOCK is initialized before any call to this function.
    // After early boot the data is effectively immutable.
    unsafe { &MEMBLOCK }
}

/// Get mutable reference to memblock (for initialization).
///
/// # Safety
/// Must only be called during early boot before the scheduler starts.
/// The caller must ensure no other thread can access MEMBLOCK concurrently.
pub fn memblock_mut() -> &'static mut MemBlock {
    // SAFETY: caller must ensure exclusive access (only during early boot,
    // before scheduler starts).
    unsafe { &mut MEMBLOCK }
}

/// Allocate a physical page from memblock.
/// Physical memory allocation from memblock.
/// Returns physical address of allocated page, or None if allocation fails.
pub fn memblock_phys_alloc() -> Option<usize> {
    // SAFETY: called only during early boot (single-threaded).
    unsafe {
        // Find a free page in available memory
        let phys = MEMBLOCK.find_in_range(PAGE_SIZE, 0, usize::MAX)?;
        // Reserve it
        MEMBLOCK.reserve(phys, PAGE_SIZE).ok()?;
        Some(phys)
    }
}
