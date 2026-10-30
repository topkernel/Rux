//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 memory management operations — MmStruct extensions, user
//! mappings, COW.
//!
//! STAGE-2 PORT of the riscv64 twin (kernel/src/arch/riscv64/mm/mm_ops.rs)
//! onto the aarch64 4-level TTBR0 walker. Contracts mirror the twin:
//! INV-COW-1..5 hold verbatim; the divergences are structural and
//! documented at each site:
//! - The user root is a PURE TTBR0 tree — the kernel half lives in the
//!   separate TTBR1 root, so there is no kernel-subtree cloning
//!   (create_user_address_space), no shared-subtree skipping and no
//!   device-window leaf restoring (clear_pte) like the riscv64 twin's
//!   single-root model needs.
//! - Permission bits are Rux SOFTWARE bits (R/W/X/U/A/D mirrors in the
//!   descriptor's upper half, see pagetable.rs); descriptors are built
//!   ONLY through the pagetable.rs constructors.
//! - TLB maintenance goes through asid.rs (tlbi) instead of sfence.vma.
//! - The PCACHE page-cache-alias optimization of the twin is not yet
//!   ported: file faults take the twin's private-copy fallback arm
//!   (functionally correct, one copy per mapper). cow_flags::PCACHE
//!   keeps the bit budget ready for it — mprotect_pte_range and the
//!   PROT_NONE marker re-presentation (page_fault.rs) preserve the bit
//!   exactly like COW, so the port will not have to revisit them
//!   (requirements list in page_fault.rs's module doc).

use core::sync::atomic::Ordering;

extern crate alloc;
use alloc::vec::Vec;

use super::memory_layout::*;
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaOrigin, VmaType};
use crate::mm::{MmStruct, alloc_pages, GfpFlags};

// Re-export AddressSpace for backward compatibility
#[allow(unused_imports)]
pub use crate::mm::AddressSpace;

// ==================== Page Table Walker ====================
//
/// Read-only 4-level walker (interface parity with the riscv64
/// `PageTableWalker`): resolves a VA to its leaf `(ppn, pte_bits)`.
///
/// `root_ppn` is the table the walk STARTS at: user VAs walk a TTBR0
/// root, kernel-half VAs walk the TTBR1 (kernel) root — the caller picks
/// the root by address-space ownership the same way the riscv64 twin's
/// callers hand in `mm.pgd`.
///
/// Index shifts descend from the root (L0 = VA[47:39] ... L3 =
/// VA[20:12]): shift for table level `cur` is PAGE_SHIFT + 9*(3-cur).
/// A BLOCK leaf at L0/L1/L2 is offset-adjusted and returned as the 4KB
/// PPN it contains (riscv64 huge-page parity); an L3 PAGE leaf returns
/// its own PPN.
pub struct PageTableWalker;

impl PageTableWalker {
    /// Walk `root_ppn` for `vaddr`; returns the leaf frame PPN (4KB
    /// granule, block leaves adjusted) and the raw descriptor bits.
    ///
    /// # Safety
    /// root_ppn must be a valid page-table root PPN; the walker only
    /// READS table frames (converted through get_page_table_virt).
    pub unsafe fn walk(root_ppn: u64, vaddr: u64) -> Option<(u64, u64)> {
        // SAFETY: get_page_table_virt translates allocated table frames;
        // the walk dereferences only frames reachable from the root.
        unsafe {
            let mut table_phys = root_ppn << PAGE_SHIFT;
            for cur in 0..4u32 {
                let shift = PAGE_SHIFT + 9 * ((3 - cur) as u64);
                let idx = ((vaddr >> shift) & 0x1FF) as usize;
                let table = super::mmu_init::get_page_table_virt(table_phys);
                let entry = (*table).get(idx);
                if !entry.is_valid() {
                    return None;
                }
                if entry.is_leaf() || cur == 3 {
                    // Block leaf (L0-L2, 1GB/2MB) or L3 page: fold the
                    // in-page offset so the PPN is 4KB-granular.
                    let page_base = entry.output_addr() + (vaddr & ((1 << shift) - 1));
                    return Some((page_base >> PAGE_SHIFT, entry.bits()));
                }
                table_phys = entry.ppn() << PAGE_SHIFT;
            }
            None
        }
    }

    /// Walk and return the level the leaf was found at (0..=3)
    ///
    /// # Safety
    /// See walk().
    pub unsafe fn walk_level(root_ppn: u64, vaddr: u64) -> Option<(u64, u64, u8)> {
        // SAFETY: see walk().
        unsafe {
            let mut table_phys = root_ppn << PAGE_SHIFT;
            for cur in 0..4u32 {
                let shift = PAGE_SHIFT + 9 * ((3 - cur) as u64);
                let idx = ((vaddr >> shift) & 0x1FF) as usize;
                let table = super::mmu_init::get_page_table_virt(table_phys);
                let entry = (*table).get(idx);
                if !entry.is_valid() {
                    return None;
                }
                if entry.is_leaf() || cur == 3 {
                    let page_base = entry.output_addr() + (vaddr & ((1 << shift) - 1));
                    return Some((page_base >> PAGE_SHIFT, entry.bits(), cur as u8));
                }
                table_phys = entry.ppn() << PAGE_SHIFT;
            }
            None
        }
    }
}

// ==================== MmStruct Extension Methods ====================

impl MmStruct {
    /// Enable this address space (switch TTBR0 + ASID)
    ///
    /// # Safety
    /// pgd must be a valid user root PPN.
    pub unsafe fn enable(&self) {
        let satp = Satp::long(self.pgd, 0);
        super::asid::write_satp(satp.0 as usize);
    }

    /// Disable the address space (switch to the kernel root)
    ///
    /// # Safety
    /// Switches TTBR0 to the kernel root with a full flush.
    pub unsafe fn disable() {
        let kernel_ppn = super::mmu_init::root_page_table_ppn();
        super::asid::write_satp(super::asid::build_satp(0, kernel_ppn as usize));
    }

    /// Flush the entire TLB
    ///
    /// # Safety
    /// Callers must have repaired the affected translations.
    pub unsafe fn flush_tlb() {
        super::asid::flush_tlb_all();
    }

