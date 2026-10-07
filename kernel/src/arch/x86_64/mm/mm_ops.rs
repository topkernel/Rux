//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 user address-space operations (MmStruct extensions, COW).
//!
//! Ported 1:1 in shape from arch/riscv64/mm/mm_ops.rs: 4-level
//! PageTableWalker, create_user_address_space (kernel PML4 half shared:
//! PML4[256..511] links, plus a leaf-only clone of the PML4[0] identity
//! subtree so kernel trap-context device access works under a user CR3),
//! map_user_region, copy_page_table_cow (fork), handle_cow_fault,
//! check_pte_permissions. Only PTE encoding and walk depth differ.
//!
//! # Safety Invariants — COW Page Table Protocol (INV-COW-*)
//!
//! - **INV-COW-1**: When the COW bit is set in a PTE, the PTE `W` (RW) bit
//!   must be clear. The page is shared read-only.
//! - **INV-COW-2**: When the COW bit is set, the page's `_refcount >= 2`
//!   (shared by at least two mappings, e.g. parent + child after fork).
//! - **INV-COW-3**: On a COW fault, if `_refcount == 1` (all other sharers
//!   have released), restore the `W` bit directly without copying.
//! - **INV-COW-4**: On a COW fault, if `_refcount > 1`, allocate a new page,
//!   copy contents, decrement the old page's refcount, and map the new page
//!   with `W=1, COW=0`.
//! - **INV-COW-5**: After `fork`, every writable PTE in the child must be
//!   downgraded to read-only (`W=0`) with `COW=1` set, and the page's
//!   refcount incremented.

use core::sync::atomic::{fence, Ordering};

extern crate alloc;
use alloc::vec::Vec;

use super::memory_layout::*;
use super::mmu_init::*;
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaType};
use crate::mm::{MmStruct, alloc_pages, GfpFlags};

// Re-export AddressSpace for backward compatibility
pub use crate::mm::AddressSpace;

// ==================== MmStruct Extension Methods ====================

impl MmStruct {
    /// Enable this address space (switch CR3 to this root)
    ///
    /// Loading CR3 itself flushes the non-global TLB (PCID off).
    pub unsafe fn enable(&self) {
        crate::arch::cpu::write_cr3(self.pgd << PAGE_SHIFT);
    }

    /// Disable address space (switch back to the kernel root)
    ///
    /// x86 has no bare mode: "disabled" means the static kernel PML4.
    pub unsafe fn disable() {
        crate::arch::cpu::write_cr3(get_kernel_page_table_ppn() << PAGE_SHIFT);
    }

    /// Flush entire TLB (reload CR3)
    pub unsafe fn flush_tlb() {
        crate::arch::mm::asid::flush_tlb_all();
    }

