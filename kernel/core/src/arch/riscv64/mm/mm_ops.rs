//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Memory Management Operations
//!
//! This module contains:
//! - MmStruct extension methods for address space management
//! - User space mapping operations
//! - Copy-on-Write (COW) support
//! - mmap/munmap implementations
//!
//! # Safety Invariants — COW Page Table Protocol
//!
//! - **INV-COW-1**: When the COW bit is set in a PTE, the PTE `W` (write) bit
//!   must be clear. The page is shared read-only.
//!
//! - **INV-COW-2**: When the COW bit is set, the page's `_refcount >= 2`
//!   (shared by at least two mappings, e.g. parent + child after fork).
//!
//! - **INV-COW-3**: On a COW fault, if `_refcount == 1` (all other sharers
//!   have released), restore the `W` bit directly without copying.
//!
//! - **INV-COW-4**: On a COW fault, if `_refcount > 1`, allocate a new page,
//!   copy contents, decrement the old page's refcount, and map the new page
//!   with `W=1, COW=0`.
//!
//! - **INV-COW-5**: After `fork`, every writable PTE in the child must be
//!   downgraded to read-only (`W=0`) with `COW=1` set, and the page's
//!   refcount incremented.

use core::arch::asm;
use core::sync::atomic::{fence, Ordering};

extern crate alloc;
use alloc::vec::Vec;

use super::memory_layout::*;
use super::r#unsafe::PtTable;
use super::mmu_init::*;
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::vma::{Vma, VmaFlags, VmaOrigin, VmaType};
use crate::mm::{MmStruct, alloc_pages, GfpFlags};

// Re-export AddressSpace for backward compatibility
pub use crate::mm::AddressSpace;

// ==================== MmStruct Extension Methods ====================

impl MmStruct {
    /// Enable this address space (switch page table)
    pub fn enable(&self) {
        let satp = Satp::sv39(self.pgd, 0);
        // SAFETY: satp CSR write + TLB fence (register-level effects; the
        // root PPN is this live mm's own).
        unsafe {
            asm!("csrw satp, {}", in(reg) satp.bits());
            asm!("sfence.vma zero, zero");
        }
    }

    /// Disable address space (switch to bare mode)
    pub fn disable() {
        let satp = Satp::new(Satp::MODE_BARE, 0, 0);
        // SAFETY: bare-mode satp write + TLB fence.
        unsafe {
            asm!("csrw satp, {}", in(reg) satp.bits());
            asm!("sfence.vma zero, zero");
        }
    }

    /// Flush entire TLB
    pub fn flush_tlb() {
        // SAFETY: TLB fence (memory-safe in any context).
        unsafe { asm!("sfence.vma zero, zero") };
    }

    /// Flush TLB for specified page
    pub fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        // SAFETY: TLB fence (memory-safe in any context).
        unsafe { asm!("sfence.vma {}, zero", in(reg) vaddr.as_usize()) };
    }

    // ==================== VMA Operations ====================


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
                crate::api::mm::page_add_anon_lru(&*page);
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
    pub fn unmap_pages(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        // Serialize against fork's table walk and COW faults (per-mm PTE
        // lock; other mms' writers proceed concurrently).
        let _pte_guard = pte_lock_for(self).lock_irqsave();
        let mut addr = start.as_usize();
        let end = addr + size;

        while addr < end {
            // SAFETY: self.pgd is a valid root PPN and addr is page-aligned within the range
            // being unmapped from this address space.
            let ppn = unsafe { PageTableWalker::walk(self.pgd, addr as u64) };

            // Swap entry (V=0 leaf): no resident page — free the swap
            // slot and clear the PTE. Without this, munmap of a swapped
            // page leaks the slot for the lifetime of the swap area.
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
                // translation the user VA range overlaps (device windows
                // cloned into every root, or one RESTORED by clear_pte
                // after the user mapping over it was unmapped). It is not
                // this mm's page: no rmap entry, no refcount, no PTE to
                // clear — freeing or punching it corrupts kernel MMIO
                // access (also closes the pre-existing hole where
                // munmap over an untouched cloned MMIO page zeroed the
                // kernel's PTE).
                if pte_bits & PageTableEntry::U == 0 {
                    addr += PAGE_SIZE_USIZE;
                    continue;
                }
                // Device frames (virtio-gpu framebuffer) are not RAM pages,
                // and neither is any other boot-reserved (kernel-heap)
                // frame — the drm GEM dumb-buffer backing. Never
                // put_page/free them (accounting corruption otherwise).
                {
                    let phys = (ppn_val as u64) << 12;
                    let fb = crate::console::get_framebuffer_info()
                        .map_or(false, |i| phys >= i.addr && phys < i.addr + i.size as u64);
                    if fb || crate::mm::page_alloc::is_foreign_user_frame(phys as usize) {
                        // SAFETY: addr is a page-aligned user address whose
                        // leaf was verified above.
                        unsafe { self.clear_pte(addr as u64); }
                        continue;
                    }
                }
                // Remove reverse mapping before clearing PTE, then drop this
                // mapping's reference; the last reference frees the page.
                use crate::mm::page_desc::page_desc;
                if let Some(page) = page_desc(ppn_val as usize) {
                    // Multi-mapping bookkeeping (review 4.10): drop this
                    // (mm, vpn) from the page's alternate rmap slots.
                    page.rmap_alt_remove(
                        self as *const _ as usize,
                        addr / PAGE_SIZE_USIZE,
                    );
                    // SIGILL-stamp adjudicated release (gnome-sigill):
                    // stamp lifecycle, put_page, and the foreign-free
                    // refusal/refusal-report in one place.
                    crate::arch::mm::mmu_init::release_user_leaf(
                        page,
                        ppn_val as usize,
                        addr,
                    );
                    // PCACHE mapping (filemap_fault, riscv64 port): drop
                    // this mapping's page-cache pin AFTER the descriptor
                    // put inside release_user_leaf — the pin's release may
                    // free an invalidated frame, so every reference must
                    // be gone by then (x86_64 twin ordering).
                    if pte_bits & cow_flags::PCACHE != 0 {
                        release_cache_mapping_pin(ppn_val as usize);
                    }
                }
                self.clear_pte(addr as u64);
                // RSS accounting (review 4.14): one resident page leaves
                // this address space.
                self.sub_rss(1);
            }

            addr += PAGE_SIZE_USIZE;
        }

        // SAFETY: sfence.vma invalidates the entire TLB; required after clearing PTEs so
        // subsequent accesses use the updated page table.
        unsafe {
            asm!("sfence.vma zero, zero");
        }

        // Remote shootdown (review 4.10): the sfence above only flushed the
        // LOCAL hart. Other CPUs may still hold cached translations of the
        // pages just unmapped — a stale hit reads/writes freed frames.
        // One broadcast per batch unmap; the IPI handler performs a full
        // flush, so pending-bit coalescing cannot lose a request.
        crate::arch::ipi::flush_tlb_others();

        Ok(())
    }

    /// Teardown of ONE present user leaf at `va`, for callers that already
    /// hold PTE_MODIFY_LOCK (mremap's page-move copy). Mirrors the per-page
    /// body of unmap_pages: kernel-only leaves (no U bit) and device
    /// frames are left untouched; a normal user leaf gets rmap removal,
    /// the adjudicated put_page release (last ref frees) and its PTE
    /// cleared. Returns true if a leaf was torn down.
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
        if let Some(page) = page_desc(ppn_val as usize) {
            page.rmap_alt_remove(
                self as *const _ as usize,
                va / PAGE_SIZE_USIZE,
            );
            // SIGILL-stamp adjudicated release (same as unmap_pages).
            crate::arch::mm::mmu_init::release_user_leaf(
                page,
                ppn_val as usize,
                va,
            );
            // PCACHE mapping: drop the pin after the descriptor put (same
            // ordering as unmap_pages; x86_64 twin).
            if pte_bits & cow_flags::PCACHE != 0 {
                release_cache_mapping_pin(ppn_val as usize);
            }
        }
        self.clear_pte(va as u64);
        self.sub_rss(1);
        true
    }

    /// Clear page table entry at specified virtual address
    fn clear_pte(&self, virt: u64) {
        let vpn2 = ((virt >> 30) & 0x1FF) as usize;
        let vpn1 = ((virt >> 21) & 0x1FF) as usize;
        let vpn0 = ((virt >> 12) & 0x1FF) as usize;

        let Some(root_table) = PtTable::from_phys(self.pgd << PAGE_SHIFT) else {
            return;
        };
        let pte2 = root_table.get(vpn2);
        if !pte2.is_valid() {
            return;
        }

        let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
            return;
        };
        let pte1 = table1.get(vpn1);
        // A still-megapaged L1 leaf means no user page was ever mapped at
        // this VA (user maps over a megapage demote it first) — nothing to
        // clear, and treating the leaf's frame as an L0 table would write
        // into MMIO memory.
        if !pte1.is_valid() || pte1.is_leaf() {
            return;
        }

        let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
            return;
        };

        // E8-MM: if this VA sits inside a kernel low-half device window
        // (PLIC/ECAM/PCI-MMIO/UART/...), the PTE being cleared may cover
        // a device page the user mapped over (megapage demotion made that
        // possible — Linux MAP_FIXED replace semantics). RESTORE the
        // device translation instead of writing 0: the kernel accesses
        // these registers from traps on the CURRENT satp (PLIC claim in
        // IRQ entry), and a hole here is an S-mode page fault —
        // KERNPANIC. Ordinary user VAs (the None case) clear to 0 as
        // before.
        let new_bits = super::mmu_init::kernel_device_window_pte(virt).unwrap_or(0);
        table0.set(vpn0, PageTableEntry::from_bits(new_bits));
    }


}

