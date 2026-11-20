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

// ---------------------------------------------------------------------------
// INV-MM-4: the slab allocator's in-region raw accesses. All pointers are
// derived from the slab base + validated index/offset arithmetic, and the
// owning cache lock (or the fresh-unshared-slab / one-time-boot argument)
// provides the exclusivity the old inline SAFETY comments stated.
// ---------------------------------------------------------------------------

/// Read the u16 free-list link stored inside a slab object
/// (INV-MM-4: the pointer is inside a slab page of the locked cache).
#[inline]
pub fn slab_read_freelist_link(obj_ptr: *mut u8) -> u16 {
    // SAFETY: INV-MM-4 — in-slab, aligned-by-construction slot; cache
    // lock held by the caller.
    unsafe { *(obj_ptr as *const u16) }
}

/// Write the u16 free-list link into a slab object (INV-MM-4).
#[inline]
pub fn slab_write_freelist_link(obj_ptr: *mut u8, v: u16) {
    // SAFETY: INV-MM-4 — in-slab slot; cache lock held (or a fresh
    // slab not yet in any list).
    unsafe { *(obj_ptr as *mut u16) = v }
}

/// The SlabHeader of a slab page (INV-MM-4: addr is page-aligned inside
/// the slab region; the caller holds the cache lock).
#[inline]
pub fn slab_header_mut(addr: usize) -> &'static mut super::slab::SlabHeader {
    // SAFETY: INV-MM-4 — page-aligned in-region address; exclusivity
    // via the cache lock.
    unsafe { &mut *(addr as *mut super::slab::SlabHeader) }
}

/// The cache_idx byte of a slab header (INV-MM-4 shared arm: written by
/// the allocating kmalloc under the same lock; stable afterwards).
#[inline]
pub fn slab_header_cache_idx(addr: usize) -> u8 {
    // SAFETY: INV-MM-4 — in-region page-aligned header read.
    unsafe { (*(addr as *const super::slab::SlabHeader)).cache_idx }
}

/// One-time boot write of the SlabPages cell (INV-MM-4 init arm: called
/// once before secondary CPUs start, before any kmalloc/kfree).
#[inline]
pub fn slab_pages_init(base_addr: usize, max_pages: usize) {
    // SAFETY: INV-MM-4 init arm — the single-threaded boot window;
    // the cell is never written again.
    unsafe {
        *super::slab::slab_pages_cell().get() = super::slab::SlabPages::new(base_addr, max_pages);
    }
}

/// Shared view of the initialized SlabPages (INV-MM-4 shared arm: after
/// init the cell is only read).
#[inline]
pub fn slab_pages() -> &'static super::slab::SlabPages {
    // SAFETY: INV-MM-4 — post-init read-only access.
    unsafe { &*super::slab::slab_pages_cell().get() }
}

/// Zero a kmalloc'd allocation for kzalloc (INV-MM-4 owned arm: ptr and
/// size came from this allocator's kmalloc in the same call).
#[inline]
pub fn slab_zero(ptr: *mut u8, size: usize) {
    // SAFETY: INV-MM-4 — the caller owns the allocation; size matches.
    unsafe { core::ptr::write_bytes(ptr, 0, size) }
}

// The two Sync proofs for the slab statics (moved from slab.rs; same
// contracts the inline impls documented).
// SAFETY: SlabPages' only concurrently-modified field is an AtomicUsize;
// the rest are immutable after init; get_header_mut() addresses are
// non-overlapping page-aligned regions.
unsafe impl Sync for super::slab::SlabPages {}

// SAFETY: all SlabAllocator fields are individually Sync (per-cache
// Spinlocks, the SlabPages pair above, an AtomicUsize gate).
unsafe impl Sync for super::slab::SlabAllocator {}

// ---------------------------------------------------------------------------
// INV-MM-5: the per-CPU page-cache rows. `PcpRow` is an UnsafeCell whose
// mutable access is confined to the owning CPU inside its irq-off window
// (INV-PCP-3: nested same-CPU entry is serialized by the disabled IRQs);
// cross-CPU readers only ever PEEK (raw loads of count/list through the
// raw pointer, moment-stale by design).
// ---------------------------------------------------------------------------

// SAFETY: cross-CPU access never forms a reference (peeks only);
// mutable access is confined to the owning CPU's irq-off window.
unsafe impl Sync for super::pcp::PcpRow {}