    /// Flush TLB for specified page (invlpg)
    pub unsafe fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        crate::arch::cpu::invlpg(vaddr.as_usize() as u64);
    }

    // ==================== VMA Operations ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let _ = perm; // demand paging: PTEs come from faults (perm kept in the VMA)
        let mut vma_mgr = self.vma_write();

        let start = vma.start();
        let end = vma.end();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Update virtual memory statistics
        let size = end.as_usize().saturating_sub(start.as_usize());
        let pages = (size / PAGE_SIZE_USIZE) as u64;
        self.add_total_vm(pages);
        self.update_highest_vm_end(end.as_usize());

        Ok(())
    }

    /// Map single page (for lazy mapping/page fault handling)
    pub fn map_single_page(&self, virt_addr: VirtAddr, perm: Perm) -> Result<(), MapError> {
        let phys_addr = alloc_user_phys_page().ok_or(MapError::OutOfMemory)? as usize;
        let flags = perm_to_flags(perm, self.space_type());

        // SAFETY: phys_addr was just allocated by alloc_user_phys_page(), so the page is
        // exclusively owned. phys_to_virt produces a valid kernel-virtual address for it.
        unsafe {
            let ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(ptr.bits() as *mut u8, 0, PAGE_SIZE_USIZE);
            core::sync::atomic::compiler_fence(Ordering::Release);
        }

        // SAFETY: self.pgd is a valid root page-table PPN, virt_addr is page-aligned, and
        // phys_addr points to a freshly allocated (and zeroed) page that we exclusively own.
        unsafe {
            map_page(
                self.pgd,
                virt_addr,
                PhysAddr::new(phys_addr as u64),
                flags,
            );
        }

        // SAFETY: phys_addr is a valid page from the allocator; pfn_to_page_mut returns a valid
        // pointer (or null). Null check guards the dereference. No aliasing: the page was just
        // allocated and is not yet shared.
        unsafe {
            use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
            let page = pfn_to_page_mut(phys_addr / (PAGE_SIZE as usize));
            if !page.is_null() {
                (*page).set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU membership: make the page reclaimable
                // via swap-out (vmscan scans LRU_INACTIVE_ANON only).
                (*page).set_flag(PageFlag::SwapBacked);
                (*page).set_index(virt_addr.bits() as usize / (PAGE_SIZE as usize));
                (*page).inc_mapcount();
                crate::mm::rmap::page_record_mapping(
                    &*page,
                    self as *const _ as usize,
                    virt_addr.bits() as usize,
                );
                crate::mm::lru::page_add_anon_lru(&*page);
            }
        }

        Ok(())
    }

    /// Unmap VMA (requires write lock)
    pub fn unmap_vma(&self, start: PageVirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let _vma = vma_mgr.find(start).ok_or(MapError::NotMapped)?;
        let _ = vma_mgr.remove(start);
        Ok(())
    }

    /// Adjust heap pointer (brk system call)
    pub fn set_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        use user_addr::{HEAP_START, HEAP_MAX_SIZE, BRK_DEFAULT, MMAP_START};

        if new_brk.as_usize() == 0 {
            return Ok(self.brk());
        }

        if self.space_type() != PageTableType::User {
            return Err(MapError::Invalid);
        }

        let heap_end = BRK_DEFAULT + HEAP_MAX_SIZE;

        if new_brk.as_usize() < HEAP_START || new_brk.as_usize() > heap_end.min(MMAP_START) {
            return Ok(self.brk());
        }

        let old_brk = self.brk().as_usize();

        if new_brk.as_usize() < old_brk {
            self.set_brk_val(new_brk.as_usize());
            return Ok(new_brk);
        }

        if new_brk.as_usize() > old_brk {
            let old_brk_aligned = old_brk & !(PAGE_SIZE_USIZE - 1);
            let new_brk_aligned = new_brk.as_usize() & !(PAGE_SIZE_USIZE - 1);

            let mut addr = old_brk_aligned;
            while addr < new_brk_aligned {
                // SAFETY: self.pgd is a valid root PPN and addr is page-aligned within the user
                // heap region which is mapped by this address space.
                if unsafe { PageTableWalker::walk(self.pgd, addr as u64) }.is_none() {
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, self.space_type());
                    // Serialize the leaf-PTE write + rmap setup against a
                    // concurrent fork()/COW walk on this mm — brk was the
                    // one leaf-PTE writer left outside PTE_MODIFY_LOCK
                    // (same protocol as the demand-fault paths in
                    // page_fault.rs).
                    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
                    // Re-check under the lock: a racing thread of this mm
                    // (CLONE_VM) may have mapped this address between the
                    // walk above and here; mapping again would orphan the
                    // winner's page.
                    if unsafe { PageTableWalker::walk(self.pgd, addr as u64) }.is_some() {
                        drop(_pte_guard);
                        crate::mm::page_alloc::free_page(phys_addr);
                    } else {
                        // SAFETY: self.pgd is a valid root PPN, addr is page-aligned in the heap
                        // region, and phys_addr is a freshly allocated exclusive page.
                        unsafe {
                            map_page(
                                self.pgd,
                                VirtAddr::new(addr as u64),
                                PhysAddr::new(phys_addr as u64),
                                flags,
                            );
                        }
                        // Set up reverse mapping for heap page
                        {
                            use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
                            let page = pfn_to_page_mut(phys_addr / (PAGE_SIZE as usize));
                            if !page.is_null() {
                                // SAFETY: phys_addr is freshly allocated and exclusively owned; the null
                                // check ensures page is valid before dereference.
                                unsafe {
                                    (*page).set_flag(PageFlag::Anonymous);
                                    // SwapBacked + LRU membership: heap pages
                                    // are swap-out candidates too (vmscan).
                                    (*page).set_flag(PageFlag::SwapBacked);
                                    (*page).set_index(addr / (PAGE_SIZE as usize));
                                    (*page).inc_mapcount();
                                    crate::mm::rmap::page_record_mapping(
                                        &*page,
                                        self as *const _ as usize,
                                        addr,
                                    );
                                    crate::mm::lru::page_add_anon_lru(&*page);
                                }
                            }
                        }
                        drop(_pte_guard);
                    }

                    let mut vma_mgr = self.vma_write();
                    let mut vma_flags = VmaFlags::new();
                    vma_flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSUP);
                    let vma = Vma::new(
                        PageVirtAddr::new(addr),
                        PageVirtAddr::new(addr + PAGE_SIZE_USIZE),
                        vma_flags,
                    );
                    let _ = vma_mgr.add(vma);
                }
                addr += PAGE_SIZE_USIZE;
            }

            self.set_brk_val(new_brk.as_usize());
        }

        Ok(new_brk)
    }

    /// mmap system call implementation
    pub fn mmap(
        &self,
        addr: PageVirtAddr,
        size: usize,
        flags: VmaFlags,
        vma_type: VmaType,
        perm: Perm,
        map_flags: u32,
    ) -> Result<PageVirtAddr, MapError> {
        use super::memory_layout::map;

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        // MAP_FIXED_NOREPLACE implies exact placement like MAP_FIXED (the
        // EEXIST overlap check happens in sys_mmap before we get here).
        let is_fixed = map_flags
            & (map::MAP_FIXED | map::MAP_FIXED_NOREPLACE)
            != 0;

        use user_addr::BRK_DEFAULT;
        use user_addr::MMAP_START;
        let end_addr = addr.as_usize() + aligned_size;
        let has_brk_conflict = addr.as_usize() < MMAP_START && end_addr > BRK_DEFAULT;

        let start = if is_fixed {
            let start = addr;
            if start.as_usize() % PAGE_SIZE_USIZE != 0 {
                return Err(MapError::Invalid);
            }
            if start.as_usize() < user_addr::USER_START {
                return Err(MapError::Invalid);
            }
            // Upper bound: a fixed mapping at or above USER_END would write
            // leaf PTEs into the kernel-shared upper tables (user page
            // tables copy the kernel PML4 entries) — reject outright.
            if start
                .as_usize()
                .checked_add(aligned_size)
                .map_or(true, |e| e > user_addr::USER_END)
            {
                return Err(MapError::Invalid);
            }
            if has_brk_conflict {
                return Err(MapError::Invalid);
            }
            start
        } else if addr.as_usize() == 0 {
            self.find_free_area(aligned_size)?
        } else {
            let end = PageVirtAddr::new(addr.as_usize() + aligned_size);
            let test_vma = Vma::new(addr, end, flags);

            let vma_mgr = self.vma_read();
            let has_vma_conflict = vma_mgr.iter().any(|v| v.overlaps(&test_vma));
            drop(vma_mgr);

            let has_brk_conflict = addr.as_usize() < MMAP_START && addr.as_usize() >= BRK_DEFAULT;

            if has_vma_conflict || has_brk_conflict {
                self.find_free_area(aligned_size)?
            } else {
                addr
            }
        };

        if is_fixed {
            // Linux semantics: MAP_FIXED replaces only the covered range.
            // Overlapping VMAs are SPLIT — their non-overlapping head/tail
            // must survive with type/flags/fd/offset preserved. Removing
            // whole overlapping VMAs destroyed the rest of the span when a
            // dynamic linker mapped a DSO's whole range and then overlaid
            // the individual segments with MAP_FIXED (glibc
            // _dl_map_object_from_fd): every page outside the final
            // segment lost its VMA and faulted with SIGSEGV (Ubuntu
            // dash/libc load).
            let fixed_end = PageVirtAddr::new(start.as_usize() + aligned_size);
            let test_vma = Vma::new(start, fixed_end, flags);

            // Snapshot full-attribute VMAs that overlap the fixed range,
            // along with their pinned backing files (the pin is keyed by
            // VMA start; split pieces must re-pin or lose the file when
            // the mapping fd is closed).
            let overlapping: Vec<(Vma, Option<alloc::sync::Arc<crate::fs::file::File>>)> = {
                let vma_mgr = self.vma_read();
                vma_mgr.iter()
                    .filter(|v| v.overlaps(&test_vma))
                    .map(|v| (v.clone(), self.get_vma_file(v.start().as_usize())))
                    .collect()
            };

            // Compute the preserved head/tail pieces.
            let mut kept: Vec<(Vma, Option<alloc::sync::Arc<crate::fs::file::File>>)> = Vec::new();
            for (vma, file) in overlapping.iter() {
                if vma.start().as_usize() < start.as_usize() {
                    if let Some((head, _)) =
                        vma.split(PageVirtAddr::new(start.as_usize()))
                    {
                        kept.push((head, file.clone()));
                    }
                }
                if vma.end().as_usize() > fixed_end.as_usize() {
                    let mut tail =
                        Vma::new(fixed_end, vma.end(), vma.flags());
                    tail.set_type(vma.vma_type());
                    tail.set_file_fd(vma.file_fd());
                    tail.set_file_size(vma.file_size());
                    tail.set_offset(
                        vma.offset() + (fixed_end.as_usize() - vma.start().as_usize()),
                    );
                    kept.push((tail, file.clone()));
                }
            }

            // Remove the overlapped VMAs, then re-add the preserved pieces.
            {
                let mut vma_mgr = self.vma_write();
                let overlap_starts: Vec<PageVirtAddr> = vma_mgr
                    .iter()
                    .filter(|v| v.overlaps(&test_vma))
                    .map(|v| v.start())
                    .collect();
                for vma_start in overlap_starts {
                    let _ = vma_mgr.remove(vma_start);
                    self.unpin_vma_file(vma_start.as_usize());
                }
                drop(vma_mgr);
                for (piece, file) in kept {
                    let _ = self.vma_write().add(piece.clone());
                    if let Some(f) = file {
                        self.pin_vma_file(piece.start().as_usize(), f);
                    }
                }
            }

            // Route through unmap_pages so the teardown (walk, rmap,
            // refcount, clear_pte, TLB flush) happens under PTE_MODIFY_LOCK —
            // the previous inline copy raced fork's table walk (round 6
            // HIGH: MAP_FIXED PTE teardown bypassed the lock).
            self.unmap_pages(start, aligned_size)?;
        }

        let end = PageVirtAddr::new(start.as_usize() + aligned_size);
        let mut vma = Vma::new(start, end, flags);
        vma.set_type(vma_type);
        self.map_vma(vma, perm)?;
        Ok(start)
    }

    /// Find free virtual address area
    pub fn find_free_area(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        use user_addr::{MMAP_START, MMAP_END, USER_END};

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let vma_mgr = self.vma_read();

        let mut search_start = MMAP_START;
        let search_end = MMAP_END.min(USER_END.saturating_sub(aligned_size));

        for vma in vma_mgr.iter() {
            let vma_start = vma.start().as_usize();

            if vma_start > search_start {
                let gap_size = vma_start - search_start;
                if gap_size >= aligned_size {
                    return Ok(PageVirtAddr::new(search_start));
                }
            }

            if vma.end().as_usize() > search_start {
                search_start = (vma.end().as_usize() + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
            }

            if search_start > search_end {
                break;
            }
        }

        if search_start <= search_end && (search_end - search_start) >= aligned_size {
            return Ok(PageVirtAddr::new(search_start));
        }

        Err(MapError::OutOfMemory)
    }

    /// munmap system call implementation — supports partial VMA removal:
    /// split VMAs at the unmapped boundaries, remove the overlapping parts.
    pub fn munmap(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        // R7-1: user-range bound — a kernel-range munmap would walk the
        // SHARED kernel PML4 entries, put_page/rmap kernel page descriptors
        // and zero shared kernel PTEs (NEW-C1 class; covers sys_munmap and
        // madvise(MADV_REMOVE), which both funnel through here).
        {
            use super::memory_layout::user_addr;
            let end_checked = match addr.as_usize().checked_add(aligned_size) {
                Some(e) => e,
                None => return Err(MapError::Invalid),
            };
            if addr.as_usize() < user_addr::USER_START || end_checked > user_addr::USER_END {
                return Err(MapError::Invalid);
            }
        }

        let end_addr = addr.as_usize() + aligned_size;

        // VMA surgery: collect AND apply under ONE vma_write guard — two
        // critical sections let a same-mm mmap/MAP_FIXED install into the
        // gap (VMA over unmapped PTEs, or stale keys removing a re-created
        // VMA). VmaManager::add is a plain method; no recursion.
        {
            let mut vma_mgr = self.vma_write();
            let mut rm = Vec::new();
            let mut add = Vec::new();
            for vma in vma_mgr.iter() {
                let vs = vma.start().as_usize();
                let ve = vma.end().as_usize();
                if ve <= addr.as_usize() || vs >= end_addr {
                    continue;
                }
                rm.push(vma.start());
                if vs < addr.as_usize() {
                    let mut head = Vma::new(
                        crate::mm::page::VirtAddr::new(vs),
                        crate::mm::page::VirtAddr::new(addr.as_usize()),
                        vma.flags(),
                    );
                    head.set_type(vma.vma_type());
                    head.set_file_fd(vma.file_fd());
                    head.set_file_size(vma.file_size());
                    head.set_offset(vma.offset());
                    add.push(head);
                }
                if ve > end_addr {
                    let mut tail = Vma::new(
                        crate::mm::page::VirtAddr::new(end_addr),
                        crate::mm::page::VirtAddr::new(ve),
                        vma.flags(),
                    );
                    tail.set_type(vma.vma_type());
                    tail.set_file_fd(vma.file_fd());
                    tail.set_file_size(vma.file_size());
                    tail.set_offset(vma.offset() + (end_addr - vs));
                    add.push(tail);
                }
            }
            for start in &rm {
                let _ = vma_mgr.remove(*start);
            }
            for vma in &add {
                let _ = vma_mgr.add(vma.clone());
            }
        }

        self.unmap_pages(addr, aligned_size)?;

        Ok(())
    }

    /// Zap the mapped pages in [start, start+size) WITHOUT touching the
    /// VMAs — madvise(MADV_DONTNEED) semantics. Reuses unmap_pages so the
    /// walk/rmap/refcount/clear_pte/TLB teardown happens under
    /// PTE_MODIFY_LOCK.
    pub fn zap_page_range(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if start.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }
        if start.as_usize().checked_add(aligned_size).is_none() {
            return Err(MapError::Invalid);
        }
        self.unmap_pages(start, aligned_size)
    }

    /// Unmap physical pages in specified range
    fn unmap_pages(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // Serialize against fork's table walk and COW faults (PTE lock).
        let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
        let mut addr = start.as_usize();
        let end = addr.saturating_add(size);

        while addr < end {
            // SAFETY: self.pgd is a valid root PPN and addr is page-aligned within the range
            // being unmapped from this address space.
            let ppn = unsafe { PageTableWalker::walk(self.pgd, addr as u64) };

            // Swap entry (P=0 leaf): no resident page — free the swap
            // slot and clear the PTE. Without this, munmap of a swapped
            // page leaks the slot for the lifetime of the swap area.
            if ppn.is_none() {
                if let Some(raw) = super::page_fault::read_pte_raw(
                    self.pgd,
                    VirtAddr::new(addr as u64),
                ) {
                    if crate::mm::swap::is_swap_entry(raw) {
                        crate::mm::swap::swap_free_slot(
                            crate::mm::swap::swap_entry_type(raw),
                            crate::mm::swap::swap_entry_offset(raw),
                        );
                        // SAFETY: addr is a page-aligned user address whose
                        // PTE we just verified above.
                        unsafe { self.clear_pte(addr as u64); }
                    }
                }
                addr += PAGE_SIZE_USIZE;
                continue;
            }

            if let Some((ppn_val, pte_bits)) = ppn {
                // E8-MM: a valid leaf WITHOUT the U bit is a KERNEL
                // translation the user VA range overlaps (identity device
                // windows cloned into every root, or one RESTORED by
                // clear_pte after the user mapping over it was unmapped).
                // It is not this mm's page: no rmap entry, no refcount, no
                // PTE to clear — freeing or punching it corrupts kernel
                // MMIO access (also closes the pre-existing hole where
                // munmap over an untouched cloned MMIO page zeroed the
                // kernel's PTE).
                if pte_bits & PageTableEntry::U == 0 {
                    addr += PAGE_SIZE_USIZE;
                    continue;
                }
                // Remove reverse mapping before clearing PTE, then drop this
                // mapping's reference; the last reference frees the page.
                use crate::mm::page_desc::pfn_to_page_mut;
                let page = pfn_to_page_mut(ppn_val as usize);
                if !page.is_null() {
                    // Multi-mapping bookkeeping: drop this (mm, vpn) from
                    // the page's alternate rmap slots.
                    unsafe {
                        (*page).rmap_alt_remove(
                            self as *const _ as usize,
                            addr / PAGE_SIZE_USIZE,
                        );
                    }
                    if unsafe { (*page).is_mapped() } {
                        // SAFETY: page is non-null (checked above) and points to a valid page
                        // descriptor for a mapped page in this address space.
                        crate::mm::rmap::page_remove_rmap(unsafe { &*page });
                    }
                    // SAFETY: page descriptor for a page mapped by this address space.
                    let new_ref = unsafe { (*page).put_page() };
                    if new_ref == 0 {
                        crate::mm::page_alloc::free_pages(
                            (ppn_val as usize) << PAGE_SHIFT, 0,
                        );
                    }
                }
                // SAFETY: addr is a valid, page-aligned virtual address in this address space,
                // and its reverse mapping has just been removed above.
                unsafe {
                    self.clear_pte(addr as u64);
                }
                // RSS accounting: one resident page leaves this address space.
                self.sub_rss(1);
            }

            addr += PAGE_SIZE_USIZE;
        }

        // Remote shootdown: other CPUs may still hold cached translations
        // of the pages just unmapped — a stale hit reads/writes freed
        // frames. One broadcast per batch unmap (local-only during x86
        // single-CPU bring-up; the call keeps shape parity).
        crate::arch::ipi::flush_tlb_others(
            start.as_usize() as u64,
            end as u64,
        );
        // Full TLB flush (CR3 reload); required after clearing PTEs so
        // subsequent accesses use the updated page table.
        crate::arch::mm::asid::flush_tlb_all();

        Ok(())
    }

    /// Clear page table entry at specified virtual address (4-level walk)
    unsafe fn clear_pte(&self, virt: u64) {
        let a = VirtAddr::new(virt);
        let vpn4 = a.pgd_index() as usize;
        let vpn3 = a.pud_index() as usize;
        let vpn2 = a.pmd_index() as usize;
        let vpn1 = a.pte_index() as usize;

        let root_table = get_page_table_virt(self.pgd << PAGE_SHIFT);

        let pte4 = (*root_table).get(vpn4);
        if !pte4.is_valid() {
            return;
        }

        let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
        let pte3 = (*table3).get(vpn3);
        // A 1GB leaf means no user page was ever mapped at this VA (user
        // maps over a leaf demote it first) — nothing to clear, and
        // treating the leaf's frame as a table would write into device
        // memory.
        if !pte3.is_valid() || pte3.is_leaf() {
            return;
        }

        let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
        let pte2 = (*table2).get(vpn2);
        // Same rule for a still-megapaged 2MB leaf.
        if !pte2.is_valid() || pte2.is_leaf() {
            return;
        }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);

        // E8-MM: if this VA sits inside a kernel identity device window
        // (ECAM/IOAPIC/LAPIC), the PTE being cleared may cover a device
        // page the user mapped over (leaf demotion made that possible —
        // Linux MAP_FIXED replace semantics). RESTORE the device
        // translation instead of writing 0: the kernel accesses these
        // registers from trap context on the CURRENT CR3 (LAPIC EOI in
        // IRQ entry), and a hole here is a ring-0 page fault — KERNPANIC.
        // Ordinary user VAs (the None case) clear to 0 as before.
        let new_bits = super::mmu_init::kernel_device_window_pte(virt).unwrap_or(0);
        (*table1).set(vpn1, PageTableEntry::from_bits(new_bits));
    }

    /// Rewrite the leaf-PTE permission flags for [start, start+size) in the
    /// USER portion of this address space, preserving each PTE's PPN.
    ///
    /// Used by the init/ELF loaders to tighten a one-shot RWX pre-map down
    /// to per-segment (W^X) permissions. `flags` holds the PTE flag bits
    /// (P/U/RW/NX/...); pages not currently mapped are left alone. Callers
    /// must ensure `end <= USER_END` (kernel range PTEs are shared and must
    /// never be rewritten from here).
    pub unsafe fn set_range_permissions(&self, start: u64, size: usize, flags: u64) {
        use super::memory_layout::user_addr;
        let aligned_start = start & !(PAGE_SIZE_USIZE as u64 - 1);
        let end = match aligned_start.checked_add(size as u64) {
            Some(e) => (e + PAGE_SIZE_USIZE as u64 - 1) & !(PAGE_SIZE_USIZE as u64 - 1),
            None => return,
        };
        if aligned_start < user_addr::USER_START as u64 || end > user_addr::USER_END as u64 {
            return; // refuse kernel-range rewrites
        }

        let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
        let mut va = aligned_start;
        while va < end {
            let a = VirtAddr::new(va);
            let vpn4 = a.pgd_index() as usize;
            let vpn3 = a.pud_index() as usize;
            let vpn2 = a.pmd_index() as usize;
            let vpn1 = a.pte_index() as usize;

            let root_table = get_page_table_virt(self.pgd << PAGE_SHIFT);
            let pte4 = (*root_table).get(vpn4);
            if !pte4.is_valid() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
            let pte3 = (*table3).get(vpn3);
            if !pte3.is_valid() || pte3.is_leaf() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
            let pte2 = (*table2).get(vpn2);
            if !pte2.is_valid() || pte2.is_leaf() {
                va += PAGE_SIZE_USIZE as u64;
                continue;
            }
            let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
            let old = (*table1).get(vpn1);
            if old.is_valid() {
                // Preserve the physical address ([51:12]), replace the
                // flag bits (low 12 + NX + software bits).
                let ppn_bits = old.bits() & PageTableEntry::PHYS_MASK_PUBLIC;
                (*table1).set(vpn1, PageTableEntry::from_bits(ppn_bits | flags));
            }
            va += PAGE_SIZE_USIZE as u64;
        }
        // SAFETY: full TLB flush so the tightened permissions take effect
        // immediately (CR3 reload).
        crate::arch::mm::asid::flush_tlb_all();
    }

    /// brk system call implementation (legacy interface)
    pub fn do_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        self.set_brk(new_brk)
    }

    /// Allocate stack space
    pub fn allocate_stack(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let stack_size = if size == 0 {
            user_addr::STACK_MAX_SIZE
        } else {
            size
        };
        let aligned_size = (stack_size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        let stack_top = PageVirtAddr::new(user_addr::STACK_TOP & !(PAGE_SIZE_USIZE - 1));
        let stack_start = PageVirtAddr::new(stack_top.as_usize() - aligned_size);

        let mut flags = VmaFlags::new();
        flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN);
        let vma = Vma::new(stack_start, stack_top, flags);
        self.map_vma(vma, Perm::ReadWrite)?;

        self.setup_stack(stack_top.as_usize(), stack_size);

        Ok(stack_top)
    }

    /// Copy address space using Copy-on-Write mechanism
    pub fn fork(&self) -> Result<MmStruct, MapError> {
        // COW-exempt VMA regions (MAP_SHARED / device mappings). fork's COW
        // walk historically write-protected EVERY user-writable PTE — in the
        // PARENT too. A MAP_SHARED mapping then COW-faulted on the first
        // post-fork store: the fault handler allocated a private copy and
        // every subsequent store landed there, never in the shared backing
        // (the fbterm framebuffer-freeze root cause on riscv64). Shared and
        // device VMAs must be inherited as-is: bump the refcount, keep the
        // write permission.
        let cow_exempt: Vec<(u64, u64)> = {
            let vma_mgr = self.vma_read();
            vma_mgr
                .iter()
                .filter(|vma| {
                    vma.flags().contains(crate::mm::vma::VmaFlags::SHARED)
                        || vma.vma_type() == crate::mm::vma::VmaType::Device
                })
                .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
                .collect()
        };

        // SAFETY: self.pgd is a valid root PPN for the current address space. The caller
        // (fork) guarantees the parent address space is fully initialized and consistent.
        let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
        let new_root_ppn = unsafe {
            copy_page_table_cow(self.pgd, &cow_exempt).ok_or(MapError::OutOfMemory)?
        };
        drop(_pte_guard);

        // SAFETY: new_root_ppn was just returned from copy_page_table_cow, so it points to
        // a valid, freshly allocated root page table. space_type and brk are valid by
        // construction from the current address space.
        let new_space = unsafe { MmStruct::new_shared(
            new_root_ppn,
            self.space_type(),
            self.brk(),
        ) };

        {
            // Snapshot VMA data under parent read-lock, then drop it before
            // acquiring child write-lock to avoid nested read-then-write deadlock.
            let vma_snapshot: Vec<(PageVirtAddr, PageVirtAddr, VmaFlags, VmaType, i32, u64, usize)> = {
                let vma_mgr = self.vma_read();
                vma_mgr.iter().map(|vma| {
                    (vma.start(), vma.end(), vma.flags(), vma.vma_type(), vma.file_fd(), vma.file_size(), vma.offset())
                }).collect()
            };
            // Parent read-lock is now dropped.

            if !vma_snapshot.is_empty() {
                let mut new_vma_mgr = new_space.vma_write();
                for (start, end, flags, vma_type, file_fd, file_size, offset) in vma_snapshot {
                    let mut new_vma = Vma::new(start, end, flags);
                    new_vma.set_type(vma_type);
                    new_vma.set_file_fd(file_fd);
                    new_vma.set_file_size(file_size);
                    // File-offset continuity across fork (P1: also needed by
                    // the exec-image-backed segment VMAs' fd fallback path).
                    new_vma.set_offset(offset);
                    // Clone the parent's pinned vm_file so the child's
                    // demand faults read the file even after fd close.
                    if vma_type == crate::mm::vma::VmaType::FileBacked {
                        if let Some(f) = self.get_vma_file(start.as_usize()) {
                            new_space.pin_vma_file(start.as_usize(), f);
                        }
                    }
                    // Increment nattch for shared memory attachments inherited by child
                    if vma_type == crate::mm::vma::VmaType::SharedMemory && file_fd >= 0 {
                        crate::ipc::sysv_shm::shm_attach_vma(file_fd);
                    }
                    let _ = new_vma_mgr.add(new_vma);
                }
            }
        }

        new_space.set_start_code(self.start_code());
        new_space.set_end_code(self.end_code());
        new_space.set_start_data(self.start_data());
        new_space.set_end_data(self.end_data());
        new_space.set_start_stack(self.start_stack());
        new_space.set_stack_limit(self.stack_limit());
        new_space.set_arg_start(self.arg_start());
        new_space.set_arg_end(self.arg_end());
        new_space.set_env_start(self.env_start());
        new_space.set_env_end(self.env_end());
        // Linux dup_mm copies mm->saved_auxv: a forked child that later
        // crashes dumps the auxv of the exec that built its image.
        {
            let auxv = self.saved_auxv();
            if !auxv.is_empty() {
                new_space.set_saved_auxv(&auxv);
            }
        }

        // RSS: the child's page tables map (COW-shared) every page the
        // parent had resident — inherit the count so OOM badness sees
        // forked hogs, not zero-rss newborns.
        new_space.add_rss(self.rss());

        Ok(new_space)
    }
}

