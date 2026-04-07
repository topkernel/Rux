//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Huge Page Support
//!
//! This module implements support for huge pages (2MB and 1GB) on RISC-V Sv39.
//!
//! RISC-V Sv39 supports:
//! - 4KB base pages (level 0 PTE)
//! - 2MB huge pages (level 1 PTE, also called "megapages")
//! - 1GB giant pages (level 2 PTE, also called "gigapages")
//!
//! Huge pages improve performance by:
//! - Reducing TLB misses (fewer entries cover more memory)
//! - Reducing page table memory overhead
//! - Improving cache utilization

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::vec::Vec;
use crate::sync::seqlock::SeqLock;

use super::PAGE_SIZE;
use super::zone::{GfpFlags, MAX_ORDER};

// ==================== Huge Page Constants ====================

/// Log2 of base page size (4KB = 2^12)
pub const PAGE_SHIFT: usize = 12;

/// Log2 of PMD page size (2MB = 2^21)
pub const PMD_SHIFT: usize = 21;

/// Log2 of PGD page size (1GB = 2^30)
pub const PGDIR_SHIFT: usize = 30;

/// PMD page size (2MB)
pub const PMD_SIZE: usize = 1 << PMD_SHIFT;

/// PGD page size (1GB)
pub const PGDIR_SIZE: usize = 1 << PGDIR_SHIFT;

/// PMD page mask
pub const PMD_MASK: usize = !(PMD_SIZE - 1);

/// PGD page mask
pub const PGDIR_MASK: usize = !(PGDIR_SIZE - 1);

/// Number of base pages in a PMD huge page
pub const HPAGE_PMD_NR: usize = PMD_SIZE / PAGE_SIZE;

/// Number of base pages in a PGD huge page
pub const HPAGE_PGD_NR: usize = PGDIR_SIZE / PAGE_SIZE;

/// Order for PMD huge page allocation
pub const HPAGE_PMD_ORDER: usize = PMD_SHIFT - PAGE_SHIFT;  // 9

/// Order for PGD huge page allocation
pub const HPAGE_PGD_ORDER: usize = PGDIR_SHIFT - PAGE_SHIFT;  // 18

// ==================== Huge Page Types ====================

/// Huge page type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HugePageType {
    /// 2MB huge page (PMD level)
    HugePagePmd,
    /// 1GB huge page (PGD level)
    HugePagePgd,
}

impl HugePageType {
    /// Get page size for this huge page type
    pub fn size(&self) -> usize {
        match self {
            HugePageType::HugePagePmd => PMD_SIZE,
            HugePageType::HugePagePgd => PGDIR_SIZE,
        }
    }

    /// Get order for allocation
    pub fn order(&self) -> usize {
        match self {
            HugePageType::HugePagePmd => HPAGE_PMD_ORDER,
            HugePageType::HugePagePgd => HPAGE_PGD_ORDER,
        }
    }

    /// Check if this order is supported
    pub fn is_supported(&self) -> bool {
        self.order() <= MAX_ORDER
    }
}

// ==================== Huge Page Allocator ====================

/// Huge page allocation statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct HugePageStats {
    /// Number of PMD huge pages allocated
    pub pmd_pages: usize,
    /// Number of PGD huge pages allocated
    pub pgd_pages: usize,
    /// Total huge page memory (bytes)
    pub total_memory: usize,
}

/// Global huge page state
static HUGEPAGE_STATS: SeqLock<HugePageStats> = SeqLock::new(HugePageStats {
    pmd_pages: 0,
    pgd_pages: 0,
    total_memory: 0,
});

/// Allocate a huge page
///
/// # Arguments
/// - `gfp_flags`: GFP flags for allocation
/// - `hp_type`: Type of huge page to allocate
///
/// # Returns
/// - Physical address of the huge page, or 0 if allocation fails
pub fn alloc_hugepage(gfp_flags: GfpFlags, hp_type: HugePageType) -> usize {
    let order = hp_type.order();

    // Check if order is supported
    if order > MAX_ORDER {
        return 0;
    }

    // Allocate from buddy allocator
    let addr = super::page_alloc::alloc_pages(gfp_flags, order);

    if addr != 0 {
        // Update statistics
        let mut stats = HUGEPAGE_STATS.write();
        match hp_type {
            HugePageType::HugePagePmd => stats.pmd_pages += 1,
            HugePageType::HugePagePgd => stats.pgd_pages += 1,
        }
        stats.total_memory += hp_type.size();
    }

    addr
}