    /// Flush the TLB for one page
    ///
    /// # Safety
    /// See flush_tlb.
    pub unsafe fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        super::asid::flush_tlb_page(vaddr.as_usize(), 0);
    }

    // ==================== VMA Operations ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let _ = perm;
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

        // SAFETY: phys_addr was just allocated by alloc_user_phys_page(),
        // so the page is exclusively owned; phys_to_virt produces a valid
        // kernel-virtual address inside the linear map.
        unsafe {
            let ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(ptr.bits() as *mut u8, 0, PAGE_SIZE_USIZE);
            core::sync::atomic::compiler_fence(Ordering::Release);
        }

        // SAFETY: self.pgd is a valid root PPN, virt_addr is page-aligned,
        // and phys_addr is a freshly zeroed page we exclusively own.
        unsafe {
            map_user_page(
                self.pgd,
                virt_addr,
                PhysAddr::new(phys_addr as u64),
                flags,
            );
        }

        // SAFETY: pfn_to_page_mut returns the page descriptor (or null);
        // the null check guards the dereference. The page was just
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
    /// LEGACY/UNUSED: the live brk path is sys_brk (syscall/memory.rs,
    /// task.brk + VMA surgery + demand-fault pages); this eager-map
    /// variant has no callers (do_brk below is its only wrapper, also
    /// uncalled). Kept in sync with the mm.brk mirror for when an
    /// in-kernel caller reappears. (riscv64 twin note, carried over.)
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
                // SAFETY: self.pgd is a valid root PPN and addr is
                // page-aligned within the user heap region.
                if unsafe { PageTableWalker::walk(self.pgd, addr as u64) }.is_none() {
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, self.space_type());
                    // Serialize the leaf-PTE write + rmap setup against a
                    // concurrent fork()/COW walk on this mm (riscv64 twin
                    // protocol, INV-LOCK-7 GLOBAL mode).
                    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
                    // Re-check under the lock: a racing CLONE_VM thread
                    // may have mapped this address between the walk and
                    // here; mapping again would orphan the winner's page.
                    if unsafe { PageTableWalker::walk(self.pgd, addr as u64) }.is_some() {
                        drop(_pte_guard);
                        crate::mm::page_alloc::free_page(phys_addr);
                    } else {
                        // SAFETY: valid root, page-aligned heap address,
                        // exclusively owned fresh page.
                        unsafe {
                            if !map_user_page(
                                self.pgd,
                                VirtAddr::new(addr as u64),
                                PhysAddr::new(phys_addr as u64),
                                flags,
                            ) {
                                // PT allocation failure: free the data
                                // page (anti-leak) and leave the address
                                // unmapped — the demand-fault path
                                // delivers the OOM later instead of a
                                // kernel panic here.
                                static HEAPMAP_N: core::sync::atomic::AtomicUsize =
                                    core::sync::atomic::AtomicUsize::new(0);
                                if HEAPMAP_N
                                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                                    < 8
                                {
                                    crate::pr_err!(
                                        "brk eager map: PT alloc failed at {:#x} — page left for demand fault",
                                        addr
                                    );
                                }
                                drop(_pte_guard);
                                continue;
                            }
                        }
                        // Reverse mapping for the heap page
                        {
                            use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
                            let page = pfn_to_page_mut(phys_addr / (PAGE_SIZE as usize));
                            if !page.is_null() {
                                // SAFETY: freshly allocated, exclusively
                                // owned frame; null-checked descriptor.
                                unsafe {
                                    (*page).set_flag(PageFlag::Anonymous);
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
                    let mut vma = Vma::new(
                        PageVirtAddr::new(addr),
                        PageVirtAddr::new(addr + PAGE_SIZE_USIZE),
                        vma_flags,
                    );
                    vma.set_origin(VmaOrigin::from_bits(VmaOrigin::BRK));
                    let brk_page = vma.page_count() as u64;
                    let _ = vma_mgr.add(vma);
                    self.add_total_vm(brk_page);
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
        let is_fixed = map_flags & (map::MAP_FIXED | map::MAP_FIXED_NOREPLACE) != 0;

        let end_addr = addr.as_usize() + aligned_size;
        // Reference-kernel parity (riscv64 twin): MAP_FIXED replaces only
        // the covered range; refuse only the LIVE brk heap span
        // [align_down(start_brk), align(brk)) — mapping over malloc's own
        // storage stays EINVAL (documented divergence: we never silently
        // free brk-managed pages).
        let brk_cur = self.brk().as_usize();
        let heap_lo = self.start_brk() & !(PAGE_SIZE_USIZE - 1);
        let heap_hi = if brk_cur > self.start_brk() {
            (brk_cur + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1)
        } else {
            heap_lo
        };
        let has_brk_conflict = addr.as_usize() < heap_hi && end_addr > heap_lo;

        // Placement + VMA insert share one write-lock section (atomic
        // pick-and-insert; riscv64/x86_64 twin TOCTOU fix).
        let mut vma_installed = false;
        let start = if is_fixed {
            let start = addr;
            if start.as_usize() % PAGE_SIZE_USIZE != 0 {
                return Err(MapError::Invalid);
            }
            if start.as_usize() < user_addr::USER_START {
                return Err(MapError::Invalid);
            }
            // Upper bound: user TASK_SIZE (256GB) — every user mapping
            // must stay under TTBR0 L0 entry 0.
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
        } else {
            // ATOMIC find + insert under ONE vma_write guard; hint policy
            // mirrors the twins: a free hint anywhere in the user range is
            // honored, MIN_MAP_ADDR / live-brk-span / past-USER_END hints
            // are ignored (never an error).
            let mut vma_mgr = self.vma_write();
            let chosen = if addr.as_usize() != 0 {
                let hint = addr.as_usize() & !(PAGE_SIZE_USIZE - 1);
                let hint_end = hint.checked_add(aligned_size);
                let hint_heap_conflict =
                    hint < heap_hi && hint_end.map_or(true, |e| e > heap_lo);
                let hint_ok = hint >= user_addr::MIN_MAP_ADDR
                    && hint_end.map_or(false, |e| e <= user_addr::USER_END)
                    && !hint_heap_conflict;
                let mut use_hint = false;
                if hint_ok {
                    let test_vma = Vma::new(
                        PageVirtAddr::new(hint),
                        PageVirtAddr::new(hint + aligned_size),
                        flags,
                    );
                    let has_vma_conflict =
                        vma_mgr.iter().any(|v| v.overlaps(&test_vma));
                    use_hint = !has_vma_conflict;
                }
                if use_hint {
                    PageVirtAddr::new(hint)
                } else {
                    self.find_free_area_locked(&vma_mgr, aligned_size)?
                }
            } else {
                self.find_free_area_locked(&vma_mgr, aligned_size)?
            };
            let vend = PageVirtAddr::new(chosen.as_usize() + aligned_size);
            let mut vma = Vma::new(chosen, vend, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;
            drop(vma_mgr);
            self.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
            self.update_highest_vm_end(chosen.as_usize() + aligned_size);
            vma_installed = true;
            chosen
        };

        if is_fixed {
            // Reference-kernel semantics: MAP_FIXED replaces only the
            // covered range; overlapping VMAs are SPLIT with
            // type/flags/fd/offset preserved, and the whole surgery —
            // snapshot, remove, piece re-add, PTE zap, replacement
            // install — runs under ONE vma_write guard (twin ATOMICITY
            // note; unmap_pages takes no VMA lock, so the extended hold
            // is safe).
            let fixed_end = PageVirtAddr::new(start.as_usize() + aligned_size);
            let test_vma = Vma::new(start, fixed_end, flags);

            let mut vma_mgr = self.vma_write();

            let overlapping: Vec<(
                Vma,
                Option<alloc::sync::Arc<crate::fs::file::File>>,
            )> = vma_mgr
                .iter()
                .filter(|v| v.overlaps(&test_vma))
                .map(|v| (v.clone(), self.get_vma_file(v.start().as_usize())))
                .collect();

            let mut kept: Vec<(
                Vma,
                Option<alloc::sync::Arc<crate::fs::file::File>>,
            )> = Vec::new();
            for (vma, file) in overlapping.iter() {
                if vma.start().as_usize() < start.as_usize() {
                    if let Some((head, _)) =
                        vma.split(PageVirtAddr::new(start.as_usize()))
                    {
                        kept.push((head, file.clone()));
                    }
                }
                if vma.end().as_usize() > fixed_end.as_usize() {
                    let mut tail = Vma::new(fixed_end, vma.end(), vma.flags());
                    tail.set_type(vma.vma_type());
                    tail.set_origin(vma.origin());
                    tail.set_file_fd(vma.file_fd());
                    tail.set_file_size(vma.file_size());
                    tail.set_offset(
                        vma.offset() + (fixed_end.as_usize() - vma.start().as_usize()),
                    );
                    kept.push((tail, file.clone()));
                }
            }

            for (vma, _) in overlapping.iter() {
                // The replaced range leaves the mapping: release its
                // virtual-size charge (head/tail pieces stay charged).
                self.release_unmapped_pages(
                    vma.start().as_usize(),
                    vma.end().as_usize(),
                    start.as_usize(),
                    fixed_end.as_usize(),
                );
                let _ = vma_mgr.remove(vma.start());
                self.unpin_vma_file(vma.start().as_usize());
            }
            for (piece, file) in kept {
                let _ = vma_mgr.add(piece.clone());
                if let Some(f) = file {
                    self.pin_vma_file(piece.start().as_usize(), f);
                }
            }

            // PTE teardown under PTE_MODIFY_LOCK (unmap_pages).
            self.unmap_pages(start, aligned_size)?;

            // Install the replacement VMA under the SAME guard.
            let end = PageVirtAddr::new(start.as_usize() + aligned_size);
            let mut vma = Vma::new(start, end, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;
            drop(vma_mgr);
            self.add_total_vm((aligned_size / PAGE_SIZE_USIZE) as u64);
            self.update_highest_vm_end(end.as_usize());
            vma_installed = true;
        }

        if !vma_installed {
            // Unreachable today (both paths install inline); safety net
            // for future placement paths.
            let end = PageVirtAddr::new(start.as_usize() + aligned_size);
            let mut vma = Vma::new(start, end, flags);
            vma.set_type(vma_type);
            vma.set_origin(VmaOrigin::from_bits(VmaOrigin::MMAP));
            self.map_vma(vma, perm)?;
        }
        Ok(start)
    }

    /// Find free virtual address area
    pub fn find_free_area(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let vma_mgr = self.vma_read();
        self.find_free_area_locked(&vma_mgr, size)
    }

    /// Scan body of find_free_area under a lock the CALLER holds (read
    /// for the wrapper, write for mmap's atomic pick-and-insert).
    fn find_free_area_locked(
        &self,
        vma_mgr: &crate::mm::vma::VmaManager,
        size: usize,
    ) -> Result<PageVirtAddr, MapError> {
        use user_addr::{MMAP_START, MMAP_END, USER_END};

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let mut search_start = MMAP_START;
        let search_end = MMAP_END.min(USER_END - aligned_size);

        for vma in vma_mgr.iter() {
            let vma_start = vma.start().as_usize();

            if vma_start > search_start {
                let gap_size = vma_start - search_start;
                if gap_size >= aligned_size {
                    return Ok(PageVirtAddr::new(search_start));
                }
            }

            if vma.end().as_usize() > search_start {
                search_start =
                    (vma.end().as_usize() + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
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

    /// munmap system call implementation — partial-VMA removal with
    /// split at the unmapped boundaries (twin review ARCH-M3 / SYSA-H1).
    pub fn munmap(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // Overflow-safe page rounding (twin s15: user lengths within one
        // page of usize::MAX must EINVAL, not panic/wrap).
        let aligned_size = match size.checked_add(PAGE_SIZE_USIZE - 1) {
            Some(rounded) => rounded & !(PAGE_SIZE_USIZE - 1),
            None => return Err(MapError::Invalid),
        };

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        // User-range bound: a kernel-range munmap would walk the TTBR1
        // tree and free kernel page descriptors (twin R7-1).
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

        // VMA surgery AND PTE zap under ONE vma_write guard (twin
        // mm-audit: releasing the guard between the two exposes the range
        // as free while its PTEs are still live). Pin-table discipline:
        // a split FILE VMA's tail re-pins at its own start.
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
                self.release_unmapped_pages(vs, ve, addr.as_usize(), end_addr);
                let pinned = self.get_vma_file(vs);
                rm.push(vma.start());
                if vs < addr.as_usize() {
                    let mut head = Vma::new(
                        PageVirtAddr::new(vs),
                        PageVirtAddr::new(addr.as_usize()),
                        vma.flags(),
                    );
                    head.set_type(vma.vma_type());
                    head.set_origin(vma.origin());
                    head.set_file_fd(vma.file_fd());
                    head.set_file_size(vma.file_size());
                    head.set_offset(vma.offset());
                    add.push((head, pinned.clone()));
                }
                if ve > end_addr {
                    let mut tail = Vma::new(
                        PageVirtAddr::new(end_addr),
                        PageVirtAddr::new(ve),
                        vma.flags(),
                    );
                    tail.set_type(vma.vma_type());
                    tail.set_origin(vma.origin());
                    tail.set_file_fd(vma.file_fd());
                    tail.set_file_size(vma.file_size());
                    tail.set_offset(vma.offset() + (end_addr - vs));
                    add.push((tail, pinned.clone()));
                }
            }
            for start in &rm {
                let _ = vma_mgr.remove(*start);
                self.unpin_vma_file(start.as_usize());
            }
            for (vma, file) in &add {
                let _ = vma_mgr.add(vma.clone());
                if let Some(f) = file {
                    self.pin_vma_file(vma.start().as_usize(), f.clone());
                }
            }

            self.unmap_pages(addr, aligned_size)?;
        }

        Ok(())
    }

    /// Zap the mapped pages in [start, start+size) WITHOUT touching the
    /// VMAs — madvise(MADV_DONTNEED) semantics (twin contract).
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

    /// Unmap physical pages in specified range (PTE teardown under
    /// PTE_MODIFY_LOCK; twin protocol)
    fn unmap_pages(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // Serialize against fork's table walk and COW faults (PTE lock).
        let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
        let mut addr = start.as_usize();
        let end = addr + size;

        while addr < end {
            // SAFETY: self.pgd is a valid root PPN and addr is
            // page-aligned inside the range being unmapped.
            let leaf = unsafe { PageTableWalker::walk(self.pgd, addr as u64) };

            let Some((ppn_val, pte_bits)) = leaf else {
                addr += PAGE_SIZE_USIZE;
                continue;
            };

            // A valid leaf WITHOUT the U bit is a kernel translation the
            // user VA range overlaps (defensive: aarch64 user roots never
            // hold one today — the kernel half is TTBR1 — but the guard
            // is free). Not this mm's page: no rmap, no refcount, no PTE
            // rewrite.
            if pte_bits & PageTableEntry::U == 0 {
                addr += PAGE_SIZE_USIZE;
                continue;
            }
            // Device frames (virtio-gpu framebuffer) and other
            // boot-reserved/foreign frames are not RAM pages: never
            // put_page/free them (accounting corruption otherwise).
            {
                let phys = (ppn_val as u64) << 12;
                let fb = crate::drivers::gpu::get_framebuffer_info()
                    .map_or(false, |i| {
                        phys >= i.addr && phys < i.addr + i.size as u64
                    });
                if fb || crate::mm::page_alloc::is_foreign_user_frame(phys as usize) {
                    // SAFETY: verified present user leaf.
                    unsafe { self.clear_pte(addr as u64); }
                    addr += PAGE_SIZE_USIZE;
                    continue;
                }
            }
            // Remove reverse mapping before clearing the PTE, then drop
            // this mapping's reference; the last reference frees the page.
            {
                use crate::mm::page_desc::pfn_to_page_mut;
                let page = pfn_to_page_mut(ppn_val as usize);
                if !page.is_null() {
                    // SAFETY: null-checked descriptor of the leaf frame
                    // mapped by this address space at `addr`.
                    unsafe {
                        (*page).rmap_alt_remove(
                            self as *const _ as usize,
                            addr / PAGE_SIZE_USIZE,
                        );
                        if (*page).is_mapped() {
                            crate::mm::rmap::page_remove_rmap(&*page);
                        }
                        (*page).put_page_site(crate::dfx::driftwatch::S_UNMAP_RV);
                    }
                    // PCACHE mapping pin drop (kept for parity with the
                    // twin's filemap port; no aarch64 PCACHE producers
                    // yet, so this is dormant).
                    if pte_bits & cow_flags::PCACHE != 0 {
                        release_cache_mapping_pin(ppn_val as usize);
                    }
                }
            }
            // SAFETY: verified present user leaf.
            unsafe {
                self.clear_pte(addr as u64);
            }
            // RSS accounting: one resident page leaves this address space.
            self.sub_rss(1);

            addr += PAGE_SIZE_USIZE;
        }

        // Full local flush + remote shootdown (twin protocol: permission
        // and mapping changes on a live mm require all-PE invalidation).
        // SAFETY: translations were repaired above.
        unsafe {
            super::asid::flush_tlb_all();
        }
        crate::arch::ipi::flush_tlb_others();

        Ok(())
    }

    /// Teardown of ONE present user leaf at `va`, for callers that
    /// already hold PTE_MODIFY_LOCK (mremap's page-move copy; twin
    /// semantics): kernel-only leaves and device frames are left alone,
    /// a normal user leaf gets rmap removal, the put_page release and
    /// its PTE cleared. Returns true if a leaf was torn down.
    ///
    /// # Safety
    /// PTE_MODIFY_LOCK must be held.
    pub unsafe fn unmap_one_user_leaf_locked(&self, va: usize) -> bool {
        // SAFETY: self.pgd is a valid root; read-only walk.
        let Some((ppn_val, pte_bits)) =
            (unsafe { PageTableWalker::walk(self.pgd, va as u64) })
        else {
            return false;
        };
        if pte_bits & PageTableEntry::U == 0 {
            return false; // kernel translation overlapped in the walk
        }
        // Device/foreign frames: clear the PTE only.
        {
            let phys = (ppn_val as u64) << 12;
            let fb = crate::drivers::gpu::get_framebuffer_info()
                .map_or(false, |i| phys >= i.addr && phys < i.addr + i.size as u64);
            if fb || crate::mm::page_alloc::is_foreign_user_frame(phys as usize) {
                // SAFETY: verified present user leaf.
                unsafe { self.clear_pte(va as u64) };
                return true;
            }
        }
        use crate::mm::page_desc::pfn_to_page_mut;
        let page = pfn_to_page_mut(ppn_val as usize);
        if !page.is_null() {
            // SAFETY: null-checked descriptor of the leaf frame mapped
            // at `va` by this address space.
            unsafe {
                (*page).rmap_alt_remove(self as *const _ as usize, va / PAGE_SIZE_USIZE);
                if (*page).is_mapped() {
                    crate::mm::rmap::page_remove_rmap(&*page);
                }
                (*page).put_page_site(crate::dfx::driftwatch::S_UNMAP_RV);
            }
            if pte_bits & cow_flags::PCACHE != 0 {
                release_cache_mapping_pin(ppn_val as usize);
            }
        }
        // SAFETY: verified present user leaf.
        unsafe { self.clear_pte(va as u64) };
        self.sub_rss(1);
        true
    }

    /// Clear the L3 leaf descriptor at `va` (write an empty entry).
    ///
    /// aarch64 divergence: the riscv64 twin RESTORES a kernel
    /// device-window PTE here (user roots clone the kernel's low-half
    /// MMIO megapages); aarch64 user roots are pure TTBR0 trees with no
    /// cloned kernel translations, so the plain 0 clear is complete.
    ///
    /// # Safety
    /// Caller holds PTE_MODIFY_LOCK; `va` is a user address of this mm.
    unsafe fn clear_pte(&self, virt: u64) {
        // SAFETY: walk-only helper on a valid root.
        unsafe {
            let Some(l3) = super::mmu_init::user_l3_table_for(self.pgd, virt) else {
                return;
            };
            let idx = ((virt >> PAGE_SHIFT) & 0x1FF) as usize;
            (*l3).set(idx, PageTableEntry::new());
        }
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
        let mut vma = Vma::new(stack_start, stack_top, flags);
        vma.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC_STACK));
        self.map_vma(vma, Perm::ReadWrite)?;

        self.setup_stack(stack_top.as_usize(), stack_size);

        Ok(stack_top)
    }

    /// Copy address space using Copy-on-Write (twin contract: COW-exempt
    /// VMA ranges inherited writable, everything else downgraded in BOTH
    /// parent and child; one vma_read guard spans the exempt scan, the
    /// PTE copy and the VMA snapshot — R-WRITESTORM-3).
    pub fn fork(&self) -> Result<MmStruct, MapError> {
        let parent_vma_guard = self.vma_read();

        let cow_exempt: Vec<(u64, u64)> = parent_vma_guard
            .iter()
            .filter(|vma| {
                vma.flags().contains(crate::mm::vma::VmaFlags::SHARED)
                    || vma.vma_type() == crate::mm::vma::VmaType::Device
            })
            .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
            .collect();

        // SAFETY: self.pgd is a valid root PPN for the current address
        // space; the caller (fork) guarantees the parent is fully
        // initialized and consistent.
        let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
        let new_root_ppn = unsafe {
            copy_page_table_cow(self, &cow_exempt).ok_or(MapError::OutOfMemory)?
        };
        drop(_pte_guard);

        // SAFETY: new_root_ppn comes from copy_page_table_cow — a valid,
        // freshly allocated root. space_type and brk are valid by
        // construction from the current address space.
        let new_space = unsafe {
            MmStruct::new_shared(
                new_root_ppn,
                self.space_type(),
                self.brk(),
            )
        };

        {
            // Snapshot VMA data under the SAME parent read-guard held
            // since before the PTE copy (twin R-WRITESTORM-3).
            let vma_snapshot: Vec<(
                PageVirtAddr,
                PageVirtAddr,
                VmaFlags,
                VmaType,
                i32,
                u64,
                usize,
                VmaOrigin,
            )> = parent_vma_guard
                .iter()
                .map(|vma| {
                    (
                        vma.start(),
                        vma.end(),
                        vma.flags(),
                        vma.vma_type(),
                        vma.file_fd(),
                        vma.file_size(),
                        vma.offset(),
                        vma.origin(),
                    )
                })
                .collect();
            drop(parent_vma_guard);

            if !vma_snapshot.is_empty() {
                let mut new_vma_mgr = new_space.vma_write();
                for (start, end, flags, vma_type, file_fd, file_size, offset, origin) in
                    vma_snapshot
                {
                    let mut new_vma = Vma::new(start, end, flags);
                    new_vma.set_type(vma_type);
                    let mut o = origin;
                    o.mark_forked();
                    new_vma.set_origin(o);
                    new_vma.set_file_fd(file_fd);
                    new_vma.set_file_size(file_size);
                    new_vma.set_offset(offset);
                    // Clone the pinned vm_file so the child's demand
                    // faults read the file after fd close (twin P1).
                    if vma_type == crate::mm::vma::VmaType::FileBacked {
                        if let Some(f) = self.get_vma_file(start.as_usize()) {
                            new_space.pin_vma_file(start.as_usize(), f);
                        }
                    }
                    // Shared-memory attachments: increment nattch.
                    if vma_type == crate::mm::vma::VmaType::SharedMemory && file_fd >= 0 {
                        crate::ipc::sysv_shm::shm_attach_vma(file_fd);
                    }
                    let copied_pages = new_vma.page_count() as u64;
                    let _ = new_vma_mgr.add(new_vma);
                    new_space.add_total_vm(copied_pages);
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
        // dup_mm parity: the child that later crashes dumps the auxv of
        // the exec that built its image.
        {
            let auxv = self.saved_auxv();
            if !auxv.is_empty() {
                new_space.set_saved_auxv(&auxv);
            }
        }

        // RSS: the child's tables map (COW-shared) every page the parent
        // had resident — inherit the count for OOM badness.
        new_space.add_rss(self.rss());

        Ok(new_space)
    }
}

/// Convert permission to page table flags (Rux software-bit mask; the
/// hardware descriptor is derived at install time — see pagetable.rs)
fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
    let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    match perm {
        Perm::None => {
            flags |= PageTableEntry::R;
        }
        Perm::Read => {
            flags |= PageTableEntry::R;
        }
        Perm::ReadWrite => {
            flags |= PageTableEntry::R | PageTableEntry::W;
        }
        Perm::ReadWriteExec => {
            flags |= PageTableEntry::R | PageTableEntry::W | PageTableEntry::X;
        }
        Perm::ReadExec => {
            flags |= PageTableEntry::R | PageTableEntry::X;
        }
        Perm::Exec => {
            flags |= PageTableEntry::X;
        }
    }
    if space_type == PageTableType::User {
        flags |= PageTableEntry::U;
    }
    flags
}

// ==================== User page mapping ====================

/// Install ONE 4KB user page into a user root (L3 PAGE descriptor built
/// by the pagetable.rs constructor from the software flag mask), with a
/// per-page TLB invalidation so single-page callers (demand faults,
/// mremap, ashmem/shm) see the new translation on retry — riscv64
/// twin's map_page contract. Returns false when an intermediate table
/// allocation failed (failible map path) or a non-table entry blocks the
/// walk (user trees never host blocks; a foreign entry is corruption).
///
/// Takes NO PTE lock (callers hold it, twin protocol).
///
/// # Safety
/// user_root_ppn must be a valid user root; the caller holds the PTE
/// section; phys is an owned frame.
pub unsafe fn map_user_page(
    user_root_ppn: u64,
    user_virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> bool {
    // SAFETY: raw install below; flush on success.
    let ok = unsafe { map_user_page_noflush(user_root_ppn, user_virt, phys, flags) };
    if ok {
        // SAFETY: the translation was just (re)written; invalidate any
        // stale entry before the first access.
        unsafe { super::asid::flush_tlb_page(user_virt.bits() as usize, 0) };
    }
    ok
}

/// map_user_page without the TLB invalidation (batch primitive; region
/// callers flush once after their loop).
///
/// # Safety
/// See map_user_page.
pub unsafe fn map_user_page_noflush(
    user_root_ppn: u64,
    user_virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> bool {
    // SAFETY: walk/build under the caller's PTE section; tables come
    // from alloc_page_table (zeroed), the leaf from the constructor.
    unsafe {
        let mut table_phys = user_root_ppn << PAGE_SHIFT;
        for cur in 0..3u32 {
            let shift = PAGE_SHIFT + 9 * (3 - cur) as u64;
            let idx = ((user_virt.bits() >> shift) & 0x1FF) as usize;
            let table = super::mmu_init::get_page_table_virt(table_phys);
            let entry = (*table).get(idx);
            if entry.is_valid() && entry.is_table() {
                table_phys = entry.ppn() << PAGE_SHIFT;
                continue;
            }
            if entry.is_valid() {
                // Block leaf in the way: user trees never install blocks
                // (the walker only writes L3 pages), so this is a foreign
                // or corrupt entry — refuse rather than overwrite.
                return false;
            }
            let Some(np) = super::mmu_init::alloc_page_table() else {
                static PTFAIL: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if PTFAIL.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "map_user_page: PT alloc failed (L{} ) at va {:#x}",
                        cur,
                        user_virt.bits()
                    );
                }
                return false;
            };
            (*table).set(idx, PageTableEntry::new_table(np >> PAGE_SHIFT));
            table_phys = np;
        }
        let l3 = super::mmu_init::get_page_table_virt(table_phys);
        let idx = ((user_virt.bits() >> PAGE_SHIFT) & 0x1FF) as usize;
        (*l3).set(idx, PageTableEntry::new_page_user_flags(phys.ppn(), flags));
        true
    }
}

/// Map a user region page-by-page (virt→phys identity stride), one full
/// TLB flush at the end. Argument order matches the riscv64 twin ABI
/// (root, virt, phys, size, flags) used by the generic tree.
///
/// # Safety
/// See map_user_page.
pub unsafe fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) -> bool {
    // Serialize against fork's table walk (PTE lock coverage).
    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
    let Some(virt_end_val) = virt_start.checked_add(size) else {
        panic!("map_user_region: virt_start + size overflow");
    };

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);

    let mut virt = virt_start_addr.floor();
    let end = VirtAddr::new(virt_end_val).ceil();

    let mut ok = true;
    while virt.bits() < end.bits() {
        let offset = virt.bits() - virt_start_addr.bits();
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        // SAFETY: batch primitive inside the lock; flush once below.
        if !unsafe { map_user_page_noflush(user_root_ppn, virt, phys, flags) } {
            ok = false;
        }
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
    // SAFETY: batch of fresh translations; one flush publishes them all.
    unsafe { super::asid::flush_tlb_all() };
    ok
}

// ==================== Allocation / registry ====================

/// Allocate a physical page for user memory (riscv64 twin semantics)
pub fn alloc_user_phys_page() -> Option<u64> {
    let phys = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys != 0 {
        Some(phys as u64)
    } else {
        None
    }
}

/// Create a fresh user address space (empty TTBR0 root).
///
/// aarch64 divergence from the riscv64 twin: NO kernel-mapping clone —
/// the kernel half lives in TTBR1 (a separate root register), so the
/// user root starts (and stays) a pure user tree. This also removes the
/// twin's shared-kernel-subtree hazards entirely (teardown skips,
/// device-window restores, MMIO re-cloning).
pub fn create_user_address_space() -> Option<u64> {
    // The root frame comes from the staged table allocator (zeroed,
    // ledger-stamped in the Late stage).
    // SAFETY: single-owner fresh root, published only on Some.
    let root_phys = unsafe { super::mmu_init::alloc_page_table() }?;
    Some(root_phys >> PAGE_SHIFT)
}

// ==================== Global mm context + PTE lock ====================

/// Global mm context id (dfx parity)
pub static MM_CTX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Global PTE modify lock (riscv64 twin: serializes fork walks against
/// COW faults and mprotect/munmap table surgery)
pub static PTE_MODIFY_LOCK: crate::mm::mm_struct::GlobalPteLock =
    crate::mm::mm_struct::GlobalPteLock::new();

/// Per-mm leaf-PTE section hook (INV-LOCK-7), GLOBAL mode like the
/// riscv64 twin: every mm maps to the same global epoch section
#[inline]
pub fn pte_lock_for(_mm: &crate::mm::mm_struct::MmStruct) -> &'static crate::mm::mm_struct::PteLock {
    &PTE_GLOBAL_HANDLE
}

static PTE_GLOBAL_HANDLE: crate::mm::mm_struct::PteLock =
    crate::mm::mm_struct::PteLock::new();

// ==================== Leaf primitives for shared mm code (FD-1) ====================
//
// mm/rmap.rs (try_to_unmap_inner) and mm/compact.rs (remap_page) used to
// carry Sv39 manual descents of their own — root indexed with bits 38:30
// — which on this 4-level arch read the WRONG L0 slot, failed
// is_valid(), and silently no-oped (swap-out unmap / compaction
// migration reclaimed nothing). The walk shape (L0 → L1 → L2 → L3, bits
// 47:39..20:12) and the descriptor encoding are arch facts and live
// here; the shared mm code calls only these three primitives.

/// Permission/flag bits of a 4K L3 descriptor — everything except the
/// output-address field (bits 47:12): valid/type, AttrIndx, AP, SH, AF,
/// nG, PXN/UXN and the Rux software permission bits. This is the exact
/// bit set [`leaf_pte_install`] puts back.
#[inline]
pub fn pte_flag_bits(bits: u64) -> u64 {
    bits & !0x0000_FFFF_FFFF_F000u64
}

/// Default flag bits for an anonymous user page — compaction's fallback
/// when the pre-unmap walk could not recover the live PTE. Valid PAGE
/// descriptor, normal cacheable, inner-shareable, AF, nG, EL0 RW
/// (software V|R|W|U|A|D), execute-never — the new_page_user shape with
/// X dropped.
pub const PTE_FLAGS_ANON_DEFAULT: u64 = PageTableEntry::V
    | PageTableEntry::TYPE_PAGE
    | PageTableEntry::ATTR_NORMAL
    | PageTableEntry::SH_IS
    | PageTableEntry::AF
    | PageTableEntry::NG
    | PageTableEntry::AP_EL0
    | PageTableEntry::PXN
    | PageTableEntry::UXN
    | PageTableEntry::R
    | PageTableEntry::W
    | PageTableEntry::U
    | PageTableEntry::A
    | PageTableEntry::D;

/// Replace the 4K L3 descriptor at `vaddr` with raw `new_bits` (0 =
/// unmap; a swap or migration entry — a non-valid descriptor may carry
/// arbitrary software bits, the fault path decodes them), under this
/// mm's PTE lock. The leaf must still be valid and map `expect_pfn`
/// (the R7-C3 re-validation: the caller's lock-free walk may have raced
/// a concurrent leaf writer). Returns true when the leaf was replaced;
/// the caller owns the TLB flush.
///
/// A BLOCK leaf at L0-L2 has no 4K slot to replace — returns false
/// (anonymous reclaim targets 4K mappings only).
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub unsafe fn leaf_pte_replace(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    expect_pfn: u64,
    new_bits: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    // SAFETY: caller guarantees a live root; the walk only reads table
    // frames reachable from it, and the leaf write goes through the
    // validated slot.
    unsafe {
        let idx = |shift: u64| ((vaddr >> shift) & 0x1FF) as usize;
        let root_table = super::mmu_init::get_page_table_virt(root_ppn << PAGE_SHIFT);
        let l0 = (*root_table).get(idx(39));
        if !l0.is_valid() || l0.is_leaf() {
            return false;
        }
        let t1 = super::mmu_init::get_page_table_virt(l0.ppn() << PAGE_SHIFT);
        let l1 = (*t1).get(idx(30));
        if !l1.is_valid() || l1.is_leaf() {
            return false;
        }
        let t2 = super::mmu_init::get_page_table_virt(l1.ppn() << PAGE_SHIFT);
        let l2 = (*t2).get(idx(21));
        if !l2.is_valid() || l2.is_leaf() {
            return false;
        }
        let t3 = super::mmu_init::get_page_table_virt(l2.ppn() << PAGE_SHIFT);
        let l3 = (*t3).get(idx(12));
        if !l3.is_valid() || l3.ppn() != expect_pfn {
            return false;
        }
        (*t3).set(idx(12), PageTableEntry::from_bits(new_bits));
    }
    true
}

/// Rebuild a VALID 4K L3 descriptor mapping `pfn` at `vaddr` from saved
/// flag bits ([`pte_flag_bits`] shape), under this mm's PTE lock. The
/// leaf slot may currently hold a non-valid marker (the compaction
/// migration window) — only the intermediate levels must exist. The
/// valid bit is OR-ed in unconditionally. Returns true when installed;
/// the caller owns the TLB flush.
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub unsafe fn leaf_pte_install(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    pfn: u64,
    flags: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    // SAFETY: see leaf_pte_replace.
    unsafe {
        let idx = |shift: u64| ((vaddr >> shift) & 0x1FF) as usize;
        let root_table = super::mmu_init::get_page_table_virt(root_ppn << PAGE_SHIFT);
        let l0 = (*root_table).get(idx(39));
        if !l0.is_valid() || l0.is_leaf() {
            return false;
        }
        let t1 = super::mmu_init::get_page_table_virt(l0.ppn() << PAGE_SHIFT);
        let l1 = (*t1).get(idx(30));
        if !l1.is_valid() || l1.is_leaf() {
            return false;
        }
        let t2 = super::mmu_init::get_page_table_virt(l1.ppn() << PAGE_SHIFT);
        let l2 = (*t2).get(idx(21));
        if !l2.is_valid() || l2.is_leaf() {
            return false;
        }
        let t3 = super::mmu_init::get_page_table_virt(l2.ppn() << PAGE_SHIFT);
        let new_bits = (pfn << 12) | pte_flag_bits(flags) | PageTableEntry::V;
        (*t3).set(idx(12), PageTableEntry::from_bits(new_bits));
    }
    true
}

// ==================== Bulk alloc-map helpers ====================

/// Allocate and map anonymous user memory (sys_brk path; riscv64 twin):
/// power-of-two buddy chunks capped at MAX_ORDER, zeroed BEFORE mapping
/// (leak-window fix), preserving present leaves (stale brk growth
/// windows), freeing block excess immediately.
///
/// # Safety
/// user_root_ppn must be a valid user root.
pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    if size == 0 {
        return None;
    }
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let mut first_phys = 0u64;
    let mut mapped_pages = 0usize;

    while mapped_pages < page_count {
        let remain = page_count - mapped_pages;
        let order = if remain == 1 {
            0
        } else {
            (remain.next_power_of_two().trailing_zeros() as usize)
                .min(crate::mm::zone::MAX_ORDER)
        };
        let block_pages = 1usize << order;
        // block_pages >= remain whenever remain <= 2^MAX_ORDER, so the
        // last chunk covers the rest exactly.
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);
        if phys_addr == 0 {
            // All-or-nothing: report failure; sys_brk leaves the break
            // unchanged (mapped-so-far chunks are zero anonymous pages a
            // retry re-maps over).
            return None;
        }

        // Zero BEFORE mapping (twin leak-window fix: the frames are
        // reachable through the linear map regardless).
        // SAFETY: exclusively owned fresh block.
        unsafe {
            let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(
                virt_addr_ptr.bits() as *mut u8,
                0,
                block_pages * PAGE_SIZE as usize,
            );
        }

        // PRESERVE PRESENT LEAVES (twin: do_brk_flags only maps ABSENT
        // pages): a stale leaf in the brk growth window must not be
        // re-mapped with the fresh zero block.
        {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for i in 0..chunk_pages {
                let va = virt_addr + ((mapped_pages + i) * PAGE_SIZE as usize) as u64;
                // SAFETY: read-only walk on a valid root.
                let present = unsafe { PageTableWalker::walk(user_root_ppn, va) }
                    .map_or(false, |(_p, bits)| bits & PageTableEntry::V != 0);
                if present {
                    // SAFETY: freshly allocated, exclusively owned page.
                    unsafe {
                        crate::mm::page_alloc::free_pages(
                            ((base_pfn + i) as usize) << PAGE_SHIFT,
                            0,
                        );
                    }
                    continue;
                }
                // SAFETY: target leaf absent; frame zeroed and owned.
                unsafe {
                    map_user_page(
                        user_root_ppn,
                        VirtAddr::new(va),
                        PhysAddr::new(((base_pfn + i) as u64) << PAGE_SHIFT),
                        flags,
                    );
                }
            }
        }

        // Free the rounded-up excess immediately (twin R7-C5: unmapped
        // excess pages have no PTE, so no teardown path would free them).
        if block_pages > chunk_pages {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for pfn in (base_pfn + chunk_pages)..(base_pfn + block_pages) {
                crate::mm::page_alloc::free_pages((pfn << PAGE_SHIFT) as usize, 0);
            }
        }

        if mapped_pages == 0 {
            first_phys = phys_addr as u64;
        }
        mapped_pages += chunk_pages;
    }

    Some(first_phys)
}

/// Allocate and wire a user page-table region (exec image/stack path;
/// riscv64 twin): BINARY-DECOMPOSITION chunks — each iteration takes the
/// largest power-of-two block that FITS the remaining request (floor),
/// zeroed before mapping, no excess to free. The returned phys base is
/// the FIRST block only; callers that write the whole range must walk
/// the page tables.
///
/// # Safety
/// See alloc_and_map_user_memory.
pub unsafe fn alloc_and_map_to_user_table(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    if size == 0 {
        return None;
    }
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let user_flags = flags | PageTableEntry::U;

    let mut first_phys = 0u64;
    let mut mapped_pages = 0usize;
    while mapped_pages < page_count {
        let remain = page_count - mapped_pages;
        // floor(log2(remain)), capped at MAX_ORDER; block_pages <=
        // remain always, so chunk == block and no excess exists.
        let order = ((usize::BITS - 1 - remain.leading_zeros()) as usize)
            .min(crate::mm::zone::MAX_ORDER);
        let block_pages = 1usize << order;
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);
        if phys_addr == 0 {
            return None; // all-or-nothing; the RAII guard frees partials
        }

        // Zero BEFORE mapping (twin leak-window fix).
        // SAFETY: exclusively owned fresh block.
        unsafe {
            let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, chunk_bytes);
        }

        // SAFETY: freshly zeroed owned block mapped into a valid root.
        unsafe {
            map_user_region(
                user_root_ppn,
                virt_addr + (mapped_pages * PAGE_SIZE as usize) as u64,
                phys_addr as u64,
                chunk_bytes as u64,
                user_flags,
            );
        }

        if mapped_pages == 0 {
            first_phys = phys_addr as u64;
        }
        mapped_pages += chunk_pages;
    }

    Some(first_phys)
}

// ==================== COW support ====================

/// COW software bits — descriptor upper-half software space (bit budget
/// documented in pagetable.rs; these never reach hardware)
pub mod cow_flags {
    use super::super::pagetable::PageTableEntry;

    /// COW marker: mapping is shared read-only until the fault copies it
    pub const COW: u64 = PageTableEntry::SW_COW;
    /// Page-cache-backed mapping (filemap fault family — reserved for
    /// the twin's PCACHE port; dormant on aarch64 today)
    pub const PCACHE: u64 = PageTableEntry::SW_PCACHE;

    /// Whether these bits describe a shared page-cache mapping
    pub fn pte_is_shared_cache(bits: u64) -> bool {
        bits & PCACHE != 0
    }
}

/// Drop the page-cache pin owned by ONE mapping of a cache-backed frame
/// (twin contract; dormant on aarch64 until the filemap port lands).
pub(super) fn release_cache_mapping_pin(ppn: usize) {
    use crate::mm::page_desc::pfn_to_page_mut;
    let page = pfn_to_page_mut(ppn);
    if page.is_null() {
        return;
    }
    // SAFETY: descriptor of a live frame that was mapped until this
    // unmap; mapping/index are only cleared by the cache's own release
    // path, which cannot run while a pin (held until this call) exists.
    unsafe {
        let key = (*page).mapping() as u64;
        let index = (*page).index() as u64;
        if key != 0 {
            crate::fs::page_cache::get_page_cache().put_by_key(key, index);
        }
    }
}

/// Copy the parent's user tree into a fresh child root with COW marking
/// (INV-COW-1..5). Every valid user leaf takes one page reference (one
/// per sharer); WRITABLE non-exempt leaves are downgraded to RO+COW in
/// BOTH parent and child. Foreign (boot-reserved / kernel-heap /
/// framebuffer) frames are copied as-is with no refcount — the owning
/// subsystem releases them. The walk runs under the caller's PTE
/// section (fork's PTE_MODIFY_LOCK).
///
/// aarch64 divergence: the child root is a pure TTBR0 tree — there are
/// no kernel PGD entries to share (the twin copies VPN2 >=
/// KERNEL_PGD_START verbatim).
///
/// # Safety
/// Parent root valid; PTE_MODIFY_LOCK held.
pub unsafe fn copy_page_table_cow(
    parent: &crate::mm::mm_struct::MmStruct,
    cow_exempt: &[(u64, u64)],
) -> Option<u64> {
    use crate::mm::page_desc::pfn_to_page_mut;

    let parent_root_ppn = parent.root_ppn();
    if parent_root_ppn == 0 {
        return None;
    }

    // Framebuffer frames are device-shared memory and must NEVER be
    // COW-marked — the physical-range exemption survives VMA-state loss
    // (twin fbterm-freeze root cause).
    let fb_frame_range: Option<(u64, u64)> =
        crate::drivers::gpu::get_framebuffer_info()
            .map(|i| (i.addr >> 12, (i.addr + i.size as u64 + 0xFFF) >> 12));

    let child_root_phys = super::mmu_init::alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    let parent_root =
        super::mmu_init::get_page_table_virt(parent_root_ppn << PAGE_SHIFT);
    let child_root = super::mmu_init::get_page_table_virt(child_root_phys);

    for l0 in 0..512usize {
        let e0 = (*parent_root).get(l0);
        if !e0.is_valid() {
            continue;
        }
        if !e0.is_table() {
            // L0 block (512GB): not a user mapping the walker ever
            // installs; copy verbatim without refcount.
            (*child_root).set(l0, e0);
            continue;
        }

        let child_t1_phys = super::mmu_init::alloc_page_table()?;
        (*child_root).set(l0, PageTableEntry::new_table(child_t1_phys >> PAGE_SHIFT));
        let pt1 = super::mmu_init::get_page_table_virt(e0.ppn() << PAGE_SHIFT);
        let ct1 = super::mmu_init::get_page_table_virt(child_t1_phys);

        for l1 in 0..512usize {
            let e1 = (*pt1).get(l1);
            if !e1.is_valid() {
                continue;
            }
            if e1.is_leaf() {
                // 1GB block leaf: refcount-and-share (twin gigapage arm).
                cow_share_block(e1);
                (*ct1).set(l1, e1);
                continue;
            }
            if !e1.is_table() {
                continue;
            }

            let child_t2_phys = super::mmu_init::alloc_page_table()?;
            (*ct1).set(l1, PageTableEntry::new_table(child_t2_phys >> PAGE_SHIFT));
            let pt2 = super::mmu_init::get_page_table_virt(e1.ppn() << PAGE_SHIFT);
            let ct2 = super::mmu_init::get_page_table_virt(child_t2_phys);

            for l2 in 0..512usize {
                let e2 = (*pt2).get(l2);
                if !e2.is_valid() {
                    continue;
                }
                if e2.is_leaf() {
                    cow_share_block(e2);
                    (*ct2).set(l2, e2);
                    continue;
                }
                if !e2.is_table() {
                    continue;
                }

                let child_t3_phys = super::mmu_init::alloc_page_table()?;
                (*ct2).set(
                    l2,
                    PageTableEntry::new_table(child_t3_phys >> PAGE_SHIFT),
                );
                let pt3 = super::mmu_init::get_page_table_virt(e2.ppn() << PAGE_SHIFT);
                let ct3 = super::mmu_init::get_page_table_virt(child_t3_phys);

                for l3 in 0..512usize {
                    // The walk runs under fork's PTE_MODIFY_LOCK — no
                    // per-leaf locking here (the lock is not reentrant).
                    let e3 = (*pt3).get(l3);
                    if !e3.is_valid() {
                        continue;
                    }

                    let leaf_va = ((l0 as u64) << 39)
                        | ((l1 as u64) << 30)
                        | ((l2 as u64) << 21)
                        | ((l3 as u64) << 12);
                    let leaf_phys = e3.output_addr();

                    let exempt = cow_exempt
                        .iter()
                        .any(|(s, e)| leaf_va >= *s && leaf_va < *e)
                        || fb_frame_range.map_or(false, |(lo, hi)| {
                            (leaf_phys >> 12) >= lo && (leaf_phys >> 12) < hi
                        });
                    let foreign = crate::mm::page_alloc::is_foreign_user_frame(
                        leaf_phys as usize,
                    );

                    let new_entry = if e3.is_user() && !foreign {
                        // SAFETY: descriptor of the mapped leaf frame.
                        unsafe {
                            let page = pfn_to_page_mut((leaf_phys >> PAGE_SHIFT) as usize);
                            if !page.is_null() {
                                // One reference per sharer (writable and
                                // read-only alike — twin protocol).
                                (*page).get_page();
                                (*page).inc_mapcount();

                                if e3.is_writable() && !exempt {
                                    // INV-COW-5: downgrade BOTH parent and
                                    // child to RO+COW. The software mask
                                    // keeps every permission bit, drops W,
                                    // adds COW; rebuild_user_leaf rederives
                                    // the hardware attrs (AP_RO).
                                    (*page).set_flag(
                                        crate::mm::page_desc::PageFlag::Cow,
                                    );
                                    let sw = (e3.bits()
                                        & PageTableEntry::SW_SOFTWARE_MASK)
                                        & !PageTableEntry::W
                                        | cow_flags::COW;
                                    let cow_e =
                                        PageTableEntry::rebuild_user_leaf(e3.bits(), sw);
                                    let pt3_mut = pt3 as *mut PageTable;
                                    (*pt3_mut).set(l3, cow_e);
                                    cow_e
                                } else {
                                    e3
                                }
                            } else {
                                e3
                            }
                        }
                    } else {
                        e3
                    };

                    (*ct3).set(l3, new_entry);
                }
            }
        }
    }

    // SMP coherence (twin protocol): the walk DOWNGRADED the parent's
    // writable PTEs to RO+COW; every PE's stale writable entries must
    // die or a parent store issued elsewhere bypasses COW into the now
    // shared frame (the twin's lost-write family).
    crate::arch::ipi::flush_tlb_others();
    // SAFETY: permissions were repaired above.
    unsafe { super::asid::flush_tlb_all() };

    Some(child_root_ppn)
}

/// Refcount-and-share one BLOCK leaf inherited by fork (never COW-marked
/// — the user walker never installs writable blocks; refcount keeps the
/// teardown put balanced).
///
/// # Safety
/// Descriptor belongs to a mapped frame.
unsafe fn cow_share_block(entry: PageTableEntry) {
    use crate::mm::page_desc::pfn_to_page_mut;
    if !entry.is_user() {
        return;
    }
    let phys = entry.output_addr();
    if phys == 0
        || crate::mm::page_alloc::is_foreign_user_frame(phys as usize)
        || crate::drivers::gpu::get_framebuffer_info().map_or(false, |i| {
            phys >= i.addr && phys < i.addr + i.size as u64
        })
    {
        return;
    }
    // SAFETY: descriptor of the mapped block frame.
    unsafe {
        let page = pfn_to_page_mut((phys >> PAGE_SHIFT) as usize);
        if !page.is_null() {
            (*page).get_page();
            (*page).inc_mapcount();
        }
    }
}

/// Outcome of a COW fault resolution attempt (twin tri-state: a changed
/// PTE is a Retry, never a kill — the fake-OOM layer-2 family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    /// COW resolved: PTE is writable again (exclusive fast path, or a
    /// private copy was installed and mapped)
    Resolved,
    /// The PTE changed under us between the lock-free COW check and the
    /// locked re-walk — re-execute the faulting instruction
    Retry,
    /// Allocating the private copy failed — genuine out-of-memory
    OutOfMemory,
}

