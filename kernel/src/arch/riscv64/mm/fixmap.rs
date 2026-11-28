//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Fixmap implementation for RISC-V Sv39
//!
//! This module provides fixed virtual address mappings for early boot devices.
//! Devices like UART are mapped to fixed
//! kernel virtual addresses to avoid conflicts with user space addresses.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicUsize, Ordering};

use super::unsafe_edges::PtTable;
use super::{PAGE_SIZE, PAGE_SHIFT, VMEMMAP_START, PageTableEntry, PageTable, map_page, get_page_table_virt};
use super::{ROOT_PAGE_TABLE, alloc_page_table};
use crate::mm::PhysFrame;

// ==================== Fixmap Address Layout ====================
//
// Fixmap region is placed below VMEMMAP_START:
// 0xffffffb8_00000000  - VMEMMAP_START
// 0xffffffb7_fffff000  - FIXADDR_TOP (= VMEMMAP_START)
// 0xffffffb7_f0000000  - FIXADDR_START (FIXADDR_SIZE = 16MB)
//
// This ensures fixmap addresses are in kernel space (bit 38 = 1 in Sv39)

/// Size of fixmap region (16MB)
pub const FIXADDR_SIZE: usize = 16 * 1024 * 1024;

/// Top of fixmap region (exclusive)
pub const FIXADDR_TOP: usize = VMEMMAP_START;

/// Start of fixmap region (inclusive)
pub const FIXADDR_START: usize = FIXADDR_TOP - FIXADDR_SIZE;

// ==================== Fixed Address Indices ====================

/// Number of fixmap slots
/// 16MB / 4KB = 4096 slots
pub const NUM_FIXMAP_SLOTS: usize = FIXADDR_SIZE / (PAGE_SIZE as usize);

/// Fixmap slot indices
///
/// Indices are used to calculate virtual addresses.
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedAddress {
    /// Reserved slot (index 0)
    FixHole = 0,

    /// Early console (UART)
    /// Maps UART physical address to kernel virtual address
    FixEarlycon,

    /// End of permanent fixed addresses
    EndOfPermanentFixed,

    /// Total number of slots (must be last)
    EndOfFixedAddresses,
}

// Compile-time check
pub(crate) const _: () = assert!((FixedAddress::EndOfFixedAddresses as usize) <= NUM_FIXMAP_SLOTS);

// ==================== Fixmap API ====================

/// Convert fixmap index to virtual address
///
/// Formula: __fix_to_virt(idx) = FIXADDR_TOP - ((idx + 1) << PAGE_SHIFT)
///
/// We place slot 0 at the highest address (just below FIXADDR_TOP)
/// and grow downward.
#[inline]
pub const fn fix_to_virt(idx: usize) -> usize {
    FIXADDR_TOP - ((idx + 1) << PAGE_SHIFT as usize)
}

/// Convert virtual address to fixmap index
///
/// Returns None if the address is not in the fixmap region.
#[inline]
pub const fn virt_to_fix(virt: usize) -> Option<usize> {
    if virt < FIXADDR_START || virt >= FIXADDR_TOP {
        return None;
    }
    // Reverse of fix_to_virt
    Some(((FIXADDR_TOP - virt) >> PAGE_SHIFT as usize) - 1)
}

/// Check if address is in fixmap region
#[inline]
pub const fn is_fixmap_addr(virt: usize) -> bool {
    virt >= FIXADDR_START && virt < FIXADDR_TOP
}

// ==================== Fixmap Mapping Functions ====================
/// Set a fixmap entry
///
/// Collected into `arch::arch_floor` (INV-ARCH-FIXMAP-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn set_fixmap(idx: FixedAddress, phys: usize, flags: u64) {
    crate::arch::arch_floor::riscv64::mm::fixmap::set_fixmap(idx, phys, flags)
}
/// Clear a fixmap entry
///
/// Collected into `arch::arch_floor` (INV-ARCH-FIXMAP-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn clear_fixmap(idx: FixedAddress) {
    crate::arch::arch_floor::riscv64::mm::fixmap::clear_fixmap(idx)
}

// ==================== UART Fixmap Management ====================

/// UART physical address
pub const UART_PHYS: usize = 0x10000000;

/// UART virtual address (set during init)
/// Initialized to 0, set to fixmap address after early_ioremap
pub(crate) static UART_VIRT_ADDR: AtomicUsize = AtomicUsize::new(0);
/// Initialize UART fixmap
///
/// Collected into `arch::arch_floor` (INV-ARCH-FIXMAP-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn init_uart_fixmap() -> usize {


                               let virt = fix_to_virt(FixedAddress::FixEarlycon as usize);

                               // Device memory flags: RW, accessed, dirty, global
                               let flags = PageTableEntry::V
                                   | PageTableEntry::R
                                   | PageTableEntry::W
                                   | PageTableEntry::A
                                   | PageTableEntry::D
                                   | PageTableEntry::G;


                                   set_fixmap(FixedAddress::FixEarlycon, UART_PHYS, flags);


                               UART_VIRT_ADDR.store(virt, Ordering::Release);

                               virt

}

/// Get UART virtual address
///
/// Returns the fixmap virtual address for UART.
/// Panics if called before init_uart_fixmap().
#[inline]
pub fn uart_virt_addr() -> usize {
    let addr = UART_VIRT_ADDR.load(Ordering::Acquire);
    if addr == 0 {
        // Fallback to physical address during very early boot
        // This should only happen before init_uart_fixmap is called
        UART_PHYS
    } else {
        addr
    }
}

/// Check if UART fixmap is initialized
#[inline]
pub fn is_uart_fixmap_initialized() -> bool {
    UART_VIRT_ADDR.load(Ordering::Acquire) != 0
}

// ==================== Copy Fixmap to User Page Table ====================

/// Copy fixmap mappings to user page table
///
/// This ensures that user processes can access devices (like UART)
/// during system calls, but not directly from user code.
///
/// # Safety
/// This function modifies the user page table directly.
///
/// # Arguments
/// - `user_root_ppn`: Physical page number of user's root page table
pub fn copy_fixmap_to_user(user_root_ppn: u64) {
    let uart_virt = UART_VIRT_ADDR.load(Ordering::Acquire);
    if uart_virt == 0 {
        return;
    }

    // Map UART at its fixmap virtual address
    let uart_flags = PageTableEntry::V
        | PageTableEntry::R
        | PageTableEntry::W
        | PageTableEntry::A
        | PageTableEntry::D
        | PageTableEntry::G;

    let virt_addr = super::VirtAddr::new(uart_virt as u64);
    let phys_addr = super::PhysAddr::new(UART_PHYS as u64);

    map_page(user_root_ppn, virt_addr, phys_addr, uart_flags);
}

// ==================== Debug Functions ====================

/// Print fixmap status
pub fn print_fixmap_status() {
    crate::println!("Fixmap region: {:#x} - {:#x}", FIXADDR_START, FIXADDR_TOP);
    crate::println!("  UART: phys={:#x}, virt={:#x}",
        UART_PHYS, uart_virt_addr());
}
