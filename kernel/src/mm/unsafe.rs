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

use core::alloc::{GlobalAlloc, Layout};

use super::page_desc::Page;
use super::pglist::PglistData;

/// The first (and only, on UMA) online node, mutably
/// (INV-MM-1: caller holds the node's `lru_lock` or is in the
/// boot/single-CPU window — no other thread may hold a reference to
/// the same `PglistData` simultaneously).
#[inline]
pub fn first_node_mut() -> Option<&'static mut PglistData> {
    // INV-MM-1 exclusivity discipline unchanged; the raw NODE_DATA read
    // now lives in pglist_node_data_mut above.
    super::pglist::first_online_node_mut()
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
pub fn walk_fp_chain<const N: usize>(s0: u64, frames: &mut [u64; N]) {
    // SAFETY: INV-MM-2 diag arm — the walker is the audited
    // frame-pointer reader; s0 came from current_frame_pointer() on
    // this CPU's stack; N matches the caller's frame array.
    unsafe { crate::debug::memwatch::walk_fp_chain(s0, frames) }
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

/// Copy one page's contents between two frames' linear-map aliases
/// (the compaction mover; INV-MM-2 copy arm: src pinned by the
/// migration window's refcount, dst exclusively owned by the claim).
#[inline]
pub fn compact_copy_page(src_pfn: usize, dst_pfn: usize) {
    // SAFETY: INV-MM-2 — both frames live for the copy; the migration
    // window holds the source and the claim owns the destination.
    unsafe { super::page_desc::copy_page_contents(src_pfn, dst_pfn) }
}

// ---------------------------------------------------------------------------
// INV-MM-11: the kernel-layout OnceCell pair (init-guarded static).
// ---------------------------------------------------------------------------

/// Write the kernel layout (INV-MM-11 init arm: the compare_exchange
/// winner in the single boot pass).
#[inline]
pub fn kernel_layout_write(layout: super::layout::KernelMemoryLayout) {
    // SAFETY: INV-MM-11 — first and only write, guarded by the caller's
    // compare_exchange.
    unsafe { super::layout::KERNEL_LAYOUT.write(layout); }
}

/// The initialized kernel layout (INV-MM-11 shared arm: the init flag's
/// Acquire load pairs with the write's completion).
#[inline]
pub fn kernel_layout_ref() -> &'static super::layout::KernelMemoryLayout {
    // SAFETY: INV-MM-11 — the flag proves initialization completed.
    unsafe { super::layout::KERNEL_LAYOUT.assume_init_ref() }
}

// ---------------------------------------------------------------------------
// INV-MM-12: the pglist NODE_DATA boot static.
// ---------------------------------------------------------------------------

/// Write node 0 (INV-MM-12 init arm: early boot, single-threaded, before
/// any other NODE_DATA access).
#[inline]
pub fn node_data_init() {
    // SAFETY: INV-MM-12 — the boot-window store the unsafe fn's contract
    // names.
    // SAFETY: INV-MM-12 — the boot-window store the unsafe fn's
    // contract names.
    unsafe { super::pglist::NODE_DATA[0] = Some(super::pglist::PglistData::new(0)) }
}

// SAFETY: VmaManager's shared state is the lock-guarded VMA tree plus
// atomics (start_brk/brk) — every mutable access runs under its lock.
unsafe impl Send for super::vma::VmaManager {}
unsafe impl Sync for super::vma::VmaManager {}

/// The caller's return address (x86_64 diagnostic: `movq 8(%rbp)`;
/// INV-MM-2 diag arm — frame pointers are forced on, this stack frame
/// is live).
#[cfg(feature = "x86_64")]
#[inline]
pub fn x86_caller_return_address() -> u64 {
    let ret: u64;
    // SAFETY: INV-MM-2 diag arm — one frame-pointer-chain read of the
    // current stack.
    unsafe { core::arch::asm!("movq 8(%rbp), {}", out(reg) ret, options(att_syntax)) };
    ret
}