/// Handle copy-on-write page fault (INV-COW-3/4; twin contract):
/// speculative private-copy allocation BEFORE the PTE section (S7 twin —
/// the allocator slowpath must never run inside the epoch), locked
/// re-walk, exclusive fast path (refcount == 1 → restore W in place),
/// copy path (allocate → copy → install → put old). `mm` is the owning
/// address space of `root_ppn`.
///
/// # Safety
/// mm/root_ppn/fault_addr describe the faulting mapping of the current task.
pub unsafe fn handle_cow_fault(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    fault_addr: VirtAddr,
) -> CowFaultResult {
    use crate::mm::page_desc::pfn_to_page_mut;

    // Speculative pre-lock allocation (twin S7-oom01wedge).
    let spec_phys = match alloc_user_phys_page() {
        Some(p) => p,
        None => return CowFaultResult::OutOfMemory,
    };
    struct SpecPage(Option<u64>);
    impl Drop for SpecPage {
        fn drop(&mut self) {
            if let Some(p) = self.0.take() {
                crate::mm::page_alloc::free_pages(p as usize, 0);
            }
        }
    }
    let mut spec = SpecPage(Some(spec_phys));

    let _pte_guard = pte_lock_for(mm).lock_irqsave();

    let virt_addr = fault_addr.bits();

    // Locked re-walk: the full 4-level descent (a missing/changed upper
    // level after the lock-free COW check is a Retry, not an error).
    // SAFETY: root_ppn valid for the faulting mm; read-only walk.
    let Some((old_ppn, old_bits)) =
        (unsafe { PageTableWalker::walk(root_ppn, virt_addr) })
    else {
        return CowFaultResult::Retry;
    };
    if old_bits & cow_flags::COW == 0 {
        // COW bit gone at re-check time: a sibling sharing this mm broke
        // the COW first. Re-execute, do NOT kill (twin fake-OOM note).
        return CowFaultResult::Retry;
    }

    let old_page = pfn_to_page_mut(old_ppn as usize);
    let refcount = if !old_page.is_null() {
        // SAFETY: null-checked descriptor.
        unsafe { (*old_page).refcount() }
    } else {
        1
    };

    // Exclusive owner: restore W in place (INV-COW-3). The software mask
    // drops COW/PCACHE and adds W|R (twin R7-A3: W implies R).
    if refcount <= 1 {
        // PCACHE leaf reaching the exclusive arm means the cache entry
        // is gone; the pin drop is a no-op then (twin note).
        let pcache = old_bits & cow_flags::PCACHE;
        let sw = (old_bits & PageTableEntry::SW_SOFTWARE_MASK)
            & !(cow_flags::COW | cow_flags::PCACHE)
            | PageTableEntry::W
            | PageTableEntry::R;
        if pcache != 0 {
            release_cache_mapping_pin(old_ppn as usize);
        }
        // Clear the descriptor flag too (twin R7-C9: "COW set with
        // refcount 1" is an invariant violation).
        // SAFETY: null-checked descriptor.
        if !old_page.is_null() {
            unsafe {
                (*old_page).clear_flag(crate::mm::page_desc::PageFlag::Cow);
            }
        }
        // SAFETY: present user leaf; rebuild preserves its output address.
        unsafe {
            write_user_leaf(root_ppn, virt_addr, PageTableEntry::rebuild_user_leaf(old_bits, sw));
            // Fast-path PTE replacement must be visible to every PE that
            // may hold the old RO entry (twin shootdown discipline).
            crate::arch::ipi::flush_tlb_others();
            super::asid::flush_tlb_page(virt_addr as usize, 0);
        }
        return CowFaultResult::Resolved;
    }

    // Shared: install the private copy (INV-COW-4). The page came from
    // the speculative pre-lock alloc; sole consumer, take() cannot fail.
    let new_phys = match spec.0.take() {
        Some(p) => p,
        None => return CowFaultResult::OutOfMemory,
    };
    let new_ppn = new_phys >> PAGE_SHIFT;

    // Copy while the old page is still pinned (refcount >= 2).
    // SAFETY: both frames are RAM pages inside the linear map.
    unsafe {
        let new_virt = phys_to_virt(PhysAddr::new(new_phys));
        let old_virt = phys_to_virt(PhysAddr::new(old_ppn << PAGE_SHIFT));
        core::ptr::copy_nonoverlapping(
            old_virt.bits() as *const u8,
            new_virt.bits() as *mut u8,
            PAGE_SIZE as usize,
        );
    }

    // New leaf: old permissions, W restored, no COW/PCACHE (the copy is
    // exclusively owned by this mapping).
    let sw = (old_bits & PageTableEntry::SW_SOFTWARE_MASK)
        & !(cow_flags::COW | cow_flags::PCACHE)
        | PageTableEntry::W
        | PageTableEntry::R;
    let new_pte = PageTableEntry::new_page_user_flags(new_ppn, sw);

    // Install the new PTE before dropping our share of the old page.
    // SAFETY: verified present leaf slot; fresh owned frame.
    unsafe {
        write_user_leaf(root_ppn, virt_addr, new_pte);
        // Twin shootdown discipline: the old frame → private copy swap
        // must not leave a stale translation on any PE.
        crate::arch::ipi::flush_tlb_others();
        super::asid::flush_tlb_page(virt_addr as usize, 0);
    }

    // Release our share of the old page: refcount AND mapcount (the
    // mapping moved to the copy; a leaked mapcount corrupts reclaim).
    // SAFETY: null-checked descriptor of the old shared frame.
    if !old_page.is_null() {
        unsafe {
            (*old_page).put_page_site(crate::dfx::driftwatch::S_COW_RV);
            (*old_page).dec_mapcount();
            (*old_page).clear_flag(crate::mm::page_desc::PageFlag::Cow);
        }
    }

    // Old PCACHE pin follows the mapping (twin ordering; dormant).
    if old_bits & cow_flags::PCACHE != 0 {
        release_cache_mapping_pin(old_ppn as usize);
    }

    // Reverse mapping + RSS for the new private page.
    {
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
        let new_page = pfn_to_page_mut(new_ppn as usize);
        if !new_page.is_null() {
            // SAFETY: null-checked; exclusively owned fresh frame.
            unsafe {
                (*new_page).set_flag(PageFlag::Anonymous);
                (*new_page).set_flag(PageFlag::SwapBacked);
                (*new_page).set_index(virt_addr as usize / (PAGE_SIZE as usize));
                (*new_page).inc_mapcount();
                crate::mm::rmap::page_record_mapping(
                    &*new_page,
                    mm as *const _ as usize,
                    virt_addr as usize,
                );
                crate::mm::lru::page_add_anon_lru(&*new_page);
            }
        }
        mm.add_rss(1);
    }

    CowFaultResult::Resolved
}

