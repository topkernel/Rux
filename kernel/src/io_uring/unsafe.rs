//! The io_uring subsystem's ONE audited unsafe file (mainline
//! minimal-first consolidation, 2026-11-27): every raw edge the ring
//! lifecycle and the shared-ring window still need lives here behind
//! one audited item per family; mod.rs is safe code over them.
//!
//! Edge families:
//! - the ring instance's Box lifecycle (INV-IOURING-INSTANCE-1)
//! - ring-region kernel memory (INV-IOURING-REGION-1)
//! - lifecycle-lock-pinned ring derefs (INV-IOURING-PIN-1)
//! - the File private-data cell read (INV-IOURING-PRIVDATA-1)
//! - the mmap page-install loop (INV-IOURING-MMAP-1)
//! - the shared-ring SQE/CQE window and head/tail reads
//!   (INV-IOURING-RING-1, already documented on RingWindow)

use super::{IoUring, IoUringCqe, IoUringSqe};
use crate::arch::mm::{PhysAddr, VirtAddr};

// ------------------------------------------------------------------
// Ring instance lifecycle (Box)
// ------------------------------------------------------------------

/// INV-IOURING-INSTANCE-1: every `IoUring` instance reaches its users
/// exclusively as a raw pointer installed by `Box::into_raw` at
/// io_uring_create time; the reference count owns the reclaim, so
/// `from_raw` runs exactly once, at the final unref (or on the two
/// install-failure paths before the fd exists, where no other
/// reference can have been taken yet).
///
/// Reclaim the instance Box (final unref or install-failure path).
pub(super) fn box_reclaim(ptr: *const IoUring) {
    // SAFETY: INV-IOURING-INSTANCE-1 above (exactly-once reclaim).
    unsafe { drop(alloc::boxed::Box::from_raw(ptr as *const IoUring as *mut IoUring)); }
}

/// Drop one reference through a raw instance pointer (the RingRef
/// guard's drop and the close-path take; both run with the lifecycle
/// proof of INV-IOURING-PIN-1 / INV-IOURING-PRIVDATA-1).
pub(super) fn unref_at(ptr: *const IoUring) {
    // SAFETY: INV-IOURING-INSTANCE-1 — the pointer is a live
    // into_raw instance while a reference is held.
    unsafe { (*ptr).unref() };
}

// ------------------------------------------------------------------
// Ring-region kernel memory
// ------------------------------------------------------------------

/// INV-IOURING-REGION-1: ring regions are whole power-of-two page
/// blocks allocated from the page allocator for the ring's lifetime;
/// the kernel virtual address of a freshly allocated block is the
/// arch's direct map translation of its physical address, valid until
/// free_ring_region.
///
/// Direct-map kernel address of a freshly allocated region block.
pub(super) fn region_kvirt(phys: usize) -> *mut u8 {
    // SAFETY: INV-IOURING-REGION-1 — direct-map translation of a live
    // allocation.
    unsafe {
        crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(phys as u64)).bits()
            as *mut u8
    }
}

/// Zero a freshly allocated region block (pre-publication).
pub(super) fn region_zero(kvirt: *mut u8, bytes: usize) {
    // SAFETY: INV-IOURING-REGION-1 — the block is `bytes` long and not
    // yet published to userspace.
    unsafe { core::ptr::write_bytes(kvirt, 0, bytes); }
}

/// Drop the owner's per-page reference for one region page (the
/// free_ring_region walk; only frees when user mappings are gone).
pub(super) fn region_page_put(page: *mut crate::mm::page_desc::Page) -> i32 {
    // SAFETY: the caller null-checked the pfn_to_page_mut result; the
    // descriptor exists for this in-RAM page (INV-IOURING-REGION-1).
    unsafe { (*page).put_page() }
}

/// Take a mapping reference on one region page (the mmap install walk,
/// review 2R.6).
pub(super) fn region_page_get(page: *mut crate::mm::page_desc::Page) {
    // SAFETY: the caller null-checked the pfn_to_page_mut result; the
    // descriptor exists for this in-RAM page (INV-IOURING-REGION-1).
    unsafe { (*page).get_page(); }
}

// ------------------------------------------------------------------
// Pinned ring derefs
// ------------------------------------------------------------------

/// INV-IOURING-PIN-1: a ring pointer is dereferenced only while the
/// LIFECYCLE_LOCK is held (pin_ring, io_uring_close, the enter/register
/// fast paths) or while a reference taken under it is still held —
/// close() cannot clear+free the ring in between, so the pointer
/// resolves to a live instance for the borrow's duration.
///
/// Shared view of the pinned ring instance.
pub(super) fn ring_ref<'a>(ptr: *const IoUring) -> &'a IoUring {
    // SAFETY: INV-IOURING-PIN-1 above (lifecycle lock held).
    unsafe { &*ptr }
}

