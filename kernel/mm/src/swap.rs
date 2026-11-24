//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Swap Subsystem
//!
//! Provides swap space management for anonymous page reclaim.
//! Swap entries are encoded in PTEs when pages are swapped out,
//! and decoded on page fault to swap pages back in.

#![forbid(unsafe_code)]

extern crate alloc;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;

use rux::mm::PAGE_SIZE;

// ==================== Swap Entry Encoding ====================
//
// The PTE-entry codec is SEAM VOCABULARY (M1b): compiled in
// mm_hooks.rs, named by both the arch fault mechanism and this policy
// module. Re-exported here so the historical swap::/make_/is_ paths
// keep resolving on both sides of the crate boundary.

pub use rux::mm::mm_hooks::{
    is_migration_entry, is_swap_entry, make_migration_entry, make_swap_entry,
    swap_entry_offset, swap_entry_type, MIGRATION_ENTRY_SIGNATURE, SWAP_ENTRY_SIGNATURE,
};

/// Maximum number of swap devices
const MAX_SWAP_DEVICES: usize = 4;

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

// ==================== Swap Device ====================

/// A swap device backed by a block device.
pub struct SwapDevice {
    /// Block device pointer (*const GenDisk as usize)
    disk: AtomicUsize,
    /// Swap type index (0-based; immutable after the const initializer)
    swap_type: u32,
    /// Starting sector on the block device (512-byte units; written once
    /// by the single swap_activate, ordered by the `enabled` Release
    /// store below)
    start_sector: AtomicU64,
    /// Total swap slots (each slot = 1 page = 8 sectors; same init-once
    /// discipline as `start_sector`)
    max_slots: AtomicUsize,
    /// Number of currently used slots
    used_slots: AtomicUsize,
    /// Slot usage bitmap (1 bit per slot)
    slot_bitmap: Spinlock<Vec<u8>>,
    /// Whether this device is enabled
    enabled: AtomicUsize,
}

/// The swap-device registry (INV-SREF-1 shape: machine-lifetime static
/// storage, init-once at swap_activate; every mutable field is atomic or
/// Spinlock-guarded, and `enabled` is stored LAST (Release) so a device
/// observed enabled is fully initialized — the Acquire load in
/// `is_enabled` orders the Relaxed init stores ahead of every reader).
/// The whole table is a plain (non-`mut`) static: no `&mut` aliasing is
/// needed or produced anywhere.
static SWAP_DEVICES: [SwapDevice; MAX_SWAP_DEVICES] = [
    SwapDevice::new_static(0),
    SwapDevice::new_static(1),
    SwapDevice::new_static(2),
    SwapDevice::new_static(3),
];

/// Bounds-checked registry accessor (any index is sound to pass).
fn swap_dev(idx: usize) -> Option<&'static SwapDevice> {
    SWAP_DEVICES.get(idx)
}

/// Map a physical swap page to its PAGE_SIZE mutable kernel slice (the
/// one audited phys-to-slice edge for the swap I/O paths; INV: `phys`
/// is a buddy-allocated page frame the caller owns for the call).
fn swap_page_slice_mut(phys_addr: usize) -> &'static mut [u8] {
    let virt_addr = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr(phys_addr as u64),
    ).0 as usize;
    // page-aligned linear-mapping address of a live page; the
    // PAGE_SIZE length matches the buddy allocation exactly
    // (INV-MM-7 page-slice arm).
    rux::mm::edges::page_slice_mut(virt_addr)
}

/// Shared-slice variant of [`swap_page_slice_mut`] (swap-out reads).
fn swap_page_slice(phys_addr: usize) -> &'static [u8] {
    let virt_addr = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr(phys_addr as u64),
    ).0 as usize;
    // as swap_page_slice_mut; the page is exclusively owned
    // (refcount == 1) during swap-out (INV-MM-7 page-slice arm).
    rux::mm::edges::page_slice(virt_addr)
}

impl SwapDevice {
    const fn new_static(idx: u32) -> Self {
        Self {
            disk: AtomicUsize::new(0),
            swap_type: idx,
            start_sector: AtomicU64::new(0),
            max_slots: AtomicUsize::new(0),
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
            crate::pr_debug!(
                "swap: enabled on disk at sector {}, {} MB, {} slots",
                start_sector,
                crate::config::SWAP_SIZE_MB,
                max_slots,
            );
        }
        Err(e) => {
            crate::pr_debug!("swap: activation refused (errno {}), swap disabled", e);
        }
    }
}

