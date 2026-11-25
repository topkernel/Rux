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
use super::r#unsafe::PtTable;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaOrigin, VmaType};
use crate::mm::{MmStruct, alloc_pages, GfpFlags};

// Re-export AddressSpace for backward compatibility
pub use crate::mm::AddressSpace;

// ==================== MmStruct Extension Methods ====================

impl MmStruct {
    /// Enable this address space (switch CR3 to this root)
    ///
    /// The load is PCID-tagged when armed (NOFLUSH keeps cached
    /// translations); without PCID it flushes non-global entries.
    pub fn enable(&self) {
        // A user mm must enter under its OWN PCID; asid 0 here means
        // kernel-root or PCID-off — both correct via build_satp.
        let asid = if self.space_type() == PageTableType::User {
            let a = self.asid();
            if a != 0 {
                a
            } else {
                self.alloc_asid().unwrap_or(0)
            }
        } else {
            0
        };
        crate::arch::mm::asid::switch_root(self.pgd, asid);
    }

    /// Disable address space (switch back to the kernel root)
    ///
    /// x86 has no bare mode: "disabled" means the static kernel PML4.
    pub fn disable() {
        crate::arch::mm::asid::switch_root(
            get_kernel_page_table_ppn(),
            crate::arch::mm::asid::ASID_KERNEL,
        );
    }

    /// Flush entire TLB (reload CR3)
    pub fn flush_tlb() {
        crate::arch::mm::asid::flush_tlb_all();
    }