// ------------------------------------------------------------------
// File private-data cell
// ------------------------------------------------------------------

/// INV-IOURING-PRIVDATA-1: the file's private-data cell is written
/// once at io_uring_create install time (`set_private_data`, before
/// the fd is published) and taken once on the close path (the cell
/// discipline of INV-FS-FILEPRIV-1); reads run under LIFECYCLE_LOCK,
/// which serializes against the close-path take.
///
/// Read the installed ring pointer out of the private-data cell.
pub(super) fn privdata_read(file: &crate::fs::file::File) -> Option<*const IoUring> {
    // SAFETY: INV-IOURING-PRIVDATA-1 above (write-once cell, lock-held
    // read; the raw *mut u8 is cast back to the type the install site
    // stored).
    unsafe { (*file.private_data.get()).map(|p| p as *const IoUring) }
}

// ------------------------------------------------------------------
// mmap page install
// ------------------------------------------------------------------

/// INV-IOURING-MMAP-1: the loop installs user PTEs for a ring region
/// the mmap handler owns: the VMA was just added to THIS address
/// space (no concurrent installer can target our range), the pages
/// are the region's live pages, and each installed PTE takes a
/// mapping reference (review 2R.6) so the page cannot be freed while
/// mapped.
///
/// Map the region's pages into the user address space and take one
/// mapping reference per page.
pub(super) fn map_region_pages(
    user_ppn: u64,
    vaddr: usize,
    phys: usize,
    npages: usize,
    prot: u32,
) {
    use crate::arch::mm::{PageTableEntry, map_page};
    // SAFETY: INV-IOURING-MMAP-1 above (own-VMA install, live pages,
    // per-PTE mapping reference).
    unsafe {
        let mut pte_flags = PageTableEntry::V | PageTableEntry::U
            | PageTableEntry::A | PageTableEntry::D;
        if prot & 0x1 != 0 { pte_flags |= PageTableEntry::R; }
        if prot & 0x2 != 0 { pte_flags |= PageTableEntry::R | PageTableEntry::W; }

        for i in 0..npages {
            let va = vaddr + i * PAGE_SIZE;
            let pa = phys + i * PAGE_SIZE;
            map_page(user_ppn, VirtAddr::new(va as u64), PhysAddr::new(pa as u64), pte_flags);
            let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(pa));
            if !page.is_null() {
                (*page).get_page();
            }
        }
        crate::arch::mm::asid::flush_tlb_all();
    }
}

// ------------------------------------------------------------------
// Shared-ring window (SQE read / CQE write / head-tail counts)
// ------------------------------------------------------------------

const PAGE_SIZE: usize = 4096;

impl IoUring {
    /// Read one SQE by index (caller validated `idx < sq_entries`).
    pub(super) fn sqe_at(&self, idx: u32) -> IoUringSqe {
        let r = crate::safety::mmio::MmioRegion::new(self.sqes.kvirt as usize, self.sqes.size);
        // SAFETY: INV-IOURING-RING-1 + idx < sq_entries (checked by every
        // caller) keeps 0..64 inside the SQE-array window. IoUringSqe is
        // 64 bytes of POD (repr(Rust) plain u64/u32 fields, no padding
        // surprises — same layout liburing shares).
        unsafe { core::ptr::read_volatile((r.base() + idx as usize * 64) as *const IoUringSqe) }
    }

    /// Write one CQE at slot `slot` (caller masked the slot).
    pub(super) fn cqe_write(&self, slot: usize, cqe: IoUringCqe) {
        let r = self.cq_hdr();
        let off = self.cq_cqes_off + slot * 16;
        // SAFETY: INV-IOURING-RING-1; off+16 <= window by the setup-time
        // cq_entries sizing (the window check is the backstop).
        unsafe {
            core::ptr::write_volatile((r.base() + off) as *mut IoUringCqe, cqe);
        }
    }
}

/// Volatile (u32, u32) head/tail read from the CQ ring's shared
/// memory (INV-IOURING-RING-1: the offsets are inside the mapped
/// window by the setup-time sizing).
pub(super) fn cq_counts(kvirt: *mut u8, head_off: usize, tail_off: usize) -> (u32, u32) {
    // SAFETY: INV-IOURING-RING-1 — offsets inside the mapped window.
    unsafe {
        let head = core::ptr::read_volatile(kvirt.add(head_off) as *const u32);
        let tail = core::ptr::read_volatile(kvirt.add(tail_off) as *const u32);
        (head, tail)
    }
}