/// Convert permission to page table flags.
///
/// x86 encoding: PRESENT implies read; RW gives write; exec is DEFAULT and
/// non-exec is expressed with NX (the mirror image of riscv, where R/X are
/// positive bits). The NX bit is therefore set unless the permission
/// includes exec.
fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
    let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D
        | PageTableEntry::NX;
    match perm {
        Perm::None => {
            // Present-readable, no write, no exec (riscv twin maps
            // PROT_NONE to V|R — keep the same observable behavior).
        }
        Perm::Read => {}
        Perm::ReadWrite => {
            flags |= PageTableEntry::W;
        }
        Perm::ReadWriteExec => {
            flags |= PageTableEntry::W;
            flags &= !PageTableEntry::NX;
        }
        Perm::ReadExec => {
            flags &= !PageTableEntry::NX;
        }
        Perm::Exec => {
            flags &= !PageTableEntry::NX;
        }
    }
    if space_type == PageTableType::User {
        flags |= PageTableEntry::U;
    }
    flags
}

// ==================== Page table walker ====================

/// 4-level page-table walker (same debug shape as riscv64: returns
/// (ppn, full_pte_bits); handles 2MB/1GB leaves).
pub struct PageTableWalker;

impl PageTableWalker {
    /// Walk to find the physical page for `virt`.
    ///
    /// # Safety
    /// `user_root_ppn` must be a live page-table root.
    pub unsafe fn walk(user_root_ppn: u64, virt: u64) -> Option<(u64, u64)> {
        let a = VirtAddr::new(virt);
        let pml4 = get_page_table_virt(user_root_ppn << PAGE_SHIFT);
        let mut table: *mut PageTable = pml4;

        // PML4 → PUD → PMD
        for level in (1..=3u8).rev() {
            let pte = (*table).get(a.vpn(level) as usize);
            if !pte.is_valid() {
                return None;
            }
            if level > 1 && pte.is_leaf() {
                // 1GB (PUD leaf) or 2MB (PMD leaf) huge page
                let shift = PAGE_SHIFT + 9 * (level as u64 - 1);
                let offset = virt & ((1 << shift) - 1);
                let phys = pte.phys_addr() + offset;
                return Some((phys >> PAGE_SHIFT, pte.bits()));
            }
            table = get_page_table_virt(pte.phys_addr());
        }

        let pte = (*table).get(a.vpn(0) as usize);
        if !pte.is_valid() {
            return None;
        }
        Some((pte.ppn(), pte.bits()))
    }
}