// ---------------------------------------------------------------------------
// INV-MM-13: the small-object allocator's in-page layout. Every address
// is derived from the kernel-heap direct map + the class geometry
// (first-object offset, object stride); the freelist links are volatile
// u16s stored inside free objects, and the in-page PageHdr is written
// under the class lock (or during fresh-page init, before publication).
// ---------------------------------------------------------------------------

/// The in-page PageHdr, mutable (INV-MM-13: class lock held or the
/// fresh page is not yet published).
#[inline]
pub fn sa_hdr_mut(page: usize) -> &'static mut super::small_alloc::PageHdr {
    // SAFETY: INV-MM-13 — page is a live class page; exclusivity via
    // the class lock / pre-publication.
    unsafe { &mut *(page as *mut super::small_alloc::PageHdr) }
}

/// The in-page PageHdr, shared (INV-MM-13 probe arm: read-only
/// magic/class fields of a live heap page).
#[inline]
pub fn sa_hdr_ref(page: usize) -> &'static super::small_alloc::PageHdr {
    // SAFETY: INV-MM-13 — the page lies inside the kernel heap direct
    // map (bitmap bit set); plain u8/u16 loads, worst case torn and
    // filtered by the magic check.
    unsafe { &*(page as *const super::small_alloc::PageHdr) }
}

/// Volatile u16 read (INV-MM-13: freelist link / diagnostic slot probe).
#[inline]
pub fn sa_read_u16(addr: usize) -> u16 {
    // SAFETY: INV-MM-13 — in-page slot address derived from the class
    // geometry.
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}

/// Volatile u16 write (INV-MM-13: freelist link install).
#[inline]
pub fn sa_write_u16(addr: usize, v: u16) {
    // SAFETY: INV-MM-13 — in-page slot address; lock held or fresh
    // page.
    unsafe { core::ptr::write_volatile(addr as *mut u16, v) }
}

/// Volatile u64 read (INV-MM-13: diagnostic slot-content probe).
#[inline]
pub fn sa_read_u64(addr: usize) -> u64 {
    // SAFETY: INV-MM-13 — live slab page of the class; slot in bounds.
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

// ---------------------------------------------------------------------------
// 2026-11-27 mainline consolidation: this file (git mv'd from edges.rs
// per the edges->unsafe.rs naming decision) is now the mm subsystem's
// ONE audited unsafe file. The families below absorbed the last inline
// unsafe blocks of the service files (pglist.rs, page_desc.rs,
// buddy_allocator.rs, vdso.rs — all four now zero-unsafe with file-level
// forbid). Existing INV-MM-1..13 contracts are unchanged; new families
// get the next free numbers.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// INV-MM-12 arms: the pglist NODE_DATA accessors (pglist.rs's own raw
// slot reads moved here so pglist.rs carries zero unsafe keywords).
// ---------------------------------------------------------------------------

/// Shared view of node `node_id`'s PglistData (INV-MM-12 read arm: the
/// slot was initialized by `node_data_init()` in the single-threaded
/// boot window and never reassigned; post-init readers are read-only).
#[inline]
pub fn pglist_node_data(node_id: usize) -> Option<&'static super::pglist::PglistData> {
    if node_id >= super::pglist::MAX_NUMNODES {
        return None;
    }
    // SAFETY: INV-MM-12 — bounds-checked post-init read of the boot
    // static's Option slot.
    unsafe { super::pglist::NODE_DATA[node_id].as_ref() }
}

/// Mutable view of node `node_id`'s PglistData (INV-MM-12 mut arm: the
/// caller is the boot init path or holds the node's `lru_lock` — the
/// INV-MM-1 exclusivity discipline).
#[inline]
pub fn pglist_node_data_mut(node_id: usize) -> Option<&'static mut super::pglist::PglistData> {
    if node_id >= super::pglist::MAX_NUMNODES {
        return None;
    }
    // SAFETY: INV-MM-12 — bounds-checked; exclusivity per INV-MM-1
    // (boot window or lru_lock held).
    unsafe { super::pglist::NODE_DATA[node_id].as_mut() }
}