/// Convert permission to page table flags
pub fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
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

// ==================== User Address Space Management ====================

pub struct PageTableWalker;

impl PageTableWalker {
    /// Walk page table to find physical page number for virtual address
    /// Returns (ppn, full_pte_value) for debugging
    /// Handles both 4KB pages and 2MB huge pages
    pub fn walk(user_root_ppn: u64, virt: u64) -> Option<(u64, u64)> {
        let virt_addr = VirtAddr::new(virt);

        let vpn2 = virt_addr.vpn(2) as usize;
        let vpn1 = virt_addr.vpn(1) as usize;
        let vpn0 = virt_addr.vpn(0) as usize;

        // Root guard (x86_64 twin): a caller whose mm was freed under it
        // (unpinned procfs borrow) feeds a garbage root ppn — bail with a
        // bounded forensic line instead of dying in phys_to_virt OVERFLOW.
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
        let root_table = PtTable::from_phys(user_root_ppn << PAGE_SHIFT)?;

        let pte2 = root_table.get(vpn2);
        if !pte2.is_valid() {
            return None;
        }

        // Check for 1GB huge page at PGD level
        if pte2.is_leaf() {
            let offset = virt & 0x3FFFFFFF; // Lower 30 bits for 1GB page
            let phys_base = pte2.ppn() << PAGE_SHIFT;
            let ppn = (phys_base + offset) >> PAGE_SHIFT;
            return Some((ppn, pte2.bits()));
        }

        // Stale-intermediate guard (x86_64 twin: exitwalk probe — a walker
        // reaching an intermediate entry whose target is not machine
        // memory is walking a freed/recycled frame; bail with a bounded
        // forensic line instead of feeding garbage to get_page_table_virt).
        {
            let pa = pte2.ppn() << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published() && pa != 0 && !crate::mm::phys_valid(pa as usize) {
                static WALK_STALE: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if WALK_STALE.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "PT-WALK-STALE: root={:#x} va={:#x} lvl=2 pte={:#x} -> phys {:#x} off-machine, pid={}",
                        user_root_ppn, virt, pte2.bits(), pa,
                        crate::sched::get_current_pid()
                    );
                }
                return None;
            }
        }

        let ppn1 = pte2.ppn();
        let table1 = PtTable::from_phys(ppn1 << PAGE_SHIFT)?;
        let pte1 = table1.get(vpn1);
        if !pte1.is_valid() {
            return None;
        }

        // Check for 2MB huge page at PMD level
        if pte1.is_leaf() {
            // For 2MB huge page at PMD level, use the correct PPN extraction
            let ppn_2mb = pte1.ppn_for_2mb_page();
            // Physical address = {PPN_2MB, offset[20:0]}
            // where PPN_2MB is the 35-bit physical page number for 2MB pages
            let offset_2mb = virt & 0x1FFFFF; // Lower 21 bits
            let phys_addr = (ppn_2mb << 21) | offset_2mb;
            // Convert to 4KB PPN for the API
            let ppn_4kb = phys_addr >> PAGE_SHIFT;

            return Some((ppn_4kb, pte1.bits()));
        }

        // Stale-intermediate guard (see the lvl=2 note above).
        {
            let pa = pte1.ppn() << PAGE_SHIFT;
            if crate::mm::page_desc::descs_published() && pa != 0 && !crate::mm::phys_valid(pa as usize) {
                static WALK_STALE1: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if WALK_STALE1.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                    crate::pr_err!(
                        "PT-WALK-STALE: root={:#x} va={:#x} lvl=1 pte={:#x} -> phys {:#x} off-machine, pid={}",
                        user_root_ppn, virt, pte1.bits(), pa,
                        crate::sched::get_current_pid()
                    );
                }
                return None;
            }
        }

        let ppn0 = pte1.ppn();
        let table0 = PtTable::from_phys(ppn0 << PAGE_SHIFT)?;
        let pte0 = table0.get(vpn0);
        if !pte0.is_valid() {
            return None;
        }

        Some((pte0.ppn(), pte0.bits()))
    }
}

/// Allocate one page from the unified zone allocator
pub fn alloc_user_phys_page() -> Option<u64> {
    let phys = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys != 0 {
        Some(phys as u64)
    } else {
        None
    }
}

