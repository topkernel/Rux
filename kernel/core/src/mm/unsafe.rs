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
//! `kernel/core/src/sync/unsafe.rs` / `kernel/core/src/syscall/unsafe.rs` (one
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

use super::types::Page;
use super::types::PglistData;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicU32, Ordering};
use super::types::{AuditScratch, BlockMeta, BUDDY_MAX_ORDER, BUDDY_MAX_PAGES, BuddyAllocator, CombinedAllocator, FREE_BITMAP_WORDS, MetaArray, PageDescStats, PhysFrameNr, PAGE_SIZE};
use super::hotpath::{HEAP_ALLOCATOR, MAX_PAGES, MIN_PFN, nr_descs, pfn_to_page, pfn_to_page_mut};
#[cfg(feature = "x86_64")]
use super::hotpath::x86_track;

/// The first (and only, on UMA) online node, mutably
/// (INV-MM-1: caller holds the node's `lru_lock` or is in the
/// boot/single-CPU window — no other thread may hold a reference to
/// the same `PglistData` simultaneously).
#[inline]
pub fn first_node_mut() -> Option<&'static mut PglistData> {
    // SAFETY: INV-MM-1 — the lock/boot exclusivity the unsafe fn's
    // contract names, proven by every caller of this edge.
    unsafe { first_online_node_mut() }
}

/// The first (and only, on UMA) online node, shared (INV-MM-1 shared
/// arm: read-only statistics/census walks).
#[inline]
pub fn first_node() -> Option<&'static PglistData> {
    super::hotpath::first_online_node()
}

/// The page descriptor of `pfn`, mutably (INV-MM-2: the PFN comes from
/// a locked LRU chain, an owned allocation, or a validated walk;
/// descriptors are machine-lifetime statics).
#[inline]
pub fn page_of(pfn: usize) -> Option<&'static mut Page> {
    let p = super::hotpath::pfn_to_page_mut(pfn);
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
    let p = super::hotpath::pfn_to_page_mut(pfn) as *const Page;
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
pub fn memblock_ref() -> &'static super::types::MemBlock {
    // SAFETY: INV-MM-3 — MEMBLOCK is initialized (memblock_init ran)
    // before any call reaches here; post-boot it is never mutated.
    unsafe { &*super::hotpath::memblock_raw() }
}

/// Mutable borrow of the memblock (INV-MM-3 exclusive arm: early boot
/// only, before the scheduler starts — no concurrent access exists).
#[inline]
pub fn memblock_mut() -> &'static mut super::types::MemBlock {
    // SAFETY: INV-MM-3 — single-threaded boot window; the caller is the
    // boot path that owns MEMBLOCK exclusively.
    unsafe { &mut *super::hotpath::memblock_raw() }
}

/// Walk the CURRENT stack's frame-pointer chain into `frames` (INV-MM-2
/// diag arm: this CPU's own stack, bounded by the callee's guard).
#[inline]
pub fn walk_fp_chain<const N: usize>(s0: u64, frames: &mut [u64; N]) {
    // SAFETY: INV-MM-2 diag arm — the walker is the audited
    // frame-pointer reader; s0 came from current_frame_pointer() on
    // this CPU's stack; N matches the caller's frame array.
    unsafe { crate::debug::hotpath::walk_fp_chain(s0, frames) }
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
pub fn slab_header_mut(addr: usize) -> &'static mut super::types::SlabHeader {
    // SAFETY: INV-MM-4 — page-aligned in-region address; exclusivity
    // via the cache lock.
    unsafe { &mut *(addr as *mut super::types::SlabHeader) }
}

/// The cache_idx byte of a slab header (INV-MM-4 shared arm: written by
/// the allocating kmalloc under the same lock; stable afterwards).
#[inline]
pub fn slab_header_cache_idx(addr: usize) -> u8 {
    // SAFETY: INV-MM-4 — in-region page-aligned header read.
    unsafe { (*(addr as *const super::types::SlabHeader)).cache_idx }
}

/// One-time boot write of the SlabPages cell (INV-MM-4 init arm: called
/// once before secondary CPUs start, before any kmalloc/kfree).
#[inline]
pub fn slab_pages_init(base_addr: usize, max_pages: usize) {
    // SAFETY: INV-MM-4 init arm — the single-threaded boot window;
    // the cell is never written again.
    unsafe {
        *super::hotpath::slab_pages_cell().get() = super::types::SlabPages::new(base_addr, max_pages);
    }
}

/// Shared view of the initialized SlabPages (INV-MM-4 shared arm: after
/// init the cell is only read).
#[inline]
pub fn slab_pages() -> &'static super::types::SlabPages {
    // SAFETY: INV-MM-4 — post-init read-only access.
    unsafe { &*super::hotpath::slab_pages_cell().get() }
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
unsafe impl Sync for super::types::SlabPages {}

// SAFETY: all SlabAllocator fields are individually Sync (per-cache
// Spinlocks, the SlabPages pair above, an AtomicUsize gate).
unsafe impl Sync for super::types::SlabAllocator {}

// ---------------------------------------------------------------------------
// INV-MM-5: the per-CPU page-cache rows. `PcpRow` is an UnsafeCell whose
// mutable access is confined to the owning CPU inside its irq-off window
// (INV-PCP-3: nested same-CPU entry is serialized by the disabled IRQs);
// cross-CPU readers only ever PEEK (raw loads of count/list through the
// raw pointer, moment-stale by design).
// ---------------------------------------------------------------------------

// SAFETY: cross-CPU access never forms a reference (peeks only);
// mutable access is confined to the owning CPU's irq-off window.
unsafe impl Sync for super::types::PcpRow {}

/// Run `f` with this CPU's row, mutably (INV-MM-5 exclusive arm: the
/// caller disabled interrupts on THIS CPU — the only CPU that may touch
/// the row — so the &mut is exclusive).
#[inline]
pub fn pcp_row_exclusive<R>(
    c: usize,
    f: impl FnOnce(&mut super::types::PcpPageset) -> R,
) -> R {
    // SAFETY: INV-MM-5 — per-CPU row, IRQs off on this CPU (the
    // with_row caller's discipline).
    let row = unsafe { &*core::ptr::addr_of!(super::hotpath::PCP_ROWS[c]) }.0.get();
    f(unsafe { &mut *row })
}

/// Peek row `c`'s cached count (INV-MM-5 peek arm: raw load, no
/// reference formed; moment-stale is acceptable for diagnostics).
#[inline]
pub fn pcp_row_count_peek(c: usize) -> usize {
    // SAFETY: INV-MM-5 — raw usize load through the row pointer.
    let row = unsafe { &*core::ptr::addr_of!(super::hotpath::PCP_ROWS[c]) }.0.get();
    unsafe { (*row).count }
}

/// Peek a shared view of row `c` for forensic scans (INV-MM-5 peek
/// arm: read-only walk of count/list; racy by design).
#[inline]
pub fn pcp_row_ref_peek(c: usize) -> &'static super::types::PcpPageset {
    // SAFETY: INV-MM-5 — read-only borrow for the scan body; no
    /// reference escapes the caller.
    let row = unsafe { &*core::ptr::addr_of!(super::hotpath::PCP_ROWS[c]) }.0.get();
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
// INV-MM-7: page-frame slices.
// ---------------------------------------------------------------------------

