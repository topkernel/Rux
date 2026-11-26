//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! aarch64 mm framework edge module (layer-1 doctrine, unsafe-slim round 5
//! lane L3): EVERY hand-written page-table/TLB edge of this arch's mm tree
//! lives here. The other files under `arch/aarch64/mm/` are unsafe-free
//! services built on these primitives.
//!
//! The reference kernel (Linux ARM64) writes the same operations by hand
//! in C (arch/arm64/mm/pgtable*.c, asm/pgtable-hwdef.h) with zero type
//! protection; the difference here is that each edge below is a named,
//! audited INV contract and everything above it is compiler-checked.
//!
//! # Safety invariants — the page-table handle (INV-AE-PT-*)
//!
//! - **INV-AE-PT-1 (liveness)** — [`PtTable`] is only constructed from a
//!   physical address that passed the stale-frame guard
//!   ([`PtTable::from_phys`]: `phys != 0` and, once page descriptors are
//!   published, `phys_valid`). A guarded frame is linear-mapped managed
//!   RAM, so every later `get`/`set` through the handle is a mapped
//!   access and cannot fault. Whether the frame is semantically a
//!   translation table is the caller's structural argument — exactly the
//!   argument every raw walk in this tree already made (a valid upper
//!   entry's target, the mm root, or a freshly allocated table).
//! - **INV-AE-PT-2 (bounds)** — accessors take any `usize` index and
//!   mask it into `0..512` (the same masking `PageTable::get/set`
//!   applied); no out-of-bounds place projection exists in this type.
//! - **INV-AE-PT-3 (aliasing)** — tables are shared across CPUs; `get`
//!   and `set` go through raw place projections
//!   (`addr_of!`/`addr_of_mut!`), never through `&mut` references, so no
//!   aliasing `&mut` is ever formed and two handles to one table are
//!   sound. Stores are plain (non-atomic, non-volatile) u64 writes — the
//!   same publication discipline the raw walks used (descriptor
//!   publication fences are the caller's TLBI, as before).
//! - **INV-AE-PT-4 (stage discipline)** — construction routes through
//!   `get_page_table_virt`, which selects the image-offset vs the linear
//!   map per the boot alloc stage; callers never re-derive the
//!   conversion.

use super::pagetable::{PageTable, PageTableEntry};

pub unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    match super::mmu_init::get_alloc_stage() {
        super::mmu_init::AllocStage::Early => {
            // Static pools live in the image; convert via the image offset
            let offset = unsafe { super::memory_layout::KERNEL_MAP.va_kernel_pa_offset } as u64;
            (phys_addr.wrapping_add(offset)) as *mut PageTable
        }
        super::mmu_init::AllocStage::Fixmap | super::mmu_init::AllocStage::Late => {
            let virt_addr = super::memory_layout::phys_to_virt(super::memory_layout::PhysAddr::new(phys_addr));
            virt_addr.bits() as *mut PageTable
        }
    }
}


/// Typed handle to one translation-table page (512 entries) — the single
/// audited phys→table edge of the aarch64 mm tree (INV-AE-PT-*).
#[derive(Clone, Copy)]
pub struct PtTable {
    ptr: *mut PageTable,
}

impl PtTable {
    /// Handle for the table page at physical address `phys`, guarded
    /// against stale/garbage frames (INV-AE-PT-1/4).
    #[inline]
    pub fn from_phys(phys: u64) -> Option<PtTable> {
        if phys == 0 {
            return None;
        }
        if crate::mm::hotpath::descs_published()
            && !crate::mm::hotpath::phys_valid(phys as usize)
        {
            return None;
        }
        // SAFETY: INV-AE-PT-4 — the one audited conversion (stage-aware,
        // via get_page_table_virt); the frame is linear-mapped by the
        // guard above (INV-AE-PT-1). Forming the raw pointer is inert;
        // every dereference goes through the bounds-masked accessors.
        Some(PtTable {
            ptr: unsafe { super::mmu_init::get_page_table_virt(phys) },
        })
    }

    /// Handle for a frame the caller knows is a LIVE table by structure
    /// (freshly allocated, or the PTE-locked walk targets of the
    /// fork/COW paths, whose exclusion argument the callers own).
    #[inline]
    pub fn from_phys_live(phys: u64) -> Option<PtTable> {
        Self::from_phys(phys)
    }

    #[inline]
    pub fn as_phys_base(&self) -> u64 {
        self.ptr as u64
    }

    /// Raw bits of entry `idx` (INV-AE-PT-2: masked into 0..512).
    #[inline]
    pub fn get_raw(&self, idx: usize) -> u64 {
        // SAFETY: INV-AE-PT-1 (mapped frame) + INV-AE-PT-2 (masked index)
        // + INV-AE-PT-3 (raw place projection, no reference formed).
        unsafe { core::ptr::addr_of!((*self.ptr).entries[idx & 0x1FF]).read() }.bits()
    }

    /// Typed entry read (see [`Self::get_raw`]).
    #[inline]
    pub fn get(&self, idx: usize) -> PageTableEntry {
        PageTableEntry::from_bits(self.get_raw(idx))
    }

    /// Write entry `idx` (INV-AE-PT-2/3).
    #[inline]
    pub fn set(&self, idx: usize, entry: PageTableEntry) {
        // SAFETY: INV-AE-PT-1 (mapped frame) + INV-AE-PT-2 (masked index)
        // + INV-AE-PT-3 (raw place projection, plain store — the
        // publication discipline the raw walks used).
        unsafe {
            core::ptr::addr_of_mut!((*self.ptr).entries[idx & 0x1FF])
                .write(entry)
        };
    }

    /// True when every entry is raw-zero (the emptied-table scans).
    pub fn all_zero(&self) -> bool {
        for i in 0..512 {
            if self.get_raw(i) != 0 {
                return false;
            }
        }
        true
    }

    /// Clear every entry (fresh-table discipline; the caller owns the
    /// frame exclusively).
    pub fn zero_entries(&self) {
        for i in 0..512 {
            self.set(i, PageTableEntry::new());
        }
    }
}

/// Copy one page between two linear-map addresses (INV-AE-COPY-1): both
/// `src_phys` and `dst_phys` are frames whose liveness the caller's path
/// holds (the COW copy runs under the PTE section with the old page
/// pinned by refcount ≥ 2 and the new page exclusively owned).
#[inline]
pub fn copy_page(src_phys: u64, dst_phys: u64) {
    use super::memory_layout::{phys_to_virt, PhysAddr, PAGE_SIZE};
    let src = phys_to_virt(PhysAddr::new(src_phys)).bits() as *const u8;
    let dst = phys_to_virt(PhysAddr::new(dst_phys)).bits() as *mut u8;
    // SAFETY: INV-AE-COPY-1 — both frames are live, page-sized, and
    // non-overlapping (distinct frames); exactly PAGE_SIZE bytes each.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE as usize) };
}
