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
//!   measured, add the DEBUG tool that measures it" rule, and an
//!   untestable accounting change risks mincore-class regressions
//!   sight unseen. Deferred until the aarch64 DEBUG tools land; the
//!   twin's anon_faultaround_forward is the reference shape.

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
// FaultFlags + MmFaultResult unified at the SEAM (api/mm.rs) with
// the M1a carve; re-export keeps every historical path resolving.
pub use crate::api::mm::{FaultFlags, MmFaultResult};

// ==================== Fault Result ====================

/// Page fault handling result

// ==================== Stack Expansion ====================


// ==================== PROT_NONE marker re-presentation ====================


// ==================== Main Fault Handler ====================


// ==================== Utility ====================

/// Get the physical address mapped at a user virtual address
/// (diagnostics; 4-level user-table walk, block leaves folded)
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    // SAFETY: read-only walk on a valid root.
    unsafe { PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << PAGE_SHIFT) }
}