    /// Flush TLB for specified page (invlpg)
    pub fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        crate::arch::cpu::invlpg(vaddr.as_usize() as u64);
    }

    // ==================== VMA Operations ====================


    /// Map single page (for lazy mapping/page fault handling)
    pub fn map_single_page(&self, virt_addr: VirtAddr, perm: Perm) -> Result<(), MapError> {
        let phys_addr = alloc_user_phys_page().ok_or(MapError::OutOfMemory)? as usize;
        let flags = perm_to_flags(perm, self.space_type());

        // INV-XE-ZP: fresh exclusive frame from alloc_user_phys_page.
        {
            let ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            super::mmu_init::zero_owned_pages(ptr.bits(), PAGE_SIZE_USIZE);
            core::sync::atomic::compiler_fence(Ordering::Release);
        }

        if !map_page(
            self.pgd,
            virt_addr,
            PhysAddr::new(phys_addr as u64),
            flags,
        ) {
            // Page-table allocation failure (fake-OOM family): free
            // the exclusively-owned data page and report OOM — the
            // caller propagates the error instead of the old kernel
            // panic.
            crate::mm::page_alloc::free_page(phys_addr);
            return Err(MapError::OutOfMemory);
        }

        {
            use crate::mm::page_desc::{page_desc, PageFlag};
            if let Some(page) = page_desc(phys_addr / (PAGE_SIZE as usize)) {
                page.set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU membership: make the page reclaimable
                // via swap-out (vmscan scans LRU_INACTIVE_ANON only).
                page.set_flag(PageFlag::SwapBacked);
                page.set_index(virt_addr.bits() as usize / (PAGE_SIZE as usize));
                page.inc_mapcount();
                crate::mm::rmap::page_record_mapping(
                    page,
                    self as *const _ as usize,
                    virt_addr.bits() as usize,
                );
                crate::api::mm::page_add_anon_lru(page);
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


    /// Unmap physical pages in specified range
    ///
    /// Reference-parity fast path (mmprof attribution, lb332/333): the old
    /// body ran a full PageTableWalker::walk PER PAGE — four dependent
    /// memory references per 4KB — measuring 23us per 1MB chunk under the
    /// OH FFRT arena churn (fixed_cold 24us / fixed_hot 64us vs the
    /// reference kernel's 1.2/3.4us, pbench P8). The reference walks the
    /// leaf table directly: resolve the upper chain ONCE per 2MB block,
    /// then iterate the 512 leaf PTEs. Per-page semantics are bit-exact
    /// with the old body (same swap/kernel-window/foreign-frame/rmap/
    /// refcount/clear discipline); exotic shapes (an upper-level leaf in
    /// the user range) fall back to the per-page walk.
    pub fn unmap_pages(
        &self,
        start: PageVirtAddr,
        size: usize,
        defer_tlb_flush: bool,
        free_emptied_tables: bool,
    ) -> Result<(), MapError> {
        // Serialize against fork's table walk and COW faults (this mm's
        // PTE section, INV-LOCK-7).
        let mp = crate::debug::mmprof::on();
        let mp_tw = if mp { crate::debug::mmprof::now() } else { 0 };
        let _pte_guard = pte_lock_for(self).lock();
        let mut addr = start.as_usize();
        let end = addr.saturating_add(size);

        // Root guard (same shape as PageTableWalker::walk): a freed-mm
        // root fed a garbage PPN used to die in phys_to_virt overflow.
        {
            let root_pa = self.pgd << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published()
                && root_pa != 0
                && !crate::mm::phys_valid(root_pa as usize)
            {
                return Ok(());
            }
        }
        let Some(root_table) = PtTable::from_phys(self.pgd << PAGE_SHIFT) else {
            return Ok(());
        };

        // PCACHE pin-release batching (perf-gap-hunt 2026-11-04): the
        // per-page release_cache_mapping_pin cost a hash probe + claim +
        // inode lock + BTreeMap get PER PAGE (~40ns — the dominant cost
        // of tearing down fault-around windows). Consecutive PCACHE
        // leaves of one inode accumulate their cache indices here and
        // flush through ONE put_range_by_key per (inode-change | 64
        // pages | end). Ordering discipline unchanged: each page's
        // descriptor put_page still happens BEFORE the flush that
        // releases its pin ("the pin's release may free an invalidated
        // frame — every reference must be gone by then": later is
        // stricter than per-page, never earlier).
        const PIN_BATCH: usize = 64;
        let mut pin_key: u64 = u64::MAX;
        let mut pin_batch: [u64; PIN_BATCH] = [0; PIN_BATCH];
        let mut pin_n: usize = 0usize;
        let mut rss_delta: u64 = 0u64;
        macro_rules! flush_pin_batch {
            () => {
                if pin_n > 0 {
                    crate::fs::page_cache::get_page_cache()
                        .put_range_by_key(pin_key, &pin_batch[..pin_n]);
                    pin_n = 0;
                }
            };
        }

        while addr < end {
            let a = VirtAddr::new(addr as u64);
            let pmd_base = addr & !(PMD_SIZE as usize - 1);
            let pmd_end = pmd_base + PMD_SIZE as usize;
            let block_end = end.min(pmd_end);

            let pte4 = root_table.get(a.pgd_index() as usize);
            if !pte4.is_valid() {
                addr = pmd_end;
                continue;
            }
            if pte4.is_leaf() {
                // 1GB leaf at PML4/PUD level: unreachable for user VAs in
                // practice; keep the old per-page behavior exactly.
                self.unmap_pages_slow(addr, block_end);
                addr = block_end;
                continue;
            }
            {
                let pa = pte4.phys_addr();
                if crate::mm::page_desc::descs_published()
                    && pa != 0
                    && !crate::mm::phys_valid(pa as usize)
                {
                    // Stale intermediate (freed/recycled table): the old
                    // walk bailed None and the read_pte_raw tail did
                    // nothing for a non-swap raw — skip the block.
                    addr = pmd_end;
                    continue;
                }
            }
            let Some(table3) = PtTable::from_phys(pte4.phys_addr()) else {
                addr = pmd_end;
                continue;
            };
            let pte3 = table3.get(a.pud_index() as usize);
            if !pte3.is_valid() {
                addr = pmd_end;
                continue;
            }
            if pte3.is_leaf() {
                if pte3.bits() & PageTableEntry::U == 0 {
                    // Kernel identity/device window leaf: the old body
                    // skipped these via the U-bit check, one page at a
                    // time; skipping the block is the same outcome.
                    addr = pmd_end;
                    continue;
                }
                self.unmap_pages_slow(addr, block_end);
                addr = block_end;
                continue;
            }
            {
                let pa = pte3.phys_addr();
                if crate::mm::page_desc::descs_published()
                    && pa != 0
                    && !crate::mm::phys_valid(pa as usize)
                {
                    addr = pmd_end;
                    continue;
                }
            }
            let Some(table2) = PtTable::from_phys(pte3.phys_addr()) else {
                addr = pmd_end;
                continue;
            };
            let pte2 = table2.get(a.pmd_index() as usize);
            if !pte2.is_valid() {
                addr = pmd_end;
                continue;
            }
            if pte2.is_leaf() {
                if pte2.bits() & PageTableEntry::U == 0 {
                    addr = pmd_end;
                    continue;
                }
                // A USER 2MB leaf covers the whole block; keep the old
                // per-page behavior exactly (fallback).
                self.unmap_pages_slow(addr, block_end);
                addr = block_end;
                continue;
            }
            {
                let pa = pte2.phys_addr();
                if crate::mm::page_desc::descs_published()
                    && pa != 0
                    && !crate::mm::phys_valid(pa as usize)
                {
                    addr = pmd_end;
                    continue;
                }
            }
            let Some(table1) = PtTable::from_phys(pte2.phys_addr()) else {
                addr = pmd_end;
                continue;
            };

            // Leaf-table iteration: identical per-page semantics to the
            // old walk()-based body.
            let mut va = addr;
            while va < block_end {
                let idx = (((va as u64) >> PAGE_SHIFT) & 0x1ff) as usize;
                let raw = table1.get_raw(idx);
                if raw == 0 {
                    va += PAGE_SIZE_USIZE;
                    continue;
                }
                if mp {
                    crate::debug::mmprof::bump(crate::debug::mmprof::S_UNPAGES, 1);
                }
                if raw & PageTableEntry::P == 0 {
                    // Swap entry (P=0 leaf): free the swap slot and clear
                    // the PTE (the old read_pte_raw tail). A non-swap P=0
                    // leaf (PROT_NONE marker / migration entry) is left
                    // alone, exactly like the old body.
                    if crate::api::mm::is_swap_entry(raw) {
                        crate::api::mm::swap_free_slot(
                            crate::api::mm::swap_entry_type(raw),
                            crate::api::mm::swap_entry_offset(raw),
                        );
                        Self::unmap_clear_leaf(table1, idx, va);
                    }
                    va += PAGE_SIZE_USIZE;
                    continue;
                }
                // E8-MM: a valid leaf WITHOUT the U bit is a KERNEL
                // translation the user VA range overlaps (identity device
                // windows cloned into every root). Not this mm's page.
                if raw & PageTableEntry::U == 0 {
                    va += PAGE_SIZE_USIZE;
                    continue;
                }
                let ppn_val = (raw & PageTableEntry::PHYS_MASK_PUBLIC) >> PAGE_SHIFT;
                // Foreign frame (kernel-heap GEM/fb backing): not this
                // mm's to put/free — clear the PTE only.
                if crate::mm::page_alloc::is_foreign_user_frame(
                    (ppn_val as usize) << PAGE_SHIFT,
                ) {
                    unsafe { Self::unmap_clear_leaf(table1, idx, va) };
                    va += PAGE_SIZE_USIZE;
                    continue;
                }
                // Remove reverse mapping before clearing PTE, then drop
                // this mapping's reference; the last reference frees the
                // page.
                use crate::mm::page_desc::page_desc;
                let leaf_is_pcache = raw & cow_flags::PCACHE != 0;
                // RO alias (PCACHE_RO): the leaf owns NEITHER a descriptor
                // reference NOR a cache pin — only the mapcount. The
                // mapcount drop (page_remove_rmap above) is its whole
                // teardown.
                let leaf_is_ro = raw & cow_flags::PCACHE_RO != 0;
                if let Some(page) = page_desc(ppn_val as usize) {
                    // Multi-mapping bookkeeping: drop this (mm, vpn) from
                    // the page's alternate rmap slots. PCACHE leaves never
                    // record an rmap (the frame's mapping/index keys
                    // belong to the page cache) — the alt-slot scan is a
                    // guaranteed no-op for them, so skip it (perf-gap-hunt:
                    // it costs a scan per page in fault-around teardown).
                    if !leaf_is_pcache {
                        page.rmap_alt_remove(
                            self as *const _ as usize,
                            va / PAGE_SIZE_USIZE,
                        );
                    }
                    if page.is_mapped() {
                        crate::mm::rmap::page_remove_rmap(page);
                    }
                    if !leaf_is_ro {
                        let new_ref =
                            page.put_page_site(crate::debug::driftwatch::S_UNMAP_X86);
                        if new_ref == 0 {
                            // Cache-backed leaf: the frame still belongs to
                            // the page cache — a zero put here is a phantom
                            // put; leaking beats wiring the live cache frame
                            // into the zone (pcachewedge lane; see
                            // pcache_leaf_phantom_zero_put).
                            if leaf_is_pcache {
                                crate::mm::page_alloc::pcache_leaf_phantom_zero_put(
                                    (ppn_val as usize) << PAGE_SHIFT,
                                    "unmap-fast",
                                );
                            } else {
                                crate::mm::page_alloc::free_pages(
                                    (ppn_val as usize) << PAGE_SHIFT, 0,
                                );
                            }
                        }
                        // PCACHE mapping (filemap_fault): drop this mapping's
                        // page-cache pin AFTER the descriptor put (see
                        // release_cache_mapping_pin). Order matters: the pin's
                        // release may free an invalidated frame — every
                        // reference must be gone by then. Consecutive leaves
                        // of one inode BATCH through put_range_by_key (one
                        // claim + lock per 64 pages; per-page release only on
                        // key change / batch full / end).
                        if leaf_is_pcache {
                            let key = page.mapping() as u64;
                            let index = page.index() as u64;
                            if pin_n == PIN_BATCH || (pin_n > 0 && key != pin_key) {
                                flush_pin_batch!();
                                pin_key = key;
                            }
                            pin_batch[pin_n] = index;
                            pin_n += 1;
                        }
                    }
                }
                unsafe { Self::unmap_clear_leaf(table1, idx, va) };
                // RSS accounting: one resident page leaves this address
                // space (accumulated; one atomic for the whole range —
                // perf-gap-hunt: the per-page fetch_add showed up in
                // fault-around window teardown).
                rss_delta += 1;
                va += PAGE_SIZE_USIZE;
            }

            // (fork-frag lane) EMPTIED-TABLE FREE — the fragmentation
            // source fix. munmap historically left emptied leaf tables
            // linked (36 tables vs 61 present PTEs in the pbench p4
            // parent); every later fork allocated a child frame per
            // table and every teardown walked them all. When this pass
            // just finished a table's LAST block, the table's PTEs are
            // as final as this syscall makes them: scan the whole 512
            // entries; all-zero (no mappings, no swap slots, no
            // PROT_NONE markers, no kernel device-window leaves) means
            // the frame is dead weight — unlink the PMD entry and
            // defer-free the frame behind the TLB-flush epoch
            // (mmu_init::pt_free_emptied_table owns the protocol).
            // `free_emptied_tables` is set by munmap only — madvise
            // (MADV_DONTNEED) zaps keep tables under live VMAs.
            if free_emptied_tables && block_end == pmd_end && table1.all_zero() {
                super::mmu_init::pt_free_emptied_table(
                    table2,
                    a.pmd_index() as usize,
                    pte2.phys_addr(),
                    self.tlb_cpu_mask(),
                );
            }
            addr = block_end;
        }

        // Release the tail of the pin batch (the macro borrows the locals).
        flush_pin_batch!();
        if rss_delta > 0 {
            self.sub_rss(rss_delta);
        }
        // hp89 hunt: record this PTE-clearing pass for post-hoc #GP
        // attribution (mm/unwatch — no printk on the record path).
        crate::mm::unwatch::note(
            crate::mm::unwatch::KIND_UNMAP_PAGES,
            start.as_usize() as u64,
            end as u64,
            rss_delta,
        );

        if mp {
            crate::debug::mmprof::add(crate::debug::mmprof::MU_WALK, mp_tw);
        }
        let mp_tt = if mp { crate::debug::mmprof::now() } else { 0 };
        if !defer_tlb_flush {
            crate::arch::ipi::flush_tlb_others_mm(
                self.tlb_cpu_mask(),
                start.as_usize() as u64,
                end as u64,
            );
            // Full TLB flush (CR3 reload); required after clearing PTEs so
            // subsequent accesses use the updated page table.
            crate::arch::mm::asid::flush_tlb_all();
        }
        if mp {
            crate::debug::mmprof::add(crate::debug::mmprof::MU_TLB, mp_tt);
        }

        Ok(())
    }

    /// Clear ONE leaf PTE with clear_pte's exact rule: restore the kernel
    /// device-window translation when the VA sits inside a kernel identity
    /// device window (E8-MM), else write 0. Caller holds the PTE section.
    #[inline]
    fn unmap_clear_leaf(table1: PtTable, idx: usize, va: usize) {
        let new_bits = super::mmu_init::kernel_device_window_pte(va as u64).unwrap_or(0);
        table1.set(idx, PageTableEntry::from_bits(new_bits));
    }

    /// The old per-page walk body, kept for the exotic shapes the fast
    /// path declines (an upper-level leaf in the user range). Semantics
    /// are the pre-fast-path behavior, unchanged.
    fn unmap_pages_slow(&self, start: usize, end: usize) {
        let mut addr = start;
        while addr < end {
            let ppn = PageTableWalker::walk(self.pgd, addr as u64);

            if ppn.is_none() {
                if let Some(raw) = super::page_fault::read_pte_raw(
                    self.pgd,
                    VirtAddr::new(addr as u64),
                ) {
                    if crate::api::mm::is_swap_entry(raw) {
                        crate::api::mm::swap_free_slot(
                            crate::api::mm::swap_entry_type(raw),
                            crate::api::mm::swap_entry_offset(raw),
                        );
                        self.clear_pte(addr as u64);
                    }
                }
                addr += PAGE_SIZE_USIZE;
                continue;
            }

            if let Some((ppn_val, pte_bits)) = ppn {
                if pte_bits & PageTableEntry::U == 0 {
                    addr += PAGE_SIZE_USIZE;
                    continue;
                }
                if crate::mm::page_alloc::is_foreign_user_frame(
                    (ppn_val as usize) << PAGE_SHIFT,
                ) {
                    self.clear_pte(addr as u64);
                    addr += PAGE_SIZE_USIZE;
                    continue;
                }
                use crate::mm::page_desc::page_desc;
                // RO alias: mapcount-only teardown (see unmap_pages fast
                // path / cow_flags::PCACHE_RO).
                let leaf_is_ro = pte_bits & cow_flags::PCACHE_RO != 0;
                if let Some(page) = page_desc(ppn_val as usize) {
                    if pte_bits & cow_flags::PCACHE == 0 {
                        page.rmap_alt_remove(
                            self as *const _ as usize,
                            addr / PAGE_SIZE_USIZE,
                        );
                    }
                    if page.is_mapped() {
                        crate::mm::rmap::page_remove_rmap(page);
                    }
                    if !leaf_is_ro {
                        let new_ref =
                            page.put_page_site(crate::debug::driftwatch::S_UNMAP_X86);
                        if new_ref == 0 {
                            // Cache-backed leaf: leak, never hand the live
                            // cache frame to the zone (pcachewedge lane).
                            if pte_bits & cow_flags::PCACHE != 0 {
                                crate::mm::page_alloc::pcache_leaf_phantom_zero_put(
                                    (ppn_val as usize) << PAGE_SHIFT,
                                    "unmap-slow",
                                );
                            } else {
                                crate::mm::page_alloc::free_pages(
                                    (ppn_val as usize) << PAGE_SHIFT, 0,
                                );
                            }
                        }
                        if pte_bits & cow_flags::PCACHE != 0 {
                            release_cache_mapping_pin(ppn_val as usize);
                        }
                    }
                }
                self.clear_pte(addr as u64);
                self.sub_rss(1);
            }

            addr += PAGE_SIZE_USIZE;
        }
    }

    /// Teardown of ONE present user leaf at `va`, for callers that already
    /// hold PTE_MODIFY_LOCK (mremap's page-move copy). Mirrors the per-page
    /// body of unmap_pages: kernel-only leaves (no U bit) and device
    /// frames are left untouched; a normal user leaf gets rmap removal,
    /// refcount drop (last ref frees) and its PTE cleared. Returns true if
    /// a leaf was torn down.
    ///
    /// Used by copy_old_to_new_pages when the mremap destination holds a
    /// page mapped by a racing same-mm fault between the destination VMA's
    /// MAP_FIXED install and the PTE copy — Linux's mremap moves PTEs
    /// atomically under mmap_lock and the move REPLACES whatever sits in
    /// the destination; skipping the page (the old MREMAP-DST-LIVE guard)
    /// permanently lost the source data.
    pub fn unmap_one_user_leaf_locked(&self, va: usize) -> bool {
        let Some((ppn_val, pte_bits)) = PageTableWalker::walk(self.pgd, va as u64) else {
            return false;
        };
        if pte_bits & PageTableEntry::U == 0 {
            return false; // kernel translation overlapped in the walk
        }
        // Device frames (virtio-gpu framebuffer) are not RAM pages, and
        // neither is any other boot-reserved (kernel-heap) frame — the
        // drm GEM dumb-buffer backing. Clear the PTE only.
        {
            let phys = (ppn_val as u64) << 12;
            let fb = crate::console::get_framebuffer_info()
                .map_or(false, |i| phys >= i.addr && phys < i.addr + i.size as u64);
            if fb || crate::mm::page_alloc::is_foreign_user_frame(phys as usize) {
                self.clear_pte(va as u64);
                return true;
            }
        }
        use crate::mm::page_desc::page_desc;
        // RO alias: mapcount-only teardown (cow_flags::PCACHE_RO).
        let leaf_is_ro = pte_bits & cow_flags::PCACHE_RO != 0;
        if let Some(page) = page_desc(ppn_val as usize) {
            if pte_bits & cow_flags::PCACHE == 0 {
                page.rmap_alt_remove(
                    self as *const _ as usize,
                    va / PAGE_SIZE_USIZE,
                );
            }
            if page.is_mapped() {
                crate::mm::rmap::page_remove_rmap(page);
            }
            if !leaf_is_ro {
                let new_ref =
                    page.put_page_site(crate::debug::driftwatch::S_UNMAP_X86);
                if new_ref == 0 {
                    // Cache-backed leaf: leak, never hand the live cache
                    // frame to the zone (pcachewedge lane).
                    if pte_bits & cow_flags::PCACHE != 0 {
                        crate::mm::page_alloc::pcache_leaf_phantom_zero_put(
                            (ppn_val as usize) << PAGE_SHIFT,
                            "mremap-dst",
                        );
                    } else {
                        crate::mm::page_alloc::free_pages(
                            (ppn_val as usize) << PAGE_SHIFT, 0,
                        );
                    }
                }
                // PCACHE mapping: drop the pin after the descriptor put (same
                // ordering as unmap_pages).
                if pte_bits & cow_flags::PCACHE != 0 {
                    release_cache_mapping_pin(ppn_val as usize);
                }
            }
        }
        self.clear_pte(va as u64);
        self.sub_rss(1);
        true
    }

    /// Clear page table entry at specified virtual address (4-level walk)
    fn clear_pte(&self, virt: u64) {
        let a = VirtAddr::new(virt);
        let vpn4 = a.pgd_index() as usize;
        let vpn3 = a.pud_index() as usize;
        let vpn2 = a.pmd_index() as usize;
        let vpn1 = a.pte_index() as usize;

        let Some(root_table) = PtTable::from_phys(self.pgd << PAGE_SHIFT) else {
            return;
        };
        let pte4 = root_table.get(vpn4);
        if !pte4.is_valid() {
            return;
        }

        let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else {
            return;
        };
        let pte3 = table3.get(vpn3);
        // A 1GB leaf means no user page was ever mapped at this VA (user
        // maps over a leaf demote it first) — nothing to clear, and
        // treating the leaf's frame as a table would write into device
        // memory.
        if !pte3.is_valid() || pte3.is_leaf() {
            return;
        }

        let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else {
            return;
        };
        let pte2 = table2.get(vpn2);
        // Same rule for a still-megapaged 2MB leaf.
        if !pte2.is_valid() || pte2.is_leaf() {
            return;
        }

        let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
            return;
        };

        // E8-MM: if this VA sits inside a kernel identity device window
        // (ECAM/IOAPIC/LAPIC), the PTE being cleared may cover a device
        // page the user mapped over (leaf demotion made that possible —
        // Linux MAP_FIXED replace semantics). RESTORE the device
        // translation instead of writing 0: the kernel accesses these
        // registers from trap context on the CURRENT CR3 (LAPIC EOI in
        // IRQ entry), and a hole here is a ring-0 page fault — KERNPANIC.
        // Ordinary user VAs (the None case) clear to 0 as before.
        let new_bits = super::mmu_init::kernel_device_window_pte(virt).unwrap_or(0);
        table1.set(vpn1, PageTableEntry::from_bits(new_bits));
    }


}

/// Convert permission to page table flags.
///
/// x86 encoding: PRESENT implies read; RW gives write; exec is DEFAULT and
/// non-exec is expressed with NX (the mirror image of riscv, where R/X are
/// positive bits). The NX bit is therefore set unless the permission
/// includes exec.
pub fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
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
    /// Walk to find the physical page for `virt`. Total for any inputs:
    /// the root and every intermediate frame pass the stale-frame guard
    /// (the inline guards the raw walk carried), so a garbage root or
    /// recycled intermediate returns None like any unmapped address.
    pub fn walk(user_root_ppn: u64, virt: u64) -> Option<(u64, u64)> {
        let a = VirtAddr::new(virt);

        // Root guard: a caller whose mm was freed under it (unpinned
        // procfs borrow) feeds a garbage root ppn — the very first
        // get_page_table_virt then dies in phys_to_virt OVERFLOW
        // (exitwalk probe: generate_smaps -> walk with root phys
        // 0xf888000000000000). Bail with the same bounded forensic line.
        {
            let root_pa = user_root_ppn << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published()
                && root_pa != 0
                && !crate::mm::phys_valid(root_pa as usize)
            {
                static ROOT_STALE: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if ROOT_STALE.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "PT-WALK-STALE: root={:#x} va={:#x} ROOT phys {:#x} off-machine, pid={}",
                        user_root_ppn, virt, root_pa,
                        crate::sched::get_current_pid()
                    );
                }
                return None;
            }
        }
        let mut table = PtTable::from_phys(user_root_ppn << PAGE_SHIFT)?;

        // PML4 → PUD → PMD
        for level in (1..=3u8).rev() {
            let pte = table.get(a.vpn(level) as usize);
            if !pte.is_valid() {
                return None;
            }
            if level >= 1 && pte.is_leaf() {
                // Huge page: PUD leaf (level 2) = 1GB, PMD leaf
                // (level 1) = 2MB. Both levels must take this branch:
                // the direct map is built from 2MB PMD leaves, and the
                // old `level > 1` guard excluded them — a PMD leaf was
                // descended into as if it were a page table (reading
                // the mapped page's own bytes as PTEs), so every
                // kernel-half walk past the first 4K-mapped 2MB
                // returned None-or-garbage (the mpk tag gap: walk=None
                // for DmaBlock/vring VAs; convicted by a raw PML4/PUD/
                // PMD dump — PMD[0x1e2]=leaf). The shift is
                // PAGE_SHIFT + 9*level (PMD 21, PUD 30): the old
                // `level - 1` form computed 2MB for a 1GB leaf and
                // returned a wrong phys for VAs deeper than 2MB into
                // it.
                let shift = PAGE_SHIFT + 9 * (level as u64);
                let offset = virt & ((1 << shift) - 1);
                let phys = pte.phys_addr() + offset;
                return Some((phys >> PAGE_SHIFT, pte.bits()));
            }
            // Stale-intermediate guard (exitwalk probe, PT-teardown vs
            // walkers): a walker that reaches an intermediate entry whose
            // target is not machine memory is walking a freed/recycled
            // (or never-table) frame. Feeding it to get_page_table_virt
            // used to die in phys_to_virt OVERFLOW (bogus phys
            // 0xf888000000000000, PageTableWalker::walk ->
            // get_page_table_virt, full-machine panic). Bail out with a
            // bounded forensic line that names the walk inputs so the
            // racing walker/lifetime gap is attributable, and return None
            // like any unmapped address.
            {
                let pa = pte.phys_addr();
                if crate::mm::page_desc::descs_published() && pa != 0 && !crate::mm::phys_valid(pa as usize) {
                    static WALK_STALE: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    if WALK_STALE.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                        crate::pr_err!(
                            "PT-WALK-STALE: root={:#x} va={:#x} lvl={} pte={:#x} -> phys {:#x} off-machine, pid={}",
                            user_root_ppn, virt, level, pte.bits(), pa,
                            crate::sched::get_current_pid()
                        );
                    }
                    return None;
                }
            }
            table = PtTable::from_phys(pte.phys_addr())?;
        }

        let pte = table.get(a.vpn(0) as usize);
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

    // K1-BAR: from this point on, new kernel-half PML4 entries installed
    // in the static root (per-BAR ioremap arena) are invisible to mm
    // trees forked earlier — they copied the kernel-half links at their
    // own creation. PCI probe must finish before the first user mm;
    // ioremap_bar_kernel reports a violation loudly.
    super::mmu_init::mark_user_mms_exist();

    let root_ppn = root_page / PAGE_SIZE;
    if let Some(root_table) = PtTable::from_phys(root_page) {
        root_table.zero_entries();
    }

    // ALWAYS clone from the static kernel root — root_page_table_ppn()
    // reads CR3, which is a USER root while another task is current.
    let kernel_ppn = get_kernel_page_table_ppn();
    copy_kernel_mappings(root_ppn, kernel_ppn);

    // Fixmap lives in the kernel half (PML4[511]) on x86 — already
    // shared by the link copy above; nothing to do (riscv64 needs the
    // explicit copy because its fixmap sits below KERNEL_PGD_START).
    // (No-op on x86; the fixmap lane owns its conversion.)
    unsafe { super::fixmap::copy_fixmap_to_user(root_ppn) };

    Some(root_ppn)
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
fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    let kernel_phys = kernel_root_ppn * PAGE_SIZE;
    let user_phys = user_root_ppn * PAGE_SIZE;

    let kernel_table = PtTable::from_phys(kernel_phys);
    let Some(user_table) = PtTable::from_phys(user_phys) else { return };

    user_table.zero_entries();

    // ---- Low half (PML4[0..256]): EMPTY — pure user mappings ----
    // The E8-MM low-identity clone used to live here (device windows
    // copied leaf-only into every mm so trap-context MMIO worked under
    // user CR3s). That put supervisor mappings inside the user half,
    // where fork copies, demand fills and teardown walked them as user
    // tables — the x86 text-scribbler family (a demoted identity window
    // freed one incarnation, reused as file data, then parsed as page
    // tables by the next walk). Trap-context MMIO now goes through the
    // KERNEL-HALF device alias (setup_device_mappings), which the shared
    // kernel-half links below already provide under every CR3.

    // ---- Kernel-space PML4 entries (256..512): shared links ----
    if let Some(kernel_table) = kernel_table {
        for i in KERNEL_PGD_START..PTRS_PER_PGD as usize {
            let pte = kernel_table.get(i);
            if pte.is_valid() {
                user_table.set(i, pte);
            }
        }
    }

    fence(Ordering::SeqCst);
}