/// Overwrite ONE present L3 user leaf with `entry` (no walk allocation;
/// the caller verified the leaf exists under the PTE lock).
///
/// # Safety
/// PTE_MODIFY_LOCK held; the leaf at `va` is present.
unsafe fn write_user_leaf(root_ppn: u64, va: u64, entry: PageTableEntry) {
    // SAFETY: present-tables-only descent on a valid root.
    unsafe {
        let Some(l3) = super::mmu_init::user_l3_table_for(root_ppn, va) else {
            return;
        };
        let idx = ((va >> PAGE_SHIFT) & 0x1FF) as usize;
        (*l3).set(idx, entry);
    }
}

/// Check if the leaf at `addr` carries the COW marker
///
/// # Safety
/// root_ppn valid; read-only walk.
pub unsafe fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    // SAFETY: read-only walk.
    unsafe {
        PageTableWalker::walk(root_ppn, addr.bits())
            .map_or(false, |(_ppn, bits)| bits & cow_flags::COW != 0)
    }
}

/// Leaf permissions as (readable, writable, executable, user) from the
/// software mirror bits
///
/// # Safety
/// root_ppn valid; read-only walk.
pub unsafe fn check_pte_permissions(
    root_ppn: u64,
    addr: VirtAddr,
) -> Option<(bool, bool, bool, bool)> {
    // SAFETY: read-only walk.
    unsafe {
        PageTableWalker::walk(root_ppn, addr.bits()).map(|(_, bits)| {
            (
                bits & PageTableEntry::R != 0,
                bits & PageTableEntry::W != 0,
                bits & PageTableEntry::X != 0,
                bits & PageTableEntry::U != 0,
            )
        })
    }
}

