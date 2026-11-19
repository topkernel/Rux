//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! riscv64 mm framework edge module (layer-1 doctrine, unsafe-slim round 5
//! lane L3): EVERY hand-written page-table/TLB edge of this arch's mm tree
//! lives here. The other files under `arch/riscv64/mm/` are unsafe-free
//! services built on these primitives.
//!
//! The reference kernel (Linux RISC-V port) writes the same operations by
//! hand in C (arch/riscv/mm/pgtable*.c, asm/pgtable-64.h) with zero type
//! protection; the difference here is that each edge below is a named,
//! audited INV contract and everything above it is compiler-checked.
//!
//! # Safety invariants — the page-table handle (INV-RE-PT-*)
//!
//! - **INV-RE-PT-1 (liveness)** — [`PtTable`] is only constructed from a
//!   physical address that passed the stale-frame guard
//!   ([`PtTable::from_phys`]: `phys != 0` and, once page descriptors are
//!   published, `phys_valid`). A guarded frame is linear-mapped managed
//!   RAM, so every later `get`/`set` through the handle is a mapped
//!   access and cannot fault. Whether the frame is semantically a
//!   page-table is the caller's structural argument — exactly the
//!   argument every raw walk in this tree already made (a valid upper
//!   entry's target, the mm root, or a freshly allocated table).
//! - **INV-RE-PT-2 (bounds)** — accessors take any `usize` index and
//!   mask it into `0..512` (the same masking `PageTable::get/set`
//!   applied); no out-of-bounds place projection exists in this type.
//! - **INV-RE-PT-3 (aliasing)** — tables are shared across harts; `get`
//!   and `set` go through raw place projections
//!   (`addr_of!`/`addr_of_mut!`), never through `&mut` references, so no
//!   aliasing `&mut` is ever formed and two handles to one table are
//!   sound. Stores are plain (non-atomic, non-volatile) u64 writes — the
//!   same publication discipline the raw walks used (Sv39 PTE
//!   publication fences are the caller's sfence.vma, as before).
//! - **INV-RE-PT-4 (stage discipline)** — construction routes through
//!   `get_page_table_virt`, which selects the early-pool VMA vs the
//!   linear map per the boot alloc stage; callers never re-derive the
//!   conversion.

use super::pagetable::{PageTable, PageTableEntry};

/// Typed handle to one page-table page (512 entries) — the single
/// audited phys→table edge of the riscv64 mm tree (INV-RE-PT-*).
#[derive(Clone, Copy)]
pub struct PtTable {
    ptr: *mut PageTable,
}

impl PtTable {
    /// Handle for the page-table page at physical address `phys`,
    /// guarded against stale/garbage frames (INV-RE-PT-1/4).
    ///
    /// `None` when `phys` is zero or fails `phys_valid` after page
    /// descriptors are published — the same root/intermediate guard the
    /// raw walks applied inline. Before descriptors are published
    /// (early/fixmap boot stages) the boot tables are trusted, as before.
    #[inline]
    pub fn from_phys(phys: u64) -> Option<PtTable> {
        if phys == 0 {
            return None;
        }
        if crate::mm::page_desc::descs_published()
            && !crate::mm::phys_valid(phys as usize)
        {
            return None;
        }
        // SAFETY: INV-RE-PT-4 — the one audited conversion (stage-aware,
        // via get_page_table_virt); the frame is linear-mapped by the
        // guard above (INV-RE-PT-1). Forming the raw pointer is inert;
        // every dereference goes through the bounds-masked accessors.
        Some(PtTable {
            ptr: unsafe { super::mmu_init::get_page_table_virt(phys) },
        })
    }

    /// Handle for a frame the caller knows is a LIVE table by structure
    /// (freshly allocated, or the Arc-pinned/PTE-locked walk targets of
    /// the fork/COW paths, whose exclusion argument the callers own).
    #[inline]
    pub fn from_phys_live(phys: u64) -> Option<PtTable> {
        Self::from_phys(phys)
    }

    #[inline]
    pub fn as_phys_base(&self) -> u64 {
        self.ptr as u64
    }

    /// Raw bits of entry `idx` (INV-RE-PT-2: masked into 0..512).
    #[inline]
    pub fn get_raw(&self, idx: usize) -> u64 {
        // SAFETY: INV-RE-PT-1 (mapped frame) + INV-RE-PT-2 (masked index)
        // + INV-RE-PT-3 (raw place projection, no reference formed).
        unsafe { core::ptr::addr_of!((*self.ptr).entries[idx & 0x1FF]).read() }.bits()
    }

    /// Typed entry read (see [`Self::get_raw`]).
    #[inline]
    pub fn get(&self, idx: usize) -> PageTableEntry {
        PageTableEntry::from_bits(self.get_raw(idx))
    }

    /// Write entry `idx` (INV-RE-PT-2/3).
    #[inline]
    pub fn set(&self, idx: usize, entry: PageTableEntry) {
        // SAFETY: INV-RE-PT-1 (mapped frame) + INV-RE-PT-2 (masked index)
        // + INV-RE-PT-3 (raw place projection, plain store — the
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

impl PtTable {
    /// Handle to the static kernel-root table (`ROOT_PAGE_TABLE`, image
    /// BSS — INV-RE-PT-1: inside the kernel image, mapped and live for
    /// the whole machine lifetime; the one table that needs no guard).
    #[inline]
    pub fn static_root() -> PtTable {
        // SAFETY: the static root lives inside the kernel image (BSS);
        // the va→pa arithmetic below mirrors stamp_kernel_tree_boot.
        let root_virt = &raw mut super::mmu_init::ROOT_PAGE_TABLE as u64;
        // SAFETY: set once at boot before any concurrent reader exists.
        let offset = unsafe { super::memory_layout::KERNEL_MAP.va_kernel_pa_offset } as u64;
        let root_phys = root_virt.wrapping_sub(offset);
        // SAFETY: image memory — the stage-aware conversion is inert.
        PtTable {
            ptr: unsafe { super::mmu_init::get_page_table_virt(root_phys) },
        }
    }
}

/// Copy one page between two linear-map addresses (INV-RE-COPY-1): both
/// `src_phys` and `dst_phys` are frames whose liveness the caller's path
/// holds (the COW copy runs under the PTE section with the old page
/// pinned by refcount ≥ 2 and the new page exclusively owned).
#[inline]
pub fn copy_page(src_phys: u64, dst_phys: u64) {
    use super::memory_layout::{phys_to_virt, PhysAddr, PAGE_SIZE};
    let src = phys_to_virt(PhysAddr::new(src_phys)).bits() as *const u8;
    let dst = phys_to_virt(PhysAddr::new(dst_phys)).bits() as *mut u8;
    // SAFETY: INV-RE-COPY-1 — both frames are live, page-sized, and
    // non-overlapping (distinct frames); exactly PAGE_SIZE bytes each.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE as usize) };
}