/// Map user page
///
/// Returns false when the mapping could not be installed (page-table
/// allocation failure — failible map_page, fake-OOM family).
pub fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    map_page(user_root_ppn, user_virt, phys, flags)
}

/// Global PTE-modification lock (interface parity with riscv64)
///
/// Leaf-PTE exclusion epoch (INV-LOCK-7; type `GlobalPteLock`, see
/// mm/mm_struct.rs and docs/architecture/lock-ordering.md).
///
/// x86_64 runs in SPLIT mode: leaf-PTE writers that can reach the owning
/// mm take that mm's `PteLock` (epoch SHARED + per-mm spin) via
/// `pte_lock_for(mm)` — writers of different mms run concurrently, which
/// is what breaks the fork-walk CPU monopoly. Holders of THIS static
/// (epoch EXCLUSIVE) keep the old whole-domain one-writer-at-a-time
/// semantics: the ipc install paths (binder / sysv_shm), `map_user_region`
/// (exec/fresh-table/device cold paths), and kernel-root maps.
///
/// Direct users must NOT be reachable while any per-mm PTE section is
/// held by the same CPU (write-inside-read self-deadlocks) — the old
/// global-Spinlock discipline forbade the same nestings, so no live path
/// regresses.
pub static PTE_MODIFY_LOCK: crate::mm::mm_struct::GlobalPteLock =
    crate::mm::mm_struct::GlobalPteLock::new();