// ---------------------------------------------------------------------------
// INV-MM-2 arms: page-descriptor raw edges (page_desc.rs's own raw
// accesses moved here so page_desc.rs carries zero unsafe keywords).
// ---------------------------------------------------------------------------

/// The linear-map page-copy body (INV-MM-2 copy arm: src/dst are live
/// frames pinned by the migration window / owned by the claim; both
/// addresses are phys_to_virt linear-map aliases, non-overlapping by
/// construction). The compact mover and `copy_page_contents` share it.
#[inline]
pub fn copy_page_contents_raw(src_pfn: usize, dst_pfn: usize) {
    use crate::arch::mm::memory_layout::{phys_to_virt, PhysAddr};
    // pfn_to_phys yields a physical address; it must go through the
    // linear mapping (phys_to_virt) before dereference — the raw PA has
    // no mapping once the MMU is on.
    let src = phys_to_virt(PhysAddr(super::zone::pfn_to_phys(src_pfn) as u64)).0 as usize as *const u8;
    let dst = phys_to_virt(PhysAddr(super::zone::pfn_to_phys(dst_pfn) as u64)).0 as usize as *mut u8;
    // SAFETY: INV-MM-2 copy arm — live, non-overlapping linear-map
    // frames; PAGE_SIZE matches the frame exactly.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, crate::mm::page::PAGE_SIZE) }
}

// ---------------------------------------------------------------------------
// INV-MM-14: the buddy allocator's metadata/scratch raw accesses. Every
// address is inside one of the allocator's own UnsafeCell arrays
// (MetaArray.meta, AuditScratch.data); exclusivity is the BuddyAllocator
// spinlock (get/get_mut/fill), the read-only diagnostic tolerance
// (snapshot: plain Copy integer loads, worst case torn — the
// small_alloc::object_span precedent), or the META_AUDIT_BUSY
// single-claim (the audit scratch).
// ---------------------------------------------------------------------------

/// Bounds-checked shared view of one BlockMeta (INV-MM-14: idx < MAX_PAGES
/// asserted by the wrapper; allocator lock held by the caller).
#[inline]
pub(super) fn buddy_meta_get(
    meta: &super::buddy_allocator::MetaArray,
    idx: usize,
) -> &super::buddy_allocator::BlockMeta {
    // SAFETY: INV-MM-14 — in-array index (bounds asserted by the
    // caller); shared read under the allocator lock.
    unsafe { &(*meta.data.get())[idx] }
}

/// Bounds-checked mutable view of one BlockMeta (INV-MM-14: allocator
/// lock held by the caller).
#[inline]
pub(super) fn buddy_meta_get_mut(
    meta: &super::buddy_allocator::MetaArray,
    idx: usize,
) -> &mut super::buddy_allocator::BlockMeta {
    // SAFETY: INV-MM-14 — in-array index (bounds asserted by the
    // caller); exclusive write under the allocator lock.
    unsafe { &mut (*meta.data.get())[idx] }
}

/// Copy-snapshot of one BlockMeta for the lockless forensic read
/// (INV-MM-14 snapshot arm: plain Copy integer loads, worst case a torn
/// combination — the same tolerance as small_alloc::object_span; the
/// caller bounds-checked idx).
#[inline]
pub(super) fn buddy_meta_snapshot(
    meta: &super::buddy_allocator::MetaArray,
    idx: usize,
) -> super::buddy_allocator::BlockMeta {
    // SAFETY: INV-MM-14 snapshot arm — in-array Copy read; a racing
    // writer can at worst tear the copy.
    unsafe { (*meta.data.get())[idx] }
}

