//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 page fault handling — stage-2 port of the riscv64 twin.
//!
//! handle_mm_fault drives the VMA layer + the 4-level user walker:
//! demand fill for anonymous/file/shmem VMAs, COW detection, stack
//! expansion, PROT_NONE marker re-presentation (mprotect(prot==0)
//! installed markers re-present under a granting VMA), and the twin's
//! locking protocol (map + rmap setup under one PTE section, R7-C3
//! re-check against CLONE_VM siblings).
//!
//! Still dormant from the twin, with the precise port requirements:
//!
//! - swap-in (swap entries in PTEs, migration entries): no producers
//!   exist on aarch64 — no swap-out path and no swap configuration,
//!   so there is nothing to swap IN yet. Porting means carrying the
//!   twin's V=0-nonzero-bits leaf decode here plus the mm/swap entry
//!   encoding in the walker; it stays dead code until a swap-out
//!   producer (kswapd/vmscan port) exists. Documented, not
//!   implemented.
//!
//! - the MAP_SHARED/MAP_PRIVATE page-cache alias (riscv64's PCACHE
//!   filemap_fault port): file faults take the twin's private-copy
//!   fallback arm (one private copy per mapper, functionally correct).
//!   A real port is a subsystem, not a patch: mapping page-cache frames
//!   directly needs the pin/refcount protocol across fork's
//!   copy_page_table_cow, unmap's release_cache_mapping_pin,
//!   writenotify-style W-diversion for private mappings, and shared
//!   writeback (dirty tracking + msync). cow_flags::PCACHE keeps the
//!   bit budget reserved, and the mprotect/marker-re-present paths
//!   written this stage preserve the bit like COW — so a later port
//!   does not have to touch them again. Deferred.
//!
//! - anonymous fault-around (riscv64 sc181): the twin installs a
//!   forward window of up to 32 zero pages on write faults into VMAs
//!   >= 1MB (cmdline `faaround=0` opt-out). The port is small in
//!   isolation, but it changes RSS/residency accounting on EVERY anon
//!   write fault in large VMAs, and aarch64 has no perf-gate tooling
//!   ported yet (no pbench/syscost/census parity) — an optimization
//!   whose effect cannot be measured here violates the "if it is not
//!   measured, add the DFX tool that measures it" rule, and an
//!   untestable accounting change risks mincore-class regressions
//!   sight unseen. Deferred until the aarch64 DFX tools land; the
//!   twin's anon_faultaround_forward is the reference shape.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use super::memory_layout::*;
use super::mm_ops::{
    alloc_user_phys_page, check_pte_permissions, is_cow_page,
    map_user_page, pte_lock_for, PageTableWalker, PTE_MODIFY_LOCK,
};
use super::pagetable::*;
use crate::mm::page::PAGE_SIZE as PAGE_SIZE_USIZE;
use crate::mm::vma::VmaType;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Fault access type flags (interface parity with the other backends)
pub struct FaultFlags;

impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 0x01;
    /// Write fault
    pub const WRITE: u32 = 0x02;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 0x04;
    /// User mode access
    pub const USER: u32 = 0x08;
    /// Kernel mode access
    pub const KERNEL: u32 = 0x10;
}

// ==================== Fault Result ====================

/// Page fault handling result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Handled successfully, can retry instruction
    Handled,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
    /// Already mapped (no handling needed)
    AlreadyMapped,
    /// COW pending (handled by handle_cow_fault)
    CowPending,
    /// Kernel exception fixed (via exception table)
    Fixed,
    /// Unfixable kernel exception
    KernelPanic,
}

// ==================== Stack Expansion ====================
/// Try to expand stack when a page fault occurs below the current stack
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn try_expand_stack( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, root_ppn: u64, ) -> MmFaultResult {
    crate::arch::arch_floor::aarch64::mm::page_fault::try_expand_stack(addr_space, fault_addr, flags, root_ppn)
}