/// The PAGE_SIZE mutable slice at a page-aligned kernel linear address
/// (INV-MM-7 page-slice arm: the frame is a buddy allocation the caller
/// owns for the call).
#[inline]
pub fn page_slice_mut(virt_addr: usize) -> &'static mut [u8] {
    // SAFETY: INV-MM-7 — page-aligned live frame; PAGE_SIZE length
    // matches the buddy allocation exactly.
    unsafe { core::slice::from_raw_parts_mut(virt_addr as *mut u8, crate::mm::types::PAGE_SIZE) }
}

/// The shared twin of [`page_slice_mut`] (INV-MM-7 page-slice arm).
#[inline]
pub fn page_slice(virt_addr: usize) -> &'static [u8] {
    // SAFETY: INV-MM-7 — as page_slice_mut, shared.
    unsafe { core::slice::from_raw_parts(virt_addr as *const u8, crate::mm::types::PAGE_SIZE) }
}

// ---------------------------------------------------------------------------
// INV-MM-8: the deferred-free pool. `POOL` is a static-mut ring guarded
// by POOL_LOCK (irqsave); the drain's dealloc routing is the same
// region dispatch buddy_allocator::dealloc_direct performs.
// ---------------------------------------------------------------------------

/// The deferred-free pool ring, mutably (INV-MM-8: caller holds
/// POOL_LOCK).
#[inline]
pub fn defer_pool_mut() -> &'static mut [super::types::Entry; super::hotpath::POOL_CAP] {
    // SAFETY: INV-MM-8 — the lock is held; the static-mut ring has no
    // other accessor.
    unsafe { &mut *core::ptr::addr_of_mut!(super::hotpath::POOL) }
}

/// Route one drained entry's dealloc by region (INV-MM-8 drain arm:
/// kfree for slab-resident pointers, the untracked buddy dealloc for
/// heap pointers — never the intercepted GlobalAlloc path, so a drained
/// entry cannot be re-deferred).
#[inline]
pub fn defer_dealloc(ptr: usize, layout: core::alloc::Layout) {
    let in_slab = crate::mm::hotpath::slab_region()
        .map(|(s, e)| ptr >= s && ptr < e)
        .unwrap_or(false);
    if in_slab {
        crate::mm::hotpath::kfree(ptr as *mut u8);
    } else {
        crate::mm::hotpath::dealloc_untracked_pub(ptr as *mut u8, layout);
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
pub fn vmemmap_stats_read() -> super::types::VmemmapStats {
    // SAFETY: INV-MM-9 — initialized before any call; Copy plain read.
    unsafe { super::hotpath::VMEMMAP_STATS }
}

/// The vmemmap stats static, mutable (INV-MM-9 init arm: the
/// VMEMMAP_INIT-guarded single write).
#[inline]
pub fn vmemmap_stats_write(v: super::types::VmemmapStats) {
    // SAFETY: INV-MM-9 — init window; no concurrent readers until
    /// init_vmemmap() returns.
    unsafe { super::hotpath::VMEMMAP_STATS = v }
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
unsafe impl Send for super::types::MmStruct {}
unsafe impl Sync for super::types::MmStruct {}

/// Copy one page's contents between two frames' linear-map aliases
/// (the compaction mover; INV-MM-2 copy arm: src pinned by the
/// migration window's refcount, dst exclusively owned by the claim).
#[inline]
pub fn compact_copy_page(src_pfn: usize, dst_pfn: usize) {
    // SAFETY: INV-MM-2 — both frames live for the copy; the migration
    // window holds the source and the claim owns the destination.
    unsafe { copy_page_contents(src_pfn, dst_pfn) }
}

// ---------------------------------------------------------------------------
// INV-MM-11: the kernel-layout OnceCell pair (init-guarded static).
// ---------------------------------------------------------------------------

/// Write the kernel layout (INV-MM-11 init arm: the compare_exchange
/// winner in the single boot pass).
#[inline]
pub fn kernel_layout_write(layout: super::types::KernelMemoryLayout) {
    // SAFETY: INV-MM-11 — first and only write, guarded by the caller's
    // compare_exchange.
    unsafe { super::hotpath::KERNEL_LAYOUT.write(layout); }
}

/// The initialized kernel layout (INV-MM-11 shared arm: the init flag's
/// Acquire load pairs with the write's completion).
#[inline]
pub fn kernel_layout_ref() -> &'static super::types::KernelMemoryLayout {
    // SAFETY: INV-MM-11 — the flag proves initialization completed.
    unsafe { super::hotpath::KERNEL_LAYOUT.assume_init_ref() }
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
    unsafe { super::hotpath::NODE_DATA[0] = Some(super::types::PglistData::new(0)) }
}

// SAFETY: VmaManager's shared state is the lock-guarded VMA tree plus
// atomics (start_brk/brk) — every mutable access runs under its lock.
unsafe impl Send for super::types::VmaManager {}
unsafe impl Sync for super::types::VmaManager {}

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
pub fn sa_hdr_mut(page: usize) -> &'static mut super::types::PageHdr {
    // SAFETY: INV-MM-13 — page is a live class page; exclusivity via
    // the class lock / pre-publication.
    unsafe { &mut *(page as *mut super::types::PageHdr) }
}

/// The in-page PageHdr, shared (INV-MM-13 probe arm: read-only
/// magic/class fields of a live heap page).
#[inline]
pub fn sa_hdr_ref(page: usize) -> &'static super::types::PageHdr {
    // SAFETY: INV-MM-13 — the page lies inside the kernel heap direct
    // map (bitmap bit set); plain u8/u16 loads, worst case torn and
    // filtered by the magic check.
    unsafe { &*(page as *const super::types::PageHdr) }
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

// ==========================================================================
// pass 3 (seed-core §2c.5): mm's collected unsafe — the inline
// blocks of the subsystem's mechanism files, one contract each.
// ==========================================================================

// ------------ vDSO data page + TSC anchor (INV-VDSO-DATA-1) ------------

/// The seqlock word IS data-page offset 0 (odd = write in progress).
///
/// It must be accessed THROUGH the page pointer, never through a
/// separate static: the user-side vDSO code reads page[0] and retries
/// while it is odd or changed across the read. The original separate
/// VDSO_SEQ static never aliased the page, so page[0] stayed 0 forever,
/// no reader ever retried, and a reader descheduled between its
/// base_tsc and base_mono_ns loads paired a stale tsc with a fresh mono
/// — a vDSO clock AHEAD of the syscall clock by the whole preemption
/// gap (LTP clock_gettime04: "Time travelled backwards (-454618735 ns)"
/// on every clock, only on loads noisy enough to deschedule the reader
/// for hundreds of ms).
// SAFETY: only written from the timer-tick refresh paths below.
/// INV-VDSO-DATA-1: the seqlock word IS data-page offset 0 — it is accessed THROUGH the page pointer, never through a separate static (the LTP clock_gettime04 ahead-clock fix); only the tick refresh paths write it.
unsafe fn vdso_seq() -> &'static AtomicU32 {
    unsafe { &*(super::hotpath::VDSO_DATA_PAGE.0.as_ptr() as *const AtomicU32) }
}
/// Refresh the data page snapshot (timer tick / clock-set paths).
///
/// Seqlock protocol: seq |= 1 (begin) → write fields → seq += 1 (end,
/// even). Readers retry while seq is odd or changed across the read.
/// INV-VDSO-DATA-1 tick arm: the page is written only here, under the seqlock (seq |= 1 begin, volatile field stores at page-constant offsets, seq += 1 end); user readers validate the seq word before trusting the fields.
#[cfg(not(feature = "x86_64"))]
pub fn vdso_data_tick() {
    let cycles = crate::timer::read_time();
    // 10 MHz source: 100 ns per cycle, exact.
    let mono_ns = cycles.saturating_mul(100);
    let epoch_ns = crate::timer::wall_epoch_offset_ns();
    // SAFETY: the page is only written here under the seqlock; user
    // readers validate via the seq word before trusting the fields.
    unsafe {
        let seq = vdso_seq();
        seq.fetch_or(1, Ordering::Release);
        let base = super::hotpath::VDSO_DATA_PAGE.0.as_mut_ptr();
        core::ptr::write_volatile(base.add(8) as *mut u64, cycles);
        core::ptr::write_volatile(base.add(16) as *mut u64, mono_ns);
        core::ptr::write_volatile(base.add(24) as *mut u64, epoch_ns);
        // tz fields (32/36) stay zero.
        seq.fetch_add(1, Ordering::Release);
    }
}
/// INV-VDSO-DATA-1 read arm: plain volatile u64 load at a page-constant offset; coherence is the seq word's job (x86 unified-track page).
#[cfg(feature = "x86_64")]
    #[inline]
    pub(crate) unsafe fn rd(off: usize) -> u64 {
        unsafe {
            core::ptr::read_volatile(
                super::hotpath::VDSO_DATA_PAGE.0.as_ptr().add(off) as *const u64,
            )
        }
    }