/// Fill the `order` byte of BlockMeta[first..last] (INV-MM-14: both
/// indices bounds-checked/asserted by the caller; allocator lock held,
/// or the single-threaded init_sized seeder).
#[inline]
pub(super) fn buddy_meta_fill_orders(
    meta: &super::buddy_allocator::MetaArray,
    first: usize,
    last: usize,
    order: u8,
) {
    // SAFETY: INV-MM-14 — both indices < MAX_PAGES (asserted by the
    // caller); exclusive byte writes under the allocator lock / init
    // window. The cell cast, the ptr::add arithmetic and the field
    // writes are the unsafe ops; the loop comparison is safe.
    let base = meta.data.get() as *mut super::buddy_allocator::BlockMeta;
    let mut p = unsafe { base.add(first) };
    let end = unsafe { base.add(last) };
    while p < end {
        unsafe {
            (*p).order = order;
            p = p.add(1);
        }
    }
}

/// The meta-audit scratch bitmap, mutably (INV-MM-14 audit arm: the
/// caller holds the META_AUDIT_BUSY claim — one audit at a time across
/// CPUs; the borrow lives only inside the audit fn).
#[inline]
pub(super) fn buddy_audit_scratch_mut(
) -> &'static mut [u64; super::buddy_allocator::FREE_BITMAP_WORDS] {
    // SAFETY: INV-MM-14 audit arm — single-claim exclusivity; no other
    // audit can run concurrently to alias the borrow.
    unsafe { &mut *super::buddy_allocator::META_AUDIT_SCRATCH.data.get() }
}

/// Copy `len` bytes between two live kernel-heap blocks (INV-MM-14
/// realloc arm: the destination is this call's fresh allocation, the
/// source the caller's own block; len = min(old, new) size).
#[inline]
pub fn heap_block_copy(src: *const u8, dst: *mut u8, len: usize) {
    // SAFETY: INV-MM-14 realloc arm — both blocks are live owned heap
    // memory for len bytes; distinct allocations, never overlapping.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, len) }
}

// Send/Sync commitments moved from buddy_allocator.rs (same contracts
// the inline impls documented; types bumped to pub(super) so this file
// can name them).

// SAFETY: MetaArray is only accessed through BuddyAllocator, which holds
// a Spinlock; all get/get_mut calls happen under that lock, preventing
// concurrent mutable access to the underlying array.
unsafe impl Send for super::buddy_allocator::MetaArray {}
unsafe impl Sync for super::buddy_allocator::MetaArray {}

// SAFETY: BuddyAllocator uses internal Spinlock for all mutable operations
// and atomics for read-only fields, making it safe to share across threads.
unsafe impl Send for super::buddy_allocator::BuddyAllocator {}
unsafe impl Sync for super::buddy_allocator::BuddyAllocator {}

// SAFETY: see AuditScratch's doc — all access is claim-serialized by the
// META_AUDIT_BUSY single-claim.
unsafe impl Send for super::buddy_allocator::AuditScratch {}
unsafe impl Sync for super::buddy_allocator::AuditScratch {}

// ---------------------------------------------------------------------------
// INV-MM-15: the GlobalAlloc trait surface. The `unsafe impl` blocks and
// their `unsafe fn` signatures (the trait's own contract) live HERE; every
// body is the safe inherent twin in buddy_allocator.rs, so the audited
// file holds the trait seams and the service file holds the logic.
// ---------------------------------------------------------------------------

unsafe impl GlobalAlloc for super::buddy_allocator::BuddyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // The inherent twin takes precedence in method resolution; the
        // real logic (small_alloc packing, allocwatch, memwatch frames)
        // lives there.
        self.alloc(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.realloc(ptr, layout, new_size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.dealloc(ptr, layout)
    }
}

unsafe impl GlobalAlloc for super::buddy_allocator::CombinedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        super::buddy_allocator::combined_alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        super::buddy_allocator::combined_dealloc(ptr, layout)
    }
}

