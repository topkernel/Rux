//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Swap Subsystem
//!
//! Provides swap space management for anonymous page reclaim.
//! Swap entries are encoded in PTEs when pages are swapped out,
//! and decoded on page fault to swap pages back in.

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;

use super::PAGE_SIZE;

// ==================== Swap Entry Encoding ====================

/// Signature bit in a swap PTE — distinguishes swap entries from
/// genuinely-empty (zeroed) PTEs. Stored in bit 62.
pub const SWAP_ENTRY_SIGNATURE: u64 = 1u64 << 62;

/// Maximum number of swap devices
const MAX_SWAP_DEVICES: usize = 4;

/// Build a swap entry value suitable for storing in a PTE.
///
/// Layout:
///   Bit 62: signature (1)
///   Bits [9:8]: swap type (up to 4 devices)
///   Bits [53:10]: swap offset (up to 2^44 pages per device)
///   Bit 0 (V): 0 (triggers page fault)
#[inline]
pub fn make_swap_entry(swap_type: u32, swap_offset: u64) -> u64 {
    SWAP_ENTRY_SIGNATURE
        | ((swap_type as u64 & 0x3) << 8)
        | ((swap_offset & 0x003F_FFFF_FFFF) << 10)
}

/// Check whether a raw PTE value represents a swap entry.
#[inline]
pub fn is_swap_entry(pte: u64) -> bool {
    (pte & SWAP_ENTRY_SIGNATURE) != 0
}

/// Extract the swap type from a swap entry.
#[inline]
pub fn swap_entry_type(pte: u64) -> u32 {
    ((pte >> 8) & 0x3) as u32
}

/// Extract the swap offset from a swap entry.
#[inline]
pub fn swap_entry_offset(pte: u64) -> u64 {
    (pte >> 10) & 0x003F_FFFF_FFFF
}

// ==================== Swap Device ====================

/// A swap device backed by a block device.
pub struct SwapDevice {
    /// Block device pointer (*const GenDisk as usize)
    disk: AtomicUsize,
    /// Swap type index (0-based)
    swap_type: u32,
    /// Starting sector on the block device (512-byte units)
    start_sector: u64,
    /// Total swap slots (each slot = 1 page = 8 sectors)
    max_slots: usize,
    /// Number of currently used slots
    used_slots: AtomicUsize,
    /// Slot usage bitmap (1 bit per slot)
    slot_bitmap: Spinlock<Vec<u8>>,
    /// Whether this device is enabled
    enabled: AtomicUsize,
}

static mut SWAP_DEVICES: [SwapDevice; MAX_SWAP_DEVICES] = [
    SwapDevice::new_static(0),
    SwapDevice::new_static(1),
    SwapDevice::new_static(2),
    SwapDevice::new_static(3),
];

impl SwapDevice {
    const fn new_static(idx: u32) -> Self {
        Self {
            disk: AtomicUsize::new(0),
            swap_type: idx,
            start_sector: 0,
            max_slots: 0,
            used_slots: AtomicUsize::new(0),
            slot_bitmap: Spinlock::new(Vec::new()),
            enabled: AtomicUsize::new(0),
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire) != 0
    }
}

// ==================== Public API ====================

