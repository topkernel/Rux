//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 fixmap — early fixed virtual slots.
//!
//! The x86 console is port-I/O (no UART MMIO fixmap needed); slots are
//! reserved for early ECAM/IOAPIC/LAPIC peeks before the device window
//! is built. X86-TODO(agent x86-mm): set_fixmap via map_kernel_page.

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

/// Map a fixed slot. X86-TODO(agent x86-mm)
pub unsafe fn set_fixmap(_idx: FixedAddress, _phys: usize, _flags: u64) {}

/// Unmap a fixed slot. X86-TODO(agent x86-mm)
pub unsafe fn clear_fixmap(_idx: FixedAddress) {}

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