/// Allocate one user page from the zone allocator (physical address).
pub fn alloc_user_phys_page() -> Option<u64> {
    let addr = crate::mm::page_alloc::alloc_pages(crate::mm::zone::GfpFlags::GFP_KERNEL, 0);
    if addr == 0 { None } else { Some(addr as u64) }
}

// ==================== User Address Space Management ====================

/// Create user address space: fresh PML4 with the kernel half shared and
/// the low identity subtree cloned leaf-only (kernel trap-context device
/// access must work under a user CR3 — LAPIC EOI, ECAM probing).
pub fn create_user_address_space() -> Option<u64> {
    let phys_addr = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys_addr == 0 {
        return None;
    }

    // Validate physical address is within actual physical memory range.
    if crate::mm::layout::is_kernel_layout_initialized() {
        let layout = crate::mm::layout::kernel_layout();
        let phys_end = layout.phys_base + layout.phys_size;

        if phys_addr < layout.phys_base || phys_addr >= phys_end {
            // Invalid physical address - outside memory range
            return None;
        }
    }

    let root_page = phys_addr as u64;

    // Stamp the root into the PT ledger (it comes from alloc_pages, NOT
    // alloc_page_table, so it is otherwise unstamped and its teardown free
    // is refused — one leaked frame per exec'd mm).
    super::mmu_init::PT_LEDGER.stamp_root(root_page >> PAGE_SHIFT);

    // SAFETY: root_page is a freshly allocated physical page; get_page_table_virt returns a
    // valid kernel-virtual pointer to it. copy_kernel_mappings expects a valid root PPN for
    // a page-sized allocation that we exclusively own.
    unsafe {
        let root_table = get_page_table_virt(root_page);
        (*root_table).zero();

        // ALWAYS clone from the static kernel root — root_page_table_ppn()
        // reads CR3, which is a USER root while another task is current.
        let kernel_ppn = get_kernel_page_table_ppn();
        let root_ppn = root_page / PAGE_SIZE;
        copy_kernel_mappings(root_ppn, kernel_ppn);

        // Fixmap lives in the kernel half (PML4[511]) on x86 — already
        // shared by the link copy above; nothing to do (riscv64 needs the
        // explicit copy because its fixmap sits below KERNEL_PGD_START).
        super::fixmap::copy_fixmap_to_user(root_ppn);

        Some(root_ppn)
    }
}

