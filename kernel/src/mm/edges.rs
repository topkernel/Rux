//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The mm subsystem's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L7 (2026-11-21), framekernel doctrine: the mm
//! SERVICE files (lru.rs, page_alloc.rs, ...) end with ZERO `unsafe`
//! keywords — checkable by grep, ready for `#![forbid(unsafe_code)]`.
//! Every genuinely-unsafe operation they need lives HERE as a named
//! edge with an INV contract, in the shape of
//! `kernel/src/sync/edges.rs` / `kernel/src/syscall/edges.rs` (one
//! audit per edge, not per call site).
//!
//! INV registry:
//!
//! - INV-MM-1 (node borrow) — the single UMA node's `PglistData` as a
//!   mutable reference: the caller holds the node's `lru_lock` (every
//!   LRU mutation) or runs before secondary CPUs come online (the
//!   boot/alloc-init paths). This is the exact contract
//!   `pglist::first_online_node_mut` documents; the edge restates it
//!   once and hands the caller safe code.
//! - INV-MM-2 (page-descriptor borrow) — the vmemmap page descriptor
//!   of a PFN: descriptors are machine-lifetime statics and the PFN
//!   was validated by the path that produced it (a locked LRU chain
//!   entry, an allocation this thread owns, or a walk under the PTE
//!   lock). `None` mirrors the null-check the call sites performed.

use super::page_desc::Page;
use super::pglist::PglistData;

/// The first (and only, on UMA) online node, mutably
/// (INV-MM-1: caller holds the node's `lru_lock` or is in the
/// boot/single-CPU window — no other thread may hold a reference to
/// the same `PglistData` simultaneously).
#[inline]
pub fn first_node_mut() -> Option<&'static mut PglistData> {
    // SAFETY: INV-MM-1 — the lock/boot exclusivity the unsafe fn's
    // contract names, proven by every caller of this edge.
    unsafe { super::pglist::first_online_node_mut() }
}

/// The first (and only, on UMA) online node, shared (INV-MM-1 shared
/// arm: read-only statistics/census walks).
#[inline]
pub fn first_node() -> Option<&'static PglistData> {
    super::pglist::first_online_node()
}

/// The page descriptor of `pfn`, mutably (INV-MM-2: the PFN comes from
/// a locked LRU chain, an owned allocation, or a validated walk;
/// descriptors are machine-lifetime statics).
#[inline]
pub fn page_of(pfn: usize) -> Option<&'static mut Page> {
    let p = super::page_desc::pfn_to_page_mut(pfn);
    if p.is_null() {
        None
    } else {
        // SAFETY: INV-MM-2 — non-null vmemmap descriptor of a frame
        // the caller's path validated; machine-lifetime storage.
        Some(unsafe { &mut *p })
    }
}

/// The page descriptor of `pfn`, shared (INV-MM-2 shared arm:
/// read-only flag/lru-field probes).
#[inline]
pub fn page_ref_of(pfn: usize) -> Option<&'static Page> {
    let p = super::page_desc::pfn_to_page_mut(pfn) as *const Page;
    if p.is_null() {
        None
    } else {
        // SAFETY: INV-MM-2 — as page_of; the shared borrow only reads.
        Some(unsafe { &*p })
    }
}

// ---------------------------------------------------------------------------
// INV-MM-3: the memblock early-boot static. `MEMBLOCK` is a `static mut`
// singleton written only before the scheduler starts (init/add/reserve/
// find+reserve, single-threaded boot) and read-only afterwards; the
// accessors below move the raw-static discipline here once.
// ---------------------------------------------------------------------------

/// Shared borrow of the initialized memblock (INV-MM-3 shared arm: after
/// early boot the data is effectively immutable; before that, the caller
/// runs in the single-threaded boot window).
#[inline]
pub fn memblock_ref() -> &'static super::memblock::MemBlock {
    // SAFETY: INV-MM-3 — MEMBLOCK is initialized (memblock_init ran)
    // before any call reaches here; post-boot it is never mutated.
    unsafe { &*super::memblock::memblock_raw() }
}

/// Mutable borrow of the memblock (INV-MM-3 exclusive arm: early boot
/// only, before the scheduler starts — no concurrent access exists).
#[inline]
pub fn memblock_mut() -> &'static mut super::memblock::MemBlock {
    // SAFETY: INV-MM-3 — single-threaded boot window; the caller is the
    // boot path that owns MEMBLOCK exclusively.
    unsafe { &mut *super::memblock::memblock_raw() }
}

/// Walk the CURRENT stack's frame-pointer chain into `frames` (INV-MM-2
/// diag arm: this CPU's own stack, bounded by the callee's guard).
#[inline]
pub fn walk_fp_chain(s0: u64, frames: &mut [u64; 10]) {
    // SAFETY: INV-MM-2 diag arm — the walker is the audited
    // frame-pointer reader; s0 came from current_frame_pointer() on
    // this CPU's stack.
    unsafe { crate::dfx::memwatch::walk_fp_chain(s0, frames) }
}
