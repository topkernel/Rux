//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory compaction — reduce external fragmentation by relocating movable
//! pages to consolidate free blocks at low addresses.
//!
//! Algorithm: two-pointer scan (migrate scanner UP, free scanner DOWN).
//! Movable pages (anonymous, mapped, refcount==1) are copied to free
//! destination pages and their PTEs are updated to the new PFN.
//!
//! Triggered by `alloc_pages()` when a high-order allocation fails.
//! Reference: `refer/linux/mm/compaction.c`


use super::page_desc::{
    Page, PageFlag, pfn_to_page, pfn_to_page_mut, page_to_pfn, copy_page_contents,
};
use super::zone::{Zone, pfn_to_phys};
use super::rmap::try_to_unmap;
use super::PAGE_SIZE;

// ============================================================================
// Types
// ============================================================================

/// Result of a compaction attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompactResult {
    /// Found a free block of the requested order.
    Success,
    /// Zone fully scanned but no suitable block found.
    Complete,
    /// No movable pages to compact.
    Skipped,
}

/// Controls a single compaction pass over a zone.
struct CompactControl {
    /// Zone being compacted.
    zone: *mut Zone,
    /// Migrate scanner: scans upward from zone start.
    migrate_pfn: usize,
    /// Free scanner: scans downward from zone end.
    free_pfn: usize,
    /// Target allocation order.
    order: usize,
    /// Pages successfully migrated.
    nr_migrated: usize,
    /// Total pages scanned (for limiting scan depth).
    nr_scanned: usize,
}

/// Maximum pages to scan in a single compaction pass.
const MAX_SCAN_PAGES: usize = 4096;

// ============================================================================
// Public API
// ============================================================================

/// Attempt to compact a zone to free a contiguous block of the given order.
///
/// Scans the zone with two pointers (migrate UP, free DOWN), relocating
/// movable anonymous pages to create contiguous free space.
///
/// # Safety
/// `zone` must be a valid pointer to an initialized zone.
pub unsafe fn compact_zone(zone: *mut Zone, order: usize) -> CompactResult {
    let start = (*zone).start_pfn();
    let end = (*zone).end_pfn();

    if start >= end {
        return CompactResult::Skipped;
    }

    let mut cc = CompactControl {
        zone,
        migrate_pfn: start,
        free_pfn: end,
        order,
        nr_migrated: 0,
        nr_scanned: 0,
    };

    crate::pr_debug!(
        "compact: start zone=[{}, {}) order={}",
        start, end, order
    );

    let result = compact_zone_inner(&mut cc);

    crate::pr_debug!(
        "compact: done migrated={} scanned={:?}",
        cc.nr_migrated, result
    );

    result
}

// ============================================================================
// Core algorithm
// ============================================================================

/// Core compaction loop: alternate between free and migrate scanners.
///
/// # Safety
/// `cc.zone` must be a valid pointer to an initialized zone.
unsafe fn compact_zone_inner(cc: &mut CompactControl) -> CompactResult {
    loop {
        // Termination: scanners met or exceeded scan limit
        if cc.migrate_pfn >= cc.free_pfn || cc.nr_scanned >= MAX_SCAN_PAGES {
            return if cc.nr_migrated > 0 {
                CompactResult::Complete
            } else {
                CompactResult::Skipped
            };
        }

        // Phase 1: Find a free destination page
        let dst_pfn = match find_free_page(cc) {
            Some(pfn) => pfn,
            None => return CompactResult::Complete,
        };

        // Phase 2: Find a movable source page
        let src_pfn = match find_migrate_page(cc) {
            Some(pfn) => pfn,
            None => return CompactResult::Complete,
        };

        if src_pfn == dst_pfn {
            continue;
        }

        // Phase 3: Migrate the page
        if migrate_page(src_pfn, dst_pfn) {
            cc.nr_migrated += 1;
        }

        // Phase 4: Check if we already have a suitable free block
        if (*cc.zone).has_free_block(cc.order) {
            return CompactResult::Success;
        }
    }
}

// ============================================================================
// Free page scanner (scans downward)
// ============================================================================