/// Copy kernel mappings into a fresh user PML4.
///
/// 1. Kernel-space PML4 entries (256..512) are copied as LINKS — every
///    process shares the kernel's PUD/PMD/PT tables. Safe because the
///    teardown walk never descends into the kernel half and these tables
///    are never freed.
/// 2. The low identity subtree (PML4[0] — the device windows at
///    0xb0000000/0xfec00000/0xfee00000 the kernel touches from trap
///    context on a user CR3) is CLONED leaf-only: new PUD/PMD/PT tables
///    whose entries reproduce the kernel's leaf translations. Sharing the
///    underlying tables would let a user mmap write into the shared boot
///    tables; cloning means a MAP_FIXED over a window demotes THIS
///    process's private tables only (and clear_pte restores the device
///    translation on teardown).
unsafe fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    let kernel_phys = kernel_root_ppn * PAGE_SIZE;
    let user_phys = user_root_ppn * PAGE_SIZE;

    let kernel_virt = get_page_table_virt(kernel_phys);
    let user_virt = get_page_table_virt(user_phys);

    let kernel_table = kernel_virt as *const PageTable;
    let user_table = user_virt as *mut PageTable;

    (*user_table).zero();

    // ---- Low identity subtree: PML4[0] ----
    let pte0 = (*kernel_table).get(0);
    if pte0.is_valid() && !pte0.is_leaf() {
        let kernel_pud = get_page_table_virt(pte0.ppn() << PAGE_SHIFT) as *const PageTable;

        if let Some(new_pud_phys) = alloc_page_table() {
            let new_pud = get_page_table_virt(new_pud_phys) as *mut PageTable;
            (*new_pud).zero();

            for vpn3 in 0..512usize {
                let pte3 = (*kernel_pud).get(vpn3);
                if !pte3.is_valid() {
                    continue;
                }

                if pte3.is_leaf() {
                    // 1GB leaf (U=0) — safe to copy directly
                    (*new_pud).set(vpn3, pte3);
                    continue;
                }

                // Non-leaf: clone the PMD table, copying only entries that
                // are safe (leaves, or links whose PT we clone leaf-only).
                let kernel_pmd = get_page_table_virt(pte3.ppn() << PAGE_SHIFT) as *const PageTable;

                if let Some(new_pmd_phys) = alloc_page_table() {
                    let new_pmd = get_page_table_virt(new_pmd_phys) as *mut PageTable;
                    (*new_pmd).zero();

                    for vpn2 in 0..512usize {
                        let pte2 = (*kernel_pmd).get(vpn2);
                        if !pte2.is_valid() {
                            continue;
                        }

                        if pte2.is_leaf() {
                            // 2MB leaf (identity/device MMIO) — copy directly
                            (*new_pmd).set(vpn2, pte2);
                            continue;
                        }

                        // Non-leaf PMD entry: points to a kernel PT table.
                        // Must NOT share the kernel PT directly because
                        // free_user_page_tables() would free it on process
                        // exit. Clone a new PT and copy only non-user leaf
                        // entries (user entries belong to another process —
                        // there are none in the kernel identity subtree, but
                        // the check is cheap defense).
                        let kernel_pt = get_page_table_virt(pte2.ppn() << PAGE_SHIFT) as *const PageTable;

                        if let Some(new_pt_phys) = alloc_page_table() {
                            let new_pt = get_page_table_virt(new_pt_phys) as *mut PageTable;
                            (*new_pt).zero();

                            for vpn1 in 0..512usize {
                                let pte1 = (*kernel_pt).get(vpn1);
                                if pte1.is_valid() && !pte1.is_user() {
                                    (*new_pt).set(vpn1, pte1);
                                }
                            }

                            let new_pt_ppn = new_pt_phys >> PAGE_SHIFT;
                            (*new_pmd).set(vpn2, PageTableEntry::new_table(new_pt_ppn));
                        }
                    }

                    let new_pmd_ppn = new_pmd_phys >> PAGE_SHIFT;
                    (*new_pud).set(vpn3, PageTableEntry::new_table(new_pmd_ppn));
                }
            }

            let new_pud_ppn = new_pud_phys >> PAGE_SHIFT;
            (*user_table).set(0, PageTableEntry::new_table(new_pud_ppn));
        }
    }

    // ---- Kernel-space PML4 entries (256..512): shared links ----
    for i in KERNEL_PGD_START..PTRS_PER_PGD as usize {
        let pte = (*kernel_table).get(i);
        if pte.is_valid() {
            (*user_table).set(i, pte);
        }
    }

    fence(Ordering::SeqCst);
}

