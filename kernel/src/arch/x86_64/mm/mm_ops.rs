//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 user address-space operations (MmStruct extensions, COW).
//!
//! X86-TODO(agent x86-mm): port the riscv64 implementation 1:1 in shape:
//! PageTableWalker (4-level), create_user_address_space (kernel PML4 copy
//! of the shared kernel half: PML4[256..511] links), map_user_region,
//! copy_page_table_cow (fork), handle_cow_fault, check_pte_permissions.
//! The riscv64 file is arch/riscv64/mm/mm_ops.rs — the logic is portable;
//! only PTE encoding and walk depth differ (both already provided by
//! pagetable.rs here).

use super::memory_layout::*;
use super::pagetable::*;
use super::mmu_init::get_page_table_virt;
use crate::mm::vma::Vma;
use crate::mm::pagemap::{MapError, Perm};
use crate::mm::page::VirtAddr as PageVirtAddr;

// ==================== MmStruct extensions ====================

impl crate::mm::MmStruct {
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        let _ = (vma, perm);
        Err(MapError::OutOfMemory) // X86-TODO(agent x86-mm)
    }

    pub fn map_single_page(&self, virt_addr: VirtAddr, perm: Perm) -> Result<(), MapError> {
        let _ = (virt_addr, perm);
        Err(MapError::OutOfMemory) // X86-TODO(agent x86-mm)
    }

    pub fn unmap_vma(&self, start: PageVirtAddr) -> Result<(), MapError> {
        let _ = start;
        Err(MapError::OutOfMemory) // X86-TODO(agent x86-mm)
    }

    pub fn set_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        let _ = new_brk;
        Err(MapError::OutOfMemory) // X86-TODO(agent x86-mm)
    }
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

/// Allocate one user page from the zone allocator. X86-TODO(agent x86-mm):
/// return physical address, mirroring the riscv64 twin's page-accounting.
pub fn alloc_user_phys_page() -> Option<u64> {
    let addr = crate::mm::page_alloc::alloc_pages(crate::mm::zone::GfpFlags::GFP_KERNEL, 0);
    if addr == 0 { None } else { Some(addr as u64) }
}

/// Create a fresh user address space root: kernel half shared, user half empty.
pub fn create_user_address_space() -> Option<u64> {
    // X86-TODO(agent x86-mm): alloc root, copy PML4[256..511] entries from
    // the kernel root (they are all links, no COW of kernel tables needed).
    None
}

/// Map a single user page. X86-TODO(agent x86-mm)
pub unsafe fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let _ = (user_root_ppn, user_virt, phys, flags);
}

/// Global PTE-modification lock (interface parity with riscv64)
pub static PTE_MODIFY_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

/// Map a user region. X86-TODO(agent x86-mm)
pub unsafe fn map_user_region(
    user_root_ppn: u64,
    user_virt: VirtAddr,
    size: u64,
    phys: Option<PhysAddr>,
    flags: u64,
) -> bool {
    let _ = (user_root_ppn, user_virt, size, phys, flags);
    false
}

/// Copy-on-Write flags
///
/// COW marker uses x86 PTE software-available bit 9 (bit 8 is GLOBAL on
/// x86, unlike riscv where bit 8 was free).
pub mod cow_flags {
    pub const COW: u64 = 1 << 9;
}

/// Copy an address space for fork (page-table COW). X86-TODO(agent x86-mm):
/// kernel half (PML4[256..511]) shared by copying links; user pages marked
/// COW (riscv64 twin at arch/riscv64/mm/mm_ops.rs is the porting template).
pub unsafe fn copy_page_table_cow(
    parent_root_ppn: u64,
    cow_exempt: &[(u64, u64)],
) -> Option<u64> {
    let _ = cow_exempt;
    let _ = parent_root_ppn;
    None
}

/// COW fault resolution outcome
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    Resolved,
    Retry,
    OutOfMemory,
    Fault,
}

/// Resolve a write fault on a COW page. X86-TODO(agent x86-mm)
pub unsafe fn handle_cow_fault(root_ppn: u64, fault_addr: VirtAddr) -> CowFaultResult {
    let _ = (root_ppn, fault_addr);
    CowFaultResult::Fault
}

/// Is the mapping at `addr` a COW page? X86-TODO(agent x86-mm)
pub unsafe fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let _ = (root_ppn, addr);
    false
}

/// (read, write, exec, user) of the PTE at `addr`. X86-TODO(agent x86-mm)
pub unsafe fn check_pte_permissions(root_ppn: u64, addr: VirtAddr) -> Option<(bool, bool, bool, bool)> {
    let _ = (root_ppn, addr);
    None
}