/// Free a huge page
///
/// # Arguments
/// - `addr`: Physical address of the huge page
/// - `hp_type`: Type of huge page
pub fn free_hugepage(addr: usize, hp_type: HugePageType) {
    if addr == 0 {
        return;
    }

    // Verify alignment
    let mask = hp_type.size() - 1;
    if addr & mask != 0 {
        // Not properly aligned
        return;
    }

    // Update statistics
    {
        let mut stats = HUGEPAGE_STATS.write();
        match hp_type {
            HugePageType::HugePagePmd => stats.pmd_pages = stats.pmd_pages.saturating_sub(1),
            HugePageType::HugePagePgd => stats.pgd_pages = stats.pgd_pages.saturating_sub(1),
        }
        stats.total_memory = stats.total_memory.saturating_sub(hp_type.size());
    }

    // Free to buddy allocator
    super::page_alloc::free_pages(addr, hp_type.order());
}

/// Allocate a PMD huge page (2MB)
pub fn alloc_hugepage_pmd(gfp_flags: GfpFlags) -> usize {
    alloc_hugepage(gfp_flags, HugePageType::HugePagePmd)
}

/// Free a PMD huge page (2MB)
pub fn free_hugepage_pmd(addr: usize) {
    free_hugepage(addr, HugePageType::HugePagePmd)
}

/// Get huge page statistics
pub fn hugepage_stats() -> HugePageStats {
    HUGEPAGE_STATS.read()
}

// ==================== Huge Page Alignment Helpers ====================

/// Check if address is aligned to PMD huge page
pub fn is_pmd_aligned(addr: usize) -> bool {
    addr & (PMD_SIZE - 1) == 0
}

/// Check if address is aligned to PGD huge page
pub fn is_pgd_aligned(addr: usize) -> bool {
    addr & (PGDIR_SIZE - 1) == 0
}

/// Align address down to PMD boundary
pub fn pmd_align_down(addr: usize) -> usize {
    addr & PMD_MASK
}

/// Align address up to PMD boundary
pub fn pmd_align_up(addr: usize) -> usize {
    (addr + PMD_SIZE - 1) & PMD_MASK
}

/// Align address down to PGD boundary
pub fn pgd_align_down(addr: usize) -> usize {
    addr & PGDIR_MASK
}

/// Align address up to PGD boundary
pub fn pgd_align_up(addr: usize) -> usize {
    (addr + PGDIR_SIZE - 1) & PGDIR_MASK
}

// ==================== VMA Huge Page Flags ====================

/// VMA flags for huge pages
pub mod vm_flags {
    /// Use huge pages for this VMA
    pub const VM_HUGETLB: u64 = 1 << 0;
    /// Huge page is at PMD level (2MB)
    pub const VM_HUGE_PMD: u64 = 1 << 1;
    /// Huge page is at PGD level (1GB)
    pub const VM_HUGE_PGD: u64 = 1 << 2;
    /// Align to huge page boundary
    pub const VM_HUGE_ALIGN: u64 = 1 << 3;
}

// ==================== Page Table Entry Helpers ====================

/// Page table entry flags for huge pages
pub mod pte_flags {
    /// Valid bit
    pub const V: u64 = 1 << 0;
    /// Readable
    pub const R: u64 = 1 << 1;
    /// Writable
    pub const W: u64 = 1 << 2;
    /// Executable
    pub const X: u64 = 1 << 3;
    /// User accessible
    pub const U: u64 = 1 << 4;
    /// Global (not flushed on context switch)
    pub const G: u64 = 1 << 5;
    /// Accessed
    pub const A: u64 = 1 << 6;
    /// Dirty
    pub const D: u64 = 1 << 7;

    /// Default kernel huge page flags
    pub const KERNEL_HUGE: u64 = V | R | W | X | A | D;

    /// Default user huge page flags
    pub const USER_HUGE: u64 = V | R | W | X | U | A | D;
}

/// Check if PTE is a huge page (leaf at level 1 or 2)
pub fn is_huge_pte(pte: u64, level: usize) -> bool {
    // Level 0 is always a base page
    if level == 0 {
        return false;
    }

    // PTE must be valid and have R/W/X bits set (leaf PTE)
    let flags = pte & 0xFF;
    (flags & pte_flags::V != 0) && (flags & (pte_flags::R | pte_flags::W | pte_flags::X) != 0)
}

// ==================== Debug/Info ====================

/// Print huge page status
pub fn print_hugepage_info() {
    let stats = hugepage_stats();

    crate::println!("Huge Page Status:");
    crate::println!("  PMD pages (2MB):  {} ({} MB)",
        stats.pmd_pages,
        stats.pmd_pages * PMD_SIZE / (1024 * 1024));
    crate::println!("  PGD pages (1GB):  {} ({} GB)",
        stats.pgd_pages,
        stats.pgd_pages);
    crate::println!("  Total memory:     {} MB",
        stats.total_memory / (1024 * 1024));
    crate::println!("  PMD order:        {}", HPAGE_PMD_ORDER);
    crate::println!("  PGD order:        {}", HPAGE_PGD_ORDER);
    crate::println!("  PMD supported:    {}", HugePageType::HugePagePmd.is_supported());
    crate::println!("  PGD supported:    {}", HugePageType::HugePagePgd.is_supported());
}