/// Initialize the swap subsystem.
///
/// Called during late boot after block devices are initialized.
/// Uses the first available VirtIO-blk disk and carves out a swap
/// area at the end of the device (as configured in Kernel.toml).
pub fn swap_init() {
    if !crate::config::ENABLE_SWAP {
        crate::println!("swap: disabled by config");
        return;
    }

    // Find first available block device
    let disk = match find_block_device() {
        Some(d) => d,
        None => {
            crate::println!("swap: no block device found, swap disabled");
            return;
        }
    };

    // Get disk capacity in 512-byte sectors
    // SAFETY: disk is a non-null pointer from find_block_device(), which
    // returns valid GenDisk pointers from the block device layer.
    let capacity = unsafe { (*disk).capacity.load(Ordering::Relaxed) } as u64;
    if capacity == 0 {
        crate::println!("swap: block device has zero capacity");
        return;
    }

    let swap_bytes = (crate::config::SWAP_SIZE_MB as u64) * 1024 * 1024;
    let swap_sectors = swap_bytes / 512;
    let max_slots = (swap_bytes / PAGE_SIZE as u64) as usize;

    if swap_sectors > capacity {
        crate::println!(
            "swap: requested {} MB ({} sectors) exceeds disk capacity {} sectors, truncating",
            crate::config::SWAP_SIZE_MB,
            swap_sectors,
            capacity,
        );
        return;
    }

    // Swap area starts at the end of the disk
    let start_sector = capacity - swap_sectors;

    // Allocate bitmap (1 bit per slot)
    let bitmap_bytes = (max_slots + 7) / 8;
    let mut bitmap = Vec::with_capacity(bitmap_bytes);
    for _ in 0..bitmap_bytes {
        bitmap.push(0);
    }

    // SAFETY: swap_init runs during late boot (single-threaded) before any
    // concurrent access to SWAP_DEVICES.
    unsafe {
        let dev = &mut SWAP_DEVICES[0];
        dev.disk.store(disk as usize, Ordering::Release);
        dev.swap_type = 0;
        dev.start_sector = start_sector;
        dev.max_slots = max_slots;
        dev.used_slots.store(0, Ordering::Release);
        *dev.slot_bitmap.lock() = bitmap;
        dev.enabled.store(1, Ordering::Release);
    }

    crate::println!(
        "swap: enabled on disk at sector {}, {} MB, {} slots",
        start_sector,
        crate::config::SWAP_SIZE_MB,
        max_slots,
    );
}

/// Check whether any swap device is active.
#[inline]
pub fn nr_active_swap() -> bool {
    // SAFETY: SWAP_DEVICES is initialized by swap_init() before this is called;
    // is_enabled() only reads an atomic field.
    unsafe { SWAP_DEVICES[0].is_enabled() }
}

/// Allocate a free swap slot.
///
/// Returns `(swap_type, swap_offset)` on success, or `None` if no free slots.
pub fn swap_alloc_slot() -> Option<(u32, u64)> {
    for i in 0..MAX_SWAP_DEVICES {
        // SAFETY: SWAP_DEVICES is initialized by swap_init(); i is in-bounds.
        unsafe {
            let dev = &SWAP_DEVICES[i];
            if !dev.is_enabled() {
                continue;
            }

            let mut bitmap = dev.slot_bitmap.lock();
            let max_slots = dev.max_slots;

            for byte_idx in 0..bitmap.len() {
                let byte = bitmap[byte_idx];
                if byte == 0xFF {
                    continue; // All bits set
                }
                for bit in 0..8 {
                    if byte & (1 << bit) == 0 {
                        let slot = byte_idx * 8 + bit;
                        if slot >= max_slots {
                            break;
                        }
                        // Mark as used
                        bitmap[byte_idx] |= 1 << bit;
                        drop(bitmap);
                        dev.used_slots.fetch_add(1, Ordering::Release);
                        return Some((dev.swap_type, slot as u64));
                    }
                }
            }
        }
    }
    None
}

/// Free a swap slot.
pub fn swap_free_slot(swap_type: u32, offset: u64) {
    if (swap_type as usize) >= MAX_SWAP_DEVICES {
        return;
    }
    // SAFETY: swap_type is bounds-checked above; SWAP_DEVICES is initialized.
    unsafe {
        let dev = &SWAP_DEVICES[swap_type as usize];
        if !dev.is_enabled() || (offset as usize) >= dev.max_slots {
            return;
        }

        let byte_idx = (offset as usize) / 8;
        let bit = (offset as usize) % 8;

        let mut bitmap = dev.slot_bitmap.lock();
        if byte_idx < bitmap.len() {
            bitmap[byte_idx] &= !(1 << bit);
        }
        drop(bitmap);
        dev.used_slots.fetch_sub(1, Ordering::Release);
    }
}