/// Per-mm leaf-PTE section hook (INV-LOCK-7): every site that can reach
/// the owning mm locks through here instead of PTE_MODIFY_LOCK.
#[inline]
pub fn pte_lock_for(mm: &crate::mm::mm_struct::MmStruct) -> &crate::mm::mm_struct::PteLock {
    mm.pte_lock()
}

/// map_page caller-context marker (corruption hunt, advisory): set right
/// before the unchecked map helpers run so the PTE-OVERWRITE-LIVE
/// tripwire can name the site. Values:
/// 1 shmat attach, 2 mmap(/dev/fb0), 3 mmap MAP_SHARED|ANON eager,
/// 4 mremap copy, 5 ashrem/ashmem pin, 6 brk growth, 7 anon demand fault,
/// 8 file/shm demand fault, 9 swap-in. 0 = unset.
pub static MM_CTX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ==================== Leaf primitives for shared mm code (FD-1) ====================
//
// mm/rmap.rs (try_to_unmap_inner) and mm/compact.rs (remap_page) used to
// carry Sv39 manual descents of their own — root indexed with bits 38:30
// — which on this 4-level arch read the WRONG PML4 slot, failed
// is_valid(), and silently no-oped: swap-out unmap reclaimed nothing and
// compaction migration always aborted. The walk shape (PML4 → PUD → PMD
// → PTE, bits 47:39..20:12) and the PTE encoding are arch facts and live
// here; the shared mm code calls only these three primitives.

/// Permission/flag bits of a 4K leaf PTE — everything except the PFN
/// field: P|RW|US|PWT|PCD|A|D|PAT|G, the COW/PCACHE software bit 9, and
/// NX (bit 63 — dropping it would widen a migrated NX mapping back to
/// executable). This is the exact bit set [`leaf_pte_install`] puts
/// back.
#[inline]
pub fn pte_flag_bits(bits: u64) -> u64 {
    (bits & 0x3FF) | (bits & PageTableEntry::NX)
}

/// Default flag bits for an anonymous user page — compaction's fallback
/// when the pre-unmap walk could not recover the live PTE. Present, RW,
/// user, accessed, dirty, NX (the demand-fault path's anon default:
/// exec's per-segment W^X keeps anon data non-exec).
pub const PTE_FLAGS_ANON_DEFAULT: u64 = PageTableEntry::P | PageTableEntry::RW
    | PageTableEntry::US | PageTableEntry::A | PageTableEntry::D
    | PageTableEntry::NX;

/// Replace the 4K leaf PTE at `vaddr` with raw `new_bits` (0 = unmap; a
/// swap or migration entry — a non-present leaf may carry arbitrary
/// software bits, the extable/fault path decodes them), under this mm's
/// PTE lock. The leaf must still be present and map `expect_pfn` (the
/// R7-C3 re-validation: the caller's lock-free walk may have raced a
/// concurrent leaf writer). Returns true when the leaf was replaced; the
/// caller owns the TLB flush.
///
/// A large-page leaf at an upper level has no 4K slot to replace —
/// returns false (anonymous reclaim targets 4K mappings only).
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub fn leaf_pte_replace(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    expect_pfn: u64,
    new_bits: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    let vpn4 = ((vaddr >> 39) & 0x1FF) as usize;
    let vpn3 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn2 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 12) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte4 = root_table.get(vpn4);
    if !pte4.is_valid() {
        return false;
    }
    let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte3 = table3.get(vpn3);
    if !pte3.is_valid() || pte3.is_leaf() {
        return false;
    }
    let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = table2.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }
    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() || pte1.ppn() != expect_pfn {
        return false;
    }
    table1.set(vpn1, PageTableEntry::from_bits(new_bits));
    // hp89 hunt: rmap-driven leaf replacement (reclaim/migration) —
    // record for post-hoc #GP attribution (mm/unwatch).
    crate::mm::unwatch::note(
        crate::mm::unwatch::KIND_LEAF_REPLACE,
        vaddr,
        new_bits,
        expect_pfn,
    );
    true
}

/// Rebuild a PRESENT 4K leaf mapping `pfn` at `vaddr` from saved flag
/// bits ([`pte_flag_bits`] shape), under this mm's PTE lock. The leaf
/// slot may currently hold a non-present marker (the compaction
/// migration window) — only the intermediate levels must exist. The
/// present bit is OR-ed in unconditionally (the saved bits of a live
/// leaf carry it; the fallback constant does too — belt for the marker
/// case). Returns true when installed; the caller owns the TLB flush.
///
/// # Safety
/// `root_ppn` must be a live page-table root of `mm` (pinned by the
/// caller), and `vaddr` a user address of that mm.
pub fn leaf_pte_install(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    vaddr: u64,
    pfn: u64,
    flags: u64,
) -> bool {
    let _g = pte_lock_for(mm).lock_irqsave();
    let vpn4 = ((vaddr >> 39) & 0x1FF) as usize;
    let vpn3 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn2 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 12) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte4 = root_table.get(vpn4);
    if !pte4.is_valid() {
        return false;
    }
    let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte3 = table3.get(vpn3);
    if !pte3.is_valid() || pte3.is_leaf() {
        return false;
    }
    let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = table2.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }
    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let new_bits = (pfn << 12) | pte_flag_bits(flags) | PageTableEntry::P;
    table1.set(vpn1, PageTableEntry::from_bits(new_bits));
    true
}

