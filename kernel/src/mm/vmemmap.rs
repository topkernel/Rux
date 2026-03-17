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
//!
//! vmemmap layout (Sv39):
//! VMEMMAP_START: 0xffffffd200000000
//! VMEMMAP_END:   0xffffffd400000000
//! Size: 128GB virtual space

use crate::println;
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
        println!("vmemmap: Range exceeds VMEMMAP_END");
        println!("  start_pfn: {:#x}", start_pfn);
        println!("  nr_pages: {:#x} ({})", nr_pages, nr_pages);
        println!("  vmemmap_start: {:#x}", vmemmap_start);
        println!("  vmemmap_end: {:#x}", vmemmap_end);
        println!("  VMEMMAP_END: {:#x}", VMEMMAP_END);
        return Err(());
    }

    println!("vmemmap: Initializing for {} pages ({} MB physical)",
             nr_pages, nr_pages * PAGE_SIZE / (1024 * 1024));
    println!("vmemmap: Virtual range {:#x} - {:#x} (size: {} MB)",
             vmemmap_start, vmemmap_end, (vmemmap_end - vmemmap_start) / (1024 * 1024));
    println!("vmemmap: Need {} descriptor pages ({} KB)",
             vmemmap_pages, vmemmap_pages * PAGE_SIZE / 1024);

    // Use memblock to find a contiguous region for vmemmap pages
    // We need vmemmap_pages * PAGE_SIZE bytes of contiguous physical memory
    let vmemmap_size = vmemmap_pages * PAGE_SIZE;

    // Try to allocate from memblock
    // Function signature: memblock_find_in_range(size, min_addr, max_addr) -> Option<usize>
    let vmemmap_phys = super::memblock::memblock_find_in_range(
        vmemmap_size,            // Size of memory needed
        0x80000000,              // Start of physical memory
        0x80000000 + 0x80000000, // End of 2GB
    );

    let vmemmap_phys = match vmemmap_phys {
        Some(addr) => addr,
        None => {
            println!("vmemmap: Failed to find contiguous memory for {} bytes", vmemmap_size);
            return Err(());
        }
    };

    // Reserve the memory for vmemmap
    if super::memblock::memblock_reserve(vmemmap_phys, vmemmap_size).is_err() {
        println!("vmemmap: Failed to reserve memory");
        return Err(());
    }

    println!("vmemmap: Allocated physical pages at {:#x} - {:#x}",
             vmemmap_phys, vmemmap_phys + vmemmap_size);

    // Zero the vmemmap pages
    unsafe {
        ptr::write_bytes(vmemmap_phys as *mut u8, 0, vmemmap_size);
    }

    // Map each vmemmap page
    // The vmemmap region is mapped to the allocated physical pages
    println!("vmemmap: Mapping {} pages (vaddr {:#x}, paddr {:#x})...",
             vmemmap_pages, vmemmap_start, vmemmap_phys);
    for i in 0..vmemmap_pages {
        let vaddr = vmemmap_start + i * PAGE_SIZE;
        let paddr = vmemmap_phys + i * PAGE_SIZE;

        // Map the vmemmap virtual address to the physical page
        // Use kernel flags: V | R | W | A | D
        let flags = PageTableEntry::V | PageTableEntry::R |
                   PageTableEntry::W | PageTableEntry::A |
                   PageTableEntry::D;

        unsafe {
            map_kernel_page(vaddr as u64, paddr as u64, flags);
        }

        // Debug: verify each mapping immediately
        if i < 5 || i >= vmemmap_pages - 2 {
            unsafe {
                let test_ptr = vaddr as *const u64;
                let _val = core::ptr::read_volatile(test_ptr);
                println!("vmemmap: Page {} at {:#x} verified (value: {:#x})", i, vaddr, _val);
            }
        }
    }

    // Verify the mapping works by reading from the first page descriptor
    println!("vmemmap: Verifying mapping...");
    unsafe {
        let first_page = vmemmap_start as *const Page;
        // Try to read the first page descriptor's refcount (public method)
        let _refcount = (*first_page).refcount();
        println!("vmemmap: First page refcount: {}", _refcount);

        // Try to read a page descriptor in the middle of the range
        let mid_offset = nr_pages / 2;
        let mid_addr = vmemmap_start + mid_offset * STRUCT_PAGE_SIZE;
        let mid_page = mid_addr as *const Page;
        let _mid_refcount = (*mid_page).refcount();
        println!("vmemmap: Mid page (offset {}) refcount: {}", mid_offset, _mid_refcount);

        // Try to read the page that's causing the fault
        // Fault address is 0xffffff96020b8008, which is offset 0xB8008 from vmemmap_start
        let fault_offset = 0xB8008usize;
        let fault_addr = vmemmap_start + fault_offset;
        println!("vmemmap: Testing fault address {:#x}...", fault_addr);
        let fault_page = fault_addr as *const Page;
        let _fault_refcount = (*fault_page).refcount();
        println!("vmemmap: Fault address page refcount: {}", _fault_refcount);
    }
    println!("vmemmap: Mapping verified successfully.");

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

    println!("vmemmap: Successfully mapped {} descriptor pages", vmemmap_pages);
    println!("vmemmap: Page descriptors at {:#x} - {:#x}",
             vmemmap_start, vmemmap_start + nr_pages * STRUCT_PAGE_SIZE);

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

/// Print vmemmap information
pub fn print_vmemmap_info() {
    let stats = vmemmap_stats();
    println!("vmemmap:");
    println!("  Initialized:   {}", stats.initialized);
    println!("  Base:          {:#x}", stats.vmemmap_base);
    println!("  Start PFN:     {:#x}", stats.start_pfn);
    println!("  Nr pages:      {} ({} MB)", stats.nr_pages, stats.nr_pages * PAGE_SIZE / (1024 * 1024));
    println!("  Vmemmap pages: {} ({} KB)", stats.vmemmap_pages, stats.vmemmap_pages * PAGE_SIZE / 1024);
}