/// Read a page from the swap device into physical memory.
///
/// # Arguments
/// - `swap_type`: Swap device index
/// - `offset`: Slot offset
/// - `phys_addr`: Physical address of the target page
pub fn swap_read_page(swap_type: u32, offset: u64, phys_addr: usize) -> Result<(), i32> {
    let device = get_swap_disk(swap_type)?;

    let sector = get_swap_sector(swap_type, offset);
    // Convert physical address to virtual address before dereferencing.
    // After MMU init, physical addresses are not directly accessible.
    let virt_addr = crate::arch::riscv64::mm::memory_layout::phys_to_virt(
        crate::arch::riscv64::mm::memory_layout::PhysAddr(phys_addr as u64)
    ).0 as usize;
    // SAFETY: virt_addr is a valid, page-aligned virtual address mapped from
    // the buddy allocator's physical page; PAGE_SIZE is the exact allocation size.
    let buf = unsafe {
        core::slice::from_raw_parts_mut(virt_addr as *mut u8, PAGE_SIZE)
    };

    match crate::drivers::blkdev::blkdev_read(device, sector, buf) {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Write a page from physical memory to the swap device.
///
/// # Arguments
/// - `swap_type`: Swap device index
/// - `offset`: Slot offset
/// - `phys_addr`: Physical address of the source page
pub fn swap_write_page(swap_type: u32, offset: u64, phys_addr: usize) -> Result<(), i32> {
    let device = get_swap_disk(swap_type)?;

    let sector = get_swap_sector(swap_type, offset);
    // Convert physical address to virtual address before dereferencing.
    let virt_addr = crate::arch::riscv64::mm::memory_layout::phys_to_virt(
        crate::arch::riscv64::mm::memory_layout::PhysAddr(phys_addr as u64)
    ).0 as usize;
    // SAFETY: virt_addr is a valid virtual address mapped from the physical page;
    // the page is exclusively owned (refcount == 1) during swap-out.
    let buf = unsafe {
        core::slice::from_raw_parts(virt_addr as *const u8, PAGE_SIZE)
    };

    match crate::drivers::blkdev::blkdev_write(device, sector, buf) {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Swap statistics for /proc/meminfo.
pub struct SwapStats {
    pub swap_total: usize,
    pub swap_free: usize,
}

/// Get swap statistics.
pub fn swap_stats() -> SwapStats {
    let mut total = 0usize;
    let mut free = 0usize;

    for i in 0..MAX_SWAP_DEVICES {
        // SAFETY: SWAP_DEVICES is initialized by swap_init(); i is in-bounds.
        unsafe {
            let dev = &SWAP_DEVICES[i];
            if !dev.is_enabled() {
                continue;
            }
            total += dev.max_slots;
            free += dev.max_slots - dev.used_slots.load(Ordering::Relaxed);
        }
    }

    SwapStats { swap_total: total, swap_free: free }
}

// ==================== Internal Helpers ====================

/// Find the first available block device (VirtIO-blk).
fn find_block_device() -> Option<*const crate::drivers::blkdev::GenDisk> {
    // Try common major numbers for VirtIO-blk
    for major in 0..256 {
        if let Some(disk) = crate::drivers::blkdev::get_disk(major) {
            if !disk.is_null() {
                return Some(disk);
            }
        }
    }
    None
}

/// Get the block device pointer for a swap type.
fn get_swap_disk(swap_type: u32) -> Result<*const crate::drivers::blkdev::GenDisk, i32> {
    if (swap_type as usize) >= MAX_SWAP_DEVICES {
        return Err(-22); // EINVAL
    }
    // SAFETY: swap_type is bounds-checked above; SWAP_DEVICES is initialized.
    unsafe {
        let dev = &SWAP_DEVICES[swap_type as usize];
        if !dev.is_enabled() {
            return Err(-22);
        }
        Ok(dev.disk.load(Ordering::Acquire) as *const crate::drivers::blkdev::GenDisk)
    }
}

/// Convert swap offset to 512-byte sector number.
fn get_swap_sector(swap_type: u32, offset: u64) -> u64 {
    if (swap_type as usize) < MAX_SWAP_DEVICES {
        // SAFETY: swap_type is bounds-checked above; SWAP_DEVICES is initialized.
        unsafe {
            let dev = &SWAP_DEVICES[swap_type as usize];
            if dev.is_enabled() {
                return dev.start_sector + offset * (PAGE_SIZE as u64 / 512);
            }
        }
    }
    offset * (PAGE_SIZE as u64 / 512)
}
