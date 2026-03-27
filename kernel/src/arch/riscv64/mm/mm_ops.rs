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

use core::arch::asm;
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
    /// Enable this address space (switch page table)
    pub unsafe fn enable(&self) {
        let satp = Satp::sv39(self.pgd, 0);
        asm!("csrw satp, {}", in(reg) satp.bits());
        asm!("sfence.vma zero, zero");
    }

    /// Disable address space (switch to bare mode)
    pub unsafe fn disable() {
        let satp = Satp::new(Satp::MODE_BARE, 0, 0);
        asm!("csrw satp, {}", in(reg) satp.bits());
        asm!("sfence.vma zero, zero");
    }

    /// Flush entire TLB
    pub unsafe fn flush_tlb() {
        asm!("sfence.vma zero, zero");
    }

    /// Flush TLB for specified page
    pub unsafe fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        asm!("sfence.vma {}, zero", in(reg) vaddr.as_usize());
    }

    // ==================== VMA Operations ====================

    /// Map VMA (requires write lock)
    ///
    /// For anonymous mappings, use lazy mapping (demand paging):
    /// Only create VMA, don't pre-map pages.
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();

        let start = vma.start();
        let end = vma.end();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Update virtual memory statistics
        let pages = ((end.as_usize() - start.as_usize()) / PAGE_SIZE_USIZE) as u64;
        self.add_total_vm(pages);
        self.update_highest_vm_end(end.as_usize());

        Ok(())
    }

    /// Map single page (for lazy mapping/page fault handling)
    pub fn map_single_page(&self, virt_addr: VirtAddr, perm: Perm) -> Result<(), MapError> {
        let phys_addr = alloc_user_phys_page().ok_or(MapError::OutOfMemory)? as usize;
        let flags = perm_to_flags(perm, self.space_type());

        unsafe {
            let ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            core::ptr::write_bytes(ptr.bits() as *mut u8, 0, PAGE_SIZE_USIZE);
            fence(Ordering::SeqCst);
        }

        unsafe {
            map_page(
                self.pgd,
                virt_addr,
                PhysAddr::new(phys_addr as u64),
                flags,
            );
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
                if unsafe { PageTableWalker::walk(self.pgd, addr as u64) }.is_none() {
                    let phys_addr = alloc_pages(GfpFlags::GFP_KERNEL, 0);
                    if phys_addr == 0 {
                        return Err(MapError::OutOfMemory);
                    }
                    let flags = perm_to_flags(Perm::ReadWrite, self.space_type());
                    unsafe {
                        map_page(
                            self.pgd,
                            VirtAddr::new(addr as u64),
                            PhysAddr::new(phys_addr as u64),
                            flags,
                        );
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

        let is_fixed = map_flags & map::MAP_FIXED != 0;

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

            use user_addr::BRK_DEFAULT;
            use user_addr::MMAP_START;
            let has_brk_conflict = addr.as_usize() < MMAP_START && addr.as_usize() >= BRK_DEFAULT;

            if has_vma_conflict || has_brk_conflict {
                self.find_free_area(aligned_size)?
            } else {
                addr
            }
        };

        if is_fixed {
            let mut vma_mgr = self.vma_write();
            let mut vmas_to_remove = Vec::new();
            for vma in vma_mgr.iter() {
                if vma.overlaps(&Vma::new(start, PageVirtAddr::new(start.as_usize() + aligned_size), flags)) {
                    vmas_to_remove.push(vma.start());
                }
            }
            drop(vma_mgr);

            for vma_start in vmas_to_remove {
                let mut vma_mgr = self.vma_write();
                let _ = vma_mgr.remove(vma_start);
            }

            let mut addr = start.as_usize();
            while addr < start.as_usize() + aligned_size {
                unsafe {
                    self.clear_pte(addr as u64);
                }
                addr += PAGE_SIZE_USIZE;
            }

            unsafe {
                asm!("sfence.vma zero, zero");
            }
        }

        let end = PageVirtAddr::new(start.as_usize() + aligned_size);
        let mut vma = Vma::new(start, end, flags);
        vma.set_type(vma_type);
        self.map_vma(vma, perm)?;
        Ok(start)
    }

    /// Find free virtual address area
    fn find_free_area(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        use user_addr::{MMAP_START, MMAP_END, USER_END};

        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let vma_mgr = self.vma_read();

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

    /// munmap system call implementation
    pub fn munmap(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        if addr.as_usize() % PAGE_SIZE_USIZE != 0 {
            return Err(MapError::Invalid);
        }

        let end_addr = addr.as_usize() + aligned_size;

        {
            let vma_mgr = self.vma_read();

            let vma_info = vma_mgr.find(addr).map(|vma| {
                (vma.start(), vma.end())
            });
            drop(vma_mgr);

            if let Some((vma_start, vma_end)) = vma_info {
                let vma_start_usize = vma_start.as_usize();
                let vma_end_usize = vma_end.as_usize();

                if addr.as_usize() <= vma_start_usize && end_addr >= vma_end_usize {
                    let mut vma_mgr = self.vma_write();
                    vma_mgr.remove(vma_start)?;
                } else if addr.as_usize() > vma_start_usize && end_addr < vma_end_usize {
                    return Err(MapError::Invalid);
                } else {
                    return Err(MapError::Invalid);
                }
            }
        }

        self.unmap_pages(addr, aligned_size)?;

        Ok(())
    }

    /// Unmap physical pages in specified range
    fn unmap_pages(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let mut addr = start.as_usize();
        let end = addr + size;

        while addr < end {
            let ppn = unsafe { PageTableWalker::walk(self.pgd, addr as u64) };

            if let Some(_ppn) = ppn {
                unsafe {
                    self.clear_pte(addr as u64);
                }
            }

            addr += PAGE_SIZE_USIZE;
        }

        unsafe {
            asm!("sfence.vma zero, zero");
        }

        Ok(())
    }

    /// Clear page table entry at specified virtual address
    unsafe fn clear_pte(&self, virt: u64) {
        let vpn2 = ((virt >> 30) & 0x1FF) as usize;
        let vpn1 = ((virt >> 21) & 0x1FF) as usize;
        let vpn0 = ((virt >> 12) & 0x1FF) as usize;

        let root_table = get_page_table_virt(self.pgd << PAGE_SHIFT);

        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            return;
        }

        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() {
            return;
        }

        let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);

        (*table0).set(vpn0, PageTableEntry::from_bits(0));
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
        let new_root_ppn = unsafe {
            copy_page_table_cow(self.pgd).ok_or(MapError::OutOfMemory)?
        };

        let new_space = unsafe { MmStruct::new_shared(
            new_root_ppn,
            self.space_type(),
            self.brk(),
        ) };

        {
            let vma_mgr = self.vma_read();
            if vma_mgr.iter().count() > 0 {
                let mut new_vma_mgr = new_space.vma_write();
                for vma in vma_mgr.iter() {
                    let new_vma = Vma::new(vma.start(), vma.end(), vma.flags());
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

        Ok(new_space)
    }
}

/// Convert permission to page table flags
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
    }
    if space_type == PageTableType::User {
        flags |= PageTableEntry::U;
    }
    flags
}

// ==================== User Address Space Management ====================

pub(crate) struct PageTableWalker;

impl PageTableWalker {
    /// Walk page table to find physical page number for virtual address
    /// Returns (ppn, full_pte_value) for debugging
    /// Handles both 4KB pages and 2MB huge pages
    pub(crate) unsafe fn walk(user_root_ppn: u64, virt: u64) -> Option<(u64, u64)> {
        let virt_addr = VirtAddr::new(virt);

        let vpn2 = virt_addr.vpn(2) as usize;
        let vpn1 = virt_addr.vpn(1) as usize;
        let vpn0 = virt_addr.vpn(0) as usize;

        let root_table = get_page_table_virt(user_root_ppn << PAGE_SHIFT);

        let pte2 = (*root_table).get(vpn2);
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

        let ppn1 = pte2.ppn();
        let table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);
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

        let ppn0 = pte1.ppn();
        let table0 = get_page_table_virt(ppn0 << PAGE_SHIFT);
        let pte0 = (*table0).get(vpn0);
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

    unsafe {
        let root_table = get_page_table_virt(root_page);
        (*root_table).zero();

        let kernel_ppn = super::mmu_init::root_page_table_ppn();
        let root_ppn = root_page / PAGE_SIZE;
        copy_kernel_mappings(root_ppn, kernel_ppn);

        // Copy fixmap (UART) to user page table so console works after switch_mm
        super::fixmap::copy_fixmap_to_user(root_ppn);

        Some(root_ppn)
    }
}

/// Copy kernel mappings to user page table (Linux-style)
///
/// This function copies PGD entries (pointers to L1 tables), NOT the L1 tables themselves.
/// All processes share the same kernel L1/L0 page tables - this is exactly how Linux does it.
///
/// Linux: sync_kernel_mappings() in arch/riscv/include/asm/pgalloc.h:
///   memcpy(pgd + USER_PTRS_PER_PGD, init_mm.pgd + USER_PTRS_PER_PGD,
///          (PTRS_PER_PGD - USER_PTRS_PER_PGD) * sizeof(pgd_t));
///
/// We copy:
/// 1. MMIO PGD entries (VPN2[0..2]) for device access from kernel mode
///    when running with a user page table (e.g., during syscall handling)
/// 2. All kernel space PGD entries (VPN2 >= KERNEL_PGD_START = 256)
unsafe fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    let kernel_phys = kernel_root_ppn * PAGE_SIZE;
    let user_phys = user_root_ppn * PAGE_SIZE;

    let kernel_virt = get_page_table_virt(kernel_phys);
    let user_virt = get_page_table_virt(user_phys);

    let kernel_table = kernel_virt as *const PageTable;
    let user_table = user_virt as *mut PageTable;

    (*user_table).zero();

    // Copy MMIO PGD entries (VPN2[0] and VPN2[1])
    // These map device memory (PLIC, CLINT, UART, VirtIO, PCI MMIO) at their
    // physical addresses. Needed because kernel may access devices during syscalls.
    //
    // IMPORTANT: Do NOT directly copy the PGD entry, because the kernel's L1 table
    // for VPN2[0..1] may contain non-leaf entries pointing to user-space L0 tables
    // (from a previous process's mappings). Instead, create a new L1 table and
    // copy only entries that are safe (MMIO), skipping entries that belong to user space.
    for i in 0..2usize {
        let pte = (*kernel_table).get(i);
        if !pte.is_valid() {
            continue;
        }

        if pte.is_leaf() {
            // L2 leaf (1GB huge page) — safe to copy directly
            (*user_table).set(i, pte);
        } else {
            // Non-leaf: create new L1 table, copy only safe entries
            let kernel_l1_phys = pte.ppn() << PAGE_SHIFT;
            let kernel_l1 = get_page_table_virt(kernel_l1_phys) as *const PageTable;

            if let Some(new_l1_phys) = super::mmu_init::alloc_page_table() {
                let new_l1_virt = get_page_table_virt(new_l1_phys) as *mut PageTable;
                (*new_l1_virt).zero();

                for j in 0..512 {
                    let l1_entry = (*kernel_l1).get(j);
                    if !l1_entry.is_valid() {
                        continue;
                    }

                    if l1_entry.is_leaf() {
                        // Leaf entry (huge page MMIO) — always safe to copy
                        (*new_l1_virt).set(j, l1_entry);
                    } else {
                        // Non-leaf entry: check if the L0 table contains user-space entries
                        let l0_phys = l1_entry.ppn() << PAGE_SHIFT;
                        let l0_table = get_page_table_virt(l0_phys) as *const PageTable;

                        let mut has_user_entry = false;
                        for k in 0..512 {
                            let l0_entry = (*l0_table).get(k);
                            if l0_entry.is_valid() && l0_entry.is_leaf() && l0_entry.is_user() {
                                has_user_entry = true;
                                break;
                            }
                        }

                        if !has_user_entry {
                            // Kernel-only L0 table (MMIO) — safe to copy
                            (*new_l1_virt).set(j, l1_entry);
                        }
                        // else: skip user L0 table (belongs to another process)
                    }
                }

                let new_l1_ppn = new_l1_phys >> PAGE_SHIFT;
                (*user_table).set(i, PageTableEntry::new_table(new_l1_ppn));
            }
        }
    }

    // Linux-style: Copy kernel-space PGD entries (VPN2 >= KERNEL_PGD_START = 256)
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

/// Map user region
pub unsafe fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
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
pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let phys_addr = if page_count == 1 {
        alloc_pages(GfpFlags::GFP_USER, 0)
    } else {
        let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(10);
        alloc_pages(GfpFlags::GFP_USER, order)
    };

    if phys_addr == 0 {
        return None;
    }

    map_user_region(user_root_ppn, virt_addr, phys_addr as u64, size, flags);

    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr as u64)
}