/// INV-VDSO-DATA-1 write arm: volatile u64 store at a page-constant offset; the caller holds the cmpxchg even-to-odd writer claim (the seq protocol).
#[cfg(feature = "x86_64")]
    #[inline]
    pub(crate) unsafe fn wr(off: usize, v: u64) {
        unsafe {
            core::ptr::write_volatile(
                super::hotpath::VDSO_DATA_PAGE.0.as_mut_ptr().add(off) as *mut u64,
                v,
            )
        }
    }
/// INV-VDSO-DATA-1: lfence;rdtsc — the ordered counter read keeps the TSC stamp inside the seq window (a bare rdtsc can hoist across the field loads; the emitted vDSO code uses the identical pair).
#[cfg(feature = "x86_64")]
    /// lfence;rdtsc — the ORDERED counter read. A bare rdtsc can be
    /// hoisted across the field loads by the CPU; the seq protocol
    /// requires the TSC stamp to sit inside the seq1..seq2 window (the
    /// emitted vDSO code uses the identical pair).
    #[inline]
    pub fn read_tsc_ordered() -> u64 {
        let lo: u32;
        let hi: u32;
        // SAFETY: pure counter read; lfence orders it against the
        // surrounding seq-word loads.
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
/// INV-VDSO-DATA-1 writer-lock arm: cmpxchg even-to-odd takes the page for writing; a losing writer skips the round (the seq word doubles as the writer lock — the multi-writer tear fix).
#[cfg(feature = "x86_64")]
    /// Take the page for writing. Returns false when another writer
    /// holds it (seq odd) — the caller skips this round.
    #[inline]
    pub(crate) unsafe fn seq_try_begin() -> bool {
        unsafe {
            let seq = vdso_seq();
            let cur = seq.load(Ordering::Relaxed);
            if cur & 1 != 0 {
                return false;
            }
            seq.compare_exchange(cur, cur | 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        }
    }
/// INV-VDSO-DATA-1: seq += 1 (Release) closes the write window; readers retry while the word is odd or changed across their read.
#[cfg(feature = "x86_64")]
    #[inline]
    pub(crate) unsafe fn seq_end() {
        unsafe { vdso_seq().fetch_add(1, Ordering::Release); }
    }
/// INV-VDSO-DATA-1 publish arm: boot-time single-threaded promote (pre-SMP, IRQs off), but the seq protocol runs anyway for uniformity; mult is write-once and base_mono_nom makes the track continuous with the previous clock.
#[cfg(feature = "x86_64")]
    /// Publish the initial anchor (promote_tsc_timekeeper; boot-time
    /// single-threaded, but the seq protocol runs anyway for uniformity).
    /// `base_mono_nom` must make the track continuous with the previous
    /// clock (the caller computes the continuity value).
    pub fn publish_tsc_anchor(mult: u64, cycle_last: u64, base_mono_nom: u64) {
        unsafe {
            if !seq_try_begin() {
                // Cannot happen today (promote runs pre-SMP with IRQs
                // off); if it ever races, the next tick refresh
                // publishes — mult stays 0 and userspace falls back to
                // the syscall until then.
                return;
            }
            wr(40, mult);
            wr(8, cycle_last);
            wr(16, base_mono_nom);
            wr(48, 1); // res_ns
            seq_end();
        }
    }
/// INV-VDSO-DATA-1 resync arm: forward-only base step under the full seq protocol (1 Hz HPET resync); a lost writer claim returns false and the caller retries next second.
#[cfg(feature = "x86_64")]
    /// Forward-only base step (1 Hz HPET resync). Returns false when
    /// another writer held the page (retry next second).
    pub fn tsc_anchor_step_forward(step_nominal: u64) -> bool {
        if step_nominal == 0 {
            return true;
        }
        unsafe {
            if !seq_try_begin() {
                return false;
            }
            let base = rd(16);
            wr(16, base.wrapping_add(step_nominal));
            seq_end();
            true
        }
    }
/// INV-VDSO-DATA-1 rebase arm: the ONE base movement that can go backward (vCPU TSC-offset smash class); the caller prints loudly and the TSC_LAST ratchet freezes the syscall track through the pull.
#[cfg(feature = "x86_64")]
    /// The >25 s rebase pull-back (vCPU TSC-offset smash class). This
    /// is the ONE base movement that can go backward; it prints loudly
    /// at the caller and the TSC_LAST ratchet freezes the syscall track
    /// through the pull (the vDSO track steps back — a broken-by-
    /// definition state, documented, better than a 25 s-off clock).
    pub fn tsc_anchor_step_back(lead_nominal: u64) -> bool {
        unsafe {
            if !seq_try_begin() {
                return false;
            }
            let base = rd(16);
            wr(16, base.wrapping_sub(lead_nominal));
            seq_end();
            true
        }
    }
/// INV-VDSO-DATA-1 read arm: seq-validated anchor triple with no TSC read; None when not promoted (mult == 0) or the writer wedged odd (bounded retry).
#[cfg(feature = "x86_64")]
    /// Seq-validated anchor triple (no TSC read). None when not
    /// promoted (mult == 0) or the writer wedged odd.
    pub fn tsc_anchor_peek() -> Option<(u64, u64, u64)> {
        unsafe {
            for _ in 0..128 {
                let s1 = vdso_seq().load(Ordering::Acquire);
                if s1 & 1 != 0 {
                    continue;
                }
                let cl = rd(8);
                let base = rd(16);
                let mult = rd(40);
                let s2 = vdso_seq().load(Ordering::Acquire);
                if s1 != s2 {
                    continue;
                }
                if mult == 0 {
                    x86_track::bump(&x86_track::PEEK_NONE, "peek-none-mult0");
                    return None; // not promoted
                }
                return Some((cl, base, mult));
            }
            x86_track::bump(&x86_track::PEEK_NONE, "peek-none-retry");
            None
        }
    }
/// INV-VDSO-DATA-1 kernel-read arm: seq1, fields, lfence;rdtsc, seq2 with the WRAP GUARD — a TSC behind the published cycle_last retries, then reports unreadable (no manufactured 2^32-second deltas); IDENTICAL arithmetic to the emitted vDSO interpolation.
#[cfg(feature = "x86_64")]
    /// The unified-track clock read (kernel side of the vDSO contract):
    /// seq1 → fields → lfence;rdtsc → seq2, then the nominal sum.
    /// IDENTICAL arithmetic to the emitted vDSO interpolation, so a
    /// syscall read and a vDSO read at the same instant agree bit for
    /// bit. None when not promoted / writer wedged (caller falls back;
    /// the TSC_LAST ratchet keeps that fallback monotonic).
    ///
    /// WRAP GUARD (vdso-clock hunt, 2026-11-18): a TSC that reads
    /// behind the published cycle_last (vCPU TSC-offset smash — the
    /// class the poisoned release boots hit) makes `raw - cl` wrap to
    /// ~2^64, and `(wrapped * mult) >> 32` manufactures ~2^32 SECONDS
    /// of time (exactly 2^32 × mult ≈ 4.3e16 nominal ticks). The
    /// emitted vDSO code already routes unsigned-above-bound deltas to
    /// the syscall fallback; the kernel twin must refuse them too —
    /// retry, then report unreadable (the caller's ratchet freeze is
    /// the safe degradation; the resync/rebase paths repair the anchor).
    pub fn tsc_track_now_nominal() -> Option<u64> {
        unsafe {
            for _ in 0..128 {
                let s1 = vdso_seq().load(Ordering::Acquire);
                if s1 & 1 != 0 {
                    continue;
                }
                let cl = rd(8);
                let base = rd(16);
                let mult = rd(40);
                if mult == 0 {
                    return None; // not promoted
                }
                // The TSC stamp sits INSIDE the window: a refresh that
                // lands between the field loads and this read bumps the
                // seq word and the whole read retries — cycle_last can
                // never be ahead of the stamped TSC (no wrapped delta).
                let raw = read_tsc_ordered();
                let s2 = vdso_seq().load(Ordering::Acquire);
                if s1 != s2 {
                    continue;
                }
                if raw < cl {
                    // Wrapped-delta refusal (see the doc above).
                    continue;
                }
                let delta = raw - cl;
                return Some(base.wrapping_add(x86_track::tsc_delta_nominal(delta, mult)));
            }
            x86_track::bump(&x86_track::TRACK_READ_NONE, "track-none");
            None
        }
    }
/// x86_64 per-tick refresh: advance the anchor to NOW under the seq
/// word, and republish the epoch and interpolation bound. Forward-only
/// by construction (the measured TSC delta is non-negative); skipped
/// entirely while the TSC timekeeper is not promoted (mult == 0 keeps
/// userspace on the syscall fallback — the HPET/jiffies clock then
/// serves every read).
/// INV-VDSO-DATA-1 x86 tick arm: the anchor advances to NOW under the writer lock, forward-only by construction; epoch and the interpolation bound republish; skipped entirely while mult == 0 (userspace stays on the syscall fallback).
#[cfg(feature = "x86_64")]
pub fn vdso_data_tick() {
    unsafe {
        if x86_track::tsc_anchor_peek().is_none() {
            x86_track::bump_pub(&x86_track::TICK_SKIP, "tick-skip-peek");
            return; // TSC timekeeper not promoted — keep the page as-is
        }
        if !x86_track::seq_try_begin() {
            x86_track::bump_pub(&x86_track::TICK_SKIP, "tick-skip-writer");
            return; // another CPU is mid-refresh; next tick catches up
        }
        let cl = x86_track::rd(8);
        let base = x86_track::rd(16);
        let mult = x86_track::rd(40);
        // Ordered TSC stamp (vdso-clock discipline): the reader side
        // uses lfence;rdtsc so the stamp sits inside its seq window;
        // the writer stamps with the SAME ordered read so the anchor it
        // publishes is coherent with every reader's comparison (a bare
        // rdtsc can execute speculatively early relative to the field
        // loads — harmless today because raw >= cl is re-checked, but
        // the ordered read keeps the writer/reader pair symmetric).
        let raw = x86_track::read_tsc_ordered();
        // TSC must never read backward; if it ever does (vCPU TSC
        // smash), keep the old anchor — the emitted bound check sends
        // readers to the syscall and the resync report paths fire.
        if raw >= cl {
            let d = x86_track::tsc_delta_nominal(raw - cl, mult);
            x86_track::wr(16, base.wrapping_add(d));
            x86_track::wr(8, raw);
        }
        let epoch_ns = crate::timer::wall_epoch_offset_ns();
        x86_track::wr(24, epoch_ns);
        let hz = crate::drivers::hotpath::intc::apic::TSC_FREQ.load(Ordering::Acquire);
        if hz >= 1_000_000 {
            // Interpolation bound: 50 ms of raw TSC. Refreshes happen
            // every tick (~10 ms); a healthy reader interpolates at most
            // a few tens of ms past cycle_last. A larger (or wrapped)
            // delta means the anchor is stale or the TSC jumped — the
            // emitter then takes the syscall fallback.
            x86_track::wr(52, hz / 20);
        }
        x86_track::seq_end();
    }
}
/// Build the vDSO ELF code page and prime the data page. Called once
/// during boot (single-threaded) before the first execve.
/// INV-VDSO-DATA-1 init arm: boot-time single-threaded; the code page is only read after install (user mappings R+X) and the data-page prime goes through the seqlock (vdso_data_tick).
pub fn vdso_init() {
    // SAFETY: boot-time single-threaded init; the code page is only read
    // afterwards (user mappings are R+X), and the data page writes are
    // seqlock-guarded.
    unsafe {
        super::hotpath::VDSO_CODE_PAGE.0 = super::hotpath::build_code_page();
    }
    // Prime the snapshot so pre-first-tick readers see sane values.
    vdso_data_tick();
}

// ------------ pglist NODE_DATA (INV-MM-12) ------------

/// The NODE_DATA slot read behind the safe node_data accessor
/// (collected by pass 3).
///
/// # Contract (INV-MM-12)
/// INV-MM-12: node_id < MAX_NUMNODES (bounds-checked by the accessor)
/// and NODE_DATA was initialized by init_node_data() before any read;
/// the shared borrow only reads.
#[inline(always)]
pub(crate) fn node_data_ref(node_id: usize) -> Option<&'static PglistData> {
    unsafe { super::hotpath::NODE_DATA[node_id].as_ref() }
}

/// The NODE_DATA slot borrow behind the safe node_data_mut accessor
/// (collected by pass 3).
///
/// # Contract (INV-MM-12)
/// INV-MM-12 exclusive arm: node_id bounds-checked; the caller
/// guarantees exclusive access (boot/init window or the lock that
/// provides it).
#[inline(always)]
pub(crate) fn node_data_ref_mut(node_id: usize) -> Option<&'static mut PglistData> {
    unsafe { super::hotpath::NODE_DATA[node_id].as_mut() }
}