/// Activate the tail-carve swap area on `disk`.
///
/// Shared by boot-time `swap_init()` and the swapon(2) syscall.
/// Returns `(start_sector, max_slots)` on success.
pub fn swap_activate(
    disk: &'static crate::drivers::blkdev::GenDisk,
) -> Result<(u64, usize), i32> {
    // Get disk capacity in 512-byte sectors (typed handle from
    // find_block_device()/get_disk(); INV-BLKDEV-1).
    let capacity = disk.capacity.load(Ordering::Relaxed) as u64;
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
            // Expected configuration outcome (image sized to the fs): the
            // structured boot row already reports swap as not-active.
            crate::pr_debug!(
                "swap: tail carve at sector {} overlaps ext4 fs (ends sector {})",
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

    // SWAP_DEVICES[0] fields are atomics or Spinlock-guarded;
    // `enabled` is stored LAST (Release) so readers never see a
    // half-initialized device (see nr_active_swap / swap_alloc_slot)
    // — INV-MM-7 init arm. (swap_type is 0 from the const initializer —
    // the old redundant `= 0` store went away with the `static mut`.)
    {
        let dev = swap_dev(0).expect("SWAP_DEVICES[0] exists");
        dev.disk.store(disk as *const _ as usize, Ordering::Release);
        dev.start_sector.store(start_sector, Ordering::Relaxed);
        dev.max_slots.store(max_slots, Ordering::Relaxed);
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
    if let Some(dev) = swap_dev(0) {
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
fn ext4_fs_end_sector(disk: &'static crate::drivers::blkdev::GenDisk) -> Option<u64> {
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
pub fn swap_boot_disk() -> Option<&'static crate::drivers::blkdev::GenDisk> {
    find_block_device()
}

/// Check whether any swap device is active.
#[inline]
pub fn nr_active_swap() -> bool {
    swap_dev(0).map(|d| d.is_enabled()).unwrap_or(false)
}

/// Allocate a free swap slot.
///
/// Returns `(swap_type, swap_offset)` on success, or `None` if no free slots.
pub fn swap_alloc_slot() -> Option<(u32, u64)> {
    for i in 0..MAX_SWAP_DEVICES {
        if let Some(dev) = swap_dev(i) {
            if !dev.is_enabled() {
                continue;
            }

            let mut bitmap = dev.slot_bitmap.lock();
            let max_slots = dev.max_slots.load(Ordering::Relaxed);

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
    if let Some(dev) = swap_dev(swap_type as usize) {
        if !dev.is_enabled() || (offset as usize) >= dev.max_slots.load(Ordering::Relaxed) {
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
    let buf = swap_page_slice_mut(phys_addr);

    match crate::drivers::blkdev::blkdev_read(device, sector, buf) {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Write a page from physical memory to the swap device.
///
/// # Arguments
/// - `swap_type`: Swap device index
/// SP-5 round-4: swap-write latency census — the reclaim round's
/// per-page cost driver (a 32-page round of >10 ms writes is a 300+
/// ms no-progress window for allocation stalls; the fake-OOM kill
/// chain starts exactly there). Relaxed counters, read by the OOM
/// forensic dump.
static SWAPW_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SWAPW_SLOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SWAPW_MAX_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// - `offset`: Slot offset
/// - `phys_addr`: Physical address of the source page
pub fn swap_write_page(swap_type: u32, offset: u64, phys_addr: usize) -> Result<(), i32> {
    let device = get_swap_disk(swap_type)?;

    let sector = get_swap_sector(swap_type, offset);
    let buf = swap_page_slice(phys_addr);

    // SP-5 round-4: swap-write latency census (module statics above).
    let t0 = crate::timer::read_time();
    let r = crate::drivers::blkdev::blkdev_write(device, sector, buf);
    let dt_ns = crate::timer::read_time().saturating_sub(t0) * 1_000_000_000
        / crate::config::TIMER_CLOCK_FREQ_HZ;
    SWAPW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if dt_ns > 10_000_000 {
        SWAPW_SLOW.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    SWAPW_MAX_NS.fetch_max(dt_ns, core::sync::atomic::Ordering::Relaxed);
    match r {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// SP-5: the swap-write latency census (see swap_write_page) —
/// (writes, writes over 10 ms, max write ns).
pub fn swap_write_census() -> (u64, u64, u64) {
    (
        SWAPW_N.load(core::sync::atomic::Ordering::Relaxed),
        SWAPW_SLOW.load(core::sync::atomic::Ordering::Relaxed),
        SWAPW_MAX_NS.load(core::sync::atomic::Ordering::Relaxed),
    )
}

// SwapStats is seam vocabulary (framework /proc consumers match on its
// fields); re-exported so swap::SwapStats keeps resolving.
pub use rux::mm::mm_hooks::SwapStats;

/// Get swap statistics.
pub fn swap_stats() -> SwapStats {
    let mut total = 0usize;
    let mut free = 0usize;

    for i in 0..MAX_SWAP_DEVICES {
        if let Some(dev) = swap_dev(i) {
            if !dev.is_enabled() {
                continue;
            }
            total += dev.max_slots.load(Ordering::Relaxed);
            free += dev.max_slots.load(Ordering::Relaxed)
                - dev.used_slots.load(Ordering::Relaxed);
        }
    }

    SwapStats { swap_total: total, swap_free: free }
}

// ==================== Internal Helpers ====================

/// Find the first available block device (VirtIO-blk).
fn find_block_device() -> Option<&'static crate::drivers::blkdev::GenDisk> {
    // Try common major numbers for VirtIO-blk
    for major in 0..256 {
        if let Some(disk) = crate::drivers::blkdev::get_disk(major) {
            return Some(disk);
        }
    }
    None
}

/// Get the block device handle for a swap type.
fn get_swap_disk(swap_type: u32) -> Result<&'static crate::drivers::blkdev::GenDisk, i32> {
    if (swap_type as usize) >= MAX_SWAP_DEVICES {
        return Err(-22); // EINVAL
    }
    let Some(dev) = swap_dev(swap_type as usize) else {
        return Err(-22);
    };
    if !dev.is_enabled() {
        return Err(-22);
    }
    // the stored bits are a registered &'static GenDisk
    // (INV-BLKDEV-1) put there by swap_activate (INV-MM-2 arm).
    Ok(crate::syscall::edges::boxed_ref(
        dev.disk.load(Ordering::Acquire) as *const crate::drivers::blkdev::GenDisk,
    ))
}

/// Convert swap offset to 512-byte sector number.
fn get_swap_sector(swap_type: u32, offset: u64) -> u64 {
    if let Some(dev) = swap_dev(swap_type as usize) {
        if dev.is_enabled() {
            return dev.start_sector.load(Ordering::Relaxed)
                + offset * (PAGE_SIZE as u64 / 512);
        }
    }
    offset * (PAGE_SIZE as u64 / 512)
}