/// Run `f` with this CPU's row, mutably (INV-MM-5 exclusive arm: the
/// caller disabled interrupts on THIS CPU — the only CPU that may touch
/// the row — so the &mut is exclusive).
#[inline]
pub fn pcp_row_exclusive<R>(
    c: usize,
    f: impl FnOnce(&mut super::pcp::PcpPageset) -> R,
) -> R {
    // SAFETY: INV-MM-5 — per-CPU row, IRQs off on this CPU (the
    // with_row caller's discipline).
    let row = unsafe { &*core::ptr::addr_of!(super::pcp::PCP_ROWS[c]) }.0.get();
    f(unsafe { &mut *row })
}

/// Peek row `c`'s cached count (INV-MM-5 peek arm: raw load, no
/// reference formed; moment-stale is acceptable for diagnostics).
#[inline]
pub fn pcp_row_count_peek(c: usize) -> usize {
    // SAFETY: INV-MM-5 — raw usize load through the row pointer.
    let row = unsafe { &*core::ptr::addr_of!(super::pcp::PCP_ROWS[c]) }.0.get();
    unsafe { (*row).count }
}

/// Peek a shared view of row `c` for forensic scans (INV-MM-5 peek
/// arm: read-only walk of count/list; racy by design).
#[inline]
pub fn pcp_row_ref_peek(c: usize) -> &'static super::pcp::PcpPageset {
    // SAFETY: INV-MM-5 — read-only borrow for the scan body; no
    /// reference escapes the caller.
    let row = unsafe { &*core::ptr::addr_of!(super::pcp::PCP_ROWS[c]) }.0.get();
    unsafe { &*row }
}

/// Copy `n` bytes between a pinned user frame's kernel alias and a
/// kernel buffer (INV-MM-6 pinned-alias arm: the pin keeps the frame
/// alive; the alias covers at least the piece length handed out by the
/// piece iterator).
#[inline]
pub fn copy_kva(dst: *mut u8, src: &[u8]) {
    // SAFETY: INV-MM-6 — dst is a live pinned-frame alias; src's slice
    // proves its own bytes; the pieces never overlap.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) }
}

/// The gather twin: copy from a pinned frame's kernel alias into a
/// kernel buffer slice (INV-MM-6 pinned-alias arm).
#[inline]
pub fn copy_kva_from(dst: &mut [u8], src: *const u8) {
    // SAFETY: INV-MM-6 — src is a live pinned-frame alias for
    // dst.len() bytes; the pieces never overlap.
    unsafe { core::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), dst.len()) }
}

// ---------------------------------------------------------------------------
// INV-MM-7: the swap-device static and page-frame slices.
// ---------------------------------------------------------------------------

/// Shared view of the swap-device static (INV-MM-7 dev-static arm: the
/// array is written only by the single-threaded swap_activate init;
/// after `enabled` publishes (Release), every field is atomic or
/// Spinlock-guarded).
#[inline]
pub fn swap_dev_static(idx: usize) -> &'static super::swap::SwapDevice {
    // SAFETY: INV-MM-7 — interior mutability is atomics/Spinlock only
    // after the init writes; the shared borrow races nothing.
    unsafe { &*super::swap::SWAP_DEVICES.as_mut_ptr().add(idx) }
}

/// Mutable view of the swap-device static (INV-MM-7 init arm: the
/// single-threaded swap_activate window).
#[inline]
pub fn swap_dev_static_mut(idx: usize) -> &'static mut super::swap::SwapDevice {
    // SAFETY: INV-MM-7 — init window; no concurrent reader exists
    // before `enabled` is stored.
    unsafe { &mut *super::swap::SWAP_DEVICES.as_mut_ptr().add(idx) }
}

/// The PAGE_SIZE mutable slice at a page-aligned kernel linear address
/// (INV-MM-7 page-slice arm: the frame is a buddy allocation the caller
/// owns for the call).
#[inline]
pub fn page_slice_mut(virt_addr: usize) -> &'static mut [u8] {
    // SAFETY: INV-MM-7 — page-aligned live frame; PAGE_SIZE length
    // matches the buddy allocation exactly.
    unsafe { core::slice::from_raw_parts_mut(virt_addr as *mut u8, crate::mm::page::PAGE_SIZE) }
}

/// The shared twin of [`page_slice_mut`] (INV-MM-7 page-slice arm).
#[inline]
pub fn page_slice(virt_addr: usize) -> &'static [u8] {
    // SAFETY: INV-MM-7 — as page_slice_mut, shared.
    unsafe { core::slice::from_raw_parts(virt_addr as *const u8, crate::mm::page::PAGE_SIZE) }
}

