//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 memory management operations — MmStruct extensions, user
//! mappings, COW.
//!
//! STAGE-1 STATE: the boot-path surface is real (MM_CTX, the PTE modify
//! lock, cow_flags encoding, alloc_user_phys_page); the user-space
//! operations (mmap/munmap/fork/COW walker) are the STAGE-2 port — they
//! carry the riscv64 twin's invariants (INV-COW-1..5) onto the aarch64
//! 4-level walker and are loud `todo!`s until then.

use core::sync::atomic::{fence, Ordering};

use super::memory_layout::*;
use super::pagetable::*;
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};
use crate::mm::pagemap::{MapError, Perm};
use crate::mm::{MmStruct};

// Re-export AddressSpace for backward compatibility
pub use crate::mm::AddressSpace;

// ==================== Page Table Walker ====================
//
/// Read-only 4-level walker (interface parity with the riscv64
/// `PageTableWalker`): resolves a VA to its leaf `(ppn, pte_bits)`.
///
/// `root_ppn` is the table the walk STARTS at: user VAs walk a TTBR0
/// root, kernel-half VAs walk the TTBR1 (kernel) root — the caller picks
/// the root by address-space ownership the same way the riscv64 twin's
/// callers hand in `mm.pgd`.
///
/// Index shifts descend from the root (L0 = VA[47:39] ... L3 =
/// VA[20:12]): shift for table level `cur` is PAGE_SHIFT + 9*(3-cur).
/// A BLOCK leaf at L0/L1/L2 is offset-adjusted and returned as the 4KB
/// PPN it contains (riscv64 huge-page parity); an L3 PAGE leaf returns
/// its own PPN.
pub struct PageTableWalker;

impl PageTableWalker {
    /// Walk `root_ppn` for `vaddr`; returns the leaf frame PPN (4KB
    /// granule, block leaves adjusted) and the raw descriptor bits.
    ///
    /// # Safety
    /// root_ppn must be a valid page-table root PPN; the walker only
    /// READS table frames (converted through get_page_table_virt).
    pub unsafe fn walk(root_ppn: u64, vaddr: u64) -> Option<(u64, u64)> {
        // SAFETY: get_page_table_virt translates allocated table frames;
        // the walk dereferences only frames reachable from the root.
        unsafe {
            let mut table_phys = root_ppn << PAGE_SHIFT;
            for cur in 0..4u32 {
                let shift = PAGE_SHIFT + 9 * ((3 - cur) as u64);
                let idx = ((vaddr >> shift) & 0x1FF) as usize;
                let table = super::mmu_init::get_page_table_virt(table_phys);
                let entry = (*table).get(idx);
                if !entry.is_valid() {
                    return None;
                }
                if entry.is_leaf() || cur == 3 {
                    // Block leaf (L0-L2, 1GB/2MB) or L3 page: fold the
                    // in-page offset so the PPN is 4KB-granular.
                    let page_base = entry.output_addr() + (vaddr & ((1 << shift) - 1));
                    return Some((page_base >> PAGE_SHIFT, entry.bits()));
                }
                table_phys = entry.ppn() << PAGE_SHIFT;
            }
            None
        }
    }

    /// Walk and return the level the leaf was found at (0..=3)
    ///
    /// # Safety
    /// See walk().
    pub unsafe fn walk_level(root_ppn: u64, vaddr: u64) -> Option<(u64, u64, u8)> {
        // SAFETY: see walk().
        unsafe {
            let mut table_phys = root_ppn << PAGE_SHIFT;
            for cur in 0..4u32 {
                let shift = PAGE_SHIFT + 9 * ((3 - cur) as u64);
                let idx = ((vaddr >> shift) & 0x1FF) as usize;
                let table = super::mmu_init::get_page_table_virt(table_phys);
                let entry = (*table).get(idx);
                if !entry.is_valid() {
                    return None;
                }
                if entry.is_leaf() || cur == 3 {
                    let page_base = entry.output_addr() + (vaddr & ((1 << shift) - 1));
                    return Some((page_base >> PAGE_SHIFT, entry.bits(), cur as u8));
                }
                table_phys = entry.ppn() << PAGE_SHIFT;
            }
            None
        }
    }
}

// ==================== MmStruct Extension Methods ====================