/// Map a user region: `phys_start` pages mapped at `virt_start` in
/// lockstep over `size` bytes.
///
/// NOTE (interface deviation, deliberate): the bring-up stub had
/// `map_user_region(root, virt, size, phys: Option<PhysAddr>, flags) ->
/// bool`; every generic caller (exec.rs, syscall/memory.rs, io_uring)
/// compiles against the riscv64 shape used here instead. Signature
/// changed to match the compiled-against arch interface.
pub fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
    // GLOBAL-mode section (INV-LOCK-7): this helper only receives the
    // root PPN, so it cannot reach the owning mm's PteLock. Its callers
    // are cold paths (exec building a fresh unpublished tree, kernel-root
    // windows, one-shot device maps), so whole-domain exclusion is the
    // right trade here.
    let _pte_guard = PTE_MODIFY_LOCK.lock();
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
pub fn alloc_and_map_user_memory(
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
        super::mmu_init::zero_owned_pages(virt_addr_ptr.bits(), block_pages * PAGE_SIZE as usize);

        // PRESERVE PRESENT LEAVES (Linux do_brk_flags only maps ABSENT
        // pages). A brk shrink that failed to unmap (its munmap error is
        // ignored), or any stale leaf left in the growth window, used to
        // be blindly re-mapped with the fresh zero block — destroying
        // whatever still lived there (PTE-OVERWRITE-LIVE burst
        // 0x502000-0x519000 in gnome-shell's heap; the [pango] FcInit
        // abort/SIGSEGV). Skip pages that already have a present
        // translation; free the block's skipped pages back.
        {
            let mut skipped = 0usize;
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for i in 0..chunk_pages {
                let va = virt_addr + ((mapped_pages + i) * PAGE_SIZE as usize) as u64;
                let present = PageTableWalker::walk(user_root_ppn, va)
                    .map_or(false, |(_p, bits)| {
                        bits & PageTableEntry::P != 0
                    });
                if present {
                    crate::mm::page_alloc::free_pages(
                        ((base_pfn + i) as usize) << PAGE_SHIFT,
                        0,
                    );
                    skipped += 1;
                    continue;
                }
                map_user_region(
                    user_root_ppn,
                    va,
                    ((base_pfn + i) as u64) << PAGE_SHIFT,
                    PAGE_SIZE as u64,
                    flags,
                );
            }
            let _ = skipped;
        }

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
pub fn alloc_and_map_to_kernel_table(
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
    super::mmu_init::zero_owned_pages(virt_addr_ptr.bits(), alloc_size);

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
///
/// Chunk sizing is BINARY DECOMPOSITION (abi-review session 2, twin of
/// the riscv64 implementation): each iteration takes the largest
/// power-of-two block that FITS in the remaining request (floor), never
/// a rounded-up block whose excess must be freed back. The old ceil-pow2
/// path zeroed the whole rounded block (128 KB zeroed for a 76 KB image,
/// 256 KB for a 132 KB stack — measured as the #1 exec cost center,
/// perf-report 8.10.23 'map') and paid one zone-locked order-0 free per
/// excess page (12 for a 19-page image, 31 for a 33-page stack). A
/// 33-page request maps as 32+1 — two blocks, NOT contiguous; only the
/// page-table walk may address the range.
pub fn alloc_and_map_to_user_table(
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
        // floor(log2(remain)), capped at MAX_ORDER; block_pages <= remain
        // always, so chunk == block and no excess exists.
        let order = ((usize::BITS - 1 - remain.leading_zeros()) as usize)
            .min(crate::mm::zone::MAX_ORDER);
        let block_pages = 1usize << order;
        let chunk_pages = remain.min(block_pages);
        let chunk_bytes = chunk_pages * PAGE_SIZE as usize;

        let phys_addr = alloc_pages(GfpFlags::GFP_USER, order);
        if phys_addr == 0 {
            return None; // all-or-nothing; the RAII guard frees partial tables
        }

        // Zero BEFORE mapping (same leak-window fix): the frames come from
        // the buddy and may carry a previous owner's data. With floor-pow2
        // chunks every allocated page is mapped, so zeroing chunk_bytes
        // zeroes the whole block — the old block_pages zero covered the
        // rounded-up excess too, pure waste.
        let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
        super::mmu_init::zero_owned_pages(virt_addr_ptr.bits(), chunk_bytes);

        map_user_region(
            user_ppn,
            virt_addr + (mapped_pages * PAGE_SIZE as usize) as u64,
            phys_addr as u64,
            chunk_bytes as u64,
            user_flags,
        );

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

    /// This PTE maps a PAGE-CACHE frame directly (mm filemap_fault, x86_64)
    /// and OWNS one page-cache pin (`CachedPage.ref_count`) plus one page
    /// descriptor reference for its mapping lifetime. Carried through
    /// mprotect rewrites and PROT_NONE markers like COW; dropped (with the
    /// matching `page_cache::put_by_key`) at unmap, mm teardown, and the
    /// COW break; duplicated (with `pin_by_key` + get_page) by the fork
    /// walk. Session 25: MAP_SHARED cache mappings carry it TOO (with the
    /// PCACHE_SHARED bit below) — the old riscv-style permanent pin is
    /// gone; every alias owns its pin, so unmap bounds alias memory by
    /// live mappings instead of forever.
    pub const PCACHE: u64 = 1 << 10;
    /// Session 25 (MAP_SHARED writeback): this PCACHE leaf belongs to a
    /// MAP_SHARED VMA and maps the cache frame WRITABLE — stores go
    /// straight into the frame (mapper coherence by construction). Unlike
    /// a private cache leaf, a W-upgrade (mprotect/marker re-present) must
    /// grant W IN PLACE instead of forcing the COW bit: a COW copy would
    /// fork a private frame, splitting the mapping from the cache and from
    /// every other mapper. Carried (preserved) through PTE rewrites like
    /// PCACHE itself; fork's cow_exempt (SHARED VMAs) copies it as-is.
    pub const PCACHE_SHARED: u64 = 1 << 11;
    /// Pin-free read-only alias of a page-cache frame (perf-gap-hunt
    /// 2026-11-04): a PCACHE leaf created by the SHARED read fault-around
    /// batch. It holds NEITHER a page-cache pin NOR a page-descriptor
    /// reference — only the descriptor mapcount (inc at install, dec at
    /// unmap). Eviction and write(2)-invalidation safety come from the
    /// mapcount: page-cache reclaim skips frames whose mapcount > 0 (the
    /// same shape as the reference kernel's page_mapped() check — one
    /// atomic per PTE instead of a pin + refcount round trip, which is
    /// what makes 512-page fault-around windows affordable). Always
    /// carried TOGETHER with PCACHE (identity consumers key on PCACHE;
    /// this bit selects the release discipline). mprotect rewrites
    /// preserve it like PCACHE; a W-grant treats it like PCACHE_SHARED
    /// (in-place W + dirty mark — the leaf is a SHARED-VMA form). Fork
    /// copies it with inc_mapcount only (no pin, no get_page). Bit 52 is
    /// above the 52-bit PHYS_MASK and below the swap-signature bits —
    /// software-available on x86-64 paging for any MAXPHYADDR ≤ 52.
    pub const PCACHE_RO: u64 = 1 << 52;
}

/// Drop the page-cache pin owned by ONE mapping of a cache-backed frame
/// (the PTE carried `cow_flags::PCACHE`). The descriptor's mapping/index
/// fields carry the page-cache eviction keys (set by cache insert); this
/// must run AFTER the descriptor's `put_page_site` so the frame's
/// refcount is already back at the cache's base reference before the pin
/// drop — which may release and free an invalidated entry — decides the
/// frame's fate. No-op when the cache entry is already gone (floored put
/// discipline, R14-16).
pub(super) fn release_cache_mapping_pin(ppn: usize) {
    use crate::mm::page_desc::page_desc;
    let Some(page) = page_desc(ppn) else { return };
    // Descriptor of a live frame that was mapped until this unmap;
    // mapping/index are only cleared by the cache's own release path,
    // which cannot run while a pin (held until this call) exists.
    let key = page.mapping() as u64;
    let index = page.index() as u64;
    if key != 0 {
        crate::fs::page_cache::get_page_cache().put_by_key(key, index);
    }
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
pub fn copy_page_table_cow(
    parent: &crate::mm::mm_struct::MmStruct,
    cow_exempt: &[(u64, u64)],
    vma_ranges: &[(u64, u64)],
) -> Option<u64> {
    let parent_root_ppn = parent.root_ppn();
    // NOTE (launcher wall fix, session-3, updated for per-mm sections):
    // the walk runs CHUNKED — one PTE section per leaf table (vpn2
    // iteration), with cond_resched() at every chunk boundary. The parent
    // chunk takes the PARENT mm's PteLock (INV-LOCK-7): per-PTE atomicity
    // vs demand faults, COW faults, mprotect/munmap of THIS mm is
    // preserved (every same-mm leaf writer takes the same per-mm spin);
    // faults of OTHER mms no longer queue behind the walk — that
    // cross-mm serialization was the fork-walk CPU monopoly. Structural
    // safety is unchanged: the parent mm is Arc-pinned by the forking
    // task (teardown cannot run) and intermediate tables are never
    // unlinked mid-life. The caller (AddressSpace::fork) holds NO lock
    // across the walk.
    use crate::mm::page_desc::page_desc;

    // forkprof scalars (armed check hoisted out of the PTE loop).
    let fp_count = crate::debug::forkprof::on();

    // Framebuffer frames (virtio-gpu scanout backing store) are
    // device-shared memory and must NEVER be COW-marked — mirror of the
    // riscv64 twin (see its long comment for the failure history).
    let fb_frame_range: Option<(u64, u64)> = crate::console::get_framebuffer_info()
        .map(|i| (i.addr >> 12, (i.addr + i.size as u64 + 0xFFF) >> 12));

    if parent_root_ppn == 0 {
        return None;
    }

    let child_root_phys = alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    let parent_root_phys = parent_root_ppn << PAGE_SHIFT;
    // Structural liveness (the Arc-pinned parent + per-chunk PTE locks);
    // the stale-frame guard adds the recycled-frame containment.
    let parent_root = PtTable::from_phys_live(parent_root_phys)?;
    let child_root = PtTable::from_phys_live(child_root_phys)?;

    for vpn4 in 0..512usize {
        let pte4 = parent_root.get(vpn4);

        if !pte4.is_valid() {
            continue;
        }

        // Kernel half (PML4[256..512]): share the link directly. These
        // entries are never freed per-mm, so no refcount games are needed.
        if vpn4 >= KERNEL_PGD_START {
            child_root.set(vpn4, pte4);
            continue;
        }

        // User half (PML4[0..256], incl. the cloned identity subtree):
        // walk and copy with COW marking.
        let ppn3 = pte4.ppn();

        let child_pud_phys = alloc_page_table()?;
        let child_ppn3 = child_pud_phys >> PAGE_SHIFT;
        // Child links inherit the parent link's USER bit — x86 enforces
        // U/S at every walk level (see map_page_noflush's link_user).
        child_root.set(
            vpn4,
            PageTableEntry::from_bits(
                PageTableEntry::new_table(child_ppn3).bits() | (pte4.bits() & PageTableEntry::US),
            ),
        );

        let parent_pud = PtTable::from_phys_live(ppn3 << PAGE_SHIFT)?;
        let child_pud_ref = PtTable::from_phys_live(child_pud_phys)?;

        for vpn3 in 0..512usize {
            let pte3 = parent_pud.get(vpn3);

            if !pte3.is_valid() {
                continue;
            }

            // 1GB leaf: kernel device/identity leaves (U=0) are shared
            // as-is; user 1GB leaves (never created by this port) bump
            // the refcount like any shared user page. Foreign
            // (boot-reserved, kernel-heap GEM/fb) frames are copied
            // as-is with NO refcount — see is_foreign_user_frame.
            if pte3.is_leaf() {
                if pte3.is_user()
                    && !crate::mm::page_alloc::is_foreign_user_frame(
                        (pte3.ppn() as usize) << PAGE_SHIFT,
                    )
                {
                    if let Some(page) = page_desc(pte3.ppn() as usize) {
                        page.get_page();
                        page.inc_mapcount();
                    }
                }
                child_pud_ref.set(vpn3, pte3);
                continue;
            }

            let ppn2 = pte3.ppn();

            let child_pmd_phys = alloc_page_table()?;
            let child_ppn2 = child_pmd_phys >> PAGE_SHIFT;
            child_pud_ref.set(
                vpn3,
                PageTableEntry::from_bits(
                    PageTableEntry::new_table(child_ppn2).bits() | (pte3.bits() & PageTableEntry::US),
                ),
            );

            let parent_pmd = PtTable::from_phys_live(ppn2 << PAGE_SHIFT)?;
            let child_pmd_ref = PtTable::from_phys_live(child_pmd_phys)?;

            for vpn2 in 0..512usize {
                let pte2 = parent_pmd.get(vpn2);

                if !pte2.is_valid() {
                    continue;
                }

                // ONE-LEAF-TABLE CHUNK under the PARENT's per-mm PTE
                // section (launcher wall fix, session-3; per-mm split,
                // INV-LOCK-7): the walk no longer holds one global
                // section for the whole parent tree — that monopolized a
                // CPU (preempt disabled) for the multi-second walk of a
                // large mm and blocked every other mm's faults behind it.
                // Exclusion still holds where it must:
                // - leaf-PTE atomicity (this table's reads/writes vs
                //   demand faults, COW faults, mprotect/munmap on the
                //   parent): the per-chunk parent PteLock, which every
                //   same-mm leaf writer also takes;
                // - structure: intermediate tables are never unlinked
                //   mid-life (only free_user_page_tables frees them, and
                //   the parent mm is Arc-pinned by the forking task, so
                //   its Drop cannot run during the walk); faults only
                //   ADD tables, and an index-ordered traversal reads
                //   additions either before or after the chunk that
                //   covers them — both consistent.
                {
                    // ONE-LEAF-TABLE CHUNK under the PARENT mm's PTE
                    // section (INV-LOCK-7) — see the walk-head comment.
                    let _pte_guard = pte_lock_for(parent).lock();

                    // (fork-frag lane) RE-READ the PMD entry under the
                    // chunk's lock: munmap now UNLINKS emptied leaf
                    // tables (pt_free_emptied_table) and the pte2 above
                    // was read WITHOUT the lock — between that read and
                    // this acquire, a concurrent munmap on the parent
                    // (another thread of the mm) may have cleared the
                    // entry and queued the frame for a deferred free.
                    // The re-read turns that race into "entry invalid,
                    // skip": this walk never dereferences an unlinked
                    // table. Upper levels (PML4/PUD/PMD tables) are
                    // never unlinked mid-life — only the PMD ENTRY is
                    // cleared — so pte3/pte4 staleness stays the benign
                    // add-only shape the walk always relied on.
                    let pte2 = parent_pmd.get(vpn2);
                    if !pte2.is_valid() {
                        continue;
                    }

                    // 2MB leaf: share (device/identity leaves, or user huge
                    // pages with a refcount bump). Foreign (boot-reserved,
                    // kernel-heap GEM/fb) frames are copied as-is with NO
                    // refcount — see is_foreign_user_frame.
                    if pte2.is_leaf() {
                        if pte2.is_user()
                            && !crate::mm::page_alloc::is_foreign_user_frame(
                                (pte2.ppn() as usize) << PAGE_SHIFT,
                            )
                        {
                            if let Some(page) = page_desc(pte2.ppn() as usize) {
                                page.get_page();
                                page.inc_mapcount();
                            }
                        }
                        child_pmd_ref.set(vpn2, pte2);
                    } else {
                        let ppn1 = pte2.ppn();

                        let child_pt_phys = alloc_page_table()?;
                        let child_ppn1 = child_pt_phys >> PAGE_SHIFT;
                        if fp_count {
                            crate::debug::forkprof::bump(crate::debug::forkprof::S_COW_TABLES, 1);
                        }
                        child_pmd_ref.set(
                            vpn2,
                            PageTableEntry::from_bits(
                                PageTableEntry::new_table(child_ppn1).bits() | (pte2.bits() & PageTableEntry::US),
                            ),
                        );

                        let parent_pt = PtTable::from_phys_live(ppn1 << PAGE_SHIFT)?;
                        let child_pt_ref = PtTable::from_phys_live(child_pt_phys)?;

                        // (sysmm-2x lane) VMA-RANGE-BOUNDED leaf scan —
                        // same shape as free_user_page_tables: by the
                        // mapping invariant only VMA-covered indices can
                        // hold present PTEs. A fragmented parent (munmap
                        // leaves emptied leaf tables behind) paid 512
                        // streamed loads per table here — the mm_cow
                        // 20 µs phase of the SMP-8 fork decomposition.
                        // A table with no covering VMA still allocates
                        // its child frame (the child links it, keeping
                        // the tree shape) and keeps the full defensive
                        // scan; >8 intersecting ranges likewise.
                        let mut scan_windows: [(usize, usize); 8] = [(0, 0); 8];
                        let mut n_windows = 0usize;
                        {
                            let wleaf_base = (vpn4 as u64) << 39
                                | (vpn3 as u64) << 30
                                | (vpn2 as u64) << 21;
                            let wleaf_end = wleaf_base + (1u64 << 21);
                            for &(rs, re) in vma_ranges {
                                let s = rs.max(wleaf_base);
                                let e = re.min(wleaf_end);
                                if e <= s {
                                    continue;
                                }
                                if n_windows == scan_windows.len() {
                                    n_windows = 0; // too fragmented: full scan
                                    break;
                                }
                                scan_windows[n_windows] = (
                                    ((s - wleaf_base) >> 12) as usize,
                                    ((e - wleaf_base) >> 12) as usize,
                                );
                                n_windows += 1;
                            }
                            if n_windows == 0 {
                                scan_windows[0] = (0, 512);
                                n_windows = 1;
                            }
                        }
                        for &(win_lo, win_hi) in &scan_windows[..n_windows] {
                        for vpn1 in win_lo..win_hi {
                            // Per-PTE atomicity vs demand/COW faults and
                            // mprotect/munmap: every access to this leaf
                            // table happens under the chunk's
                            // PTE_MODIFY_LOCK above.
                            let pte1 = parent_pt.get(vpn1);

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
                    // Leaf-table fast path (perf review 2026-10-31): when
                    // this whole 2MB leaf table lies outside every exempt
                    // region and the framebuffer range, the per-PTE scans
                    // below cannot match — skip them. One range check per
                    // table replaces one scan per PTE; tables overlapping
                    // a region keep the exact per-PTE granularity.
                    let leaf_base = leaf_va & !0x1F_FFFF;
                    let leaf_end = leaf_base + 0x20_0000;
                    let leaf_all_clear = cow_exempt
                        .iter()
                        .all(|(s, e)| leaf_end <= *s || leaf_base >= *e)
                        && fb_frame_range.map_or(true, |(lo, hi)| {
                            leaf_end <= (lo << 12) || leaf_base >= (hi << 12)
                        });
                    let cow_exempt_leaf = !leaf_all_clear
                        && (cow_exempt
                            .iter()
                            .any(|(s, e)| leaf_va >= *s && leaf_va < *e)
                            || fb_frame_range.map_or(false, |(lo, hi)| {
                                pte1.ppn() >= lo && pte1.ppn() < hi
                            }));

                    let new_pte = if is_user
                        && !crate::mm::page_alloc::is_foreign_user_frame(
                            (pte1.ppn() as usize) << PAGE_SHIFT,
                        )
                    {
                        // Foreign (boot-reserved, kernel-heap GEM/fb) frames
                        // take no refcount and never COW-mark — the else
                        // branch copies the PTE as-is (the mapping is
                        // inherited unchanged; the owning subsystem frees
                        // the memory).
                        let phys_ppn = pte1.ppn() as usize;
                        page_desc(phys_ppn).map_or(pte1, |page| {
                            // RO alias (cow_flags::PCACHE_RO): the child's
                            // copy is mapcount-only TOO — no pin to
                            // duplicate, no descriptor ref to take; the
                            // inc_mapcount below is the child's whole
                            // claim on the frame (eviction safety).
                            let leaf_is_ro =
                                pte1.bits() & cow_flags::PCACHE_RO != 0;
                            // PCACHE leaf (filemap_fault): the child's copy
                            // owns its own pin — without this, the parent's
                            // unmap would drop the LAST pin and eviction
                            // could free a frame the child still maps.
                            if !leaf_is_ro && pte1.bits() & cow_flags::PCACHE != 0 {
                                let key = page.mapping() as u64;
                                let index = page.index() as u64;
                                if !crate::fs::page_cache::get_page_cache()
                                    .pin_by_key(key, index)
                                {
                                    // Entry vanished while a pin existed —
                                    // cache/frame bookkeeping drift. The
                                    // mapping still works (frame is pinned
                                    // by the parent's not-yet-dropped pin);
                                    // do not COW-mark so the child's write
                                    // takes the normal COW copy path.
                                }
                            }

                            // Increment refcount for shared user pages (one ref per sharer)
                            if !leaf_is_ro {
                                page.get_page();
                            }
                            page.inc_mapcount();

                            if is_writable && !cow_exempt_leaf {
                                // COW: mark both parent and child PTEs as read-only
                                page.set_flag(crate::mm::page_desc::PageFlag::Cow);

                                let cow_pte_bits = pte1.bits() & !PageTableEntry::W | cow_flags::COW;

                                parent_pt.set(vpn1, PageTableEntry::from_bits(cow_pte_bits));

                                PageTableEntry::from_bits(cow_pte_bits)
                            } else {
                                pte1
                            }
                        })
                    } else {
                        pte1
                    };

                    child_pt_ref.set(vpn1, new_pte);
                    // forkprof scalar: PTEs copied (hoisted armed check —
                    // one relaxed load when off).
                    if fp_count {
                        crate::debug::forkprof::bump(crate::debug::forkprof::S_COW_PTES, 1);
                    }
                    pte_install_log(
                        child_root_ppn,
                        leaf_va,
                        new_pte.ppn(),
                    );
                        }
                        } // (sysmm-2x) end VMA-range-bounded leaf scan window
                    } // else (non-leaf) — end of leaf-table processing
                } // end one-leaf-table chunk (PTE lock released)

                // Chunk boundary: no lock is held and the parent mm stays
                // Arc-pinned by the caller, so the walk state is stable.
                // cond_resched() yields ONLY when task context + fully
                // preemptible (it no-ops under any lock/bh/rcu section) —
                // with need_resched set this is what breaks the CPU
                // monopoly of a large-mm fork walk.
                crate::sched::cond_resched();
            }
        }
    }

    // SMP coherence: the walk above DOWNGRADED the parent's writable PTEs
    // to read-only+COW. A remote CPU's stale WRITABLE entries would bypass
    // COW and land in the frame now shared with the child (a silently LOST
    // private write), and stale reads resurrect pre-fork page contents.
    // PTE permission changes on a live mm require a TLB shootdown on ALL
    // CPUs that ever ran the mm (Linux flush_tlb_mm shape): IPI exactly
    // those peers (mask-filtered, 2026-11-12), full-flush locally.
    crate::arch::ipi::flush_tlb_others_mm(parent.tlb_cpu_mask(), 0, 0);
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

/// Handle copy-on-write page fault (4-level walk under the faulting mm's
/// PTE section, INV-LOCK-7). `mm` must be the address space that owns
/// `root_ppn`.
pub fn handle_cow_fault(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    fault_addr: VirtAddr,
) -> CowFaultResult {
    // S7-oom01wedge (x86 twin of the riscv64 fix): the private-copy
    // alloc_pages(GFP_USER) used to run INSIDE this PTE section — the
    // allocator slowpath under the PTE epoch starves global PTE writers
    // and same-mm sections on the other hart for the slowpath's full
    // duration (fleet s2 dump: 100+ s strands; riscv64 twin carried the
    // observed wedges). Linux discipline (wp_page_copy): allocate the
    // private copy BEFORE the PTE lock; every no-copy outcome here
    // already returns Retry/Resolved, so a wasted page is just freed.
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

    let _pte_guard = pte_lock_for(mm).lock();
    use crate::mm::page_desc::page_desc;

    let virt_addr = fault_addr.bits();

    let a = VirtAddr::new(virt_addr);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    // Re-walk under the section; a guard miss returns the same Retry the
    // restructured-tree arms below produce.
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return CowFaultResult::Retry;
    };
    let pte4 = root_table.get(vpn4);
    if !pte4.is_valid() {
        // Walk failure at re-check time: the upper levels were restructured
        // (teardown/fork) after handle_mm_fault's lock-free COW check.
        return CowFaultResult::Retry;
    }

    let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else {
        return CowFaultResult::Retry;
    };
    let pte3 = table3.get(vpn3);
    if !pte3.is_valid() || pte3.is_leaf() {
        return CowFaultResult::Retry;
    }

    let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else {
        return CowFaultResult::Retry;
    };
    let pte2 = table2.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return CowFaultResult::Retry;
    }

    let table1_phys = pte2.ppn() << PAGE_SHIFT;
    let Some(table1) = PtTable::from_phys(table1_phys) else {
        return CowFaultResult::Retry;
    };

    let old_pte = table1.get(vpn1);
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
    let old_page = page_desc(old_ppn as usize);

    let refcount = match old_page {
        Some(p) => p.refcount(),
        None => 1,
    };

    // If refcount <= 1, we're the only owner - just enable write.
    // EXCEPT a cache-backed leaf: the page cache holds the frame's base
    // reference for as long as the entry lives, so refcount 1 on a
    // PCACHE leaf means the MAPPING's reference is missing (a phantom
    // put consumed it) — enabling W in place would (a) let MAP_PRIVATE
    // stores land in the shared cache frame and (b) arm the unmap path
    // to zero-put the frame the cache still owns. Force the copy path;
    // the copy's put then just exposes the drift (pcachewedge lane).
    if refcount <= 1 && old_bits & cow_flags::PCACHE == 0 {
        let new_pte = PageTableEntry::from_bits(
            (old_bits & !cow_flags::COW) | PageTableEntry::W | PageTableEntry::P
        );

        // the page is exclusively owned now — clear the descriptor flag
        // too, or stats/invariants see "COW set with refcount 1" (R7-C9).
        if let Some(p) = old_page {
            p.clear_flag(crate::mm::page_desc::PageFlag::Cow);
        }

        table1.set(vpn1, new_pte);

        // SMP coherence: the fast-path PTE replacement (RO+COW → RW) must
        // be visible to EVERY CPU that may hold the old entry — the task
        // can migrate, and a remote stale entry turns the retry into a
        // permanent fault or serves stale page contents. Shoot the entry
        // down on the peers that ever loaded this mm (mask-filtered,
        // 2026-11-12; the remote handler takes no locks, so doing this
        // under PTE_MODIFY_LOCK cannot deadlock).
        crate::arch::ipi::flush_tlb_others_mm(mm.tlb_cpu_mask(), virt_addr, virt_addr + PAGE_SIZE);
        crate::arch::cpu::invlpg(virt_addr);

        return CowFaultResult::Resolved;
    }

    // NOTE: put_page() on the old page is deferred until AFTER the copy and
    // PTE update. Calling it earlier opens a race window: the child on
    // another CPU could see refcount drop to 1, re-enable W without
    // copying, then exec and free the page — all before this side finishes
    // copying from it.

    // Allocate new page and copy content. The page came from the
    // speculative pre-lock alloc (S7-oom01wedge): the allocator slowpath
    // must never run inside this PTE section. Sole consumer.
    let new_phys = match spec.0.take() {
        Some(p) => p,
        None => {
            return CowFaultResult::OutOfMemory;
        }
    };
    let new_ppn = new_phys >> PAGE_SHIFT;

    // Copy while old page is still pinned (refcount >= 2)
    super::r#unsafe::copy_page(old_ppn << PAGE_SHIFT, new_phys);

    // Preserve V/U/G/A/D and the exec state (NX); add W; drop COW.
    let flags = (old_bits & (PageTableEntry::P | PageTableEntry::U | PageTableEntry::GLOBAL
        | PageTableEntry::ACCESSED | PageTableEntry::DIRTY | PageTableEntry::NX))
        | PageTableEntry::W;
    let new_pte = PageTableEntry::from_bits((new_ppn << PAGE_SHIFT) | flags);

    // Install new PTE before dropping our reference to the old page
    table1.set(vpn1, new_pte);

    // SMP coherence: this replacement (old frame → private copy) is exactly
    // the stale-entry hazard — without a shootdown, a CPU the task migrated
    // from keeps translating the VA onto the OLD frame, splitting the
    // task's memory view. Mask-filtered shootdown (2026-11-12): only the
    // CPUs that ever loaded this mm can hold the old entry.
    crate::arch::ipi::flush_tlb_others_mm(mm.tlb_cpu_mask(), virt_addr, virt_addr + PAGE_SIZE);
    crate::arch::cpu::invlpg(virt_addr);

    // Now safe to release our share of the old page.
    // Both refcount and mapcount must be released: the mapping no longer
    // points to the old shared page. Without dec_mapcount the count leaks
    // across COW cycles and eventually corrupts page reclaim / rmap
    // decisions.
    if let Some(p) = old_page {
        p.put_page_site(crate::debug::driftwatch::S_COW_X86);
        p.dec_mapcount();
    }

    // PCACHE old leaf (filemap_fault private mapping): the mapping moved to
    // the private copy — this mapping's cache pin goes with it. Same
    // put-after-descriptor-put ordering as unmap_pages.
    if old_bits & cow_flags::PCACHE != 0 {
        release_cache_mapping_pin(old_ppn as usize);
    }

    // Set up reverse mapping for the new COW page
    {
        use crate::mm::page_desc::PageFlag;
        let new_page = page_desc(new_ppn as usize);
        if let Some(np) = new_page {
            np.set_flag(PageFlag::Anonymous);
            // SwapBacked + LRU membership: the COW copy is an ordinary
            // anonymous page and must stay reclaimable (vmscan).
            np.set_flag(PageFlag::SwapBacked);
            np.set_index(virt_addr as usize / (PAGE_SIZE as usize));
            np.inc_mapcount();
        }
        // Multi-mapping bookkeeping + RSS: the copy is a new resident page
        // of the faulting address space (the old page remains resident only
        // for its other owners).
        if let Some(mm) = crate::sched::current().and_then(|t| t.address_space()) {
            if let Some(np) = new_page {
                crate::mm::rmap::page_record_mapping(
                    np,
                    mm as *const _ as usize,
                    virt_addr as usize,
                );
                crate::api::mm::page_add_anon_lru(np);
            }
            mm.add_rss(1);
        }
    }

    CowFaultResult::Resolved
}

/// Check if page is a COW page
pub fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let a = VirtAddr::new(addr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte4 = root_table.get(vpn4);

    if !pte4.is_valid() {
        return false;
    }

    // Leaf levels: a leaf (2MB/1GB) is never a COW PTE (COW only exists
    // on 4KB user leaves; the identity/device windows map as leaves).
    let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte3 = table3.get(vpn3);

    if !pte3.is_valid() || pte3.is_leaf() {
        return false;
    }

    let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = table2.get(vpn2);

    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }

    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);

    if !pte1.is_valid() {
        return false;
    }

    (pte1.bits() & cow_flags::COW) != 0
}

