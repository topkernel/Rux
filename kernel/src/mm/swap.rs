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

// ==================== Migration Entries (compaction, review 4.16) ====================

/// Signature bit distinguishing a compaction migration entry from empty
/// and swap-entry PTEs. Stored in bit 61 (V=0, so accessing it faults).
pub const MIGRATION_ENTRY_SIGNATURE: u64 = 1u64 << 61;

/// Build a migration-entry marker PTE (installed while a page is being
/// relocated by compaction — see mm/compact.rs).
#[inline]
pub fn make_migration_entry() -> u64 {
    MIGRATION_ENTRY_SIGNATURE
}

/// Check whether a raw PTE value is a migration marker.
#[inline]
pub fn is_migration_entry(pte: u64) -> bool {
    (pte & MIGRATION_ENTRY_SIGNATURE) != 0
}

// ==================== In-flight swap-out tracking (review 4.12) ====================
//
// For exclusively-owned pages the swap-out path installs the swap entry in
// the PTE BEFORE writing the page (unmap-first prevents user stores during
// the block write from being lost). Between PTE-install and write
// completion, a fault on that PTE must WAIT instead of reading the slot.
// A swap cache would track this per-slot on the page; pending vector is
// the minimal equivalent.

/// Slots whose swap entry is installed but whose data write has not yet
/// completed: (swap_type, offset).
static PENDING_WRITES: Spinlock<Vec<(u32, u64)>> = Spinlock::new(Vec::new());

/// Mark a slot as write-in-flight (called after the PTE is replaced, before
/// the device write).
pub fn swap_mark_pending(swap_type: u32, offset: u64) {
    PENDING_WRITES.lock().push((swap_type, offset));
}

/// Clear the in-flight mark after a successful (or failed) write.
pub fn swap_clear_pending(swap_type: u32, offset: u64) {
    PENDING_WRITES
        .lock()
        .retain(|&(t, o)| t != swap_type || o != offset);
}

/// True while the slot's data is still being written — swap-in must wait.
pub fn swap_slot_pending(swap_type: u32, offset: u64) -> bool {
    PENDING_WRITES
        .lock()
        .iter()
        .any(|&(t, o)| t == swap_type && o == offset)
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
///
/// Safety guard (P1 wiring, 2026-09): the tail carve must not overlap
/// the ext4 filesystem that may span the whole disk — mkrootfs images
/// historically filled the entire disk, and a swap write into live fs
/// blocks would silently corrupt the rootfs. `swap_activate` reads the
/// ext4 superblock and refuses to enable swap when the carve overlaps.
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

    match swap_activate(disk) {
        Ok((start_sector, max_slots)) => {
            crate::println!(
                "swap: enabled on disk at sector {}, {} MB, {} slots",
                start_sector,
                crate::config::SWAP_SIZE_MB,
                max_slots,
            );
        }
        Err(e) => {
            crate::println!(
                "swap: activation refused (errno {}), swap disabled",
                e
            );
        }
    }
}