impl MmStruct {
    /// Enable this address space (switch TTBR0 + ASID)
    ///
    /// # Safety
    /// pgd must be a valid user root PPN.
    pub unsafe fn enable(&self) {
        let satp = Satp::long(self.pgd, 0);
        super::asid::write_satp(satp.0 as usize);
    }

    /// Disable the address space (switch to the kernel root)
    ///
    /// # Safety
    /// Switches TTBR0 to the kernel root with a full flush.
    pub unsafe fn disable() {
        let kernel_ppn = super::mmu_init::root_page_table_ppn();
        super::asid::write_satp(super::asid::build_satp(0, kernel_ppn as usize));
    }

    /// Flush the entire TLB
    ///
    /// # Safety
    /// Callers must have repaired the affected translations.
    pub unsafe fn flush_tlb() {
        super::asid::flush_tlb_all();
    }

    /// Flush the TLB for one page
    ///
    /// # Safety
    /// See flush_tlb.
    pub unsafe fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        super::asid::flush_tlb_page(vaddr.as_usize(), 0);
    }

    // ==================== VMA Operations (stage 2) ====================

    /// Map VMA (riscv64 twin: lazy anonymous demand fill)
    pub fn mmap(
        &self,
        addr: PageVirtAddr,
        size: usize,
        flags: crate::mm::vma::VmaFlags,
        vma_type: crate::mm::vma::VmaType,
        perm: Perm,
        map_flags: u32,
    ) -> Result<PageVirtAddr, MapError> {
        let _ = (addr, size, flags, vma_type, perm, map_flags);
        todo!("aarch64 stage-2: MmStruct::mmap")
    }

    pub fn map_vma(
        &self,
        vma: crate::mm::vma::Vma,
        perm: Perm,
    ) -> Result<(), MapError> {
        let _ = (vma, perm);
        todo!("aarch64 stage-2: MmStruct::map_vma")
    }

    pub fn map_single_page(
        &self,
        virt_addr: VirtAddr,
        perm: Perm,
    ) -> Result<(), MapError> {
        let _ = (virt_addr, perm);
        todo!("aarch64 stage-2: MmStruct::map_single_page")
    }

    pub fn unmap_vma(&self, start: PageVirtAddr) -> Result<(), MapError> {
        let _ = start;
        todo!("aarch64 stage-2: MmStruct::unmap_vma")
    }

    pub fn set_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        let _ = new_brk;
        todo!("aarch64 stage-2: MmStruct::set_brk")
    }

    pub fn find_free_area(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let _ = size;
        todo!("aarch64 stage-2: MmStruct::find_free_area")
    }

    pub fn munmap(&self, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let _ = (addr, size);
        todo!("aarch64 stage-2: MmStruct::munmap")
    }

    pub fn zap_page_range(&self, start: PageVirtAddr, size: usize) -> Result<(), MapError> {
        let _ = (start, size);
        todo!("aarch64 stage-2: MmStruct::zap_page_range")
    }

    pub fn do_brk(&self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        let _ = new_brk;
        todo!("aarch64 stage-2: MmStruct::do_brk")
    }

    pub fn allocate_stack(&self, size: usize) -> Result<PageVirtAddr, MapError> {
        let _ = size;
        todo!("aarch64 stage-2: MmStruct::allocate_stack")
    }

    pub fn fork(&self) -> Result<MmStruct, MapError> {
        todo!("aarch64 stage-2: MmStruct::fork (COW page-table copy)")
    }

    /// Unmap a single user leaf under the PTE lock (tlb-flush batching
    /// family; riscv64 twin semantics)
    ///
    /// # Safety
    /// PTE_MODIFY_LOCK must be held.
    pub unsafe fn unmap_one_user_leaf_locked(&self, va: usize) -> bool {
        let _ = va;
        todo!("aarch64 stage-2: MmStruct::unmap_one_user_leaf_locked")
    }
}

// ==================== User page mapping (stage 2) ====================

/// Map a single user page into a user root
///
/// # Safety
/// user_root_ppn must be a valid user root; caller holds the PTE lock.
pub unsafe fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) -> bool {
    let _ = (user_root_ppn, user_virt, phys, flags);
    todo!("aarch64 stage-2: map_user_page (4-level user walker)")
}

