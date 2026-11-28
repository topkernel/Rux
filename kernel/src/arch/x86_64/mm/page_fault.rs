//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 page-fault outcome types + handle_mm_fault entry.
//!
//! Ported from arch/riscv64/mm/page_fault.rs: demand paging (anon
//! zero-fill, file-backed reads through the pinned vm_file, swap-in),
//! COW dispatch, stack growth, permission enforcement — all driven
//! through the generic VMA layer; only PTE flag construction and the
//! walk depth are x86-specific.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use super::memory_layout::*;
use super::unsafe_edges::PtTable;
use super::mm_ops::{alloc_user_phys_page, check_pte_permissions, is_cow_page, PageTableWalker};
use super::mmu_init::{get_page_table_virt, map_page, map_page_noflush, ROOT_PAGE_TABLE};
use super::pagetable::*;
use crate::mm::page::PAGE_SIZE as PAGE_SIZE_USIZE;
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Fault access flags (u32 bitset, matching generic callers)
pub struct FaultFlags;
impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 1 << 0;
    /// Write fault
    pub const WRITE: u32 = 1 << 1;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 1 << 2;
    /// User mode access
    pub const USER: u32 = 1 << 3;
    /// Kernel mode access
    pub const KERNEL: u32 = 1 << 4;
}

// ==================== Fault Result ====================

/// Outcome of a handled fault
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Fault fully handled — retry the instruction
    Handled,
    /// Anonymous page materialized — retry
    Fixed,
    /// COW resolved — retry
    CowPending,
    /// Mapping already exists — retry
    AlreadyMapped,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
    /// Kernel exception fixed (via exception table)
    KernelPanic,
}

// ==================== Stack Expansion ====================
/// Try to expand stack when page fault occurs below current stack bottom
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-16): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn try_expand_stack( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, root_ppn: u64, ) -> MmFaultResult {
    crate::arch::arch_floor::x86_64::mm::page_fault::try_expand_stack(addr_space, fault_addr, flags, root_ppn)
}

// ==================== Fault-Around ====================

/// Fault-around window in pages for read/exec demand faults on file
/// VMAs — Linux's fault_around_bytes default shape (64 KiB / 4 KiB = 16).
/// One fault fills and maps up to this many pages starting at the faulting
/// page (session 14, the mm-side half of the 16x lib-load win; the fs-side
/// readahead detector sees the sequential read_at stream and prefetches
/// the window through the batch block layer).
///
/// Around pages are held to SINGLE-page semantics, only amortized: each
/// page is an exclusive private copy filled from the pinned vm_file at
/// its own file offset, mapped with the VMA's flags, past-EOF pages stay
/// unmapped (a later touch re-faults into the SIGBUS gate), and write
/// faults opt out (Linux do_fault_around parity).
pub(crate) const FAULT_AROUND_PAGES: usize = 16;

/// SHARED read fault-around window (perf-gap-hunt 2026-11-04): pages
/// prefetched per SHARED read fault, clamped by VMA end and EOF, and
/// bounded to the CONTIGUOUS RESIDENT run (holes stop the batch — a cold
/// or sparse mapping still faults page-by-page through the populate arm).
/// 512 pages = 2 MiB per fault; sequential mappers re-fault once per 2 MiB,
/// which is where the lat_pagefault amortization sits after the slot-batch
/// walker removed the per-page walk cost. The window is BIDIRECTIONAL
/// (2026-11-05): the same cap applies below the faulting page, so a random
/// touch order consumes the whole unmapped run containing the fault in one
/// trap instead of leaving the lower half to trap per page.
pub(crate) const SHARED_FAULT_AROUND_PAGES: usize = 512;
/// Pin-run chunk for the SHARED fault-around (bounds the stack array).
pub(crate) const SHARED_PIN_CHUNK: usize = 64;

/// sc181: forward fault-around window (pages) for anonymous WRITE faults
/// on large VMAs — the hilogd-ring family (see the anon batch in
/// handle_mm_fault).
pub(crate) const ANON_FAULTAROUND_PAGES: usize = 32;

/// sc181: minimum VMA size that gets the anonymous fault-around (small
/// stacks/heaps/brk keep exact one-page semantics).
pub(crate) const ANON_FAULTAROUND_MIN_VMA: usize = 1024 * 1024;
/// Fill one freshly allocated (exclusively owned) user page from `file`
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-15): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn fill_file_page( file: &crate::fs::file::File, file_offset: usize, page_ptr: *mut u8, effective_size: usize, ) {
    crate::arch::arch_floor::x86_64::mm::page_fault::fill_file_page(file, file_offset, page_ptr, effective_size)
}