/// Check if page has required permissions: (read, write, exec, user)
pub fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let a = VirtAddr::new(addr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
    let pte4 = root_table.get(vpn4);

    if !pte4.is_valid() {
        return None;
    }

    let table3 = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT)?;
    let pte3 = table3.get(vpn3);

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

    let table2 = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT)?;
    let pte2 = table2.get(vpn2);

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

    let table1 = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)?;
    let pte1 = table1.get(vpn1);

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

/// mprotect PTE rewrite for x86_64: set the protection bits of every leaf
/// PTE in [start_virt, start_virt + num_pages * PAGE_SIZE).
///
/// The shared syscall code previously carried an Sv39-shaped 3-level walk
/// (`vpn2/vpn1/vpn0`, `ppn << 10`): on the 4-level x86_64 table that
/// indexed the wrong tables, "rebuilt" entries with the riscv PTE layout,
/// and clobbered PMD link entries — while the real 4K leaf PTEs were left
/// untouched. Every mprotect silently no-opped on the leaves, so a
/// PROT_READ|PROT_WRITE JIT mapping later upgraded with
/// mprotect(PROT_READ|PROT_EXEC) stayed NX and the first jump into the
/// code died with `pagefault: Permission denied, rip == fault_addr`
/// (gnome-shell: SpiderMonkey/llvmpipe executable pages).
///
/// x86 encoding notes (vs the riscv64 twin):
/// - readable is implied by PRESENT; non-exec is the NX bit (bit 63), not
///   a missing X bit;
/// - PROT_NONE is expressed as P=0 plus an NX|U|A|D software marker:
///   non-zero (not a hole), bits 61/62 clear (not a swap/migration
///   entry) — any access faults and the VMA layer raises SIGSEGV. The
///   PPN is preserved so a later re-protect restores the same page
///   instead of the demand-fault path installing a fresh zero page over
///   live data;
/// - the COW software bit (bit 9) is preserved and W stays deferred to
///   the COW fault, matching the fork protocol (INV-COW-1).
///
/// Caller must hold PTE_MODIFY_LOCK; swap/migration entries and huge
/// leaves are left alone.
pub fn mprotect_pte_range(root_ppn: u64, start_virt: u64, num_pages: u64, prot: u32) {
    // Base flag set for a present leaf. W adds RW; non-exec sets NX.
    // Readable needs no bit (P implies it); PROT_WRITE without
    // PROT_READ still gets P (W-requires-R fold, like the fault path).
    let mut base = PageTableEntry::P | PageTableEntry::U
        | PageTableEntry::A | PageTableEntry::D;
    if prot & 0x2 != 0 {
        base |= PageTableEntry::W;
    }
    if prot & 0x4 == 0 {
        base |= PageTableEntry::NX;
    }
    // PROT_NONE: drop PRESENT, keep the software marker bits.
    if prot & 0x7 == 0 {
        base &= !PageTableEntry::P;
    }

    // REVERTED hoisted-walk attempt (gb-opt lane, 2026-11-15): resolving
    // the upper chain once per 2 MB block measured 2.36 µs → ~0.4 µs on
    // the mm-probe D3 shape, but wedged the machine 4/4 boots (soft
    // lockup inside handle_timer_tick, "not servicing IPIs", garbage pc)
    // exactly at this section, while the pre-hoist kernel passed it —
    // the per-page walk apparently tolerates a concurrent upper-level
    // mutation the hoist does not (candidate mutators: tick-context
    // mm walkers). The per-page body below is the original; re-land the
    // hoist only after the mutator is named and the walk made safe
    // against it.
    for i in 0..num_pages {
        let va = start_virt.wrapping_add(i * PAGE_SIZE as u64);
        let a = VirtAddr::new(va);

        // Read-only walk of the upper levels through the typed handle;
        // a guard miss is a skip like any absent level.
        let Some(pml4) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
            continue;
        };
        let e4 = pml4.get(a.pgd_index() as usize);
        if !e4.is_valid() {
            continue;
        }
        let Some(pud) = PtTable::from_phys(e4.phys_addr()) else { continue };
        let e3 = pud.get(a.pud_index() as usize);
        if !e3.is_valid() || e3.is_leaf() {
            continue; // 1GB leaves never cover mprotect'able user 4K pages
        }
        let Some(pmd) = PtTable::from_phys(e3.phys_addr()) else { continue };
        let e2 = pmd.get(a.pmd_index() as usize);
        if !e2.is_valid() || e2.is_leaf() {
            continue; // 2MB leaves: kernel/device windows only — skip
        }
        let Some(pt) = PtTable::from_phys(e2.phys_addr()) else { continue };
        let e1 = pt.get(a.pte_index() as usize);
        let raw = e1.bits();
        // Not populated at all, or a swap/migration entry (P=0 with the
        // signature bits) — the latter must survive for swap-in.
        if raw == 0
            || crate::api::mm::is_swap_entry(raw)
            || crate::api::mm::is_migration_entry(raw)
        {
            continue;
        }
        // A P=0 non-swap leaf is our own PROT_NONE marker: still rewrite
        // (restoring P) while preserving its PPN.

        let cow_bit = raw & cow_flags::COW;
        // PCACHE/PCACHE_SHARED/PCACHE_RO are mapping-ownership bits (this
        // PTE owns a page-cache pin — or, for PCACHE_RO, holds the
        // mapcount-only read-only alias), not protection bits — they must
        // survive protection rewrites exactly like the COW bit, or a later
        // unmap/COW would leak the pin (frame unevictable forever).
        let pcache_bit =
            raw & (cow_flags::PCACHE | cow_flags::PCACHE_SHARED | cow_flags::PCACHE_RO);
        let mut new_bits = (raw & PageTableEntry::PHYS_MASK_PUBLIC) | base;
        if cow_bit != 0 {
            new_bits |= cow_bit;
            new_bits &= !PageTableEntry::W; // W deferred to the COW fault
        }
        if pcache_bit != 0 {
            new_bits |= pcache_bit;
            if pcache_bit & (cow_flags::PCACHE_SHARED | cow_flags::PCACHE_RO) != 0 {
                // SHARED cache leaf (session 25): W is granted IN PLACE —
                // stores must land in the cache frame (every mapper + the
                // writeback path observe them). A COW copy here would fork
                // a private frame and silently detach this mapping from
                // the file's shared state. An upgrade that GRANTS W is a
                // W-install (the leaf faulted under an RO VMA): mark the
                // cache page dirty (conservative v1) so a later msync/fsync
                // persists stores made through it — fault-path installs
                // only mark dirty for leaves that faulted under a W VMA.
                if new_bits & PageTableEntry::W != 0 && raw & PageTableEntry::W == 0 {
                    crate::fs::page_cache::get_page_cache()
                        .mark_dirty_pfn(e1.ppn() as usize);
                }
            } else {
                // A (private) cache-backed leaf NEVER gets W in place: the
                // frame is shared with the page cache (and possibly other
                // mappers). A W-upgrade on a leaf that faulted under a
                // read-only VMA (no COW bit yet) must divert the first
                // write through COW like every other private file page
                // (Linux writenotify).
                if new_bits & PageTableEntry::W != 0 {
                    new_bits |= cow_flags::COW;
                    new_bits &= !PageTableEntry::W;
                }
            }
        }
        pt.set(a.pte_index() as usize, PageTableEntry::from_bits(new_bits));

        // Invalidates the TLB entry for the rewritten leaf so the next
        // access observes the new permissions.
        crate::arch::cpu::invlpg(va);
    }
}