/// Create user address space
pub fn create_user_address_space() -> Option<u64> {
    let phys_addr = alloc_pages(GfpFlags::GFP_USER, 0);
    if phys_addr == 0 {
        return None;
    }

    // Validate physical address is within actual physical memory range
    // This prevents using addresses that are outside the system's physical memory
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

    if let Some(root_table) = PtTable::from_phys(root_page) {
        root_table.zero_entries();
    }

    let kernel_ppn = super::mmu_init::root_page_table_ppn();
    let root_ppn = root_page / PAGE_SIZE;
    copy_kernel_mappings(root_ppn, kernel_ppn);

    // Copy fixmap (UART) to user page table so console works after switch_mm
    // (fixmap lane owns its conversion; no-op contract documented there.)
    unsafe { super::fixmap::copy_fixmap_to_user(root_ppn) };

    Some(root_ppn)
}

/// Copy kernel mappings to user page table
///
/// NOTE (observed 2026-11-19): this inherits the device MMIO PGDs
/// VPN2[0..2] into every user root, so user VAs in 0x0..0x7FFF_FFFF
/// resolve to DEVICE 2MB leaves — a MAP_FIXED anon mapping placed there
/// silently aliases MMIO registers (PTE ppn=0x40000 = the virtio window)
/// instead of faulting in RAM. Anything mapping user memory must place
/// it at VPN2[3] or above.
///
/// This function copies PGD entries (pointers to L1 tables), NOT the L1 tables themselves.
/// All processes share the same kernel L1/L0 page tables.
///
/// We copy:
/// 1. MMIO PGD entries (VPN2[0..2]) for device access from kernel mode
///    when running with a user page table (e.g., during syscall handling)
/// 2. All kernel space PGD entries (VPN2 >= KERNEL_PGD_START = 256)
fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    let kernel_phys = kernel_root_ppn * PAGE_SIZE;
    let user_phys = user_root_ppn * PAGE_SIZE;

    let kernel_table = PtTable::from_phys(kernel_phys);
    let Some(user_table) = PtTable::from_phys(user_phys) else { return };

    user_table.zero_entries();

    // Copy MMIO PGD entries (VPN2[0] and VPN2[1])
    // These map device memory (PLIC, CLINT, UART, VirtIO, PCI MMIO) at their
    // physical addresses. Needed because kernel may access devices during syscalls.
    //
    // IMPORTANT: Do NOT directly copy the PGD entry, because the kernel's L1 table
    // for VPN2[0..1] may contain non-leaf entries pointing to user-space L0 tables
    // (from a previous process's mappings). Instead, create a new L1 table and
    // copy only entries that are safe (MMIO), skipping entries that belong to user space.
    let Some(kernel_table) = kernel_table else { return };
    for i in 0..2usize {
        let pte = kernel_table.get(i);
        if !pte.is_valid() {
            continue;
        }

        if pte.is_leaf() {
            // L2 leaf (1GB huge page) — safe to copy directly
            user_table.set(i, pte);
        } else {
            // Non-leaf: create new L1 table, copy only safe entries
            let kernel_l1_phys = pte.ppn() << PAGE_SHIFT;
            let Some(kernel_l1) = PtTable::from_phys(kernel_l1_phys) else { continue };

            if let Some(new_l1_phys) = super::mmu_init::alloc_page_table() {
                let Some(new_l1_virt) = PtTable::from_phys(new_l1_phys) else { continue };
                new_l1_virt.zero_entries();

                for j in 0..512 {
                    let l1_entry = kernel_l1.get(j);
                    if !l1_entry.is_valid() {
                        continue;
                    }

                    if l1_entry.is_leaf() {
                        // Leaf entry (huge page MMIO) — safe to copy
                        new_l1_virt.set(j, l1_entry);
                    } else {
                        // Non-leaf L1 entry: points to kernel L0 table.
                        // Must NOT share the kernel L0 table pointer directly because
                        // free_user_page_tables() would free it on process exit.
                        // Instead, create a new L0 table and copy only leaf entries.
                        let l0_phys = l1_entry.ppn() << PAGE_SHIFT;
                        let Some(l0_table) = PtTable::from_phys(l0_phys) else { continue };

                        if let Some(new_l0_phys) = super::mmu_init::alloc_page_table() {
                            let Some(new_l0_virt) = PtTable::from_phys(new_l0_phys) else { continue };
                            new_l0_virt.zero_entries();

                            for k in 0..512 {
                                let l0_entry = l0_table.get(k);
                                if l0_entry.is_valid() && l0_entry.is_leaf() {
                                    // Copy leaf entries (kernel MMIO pages, U=0)
                                    // Skip user entries (U=1) — they belong to another process
                                    if !l0_entry.is_user() {
                                        new_l0_virt.set(k, l0_entry);
                                    }
                                }
                            }

                            let new_l0_ppn = new_l0_phys >> PAGE_SHIFT;
                            new_l1_virt.set(j, PageTableEntry::new_table(new_l0_ppn));
                        }
                    }
                }

                let new_l1_ppn = new_l1_phys >> PAGE_SHIFT;
                user_table.set(i, PageTableEntry::new_table(new_l1_ppn));
            }
        }
    }

    // Copy kernel-space PGD entries (VPN2 >= KERNEL_PGD_START = 256)
    for i in KERNEL_PGD_START..PTRS_PER_PGD as usize {
        let pte = kernel_table.get(i);
        if pte.is_valid() {
            user_table.set(i, pte);
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

/// Map user region
///
/// Leaf-PTE exclusion epoch (INV-LOCK-7; type `GlobalPteLock`, see
/// mm/mm_struct.rs and docs/architecture/lock-ordering.md).
///
/// Split mode on all arches since the 2026-10-31 perf review: PTE sites
/// with a reachable `MmStruct` take that mm's `PteLock` via
/// `pte_lock_for(mm)` (global epoch SHARED + per-mm spin — writers on
/// different mms run concurrently). The DIRECT `PTE_MODIFY_LOCK`
/// (exclusive) users below are the whole-domain minority:
/// `map_user_region`, kernel-root/unpublished-tree maps — sites that
/// cannot reach the owning mm and must exclude against everyone.

/// map_page caller-context marker (advisory, interface parity with
/// x86_64's MM_CTX): arch-neutral sites (sysv_shm, mremap, the mmap
/// paths) store a small call-site id before the unchecked map helpers
/// run. On x86_64 the PTE-OVERWRITE-LIVE tripwire reads it to name the
/// site; on riscv64 no reader exists yet — the stores stay cheap and
/// inert.
pub static MM_CTX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub static PTE_MODIFY_LOCK: crate::mm::mm_struct::GlobalPteLock =
    crate::mm::mm_struct::GlobalPteLock::new();

/// Per-mm leaf-PTE section hook (INV-LOCK-7), GLOBAL mode on riscv64:
/// every mm maps to the same global epoch section, so arch-neutral
/// callers compile unchanged against the x86_64 shape.
#[inline]
pub fn pte_lock_for(
    mm: &crate::mm::mm_struct::MmStruct,
) -> &crate::mm::mm_struct::PteLock {
    // Split mode on all arches (perf review 2026-10-31): this mm's own
    // leaf-PTE writers serialize on the per-mm spin; writers of other
    // mms run concurrently; the global-mode users (map_user_region,
    // foreign IPC installs) still hold the epoch EXCLUSIVE and exclude
    // against everyone. See docs/architecture/lock-ordering.md.
    mm.pte_lock()
}

static PTE_GLOBAL_HANDLE: crate::mm::mm_struct::PteLock =
    crate::mm::mm_struct::PteLock::new();

// ==================== Leaf primitives for shared mm code (FD-1) ====================
//
// mm/rmap.rs (try_to_unmap_inner) and mm/compact.rs (remap_page) used to
// carry Sv39 manual descents of their own — root indexed with bits 38:30,
// Sv39 PTE encoding — which silently no-oped on the 4-level arches (wrong
// slot, is_valid() false, unmap/migration aborted, swap-out reclaimed
// nothing). The walk shape and the PTE encoding are arch facts and live
// here; the shared mm code calls only these three primitives.

/// Permission/flag bits of a 4K leaf PTE — everything except the PPN
/// field: V|R|W|X|U|G|A|D plus the COW/PCACHE software bits. This is the
/// exact bit set [`leaf_pte_install`] puts back.
#[inline]
pub fn pte_flag_bits(bits: u64) -> u64 {
    bits & 0x3FF
}

/// Default flag bits for an anonymous user page — compaction's fallback
/// when the pre-unmap walk could not recover the live PTE. Present, RW,
/// user, accessed, dirty; non-exec (the historical 0xD7 shape).
pub const PTE_FLAGS_ANON_DEFAULT: u64 = PageTableEntry::V | PageTableEntry::R
    | PageTableEntry::W | PageTableEntry::U | PageTableEntry::A
    | PageTableEntry::D;

/// Replace the 4K leaf PTE at `vaddr` with raw `new_bits` (0 = unmap; a
/// swap or migration entry), under this mm's PTE lock. The leaf must
/// still be present and map `expect_pfn` — the R7-C3 re-validation: the
/// caller's lock-free walk may have raced a concurrent leaf writer (COW
/// break / swap-in / munmap). Returns true when the leaf was replaced;
/// the caller owns the TLB flush.
///
/// A huge-leaf entry at an upper level has no 4K slot to replace —
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
    let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = root_table.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }
    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() || pte1.is_leaf() {
        return false;
    }
    let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte0 = table0.get(vpn0);
    if !pte0.is_valid() || pte0.ppn() != expect_pfn {
        return false;
    }
    table0.set(vpn0, PageTableEntry::from_bits(new_bits));
    true
}