/// Find a free page to use as migration destination.
///
/// First tries the buddy free list. If empty, walks downward from
/// `free_pfn` looking for an unmapped, free page descriptor.
/// Find a free page to use as migration destination.
///
/// First tries the buddy free list. If empty, walks downward from
/// `free_pfn` looking for an unmapped, free page descriptor.
///
/// # Safety
/// `cc.zone` must be a valid pointer; `cc.free_pfn` and `cc.migrate_pfn`
/// must be within the zone's PFN range.
unsafe fn find_free_page(cc: &mut CompactControl) -> Option<usize> {
    let zone = &*cc.zone;

    // Fast path: try buddy allocator
    if let Some(pfn) = zone.alloc_single_page() {
        return Some(pfn);
    }

    // Slow path: walk downward from free_pfn
    let start = zone.start_pfn();
    while cc.free_pfn > cc.migrate_pfn && cc.free_pfn > start {
        cc.free_pfn -= 1;
        cc.nr_scanned += 1;

        let page = pfn_to_page(cc.free_pfn);
        if page.is_null() {
            continue;
        }

        let p = &*page;
        if p.is_free() && !p.test_flag(PageFlag::Reserved) {
            return Some(cc.free_pfn);
        }
    }

    None
}

// ============================================================================
// Migrate page scanner (scans upward)
// ============================================================================

/// Find a movable anonymous page to relocate.
///
/// A page is movable if:
/// - It is anonymous and mapped
/// - Reference count == 1 (only page-table references)
/// - Not reserved, not locked
/// - Not dirty (avoids writeback complexity)
/// Find a movable anonymous page to relocate.
///
/// A page is movable if:
/// - It is anonymous and mapped
/// - Reference count == 1 (only page-table references)
/// - Not reserved, not locked
/// - Not dirty (avoids writeback complexity)
///
/// # Safety
/// `cc.zone` must be a valid pointer; `cc.migrate_pfn` must be within
/// the zone's PFN range.
unsafe fn find_migrate_page(cc: &mut CompactControl) -> Option<usize> {
    let zone = &*cc.zone;
    let end = zone.end_pfn();

    while cc.migrate_pfn < cc.free_pfn && cc.migrate_pfn < end {
        let pfn = cc.migrate_pfn;
        cc.migrate_pfn += 1;
        cc.nr_scanned += 1;

        let page = pfn_to_page(pfn);
        if page.is_null() {
            continue;
        }

        let p = &*page;

        // Skip free pages
        if p.is_free() {
            continue;
        }

        // Skip reserved pages
        if p.test_flag(PageFlag::Reserved) {
            continue;
        }

        // Skip non-anonymous pages (page cache, slab, etc.)
        if !p.is_anonymous() {
            continue;
        }

        // Skip unmapped pages
        if !super::rmap::page_mapped(p) {
            continue;
        }

        // Skip pages with extra references (pinned, etc.)
        if p.refcount() != 1 {
            continue;
        }

        // Skip dirty pages (writeback not supported in compaction)
        if p.is_dirty() {
            continue;
        }

        return Some(pfn);
    }

    None
}

// ============================================================================
// Page migration
// ============================================================================

/// Migrate a page from `src_pfn` to `dst_pfn`.
///
/// Steps:
/// 1. Save the virtual address from `src_page.index`
/// 2. `try_to_unmap(src_page)` — remove all PTEs
/// 3. `copy_page_contents(src, dst)` — memcpy 4KB
/// 4. `remap_page(dst_page, vaddr)` — install new PTEs pointing to dst
/// 5. Transfer metadata (anon flags, mapping, index) from src to dst
/// 6. `free_pages(src_pfn, 0)` — release source to buddy
///
/// Returns true on success.
/// Migrate a page from `src_pfn` to `dst_pfn`.
///
/// Steps:
/// 1. Save the virtual address from `src_page.index`
/// 2. `try_to_unmap(src_page)` — remove all PTEs
/// 3. `copy_page_contents(src, dst)` — memcpy 4KB
/// 4. `remap_page(dst_page, vaddr)` — install new PTEs pointing to dst
/// 5. Transfer metadata (anon flags, mapping, index) from src to dst
/// 6. `free_pages(src_pfn, 0)` — release source to buddy
///
/// # Safety
/// Both PFNs must be valid, mapped page descriptors. The source page must
/// be exclusively owned (refcount == 1). The destination must be free.
///
/// Returns true on success.
unsafe fn migrate_page(src_pfn: usize, dst_pfn: usize) -> bool {
    let src_page = pfn_to_page(src_pfn);
    let dst_page = pfn_to_page_mut(dst_pfn);
    if src_page.is_null() || dst_page.is_null() {
        return false;
    }

    let src = &*src_page;
    let dst = &mut *dst_page;

    // Step 1: Save virtual address (stored as VPN in page.index)
    let old_vaddr = src.index();
    if old_vaddr == 0 {
        return false;
    }

    // Step 2: Unmap from all processes
    let unmapped = try_to_unmap(src);
    if unmapped == 0 {
        return false;
    }

    // Step 3: Copy page contents
    copy_page_contents(src_pfn, dst_pfn);

    // Step 4: Install new PTEs pointing to dst_pfn
    remap_page(dst, old_vaddr);

    // Step 5: Transfer rmap metadata from src to dst
    let mapping = src.mapping();
    let index = src.index();

    dst.set_mapping(mapping);
    dst.set_index(index);
    dst.set_flag(PageFlag::Anonymous);
    dst.set_flag(PageFlag::SwapBacked);
    dst.set_flag(PageFlag::Referenced);
    dst.set_refcount(1);

    // Clear src metadata
    // (refcount is already 0 after try_to_unmap decremented it)

    // Step 6: Release source page back to buddy
    super::page_alloc::free_pages(pfn_to_phys(src_pfn), 0);

    true
}