/// Get first mutable node.
///
/// # Safety
/// Caller must ensure exclusive access to the node.  On a single-node UMA
/// system this means no other thread may hold a mutable or shared reference
/// to the same `PglistData` simultaneously.  Typically this is safe only
/// during early boot or when the caller holds a lock that prevents
/// concurrent page allocation / reclaim on the same node.
/// INV-MM-1: the caller holds the node's lru_lock or runs in the boot/single-CPU window — no other thread may hold a mutable or shared reference to the same PglistData simultaneously (the contract the first_node_mut edge documents).
pub unsafe fn first_online_node_mut() -> Option<&'static mut PglistData> {
    super::hotpath::node_data_mut(0)
}

// ------------ page descriptors: mem_map / vmemmap (INV-FRAME-1) ------------

/// Copy contents of one physical page to another.
///
/// Both `src_pfn` and `dst_pfn` must be valid page frame numbers.
/// The pages must not overlap.
///
/// # Safety
/// - src_pfn and dst_pfn must be valid
/// - The source page must contain valid data
/// - The caller must ensure no concurrent writes to either page
/// INV-MM-2 copy arm: both pfns are live frames — src pinned by the migration window's refcount, dst exclusively owned by the claim; the copies run through the linear-map aliases and never overlap.
pub unsafe fn copy_page_contents(src_pfn: usize, dst_pfn: usize) {
    use crate::arch::mm::memory_layout::{phys_to_virt, PhysAddr};
    // pfn_to_phys yields a physical address; it must go through the linear
    // mapping (phys_to_virt) before dereference — the raw PA has no mapping
    // once the MMU is on.
    let src = phys_to_virt(PhysAddr(super::hotpath::pfn_to_phys(src_pfn) as u64)).0 as usize as *const u8;
    let dst = phys_to_virt(PhysAddr(super::hotpath::pfn_to_phys(dst_pfn) as u64)).0 as usize as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
}
/// Mark every PFN in the descriptor span that is NOT covered by a usable
/// RAM region as Reserved (fake-OOM family, the phantom-drift mechanism):
/// descriptors are indexed linearly over the whole PFN span, so the
/// descriptors backing MMIO holes / firmware gaps would otherwise sit at
/// power-on zero — refcount 0, no flags — and the MEMLEDGER census would
/// count them as free pages (b126: 524,804 phantom "free" pages inside
/// the 3G..4G hole alone), while the free-path tripwires would not stop
/// a forged free of them.
///
/// A page is RAM only if FULLY contained in one region (partial edge
/// pages of unaligned regions count as non-RAM, like Linux marking
/// partial e820 pages reserved).
/// INV-MM-3 boot arm: single-threaded boot init; the descriptors come from pfn_to_page's span validation and init_reserved() only flips flags/refcount (the fake-OOM phantom-drift fix).
pub fn mark_non_ram_pages(regions: &[crate::cmdline::MemoryRegion]) {
    use super::types::PAGE_SIZE;
    let start_pfn = MIN_PFN;
    let end_pfn = MIN_PFN + nr_descs();

    // Regions sorted by base (e820/FDT maps come sorted; sort again —
    // n <= 64, insertion sort is fine).
    let mut sorted: alloc::vec::Vec<(usize, usize)> =
        regions.iter().map(|r| (r.base, r.base + r.size)).collect();
    sorted.sort_unstable();

    let is_ram = |pfn: usize| -> bool {
        let pa = pfn * PAGE_SIZE;
        let pe = pa + PAGE_SIZE;
        sorted
            .iter()
            .any(|&(b, e)| pa >= b && pe <= e)
    };

    let mut marked = 0usize;
    for pfn in start_pfn..end_pfn {
        if is_ram(pfn) {
            continue;
        }
        let page = pfn_to_page(pfn);
        if !page.is_null() {
            // SAFETY: boot-time single-threaded init; descriptor in span.
            unsafe {
                (*page).init_reserved();
            }
            marked += 1;
        }
    }
    if marked > 0 {
        crate::println!(
            "page_desc: marked {} non-RAM hole pages Reserved ({} MB)",
            marked,
            marked * PAGE_SIZE / (1024 * 1024)
        );
    }
}
/// Initialize page array
///
/// Initialize page descriptors within specified range.
/// Pages backing memblock-RESERVED regions (kernel image, dtb, early
/// allocations, vmemmap storage) are marked Reserved so they can never be
/// mistaken for free memory (review 4.4: descriptors left at power-on
/// zero look like free/unmapped pages).
///
/// # Arguments
/// - `start_pfn`: Available memory start PFN
/// - `nr_pages`: Number of available pages
/// INV-MM-3 boot arm: MEM_MAP_INIT makes the init one-shot; pages initialize free before any allocator publication (single CPU), and memblock-reserved ranges re-mark Reserved + refcount 1 inside the initialized span.
pub fn init_mem_map(start_pfn: PhysFrameNr, nr_pages: usize) {
    // Prevent duplicate initialization
    if super::hotpath::MEM_MAP_INIT.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    // Use vmemmap to access page descriptors
    // Only iterate over the pages that are actually mapped in vmemmap
    let init_count = if nr_pages > MAX_PAGES { MAX_PAGES } else { nr_pages };

    // Initialize available pages as free.
    // (The first loop that marked all pages reserved has been removed because
    // init_free() fully resets every field, making the prior reserved pass
    // wasted work.)
    for i in 0..init_count {
        let pfn = start_pfn + i;
        let page = pfn_to_page(pfn);
        if !page.is_null() {
            unsafe {
                (*page).init_free();
            }
        }
    }

    // Re-mark reserved ranges (review 4.4): iterate memblock's reserved
    // list and set Reserved + refcount 1 on every covered page inside the
    // initialized span, mirroring Linux's memblock_reserve → reserve pages
    // marking during free_area_init.  Runs once during boot (single CPU).
    let span_start = start_pfn * PAGE_SIZE;
    let span_end = (start_pfn + init_count) * PAGE_SIZE;
    super::hotpath::memblock().reserved().iter().for_each(|region| {
        let res_start = region.base.max(span_start) & !(PAGE_SIZE - 1);
        let res_end = (region.base + region.size).min(span_end);
        if res_end <= res_start {
            return;
        }
        let mut addr = res_start;
        while addr < res_end {
            let page = pfn_to_page(addr / PAGE_SIZE);
            if !page.is_null() {
                unsafe {
                    (*page).init_reserved();
                }
            }
            addr += PAGE_SIZE;
        }
    });
}
/// Get page descriptor statistics
/// INV-FRAME-1 diag arm: the walk clamps to nr_descs() and the descriptors vmemmap actually mapped (vmemmap_stats().nr_pages) — dereferencing beyond would fault (the KERNPANIC-in-page_desc_stats fix); descriptors are machine-lifetime statics.
pub fn page_desc_stats() -> PageDescStats {
    // F12: iterate only the pages the RUNNING machine actually has.
    // MAX_PAGES is the compile-time 2 GB worst case; on a 1 GB QEMU the
    // vmemmap pages past the real RAM are unmapped, and the debug periodic
    // dump walked straight into them (KERNPANIC pfault from
    // Page::refcount — the diagnostic killed the box it was diagnosing).
    // Runtime descriptor count (discovered memory map; config fallback
    // before init). F12: never scan past the descriptors vmemmap actually
    // mapped — the min() below is the dereference-safety bound.
    let live_pages = nr_descs();
    let mut stats = PageDescStats {
        total_pages: live_pages,
        ..Default::default()
    };

    let base_pfn = MIN_PFN;
    // Clamp to the descriptors actually mapped by init_vmemmap: MAX_PAGES
    // is the compile-time PHYS_MEMORY_SIZE (2 GiB) while the machine may
    // boot with less RAM — descriptors past the mapped range are unmapped
    // vmemmap, and dereferencing them panics the periodic DEBUG dump
    // (observed: KERNPANIC in page_desc_stats 8s into a -m 1G boot with
    // rux:periodic (retired debug=periodic),taskdump). The vmemmap-mapped count is the direct
    // dereference-safety bound; F12's phys_memory_size estimate is kept
    // for total_pages above.
    let live_pages = live_pages.min(super::hotpath::vmemmap_stats().nr_pages);
    for i in 0..live_pages {
        let page = pfn_to_page(base_pfn + i);
        if page.is_null() {
            continue;
        }
        unsafe {
            if (*page).refcount() == 0 {
                stats.free_pages += 1;
            } else {
                stats.used_pages += 1;
            }

            if (*page).is_reserved() {
                stats.reserved_pages += 1;
            }

            if (*page).is_mapped() {
                stats.mapped_pages += 1;
            }

            if (*page).is_dirty() {
                stats.dirty_pages += 1;
            }

            if (*page).is_cow() {
                stats.cow_pages += 1;
            }

            if (*page).is_anonymous() {
                stats.anonymous_pages += 1;
            }
        }
    }

    stats
}
/// Get Page reference for a physical frame
///
/// # Safety
/// Caller must ensure pfn is in valid range
/// INV-FRAME-1: pfn is in the valid span (caller-checked); a shared &'static Page is sound because Page mutates only through atomics (interior-mutable by construction), however many holders alias the frame.
#[inline]
pub unsafe fn get_page(pfn: PhysFrameNr) -> &'static Page {
    &*pfn_to_page(pfn)
}
/// Get mutable Page reference for a physical frame
///
/// # Safety
/// Caller must ensure pfn is in valid range and there are no other references
/// INV-FRAME-1 exclusive arm: pfn valid AND no other reference exists (the caller's discipline — init/boot or exclusive-owner paths only).
#[inline]
pub unsafe fn get_page_mut(pfn: PhysFrameNr) -> &'static mut Page {
    &mut *pfn_to_page_mut(pfn)
}
/// The descriptor borrow behind the safe page_desc accessor (collected
/// by pass 3): one checked raw-pointer to &'static Page step.
///
/// # Contract (INV-FRAME-1)
/// INV-FRAME-1: p is a non-null vmemmap descriptor (the caller lifted
/// the null check into page_desc and wraps the borrow in Some);
/// descriptors are machine-lifetime statics and Page mutates only
/// through atomics, so the shared borrow is sound however many
/// holders alias the frame.
#[inline(always)]
pub(crate) fn page_desc_ref(p: *const Page) -> &'static Page {
unsafe { &*p }
}