// ==================== filemap_fault (page-cache mapping) ====================
/// Pin the page-cache frame for one page-shaped file offset, populating
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-14): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn filemap_pin_cache_page( file: &crate::fs::file::File, file_offset: usize, effective_size: usize, hole_as_zero: bool, ) -> Option<u64> {
    crate::arch::arch_floor::x86_64::mm::page_fault::filemap_pin_cache_page(file, file_offset, effective_size, hole_as_zero)
}

/// Bookkeeping for a successfully mapped page-cache frame: one descriptor
/// reference (dropped by unmap/COW's put_page) and mapcount visibility.
/// Deliberately NO set_index / page_record_mapping / LRU add — the
/// descriptor's mapping/index are the page cache's eviction keys and the
/// file-LRU membership belongs to the cache (the fake-OOM-era clobber of
/// either would corrupt eviction: wrong-key lookup or an unlisted frame).
///
/// # SAFETY
/// `phys` is a page-cache frame pinned for THIS mapping (pin taken by
/// `filemap_pin_cache_page`), being mapped right now under the PTE lock.
pub(crate) fn filemap_bookkeep_mapped_cache_page(phys: u64) {
    use crate::mm::page_desc::page_desc;
    // Descriptor of a live, pinned cache frame.
    if let Some(page) = page_desc((phys >> PAGE_SHIFT) as usize) {
        page.get_page();
        page.inc_mapcount();
    }
}

/// Release a fill-phase page that lost the map race (or whose map failed):
/// a pinned cache frame returns its pin; a private fill copy is freed.
pub(crate) fn release_fill_page(phys: u64, from_cache: bool) {
    if from_cache {
        super::mm_ops::release_cache_mapping_pin((phys >> PAGE_SHIFT) as usize);
    } else {
        crate::mm::page_alloc::free_page(phys as usize);
    }
}

// ==================== Main Fault Handler ====================

/// E8-MM port: does a leaf PTE (raw bits) at `va` count as "mapped" for a
/// fault on this address space? A valid KERNEL (U=0) PTE inside a
/// low-half identity device window is the RESTORE fill clear_pte leaves
/// behind after a user mapping over the window was unmapped (leaf
/// demotion made those mmaps possible — MAP_FIXED replace semantics). A
/// user fault there is a demand fault on the user mapping, not a
/// protection violation: treat the device PTE as a hole and let the VMA
/// layer decide (a stray access with no covering VMA still SIGSEGVs, just
/// via the correct path).
#[inline]
pub(crate) fn pte_counts_as_mapped(bits: u64, va: u64) -> bool {
    !(bits & PageTableEntry::U == 0
        && crate::arch::mm::mmu_init::kernel_device_window_pte(va).is_some())
}

/// handle_mm_fault - Handle user mode page fault
///
/// # Arguments
/// - `addr_space`: Address space
/// - `fault_addr`: Virtual address that triggered fault
/// - `flags`: Fault type flags (FaultFlags)
/// L7 defect-A instrument: WHICH early-refusal arm returned Segfault
/// (set at each site; printed once per boot by the trap Segfault arm).
/// 1=stack_limit 2=no_stack_vma 3=not_below_start 4=below_stack_limit
/// 5=expand_failed 6=perm(is_read&&!readable via Segfault) 9=entry_perm
pub(crate) static FILL_REFUSAL_SITE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-13): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_mm_fault( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, ) -> MmFaultResult {
    crate::arch::arch_floor::x86_64::mm::page_fault::handle_mm_fault(addr_space, fault_addr, flags)
}

// ==================== Utility ====================

/// Physical address behind a user virtual address, if mapped.
///
/// Walks the page table to find the PTE for the given virtual address
/// and returns the physical page address (page-aligned).
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    // SAFETY: caller guarantees root_ppn is a live user root.
     PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << 12) 
}

// ==================== Swap-In Support ====================

/// Read the raw PTE value at a virtual address.
///
/// Walks the four-level page table and returns the raw bits of the leaf
/// PTE, even if P=0 (e.g. a swap entry). Returns None if the walk cannot
/// reach the leaf level.
/// R-WRITESTORM-2: is this leaf a PROT_NONE marker (P=0, U=1, non-swap,
/// non-migration, with a preserved PPN) as written by mprotect_pte_range
/// for prot==0? Such a leaf is NOT a mapping decision — the covering VMA
/// owns protection; the fault layer re-presents it when the VMA grants
/// the access.
pub(crate) fn is_protnone_marker(raw: u64) -> bool {
    (raw & PageTableEntry::P) == 0
        && (raw & PageTableEntry::U) != 0
        && !crate::mm::swap::is_swap_entry(raw)
        && !crate::mm::swap::is_migration_entry(raw)
        && (raw & PageTableEntry::PHYS_MASK_PUBLIC) != 0
}