/// Map a user region
///
/// # Safety
/// See map_user_page.
pub unsafe fn map_user_region(
    root_ppn: u64,
    start_virt: u64,
    size: u64,
    phys_start: u64,
    flags: u64,
) -> bool {
    let _ = (root_ppn, start_virt, size, phys_start, flags);
    todo!("aarch64 stage-2: map_user_region")
}

// ==================== Allocation / registry ====================

/// Allocate a physical page for user memory (riscv64 twin semantics)
pub fn alloc_user_phys_page() -> Option<u64> {
    use crate::mm::GfpFlags;
    let phys = crate::mm::alloc_pages(GfpFlags::GFP_USER, 0);
    if phys != 0 {
        Some(phys as u64)
    } else {
        None
    }
}

/// Create a fresh user address space (empty TTBR0 root)
pub fn create_user_address_space() -> Option<u64> {
    todo!("aarch64 stage-2: create_user_address_space")
}

// ==================== Global mm context + PTE lock ====================

/// Global mm context id (dfx parity)
pub static MM_CTX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Global PTE modify lock (riscv64 twin: serializes fork walks against
/// COW faults and mprotect/munmap table surgery)
pub static PTE_MODIFY_LOCK: crate::mm::mm_struct::GlobalPteLock =
    crate::mm::mm_struct::GlobalPteLock::new();

/// Per-mm leaf-PTE section hook (INV-LOCK-7), GLOBAL mode like the
/// riscv64 twin: every mm maps to the same global epoch section
#[inline]
pub fn pte_lock_for(_mm: &crate::mm::mm_struct::MmStruct) -> &'static crate::mm::mm_struct::PteLock {
    &PTE_GLOBAL_HANDLE
}

static PTE_GLOBAL_HANDLE: crate::mm::mm_struct::PteLock =
    crate::mm::mm_struct::PteLock::new();

// ==================== COW fault + alloc-map helpers (stage 2) ====================

/// Resolve a COW fault (INV-COW-3/4 semantics; riscv64 twin contract)
///
/// # Safety
/// PTE_MODIFY_LOCK must be held by the caller.
pub unsafe fn handle_cow_fault(
    _mm: &crate::mm::mm_struct::MmStruct,
    _root_ppn: u64,
    _fault_addr: VirtAddr,
) -> CowFaultResult {
    todo!("aarch64 stage-2: handle_cow_fault")
}

/// Allocate and map anonymous user memory (riscv64 twin)
///
/// # Safety
/// user_root_ppn must be a valid user root.
pub unsafe fn alloc_and_map_user_memory(
    _user_root_ppn: u64,
    _virt_addr: u64,
    _size: u64,
    _flags: u64,
) -> Option<u64> {
    todo!("aarch64 stage-2: alloc_and_map_user_memory")
}

/// Allocate and wire a user page-table region (riscv64 twin)
///
/// # Safety
/// See alloc_and_map_user_memory.
pub unsafe fn alloc_and_map_to_user_table(
    _user_root_ppn: u64,
    _virt_addr: u64,
    _size: u64,
    _flags: u64,
) -> Option<u64> {
    todo!("aarch64 stage-2: alloc_and_map_to_user_table")
}

// ==================== COW flags ====================
//
/// COW software bits — descriptor upper-half software space (see
/// pagetable.rs for the bit budget; these never reach hardware)
pub mod cow_flags {
    /// COW marker: mapping is shared read-only until the fault copies it
    pub const COW: u64 = 1 << 61;
    /// Page-cache-backed mapping (filemap fault family)
    pub const PCACHE: u64 = 1 << 62;

    /// Whether these bits describe a shared page-cache mapping
    pub fn pte_is_shared_cache(bits: u64) -> bool {
        bits & PCACHE != 0
    }
}

/// COW fault result (riscv64 twin)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowFaultResult {
    /// COW resolved: PTE is writable again (exclusive fast path, or a
    /// private copy was installed and mapped)
    Resolved,
    /// The PTE changed under us — re-execute the faulting instruction
    /// (riscv64 twin: do_wp_page-style retry, never a kill)
    Retry,
    /// Allocating the private copy failed — genuine out-of-memory
    OutOfMemory,
}