/// Map user page
pub unsafe fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) {
    map_page(user_root_ppn, user_virt, phys, flags);
}

/// Global PTE-modification lock (interface parity with riscv64)
///
/// Coarse leaf-PTE serialization: copy_page_table_cow (fork) mutates the
/// PARENT's PTEs while walking them; handle_cow_fault atomically swaps a
/// parent PTE; exec/exit teardown frees them. With no per-PTE locks, any
/// two of these racing on the same page produce stale refcounts and
/// dangling child PTEs — the NEW2 class. One writer at a time.
pub static PTE_MODIFY_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

/// Map a user region: `phys_start` pages mapped at `virt_start` in
/// lockstep over `size` bytes.
///
/// NOTE (interface deviation, deliberate): the bring-up stub had
/// `map_user_region(root, virt, size, phys: Option<PhysAddr>, flags) ->
/// bool`; every generic caller (exec.rs, syscall/memory.rs, io_uring)
/// compiles against the riscv64 shape used here instead. Signature
/// changed to match the compiled-against arch interface.
pub unsafe fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
    // Serialize against fork's table walk (PTE lock coverage).
    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
    let virt_end_checked = virt_start.checked_add(size);
    let virt_end_val = match virt_end_checked {
        Some(v) => v,
        None => panic!("map_user_region: virt_start + size overflow"),
    };

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);

    let mut virt = virt_start_addr.floor();
    let end = VirtAddr::new(virt_end_val).ceil();

    while virt.bits() < end.bits() {
        let offset = virt.bits() - virt_start_addr.bits();
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        map_page(user_root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}

/// Allocate and map user memory.
///
/// Serves the request with power-of-two buddy blocks capped at order 10
/// (4 MiB). A single block smaller than the request while the mapping
/// still covered the FULL size walked past the allocated block into
/// arbitrary neighbouring physical frames (riscv64 history). Chunking the
/// allocation keeps the mapped range inside pages we own.
pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    if size == 0 { return None; }
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let mut first_phys = 0u64;
    let mut mapped_pages = 0usize;

    while mapped_pages < page_count {
        let remain = page_count - mapped_pages;
        let order = if remain == 1 {
            0
        } else {
            (remain.next_power_of_two().trailing_zeros() as usize).min(10)
        };
        let block_pages = 1usize << order;
        // block_pages >= remain whenever remain <= 1024 (ceil-log2), so the
        // last chunk covers the rest exactly; bigger remain takes 1024-page
        // (4 MiB) blocks.
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);

        if phys_addr == 0 {
            // Keep the pre-failure all-or-nothing semantics: report
            // failure; sys_brk then leaves the break unchanged (chunks
            // mapped so far are zeroed anonymous pages that a retry
            // re-maps over).
            return None;
        }

        // Zero BEFORE mapping: the pages are reachable through the linear
        // map regardless. Mapping first exposed stale page contents (old
        // freed data) to user space for the duration of the memset — an
        // information leak window on every execve.
        let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
        core::ptr::write_bytes(
            virt_addr_ptr.bits() as *mut u8,
            0,
            block_pages * PAGE_SIZE as usize,
        );

        map_user_region(
            user_root_ppn,
            virt_addr + (mapped_pages * PAGE_SIZE as usize) as u64,
            phys_addr as u64,
            chunk_bytes as u64,
            flags,
        );

        // R7-C5: the rounded-up block allocated 2^order pages but the
        // mapping only covers chunk_pages — the unmapped excess has no
        // PTE, so no teardown path would ever free it. Free the excess as
        // order-0 pages right away; buddy coalescing rebuilds larger
        // blocks lazily.
        if block_pages > chunk_pages {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for pfn in (base_pfn + chunk_pages)..(base_pfn + block_pages) {
                crate::mm::page_alloc::free_pages(pfn << PAGE_SHIFT, 0);
            }
        }

        if mapped_pages == 0 {
            first_phys = phys_addr as u64;
        }
        mapped_pages += chunk_pages;
    }

    Some(first_phys)
}

/// Allocate and map to the kernel table (user-flagged)
pub unsafe fn alloc_and_map_to_kernel_table(
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    if size == 0 { return None; }

    // Single contiguous block only (see alloc_and_map_user_memory): a
    // request no buddy block can cover must FAIL, not map past the block
    // into frames the buddy still owns.
    let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(crate::mm::zone::MAX_ORDER);
    if (1usize << order) < page_count {
        return None;
    }
    let alloc_size = (1usize << order) * PAGE_SIZE as usize;

    let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);

    if phys_addr == 0 {
        return None;
    }

    let kernel_ppn = get_kernel_page_table_ppn();

    let user_flags = flags | PageTableEntry::U;

    // Zero BEFORE mapping (same leak-window fix as alloc_and_map_user_memory).
    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, alloc_size);

    map_user_region(kernel_ppn, virt_addr, phys_addr as u64, size, user_flags);

    // R7-C5: free the rounded-up block's unmapped excess at once. The
    // mapped prefix stays physically contiguous (exec writes via
    // phys_base + vaddr offset).
    {
        let block_pages = 1usize << order;
        if block_pages > page_count {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for pfn in (base_pfn + page_count)..(base_pfn + block_pages) {
                crate::mm::page_alloc::free_pages(pfn << PAGE_SHIFT, 0);
            }
        }
    }

    Some(phys_addr as u64)
}

/// Allocate and map to a user table (chunked, see
/// alloc_and_map_user_memory — the returned phys base is the FIRST block
/// only; callers that need to write the whole range must walk the page
/// tables, not assume phys contiguity).
pub unsafe fn alloc_and_map_to_user_table(
    user_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    if size == 0 { return None; }

    let user_flags = flags | PageTableEntry::U;

    let mut first_phys = 0u64;
    let mut mapped_pages = 0usize;
    while mapped_pages < page_count {
        let remain = page_count - mapped_pages;
        let order = if remain == 1 {
            0
        } else {
            (remain.next_power_of_two().trailing_zeros() as usize).min(crate::mm::zone::MAX_ORDER)
        };
        let block_pages = 1usize << order;
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);
        if phys_addr == 0 {
            return None; // all-or-nothing; the RAII guard frees partial tables
        }

        // Zero BEFORE mapping (same leak-window fix): the frames come from
        // the buddy and may carry a previous owner's data.
        let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
        core::ptr::write_bytes(
            virt_addr_ptr.bits() as *mut u8,
            0,
            block_pages * PAGE_SIZE as usize,
        );

        map_user_region(
            user_ppn,
            virt_addr + (mapped_pages * PAGE_SIZE as usize) as u64,
            phys_addr as u64,
            chunk_bytes as u64,
            user_flags,
        );

        // R7-C5: free the rounded-up block's unmapped excess at once.
        if block_pages > chunk_pages {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for pfn in (base_pfn + chunk_pages)..(base_pfn + block_pages) {
                crate::mm::page_alloc::free_pages(pfn << PAGE_SHIFT, 0);
            }
        }

        if mapped_pages == 0 {
            first_phys = phys_addr as u64;
        }
        mapped_pages += chunk_pages;
    }

    Some(first_phys)
}

// ==================== Copy-on-Write Support ====================

/// Copy-on-Write flags
///
/// COW marker uses x86 PTE software-available bit 9 (bit 8 is GLOBAL on
/// x86, unlike riscv where bit 8 was free).
pub mod cow_flags {
    pub const COW: u64 = 1 << 9;
}

