//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Virtual Memory Map (vmemmap) Management
//!
//! This module implements vmemmap for page descriptor mapping.
//! Instead of using a static array, page descriptors are mapped at a fixed
//! virtual address region (VMEMMAP_START).
//!
//! Advantages:
//! - No large static array in kernel image
//! - Dynamically mapped based on actual physical memory
//! - O(1) PFN to page conversion via simple arithmetic

use core::ptr;

use super::PAGE_SIZE;
use super::page_desc::Page;
use crate::arch::riscv64::mm::{VMEMMAP_START, VMEMMAP_END};
use crate::arch::riscv64::mm::{PageTableEntry, map_kernel_page, phys_to_virt, PhysAddr};

/// Page descriptor size (64 bytes for struct Page)
pub const STRUCT_PAGE_SIZE: usize = core::mem::size_of::<Page>();

/// Number of struct Page entries per page
/// One 4KB page can hold 4096 / 64 = 64 page descriptors
pub const PAGES_PER_VMEMMAP_PAGE: usize = PAGE_SIZE / STRUCT_PAGE_SIZE;

/// vmemmap initialized flag
static VMEMMAP_INIT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// vmemmap statistics
static mut VMEMMAP_STATS: VmemmapStats = VmemmapStats {
    initialized: false,
    vmemmap_base: VMEMMAP_START,
    start_pfn: 0,
    nr_pages: 0,
    vmemmap_pages: 0,
};

/// Convert PFN to vmemmap virtual address
///
/// vmemmap_addr = VMEMMAP_START + (pfn - vmemmap_start_pfn) * sizeof(Page)
///
/// vmemmap = VMEMMAP_START - vmemmap_start_pfn
/// So pfn_to_page(pfn) = vmemmap + pfn = VMEMMAP_START + (pfn - vmemmap_start_pfn) * sizeof(Page)
#[inline]
pub fn pfn_to_vmemmap(pfn: usize) -> usize {
    // Use stored start_pfn as vmemmap_start_pfn
    // SAFETY: VMEMMAP_STATS is initialized by init_vmemmap() before any
    // pfn_to_vmemmap() call; start_pfn is a plain usize field (no mutation
    // concurrent with vmemmap reads).
    let start_pfn = unsafe { VMEMMAP_STATS.start_pfn };
    VMEMMAP_START + (pfn - start_pfn) * STRUCT_PAGE_SIZE
}

/// Convert vmemmap virtual address to PFN
///
/// pfn = (vmemmap_addr - VMEMMAP_START) / sizeof(Page) + vmemmap_start_pfn
#[inline]
pub fn vmemmap_to_pfn(vaddr: usize) -> usize {
    // SAFETY: same as pfn_to_vmemmap — VMEMMAP_STATS is initialized before use.
    let start_pfn = unsafe { VMEMMAP_STATS.start_pfn };
    start_pfn + (vaddr - VMEMMAP_START) / STRUCT_PAGE_SIZE
}

/// Check if vmemmap is initialized
#[inline]
pub fn is_vmemmap_initialized() -> bool {
    VMEMMAP_INIT.load(core::sync::atomic::Ordering::Acquire)
}