/// Rebuild a PRESENT 4K leaf mapping `pfn` at `vaddr` from saved flag
/// bits ([`pte_flag_bits`] shape), under this mm's PTE lock. The leaf
/// slot may currently hold a non-present marker (the compaction
/// migration window) — only the intermediate levels must exist. The
/// valid bit is OR-ed in unconditionally (the saved bits of a live leaf
/// carry it; the fallback constant does too — belt for the marker case).
/// Returns true when installed; the caller owns the TLB flush.
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
    let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = root_table.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return false;
    }
    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() || pte1.is_leaf() {
        return false;
    }
    let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let new_bits = (pfn << 10) | (flags & 0x3FF) | PageTableEntry::V;
    table0.set(vpn0, PageTableEntry::from_bits(new_bits));
    true
}

pub fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
    // Serialize against fork's table walk (PTE lock coverage).
    let _pte_guard = PTE_MODIFY_LOCK.lock_irqsave();
    let virt_end_checked = virt_start.checked_add(size);
    if virt_end_checked.is_none() {
        panic!("map_user_region: virt_start + size overflow");
    }
    let virt_end_val = virt_end_checked.unwrap();

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);
    let virt_end = VirtAddr::new(virt_end_val);

    let mut virt = virt_start_addr.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        let virt_bits = virt.bits();
        let virt_start_bits = virt_start_addr.bits();
        let offset = virt_bits - virt_start_bits;
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        map_page(user_root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}

/// Allocate and map user memory
///
/// Serves the request with power-of-two buddy blocks capped at order 10
/// (4 MiB). A single block used to be allocated here even when smaller
/// than the request while `map_user_region` still mapped the FULL size:
/// every mapping larger than 4 MiB walked past the allocated block into
/// arbitrary neighbouring physical frames. A 16 MiB heap growth (glibc
/// raises its mmap threshold after large frees, so later big requests are
/// served from the main arena via one large sbrk) then memset over other
/// tasks' page tables — observed downstream as glibc "double free or
/// corruption (!prev)", bogus swap-in entries and random pagefault OOM
/// kills. Chunking the allocation keeps the mapped range inside pages we
/// own.
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
            // Keep the pre-failure all-or-nothing semantics of the old
            // single-block path: report failure; sys_brk then leaves the
            // break unchanged (chunks mapped so far are zeroed anonymous
            // pages that a retry re-maps over).
            return None;
        }

        // Zero BEFORE mapping: the pages are reachable through the linear
        // map regardless. Mapping first exposed stale page contents (old
        // freed data) to user space for the duration of the memset — an
        // information leak window on every execve.
        let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
        // SAFETY: fresh exclusive buddy block (INV-RE-ZP).
        unsafe {
            core::ptr::write_bytes(
                virt_addr_ptr.bits() as *mut u8,
                0,
                block_pages * PAGE_SIZE as usize,
            )
        };

        // PRESERVE PRESENT LEAVES (Linux do_brk_flags only maps ABSENT
        // pages — twin of the x86_64 fix): a stale leaf left in the brk
        // growth window (shrink whose munmap failed) must not be
        // re-mapped with the fresh zero block; that silently destroyed
        // live user data.
        {
            let base_pfn = phys_addr >> PAGE_SHIFT;
            for i in 0..chunk_pages {
                let va = virt_addr + ((mapped_pages + i) * PAGE_SIZE as usize) as u64;
                // SAFETY: user_root_ppn is a valid root and va is a
                // page-aligned user address; walk is read-only.
                let present = unsafe { PageTableWalker::walk(user_root_ppn, va) }
                    .map_or(false, |(_p, bits)| {
                        bits & PageTableEntry::V != 0
                    });
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
                // SAFETY: user_root_ppn valid; the target leaf is absent;
                // the frame is freshly zeroed and exclusively owned.
                unsafe {
                    map_user_region(
                        user_root_ppn,
                        va,
                        ((base_pfn + i) as u64) << PAGE_SHIFT,
                        PAGE_SIZE as u64,
                        flags,
                    );
                }
            }
        }

        // R7-C5: the rounded-up block allocated 2^order pages but the
        // mapping only covers chunk_pages — the unmapped excess has no
        // PTE, so no teardown path would ever free it (~192KB leaked per
        // execve; a shell loop drove the machine to OOM). Free the excess
        // as order-0 pages right away; buddy coalescing rebuilds larger
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

/// Allocate and map to kernel table
pub fn alloc_and_map_to_kernel_table(
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    if size == 0 { return None; }

    // Single contiguous block only (see alloc_and_map_to_user_table): a
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
    // SAFETY: fresh exclusive buddy block (INV-RE-ZP).
    unsafe { core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, alloc_size) };

    map_user_region(kernel_ppn, virt_addr, phys_addr as u64, size, user_flags);

    // R7-C5: the rounded-up block allocated 2^order pages but the mapping
    // only covers page_count — the unmapped excess has no PTE, so no
    // teardown path would ever free it (~192KB leaked per execve; a shell
    // loop drove the machine to OOM). Free the excess as order-0 pages
    // right away; buddy coalescing rebuilds larger blocks lazily. The
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

