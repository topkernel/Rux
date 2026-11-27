//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Page Fault Handling
//!
//! This module contains:
//! - Fault type flags
//! - Page fault handling result types
//! - handle_mm_fault() implementation
//! - Stack expansion support


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;

use super::memory_layout::*;
use super::unsafe_edges::PtTable;
use super::mmu_init::{map_page, get_page_table_virt, ROOT_PAGE_TABLE};
use super::mm_ops::{alloc_user_phys_page, is_cow_page, check_pte_permissions, PageTableWalker};
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Page fault type flags
pub struct FaultFlags;

impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 0x01;
    /// Write fault
    pub const WRITE: u32 = 0x02;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 0x04;
    /// User mode access
    pub const USER: u32 = 0x08;
    /// Kernel mode access
    pub const KERNEL: u32 = 0x10;
}

// ==================== Fault Result ====================

/// Page fault handling result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Handled successfully, can retry instruction
    Handled,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
    /// Already mapped (no handling needed)
    AlreadyMapped,
    /// COW pending (handled by handle_cow_fault)
    CowPending,
    /// Kernel exception fixed (via exception table)
    Fixed,
    /// Unfixable kernel exception
    KernelPanic,
}

// ==================== Stack Expansion ====================
/// Try to expand stack when page fault occurs below current stack bottom
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn try_expand_stack( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, root_ppn: u64, ) -> MmFaultResult {
    crate::arch::arch_floor::riscv64::mm::page_fault::try_expand_stack(addr_space, fault_addr, flags, root_ppn)
}

// ==================== filemap_fault (page-cache mapping, riscv64 port) ====================
/// Pin the page-cache frame for one page-shaped file offset, populating
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-9): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn filemap_pin_cache_page( file: &crate::fs::file::File, file_offset: usize, effective_size: usize, hole_as_zero: bool, ) -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::page_fault::filemap_pin_cache_page(file, file_offset, effective_size, hole_as_zero)
}

// ==================== Main Fault Handler ====================

/// handle_mm_fault - Handle user mode page fault
///
/// # Arguments
/// - `addr_space`: Address space
/// - `fault_addr`: Virtual address that triggered fault
/// - `flags`: Fault type flags (FaultFlags)
///
/// # Returns
/// Returns handling result
///
/// # Function
/// 1. Find VMA to validate address validity and permissions
/// 2. Check if page is already mapped
/// 3. If COW page, return CowPending
/// 4. If unmapped, allocate new page (zero anonymous pages)
/// 5. Update page table, set correct permission bits
/// E8-MM: does a leaf PTE (raw bits) at `va` count as "mapped" for a
/// fault on this address space? A valid KERNEL (U=0) PTE inside a
/// low-half device window is the RESTORE fill clear_pte leaves behind
/// after a user mapping over the window was unmapped (megapage
/// demotion made those mmaps possible — MAP_FIXED replace semantics).
/// A user fault there is a demand fault on the user mapping, not a
/// protection violation: treat the device PTE as a hole and let the
/// VMA layer decide (a stray access with no covering VMA still
/// SIGSEGVs, just via the correct path). Caught by megapage_mmap's
/// re-map step: the old code returned Permission denied at 0x0c008007
/// (sig=11), and without this same rule in the R7-C3 re-check the
/// demand path spun forever "Handling" refaults against the restored
/// kernel PTE.
#[inline]
pub(crate) fn pte_counts_as_mapped(bits: u64, va: u64) -> bool {
    !(bits & PageTableEntry::U == 0
        && crate::arch::mm::mmu_init::kernel_device_window_pte(va).is_some())
}
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-8): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_mm_fault( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, ) -> MmFaultResult {
    crate::arch::arch_floor::riscv64::mm::page_fault::handle_mm_fault(addr_space, fault_addr, flags)
}

// ==================== Anonymous Fault-Around (sc181) ====================

/// sc181: forward fault-around window (pages) for anonymous write faults.
pub(crate) const ANON_FAULTAROUND_PAGES: usize = 32;

/// sc181: minimum VMA size that gets fault-around (small stacks/brk keep
/// exact one-page semantics).
pub(crate) const ANON_FAULTAROUND_MIN_VMA: usize = 1024 * 1024;
/// sc181: install a FORWARD window of zero pages after a successful
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn anon_faultaround_forward( addr_space: &AddressSpace, root_ppn: u64, from_page: usize, vma_end: usize, pte_flags: u64, ) {
    crate::arch::arch_floor::riscv64::mm::page_fault::anon_faultaround_forward(addr_space, root_ppn, from_page, vma_end, pte_flags)
}