/// Allocate and map to kernel table
pub unsafe fn alloc_and_map_to_kernel_table(
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let phys_addr = if page_count == 1 {
        alloc_pages(GfpFlags::GFP_USER, 0)
    } else {
        let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(10);
        alloc_pages(GfpFlags::GFP_USER, order)
    };

    if phys_addr == 0 {
        return None;
    }

    let kernel_ppn = get_kernel_page_table_ppn();

    let user_flags = flags | PageTableEntry::U;

    map_user_region(kernel_ppn, virt_addr, phys_addr as u64, size, user_flags);

    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr as u64)
}

/// Allocate and map to user table
pub unsafe fn alloc_and_map_to_user_table(
    user_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    let phys_addr = if page_count == 1 {
        alloc_pages(GfpFlags::GFP_USER, 0)
    } else {
        let order = (page_count.next_power_of_two().trailing_zeros() as usize).min(10);
        alloc_pages(GfpFlags::GFP_USER, order)
    };

    if phys_addr == 0 {
        return None;
    }

    let user_flags = flags | PageTableEntry::U;
    map_user_region(user_ppn, virt_addr, phys_addr as u64, size, user_flags);

    let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64));
    core::ptr::write_bytes(virt_addr_ptr.bits() as *mut u8, 0, page_count * PAGE_SIZE as usize);

    Some(phys_addr as u64)
}

