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
            // SAFETY: ptr was stored by set_vma() and the VMA outlives the page.
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
    // SAFETY: page is exclusively owned (refcount == 1) when adding rmap.
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
    // SAFETY: page is exclusively owned when adding file rmap.
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
    // SAFETY: caller holds page lock (or page is unshared).
    unsafe {
        // R21-1: dec_mapcount returns the POST-decrement value; the
        // convention is -1 (PAGE_MAPCOUNT_BIAS) = unmapped, 0 = one
        // mapping. The old `== 0` check fired one mapping EARLY (a
        // fork-COW page lost Anonymous/LRU at 2->1) and never on the true
        // last unmap (page freed to buddy still LRU-linked).
        let new_count = page.dec_mapcount();

        // If last mapping, clear flags and remove from LRU.
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        if new_count == -1 {
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
/// # Returns
/// Number of PTEs successfully unmapped.
pub fn try_to_unmap(page: &Page) -> i32 {
    try_to_unmap_inner(page, 0)
}

/// Try to unmap a page from all processes, replacing PTEs with a swap entry.
///
/// Like `try_to_unmap()` but writes `swap_entry` into each PTE instead of
/// zeroing it. Used by the swap-out path in vmscan.
///
/// # Returns
/// Number of PTEs successfully replaced with swap entries.
pub fn try_to_unmap_with_swap(page: &Page, swap_entry: u64) -> i32 {
    try_to_unmap_inner(page, swap_entry)
}

/// Record a (mm, virtual page number) mapping on the page (review 4.10).
///
/// Slot 0 (the `index` field) holds the FIRST mapping; up to three more
/// fit in `rmap_alt`. Beyond that the page keeps only slot 0 and the
/// `RmapOverflow` flag sends reverse-map lookups through the full task
/// scan (degraded single-value recording).
///
/// Called from the fault paths that install a PTE (page_fault.rs /
/// mm_ops.rs) — `page_add_anon_rmap` has no live callers.
pub fn page_record_mapping(page: &Page, mm_ptr: usize, vaddr: usize) {
    let vpn = vaddr / super::PAGE_SIZE;
    if page.index() == 0 {
        // First mapping — slot 0.
        page.set_index(vpn);
        return;
    }
    if page.index() == vpn {
        return; // re-recording the same mapping (index only) — no-op
    }
    if !page.rmap_alt_add(mm_ptr, vpn) {
        // All 3 alternate slots busy: degrade to single-value recording.
        page.set_flag(super::page_desc::PageFlag::RmapOverflow);
    }
}

/// Try to unmap a page from all processes, replacing PTEs with a
/// migration-entry marker (compaction, review 4.16).
///
/// Like `try_to_unmap()` but writes a migration marker into each PTE
/// instead of zeroing it: a fault on the marker WAITS for the migration to
/// complete instead of installing a zero page (which the subsequent remap
/// would overwrite — silent user-write loss).
///
/// # Returns
/// Number of PTEs successfully replaced with migration markers.
pub fn try_to_unmap_migration(page: &Page) -> i32 {
    try_to_unmap_inner(page, super::swap::make_migration_entry())
}

/// Shared implementation for try_to_unmap and try_to_unmap_with_swap.
///
/// When `swap_entry == 0`, PTEs are zeroed (unmap).
/// When `swap_entry != 0`, PTEs are replaced with the swap entry (swap-out).
fn try_to_unmap_inner(page: &Page, swap_entry: u64) -> i32 {
    if !page_mapped(page) {
        return 0;
    }

    if !page.is_anonymous() {
        // File-backed pages: not yet supported (needs address_space walk).
        return 0;
    }

    let target_pfn = super::page_desc::page_to_pfn(page as *const Page);
    let target_index = page.index();
    if target_index == 0 && !page.test_flag(super::page_desc::PageFlag::RmapOverflow) {
        return 0;
    }

    // Candidate virtual addresses: slot 0 (page.index) plus every alternate
    // rmap slot (multi-mapping pages — MAP_FIXED re-maps of the same frame).
    // Review 4.10: with only slot 0, unmapping a double-mapped page left the
    // second PTE in place while the page was freed/reused (stale-PTE UAF).
    let mut target_vaddrs: Vec<usize> = Vec::new();
    if target_index != 0 {
        target_vaddrs.push(target_index * (super::PAGE_SIZE as usize));
    }
    page.rmap_alt_for_each(|_mm, vpn| {
        let vaddr = vpn * (super::PAGE_SIZE as usize);
        if !target_vaddrs.contains(&vaddr) {
            target_vaddrs.push(vaddr);
        }
    });

    let unmapped_count = Cell::new(0i32);

    crate::sched::for_each_task(|task_ptr| {
        // SAFETY: for_each_task provides valid task pointers; the task struct
        // is protected by the task list lock inside for_each_task.
        unsafe {
            let task = &*task_ptr;

            // Skip tasks without an address space (kernel threads)
            // R25-2: PIN the address space — do_exit's set_address_space(None)
            // + Drop(MmStruct) free the page tables without the VMA lock;
            // a raw borrow raced that free (NEW2-shaped UAF-read).
            let _mm_arc = match task.address_space_arc() {
                Some(a) => a,
                None => return,
            };
            let mm = _mm_arc.as_ref();

            for target_vaddr in target_vaddrs.iter() {
                let target_vaddr = *target_vaddr;

                // Hold VMA lock across both the check and page table walk
                // to prevent concurrent munmap from freeing page tables (fixes F03-07).
                let vma_mgr = mm.vma_read();
                let vma_matches = vma_mgr.iter().any(|vma| {
                    vma.vma_type() == super::vma::VmaType::Anonymous
                        && vma.contains(super::page::VirtAddr::new(target_vaddr))
                });

                if !vma_matches {
                    continue;
                }
                // vma_mgr still held — protects page table walk below

                let root_ppn = mm.pgd();
                let walk_result = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(
                    root_ppn, target_vaddr as u64,
                );

                if let Some((ppn, _pte_bits)) = walk_result {
                    if ppn as usize == target_pfn {
                        let vpn2 = ((target_vaddr >> 30) & 0x1FF) as usize;
                        let vpn1 = ((target_vaddr >> 21) & 0x1FF) as usize;
                        let vpn0 = ((target_vaddr >> 12) & 0x1FF) as usize;

                        let root_table = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                            root_ppn << crate::arch::riscv64::mm::PAGE_SHIFT,
                        );
                        let pte2 = (*root_table).get(vpn2);
                        if !pte2.is_valid() { continue; }

                        let table1 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                            pte2.ppn() << crate::arch::riscv64::mm::PAGE_SHIFT,
                        );
                        let pte1 = (*table1).get(vpn1);
                        if !pte1.is_valid() { continue; }

                        let table0 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                            pte1.ppn() << crate::arch::riscv64::mm::PAGE_SHIFT,
                        );

                        // R22-3 (§17.4 close): leaf-PTE mutation under the
                        // PTE lock like every other writer (fork-COW/munmap/
                        // mprotect/fault-map) — was racing them.
                        let _pte_g = crate::arch::riscv64::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();

                        // Re-validate the leaf under the lock (R7-C3 pattern):
                        // the walk above ran OUTSIDE it, and a concurrent leaf
                        // writer (COW break / swap-in / munmap) may have
                        // swapped in a different physical page in between.
                        // Overwriting blind would zero the NEW page's PTE while
                        // decrementing THIS page's mapcount. Intermediate
                        // levels are stable here: page tables are only unlinked
                        // at mm teardown, excluded by the mm pin above.
                        let pte0 = (*table0).get(vpn0);
                        if !pte0.is_valid() || pte0.ppn() as usize != target_pfn {
                            drop(_pte_g);
                            continue;
                        }

                        // Write new PTE value (0 for unmap, swap_entry for swap-out,
                        // migration marker during compaction)
                        (*table0).set(
                            vpn0,
                            crate::arch::riscv64::mm::pagetable::PageTableEntry::from_bits(swap_entry),
                        );

                        drop(_pte_g);
                        // R10-6: sfence.vma is HART-LOCAL — a "global" flush
                        // buys nothing over the per-address one (round-9's
                        // R9-18 was ineffective by ISA semantics). Keep the
                        // cheap form; cross-CPU shootdown is issued in batch
                        // by the unmap/exit callers via IPI (see
                        // arch::ipi::flush_tlb_others, review 4.10).
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