// ------------ allocator core: BuddyAllocator / CombinedAllocator (INV-HEAP-3) ------------

// SAFETY: MetaArray is only accessed through BuddyAllocator, which holds a
// Spinlock.  All get/get_mut calls happen under that lock, preventing
// concurrent mutable access to the underlying array.
/// INV-HEAP-3: MetaArray is only accessed through BuddyAllocator, which holds a Spinlock — every get/get_mut call happens under that lock, so no concurrent mutable access to the underlying array exists.
unsafe impl Send for MetaArray {}
/// INV-HEAP-3: as the Send proof — all MetaArray access is under the BuddyAllocator lock; the lock-free shadow audit reads it only as plain integer loads.
unsafe impl Sync for MetaArray {}
impl MetaArray {
    /// Get metadata reference
    /// INV-HEAP-3: idx is bounds-checked by the assert and the reference is taken under the BuddyAllocator lock (or in the single-threaded init_sized seeder); the method's home moves, the body text is unchanged.
    pub(super) fn get(&self, idx: usize) -> &BlockMeta {
        assert!(idx < BUDDY_MAX_PAGES, "buddy allocator: metadata index {} out of range (MAX_PAGES={})", idx, BUDDY_MAX_PAGES);
        // SAFETY: idx is bounds-checked above; caller holds BuddyAllocator lock.
        unsafe { &(*self.data.get())[idx] }
    }
}
impl MetaArray {
    /// Get mutable metadata reference
    /// INV-HEAP-3 exclusive arm: idx bounds-checked; the caller holds the BuddyAllocator lock (or is the single-threaded init seeder) — the &mut is exclusive.
    pub(super) fn get_mut(&self, idx: usize) -> &mut BlockMeta {
        assert!(idx < BUDDY_MAX_PAGES, "buddy allocator: metadata index {} out of range (MAX_PAGES={})", idx, BUDDY_MAX_PAGES);
        // SAFETY: idx is bounds-checked above; caller holds BuddyAllocator lock.
        unsafe { &mut (*self.data.get())[idx] }
    }
}
// SAFETY: BuddyAllocator uses internal Spinlock for all mutable operations
// and atomics for read-only fields, making it safe to share across threads.
/// INV-HEAP-3: BuddyAllocator uses its internal Spinlock for all mutable operations and atomics for read-only fields, making it safe to move across threads.
unsafe impl Send for BuddyAllocator {}
/// INV-HEAP-3: as the Send proof — the raw-lock discipline (irqsave) serializes every mutable operation; shared access reads atomics only.
unsafe impl Sync for BuddyAllocator {}
impl BuddyAllocator {
    /// oh-crash 98th (SPINLOCK-STUCK holder hunt): map an arbitrary
    /// kernel-heap address to its containing DIRECT BUDDY allocation
    /// (the >1KB kmalloc path — Task structs, large fs/ipc objects).
    /// small_alloc::object_span names slab objects; this names the rest
    /// of the heap. Returns `(block_base, order, meta_free)` on a live
    /// allocated page, None outside the window / on a FREE block (a lock
    /// address inside a free block is itself strong evidence — the
    /// owning object was freed with the lock still referenced).
    /// Read-only over BlockMeta WITHOUT the buddy lock: fields are
    /// plain u8/u32 loads, worst case a torn value — acceptable for a
    /// forensic print (same rationale as small_alloc::object_span).
    /// INV-HEAP-3 diagnostic arm: read-only BlockMeta probe WITHOUT the buddy lock — plain u8/u32 loads, worst case a torn value; acceptable for a forensic print (same rationale as small_alloc::object_span).
    pub fn buddy_block_span(&self, addr: usize) -> Option<(usize, u32, u8)> {
        let heap_start = self.heap_start.load(Ordering::Acquire);
        let heap_end = self.heap_end.load(Ordering::Acquire);
        if heap_start == 0 || addr < heap_start || addr >= heap_end {
            return None;
        }
        let idx = (addr - heap_start) / PAGE_SIZE;
        if idx >= BUDDY_MAX_PAGES {
            return None;
        }
        // SAFETY: read-only diagnostic access to the metadata array;
        // idx is bounds-checked above. Concurrent mutation may tear a
        // field but cannot make the read unsound (plain integer loads).
        let m = unsafe { (*self.meta.data.get())[idx] };
        if m.free != 0 {
            return None; // free block: no containing allocation
        }
        let order = (m.order as u32).min(BUDDY_MAX_ORDER as u32);
        let block = idx & !((1usize << order) - 1);
        Some((heap_start + block * PAGE_SIZE, order, m.free))
    }
}
impl BuddyAllocator {
    /// Fill the INTERIOR pages' `order` bytes for a block (leader is
    /// written by the caller). Tight raw-pointer byte loop, no per-page
    /// bounds call — this replaces the per-page `get_mut` interior marking
    /// that made order-scale add/remove the ww-v2 wedge grind. The order
    /// bytes keep the R9-14 ORDER_MISMATCH arm and report fidelity for
    /// aliased interior frees; the load-bearing free bit is the bitmap.
    /// INV-HEAP-3: indices bounds-checked by the assert; the caller holds the allocator lock (or is the single-threaded init_sized seeder); only the order byte of each BlockMeta is written.
    pub(super) fn fill_interior_order(&self, page_idx: usize, order: usize) {
        if order == 0 {
            return;
        }
        let last = page_idx + (1usize << order); // exclusive
        assert!(
            last <= BUDDY_MAX_PAGES,
            "buddy fill_interior_order out of range ({}+2^{} > MAX_PAGES={})",
            page_idx,
            order,
            BUDDY_MAX_PAGES
        );
        // SAFETY: indices bounds-checked above; callers hold the allocator
        // lock (or are the single-threaded init_sized seeder); only the
        // `order` byte of each BlockMeta is written.
        unsafe {
            let base = self.meta.data.get() as *mut BlockMeta;
            let mut p = base.add(page_idx + 1);
            let end = base.add(last);
            let o = order as u8;
            while p < end {
                (*p).order = o;
                p = p.add(1);
            }
        }
    }
}
/// INV-HEAP-3: alloc/realloc/dealloc route through the irqsave buddy lock; the deferred-free interception lives at the dealloc boundary; the raw GlobalAlloc contract (null on failure, layout honored) is the trait's own.
unsafe impl GlobalAlloc for BuddyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // DEBUG memwatch: own s0, captured unconditionally at the top of the
        // body. s0 is callee-saved so the value stays valid for the whole
        // body even across helper calls; the actual frame walk runs just
        // before the note call (see below). -O0 riscv64 frames keep
        // s0 = frame TOP: [s0-8] = own saved ra, [s0-0x10] = caller's s0,
        // so walking from s0 yields [ra-into-__rust_alloc, callers...].
        let mw_s0: u64 = crate::debug::hotpath::current_frame_pointer();

        // Opportunistic reclamation of time-deferred frees (see
        // mm/deferred_free.rs): one relaxed load on the fast path; the
        // same allocation churn that fills the pool drains it.
        super::hotpath::drain_if_due();

        // Check magic number and initialization state
        if self.magic.load(Ordering::Acquire) != 0xDEADBEEF
            || self.initialized.load(Ordering::Acquire) == 0
            || self.heap_start.load(Ordering::Acquire) == 0 {
            return core::ptr::null_mut();
        }

        let size = layout.size();
        let align = layout.align();

        let order = self.size_to_order(size.max(align));

        // R75: requests larger than the biggest buddy block must FAIL, not
        // come back as an undersized BUDDY_MAX_ORDER block (see size_to_order).
        if order > BUDDY_MAX_ORDER {
            if size > 4 * 1024 * 1024 {
                crate::pr_err!(
                    "bigalloc: REJECTED size={:#x} (> max block {:#x})",
                    size,
                    PAGE_SIZE << BUDDY_MAX_ORDER
                );
            }
            return core::ptr::null_mut();
        }

        // Small-object packing layer (alloc-crash r12-16): layouts that
        // fit a <=1 KiB class are served from packed slab pages — one
        // buddy page holds dozens of objects instead of one. Falls
        // through to the plain buddy path if the layer cannot serve
        // (uninitialized heap, out of pages).
        if size <= super::hotpath::SMALL_MAX
            && align <= super::hotpath::SMALL_MAX
        {
            let p = super::hotpath::alloc(size, align);
            if !p.is_null() {
                trace_kmalloc_at(
                    mw_s0,
                    p as u64,
                    size,
                    super::hotpath::effective_size(size, align),
                );
                return p;
            }
        }

        let _guard = self.lock.lock_irqsave();
        // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
        // The GlobalAlloc main path — RawVec finish_grow lands here for
        // sizes above the small_alloc classes.
        let _aw = crate::debug::hotpath::guard(
            crate::debug::hotpath::SITE_BUDDY,
            0,
        );
        let ptr = self.alloc_blocks(order);
        if ptr.is_null() && size > 4 * 1024 * 1024 {
            crate::pr_err!("bigalloc: FAILED size={:#x}", size);
        }
        drop(_guard);
        // kmem:kmalloc — the buddy-served tail of the same event: one
        // record per allocation through the GlobalAlloc chokepoint, with
        // bytes_alloc the true serving size (the buddy block).
        if !ptr.is_null() {
            trace_kmalloc_at(mw_s0, ptr as u64, size, PAGE_SIZE << order);
        }
        if !ptr.is_null() && crate::debug::hotpath::memwatch_armed() {
            let mut mw_frames: [u64; crate::debug::types::SITE_FRAMES] =
                [0; crate::debug::types::SITE_FRAMES];
            // f[0] = own saved ra (== the ra at entry, before any helper
            // could clobber the register), f[1..] = the s0 chain above.
            // s0 is callee-saved, so the walk stays valid even here, after
            // alloc_blocks().
            unsafe {
                crate::debug::hotpath::walk_fp_chain(mw_s0, &mut mw_frames);
            }
            crate::debug::hotpath::note_alloc(
                ptr,
                self.heap_start.load(Ordering::Relaxed),
                size,
                &mw_frames,
            );
        }
        ptr
    }

    // DEBUG memwatch leak hunt: explicit realloc (instead of the default
    // trait method) so the frame-pointer chain can be walked past the whole
    // raw_vec/alloc growth machinery into the allocating kernel code. This
    // is how the GNOME-oom leak was run to ground (the "129..256-byte
    // buffer reallocated ~4.6x/s" signature was BTreeMap node growth in
    // timer add_timer_wakeup). At -O0 the chain is: this method <-
    // __rust_realloc <- realloc_nonnull <- grow_impl <- Allocator::grow <-
    // finish_grow <- grow_amortized <- do_reserve <- reserve <-
    // append_elements <- THE KERNEL CODE, so 12 frames are captured.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // DEBUG memwatch leak hunt (see the method-level comment below for the
        // chain shape). The walk is gated: disabled = zero added work on the
        // realloc hot path; enabled = 12-frame pointer chase per realloc.
        let mut mw_frames: [u64; crate::debug::types::SITE_FRAMES] =
            [0; crate::debug::types::SITE_FRAMES];
        if crate::debug::hotpath::memwatch_armed() {
            let mut fp: u64 = crate::debug::hotpath::current_frame_pointer();
            // f[0] = return address out of this method (into __rust_realloc);
            // f[1..] ascend the caller chain through the growth machinery.
            unsafe {
                crate::debug::hotpath::walk_fp_chain(fp, &mut mw_frames);
            }
            crate::debug::hotpath::note_realloc(new_size, &mw_frames);
        }

        // Default GlobalAlloc::realloc behavior.
        let new_layout = match Layout::from_size_align(new_size, layout.align()) {
            Ok(l) => l,
            Err(_) => return core::ptr::null_mut(),
        };
        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() {
            return core::ptr::null_mut();
        }
        let copy_len = layout.size().min(new_size);
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_len);
        self.dealloc(ptr, layout);
        new_ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if self.initialized.load(Ordering::Acquire) == 0 {
            return;
        }
        // kmem:kfree — one event per free, at entry, before the
        // deferred-free interception decides where the block goes (the
        // reference fires kfree at kfree() entry, whatever the backend
        // does with it afterwards). armed() first: the disabled path on
        // this, the kernel's hottest function pair, is one branch.
        trace_kfree_at(ptr as u64);

        // Teardown-race guard (mm/deferred_free.rs): frees of the
        // registered size class (the MmStruct ArcInner block) are held
        // back briefly so unaccounted &MmStruct borrowers (procfs
        // generators) preempted mid-syscall resume against coherent
        // post-teardown memory instead of heap-reused garbage (the
        // #GP-in-btree-navigate family). NOTE: this impl is the live
        // #[global_allocator] (GLOBAL_ALLOCATOR: BuddyAllocator) — the
        // interception must live HERE, not in CombinedAllocator.
        if super::hotpath::try_defer(ptr, &layout) {
            return;
        }

        self.dealloc_untracked(ptr, layout);
    }
}
/// kmem:kmalloc record from the GlobalAlloc boundary. `s0` is the
/// caller's own frame pointer captured at entry (callee-saved — the
/// same discipline memwatch uses); the call_site is the first frame
/// above the __rust_alloc plumbing. Disarmed = two relaxed loads and a
/// branch, the tracepoint contract.
/// INV-HEAP-3 trace arm: s0 is GlobalAlloc::alloc's own frame pointer (callee-saved, captured at entry); the walk reads only kernel frames and is gated by the tracepoint arming.
#[inline]
fn trace_kmalloc_at(s0: u64, ptr: u64, bytes_req: usize, bytes_alloc: usize) {
    if !crate::trace::EVENT_KMALLOC.armed() {
        return;
    }
    let mut frames = [0u64; 8];
    // SAFETY: s0 is GlobalAlloc::alloc's own fp (captured at entry,
    // callee-saved); the walk reads only kernel frames.
    unsafe {
        crate::debug::hotpath::walk_fp_chain(s0, &mut frames);
    }
    crate::trace::trace_kmalloc(
        crate::trace::alloc_call_site(&frames),
        ptr,
        bytes_req as u64,
        bytes_alloc as u64,
        0xcc0, // GFP_KERNEL — Rux's allocator takes no gfp mask
        -1,    // NUMA_NO_NODE
        false, // __GFP_ACCOUNT not set
    );
}
/// kmem:kfree record from the GlobalAlloc boundary, same origin
/// discipline as the alloc twin.
/// INV-HEAP-3 trace arm: s0 is current_frame_pointer() read in this frame, the dealloc twin of the alloc capture; same arming gate.
#[inline]
fn trace_kfree_at(ptr: u64) {
    if !crate::trace::EVENT_KFREE.armed() {
        return;
    }
    let s0 = crate::debug::hotpath::current_frame_pointer();
    let mut frames = [0u64; 8];
    // SAFETY: s0 is current_frame_pointer() read in THIS frame, the
    // dealloc twin of the alloc capture.
    unsafe {
        crate::debug::hotpath::walk_fp_chain(s0, &mut frames);
    }
    crate::trace::trace_kfree(crate::trace::alloc_call_site(&frames), ptr);
}
// SAFETY: see `AuditScratch` doc — all access is claim-serialized.
/// INV-HEAP-3 audit arm: see the AuditScratch doc — all access is claim-serialized (META_AUDIT_BUSY), so the scratch never aliases.
unsafe impl Send for AuditScratch {}
/// INV-HEAP-3 audit arm: as the Send proof — the META_AUDIT_BUSY claim admits one audit at a time across CPUs.
unsafe impl Sync for AuditScratch {}
/// The meta shadow audit's scratch-bitmap borrow (the body of
/// meta_audit_inner's scratch take, collected by pass 3).
///
/// # Contract (INV-HEAP-3)
/// The caller holds the META_AUDIT_BUSY claim (single audit at a time
/// across CPUs), so the UnsafeCell is never aliased; the borrow lives
/// only inside the audit walk (INV-HEAP-3 audit arm).
#[inline(always)]
pub(crate) fn meta_audit_scratch() -> &'static mut [u64; FREE_BITMAP_WORDS] {
unsafe { &mut (*super::hotpath::META_AUDIT_SCRATCH.data.get()) }
}