/// Allocate and map to user table
pub fn alloc_and_map_to_user_table(
    user_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // The buddy allocator tops out at MAX_ORDER (2^10 pages = 4 MiB).
    // The old single-block path allocated one order<=10 block and then
    // mapped `size` bytes from its base regardless: any request larger
    // than 4 MiB (exec images of big static binaries, a 5 MiB-BSS probe)
    // had its tail PTEs pointed PAST the block into frames still owned by
    // other users, and the exec loader's copy/BSS-zeroing then wrote
    // through them — cross-frame corruption, and a deterministic KERNPANIC
    // when the walk crossed the end of the linear map (pfault at exactly
    // phys 0x40000000, the 1 GiB boundary, 3/3 boots).
    //
    // Chunked allocation instead: each chunk is mapped within its own
    // block, exactly like alloc_and_map_user_memory (sys_brk). The
    // returned phys base is the FIRST block only — callers that need to
    // write the whole range must walk the page tables (see
    // exec.rs's image-copy helpers), not assume phys contiguity.
    //
    // Chunk sizing is BINARY DECOMPOSITION (abi-review session 2): each
    // iteration takes the largest power-of-two block that FITS in the
    // remaining request (floor), never a rounded-up block whose excess
    // must be freed back. The old ceil-pow2 path zeroed the whole rounded
    // block (128 KB zeroed for a 76 KB image, 256 KB for a 132 KB stack,
    // 2 MB for a 1.2 MB ld.so — measured as the #1 exec cost center,
    // perf-report 8.10.23 'map') and paid one zone-locked order-0 free
    // per excess page (12 for a 19-page image, 31 for a 33-page stack,
    // 212 for a 300-page interpreter). Floor-pow2 chunks have NO excess:
    // the zero covers exactly the mapped prefix, nothing is freed, and
    // smaller requests also succeed under fragmentation where the one
    // rounded-up request would fail. A 33-page request maps as 32+1 —
    // two blocks, NOT contiguous; only the page-table walk may address
    // the range.
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

        // Zero BEFORE mapping (same leak-window fix as
        // alloc_and_map_user_memory): the frames come from the buddy and
        // may carry a previous owner's data. With floor-pow2 chunks every
        // allocated page is mapped, so zeroing chunk_bytes zeroes the
        // whole block — the old block_pages zero covered the rounded-up
        // excess too, pure waste.
        let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
        // SAFETY: fresh exclusive buddy block (INV-RE-ZP).
        unsafe {
            core::ptr::write_bytes(
                virt_addr_ptr.bits() as *mut u8,
                0,
                chunk_bytes,
            )
        };

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
pub mod cow_flags {
    /// COW flag - page is marked as copy-on-write
    pub const COW: u64 = 1 << 8;  // Use bit 8 (after A and D)

    /// This PTE maps a PAGE-CACHE frame directly (mm filemap_fault, riscv64
    /// port of the x86_64 session-23 fix) and OWNS one page-cache pin
    /// (`CachedPage.ref_count`) plus one page-descriptor reference for its
    /// mapping lifetime. Sv39 RSW bit 1, beside the COW bit (RSW bit 0).
    /// Carried through mprotect rewrites and PROT_NONE markers like COW;
    /// dropped (with the matching `page_cache::put_by_key`) at unmap, mm
    /// teardown, and the COW break; duplicated (with `pin_by_key` +
    /// get_page) by the fork walk. Session 25: MAP_SHARED cache mappings
    /// carry it TOO — the riscv64 alias's PERMANENT pin is retired; every
    /// alias owns its pin, so unmap bounds alias memory by live mappings
    /// instead of forever.
    pub const PCACHE: u64 = 1 << 9; // RSW bit 1

    /// Session 25 (MAP_SHARED writeback): SHARED cache leaves (a MAP_SHARED
    /// VMA mapping its page-cache frame WRITABLE — stores go straight into
    /// the frame, mapper coherence by construction, and a W install marks
    /// the page dirty for msync/fsync writeback) are identified by
    /// PREDICATE, not by a dedicated PTE bit: Sv39 offers only the two RSW
    /// bits (COW, PCACHE) — bit 10 and up are the PPN field, and an earlier
    /// draft of this change defined `PCACHE_SHARED = 1 << 10`, which added
    /// 4 KiB to every shared mapping's PHYSICAL ADDRESS (PTE bits are
    /// `(ppn << 10) | flags`): each mapper wrote a DIFFERENT, wrong frame
    /// one page away from the cache page. The x86_64 twin keeps a real bit
    /// (AVL bit 11) because that arch has three software bits.
    ///
    /// The predicate: PRIVATE cache leaves ALWAYS carry COW — writable
    /// installs get it for the writenotify divert, and session 26 extends
    /// it to read-only installs so the property "PCACHE set and COW clear"
    /// holds EXACTLY for SHARED cache leaves (a W-upgrade on a RO-faulted
    /// private leaf can then still be told apart from a shared one without
    /// consulting the VMA). SHARED installs never set COW. Consumers:
    /// `pte_is_shared_cache(bits)` below — the mprotect/marker re-present
    /// sites grant W IN PLACE for shared leaves (a COW copy would fork a
    /// private frame, splitting the mapping from the cache and from every
    /// other mapper) and force the COW divert for private ones.
    pub fn pte_is_shared_cache(bits: u64) -> bool {
        bits & PCACHE != 0 && bits & COW == 0
    }
}

/// Drop the page-cache pin owned by ONE mapping of a cache-backed frame
/// (the PTE carried `cow_flags::PCACHE`; x86_64 twin in the x86 mm_ops).
/// The descriptor's mapping/index fields carry the page-cache eviction
/// keys (set by cache insert); this must run AFTER the descriptor's
/// `put_page_site` so the frame's refcount is already back at the cache's
/// base reference before the pin drop — which may release and free an
/// invalidated entry — decides the frame's fate. No-op when the cache
/// entry is already gone (floored put discipline, R14-16).
pub fn release_cache_mapping_pin(ppn: usize) {
    use crate::mm::page_desc::page_desc;
    let Some(page) = page_desc(ppn) else { return };
    // Descriptor of a live frame that was mapped until this unmap;
    // mapping/index are only cleared by the cache's own release path,
    // which cannot run while a pin (held until this call) exists.
    let key = page.mapping() as u64;
    let index = page.index() as u64;
    if key != 0 {
        crate::fs::hotpath::get_page_cache().put_by_key(key, index);
    }
}

/// Copy page table with COW marking
///
/// Kernel mappings (VPN2 >= KERNEL_PGD_START or U=0 entries) are shared by copying PGD entries.
/// User space mappings are copied with COW marking for writable pages.
///
/// `parent` supplies the walk's root (and, on split-mode arches, the
/// per-mm PTE section; riscv64 is GLOBAL mode — INV-LOCK-7).
pub fn copy_page_table_cow(
    parent: &crate::mm::mm_struct::MmStruct,
    cow_exempt: &[(u64, u64)],
) -> Option<u64> {
    let parent_root_ppn = parent.root_ppn();
    // NOTE: the whole walk runs under the caller's (AddressSpace::fork)
    // PTE section, serializing it against demand faults, COW faults,
    // and exec/unmap teardown. Per-leaf PTL granularity (Linux-style) is
    // still a TODO for SMP scalability.
    use crate::mm::page_desc::page_desc;

    // Framebuffer frames (virtio-gpu scanout backing store) are
    // device-shared memory and must NEVER be COW-marked. The VMA-range
    // exemption (cow_exempt) keys off the mapping's VMA flags/type, which
    // is a derived, fragile identity: any fork whose VMA state has lost
    // SHARED/Device downgrades the mapping's PTEs to RO+COW, and the very
    // next store from the fb owner (Xorg's fbdev driver) COW-faults into
    // a PRIVATE copy — from then on every pixel lands off-screen while
    // /dev/fb0 readers (fbmap-style mmap probes) see only the stale
    // pre-fork image, usually zeros (the fb0 read-regression family).
    // The PHYSICAL frame range is invariant, so key the exemption on it
    // as well. This mirrors Linux fbdev, which maps the scanout with
    // VM_IO|VM_SHARED semantics regardless of MAP_PRIVATE — writes from
    // any mapper always reach the device memory.
    let fb_frame_range: Option<(u64, u64)> = crate::console::get_framebuffer_info()
        .map(|i| (i.addr >> 12, (i.addr + i.size as u64 + 0xFFF) >> 12));

    if parent_root_ppn == 0 {
        return None;
    }

    let child_root_phys = alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    let parent_root_phys = parent_root_ppn << PAGE_SHIFT;
    // Structural liveness: the caller holds this mm's PTE section for the
    // whole walk (see the NOTE above); the stale-frame guard adds the
    // recycled-frame containment.
    let parent_root = PtTable::from_phys_live(parent_root_phys)?;
    let child_root = PtTable::from_phys_live(child_root_phys)?;

    for vpn2 in 0..512 {
        let pte2 = parent_root.get(vpn2);

        if !pte2.is_valid() {
            continue;
        }

        // Check if L2 is a leaf (gigapage)
        let is_l2_leaf = pte2.is_readable() || pte2.is_writable() || pte2.is_executable();

        // Kernel region (VPN2 >= KERNEL_PGD_START): directly share PGD entry
        // For leaf entries, also check U bit - kernel pages (U=0) are shared
        if vpn2 >= KERNEL_PGD_START || (is_l2_leaf && !pte2.is_user()) {
            child_root.set(vpn2, pte2);
            continue;
        }

        // User space: need to copy with COW
        let ppn1 = pte2.ppn();

        if is_l2_leaf {
            // Gigapage - increment refcount for shared user pages.
            // Foreign (boot-reserved, kernel-heap GEM/fb) frames are
            // copied as-is with NO refcount — see is_foreign_user_frame.
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
            child_root.set(vpn2, pte2);
            continue;
        }

        let child_table1_phys = alloc_page_table()?;
        let child_ppn1 = child_table1_phys >> PAGE_SHIFT;
        child_root.set(vpn2, PageTableEntry::new_table(child_ppn1));

        let parent_table1 = PtTable::from_phys_live(ppn1 << PAGE_SHIFT)?;
        let child_table1_ref = PtTable::from_phys_live(child_table1_phys)?;

        for vpn1 in 0..512 {
            let pte1 = parent_table1.get(vpn1);

            if !pte1.is_valid() {
                continue;
            }

            let is_l1_leaf = pte1.is_readable() || pte1.is_writable() || pte1.is_executable();
            if is_l1_leaf {
                // Increment refcount for shared user pages (including read-only).
                // Foreign (boot-reserved, kernel-heap GEM/fb) frames are
                // copied as-is with NO refcount — see is_foreign_user_frame.
                if pte1.is_user()
                    && !crate::mm::page_alloc::is_foreign_user_frame(
                        (pte1.ppn() as usize) << PAGE_SHIFT,
                    )
                {
                    if let Some(page) = page_desc(pte1.ppn() as usize) {
                        page.get_page();
                        page.inc_mapcount();
                    }
                }
                child_table1_ref.set(vpn1, pte1);
                continue;
            }

            let ppn0 = pte1.ppn();

            let child_table0_phys = alloc_page_table()?;
            let child_ppn0 = child_table0_phys >> PAGE_SHIFT;
            child_table1_ref.set(vpn1, PageTableEntry::new_table(child_ppn0));

            let parent_table0_phys = ppn0 << PAGE_SHIFT;
            let parent_table0 = PtTable::from_phys_live(parent_table0_phys)?;
            let child_table0_ref = PtTable::from_phys_live(child_table0_phys)?;

            for vpn0 in 0..512 {
                // The whole walk runs under AddressSpace::fork()'s
                // PTE_MODIFY_LOCK (mm_ops.rs fork()), which serializes it
                // against demand faults, COW faults, and teardown — no
                // per-leaf locking here (the lock is not reentrant).
                let pte0 = parent_table0.get(vpn0);

                if !pte0.is_valid() {
                    continue;
                }

                let is_user = pte0.bits() & PageTableEntry::U != 0;
                let is_writable = pte0.is_writable();

                // COW exemption (see MmStruct::fork): MAP_SHARED and
                // device VMA pages are inherited as-is — write-protecting
                // them in the PARENT would COW-divert every later store
                // (the fbterm framebuffer-freeze root cause). Framebuffer
                // frames are additionally exempted by PHYSICAL range (see
                // fb_frame_range above): the scanout is device-shared
                // memory, so no fork may ever COW it away.
                let leaf_va = ((vpn2 as u64) << 30)
                    | ((vpn1 as u64) << 21)
                    | ((vpn0 as u64) << 12);
                let leaf_ppn = pte0.ppn();
                // Leaf-table fast path (perf review 2026-10-31; x86 twin):
                // riscv leaf tables are 2MB — when the whole table lies
                // outside every exempt region and the framebuffer range,
                // skip the per-PTE scans. One range check per table
                // replaces one scan per PTE; overlapping tables keep the
                // exact per-PTE granularity.
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
                            leaf_ppn >= lo && leaf_ppn < hi
                        })
                    );

                let new_pte = if is_user
                    && !crate::mm::page_alloc::is_foreign_user_frame(
                        (pte0.ppn() as usize) << PAGE_SHIFT,
                    )
                {
                    // Foreign (boot-reserved, kernel-heap GEM/fb) frames
                    // take no refcount and never COW-mark — the else
                    // branch copies the PTE as-is (the mapping is
                    // inherited unchanged; the owning subsystem frees the
                    // memory).
                    let phys_ppn = pte0.ppn() as usize;
                    page_desc(phys_ppn).map_or(pte0, |page| {
                        // PCACHE leaf (filemap_fault, riscv64 port): the
                        // child's copy owns its own pin — without this, the
                        // parent's unmap would drop the LAST pin and
                        // eviction could free a frame the child still maps
                        // (x86_64 twin).
                        if pte0.bits() & cow_flags::PCACHE != 0 {
                            let key = page.mapping() as u64;
                            let index = page.index() as u64;
                            if !crate::fs::hotpath::get_page_cache()
                                .pin_by_key(key, index)
                            {
                                // Entry vanished while a pin existed —
                                // cache/frame bookkeeping drift. The
                                // mapping still works (frame is pinned by
                                // the parent's not-yet-dropped pin); the
                                // child's write takes the normal COW copy
                                // path either way (the leaf carries COW).
                            }
                        }
                        // Increment refcount for shared user pages (one ref per sharer)
                        let after = page.get_page();
                        page.inc_mapcount();
                        // Lifecycle ring (gnome-sigill): this fork just
                        // took a reference on a stamped (file-fill) frame.
                        crate::mm::page_desc::text_stamp_note(
                            phys_ppn,
                            crate::process::current_pid() as u32,
                            'K',
                            after,
                        );

                        if is_writable && !cow_exempt_leaf {
                            // COW: mark both parent and child PTEs as read-only
                            page.set_flag(crate::mm::page_desc::PageFlag::Cow);

                            let cow_pte_bits = pte0.bits() & !PageTableEntry::W | cow_flags::COW;

                            parent_table0.set(vpn0, PageTableEntry::from_bits(cow_pte_bits));

                            PageTableEntry::from_bits(cow_pte_bits)
                        } else {
                            pte0
                        }
                    })
                } else {
                    pte0
                };

                child_table0_ref.set(vpn0, new_pte);
                crate::arch::mm::mmu_init::pte_install_log(
                    child_root_ppn,
                    (vpn2 << 30) as u64 | ((vpn1 as u64) << 21) | ((vpn0 as u64) << 12),
                    new_pte.ppn(),
                );
            }
        }
    }

    // SMP coherence fix: the walk above DOWNGRADED the parent's writable
    // PTEs to read-only+COW. The old single LOCAL sfence left every OTHER
    // hart's stale WRITABLE entries live — a parent store issued from
    // another hart after the fork then bypassed COW and landed in the
    // frame now shared with the child (a silently LOST private write —
    // observed as the rt_sigprocmask old-mask write-back vanishing and
    // dash resuming a stale sigsuspend mask, wedging `wait`), and stale
    // reads resurrected pre-fork page contents. PTE permission changes on
    // a live mm require a TLB shootdown on ALL harts (Linux
    // flush_tlb_mm-style): IPI the peers, full-flush locally.
    crate::arch::ipi::flush_tlb_others();
    // SAFETY: TLB fence (memory-safe in any context).
    unsafe { asm!("sfence.vma zero, zero", options(nostack, preserves_flags)) };

    Some(child_root_ppn)
}