/// R-WRITESTORM-2: write one 4K leaf PTE (raw bits), following the same
/// 4-level walk as read_pte_raw. Caller must hold PTE_MODIFY_LOCK and
/// only call it for addresses whose walk read_pte_raw already resolved
/// (the upper levels exist).
pub(crate) fn write_pte_raw(root_ppn: u64, vaddr: VirtAddr, bits: u64) {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;
    // Same walk as read_pte_raw (typed); the leaf exists (caller read it
    // via read_pte_raw on the same root/addr), so the intermediate
    // tables are present. Only the leaf is replaced.
    {
        let Some(root_table) = PtTable::from_phys(root_ppn << PAGE_SHIFT) else { return };
        let pte4 = root_table.get(vpn4);
        if !pte4.is_valid() {
            return;
        }
        let Some(table3) = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT) else { return };
        let pte3 = table3.get(vpn3);
        if !pte3.is_valid() || pte3.is_leaf() {
            return;
        }
        let Some(table2) = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT) else { return };
        let pte2 = table2.get(vpn2);
        if !pte2.is_valid() || pte2.is_leaf() {
            return;
        }
        let Some(table1) = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT) else { return };
        // 0xb9 install validator (oh-launch round 7): a PRESENT entry
        // with zero address bits is corrupt by construction (physical
        // page 0 is reserved at boot on x86). The hilogd teardown
        // fault walked one; this arm names the INSTALLER the moment a
        // bad value is written, live. Gated by rux:mmforensics;
        // deferred printk (lock-safe); one load when off.
        if crate::trace::EVENT_MMFORENSICS.armed()
            && (bits & 0x1) != 0
            && (bits & 0x000F_FFFF_FFFF_F000) == 0
        {
            static BADPT_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if BADPT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16 {
                crate::pr_err!(
                    "PT-BAD-INSTALL: write_pte_raw bits={:#x} vaddr={:#x} root={:#x} pid={} ra={:#x}",
                    bits,
                    a.bits(),
                    root_ppn,
                    crate::sched::get_current_pid(),
                    crate::debug::backtrace::caller_return_address()
                );
            }
        }
        table1.set(vpn1, super::pagetable::PageTableEntry::from_bits(bits));
    }
}

/// Slot-batch helper (fault-around map phase, perf-gap-hunt): walk the
/// upper three levels ONLY and return the level-1 (PT) table pointer
/// covering `vaddr`'s 2 MB region. Returns None when an intermediate
/// level is missing (caller falls back to `map_page_noflush`, which
/// allocates) or is a huge leaf needing demotion (same fallback). The
/// leaf slot itself is the CALLER's to read/write — this is the one-walk
/// primitive that keeps a 512-page batch at one leaf load + one leaf
/// store per page.
///
/// # SAFETY
/// `root_ppn` is a valid root page-table PPN for the current address
/// space. Read-only walk of paging structures.
/// Typed leaf-table handle for the fault paths (the stale-frame guard is
/// baked into construction).
pub(crate) fn l1_table_for(root_ppn: u64, vaddr: VirtAddr) -> Option<PtTable> {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
    let pte4 = root_table.get(vpn4);
    if !pte4.is_valid() {
        return None;
    }
    let table3 = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT)?;
    let pte3 = table3.get(vpn3);
    if !pte3.is_valid() || pte3.is_leaf() {
        return None;
    }
    let table2 = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT)?;
    let pte2 = table2.get(vpn2);
    if !pte2.is_valid() || pte2.is_leaf() {
        return None;
    }
    PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)
}

