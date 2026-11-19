//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Fixmap implementation for RISC-V Sv39
//!
//! This module provides fixed virtual address mappings for early boot devices.
//! Devices like UART are mapped to fixed
//! kernel virtual addresses to avoid conflicts with user space addresses.

use core::sync::atomic::{AtomicUsize, Ordering};

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
const _: () = assert!((FixedAddress::EndOfFixedAddresses as usize) <= NUM_FIXMAP_SLOTS);

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
/// Maps the physical address to the fixed virtual address slot.
///
/// # Safety
/// This function modifies the kernel page table directly.
/// Caller must ensure the physical address is valid device memory.
///
/// # Arguments
/// - `idx`: Fixmap slot index
/// - `phys`: Physical address to map (must be page-aligned)
/// - `flags`: Page table entry flags
pub unsafe fn set_fixmap(idx: FixedAddress, phys: usize, flags: u64) {
    let idx_usize = idx as usize;
    let virt = fix_to_virt(idx_usize);
    let virt_addr = super::VirtAddr::new(virt as u64);
    let phys_addr = super::PhysAddr::new(phys as u64);

    // Get kernel root page table PPN (physical)
    let root_ppn = super::mmu_init::root_page_table_ppn();

    map_page(root_ppn, virt_addr, phys_addr, flags);

    // Flush TLB for this address
    core::arch::asm!(
        "sfence.vma {virt}, zero",
        virt = in(reg) virt,
        options(nomem, nostack)
    );
}

/// Clear a fixmap entry
///
/// Unmaps the fixed virtual address slot.
///
/// # Safety
/// This function modifies the kernel page table directly.
pub unsafe fn clear_fixmap(idx: FixedAddress) {
    let idx_usize = idx as usize;
    let virt = fix_to_virt(idx_usize);

    // Get the page table entry and clear it
    let vpn2 = (virt >> 30) & 0x1FF;
    let vpn1 = (virt >> 21) & 0x1FF;
    let vpn0 = (virt >> 12) & 0x1FF;

    let root = &mut ROOT_PAGE_TABLE;
    let pte2 = root.get(vpn2);

    if pte2.is_valid() {
        let ppn1 = pte2.ppn();
        let table1_phys = ppn1 << PAGE_SHIFT;
        let table1 = get_page_table_virt(table1_phys);

        let pte1 = (*table1).get(vpn1);
        if pte1.is_valid() && !pte1.is_leaf() {
            let ppn0 = pte1.ppn();
            let table0_phys = ppn0 << PAGE_SHIFT;
            let table0 = get_page_table_virt(table0_phys);

            // Clear the PTE
            (*table0).set(vpn0, PageTableEntry::from_bits(0));

            // Flush TLB
            core::arch::asm!(
                "sfence.vma {virt}, zero",
                virt = in(reg) virt,
                options(nomem, nostack)
            );
        }
    }
}

// ==================== UART Fixmap Management ====================

/// UART physical address
pub const UART_PHYS: usize = 0x10000000;

/// UART virtual address (set during init)
/// Initialized to 0, set to fixmap address after early_ioremap
static UART_VIRT_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Initialize UART fixmap
///
/// Maps UART to FixEarlycon slot and returns the virtual address.
/// This should be called early in boot, before console is used.
///
/// # Returns
/// The virtual address of the UART mapping.
pub fn init_uart_fixmap() -> usize {
    let virt = fix_to_virt(FixedAddress::FixEarlycon as usize);

    // Device memory flags: RW, accessed, dirty, global
    let flags = PageTableEntry::V
        | PageTableEntry::R
        | PageTableEntry::W
        | PageTableEntry::A
        | PageTableEntry::D
        | PageTableEntry::G;

    unsafe {
        set_fixmap(FixedAddress::FixEarlycon, UART_PHYS, flags);
    }

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
pub unsafe fn copy_fixmap_to_user(user_root_ppn: u64) {
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