/// Outcome of a COW fault resolution attempt.
///
/// `handle_cow_fault` used to return `Option<()>`, collapsing two very
/// different situations into `None`:
///
///  - the re-walk under `PTE_MODIFY_LOCK` no longer sees a COW PTE — a
///    sibling thread sharing the mm (CLONE_VM) broke the COW between
///    `handle_mm_fault`'s LOCK-FREE `is_cow_page()` check and the locked
///    re-walk, or the mapping was concurrently unmapped/replaced;
///  - allocating the private copy genuinely failed.
///
/// `do_page_fault` mapped every `None` to `OutOfMemory`, so the race loser
/// was SIGKILLed by a fabricated OOM while the allocator sat nearly full
/// (the "fake OOM layer 2" family: GNOME gsettings/gmain/dconf-worker
/// deaths, OOM-FORENSIC dumps with ~450k free pages, zero corruption
/// counters). The tri-state result keeps the two cases apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    /// COW resolved: PTE is writable again (exclusive fast path, or a
    /// private copy was installed and mapped).
    Resolved,
    /// The PTE changed under us between the lock-free COW check and the
    /// locked re-walk. Not an error: re-execute the faulting instruction.
    /// If the mapping is now writable the store simply succeeds; if it was
    /// unmapped or swapped the NEXT fault takes the proper path
    /// (Segfault / swap-in). This mirrors Linux's do_wp_page() re-check
    /// under the PTL, which returns VM_FAULT_RETRY/0 — never a kill —
    /// when the pte changed.
    Retry,
    /// Allocating the private copy failed — a genuine out-of-memory.
    OutOfMemory,
}