pub(crate) fn read_pte_raw(root_ppn: u64, vaddr: VirtAddr) -> Option<u64> {
    let a = VirtAddr::new(vaddr.bits());
    let vpn4 = a.pgd_index() as usize;
    let vpn3 = a.pud_index() as usize;
    let vpn2 = a.pmd_index() as usize;
    let vpn1 = a.pte_index() as usize;

    // Typed walk; only reads PTEs, including the leaf even if P=0.
    {
        let root_table = PtTable::from_phys(root_ppn << PAGE_SHIFT)?;
        let pte4 = root_table.get(vpn4);
        if !pte4.is_valid() {
            return None;
        }

        let table3 = PtTable::from_phys(pte4.ppn() << PAGE_SHIFT)?;
        let pte3 = table3.get(vpn3);
        if !pte3.is_valid() {
            return None;
        }
        // Superpage leaves (identity/device windows): return the leaf
        // itself — never a swap/migration entry, and its PPN must not be
        // followed as a table pointer.
        if pte3.is_leaf() {
            return Some(pte3.bits());
        }

        let table2 = PtTable::from_phys(pte3.ppn() << PAGE_SHIFT)?;
        let pte2 = table2.get(vpn2);
        if !pte2.is_valid() {
            return None;
        }
        if pte2.is_leaf() {
            return Some(pte2.bits());
        }

        let table1 = PtTable::from_phys(pte2.ppn() << PAGE_SHIFT)?;
        let pte1 = table1.get(vpn1);

        let raw = pte1.bits();
        if raw == 0 {
            return None;
        }

        Some(raw)
    }
}
/// Handle a swap fault — read the page back from swap and map it.
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn handle_swap_fault( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, swap_entry: u64, root_ppn: u64, ) -> MmFaultResult {


                                                                                                                                      use crate::mm::page::VirtAddr as PageVirtAddr;
                                                                                                                                      use crate::mm::page_desc::{page_desc, PageFlag};
                                                                                                                                      use crate::mm::swap;

                                                                                                                                      // Extract swap type and offset
                                                                                                                                      let swap_type = swap::swap_entry_type(swap_entry);
                                                                                                                                      let swap_offset = swap::swap_entry_offset(swap_entry);

                                                                                                                                      // Find VMA for permission bits
                                                                                                                                      let page_virt_addr = PageVirtAddr::new(fault_addr.as_usize());
                                                                                                                                      let vma_mgr = addr_space.vma_read();
                                                                                                                                      let vma = match vma_mgr.find(page_virt_addr) {
                                                                                                                                          Some(v) => v,
                                                                                                                                          None => return MmFaultResult::Segfault,
                                                                                                                                      };

                                                                                                                                      let vma_flags = vma.flags();
                                                                                                                                      let is_write = flags & FaultFlags::WRITE != 0;
                                                                                                                                      let is_exec = flags & FaultFlags::EXEC != 0;
                                                                                                                                      let is_read = flags & FaultFlags::READ != 0;

                                                                                                                                      // Verify permissions
                                                                                                                                      if is_write && !vma_flags.is_writable() {
                                                                                                                                          return MmFaultResult::PermissionDenied;
                                                                                                                                      }
                                                                                                                                      if is_exec && !vma_flags.is_executable() {
                                                                                                                                          return MmFaultResult::PermissionDenied;
                                                                                                                                      }
                                                                                                                                      if is_read && !vma_flags.is_readable() {
                                                                                                                                          return MmFaultResult::PermissionDenied;
                                                                                                                                      }

                                                                                                                                      drop(vma_mgr);

                                                                                                                                      // Allocate a physical page
                                                                                                                                      let phys_addr = match alloc_user_phys_page() {
                                                                                                                                          Some(addr) => addr,
                                                                                                                                          None => return MmFaultResult::OutOfMemory,
                                                                                                                                      };

                                                                                                                                      // Wait for an in-flight swap-out of this slot: the single-mapping
                                                                                                                                      // reclaim path installs the swap entry in the PTE BEFORE the device
                                                                                                                                      // write; reading the slot before that write lands would swap in
                                                                                                                                      // garbage. Bounded spin — the writer is a block write on another
                                                                                                                                      // context.
                                                                                                                                      {
                                                                                                                                          let mut spins: u64 = 0;
                                                                                                                                          while swap::swap_slot_pending(swap_type, swap_offset) {
                                                                                                                                              spins += 1;
                                                                                                                                              if spins > 64_000_000 {
                                                                                                                                                  break; // give up waiting; the read below fails -> retry
                                                                                                                                              }
                                                                                                                                              core::hint::spin_loop();
                                                                                                                                          }
                                                                                                                                      }

                                                                                                                                      // Read page contents from swap device
                                                                                                                                      if swap::swap_read_page(swap_type, swap_offset, phys_addr as usize).is_err() {
                                                                                                                                          crate::println!(
                                                                                                                                              "swap: failed to read page from swap (type={}, offset={})",
                                                                                                                                              swap_type,
                                                                                                                                              swap_offset
                                                                                                                                          );
                                                                                                                                          // R21-6: free the freshly allocated page on the failed read (was a
                                                                                                                                          // per-error page leak).
                                                                                                                                          crate::mm::page_alloc::free_page(phys_addr as usize);
                                                                                                                                          return MmFaultResult::OutOfMemory;
                                                                                                                                      }

                                                                                                                                      // Build PTE flags from VMA permissions
                                                                                                                                      let mut pte_flags =
                                                                                                                                          PageTableEntry::P | PageTableEntry::A | PageTableEntry::D | PageTableEntry::NX;
                                                                                                                                      pte_flags |= PageTableEntry::U;

                                                                                                                                      if vma_flags.is_writable() {
                                                                                                                                          pte_flags |= PageTableEntry::W;
                                                                                                                                      }
                                                                                                                                      if vma_flags.is_executable() {
                                                                                                                                          pte_flags &= !PageTableEntry::NX;
                                                                                                                                      }

                                                                                                                                      // Map the page
                                                                                                                                      // Swap-in map under the PTE-modify lock (round 6 MED). The swap read
                                                                                                                                      // finished above, so no I/O happens inside the irqsave section.
                                                                                                                                      let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(addr_space).lock();
                                                                                                                                      // Re-check under the lock (same double-fault race as the demand
                                                                                                                                      // paths). The swap-in page is exclusively owned, so freeing on loss is
                                                                                                                                      // safe; the swap slot is freed by the winner. E8-MM: device-window-
                                                                                                                                      // hole rule applies here too (see pte_counts_as_mapped).
                                                                                                                                      if  PageTableWalker::walk(root_ppn, fault_addr.bits() as u64) 
                                                                                                                                          .map_or(false, |(_ppn, bits)| {
                                                                                                                                              pte_counts_as_mapped(bits, fault_addr.bits() as u64)
                                                                                                                                          })
                                                                                                                                      {
                                                                                                                                          drop(_pte_guard);
                                                                                                                                          crate::mm::page_alloc::free_page(phys_addr as usize);
                                                                                                                                          // R8-4: Handled — see the demand-fault sites (no SIGSEGV for the
                                                                                                                                          // race loser).
                                                                                                                                          return MmFaultResult::Handled;
                                                                                                                                      }
                                                                                                                                      // SAFETY: root_ppn is a valid page table root, phys_addr was just
                                                                                                                                      // allocated, and pte_flags are built from valid VMA permissions. The
                                                                                                                                      // page is exclusively owned.

                                                                                                                                          if !map_page(root_ppn, fault_addr, PhysAddr::new(phys_addr), pte_flags) {
                                                                                                                                              // PT allocation failure (fake-OOM family): free the swap-in
                                                                                                                                              // page, deliver OOM.
                                                                                                                                              crate::mm::page_alloc::free_page(phys_addr as usize);
                                                                                                                                              return MmFaultResult::OutOfMemory;
                                                                                                                                          }

                                                                                                                                          // TLB flush
                                                                                                                                          crate::arch::cpu::invlpg(fault_addr.bits());


                                                                                                                                      // Set up rmap for the swapped-in page
                                                                                                                                      let page_pfn = (phys_addr >> PAGE_SHIFT) as usize;
                                                                                                                                      // Fresh exclusive page (INV-FRAME-1).
                                                                                                                                      if let Some(page) = crate::mm::page_desc::page_desc(page_pfn) {
                                                                                                                                          page.set_flag(PageFlag::Anonymous);
                                                                                                                                          page.set_flag(PageFlag::SwapBacked);
                                                                                                                                          page.set_index(fault_addr.bits() as usize / (PAGE_SIZE as usize));
                                                                                                                                          page.inc_mapcount();
                                                                                                                                          crate::mm::rmap::page_record_mapping(
                                                                                                                                              page,
                                                                                                                                              addr_space as *const _ as usize,
                                                                                                                                              fault_addr.bits() as usize,
                                                                                                                                          );

                                                                                                                                          // Add back to anon LRU
                                                                                                                                          crate::mm::lru::page_add_anon_lru(page);
                                                                                                                                      }
                                                                                                                                      drop(_pte_guard);

                                                                                                                                      // Free the swap slot (page is back in memory)
                                                                                                                                      swap::swap_free_slot(swap_type, swap_offset);

                                                                                                                                      // RSS accounting: the page is resident again.
                                                                                                                                      addr_space.add_rss(1);

                                                                                                                                      MmFaultResult::Handled

}

// Keep the static root reachable for diagnostics (interface parity with
// the riscv64 twin's imports).
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
#[allow(unused)]
pub(crate) 
            // INV-ARCH-PAGE-FAULT-11 (collected 2026-11-27 from x86_64/mm/page_fault.rs): verbatim body; callers use the safe
            // wrapper at the original path — obligations unchanged.
fn _root_table_ref() -> usize {


                              // SAFETY: address-of only, no dereference.
                              { &raw mut ROOT_PAGE_TABLE as usize }

}