// ==================== Copy-on-Write Support ====================

/// Copy-on-Write flags
pub mod cow_flags {
    /// COW flag - page is marked as copy-on-write
    pub const COW: u64 = 1 << 8;  // Use bit 8 (after A and D)
}

/// Copy page table with COW marking
///
/// Kernel mappings (VPN2 >= KERNEL_PGD_START or U=0 entries) are shared by copying PGD entries.
/// User space mappings are copied with COW marking for writable pages.
pub unsafe fn copy_page_table_cow(parent_root_ppn: u64) -> Option<u64> {
    use crate::mm::page_desc::pfn_to_page_mut;

    if parent_root_ppn == 0 {
        return None;
    }

    let child_root_phys = alloc_page_table()?;
    let child_root_ppn = child_root_phys >> PAGE_SHIFT;

    let parent_root_phys = parent_root_ppn << PAGE_SHIFT;
    let parent_root = get_page_table_virt(parent_root_phys);
    let child_root = get_page_table_virt(child_root_phys);

    for vpn2 in 0..512 {
        let pte2 = (*parent_root).get(vpn2);

        if !pte2.is_valid() {
            continue;
        }

        // Check if L2 is a leaf (gigapage)
        let is_l2_leaf = pte2.is_readable() || pte2.is_writable() || pte2.is_executable();

        // Kernel region (VPN2 >= KERNEL_PGD_START): directly share PGD entry
        // For leaf entries, also check U bit - kernel pages (U=0) are shared
        if vpn2 >= KERNEL_PGD_START || (is_l2_leaf && !pte2.is_user()) {
            (*child_root).set(vpn2, pte2);
            continue;
        }

        // User space: need to copy with COW
        let ppn1 = pte2.ppn();

        if is_l2_leaf {
            // Gigapage - just share it
            (*child_root).set(vpn2, pte2);
            continue;
        }

        let child_table1_phys = alloc_page_table()?;
        let child_ppn1 = child_table1_phys >> PAGE_SHIFT;
        (*child_root).set(vpn2, PageTableEntry::new_table(child_ppn1));

        let parent_table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);
        let child_table1_ref = &mut *get_page_table_virt(child_table1_phys);

        for vpn1 in 0..512 {
            let pte1 = (*parent_table1).get(vpn1);

            if !pte1.is_valid() {
                continue;
            }

            let is_l1_leaf = pte1.is_readable() || pte1.is_writable() || pte1.is_executable();
            if is_l1_leaf {
                (*child_table1_ref).set(vpn1, pte1);
                continue;
            }

            let ppn0 = pte1.ppn();

            let child_table0_phys = alloc_page_table()?;
            let child_ppn0 = child_table0_phys >> PAGE_SHIFT;
            (*child_table1_ref).set(vpn1, PageTableEntry::new_table(child_ppn0));

            let parent_table0_phys = ppn0 << PAGE_SHIFT;
            let parent_table0 = get_page_table_virt(parent_table0_phys);
            let child_table0_ref = &mut *get_page_table_virt(child_table0_phys);

            for vpn0 in 0..512 {
                let pte0 = (*parent_table0).get(vpn0);

                if !pte0.is_valid() {
                    continue;
                }

                let is_user = pte0.bits() & PageTableEntry::U != 0;
                let is_writable = pte0.is_writable();

                let new_pte = if is_user && is_writable {
                    let phys_ppn = pte0.ppn() as usize;
                    let page = pfn_to_page_mut(phys_ppn);

                    if !page.is_null() {
                        let old_ref = (*page).refcount();
                        if old_ref == 0 {
                            (*page).get_page();
                        }
                        (*page).get_page();
                        (*page).set_flag(crate::mm::page_desc::PageFlag::Cow);

                        let cow_pte_bits = pte0.bits() & !PageTableEntry::W | cow_flags::COW;

                        let parent_table0_mut = parent_table0 as *mut PageTable;
                        (*parent_table0_mut).set(vpn0, PageTableEntry::from_bits(cow_pte_bits));

                        PageTableEntry::from_bits(cow_pte_bits)
                    } else {
                        pte0
                    }
                } else {
                    pte0
                };

                (*child_table0_ref).set(vpn0, new_pte);
            }
        }
    }

    asm!("sfence.vma", options(nostack, preserves_flags));

    Some(child_root_ppn)
}

