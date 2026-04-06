//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Reverse Mapping (rmap) Infrastructure
//!
//! This module implements reverse mapping, which allows finding all
//! virtual addresses that map a given physical page. This is essential
//! for page migration, memory compaction, and page reclamation.

extern crate alloc;

use core::cell::Cell;
use core::sync::atomic::{AtomicUsize, AtomicPtr, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;
use crate::sync::rwlock::RwSpinlock;

use super::page_desc::Page;
use super::vma::Vma;

// ==================== AnonVma ====================

/// Anonymous VMA
///
/// Represents a group of VMAs that share anonymous pages.
/// When a page is shared (e.g., after fork), all processes
/// mapping that page are linked through anon_vma.
pub struct AnonVma {
    /// Reference count
    refcount: AtomicUsize,

    /// Root anon_vma (for hierarchical anon_vmas)
    root: AtomicPtr<AnonVma>,

    /// List of child anon_vmas
    children: RwSpinlock<Vec<Arc<AnonVma>>>,

    /// Associated VMA
    vma: AtomicUsize,
}

impl AnonVma {
    /// Create a new anon_vma
    pub fn new() -> Self {
        Self {
            refcount: AtomicUsize::new(1),
            root: AtomicPtr::new(core::ptr::null_mut()),
            children: RwSpinlock::new(Vec::new()),
            vma: AtomicUsize::new(0),
        }
    }

    /// Increment reference count
    pub fn get(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement reference count
    /// Returns true if this was the last reference
    pub fn put(&self) -> bool {
        self.refcount.fetch_sub(1, Ordering::AcqRel) == 1
    }

    /// Get reference count
    pub fn refcount(&self) -> usize {
        self.refcount.load(Ordering::Acquire)
    }

    /// Set associated VMA
    pub fn set_vma(&self, vma: *const Vma) {
        self.vma.store(vma as usize, Ordering::Release);
    }

    /// Get associated VMA
    pub fn get_vma(&self) -> Option<&Vma> {
        let ptr = self.vma.load(Ordering::Acquire) as *const Vma;
        if ptr.is_null() {
            None
        } else {
            unsafe { Some(&*ptr) }
        }
    }
}

/// Anonymous VMA chain entry
pub type AnonVmaChain = AnonVma;

// ==================== Page Rmap Operations ====================

/// Add reverse mapping for an anonymous page
///
/// # Arguments
/// - `page`: Page descriptor
/// - `vma`: VMA containing the mapping
/// - `address`: Virtual address of the mapping
/// - `exclusive`: Whether this is an exclusive mapping
pub fn page_add_anon_rmap(page: &Page, _vma: &Vma, address: usize, _exclusive: bool) {
    unsafe {
        // Set anonymous and swap-backed flags
        page.set_flag(super::page_desc::PageFlag::Anonymous);
        page.set_flag(super::page_desc::PageFlag::SwapBacked);

        // Set index (virtual page offset) for rmap
        let index = address / super::PAGE_SIZE;
        page.set_index(index);

        // Increment map count
        page.inc_mapcount();

        // Add to LRU_INACTIVE_ANON on first mapping
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        if page.mapcount() == 0 {
            super::lru::page_add_anon_lru(page);
        }
    }
}

/// Add reverse mapping for a file-backed page
///
/// # Arguments
/// - `page`: Page descriptor
/// - `mapping`: Address space (file mapping)
/// - `index`: Page offset in the file
pub fn page_add_file_rmap(page: &Page, mapping: usize, index: usize) {
    unsafe {
        // Set mapping and index (rmap only; LRU uses dedicated field)
        page.set_mapping(mapping as *mut core::ffi::c_void);
        page.set_index(index);

        // Increment map count
        page.inc_mapcount();

        // Add to LRU_INACTIVE_FILE on first mapping
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        if page.mapcount() == 0 {
            super::lru::page_add_file_lru(page);
        }
    }
}

/// Remove reverse mapping for a page
///
/// # Arguments
/// - `page`: Page descriptor
pub fn page_remove_rmap(page: &Page) {
    unsafe {
        // Decrement map count
        let old_count = page.dec_mapcount();

        // If last mapping, clear flags and remove from LRU.
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        if old_count == 0 {
            page.clear_flag(super::page_desc::PageFlag::Anonymous);
            page.clear_flag(super::page_desc::PageFlag::SwapBacked);
            super::lru::page_remove_lru(page);
        }
    }
}

/// Check if a page is mapped (has at least one PTE)
pub fn page_mapped(page: &Page) -> bool {
    page.mapcount() >= 0
}

/// Get all virtual addresses mapping a page
///
/// This is used for page migration and memory compaction.
///
/// # Returns
/// Vector of (mm_struct_ptr, virtual_address) pairs
pub fn page_get_mappings(_page: &Page) -> Vec<(usize, usize)> {
    // In a full implementation, this would:
    // 1. Check if page is anonymous or file-backed
    // 2. For anonymous pages: walk anon_vma chain
    // 3. For file pages: walk address_space i_mmap tree
    // 4. Return all (mm, address) pairs

    // Placeholder: return empty vector
    Vec::new()
}

/// Check if page was recently referenced
///
/// Used by page reclamation to determine if a page is still
/// being accessed.
pub fn page_referenced(page: &Page) -> bool {
    // Check referenced flag
    page.test_flag(super::page_desc::PageFlag::Referenced)
}

/// Clear referenced flag on a page
pub fn page_clear_referenced(page: &Page) {
    page.clear_flag(super::page_desc::PageFlag::Referenced);
}

/// Try to unmap a page from all processes.
///
/// Used during page reclamation (vmscan) to remove all PTEs mapping
/// a given physical page so it can be freed back to the zone allocator.
///
/// Approach: task scan — iterate all tasks, check VMAs for the virtual
/// address stored in `page.index()`, verify the PTE maps the target PFN,
/// then clear the PTE and flush TLB.
///
/// # Returns
/// Number of PTEs successfully unmapped.
pub fn try_to_unmap(page: &Page) -> i32 {
    if !page_mapped(page) {
        return 0;
    }

    if !page.is_anonymous() {
        // File-backed pages: not yet supported (needs address_space walk).
        // These are typically reclaimed via the page cache shrinker instead.
        return 0;
    }

    let target_pfn = super::page_desc::page_to_pfn(page as *const Page);
    let target_index = page.index();
    if target_index == 0 {
        return 0;
    }
    let target_vaddr = target_index * (super::PAGE_SIZE as usize);

    let unmapped_count = Cell::new(0i32);

    // Scan all tasks for PTEs mapping this physical page.
    crate::sched::for_each_task(|task_ptr| {
        unsafe {
            let task = &*task_ptr;

            // Skip tasks without an address space (kernel threads)
            let mm = match task.address_space() {
                Some(m) => m,
                None => return,
            };

            // Quick check: does any anonymous VMA contain target_vaddr?
            let vma_matches = {
                let vma_mgr = mm.vma_read();
                let matches = vma_mgr.iter().any(|vma| {
                    vma.vma_type() == super::vma::VmaType::Anonymous
                        && vma.contains(super::page::VirtAddr::new(target_vaddr))
                });
                matches
            };

            if !vma_matches {
                return;
            }

            // Walk the page table at target_vaddr to verify PPN matches.
            let root_ppn = mm.pgd();
            let walk_result = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(
                root_ppn, target_vaddr as u64,
            );

            if let Some((ppn, _pte_bits)) = walk_result {
                if ppn as usize == target_pfn {
                    // Match found — clear the PTE.
                    let vpn2 = ((target_vaddr >> 30) & 0x1FF) as usize;
                    let vpn1 = ((target_vaddr >> 21) & 0x1FF) as usize;
                    let vpn0 = ((target_vaddr >> 12) & 0x1FF) as usize;

                    let root_table = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                        root_ppn << crate::arch::riscv64::mm::PAGE_SHIFT,
                    );
                    let pte2 = (*root_table).get(vpn2);
                    if !pte2.is_valid() { return; }

                    let table1 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                        pte2.ppn() << crate::arch::riscv64::mm::PAGE_SHIFT,
                    );
                    let pte1 = (*table1).get(vpn1);
                    if !pte1.is_valid() { return; }

                    let table0 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                        pte1.ppn() << crate::arch::riscv64::mm::PAGE_SHIFT,
                    );

                    // Zero the PTE
                    (*table0).set(
                        vpn0,
                        crate::arch::riscv64::mm::pagetable::PageTableEntry::from_bits(0),
                    );

                    // Flush TLB for this address
                    core::arch::asm!(
                        "fence",
                        "sfence.vma {}, zero",
                        "fence",
                        in(reg) target_vaddr,
                        options(nostack, preserves_flags)
                    );

                    // Decrement mapcount
                    page.dec_mapcount();
                    unmapped_count.set(unmapped_count.get() + 1);
                }
            }
        }
    });

    unmapped_count.get()
}

