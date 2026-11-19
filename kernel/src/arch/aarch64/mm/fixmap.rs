//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 fixmap — early fixed mappings.
//!
//! Slot 0 sits just below FIXADDR_TOP (which anchors at VMEMMAP_START)
//! and grows downward, exactly like the riscv64 backend. The UART
//! (PL011) fixmap is the boot-critical one: console output switches from
//! the early identity VA to the fixmap VA once the kernel map is live.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::memory_layout::{PAGE_SHIFT, VMEMMAP_START};

/// Fixmap region size (16MB)
pub const FIXADDR_SIZE: usize = 16 * 1024 * 1024;

/// Top of the fixmap region (anchors below vmemmap)
pub const FIXADDR_TOP: usize = VMEMMAP_START;

/// Bottom of the fixmap region
pub const FIXADDR_START: usize = FIXADDR_TOP - FIXADDR_SIZE;

/// Number of fixmap slots (one page each)
pub const NUM_FIXMAP_SLOTS: usize = FIXADDR_SIZE / (1 << PAGE_SHIFT);

/// Fixmap slot indices
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedAddress {
    /// Reserved slot (index 0)
    FixHole = 0,
    /// Early console (PL011 UART)
    FixEarlycon,
    /// End of permanent fixed addresses
    EndOfPermanentFixed,
    /// Total number of slots (must be last)
    EndOfFixedAddresses,
}

const _: () = assert!((FixedAddress::EndOfFixedAddresses as usize) <= NUM_FIXMAP_SLOTS);

/// Convert fixmap index to virtual address (slot 0 at the top, growing down)
#[inline]
pub const fn fix_to_virt(idx: usize) -> usize {
    FIXADDR_TOP - ((idx + 1) << PAGE_SHIFT)
}

/// Convert a fixmap virtual address back to its slot index
#[inline]
pub const fn virt_to_fix(virt: usize) -> Option<usize> {
    if virt >= FIXADDR_START && virt < FIXADDR_TOP {
        Some((FIXADDR_TOP - virt - 1) >> PAGE_SHIFT)
    } else {
        None
    }
}

/// Check whether a VA is inside the fixmap region
#[inline]
pub const fn is_fixmap_addr(virt: usize) -> bool {
    virt >= FIXADDR_START && virt < FIXADDR_TOP
}

// ==================== UART Fixmap Management ====================

/// PL011 UART physical address (QEMU virt)
pub const UART_PHYS: usize = 0x0900_0000;

/// UART virtual address (set during init; 0 before)
static UART_VIRT_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Initialize the UART fixmap (map PL011 into the FixEarlycon slot)
pub fn init_uart_fixmap() -> usize {
    let virt = fix_to_virt(FixedAddress::FixEarlycon as usize);
    // Device mapping, kernel RW
    // SAFETY: early single-threaded boot; the fixmap slot is exclusively
    // ours and the kernel tables are being built.
    unsafe { super::mmu_init::map_device_page(virt, UART_PHYS, 0) };
    UART_VIRT_ADDR.store(virt, Ordering::Release);
    virt
}

/// Get the UART virtual address (falls back to the physical address
/// during very early boot, exactly like the riscv64 twin)
#[inline]
pub fn uart_virt_addr() -> usize {
    let addr = UART_VIRT_ADDR.load(Ordering::Acquire);
    if addr == 0 {
        UART_PHYS
    } else {
        addr
    }
}

/// Check if the UART fixmap is initialized
#[inline]
pub fn is_uart_fixmap_initialized() -> bool {
    UART_VIRT_ADDR.load(Ordering::Acquire) != 0
}

/// Print fixmap status (diagnostics)
pub fn print_fixmap_status() {
    crate::println!(
        "fixmap: [{:#x}-{:#x}) uart={} ({} slots)",
        FIXADDR_START,
        FIXADDR_TOP,
        if is_uart_fixmap_initialized() { "mapped" } else { "unmapped" },
        NUM_FIXMAP_SLOTS
    );
}
