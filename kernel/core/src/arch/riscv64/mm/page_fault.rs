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
// FaultFlags + MmFaultResult unified at the SEAM (mm/mm_hooks.rs) with
// the M1a carve; re-export keeps every historical path resolving.
pub use crate::mm::mm_hooks::{FaultFlags, MmFaultResult};

// ==================== Fault Result ====================

/// Page fault handling result

// ==================== Stack Expansion ====================


// ==================== filemap_fault (page-cache mapping, riscv64 port) ====================

/// Pin the page-cache frame for one page-shaped file offset, populating
/// the cache on a miss through the ext4 read machinery (the fs-side
/// readahead/fault-around detector warms the surrounding window in the
/// same device round trips a read_at would have used). x86_64 twin (the
/// session-23 fake-OOM fix); the riscv64 MAP_PRIVATE arm adopts the same
/// protocol so file demand faults stop allocating one unreclaimable
/// private copy per fault (the GNOME-image fake-OOM family).
///
/// Returns the PINNED physical address for direct mapping. The pin (plus
/// the descriptor `get_page` the map path takes) is owned by the PTE for
/// its mapping lifetime — see `cow_flags::PCACHE`.
pub fn filemap_pin_cache_page(
    file: &crate::fs::file::File,
    file_offset: usize,
    effective_size: usize,
    hole_as_zero: bool,
) -> Option<u64> {
    if file_offset >= effective_size {
        return None;
    }
    // The demand-populate capability IS the ext4 probe (rung-5 fault
    // seam: FileOps.fault — a data-field check, no fs named).
    let fault_op = file.get_ops().and_then(|ops| ops.fault);
    if fault_op.is_none() {
        return None;
    }
    // SAFETY: inode cell written at open time, read-only afterwards.
    let inode = unsafe { &*file.inode.get() }.as_ref()?;
    let (fs_id, ino) = (inode.fs_id, inode.ino as u64);
    let page_index = (file_offset / PAGE_SIZE_USIZE) as u64;
    let cache = crate::fs::page_cache::get_page_cache();
    if let Some(phys) = cache.get_phys(fs_id, ino, page_index) {
        return Some(phys);
    }
    // Miss: warm the cache (ReadDst::Discard — no bounce copy), then pin.
    // Holes never enter the page cache through the READ path (the fill
    // skips unallocated blocks and zero-fills the destination in place),
    // so populate can "succeed" without the page existing. For the
    // hole_as_zero arm (SHARED mappings, session 25) that must not fall
    // back to a private zero copy — mapper incoherence, nothing persists:
    // insert an explicit ZERO page into the cache and alias THAT (x86_64
    // twin). The flush path allocates the on-disk block at msync/fsync
    // time. Private arms keep the private-copy fill.
    let populated = fault_op.unwrap()(file, file_offset as u64);
    if populated {
        if let Some(phys) = cache.get_phys(fs_id, ino, page_index) {
            return Some(phys);
        }
    }
    if !hole_as_zero {
        return None;
    }
    // SAFETY: same read-only inode resolution as above.
    let inode = unsafe { &*file.inode.get() }.as_ref()?;
    static ZERO_PAGE: [u8; 4096] = [0; 4096];
    cache.insert(inode.fs_id, inode.ino as u64, page_index, 0, &ZERO_PAGE);
    cache.get_phys(fs_id, ino, page_index)
}

// ==================== Main Fault Handler ====================


// ==================== Anonymous Fault-Around (sc181) ====================


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


/// R-WRITESTORM-2: write one 4K leaf PTE (raw bits), same 3-level walk as
/// read_pte_raw. Caller must hold PTE_MODIFY_LOCK and have resolved the
/// leaf via read_pte_raw first (upper levels exist).
pub fn write_pte_raw(root_ppn: u64, vaddr: VirtAddr, bits: u64) {
    use super::PAGE_SHIFT;
    let vpn2 = ((vaddr.bits() >> 30) & 0x1FF) as usize;
    let vpn1 = ((vaddr.bits() >> 21) & 0x1FF) as usize;
    let vpn0 = ((vaddr.bits() >> 12) & 0x1FF) as usize;
    // SAFETY: same walk as read_pte_raw; the leaf exists (the caller just
    // read it), so intermediate tables are present. Only the leaf changes.
    unsafe {
        let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);
        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() { return; }
        let table1 = get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() || pte1.is_leaf() { return; }
        let table0 = get_page_table_virt(pte1.ppn() << PAGE_SHIFT);
        (*table0).set(vpn0, PageTableEntry::from_bits(bits));
    }
}

pub fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
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