/// Handle copy-on-write page fault
pub unsafe fn handle_cow_fault(root_ppn: u64, fault_addr: VirtAddr) -> Option<()> {
    use crate::mm::page_desc::pfn_to_page_mut;
    use crate::sched;

    let virt_addr = fault_addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);

    let pte2 = (*root_table).get(vpn2);
    if !pte2.is_valid() {
        return None;
    }

    let ppn1 = pte2.ppn();
    let table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);

    let pte1 = (*table1).get(vpn1);
    if !pte1.is_valid() {
        return None;
    }

    let ppn0 = pte1.ppn();
    let table0_phys = ppn0 << PAGE_SHIFT;
    let table0 = get_page_table_virt(table0_phys);

    let old_pte = (*table0).get(vpn0);
    if !old_pte.is_valid() {
        return None;
    }

    let old_bits = old_pte.bits();
    if old_bits & cow_flags::COW == 0 {
        return None;
    }

    let old_ppn = old_pte.ppn();
    let old_page = pfn_to_page_mut(old_ppn as usize);

    let refcount = if !old_page.is_null() {
        (*old_page).refcount()
    } else {
        1
    };

    // If refcount <= 1, we're the only owner - just enable write
    if refcount <= 1 {
        let new_pte = PageTableEntry::from_bits(
            (old_bits & !cow_flags::COW) | PageTableEntry::W
        );

        (*table0).set(vpn0, new_pte);

        // Flush TLB for this address
        let vaddr = virt_addr;
        asm!(
            "fence",
            "sfence.vma {0}, zero",
            "fence",
            in(reg) vaddr,
            options(nostack)
        );

        return Some(());
    }

    if !old_page.is_null() {
        (*old_page).put_page();
    }

    // Allocate new page and copy content
    let new_phys = alloc_user_phys_page()?;
    let new_ppn = new_phys >> PAGE_SHIFT;

    let new_virt = phys_to_virt(PhysAddr::new(new_phys));
    let old_virt = phys_to_virt(PhysAddr::new(old_ppn << PAGE_SHIFT));

    core::ptr::copy_nonoverlapping(
        old_virt.bits() as *const u8,
        new_virt.bits() as *mut u8,
        PAGE_SIZE as usize
    );

    let flags = (old_bits & 0xFF) | PageTableEntry::W;
    let new_pte = PageTableEntry::from_bits((new_ppn << 10) | flags);

    (*table0).set(vpn0, new_pte);

    asm!("sfence.vma zero, zero");

    Some(())
}

/// Check if page is a COW page
pub unsafe fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let virt_addr = addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return false;
    }

    let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return false;
    }

    let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
    let pte0 = (*table0).get(vpn0);

    if !pte0.is_valid() {
        return false;
    }

    (pte0.bits() & cow_flags::COW) != 0
}

/// Check if page has required permissions
pub unsafe fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let virt_addr = addr.bits();

    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return None;
    }

    let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return None;
    }

    let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
    let pte0 = (*table0).get(vpn0);

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