// ============================================================================
// PTE remap
// ============================================================================

/// Install PTEs mapping `old_vaddr` to the new page (`dst`).
///
/// Walks all tasks' page tables looking for anonymous VMAs that contain
/// `old_vaddr`, and updates the PTE's PPN to point to the new page.
/// This is the reverse of `try_to_unmap()`.
/// Install PTEs mapping `old_vaddr` to the new page (`dst`).
///
/// Walks all tasks' page tables looking for anonymous VMAs that contain
/// `old_vaddr`, and updates the PTE's PPN to point to the new page.
/// This is the reverse of `try_to_unmap()`.
///
/// # Safety
/// `dst` must be a valid, initialized page descriptor. `old_vaddr` must
/// be a virtual address that was previously mapped to the source page.
unsafe fn remap_page(dst: &Page, old_vaddr: usize) {
    let new_pfn = page_to_pfn(dst as *const Page);
    let new_ppn = new_pfn as u64;

    crate::sched::for_each_task(|task_ptr| {
        let task = &*task_ptr;

        // Skip tasks without an address space
        let mm = match task.address_space() {
            Some(m) => m,
            None => return,
        };

        // Quick check: does any anonymous VMA contain old_vaddr?
        let vma_matches = {
            let vma_mgr = mm.vma_read();
            let m = vma_mgr.iter().any(|vma| {
                vma.vma_type() == super::vma::VmaType::Anonymous
                    && vma.contains(super::page::VirtAddr::new(old_vaddr))
            });
            m
        };
        // vma_mgr guard dropped here

        if !vma_matches {
            return;
        }

        // Walk page table to find and update the PTE
        let root_ppn = mm.pgd();
        let walk_result = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(
            root_ppn, old_vaddr as u64,
        );

        if let Some((_ppn, _pte_bits)) = walk_result {
            let vpn2 = ((old_vaddr >> 30) & 0x1FF) as usize;
            let vpn1 = ((old_vaddr >> 21) & 0x1FF) as usize;
            let vpn0 = ((old_vaddr >> 12) & 0x1FF) as usize;

            let root_table = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                root_ppn << super::hugepage::PAGE_SHIFT,
            );
            let pte2 = (*root_table).get(vpn2);
            if !pte2.is_valid() {
                return;
            }

            let table1 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                pte2.ppn() << super::hugepage::PAGE_SHIFT,
            );
            let pte1 = (*table1).get(vpn1);
            if !pte1.is_valid() {
                return;
            }

            let table0 = crate::arch::riscv64::mm::mmu_init::get_page_table_virt(
                pte1.ppn() << super::hugepage::PAGE_SHIFT,
            );
            let old_pte = (*table0).get(vpn0);

            // Update PPN bits, preserve all flags (R/W/X/U/D/A/G)
            let new_pte_bits = (old_pte.bits() & !(0x00FFFFFFFFFFFFFF)) | (new_ppn << 10);
            (*table0).set(
                vpn0,
                crate::arch::riscv64::mm::pagetable::PageTableEntry::from_bits(new_pte_bits),
            );

            // Flush TLB for this address
            core::arch::asm!(
                "fence",
                "sfence.vma {}, zero",
                "fence",
                in(reg) old_vaddr,
                options(nostack, preserves_flags)
            );

            // Increment mapcount on the new page
            dst.add_mapcount();
        }
    });
}