/// Handle copy-on-write page fault (GLOBAL-mode PTE section on riscv64,
/// INV-LOCK-7; `mm` is the owning address space of `root_ppn`).
pub fn handle_cow_fault(
    mm: &crate::mm::mm_struct::MmStruct,
    root_ppn: u64,
    fault_addr: VirtAddr,
) -> CowFaultResult {
    // S7-oom01wedge (fleet s2, live gdb strand dump 2026-10-26 02:16):
    // the private-copy alloc_pages(GFP_USER) used to run INSIDE the PTE
    // section, entered with lock_irqsave — this hart's IRQs off and the
    // PTE epoch held across the FULL allocator slowpath. Under memory
    // pressure that slowpath (reclaim retry / OOM-kill wait / free-list
    // churn) ground for 100+ s while every global PTE writer and every
    // same-mm PTE section on the other hart starved (ltp-a r40/r41,
    // ltp-b r38/r39 oom01 wedges: the stuck task sampled pc=0x100 for
    // minutes; dump: hart0 spinning in GlobalPteLock write vs hart1 in
    // Zone::remove_from_free_list). Linux discipline (wp_page_copy):
    // allocate the private copy BEFORE the PTE lock. The section's
    // re-checks already return Retry/Resolved for every no-copy-needed
    // outcome, so a wasted speculative page is simply freed by Drop.
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
    use crate::mm::page_desc::page_desc;

    let virt_addr = fault_addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Re-walk under the section; a guard miss returns the same Retry the
    // restructured-tree arms below produce.
    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return CowFaultResult::Retry;
    };
    let pte2 = root_table.get(vpn2);
    if !pte2.is_valid() {
        // Walk failure at re-check time: the upper levels were restructured
        // (teardown/fork) after handle_mm_fault's lock-free COW check.
        return CowFaultResult::Retry;
    }

    let ppn1 = pte2.ppn();
    let Some(table1) = PtTable::from_phys(ppn1 << PAGE_SHIFT) else {
        return CowFaultResult::Retry;
    };

    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() {
        return CowFaultResult::Retry;
    }

    let ppn0 = pte1.ppn();
    let table0_phys = ppn0 << PAGE_SHIFT;
    let Some(table0) = PtTable::from_phys(table0_phys) else {
        return CowFaultResult::Retry;
    };

    let old_pte = table0.get(vpn0);
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
    // R7-A3: always include R when setting W — after mprotect(PROT_NONE /
    // PROT_EXEC-only) on a COW page the PTE has no R, and writing a W-only
    // leaf produces SV39's reserved encoding (V=1,R=0,W=1): every retry
    // faults forever instead of raising SIGSEGV (fault-path livelock).
    if refcount <= 1 {
        // PCACHE leaf reaching the exclusive arm means the cache entry is
        // gone (frames mapped by the cache always carry the cache's base
        // refcount, so a live entry keeps refcount >= 2 — x86 twin). The
        // leaf must NOT keep the PCACHE bit while gaining W in place; the
        // pin drop is a no-op when the entry already vanished.
        let pcache = old_bits & cow_flags::PCACHE;
        let new_pte = PageTableEntry::from_bits(
            (old_bits & !(cow_flags::COW | cow_flags::PCACHE))
                | PageTableEntry::W
                | PageTableEntry::R,
        );
        if pcache != 0 {
            release_cache_mapping_pin(old_ppn as usize);
        }

        // R7-C9: the page is exclusively owned now — clear the descriptor
        // flag too, or stats/invariants see "COW set with refcount 1".
        if let Some(p) = old_page {
            p.clear_flag(crate::mm::page_desc::PageFlag::Cow);
        }

        table0.set(vpn0, new_pte);

        // SMP coherence fix: the fast-path PTE replacement (RO+COW → RW)
        // must be visible to EVERY hart that may hold the old entry —
        // the task can migrate, and a remote stale entry turns the retry
        // into a permanent fault or serves stale page contents. Shoot the
        // entry down on all peers (the local flush below covers this hart;
        // the remote handler takes no locks, so doing this under
        // PTE_MODIFY_LOCK cannot deadlock).
        crate::arch::ipi::flush_tlb_others();
        let vaddr = virt_addr;
        // SAFETY: ASID-scoped TLB fence (memory-safe in any context).
        unsafe {
            asm!(
                "fence",
                "sfence.vma {0}, zero",
                "fence",
                in(reg) vaddr,
                options(nostack)
            )
        };

        return CowFaultResult::Resolved;
    }

    // NOTE: put_page() on the old page is deferred until AFTER the copy and
    // PTE update.  Calling it earlier opens a race window on multi-hart
    // systems (even with TCG thread=single): the child on another hart could
    // see refcount drop to 1, re-enable W without copying, then exec and free
    // the page — all before the parent finishes copying from it.

    // Allocate new page and copy content. The page came from the
    // speculative pre-lock alloc (S7-oom01wedge): the allocator slowpath
    // must never run inside this PTE section. Sole consumer: `take()`
    // cannot fail here; the None arm keeps the type honest if a future
    // edit adds a second consumer.
    let new_phys = match spec.0.take() {
        Some(p) => p,
        None => {
            return CowFaultResult::OutOfMemory;
        }
    };
    let new_ppn = new_phys >> PAGE_SHIFT;

    // Copy while old page is still pinned (refcount >= 2)
    super::r#unsafe::copy_page(old_ppn << PAGE_SHIFT, new_phys);

    // R7-A3: W requires R (reserved-encoding livelock otherwise — see the
    // exclusive branch above).
    let flags = (old_bits & (PageTableEntry::V | PageTableEntry::R | PageTableEntry::X | PageTableEntry::U | PageTableEntry::G | PageTableEntry::A | PageTableEntry::D)) | PageTableEntry::W | PageTableEntry::R;
    let new_pte = PageTableEntry::from_bits((new_ppn << 10) | flags);

    // Install new PTE before dropping our reference to the old page
    table0.set(vpn0, new_pte);

    // SMP coherence fix: this replacement (old frame → private copy) is
    // exactly the stale-entry hazard — without a shootdown, a hart the
    // task migrated from keeps translating the VA onto the OLD frame,
    // splitting the task's memory view (kernel writes reach the new page,
    // user reads/writes from the stale hart hit the old one — the
    // lost-write/stale-read family behind the sigsuspend mask corruption).
    crate::arch::ipi::flush_tlb_others();
    // SAFETY: TLB fence.
    unsafe { asm!("sfence.vma zero, zero") };

    // Now safe to release our share of the old page.
    // Both refcount and mapcount must be released: the child's PTE no
    // longer points to the old shared page, so the old page loses one
    // mapping reference.  Without dec_mapcount the count leaks across
    // COW cycles and eventually corrupts page reclaim / rmap decisions.
    if let Some(op) = old_page {
        let after = op.put_page_site(crate::debug::driftwatch::S_COW_RV);
        op.dec_mapcount();
        // Stamp lifecycle (gnome-sigill): the faulting process no longer
        // maps the OLD frame — if it owned the text stamp, drop it, or
        // the fork peer's later LEGAL final free is refused as foreign
        // (the TEXTFREE-STALE false-positive family). Record the event
        // either way for the lifecycle ring.
        crate::mm::page_desc::text_stamp_note(
            old_ppn as usize,
            crate::process::current_pid() as u32,
            'C',
            after,
        );
        crate::mm::page_desc::text_stamp_remove_if_owner(
            old_ppn as usize,
            crate::process::current_pid() as u32,
        );
    }

    // PCACHE old leaf (filemap_fault private mapping, riscv64 port): the
    // mapping moved to the private copy — this mapping's cache pin goes
    // with it. Same put-after-descriptor-put ordering as unmap_pages
    // (x86_64 twin). The new PTE above deliberately rebuilds from the
    // protection bits only, so it carries no PCACHE bit.
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
        // Multi-mapping bookkeeping + RSS (review 4.10/4.14): the copy is a
        // new resident page of the faulting address space (the old page
        // remains resident only for its other owners).
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
    let virt_addr = addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
        return false;
    };
    let pte2 = root_table.get(vpn2);

    if !pte2.is_valid() {
        return false;
    }

    // Superpage leaf (PERF: boot-time MMIO windows map as 2MB L1 leaves;
    // the framebuffer maps as a 1GB L2 leaf): a leaf is never a COW PTE
    // (COW only exists on 4KB user leaves).
    if pte2.is_leaf() {
        return false;
    }

    let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte1 = table1.get(vpn1);

    if !pte1.is_valid() {
        return false;
    }
    if pte1.is_leaf() {
        return false;
    }

    let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
        return false;
    };
    let pte0 = table0.get(vpn0);

    if !pte0.is_valid() {
        return false;
    }

    (pte0.bits() & cow_flags::COW) != 0
}