// ==================== mprotect + PROT_NONE markers ====================

/// R-WRITESTORM-2 twin: is this leaf a PROT_NONE marker installed by
/// mprotect(prot==0)? Software mirror V|U with no R/W/X (the descriptor
/// also denies EL0 in hardware — see rebuild_user_leaf_protnone).
pub fn is_protnone_marker(bits: u64) -> bool {
    bits & PageTableEntry::V != 0
        && bits & PageTableEntry::U != 0
        && bits & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X) == 0
}

/// Rewrite the permissions of every PRESENT user leaf in
/// [start_virt, start_virt + num_pages*4K) — the mprotect arm
/// (x86_64 `mprotect_pte_range` twin on the aarch64 4-level walker).
///
/// - The output address (PPN) always survives; COW/PCACHE software
///   bits ride along (COW defers W to the copy fault; PCACHE is a
///   mapping-ownership pin — dropping it would leak the pin).
/// - prot == 0 installs the PROT_NONE MARKER: valid descriptor,
///   software V|U with no R/W/X, hardware denies EL0 entirely. The
///   marker is advisory PTE state — a later fault under a VMA that
///   grants the access re-presents the leaf (page_fault.rs).
/// - User trees only carry L3 page leaves; table/invalid slots are
///   skipped (the range was validated against the VMA list by the
///   caller, so unmapped middles mean "not populated yet" — the
///   demand-fill path will materialize them with the VMA's perms).
///
/// TLB: per-page `tlbi aside1e1is`; the caller holds the PTE section.
///
/// # Safety
/// root_ppn must be the caller's live user page-table root; caller
/// holds the PTE modify section (PTE_MODIFY_LOCK).
pub unsafe fn mprotect_pte_range(root_ppn: u64, start_virt: u64, num_pages: u64, prot: u32) {
    // Software base mask. W requires R (hardware AP_RO covers the W=0
    // half; the software mirror keeps the same fold the fault path uses).
    let mut base = PageTableEntry::V | PageTableEntry::U
        | PageTableEntry::A | PageTableEntry::D;
    if prot & 0x1 != 0 {
        base |= PageTableEntry::R;
    }
    if prot & 0x2 != 0 {
        base |= PageTableEntry::R | PageTableEntry::W;
    }
    if prot & 0x4 != 0 {
        base |= PageTableEntry::X;
    }

    for i in 0..num_pages {
        let va = start_virt.wrapping_add(i * PAGE_SIZE);

        // SAFETY: root_ppn is the live user root; the helper only walks
        // table descriptors (no leaf writes at upper levels).
        let Some(l3) = (unsafe { super::mmu_init::user_l3_table_for(root_ppn, va) }) else {
            continue;
        };
        let idx = ((va >> PAGE_SHIFT) & 0x1FF) as usize;
        // SAFETY: l3 comes from the walker; idx is masked to 9 bits.
        let raw = unsafe { (*l3).get(idx).bits() };
        // Empty or non-leaf slots: nothing to rewrite (see doc).
        if raw & PageTableEntry::V == 0 || raw & PageTableEntry::TYPE_MASK != PageTableEntry::TYPE_PAGE {
            continue;
        }
        // Foreign/kernel-frame leaves carry no U bit in the mirror —
        // leave them alone (a TTBR0 tree should not hold any, but a
        // stray boot-reserved leaf must not be user-rewritten).
        if raw & PageTableEntry::U == 0 {
            continue;
        }

        let cow_bit = raw & cow_flags::COW;
        let pcache_bit = raw & cow_flags::PCACHE;

        let new_pte = if prot & 0x7 == 0 {
            // PROT_NONE marker (constructor preserves COW/PCACHE itself)
            PageTableEntry::rebuild_user_leaf_protnone(raw)
        } else {
            let mut flags = base;
            if cow_bit != 0 {
                flags |= cow_bit;
                flags &= !PageTableEntry::W; // W deferred to the COW fault
            }
            if pcache_bit != 0 {
                flags |= pcache_bit;
                // A PRIVATE cache leaf never gains W in place: the frame
                // is shared with the page cache, so a W-upgrade diverts
                // the first write through COW (writenotify; twins). A
                // SHARED cache leaf keeps W — stores must land in the
                // cache frame (PCACHE is dormant on aarch64 today, but
                // the ownership rule rides the bit).
                if flags & PageTableEntry::W != 0 && !cow_flags::pte_is_shared_cache(raw) {
                    flags |= cow_flags::COW;
                    flags &= !PageTableEntry::W;
                }
            }
            PageTableEntry::rebuild_user_leaf(raw, flags)
        };

        // SAFETY: same slot the raw bits came from, under the caller's
        // PTE section.
        unsafe { (*l3).set(idx, new_pte) };
        // SAFETY: this translation changed; drop any cached walk.
        unsafe { super::asid::flush_tlb_page(va as usize, 0) };
    }
}
