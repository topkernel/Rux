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
    // Set mapping field in Page
    unsafe {
        // Set anonymous flag
        page.set_flag(super::page_desc::PageFlag::Anonymous);

        // Set index (virtual page offset)
        let index = address / super::PAGE_SIZE;
        page.set_index(index);

        // Increment map count
        page.inc_mapcount();
    }

    // TODO: Add to LRU_INACTIVE_ANON on first mapping
    // Disabled until try_to_unmap is properly implemented
    // if page.mapcount() == 0 {
    //     super::lru::page_add_anon_lru(page);
    // }
}

/// Add reverse mapping for a file-backed page
///
/// # Arguments
/// - `page`: Page descriptor
/// - `mapping`: Address space (file mapping)
/// - `index`: Page offset in the file
pub fn page_add_file_rmap(page: &Page, mapping: usize, index: usize) {
    unsafe {
        // Set mapping and index
        page.set_mapping(mapping as *mut core::ffi::c_void);
        page.set_index(index);

        // Increment map count
        page.inc_mapcount();
    }

    // Add to LRU_INACTIVE_FILE on first mapping
    if page.mapcount() == 0 {
        super::lru::page_add_file_lru(page);
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

        // If last mapping, clear mapping field and remove from LRU
        if old_count == 0 {
            page.set_mapping(core::ptr::null_mut());
            page.clear_flag(super::page_desc::PageFlag::Anonymous);
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

/// Try to unmap a page from all processes
///
/// Used during page migration and reclamation.
///
/// # Returns
/// - Number of unmapped PTEs, or error code
pub fn try_to_unmap(page: &Page) -> i32 {
    if !page_mapped(page) {
        return 0;
    }

    // In a full implementation, this would:
    // 1. Get all mappings
    // 2. For each mapping, clear the PTE
    // 3. Flush TLB entries
    // 4. Return count of unmapped PTEs

    // Placeholder: just clear mapcount
    unsafe {
        while page.mapcount() >= 0 {
            page.dec_mapcount();
        }
    }

    0
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