/// Check if page has required permissions
pub fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let virt_addr = addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
    let pte2 = root_table.get(vpn2);

    if !pte2.is_valid() {
        return None;
    }

    // Superpage leaf (PERF: MMIO windows as 2MB L1 leaves, framebuffer as
    // 1GB L2 leaf): report the LEAF's permissions — following its PPN as
    // a table pointer would read device/framebuffer memory as PTEs.
    if pte2.is_leaf() {
        let bits = pte2.bits();
        return Some((
            (bits & PageTableEntry::R) != 0,
            (bits & PageTableEntry::W) != 0,
            (bits & PageTableEntry::X) != 0,
            (bits & PageTableEntry::U) != 0,
        ));
    }

    let table1 = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)?;
    let pte1 = table1.get(vpn1);

    if !pte1.is_valid() {
        return None;
    }
    if pte1.is_leaf() {
        let bits = pte1.bits();
        return Some((
            (bits & PageTableEntry::R) != 0,
            (bits & PageTableEntry::W) != 0,
            (bits & PageTableEntry::X) != 0,
            (bits & PageTableEntry::U) != 0,
        ));
    }

    let table0 = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT)?;
    let pte0 = table0.get(vpn0);

    if !pte0.is_valid() {
        return None;
    }

    let bits = pte0.bits();
    let has_read = (bits & PageTableEntry::R) != 0;
    let has_write = (bits & PageTableEntry::W) != 0;
    let has_exec = (bits & PageTableEntry::X) != 0;
    let is_user = (bits & PageTableEntry::U) != 0;

    Some((has_read, has_write, has_exec, is_user))
}