/// Copy page table with COW marking (4-level).
///
/// Kernel mappings (PML4[256..512], links) are shared by copying PML4
/// entries. The low identity subtree (PML4[0], cloned per-mm at creation)
/// and all user mappings are walked: leaves with U=0 (kernel device
/// translations) are copied as-is; user-writable leaves are downgraded to
/// read-only + COW in BOTH parent and child (unless cow_exempt).
///
/// NOTE vs riscv64 twin: same discipline — the framebuffer physical-range
/// exemption keys on the invariant device-shared frame range, not the
/// (derived, fragile) VMA flags, so no fork can ever COW the scanout away
/// (see the twin's comment for the fb0 read-regression history).
pub unsafe fn copy_page_table_cow(
    parent_root_ppn: u64,
    cow_exempt: &[(u64, u64)],
) -> Option<u64> {
    // NOTE: the whole walk runs under the caller's (AddressSpace::fork)
    // PTE_MODIFY_LOCK, serializing it against demand faults, COW faults,
    // and exec/unmap teardown. Per-leaf PTL granularity (Linux-style) is
    // still a TODO for SMP scalability.
    use crate::mm::page_desc::pfn_to_page_mut;

    // Framebuffer frames (virtio-gpu scanout backing store) are
    // device-shared memory and must NEVER be COW-marked — mirror of the
    // riscv64 twin (see its long comment for the failure history).
    let fb_frame_range: Option<(u64, u64)> = crate::drivers::gpu::get_framebuffer_info()
        .map(|i| (i.addr >> 12, (i.addr + i.size as u64 + 0xFFF) >> 12));

    if parent_root_ppn == 0 {
        return None;
    }

    let child_root_phys = alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    let parent_root_phys = parent_root_ppn << PAGE_SHIFT;
    let parent_root = get_page_table_virt(parent_root_phys);
    let child_root = get_page_table_virt(child_root_phys);

    for vpn4 in 0..512usize {
        let pte4 = (*parent_root).get(vpn4);

        if !pte4.is_valid() {
            continue;
        }

        // Kernel half (PML4[256..512]): share the link directly. These
        // entries are never freed per-mm, so no refcount games are needed.
        if vpn4 >= KERNEL_PGD_START {
            (*child_root).set(vpn4, pte4);
            continue;
        }

        // User half (PML4[0..256], incl. the cloned identity subtree):
        // walk and copy with COW marking.
        let ppn3 = pte4.ppn();

        let child_pud_phys = alloc_page_table()?;
        let child_ppn3 = child_pud_phys >> PAGE_SHIFT;
        (*child_root).set(vpn4, PageTableEntry::new_table(child_ppn3));

        let parent_pud = get_page_table_virt(ppn3 << PAGE_SHIFT);
        let child_pud_ref = &mut *get_page_table_virt(child_pud_phys);

        for vpn3 in 0..512usize {
            let pte3 = (*parent_pud).get(vpn3);

            if !pte3.is_valid() {
                continue;
            }

            // 1GB leaf: kernel device/identity leaves (U=0) are shared
            // as-is; user 1GB leaves (never created by this port) bump
            // the refcount like any shared user page.
            if pte3.is_leaf() {
                if pte3.is_user() {
                    let page = pfn_to_page_mut(pte3.ppn() as usize);
                    if !page.is_null() {
                        (*page).get_page();
                        (*page).inc_mapcount();
                    }
                }
                (*child_pud_ref).set(vpn3, pte3);
                continue;
            }

            let ppn2 = pte3.ppn();

            let child_pmd_phys = alloc_page_table()?;
            let child_ppn2 = child_pmd_phys >> PAGE_SHIFT;
            (*child_pud_ref).set(vpn3, PageTableEntry::new_table(child_ppn2));

            let parent_pmd = get_page_table_virt(ppn2 << PAGE_SHIFT);
            let child_pmd_ref = &mut *get_page_table_virt(child_pmd_phys);

            for vpn2 in 0..512usize {
                let pte2 = (*parent_pmd).get(vpn2);

                if !pte2.is_valid() {
                    continue;
                }

                // 2MB leaf: share (device/identity leaves, or user huge
                // pages with a refcount bump).
                if pte2.is_leaf() {
                    if pte2.is_user() {
                        let page = pfn_to_page_mut(pte2.ppn() as usize);
                        if !page.is_null() {
                            (*page).get_page();
                            (*page).inc_mapcount();
                        }
                    }
                    (*child_pmd_ref).set(vpn2, pte2);
                    continue;
                }

                let ppn1 = pte2.ppn();

                let child_pt_phys = alloc_page_table()?;
                let child_ppn1 = child_pt_phys >> PAGE_SHIFT;
                (*child_pmd_ref).set(vpn2, PageTableEntry::new_table(child_ppn1));

                let parent_pt = get_page_table_virt(ppn1 << PAGE_SHIFT);
                let child_pt_ref = &mut *get_page_table_virt(child_pt_phys);

                for vpn1 in 0..512usize {
                    // The whole walk runs under AddressSpace::fork()'s
                    // PTE_MODIFY_LOCK, which serializes it against demand
                    // faults, COW faults, and teardown — no per-leaf
                    // locking here (the lock is not reentrant).
                    let pte1 = (*parent_pt).get(vpn1);

                    if !pte1.is_valid() {
                        continue;
                    }

                    let is_user = pte1.bits() & PageTableEntry::U != 0;
                    let is_writable = pte1.is_writable();

                    // COW exemption (see MmStruct::fork): MAP_SHARED and
                    // device VMA pages are inherited as-is —
                    // write-protecting them in the PARENT would COW-divert
                    // every later store (the fbterm framebuffer-freeze
                    // root cause on riscv64).
                    let leaf_va = ((vpn4 as u64) << 39)
                        | ((vpn3 as u64) << 30)
                        | ((vpn2 as u64) << 21)
                        | ((vpn1 as u64) << 12);
                    let cow_exempt_leaf = cow_exempt
                        .iter()
                        .any(|(s, e)| leaf_va >= *s && leaf_va < *e)
                        || fb_frame_range.map_or(false, |(lo, hi)| {
                            pte1.ppn() >= lo && pte1.ppn() < hi
                        });

                    let new_pte = if is_user {
                        let phys_ppn = pte1.ppn() as usize;
                        let page = pfn_to_page_mut(phys_ppn);

                        if !page.is_null() {
                            // Increment refcount for shared user pages (one ref per sharer)
                            (*page).get_page();
                            (*page).inc_mapcount();

                            if is_writable && !cow_exempt_leaf {
                                // COW: mark both parent and child PTEs as read-only
                                (*page).set_flag(crate::mm::page_desc::PageFlag::Cow);

                                let cow_pte_bits = pte1.bits() & !PageTableEntry::W | cow_flags::COW;

                                let parent_pt_mut = parent_pt as *mut PageTable;
                                (*parent_pt_mut).set(vpn1, PageTableEntry::from_bits(cow_pte_bits));

                                PageTableEntry::from_bits(cow_pte_bits)
                            } else {
                                pte1
                            }
                        } else {
                            pte1
                        }
                    } else {
                        pte1
                    };

                    (*child_pt_ref).set(vpn1, new_pte);
                    pte_install_log(
                        child_root_ppn,
                        leaf_va,
                        new_pte.ppn(),
                    );
                }
            }
        }
    }

    // SMP coherence: the walk above DOWNGRADED the parent's writable PTEs
    // to read-only+COW. A remote CPU's stale WRITABLE entries would bypass
    // COW and land in the frame now shared with the child (a silently LOST
    // private write), and stale reads resurrect pre-fork page contents.
    // PTE permission changes on a live mm require a TLB shootdown on ALL
    // CPUs (Linux flush_tlb_mm-style): IPI the peers (local-only during
    // x86 bring-up), full-flush locally.
    crate::arch::ipi::flush_tlb_others(0, 0);
    crate::arch::mm::asid::flush_tlb_all();

    Some(child_root_ppn)
}

/// Outcome of a COW fault resolution attempt.
///
/// Keeps the "PTE changed under us" race apart from a genuine allocation
/// failure — mapping both to OutOfMemory SIGKILLed race losers with the
/// allocator nearly full (the riscv64 "fake OOM layer 2" family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    /// COW resolved: PTE is writable again (exclusive fast path, or a
    /// private copy was installed and mapped).
    Resolved,
    /// The PTE changed under us between the lock-free COW check and the
    /// locked re-walk. Not an error: re-execute the faulting instruction
    /// (mirrors Linux do_wp_page()'s re-check under the PTL).
    Retry,
    /// Allocating the private copy failed — a genuine out-of-memory.
    OutOfMemory,
}