// ==================== PROT_NONE marker re-presentation ====================
/// R-WRITESTORM-2 twin: a fault landed on a PROT_NONE marker leaf
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn protnone_represent( addr_space: &AddressSpace, root_ppn: u64, fault_addr: VirtAddr, flags: u32, ) -> MmFaultResult {


                                                                                                                      use crate::mm::page::VirtAddr as PageVirtAddr;

                                                                                                                      let is_write = flags & FaultFlags::WRITE != 0;
                                                                                                                      let is_exec = flags & FaultFlags::EXEC != 0;
                                                                                                                      let is_read = flags & FaultFlags::READ != 0;

                                                                                                                      // VMA lookup + access grant (riscv64 twin's verify order)
                                                                                                                      let vma_mgr = addr_space.vma_read();
                                                                                                                      let Some(vma) = (vma_mgr.find(PageVirtAddr::new(fault_addr.as_usize()))) else {
                                                                                                                          // Was mapped (marker), now outside every VMA — a real gap.
                                                                                                                          return MmFaultResult::PermissionDenied;
                                                                                                                      };
                                                                                                                      let vma_flags = vma.flags();
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

                                                                                                                      // Compose the re-presented software mask from the VMA grant
                                                                                                                      let mut sw = PageTableEntry::V | PageTableEntry::U
                                                                                                                          | PageTableEntry::A | PageTableEntry::D;
                                                                                                                      if vma_flags.is_readable() {
                                                                                                                          sw |= PageTableEntry::R;
                                                                                                                      }
                                                                                                                      if vma_flags.is_writable() {
                                                                                                                          sw |= PageTableEntry::W | PageTableEntry::R; // W requires R
                                                                                                                      }
                                                                                                                      if vma_flags.is_executable() {
                                                                                                                          sw |= PageTableEntry::X;
                                                                                                                      }

                                                                                                                      // Re-present under the PTE section, re-checking the marker (R7-C3
                                                                                                                      // discipline: the leaf may have moved under us between the
                                                                                                                      // lock-free walk and the section).
                                                                                                                      let _pte_guard = pte_lock_for(addr_space).lock_irqsave();
                                                                                                                      // SAFETY: read-only walk on the mm's root.
                                                                                                                      let Some((_ppn, now)) = ({ PageTableWalker::walk(root_ppn, fault_addr.bits()) }) else {
                                                                                                                          return MmFaultResult::PermissionDenied;
                                                                                                                      };
                                                                                                                      if !crate::arch::aarch64::mm::mm_ops::is_protnone_marker(now) {
                                                                                                                          // Someone re-presented/unmapped it first: the fresh state rules.
                                                                                                                          return MmFaultResult::Handled;
                                                                                                                      }
                                                                                                                      let cow_bit = now & crate::arch::aarch64::mm::mm_ops::cow_flags::COW;
                                                                                                                      let pcache_bit = now & crate::arch::aarch64::mm::mm_ops::cow_flags::PCACHE;
                                                                                                                      if cow_bit != 0 {
                                                                                                                          sw |= cow_bit;
                                                                                                                          sw &= !PageTableEntry::W; // W deferred to the COW fault
                                                                                                                      }
                                                                                                                      if pcache_bit != 0 {
                                                                                                                          sw |= pcache_bit;
                                                                                                                          // PRIVATE cache leaf never gains W in place (first write goes
                                                                                                                          // through COW); a SHARED cache leaf keeps W (stores must land
                                                                                                                          // in the cache frame). PCACHE is dormant on aarch64 today; the
                                                                                                                          // ownership rule rides the bit (riscv64 twin).
                                                                                                                          if sw & PageTableEntry::W != 0
                                                                                                                              && !crate::arch::aarch64::mm::mm_ops::cow_flags::pte_is_shared_cache(now)
                                                                                                                          {
                                                                                                                              sw |= crate::arch::aarch64::mm::mm_ops::cow_flags::COW;
                                                                                                                              sw &= !PageTableEntry::W;
                                                                                                                          }
                                                                                                                      }

                                                                                                                      // Write the L3 leaf: rebuild preserves the PPN (OUTPUT_ADDR field).
                                                                                                                      // SAFETY: root_ppn is this mm's live root; marker re-checked under
                                                                                                                      // the section.
                                                                                                                      if let Some(l3) = crate::arch::aarch64::mm::mmu_init::user_l3_table_for(root_ppn, fault_addr.bits()) {
                                                                                                                          let idx = ((fault_addr.bits() >> PAGE_SHIFT) & 0x1FF) as usize;
                                                                                                                          l3.set(idx, PageTableEntry::rebuild_user_leaf(now, sw));
                                                                                                                          // This translation changed; drop any cached walk.
                                                                                                                          crate::arch::aarch64::mm::asid::flush_tlb_page(fault_addr.bits() as usize, 0);
                                                                                                                          return MmFaultResult::Handled;
                                                                                                                      }
                                                                                                                      MmFaultResult::PermissionDenied

}

// ==================== Main Fault Handler ====================
/// handle_mm_fault - handle a user-mode page fault (twin contract):
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn handle_mm_fault( addr_space: &AddressSpace, fault_addr: VirtAddr, flags: u32, ) -> MmFaultResult {
    crate::arch::arch_floor::aarch64::mm::page_fault::handle_mm_fault(addr_space, fault_addr, flags)
}

// ==================== Utility ====================
/// Get the physical address mapped at a user virtual address
///
/// Collected into `arch::arch_floor` (INV-ARCH-PAGE-FAULT-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {


                                                           // SAFETY: read-only walk on a valid root.
                                                           { PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << PAGE_SHIFT) }

}