/// The real (unintercepted) free path: slab-region or buddy-heap by
/// pointer range. `CombinedAllocator::dealloc` routes here after the
/// deferred-free check declines; the deferred-free drainer calls this
/// directly so re-deferring is impossible.
/// INV-HEAP-3: the pointer routes by region — slab-resident goes to kfree, the buddy heap window to BuddyAllocator::dealloc; other regions are ignored; this is the drainer entry, so a drained entry can never be re-deferred.
pub unsafe fn dealloc_direct(ptr: *mut u8, layout: Layout) {
    if ptr.is_null() {
        return;
    }

    let ptr_addr = ptr as usize;
    let heap_start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
    let heap_end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);

    // Slab-region membership uses the TRUE bounds registered by
    // init_slab() (review 4.8) — the old `heap_end + hardcoded 4MB`
    // duplicated the layout and silently diverged whenever the slab
    // region moved or resized.
    let in_slab = super::hotpath::slab_region()
        .map(|(s, e)| ptr_addr >= s && ptr_addr < e)
        .unwrap_or(false);

    if in_slab {
        // In Slab area, use kfree
        super::hotpath::kfree(ptr);
    } else if ptr_addr >= heap_start && ptr_addr < heap_end {
        // In Buddy heap area
        HEAP_ALLOCATOR.dealloc(ptr, layout);
    }
    // Pointers in other regions are ignored
}
/// INV-HEAP-3: the front routes small 8-aligned layouts to the slab allocator (kmalloc) and everything else to the buddy; frees pass the deferred-free guard then dealloc_direct's region routing.
unsafe impl GlobalAlloc for CombinedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();

        // Opportunistic reclamation of time-deferred frees (see
        // mm/deferred_free.rs): one relaxed load on the fast path; the
        // same allocation churn that fills the pool drains it.
        super::hotpath::drain_if_due();

        // Small objects (<= 4096 bytes) try Slab allocator. The align <= 8
        // guard is load-bearing: slab objects are at least 8-aligned (16-byte
        // header + 8-multiple object sizes) but NOT 16-aligned in general, so
        // higher alignments must fall through to the buddy, which hands out
        // naturally aligned power-of-two blocks. Routing an align-16 layout
        // through the slab would hand back a misaligned pointer.
        // Without the slab, EVERY small Rust allocation (Box/Arc/String/Vec)
        // pinned a whole 4KB buddy page — a 30K-object workload exhausted the
        // 128MB heap (LTP inode02 ALLOCTHROW storm).
        if size <= 4096 && layout.align() <= 8 && super::hotpath::is_slab_initialized() {
            let ptr = super::hotpath::kmalloc(size);
            if !ptr.is_null() {
                return ptr;
            }
            // Slab allocation failed, fall back to Buddy
        }

        // Large objects or Slab failure uses Buddy allocator
        HEAP_ALLOCATOR.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }

        // Teardown-race guard (mm/deferred_free.rs): frees of the
        // registered size class (MmStruct) are held back briefly so
        // unaccounted &MmStruct borrowers (procfs generators) that were
        // preempted mid-syscall resume against coherent post-teardown
        // memory instead of heap-reused garbage (the #GP-in-btree
        // family).
        if super::hotpath::try_defer(ptr, &layout) {
            return;
        }

        dealloc_direct(ptr, layout);
    }
}
