//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Virtual Memory Map (vmemmap) Management
//!
//! This module implements Linux-style vmemmap for page descriptor mapping.
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
use crate::arch::riscv64::mm::{PageTableEntry, map_kernel_page};

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
/// vmemmap_addr = VMEMMAP_START + pfn * sizeof(Page)
#[inline]
pub const fn pfn_to_vmemmap(pfn: usize) -> usize {
    VMEMMAP_START + pfn * STRUCT_PAGE_SIZE
}

/// Convert vmemmap virtual address to PFN
///
/// pfn = (vmemmap_addr - VMEMMAP_START) / sizeof(Page)
#[inline]
pub const fn vmemmap_to_pfn(vaddr: usize) -> usize {
    (vaddr - VMEMMAP_START) / STRUCT_PAGE_SIZE
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

    // Calculate how many vmemmap pages we need
    // Each vmemmap page (4KB) can hold 64 page descriptors
    let vmemmap_pages = (nr_pages + PAGES_PER_VMEMMAP_PAGE - 1) / PAGES_PER_VMEMMAP_PAGE;

    // Calculate virtual address range for vmemmap
    let vmemmap_start = pfn_to_vmemmap(start_pfn);
    let vmemmap_end = pfn_to_vmemmap(start_pfn + nr_pages);

    // Check if vmemmap range is valid
    if vmemmap_end > VMEMMAP_END {
        return Err(());
    }

    // Use memblock to find a contiguous region for vmemmap pages
    let vmemmap_size = vmemmap_pages * PAGE_SIZE;

    let vmemmap_phys = super::memblock::memblock_find_in_range(
        vmemmap_size,
        0x80000000,
        0x80000000 + 0x80000000,
    );

    let vmemmap_phys = match vmemmap_phys {
        Some(addr) => addr,
        None => return Err(()),
    };

    // Reserve the memory for vmemmap
    if super::memblock::memblock_reserve(vmemmap_phys, vmemmap_size).is_err() {
        return Err(());
    }

    // Zero the vmemmap pages
    unsafe {
        ptr::write_bytes(vmemmap_phys as *mut u8, 0, vmemmap_size);
    }

    // Map each vmemmap page
    let flags = PageTableEntry::V | PageTableEntry::R |
               PageTableEntry::W | PageTableEntry::A |
               PageTableEntry::D;

    for i in 0..vmemmap_pages {
        let vaddr = vmemmap_start + i * PAGE_SIZE;
        let paddr = vmemmap_phys + i * PAGE_SIZE;

        unsafe {
            map_kernel_page(vaddr as u64, paddr as u64, flags);
        }
    }

    // Final TLB flush after all mappings
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nomem, nostack));
    }

    // Store statistics
    unsafe {
        VMEMMAP_STATS = VmemmapStats {
            initialized: true,
            vmemmap_base: VMEMMAP_START,
            start_pfn,
            nr_pages,
            vmemmap_pages,
        };
    }

    Ok(())
}

/// Get vmemmap statistics
pub fn vmemmap_stats() -> VmemmapStats {
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
