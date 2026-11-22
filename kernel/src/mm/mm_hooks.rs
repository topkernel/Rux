//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! M1a FLIP SEAM: the mm-policy hook table (the NetHooks/FsHooks
//! pattern, applied to the design doc's rung M1). The framework's mm
//! entry points that are POLICY — mmap/munmap placement, the free-area
//! search, the zap range, fork's VMA-decision half, and the fault
//! ROUTING (`handle_mm_fault`) — dispatch through THIS nullable table.
//! Until the per-arch carve lands in the `rux-mm` crate, the framework
//! registers its own in-tree policy (method/fn pointers, behavior
//! identical); each arch's carve replaces its rows with the crate's.
//!
//! THE SELF-RECURSION RULE (the S1 lesson, binding): a registered row
//! body must NEVER forward back into the accessor that dispatches the
//! same slot — registered bodies are the REAL policy fns (in-tree
//! method pointers pre-carve, crate fns post-carve), and call sites
//! read the table only through these accessors.

use crate::arch::mm::{MmFaultResult, VirtAddr as ArchVirtAddr};
use crate::mm::mm_struct::MmStruct;
use crate::mm::page::VirtAddr as PageVirtAddr;
use crate::mm::pagemap::{MapError, Perm};
use crate::mm::vma::{VmaFlags, VmaType};

/// The mm-policy hook set (one row per framework->policy entry point;
/// the reverse surface measured in docs/development/mm-split-rung.md —
/// 6 rows cover the ~40 framework call sites).
#[derive(Clone, Copy)]
pub struct MmHooks {
    /// The fault ROUTING (which arm handles the fault). Signature is
    /// identical on all three arches; the types are the compiling
    /// arch's own (`ArchVirtAddr`/`MmFaultResult`).
    pub handle_mm_fault: fn(&MmStruct, ArchVirtAddr, u32) -> MmFaultResult,
    /// mmap placement + VMA insertion.
    pub mmap: fn(
        &MmStruct,
        PageVirtAddr,
        usize,
        VmaFlags,
        VmaType,
        Perm,
        u32,
    ) -> Result<PageVirtAddr, MapError>,
    /// munmap VMA-piece removal + PTE teardown orchestration.
    pub munmap: fn(&MmStruct, PageVirtAddr, usize) -> Result<(), MapError>,
    /// The top-down free-area search.
    pub find_free_area: fn(&MmStruct, usize) -> Result<PageVirtAddr, MapError>,
    /// Zap a PTE range (keep the VMA).
    pub zap_page_range: fn(&MmStruct, PageVirtAddr, usize) -> Result<(), MapError>,
    /// fork's decision half (snapshot, COW-exempt scan, VMA replay);
    /// the PTE walk itself stays core-side (`copy_page_table_cow`).
    pub fork: fn(&MmStruct) -> Result<MmStruct, MapError>,
}

/// An unfilled row is a kernel misbuild, not a degraded mode: every
/// user address-space op routes through this table from the first
/// exec. Name it loudly instead of failing softly.
impl MmHooks {
    pub const EMPTY: MmHooks = MmHooks {
        handle_mm_fault: no_policy_handle_mm_fault,
        mmap: no_policy_mmap,
        munmap: no_policy_munmap,
        find_free_area: no_policy_find_free_area,
        zap_page_range: no_policy_zap_page_range,
        fork: no_policy_fork,
    };
}

fn no_policy_handle_mm_fault(_: &MmStruct, _: ArchVirtAddr, _: u32) -> MmFaultResult {
    panic!("M1a seam: mm policy not registered (handle_mm_fault)")
}
fn no_policy_mmap(
    _: &MmStruct,
    _: PageVirtAddr,
    _: usize,
    _: VmaFlags,
    _: VmaType,
    _: Perm,
    _: u32,
) -> Result<PageVirtAddr, MapError> {
    panic!("M1a seam: mm policy not registered (mmap)")
}
fn no_policy_munmap(_: &MmStruct, _: PageVirtAddr, _: usize) -> Result<(), MapError> {
    panic!("M1a seam: mm policy not registered (munmap)")
}
fn no_policy_find_free_area(_: &MmStruct, _: usize) -> Result<PageVirtAddr, MapError> {
    panic!("M1a seam: mm policy not registered (find_free_area)")
}
fn no_policy_zap_page_range(_: &MmStruct, _: PageVirtAddr, _: usize) -> Result<(), MapError> {
    panic!("M1a seam: mm policy not registered (zap_page_range)")
}
fn no_policy_fork(_: &MmStruct) -> Result<MmStruct, MapError> {
    panic!("M1a seam: mm policy not registered (fork)")
}

pub static MM_HOOKS: crate::safety::slot::OnceSlot<MmHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins —
/// the composition bin's `rux_mm::register_all` runs before the
/// framework's in-tree registration, so a carved arch's crate rows
/// prevail).
pub fn mm_register_hooks(hooks: MmHooks) {
    let _ = MM_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static MmHooks {
    MM_HOOKS.get().unwrap_or(&MmHooks::EMPTY)
}

// ---- accessors: the framework's only door into mm policy ----

#[inline]
pub fn handle_mm_fault(mm: &MmStruct, addr: ArchVirtAddr, flags: u32) -> MmFaultResult {
    (hooks().handle_mm_fault)(mm, addr, flags)
}

#[inline]
pub fn mmap(
    mm: &MmStruct,
    addr: PageVirtAddr,
    size: usize,
    flags: VmaFlags,
    vma_type: VmaType,
    perm: Perm,
    map_flags: u32,
) -> Result<PageVirtAddr, MapError> {
    (hooks().mmap)(mm, addr, size, flags, vma_type, perm, map_flags)
}

#[inline]
pub fn munmap(mm: &MmStruct, addr: PageVirtAddr, size: usize) -> Result<(), MapError> {
    (hooks().munmap)(mm, addr, size)
}

#[inline]
pub fn find_free_area(mm: &MmStruct, size: usize) -> Result<PageVirtAddr, MapError> {
    (hooks().find_free_area)(mm, size)
}

#[inline]
pub fn zap_page_range(
    mm: &MmStruct,
    start: PageVirtAddr,
    size: usize,
) -> Result<(), MapError> {
    (hooks().zap_page_range)(mm, start, size)
}

#[inline]
pub fn fork(mm: &MmStruct) -> Result<MmStruct, MapError> {
    (hooks().fork)(mm)
}
