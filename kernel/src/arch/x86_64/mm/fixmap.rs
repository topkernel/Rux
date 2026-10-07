//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 fixmap — early fixed virtual slots.
//!
//! The x86 console is port-I/O (no UART MMIO fixmap needed); slots are
//! reserved for early ECAM/IOAPIC/LAPIC peeks before the device window
//! is built. set_fixmap/clear_fixmap map/unmap through map_kernel_page;
//! the fixmap PUD/PMD is pre-linked by mmu_init::init().

use super::memory_layout::*;

pub const FIXADDR_SIZE: usize = 16 * 1024 * 1024;
pub const FIXADDR_TOP: usize = 0xffff_ffff_ff60_0000;
pub const FIXADDR_START: usize = FIXADDR_TOP - FIXADDR_SIZE;

pub const NUM_FIXMAP_ENTRIES: usize = 4096;

/// Fixed slots (index = entry, maps top-down like Linux)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum FixedAddress {
    EarlyConsole = 0, // unused on x86 (portio), kept for shape
    EarlyEcam,
    EarlyIoapic,
    EarlyLapic,
    DynamicBase, // first dynamic slot
}

/// Slot index → virtual address
pub const fn fix_to_virt(idx: usize) -> usize {
    FIXADDR_TOP - (idx + 1) * (PAGE_SIZE as usize)
}

/// Virtual address → slot index
pub const fn virt_to_fix(virt: usize) -> Option<usize> {
    if virt >= FIXADDR_START && virt < FIXADDR_TOP {
        Some((FIXADDR_TOP - virt - 1) / (PAGE_SIZE as usize))
    } else {
        None
    }
}

pub const fn is_fixmap_addr(virt: usize) -> bool {
    virt >= FIXADDR_START && virt < FIXADDR_TOP
}

/// Map a fixed slot: `phys` at the slot's virtual address with `flags`.
///
/// The fixmap PUD is pre-linked by mmu_init::init(), so no intermediate
/// table allocation can happen here (safe to call before the buddy
/// allocator exists).
pub unsafe fn set_fixmap(idx: FixedAddress, phys: usize, flags: u64) {
    let virt = fix_to_virt(idx as usize);
    super::mmu_init::map_kernel_page(virt as u64, phys as u64, flags);
}

/// Unmap a fixed slot (zero the leaf PTE).
pub unsafe fn clear_fixmap(idx: FixedAddress) {
    use super::pagetable::PageTableEntry;
    use super::memory_layout::PAGE_SHIFT;
    let virt = fix_to_virt(idx as usize) as u64;
    let a = super::memory_layout::VirtAddr::new(virt);
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    // SAFETY: walks the static kernel root for a fixmap VA (kernel-half
    // links are shared in every root); every level is guaranteed present
    // (PUD pre-linked at boot; the slot was mapped by a prior set_fixmap).
    unsafe {
        let root = super::mmu_init::get_page_table_virt(
            super::mmu_init::get_kernel_page_table_ppn() << PAGE_SHIFT,
        );
        let pte4 = (*root).get(vpn4);
        if !pte4.is_valid() {
            return;
        }
        let table3 = super::mmu_init::get_page_table_virt(pte4.ppn() << PAGE_SHIFT);
        let pte3 = (*table3).get(vpn3);
        if !pte3.is_valid() || pte3.is_leaf() {
            return;
        }
        let table2 = super::mmu_init::get_page_table_virt(pte3.ppn() << PAGE_SHIFT);
        let pte2 = (*table2).get(vpn2);
        if !pte2.is_valid() || pte2.is_leaf() {
            return;
        }
        let table1 = super::mmu_init::get_page_table_virt(pte2.ppn() << PAGE_SHIFT);
        (*table1).set(vpn1, PageTableEntry::from_bits(0));
        crate::arch::cpu::invlpg(virt);
    }
}

// ---- UART parity shims: x86 console is port I/O — no fixmap UART ----

pub const UART_PHYS: usize = 0;

pub fn init_uart_fixmap() -> usize {
    0
}

pub fn uart_virt_addr() -> usize {
    0
}

pub fn is_uart_fixmap_initialized() -> bool {
    false
}

/// Copy fixmap PTEs into a user root (interface parity; x86 shares the
/// kernel PML4 half instead). X86-TODO: confirm callers can no-op.
pub unsafe fn copy_fixmap_to_user(_user_root_ppn: u64) {}

pub fn print_fixmap_status() {}