// ---------------------------------------------------------------------------
// INV-MM-8: the deferred-free pool. `POOL` is a static-mut ring guarded
// by POOL_LOCK (irqsave); the drain's dealloc routing is the same
// region dispatch buddy_allocator::dealloc_direct performs.
// ---------------------------------------------------------------------------

/// The deferred-free pool ring, mutably (INV-MM-8: caller holds
/// POOL_LOCK).
#[inline]
pub fn defer_pool_mut() -> &'static mut [super::deferred_free::Entry; super::deferred_free::POOL_CAP] {
    // SAFETY: INV-MM-8 — the lock is held; the static-mut ring has no
    // other accessor.
    unsafe { &mut *core::ptr::addr_of_mut!(super::deferred_free::POOL) }
}

/// Route one drained entry's dealloc by region (INV-MM-8 drain arm:
/// kfree for slab-resident pointers, the untracked buddy dealloc for
/// heap pointers — never the intercepted GlobalAlloc path, so a drained
/// entry cannot be re-deferred).
#[inline]
pub fn defer_dealloc(ptr: usize, layout: core::alloc::Layout) {
    let in_slab = crate::mm::slab::slab_region()
        .map(|(s, e)| ptr >= s && ptr < e)
        .unwrap_or(false);
    if in_slab {
        crate::mm::kfree(ptr as *mut u8);
    } else {
        crate::mm::buddy_allocator::dealloc_untracked_pub(ptr as *mut u8, layout);
    }
}

// ---------------------------------------------------------------------------
// INV-MM-9: the vmemmap boot static + init writes. `VMEMMAP_STATS` is a
// static-mut written once by init_vmemmap() (guarded by VMEMMAP_INIT)
// and only read afterwards; the init-path region zeroing and kernel-page
// mappings run in the single-threaded boot window.
// ---------------------------------------------------------------------------

/// The vmemmap stats static, shared (INV-MM-9: post-init read-only).
#[inline]
pub fn vmemmap_stats_read() -> super::vmemmap::VmemmapStats {
    // SAFETY: INV-MM-9 — initialized before any call; Copy plain read.
    unsafe { super::vmemmap::VMEMMAP_STATS }
}

/// The vmemmap stats static, mutable (INV-MM-9 init arm: the
/// VMEMMAP_INIT-guarded single write).
#[inline]
pub fn vmemmap_stats_write(v: super::vmemmap::VmemmapStats) {
    // SAFETY: INV-MM-9 — init window; no concurrent readers until
    /// init_vmemmap() returns.
    unsafe { super::vmemmap::VMEMMAP_STATS = v }
}

/// Zero the freshly reserved vmemmap region (INV-MM-9 init arm:
/// single-threaded boot; `size` is the exact reservation).
#[inline]
pub fn vmemmap_zero_region(virt: usize, size: usize) {
    // SAFETY: INV-MM-9 — boot window; the region came from
    // memblock_find_in_range and is page-backed by the mapping loop
    // that follows.
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, size) }
}

/// One kernel-page mapping of the vmemmap region (INV-MM-9 init arm;
/// map_kernel_page is a safe fn on every arch — this edge exists to
/// carry the boot-window discipline in one place).
#[inline]
pub fn vmemmap_map_kernel_page(vaddr: usize, paddr: usize, flags: u64) {
    crate::arch::mm::map_kernel_page(vaddr as u64, paddr as u64, flags);
}

// ---------------------------------------------------------------------------
// INV-MM-10: the address-space switch (riscv64/x86_64 switch_mm are
// unsafe arch primitives; the edge carries the live-root contract).
// ---------------------------------------------------------------------------

/// Activate a page-table root on the current CPU (INV-MM-10: `root_ppn`
/// is a live user/kernel root whose kernel half is mapped — the
/// context-switch discipline mm::switch_address_space documents).
#[inline]
pub fn switch_address_space(root_ppn: u64, asid: u16) {
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: INV-MM-10 — live root; the satp install is the
        // audited arch primitive.
        unsafe { crate::arch::context::switch_mm(root_ppn, asid) }
    }
    #[cfg(feature = "x86_64")]
    {
        crate::arch::mm::asid::switch_root(root_ppn, asid);
    }
    #[cfg(feature = "aarch64")]
    {
        crate::arch::mm::write_satp(crate::arch::mm::build_satp(asid, root_ppn as usize));
    }
}

// SAFETY: MmStruct's shared state is atomics + Spinlocks; the raw pgd
// root crosses threads only under the Arc-pin discipline (last-Arc
// teardown drains readers before Drop).
unsafe impl Send for super::mm_struct::MmStruct {}
unsafe impl Sync for super::mm_struct::MmStruct {}