/// Activate the tail-carve swap area on `disk`.
///
/// Shared by boot-time `swap_init()` and the swapon(2) syscall.
/// Returns `(start_sector, max_slots)` on success.
pub fn swap_activate(
    disk: *const crate::drivers::blkdev::GenDisk,
) -> Result<(u64, usize), i32> {
    // Get disk capacity in 512-byte sectors
    // SAFETY: disk is a non-null pointer from find_block_device() (boot) or
    // get_disk() (swapon), both of which return valid GenDisk pointers from
    // the block device layer.
    let capacity = unsafe { (*disk).capacity.load(Ordering::Relaxed) } as u64;
    if capacity == 0 {
        return Err(-5); // EIO: zero-capacity device
    }

    let swap_bytes = (crate::config::SWAP_SIZE_MB as u64) * 1024 * 1024;
    let swap_sectors = swap_bytes / 512;
    let mut max_slots = (swap_bytes / PAGE_SIZE as u64) as usize;

    if swap_sectors > capacity {
        crate::println!(
            "swap: requested {} MB ({} sectors) exceeds disk capacity {} sectors",
            crate::config::SWAP_SIZE_MB,
            swap_sectors,
            capacity,
        );
        return Err(-22); // EINVAL
    }

    // Swap area starts at the end of the disk
    let mut start_sector = capacity - swap_sectors;

    // ext4 overlap guard: the filesystem may span the whole disk — writing
    // swap pages into live fs blocks would corrupt it.
    if let Some(fs_end_sector) = ext4_fs_end_sector(disk) {
        if start_sector < fs_end_sector {
            crate::println!(
                "swap: tail carve at sector {} overlaps ext4 fs (ends sector {}); \
                 grow the disk image past the fs to enable swap",
                start_sector,
                fs_end_sector
            );
            return Err(-16); // EBUSY: area belongs to the filesystem
        }
    }

    // If a mkswap-style header page is present at the carve start, honor
    // its page count (version 1, magic "SWAPSPACE2" at page offset 4086).
    // The boot-time carve does not write a header, so a missing/invalid
    // header falls back to the config-derived slot count.
    let mut header = [0u8; PAGE_SIZE];
    if crate::drivers::blkdev::blkdev_read(disk, start_sector, &mut header).is_ok() {
        if &header[4086..4096] == b"SWAPSPACE2" {
            let version = u32::from_le_bytes(
                header[1024..1028].try_into().unwrap(),
            );
            let last_page = u32::from_le_bytes(
                header[1028..1032].try_into().unwrap(),
            );
            if version != 1 {
                return Err(-22); // EINVAL: unknown swap version
            }
            // mkswap geometry: page 0 is the header, pages 1..=last_page
            // are usable. Slot offsets in this kernel start at 0, so the
            // data area begins one page past the carve start.
            let pages = last_page as usize;
            let sectors_per_page = PAGE_SIZE / 512;
            if pages == 0 || (pages + 1) * sectors_per_page > swap_sectors as usize {
                return Err(-22); // EINVAL: header lies about the carve size
            }
            start_sector += sectors_per_page as u64;
            max_slots = pages;
        }
    }

    // Allocate bitmap (1 bit per slot)
    let bitmap_bytes = (max_slots + 7) / 8;
    let mut bitmap = Vec::with_capacity(bitmap_bytes);
    for _ in 0..bitmap_bytes {
        bitmap.push(0);
    }

    // SAFETY: SWAP_DEVICES[0] fields are atomics or Spinlock-guarded;
    // `enabled` is stored LAST (Release) so readers never see a
    // half-initialized device (see nr_active_swap / swap_alloc_slot).
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

    Ok((start_sector, max_slots))
}

/// Deactivate swap. Refuses with EBUSY while slots are still in use
/// (paged-out data would become unreachable — swap-in-before-disable is
/// not implemented).
pub fn swap_deactivate() -> Result<(), i32> {
    // SAFETY: SWAP_DEVICES[0] is a static; fields are atomic.
    unsafe {
        let dev = &SWAP_DEVICES[0];
        if !dev.is_enabled() {
            return Err(-22); // EINVAL: swap not active
        }
        if dev.used_slots.load(Ordering::Acquire) > 0 {
            return Err(-16); // EBUSY: pages still swapped out
        }
        // Disable FIRST so no new slots are allocated while we tear down.
        dev.enabled.store(0, Ordering::Release);
        dev.disk.store(0, Ordering::Release);
        *dev.slot_bitmap.lock() = Vec::new();
    }
    Ok(())
}

/// Read the ext4 superblock and return the filesystem end in 512-byte
/// sectors, or None when the disk does not carry an ext4 filesystem.
fn ext4_fs_end_sector(disk: *const crate::drivers::blkdev::GenDisk) -> Option<u64> {
    // The ext4 superblock lives at byte offset 1024 (sector 2) regardless
    // of block size; 1024 bytes cover the fields we need.
    let mut sb = [0u8; 1024];
    if crate::drivers::blkdev::blkdev_read(disk, 2, &mut sb).is_err() {
        return None;
    }

    // s_magic @ sb+56 must be 0xEF53
    let magic = u16::from_le_bytes([sb[56], sb[57]]);
    if magic != 0xEF53 {
        return None;
    }

    // s_blocks_count_lo @ sb+4, s_log_block_size @ sb+24
    let blocks_lo = u32::from_le_bytes(sb[4..8].try_into().unwrap()) as u64;
    let log_block_size = u32::from_le_bytes(sb[24..28].try_into().unwrap());
    let block_size: u64 = 1024u64.checked_shl(log_block_size)?;

    Some(blocks_lo.checked_mul(block_size)? / 512)
}

/// Public accessor for the syscall layer: the first registered block
/// device (the same disk selection boot-time swap_init uses).
pub fn swap_boot_disk() -> Option<*const crate::drivers::blkdev::GenDisk> {
    find_block_device()
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