/// Try to unmap a page from all processes, replacing PTEs with a swap entry.
///
/// Like `try_to_unmap()` but writes `swap_entry` into each PTE instead of
/// zeroing it. Used by the swap-out path in vmscan.
///
/// # Returns
/// Number of PTEs successfully replaced with swap entries.
pub fn try_to_unmap_with_swap(page: &Page, swap_entry: u64) -> i32 {
    if !page_mapped(page) {
        return 0;
    }

    if !page.is_anonymous() {
        return 0;
    }

    let target_pfn = super::page_desc::page_to_pfn(page as *const Page);
    let target_index = page.index();
    if target_index == 0 {
        return 0;
    }
    let target_vaddr = target_index * (super::PAGE_SIZE as usize);

    let unmapped_count = Cell::new(0i32);

    crate::sched::for_each_task(|task_ptr| {
        unsafe {
            let task = &*task_ptr;

            let mm = match task.address_space() {
                Some(m) => m,
                None => return,
            };

            // Quick check: does any anonymous VMA contain target_vaddr?
            let vma_matches = {
                let vma_mgr = mm.vma_read();
                let matches = vma_mgr.iter().any(|vma| {
                    vma.vma_type() == super::vma::VmaType::Anonymous
                        && vma.contains(super::page::VirtAddr::new(target_vaddr))
                });
                matches
            };

            if !vma_matches {
                return;
            }

            // Walk the page table at target_vaddr to verify PPN matches.
            let root_ppn = mm.pgd();
            let walk_result = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(
                root_ppn, target_vaddr as u64,
            );

            if let Some((ppn, _pte_bits)) = walk_result {
                if ppn as usize == target_pfn {
                    // Match found — write swap entry into PTE.
                    let vpn2 = ((target_vaddr >> 30) & 0x1FF) as usize;
                    let vpn1 = ((target_vaddr >> 21) & 0x1FF) as usize;
                    let vpn0 = ((target_vaddr >> 12) & 0x1FF) as usize;

                    let root_table = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                        root_ppn << crate::arch::riscv64::mm::PAGE_SHIFT,
                    );
                    let pte2 = (*root_table).get(vpn2);
                    if !pte2.is_valid() { return; }

                    let table1 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                        pte2.ppn() << crate::arch::riscv64::mm::PAGE_SHIFT,
                    );
                    let pte1 = (*table1).get(vpn1);
                    if !pte1.is_valid() { return; }

                    let table0 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                        pte1.ppn() << crate::arch::riscv64::mm::PAGE_SHIFT,
                    );

                    // Write swap entry into PTE (V=0, triggers fault on next access)
                    (*table0).set(
                        vpn0,
                        crate::arch::riscv64::mm::pagetable::PageTableEntry::from_bits(swap_entry),
                    );

                    // Flush TLB for this address
                    core::arch::asm!(
                        "fence",
                        "sfence.vma {}, zero",
                        "fence",
                        in(reg) target_vaddr,
                        options(nostack, preserves_flags)
                    );

                    // Decrement mapcount
                    page.dec_mapcount();
                    unmapped_count.set(unmapped_count.get() + 1);
                }
            }
        }
    });

    unmapped_count.get()
}

// ==================== Rmap Statistics ====================

/// Reverse mapping statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct RmapStats {
    /// Number of anon_vmas allocated
    pub anon_vma_count: usize,
    /// Number of pages with reverse mappings
    pub mapped_pages: usize,
    /// Number of pages currently being migrated
    pub migrating_pages: usize,
}

/// Get rmap statistics
pub fn rmap_stats() -> RmapStats {
    // Placeholder: would need to track these globally
    RmapStats::default()
}