// ---------------------------------------------------------------------------
// INV-MM-VDSO: the vDSO data/code page raw edges (vdso.rs's own page-word
// accesses moved here so vdso.rs carries zero unsafe keywords; the file
// keeps the seqlock protocol docs and all safe wrappers). The page is a
// fixed machine-lifetime static (INV-VDSO-DATA-1 in vdso.rs).
// ---------------------------------------------------------------------------

/// The seqlock word of the vDSO data page, shared (INV-MM-VDSO-1: the
/// word IS data-page offset 0 — user-side readers validate it before
/// trusting the fields; kernel writes go through the tick/writer paths'
/// cmpxchg/fetch discipline).
#[inline]
pub fn vdso_seq_ref() -> &'static core::sync::atomic::AtomicU32 {
    // SAFETY: INV-MM-VDSO-1 — offset 0 of the fixed data-page static;
    // 4-aligned by the page's 4096 alignment.
    unsafe { &*(super::vdso::data_page_addr() as *const core::sync::atomic::AtomicU32) }
}

/// Volatile u64 read at byte offset `off` of the vDSO data page
/// (INV-MM-VDSO-2: page-constant offsets 8/16/24/40/48/52; volatile by
/// contract — user mappings read the same words, coherence is the seq
/// word's job).
#[inline]
pub fn vdso_field_read64(off: usize) -> u64 {
    // SAFETY: INV-MM-VDSO-2 — aligned in-page volatile load.
    unsafe { core::ptr::read_volatile(super::vdso::data_page_addr().add(off) as *const u64) }
}

/// Volatile u64 write at byte offset `off` of the vDSO data page
/// (INV-MM-VDSO-2 write arm: the caller is inside the seqlock write
/// section — seq odd; the volatile store pairs with the user readers'
/// seq-validated loads).
#[inline]
pub fn vdso_field_write64(off: usize, v: u64) {
    // SAFETY: INV-MM-VDSO-2 — aligned in-page volatile store under the
    // seqlock write section.
    unsafe { core::ptr::write_volatile(super::vdso::data_page_addr().add(off) as *mut u64, v) }
}

/// The riscv64 tick's three-field publish (INV-MM-VDSO-3 write arm: the
/// caller brackets this with seq |= 1 .. seq += 1; volatile stores so a
/// user reader mid-load sees either the old or the new full set, never a
/// torn layout).
#[inline]
pub fn vdso_rv_tick_publish(cycles: u64, mono_ns: u64, epoch_ns: u64) {
    let base = super::vdso::data_page_addr();
    // SAFETY: INV-MM-VDSO-3 — under the seqlock write section; aligned
    // in-page volatile stores.
    unsafe {
        core::ptr::write_volatile(base.add(8) as *mut u64, cycles);
        core::ptr::write_volatile(base.add(16) as *mut u64, mono_ns);
        core::ptr::write_volatile(base.add(24) as *mut u64, epoch_ns);
    }
}

/// One-time boot prime of the vDSO code page (INV-MM-VDSO-4 init arm:
/// `vdso_init()` runs in the single-threaded boot window before the page
/// is ever mapped R+X for userspace; afterwards the page is read-only).
#[inline]
pub fn vdso_code_page_prime(bytes: &[u8; 4096]) {
    // SAFETY: INV-MM-VDSO-4 — boot-window exclusive write of the fixed
    // static; the built page is exactly 4096 bytes.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), super::vdso::code_page_addr(), 4096) }
}

/// lfence;rdtsc — the ORDERED counter read (x86_64). A bare rdtsc can be
/// hoisted across the field loads by the CPU; the seq protocol requires
/// the TSC stamp to sit inside the seq1..seq2 window (the emitted vDSO
/// code uses the identical pair).
#[cfg(feature = "x86_64")]
#[inline]
pub fn lfence_rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: pure counter read; lfence orders it against the
    // surrounding seq-word loads (nomem/nostack: no memory effects).
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack)
        );
    }
    (lo as u64) | ((hi as u64) << 32)
}