/// Handle copy-on-write page fault (4-level walk under PTE_MODIFY_LOCK)
pub unsafe fn handle_cow_fault(root_ppn: u64, fault_addr: VirtAddr) -> CowFaultResult {
    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
    use crate::mm::page_desc::pfn_to_page_mut;

    let virt_addr = fault_addr.bits();

    let a = VirtAddr::new(virt_addr);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);

    let pte4 = (*root_table).get(vpn4);
    if !pte4.is_valid() {
        // Walk failure at re-check time: the upper levels were restructured
        // (teardown/fork) after handle_mm_fault's lock-free COW check.
        return CowFaultResult::Retry;
    }

    let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
    let pte3 = (*table3).get(vpn3);
    if !pte3.is_valid() || pte3.is_leaf() {
        return CowFaultResult::Retry;
    }

    let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
    let pte2 = (*table2).get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return CowFaultResult::Retry;
    }

    let table1_phys = pte2.ppn() << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);

    let old_pte = (*table1).get(vpn1);
    if !old_pte.is_valid() {
        return CowFaultResult::Retry;
    }

    let old_bits = old_pte.bits();
    if old_bits & cow_flags::COW == 0 {
        // COW bit gone at re-check time: a sibling thread sharing this mm
        // broke the COW first (or the page was mprotected/remapped). The
        // winner's PTE (possibly a fresh private copy, RW) is already
        // installed for the whole mm — re-execute the store, do NOT kill.
        return CowFaultResult::Retry;
    }

    let old_ppn = old_pte.ppn();
    let old_page = pfn_to_page_mut(old_ppn as usize);

    let refcount = if !old_page.is_null() {
        (*old_page).refcount()
    } else {
        1
    };

    // If refcount <= 1, we're the only owner - just enable write.
    if refcount <= 1 {
        let new_pte = PageTableEntry::from_bits(
            (old_bits & !cow_flags::COW) | PageTableEntry::W | PageTableEntry::P
        );

        // the page is exclusively owned now — clear the descriptor flag
        // too, or stats/invariants see "COW set with refcount 1" (R7-C9).
        if !old_page.is_null() {
            (*old_page).clear_flag(crate::mm::page_desc::PageFlag::Cow);
        }

        (*table1).set(vpn1, new_pte);

        // SMP coherence: the fast-path PTE replacement (RO+COW → RW) must
        // be visible to EVERY CPU that may hold the old entry — the task
        // can migrate, and a remote stale entry turns the retry into a
        // permanent fault or serves stale page contents. Shoot the entry
        // down on all peers (local-only during x86 bring-up; the remote
        // handler takes no locks, so doing this under PTE_MODIFY_LOCK
        // cannot deadlock).
        crate::arch::ipi::flush_tlb_others(virt_addr, virt_addr + PAGE_SIZE);
        crate::arch::cpu::invlpg(virt_addr);

        return CowFaultResult::Resolved;
    }

    // NOTE: put_page() on the old page is deferred until AFTER the copy and
    // PTE update. Calling it earlier opens a race window: the child on
    // another CPU could see refcount drop to 1, re-enable W without
    // copying, then exec and free the page — all before this side finishes
    // copying from it.

    // Allocate new page and copy content
    let new_phys = match alloc_user_phys_page() {
        Some(p) => p,
        None => {
            return CowFaultResult::OutOfMemory;
        }
    };
    let new_ppn = new_phys >> PAGE_SHIFT;

    let new_virt = phys_to_virt(PhysAddr::new(new_phys));
    let old_virt = phys_to_virt(PhysAddr::new(old_ppn << PAGE_SHIFT));

    // Copy while old page is still pinned (refcount >= 2)
    core::ptr::copy_nonoverlapping(
        old_virt.bits() as *const u8,
        new_virt.bits() as *mut u8,
        PAGE_SIZE as usize
    );

    // Preserve V/U/G/A/D and the exec state (NX); add W; drop COW.
    let flags = (old_bits & (PageTableEntry::P | PageTableEntry::U | PageTableEntry::GLOBAL
        | PageTableEntry::ACCESSED | PageTableEntry::DIRTY | PageTableEntry::NX))
        | PageTableEntry::W;
    let new_pte = PageTableEntry::from_bits((new_ppn << PAGE_SHIFT) | flags);

    // Install new PTE before dropping our reference to the old page
    (*table1).set(vpn1, new_pte);

    // SMP coherence: this replacement (old frame → private copy) is exactly
    // the stale-entry hazard — without a shootdown, a CPU the task migrated
    // from keeps translating the VA onto the OLD frame, splitting the
    // task's memory view.
    crate::arch::ipi::flush_tlb_others(virt_addr, virt_addr + PAGE_SIZE);
    crate::arch::cpu::invlpg(virt_addr);

    // Now safe to release our share of the old page.
    // Both refcount and mapcount must be released: the mapping no longer
    // points to the old shared page. Without dec_mapcount the count leaks
    // across COW cycles and eventually corrupts page reclaim / rmap
    // decisions.
    if !old_page.is_null() {
        (*old_page).put_page();
        (*old_page).dec_mapcount();
    }

    // Set up reverse mapping for the new COW page
    {
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
        let new_page = pfn_to_page_mut(new_ppn as usize);
        if !new_page.is_null() {
            (*new_page).set_flag(PageFlag::Anonymous);
            // SwapBacked + LRU membership: the COW copy is an ordinary
            // anonymous page and must stay reclaimable (vmscan).
            (*new_page).set_flag(PageFlag::SwapBacked);
            (*new_page).set_index(virt_addr as usize / (PAGE_SIZE as usize));
            (*new_page).inc_mapcount();
        }
        // Multi-mapping bookkeeping + RSS: the copy is a new resident page
        // of the faulting address space (the old page remains resident only
        // for its other owners).
        if let Some(mm) = crate::sched::current().and_then(|t| t.address_space()) {
            if !new_page.is_null() {
                crate::mm::rmap::page_record_mapping(
                    &*new_page,
                    mm as *const _ as usize,
                    virt_addr as usize,
                );
                crate::mm::lru::page_add_anon_lru(&*new_page);
            }
            mm.add_rss(1);
        }
    }

    CowFaultResult::Resolved
}

/// Check if page is a COW page
pub unsafe fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let a = VirtAddr::new(addr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let pte4 = (*root_table).get(vpn4);

    if !pte4.is_valid() {
        return false;
    }

    // Leaf levels: a leaf (2MB/1GB) is never a COW PTE (COW only exists
    // on 4KB user leaves; the identity/device windows map as leaves).
    let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
    let pte3 = (*table3).get(vpn3);

    if !pte3.is_valid() || pte3.is_leaf() {
        return false;
    }

    let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
    let pte2 = (*table2).get(vpn2);

    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }

    let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return false;
    }

    (pte1.bits() & cow_flags::COW) != 0
}

/// Check if page has required permissions: (read, write, exec, user)
pub unsafe fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let a = VirtAddr::new(addr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let pte4 = (*root_table).get(vpn4);

    if !pte4.is_valid() {
        return None;
    }

    let table3 = get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
    let pte3 = (*table3).get(vpn3);

    if !pte3.is_valid() {
        return None;
    }
    // Superpage leaf (identity/device windows): report the LEAF's
    // permissions — following its PPN as a table pointer would read device
    // memory as PTEs.
    if pte3.is_leaf() {
        let bits = pte3.bits();
        return Some((
            (bits & PageTableEntry::P) != 0,
            (bits & PageTableEntry::W) != 0,
            (bits & PageTableEntry::NX) == 0,
            (bits & PageTableEntry::U) != 0,
        ));
    }

    let table2 = get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
    let pte2 = (*table2).get(vpn2);

    if !pte2.is_valid() {
        return None;
    }
    if pte2.is_leaf() {
        let bits = pte2.bits();
        return Some((
            (bits & PageTableEntry::P) != 0,
            (bits & PageTableEntry::W) != 0,
            (bits & PageTableEntry::NX) == 0,
            (bits & PageTableEntry::U) != 0,
        ));
    }

    let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return None;
    }

    let bits = pte1.bits();
    let has_read = (bits & PageTableEntry::P) != 0;
    let has_write = (bits & PageTableEntry::W) != 0;
    let has_exec = (bits & PageTableEntry::NX) == 0;
    let is_user = (bits & PageTableEntry::U) != 0;

    Some((has_read, has_write, has_exec, is_user))
}