/// Initialize vmemmap mapping
///
/// This function maps the vmemmap region to physical pages.
/// Each physical page can hold 64 page descriptors.
///
/// # Arguments
/// - `start_pfn`: Start PFN of physical memory (e.g., 0x80000 for 0x80000000)
/// - `nr_pages`: Number of physical pages to map
///
/// # Returns
/// - Ok(()) on success
/// - Err(()) if mapping fails
pub fn init_vmemmap(start_pfn: usize, nr_pages: usize) -> Result<(), ()> {
    if VMEMMAP_INIT.swap(true, core::sync::atomic::Ordering::AcqRel) {
        // Already initialized
        return Ok(());
    }

    // Validate nr_pages against MAX_PAGES from page_desc
    // This ensures pfn_to_page() bounds check is consistent with vmemmap mapping
    let max_pages = super::page_desc::MAX_PAGES;
    let effective_nr_pages = nr_pages.min(max_pages);
    if effective_nr_pages != nr_pages {
        crate::println!("vmemmap: nr_pages {} exceeds MAX_PAGES {}, truncating",
            nr_pages, max_pages);
    }

    // Calculate how many vmemmap pages we need
    // Each vmemmap page (4KB) can hold 64 page descriptors
    let vmemmap_pages = (effective_nr_pages + PAGES_PER_VMEMMAP_PAGE - 1) / PAGES_PER_VMEMMAP_PAGE;

    // vmemmap starts at VMEMMAP_START
    // page descriptors are accessed via: VMEMMAP_START + (pfn - start_pfn) * sizeof(Page)
    let vmemmap_start = VMEMMAP_START;
    let vmemmap_end = VMEMMAP_START + effective_nr_pages * STRUCT_PAGE_SIZE;

    // Check if vmemmap range is valid
    if vmemmap_end > VMEMMAP_END {
        return Err(());
    }

    // Use memblock to find a contiguous region for vmemmap pages
    let vmemmap_size = vmemmap_pages * PAGE_SIZE;

    // Calculate the actual physical memory end address
    let phys_end = 0x80000000 + effective_nr_pages * PAGE_SIZE;
    let vmemmap_phys = super::memblock::memblock_find_in_range(
        vmemmap_size,
        0x80000000,
        phys_end,
    );

    let vmemmap_phys = match vmemmap_phys {
        Some(addr) => addr,
        None => return Err(()),
    };

    // Reserve the memory for vmemmap
    if super::memblock::memblock_reserve(vmemmap_phys, vmemmap_size).is_err() {
        return Err(());
    }

    // Zero the vmemmap pages using linear mapping
    // Must use virtual address since MMU is enabled
    let vmemmap_virt = phys_to_virt(PhysAddr::new(vmemmap_phys as u64));
    // SAFETY: vmemmap_virt points to the reserved physical region returned by
    // memblock_find_in_range; vmemmap_size is the exact allocation size.
    unsafe {
        ptr::write_bytes(vmemmap_virt.bits() as *mut u8, 0, vmemmap_size);
    }

    // Map each vmemmap page
    let flags = PageTableEntry::V | PageTableEntry::R |
               PageTableEntry::W | PageTableEntry::A |
               PageTableEntry::D;

    for i in 0..vmemmap_pages {
        let vaddr = vmemmap_start + i * PAGE_SIZE;
        let paddr = vmemmap_phys + i * PAGE_SIZE;

        // SAFETY: vaddr is within [VMEMMAP_START, VMEMMAP_END) (checked above);
        // paddr is the reserved physical page from memblock.
        unsafe {
            map_kernel_page(vaddr as u64, paddr as u64, flags);
        }
    }

    // Final TLB flush after all mappings - MUST flush before accessing!
    // SAFETY: sfence.vma is a valid RISC-V instruction; must be issued after
    // new page table entries are written.
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
    }

    // Store statistics
    // SAFETY: VMEMMAP_INIT guard ensures single initialization; no concurrent
    // readers until init_vmemmap() returns.
    unsafe {
        VMEMMAP_STATS = VmemmapStats {
            initialized: true,
            vmemmap_base: VMEMMAP_START,
            start_pfn,
            nr_pages: effective_nr_pages,
            vmemmap_pages,
        };
    }

    Ok(())
}

/// Get vmemmap statistics
pub fn vmemmap_stats() -> VmemmapStats {
    // SAFETY: VMEMMAP_STATS is initialized before any call to this function;
    // VmemmapStats is Copy, so this is a plain read of a static.
    unsafe { VMEMMAP_STATS }
}

/// vmemmap statistics
#[derive(Debug, Clone, Copy)]
pub struct VmemmapStats {
    pub initialized: bool,
    pub vmemmap_base: usize,
    pub start_pfn: usize,
    pub nr_pages: usize,
    pub vmemmap_pages: usize,
}