// ==================== Utility ====================

/// Get the physical address mapped at a user virtual address.
///
/// Walks the page table to find the PTE for the given virtual address
/// and returns the physical page address (page-aligned).
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    use super::PAGE_SHIFT;

    let vpn2 = (vaddr >> 30) & 0x1FF;
    let vpn1 = (vaddr >> 21) & 0x1FF;
    let vpn0 = (vaddr >> 12) & 0x1FF;

    // Typed walk (the round-1 PtTable edge); superpage leaves
    // compute the 4KB frame inside the leaf, never following a
    // leaf PPN as a table pointer.
    {

let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else {
    return None;
};
let pte2 = root_table.get(vpn2 as usize);
if !pte2.is_valid() { return None; }
// Superpage leaves: compute the 4KB frame inside the leaf (PERF:
// MMIO windows map as 2MB L1 leaves; never follow a leaf PPN as a
// table pointer).
if pte2.is_leaf() {
    // 1GB leaf: PPN[2] (bits 53:28) is the 1GB frame number.
    let ppn2 = (pte2.bits() >> 28) & 0x3FF_FFFF;
    return Some((ppn2 << 30) | (vaddr & 0x3FFF_FFFF & !0xFFF));
}

let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else {
    return None;
};
let pte1 = table1.get(vpn1 as usize);
if !pte1.is_valid() { return None; }
if pte1.is_leaf() {
    return Some((pte1.ppn_for_2mb_page() << 21) | (vaddr & 0x1F_FFFF & !0xFFF));
}

let Some(table0) = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT) else {
    return None;
};
let pte0 = table0.get(vpn0 as usize);
if !pte0.is_valid() { return None; }

Some((pte0.ppn() as u64) << PAGE_SHIFT)

    }
}

// ==================== Swap-In Support ====================

/// Read the raw PTE value at a virtual address.
///
/// Walks the three-level page table and returns the raw bits of the
/// leaf PTE, even if V=0 (e.g. a swap entry).  Returns None if the
/// page table walk cannot reach the leaf level.
/// R-WRITESTORM-2: is this leaf a PROT_NONE marker (V|U set, no R/W/X,
/// PPN preserved) as written by sys_mprotect for prot==0? The covering
/// VMA owns the protection decision; the fault layer re-presents the
/// leaf when the VMA grants the access.
pub(crate) fn is_protnone_marker(raw: u64) -> bool {
    (raw & PageTableEntry::V) != 0
        && (raw & PageTableEntry::U) != 0
        && (raw & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X)) == 0
        && (raw >> 10) != 0 // has a PPN
}
/// R-WRITESTORM-2: write one 4K leaf PTE (raw bits), same 3-level walk as
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn write_pte_raw(root_ppn: u64, vaddr: VirtAddr, bits: u64) {
    crate::arch::arch_floor::riscv64::mm::page_fault::write_pte_raw(root_ppn, vaddr, bits)
}

pub(crate) fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
    use super::PAGE_SHIFT;

    let vpn2 = (vaddr.bits() >> 30) & 0x1FF;
    let vpn1 = (vaddr.bits() >> 21) & 0x1FF;
    let vpn0 = (vaddr.bits() >> 12) & 0x1FF;

    // Typed walk; only reads PTEs, including the leaf even if V=0.
    {
        let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
        let pte2 = root_table.get(vpn2 as usize);
        if !pte2.is_valid() { return None; }
        // Superpage leaves (PERF: 2MB MMIO L1 leaves / 1GB L2 framebuffer):
        // return the leaf itself — never a swap/migration entry, and its
        // PPN must not be followed as a table pointer.
        if pte2.is_leaf() { return Some(pte2.bits()); }

        let table1 = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)?;
        let pte1 = table1.get(vpn1 as usize);
        if !pte1.is_valid() { return None; }
        if pte1.is_leaf() { return Some(pte1.bits()); }

        let table0 = PtTable::from_phys(pte1.ppn() << PAGE_SHIFT)?;
        let pte0 = table0.get(vpn0 as usize);

        let raw = pte0.bits();
        if raw == 0 { return None; }

        Some(raw)
    }
}
/// Handle a swap fault — read the page back from swap and map it.
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_swap_fault( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, swap_entry: u64, root_ppn: u64, ) -> MmFaultResult {
    crate::arch::arch_floor::riscv64::mm::page_fault::handle_swap_fault(addr_space, fault_addr, flags, swap_entry, root_ppn)
}
