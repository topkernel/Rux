//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The mm subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `unsafe.rs` nor `types.rs`, in one file.
//!
//! Sections follow the order of the old module declarations; each banner
//! names the topic file it was (`page_desc` was page_desc.rs) and the
//! file's module doc follows it as the section's header comment. Inside a
//! section the items keep their original order. Type definitions are in
//! `types.rs`, the audited unsafe edges in `unsafe.rs`; the unsafe that is
//! still scattered over the buddy_allocator, page_desc, pglist and vdso
//! sections is collected in pass 3.
//!
//! The file's NAME overstates until the shrink stage has run: it holds all
//! of the subsystem's code under the hot-path name, not a verified set of
//! hot-path functions (§2c.3, user decision 2026-11-25).

use super::types::{
    AT_VECTOR_SIZE,
    AddressSpace,
    AnonVma,
    AuditScratch,
    BUDDY_MAX_ORDER,
    BUDDY_MAX_PAGES,
    BlockMeta,
    BuddyAllocator,
    BuddyStats,
    CacheStats,
    CombinedAllocator,
    EffectiveStaticRegions,
    Entry,
    FREE_BITMAP_WORDS,
    FreeArea,
    GfpFlags,
    GlobalPteLock,
    HEAP_SIZE,
    KernelAddressSpaceLayout,
    KernelMemoryLayout,
    MAG_DEPTH,
    MAX_MEMBLOCK_REGIONS,
    MAX_NR_ZONES,
    MAX_ORDER,
    Magazine,
    MapError,
    MemBlock,
    MemBlockFlags,
    MemBlockRegion,
    MemBlockType,
    MemoryInfo,
    MemoryInfoFormatter,
    MemorySummary,
    MetaArray,
    MetaFindKind,
    MetaFinding,
    MetaScanOutcome,
    MmFlags,
    MmStruct,
    NR_LRU_LISTS,
    NR_WMARK,
    NUM_CACHES,
    NodeStats,
    OBJECT_SIZES,
    PAGE_SIZE,
    PCP_BATCH,
    PCP_HIGH,
    Page,
    PageDescStats,
    PageFlag,
    PageFlags,
    PageHdr,
    PageTableType,
    PageType,
    PcpPageset,
    PcpRow,
    Perm,
    PglistData,
    PhysAddr,
    PhysFrame,
    PhysFrameNr,
    PinnedUserPages,
    PteGuard,
    PteIrqGuard,
    PteLock,
    RmapStats,
    SlabAllocator,
    SlabCache,
    SlabHeader,
    SlabPages,
    SlabStats,
    SmallCache,
    TextFrameEvent,
    UnEvent,
    VdsoCodePage,
    VdsoDataPage,
    VirtAddr,
    VirtPage,
    VirtPageNr,
    Vma,
    VmaError,
    VmaFlags,
    VmaManager,
    VmaOrigin,
    VmaType,
    VmemmapStats,
    WMARK_HIGH,
    WMARK_LOW,
    WMARK_MIN,
    Zone,
    ZoneStats,
    ZoneType,
};

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::{Cell, UnsafeCell};
use core::sync::atomic::{
    AtomicBool,
    AtomicI32,
    AtomicPtr,
    AtomicU16,
    AtomicU32,
    AtomicU64,
    AtomicUsize,
    Ordering,
};
use crate::sync::spinlock::Spinlock;
use core::mem::MaybeUninit;
use alloc::collections::BTreeMap;
use crate::sync::rwlock::RwSpinlock;
use crate::sync::rwsem::Rwsem;
use alloc::vec::Vec;
use alloc::string::ToString;
use core::ptr;
// (vmemmap.rs also imported PhysAddr and VMEMMAP_START from here. `PhysAddr`
// is types.rs's in every other section, so vmemmap's one use is written
// crate::arch::mm::PhysAddr; VMEMMAP_START resolves to the page_desc
// section's `pub const VMEMMAP_START: usize = crate::arch::mm::VMEMMAP_START;`.)
use crate::arch::mm::{PageTableEntry, VMEMMAP_END, map_kernel_page, phys_to_virt};
use crate::config::MAX_CPUS;

// ════════════════ mm root ═════════════════════════════
// (was mod.rs: the module's own constants and functions; its re-exports are
// gone — every path now names hotpath or types)
// Memory Management Module

// The mm-policy hook tables (MmHooks, MmPolicyHooks) are crate::api::mm.


// pcp.rs history: an earlier per-CPU pages module was removed in wave-6
// (review 4.6 / batch-4 S3) — it had zero live call sites while carrying
// a `&'static mut` array and next_free linkage that collided with the
// OnFreelist discipline. The revisit condition ("if zone-lock contention
// shows up") is now met: the perf review of 2026-10-31 re-introduced the
// pageset as mm/pcp.rs with an OnPcp three-state frame lifecycle
// (INV-PCP-1), zone-counter parity (INV-PCP-2) and irq-off same-CPU
// exclusivity (INV-PCP-3); see that module's header for the invariants.


/// mmap mapping flags (Linux generic ABI — identical on all supported
/// architectures). riscv64 re-exports its arch module (the values are the
/// ABI); x86_64's pinned arch tree carries no such module, so the
/// constants are defined here.
#[cfg(any(feature = "riscv64", feature = "aarch64"))]
pub use crate::arch::mm::map;

#[cfg(feature = "x86_64")]
pub mod map {
    /// Shared mapping
    pub const MAP_SHARED: u32 = 0x01;
    /// Private copy-on-write mapping
    pub const MAP_PRIVATE: u32 = 0x02;
    /// Mapping type mask
    pub const MAP_TYPE_MASK: u32 = 0x0f;
    /// Fixed address mapping
    pub const MAP_FIXED: u32 = 0x10;
    /// Anonymous mapping (not file-based)
    pub const MAP_ANONYMOUS: u32 = 0x20;
    /// Stack mapping (grows down)
    pub const MAP_STACK: u32 = 0x20000;
    /// Fixed but allows relocation
    pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;
    /// Fill with huge pages
    pub const MAP_HUGETLB: u32 = 0x40000;
    /// Lock pages
    pub const MAP_LOCKED: u32 = 0x2000;
    /// No swap space reservation
    pub const MAP_NORESERVE: u32 = 0x4000;
    /// Fill (align)
    pub const MAP_POPULATE: u32 = 0x8000;
    /// No core dump
    pub const MAP_NODUMP: u32 = 0x10000;
    /// x86_64-only: place the mapping inside the first 2GB (placement
    /// hint honored by the bottom-up 32-bit area search; riscv64 has no
    /// such flag in the Linux UAPI and keeps rejecting bit 0x40).
    pub const MAP_32BIT: u32 = 0x40;
}

/// Allocate fresh physical memory and map it into a user page table.
///
/// Returns the physical address of the FIRST page (callers that write the
/// whole range must walk the page tables — chunks are not contiguous).
/// riscv64 delegates to the arch implementation (chunked buddy blocks);
/// x86_64 walks page-at-a-time through the arch interface until the
/// backend grows its own chunked allocator.
///
/// Contract: `user_ppn` must be a live user page-table root;
/// `virt_addr` must be page-aligned and unmapped in it (the arch
/// primitives are safe fns; the old unsafe wrappers were vestigial).
pub fn alloc_and_map_user_table(
    user_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    #[cfg(any(feature = "riscv64", feature = "aarch64"))]
    {
        crate::arch::mm::alloc_and_map_to_user_table(user_ppn, virt_addr, size, flags)
    }
    #[cfg(feature = "x86_64")]
    {
        crate::arch::mm::alloc_and_map_to_user_table(user_ppn, virt_addr, size, flags)
    }
}

/// Same contract as [`alloc_and_map_user_table`], for anonymous heap
/// growth (sys_brk).
///
/// Contract: same as [`alloc_and_map_user_table`].
pub fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    #[cfg(any(feature = "riscv64", feature = "aarch64"))]
    {
        crate::arch::mm::alloc_and_map_user_memory(user_root_ppn, virt_addr, size, flags)
    }
    #[cfg(feature = "x86_64")]
    {
        crate::arch::mm::alloc_and_map_user_memory(user_root_ppn, virt_addr, size, flags)
    }
}

/// Activate a page-table root on the current CPU.
///
/// Bridges the per-arch context-switch primitives: riscv64 drives
/// `context::switch_mm(ppn, asid)` (satp + ASID-scoped sfence); x86_64
/// builds the tagged CR3 (PCID + NOFLUSH when PCID is armed — asid 0
/// callers are kernel-root switches; user roots must carry a resolved
/// PCID, see exec.rs); aarch64 decomposes the composed satp into
/// TTBR0_EL1 (root PPA + ASID in [63:48]) with a full flush.
///
/// Contract (INV-MM-10): `root_ppn` must be a live page-table root and
/// the kernel half must be mapped in the target root — the unsafe
/// arch-switch primitives live behind `edges::switch_address_space`.
#[inline]
pub fn switch_address_space(root_ppn: u64, asid: u16) {
    super::r#unsafe::switch_address_space(root_ppn, asid);
}

/// Invalidate one user page mapping in the local TLB (no cross-CPU
/// shootdown — callers issue that separately via arch::ipi when needed).
///
/// Bridges the per-arch `flush_tlb_page` signatures: riscv64 takes
/// `(vaddr, asid)` (asid 0 = all address spaces), x86_64 takes the
/// address alone (PCID-less invlpg), aarch64 takes `(vaddr, 0)` — the
/// asid-0 convention maps to `tlbi vaae1is` (VA across ALL ASIDs).
#[inline]
pub fn flush_tlb_page_local(addr: u64) {
    #[cfg(feature = "riscv64")]
    crate::arch::mm::flush_tlb_page(addr as usize, 0);
    #[cfg(feature = "x86_64")]
    crate::arch::mm::flush_tlb_page(addr);
    #[cfg(feature = "aarch64")]
    crate::arch::mm::flush_tlb_page(addr as usize, 0);
}

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
// This allows runtime configuration instead of hardcoding
pub const PHYS_MEMORY_SIZE: usize = crate::config::PHYS_MEMORY_SIZE;

pub const KERNEL_VIRT_BASE: usize = 0xffff_0000_0000_0000;

pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7fff_ffff;

// hugepage: flipped to the mm crate (M1b) — its items no longer
// re-export at the mm root; framework consumers reach them through
// the crate's crate::hugepage (zero such consumers exist today).


// ════════════════ buddy_allocator ═════════════════════
// (was buddy_allocator.rs)
// Buddy System Memory Allocator
//
// Improved version: Metadata and user data are stored separately to prevent BlockHeader corruption
//
// # Safety Invariants — Kernel Heap Buddy Free List
//
// This is the `#[global_allocator]` buddy for kernel heap allocations.
// The same INV-BUDDY-1 through INV-BUDDY-5 invariants from `zone.rs` apply,
// with `BlockMeta.free == 1` replacing `_refcount == 0` as the free indicator
// and `BlockMeta.order` as the per-page order field.
//
// Additionally:
//
// - **INV-HEAP-1**: `BlockMeta.prev`/`next` are valid page indices (< BUDDY_MAX_PAGES)
//   or `u32::MAX` (null sentinel). No truncation occurs — fields are `u32`.
//
// - **INV-HEAP-2**: `free_lists[order]` is either `EMPTY_LIST` or a valid
//   page index within `[0, BUDDY_MAX_PAGES)`.
//
// - **INV-HEAP-3** (session 5, wedge fix): the per-page FREE bit lives in
//   `free_bits`, a word-strided bitmap; `BlockMeta.free` is authoritative
//   for LIST LEADERS only (interior pages' byte stays stale). The bitmap
//   is the exact per-page free state and is what the R9-14 double-free
//   check consults. Interior pages' `order` bytes are maintained by a
//   tight raw-pointer fill (report fidelity for the ORDER_MISMATCH arm);
//   they carry the order of the block they currently belong to.
//
//   Why: the old mark-every-page loops ran `meta.get_mut()` (bounds
//   assert + UnsafeCell call) per page inside `add_to_free_list` /
//   `remove_from_free_list` / `init_block`, all under the single
//   irqsave heap lock. On a 1 GiB heap (2^18 pages, seeded as one
//   order-18 block) an order-scale add/remove walked 262144 heavy
//   iterations per level; gdb on the ww-v2 wedge caught cpu1 grinding
//   exactly that loop under tcp_timer softirq while cpu0 spun on the
//   same lock in the alloc path (terminal 15+ min system freeze,
//   session-4 record). The bitmap fill is 4096 word ops for the same
//   block; the order fill is a call-free byte loop.

// BUDDY_MAX_ORDER, HEAP_SIZE, BUDDY_MAX_PAGES, FREE_BITMAP_WORDS, BlockMeta, MetaArray and
// BuddyAllocator — the definitions live in mm/types.rs; every impl and
// function that uses them stays in this file. Re-exported so the
// existing `mm::buddy_allocator::…` paths are unchanged.

const MIN_ORDER: usize = 0;

// Hardcoded heap start address for early boot.
// Uses phys_to_virt() to get the virtual address in the linear mapping region.
// riscv64: PAGE_OFFSET 0xffffffd600000000 over PHYS base 0x80000000.
// x86_64: linear map at 0xffff888000000000 over phys 0; heap placed at 1GB
// (clear of the image window 0x200000..0xa00000 and the q35 ECAM hole).
#[cfg(feature = "riscv64")]
// RUX-RISCV-LAYOUT-8 (cov campaign): moved with the window end again,
// 0x81e0_0000 -> 0x8220_0000 (see main.rs KERNEL_RESERVE_SIZE note).
const KERNEL_HEAP_PHYS: usize = 0x8220_0000;
#[cfg(feature = "riscv64")]
const VA_PA_OFFSET: usize = 0xffffffd600000000 - 0x80000000;
#[cfg(feature = "x86_64")]
const KERNEL_HEAP_PHYS: usize = 0x4000_0000;
#[cfg(feature = "x86_64")]
const VA_PA_OFFSET: usize = 0xffff_8880_0000_0000;
// aarch64: heap follows the 32MB kernel window (see memory_layout.rs)
#[cfg(feature = "aarch64")]
const KERNEL_HEAP_PHYS: usize = 0x4280_0000;
#[cfg(feature = "aarch64")]
// The aarch64 linear map places phys P at PAGE_OFFSET + P (identity from 0),
// unlike riscv64 which mounts phys RAM_BASE at PAGE_OFFSET.
const VA_PA_OFFSET: usize = 0xffff_c000_0000_0000;
const HEAP_START: usize = KERNEL_HEAP_PHYS + VA_PA_OFFSET;

// Heap size - read from configuration file
/// Empty list marker (use out-of-range value)
const EMPTY_LIST: usize = BUDDY_MAX_PAGES + 1;

// ---------------------------------------------------------------------------
// alloc-crash r12-16 forensics: exact page-flow accounting for the
// "heap 100% full while the live-site ledger is nearly empty" contradiction
// (a2: 1015808K window, 68K free; a3: 0K free, tracked-live ~13MB).
//
// NET_PAGES[order] counts pages HANDED OUT minus pages RETURNED at each
// order (unconditional relaxed atomics — correctness-neutral). The sum
// over orders is the allocator's own "used" figure; comparing it with the
// free-list census at the ALLOCTHROW moment tells whether the pages are
// held by live allocations or vanished from both the free lists and the
// ledger (orphaning via the free-refuse paths below).
//
// FREE_REFUSED counts frees dropped on the floor by the guard rails —
// every refusal permanently orphans that block's pages (the block never
// returns to a free list). A refused-free count that grows with uptime IS
// the leak when the live table is clean.
// ---------------------------------------------------------------------------
pub static NET_PAGES: [AtomicUsize; BUDDY_MAX_ORDER + 1] =
    [const { AtomicUsize::new(0) }; BUDDY_MAX_ORDER + 1];

/// Free-refusal breakdown, indexed by FreeRefuse kind.
pub static FREE_REFUSED: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

/// Free-refusal kinds (index into FREE_REFUSED).
pub const FR_OUT_OF_WINDOW: usize = 0;
pub const FR_DOUBLE_FREE: usize = 1;
pub const FR_ORDER_MISMATCH: usize = 2;
pub const FR_TASK_LIVE: usize = 3;
pub const FR_ORDER_TOO_BIG: usize = 4;
pub const FR_UNALIGNED: usize = 5;

/// Last N refused frees: (addr, incoming order, meta_order, meta_free).
pub static REFUSED_RING: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static REFUSED_RING_POS: AtomicUsize = AtomicUsize::new(0);

fn refused_free_note(kind: usize, addr: usize, order: usize, meta_order: u8, meta_free: u8) {
    FREE_REFUSED[kind.min(7)].fetch_add(1, Ordering::Relaxed);
    let pos = REFUSED_RING_POS.fetch_add(1, Ordering::Relaxed) % 16;
    let slot = pos * 4;
    REFUSED_RING[slot].store(addr, Ordering::Relaxed);
    REFUSED_RING[slot + 1].store(order, Ordering::Relaxed);
    REFUSED_RING[slot + 2].store(meta_order as usize, Ordering::Relaxed);
    REFUSED_RING[slot + 3].store(meta_free as usize, Ordering::Relaxed);
}


impl BlockMeta {
    const fn new() -> Self {
        Self {
            order: 0,
            free: 0,
            prev: 0,
            next: 0,
        }
    }
}

// SAFETY: MetaArray is only accessed through BuddyAllocator, which holds a
// Spinlock.  All get/get_mut calls happen under that lock, preventing
// concurrent mutable access to the underlying array.
unsafe impl Send for MetaArray {}
unsafe impl Sync for MetaArray {}

impl MetaArray {
    const fn new() -> Self {
        Self {
            data: UnsafeCell::new([const { BlockMeta::new() }; BUDDY_MAX_PAGES]),
        }
    }

    /// Get metadata reference
    fn get(&self, idx: usize) -> &BlockMeta {
        assert!(idx < BUDDY_MAX_PAGES, "buddy allocator: metadata index {} out of range (MAX_PAGES={})", idx, BUDDY_MAX_PAGES);
        // SAFETY: idx is bounds-checked above; caller holds BuddyAllocator lock.
        unsafe { &(*self.data.get())[idx] }
    }

    /// Get mutable metadata reference
    fn get_mut(&self, idx: usize) -> &mut BlockMeta {
        assert!(idx < BUDDY_MAX_PAGES, "buddy allocator: metadata index {} out of range (MAX_PAGES={})", idx, BUDDY_MAX_PAGES);
        // SAFETY: idx is bounds-checked above; caller holds BuddyAllocator lock.
        unsafe { &mut (*self.data.get())[idx] }
    }
}

// SAFETY: BuddyAllocator uses internal Spinlock for all mutable operations
// and atomics for read-only fields, making it safe to share across threads.
unsafe impl Send for BuddyAllocator {}
unsafe impl Sync for BuddyAllocator {}

impl BuddyAllocator {
    /// R13-6: current heap virtual range (for allocation-validity checks).
    pub fn heap_bounds(&self) -> (usize, usize) {
        (self.heap_start.load(Ordering::Acquire), self.heap_end.load(Ordering::Acquire))
    }

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

    pub const fn new() -> Self {
        Self {
            magic: AtomicUsize::new(0xDEADBEEF),
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            free_lists: [const { AtomicUsize::new(0) }; BUDDY_MAX_ORDER + 1],
            initialized: AtomicUsize::new(0),
            meta: MetaArray::new(),
            free_bits: [const { AtomicU64::new(0) }; FREE_BITMAP_WORDS],
            lock: Spinlock::new(()),
        }
    }

    /// Check if magic number is valid
    fn check_magic(&self) -> bool {
        self.magic.load(Ordering::Acquire) == 0xDEADBEEF
    }

    /// Initialize the allocator
    ///
    /// Uses hardcoded addresses for early boot compatibility.
    /// The actual layout is tracked in mm::layout module for other components.
    pub fn init(&self) {
        self.init_sized(HEAP_SIZE);
    }

    /// Initialize the allocator with a heap window CLAMPED to the
    /// discovered memory map (the -m 512M panic family: `HEAP_SIZE` is the
    /// config NOMINAL size; on machines with less RAM the window must
    /// shrink or the buddy hands out addresses past the last DRAM page).
    ///
    /// Unlike the old single-block seeding (`heap_size_to_order` rounds UP
    /// to a power of two, poking the last block past the window), the
    /// window is seeded with the largest whole blocks that fit — the same
    /// discipline as zone init. `size` is capped at `HEAP_SIZE` (the
    /// metadata array bound).
    pub fn init_sized(&self, size: usize) {
        if self.initialized.load(Ordering::Acquire) != 0 {
            return;
        }

        if self.initialized.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let size = size.min(HEAP_SIZE);
            // Set magic number
            self.magic.store(0xDEADBEEF, Ordering::Release);
            self.heap_start.store(HEAP_START, Ordering::Release);
            self.heap_end.store(HEAP_START + size, Ordering::Release);

            // Initialize all free lists to empty
            for i in 0..=BUDDY_MAX_ORDER {
                self.free_lists[i].store(EMPTY_LIST, Ordering::Release);
            }

            if size == 0 {
                return; // no heap window on this machine — allocations fail
            }

            // Seed: repeatedly add the largest order that is index-aligned
            // and fits in the remaining pages. Page 0 == HEAP_START.
            let mut remaining = size / PAGE_SIZE;
            let mut idx = 0usize;
            while remaining > 0 {
                let mut order = 0usize;
                for o in (0..=BUDDY_MAX_ORDER).rev() {
                    let block = 1usize << o;
                    if idx % block == 0 && remaining >= block {
                        order = o;
                        break;
                    }
                }
                self.init_block(idx, order, false);
                self.add_to_free_list(idx, order);
                idx += 1usize << order;
                remaining -= 1usize << order;
            }
        }
    }

    /// Read the per-page free bit (INV-HEAP-3). Exact for every page of
    /// the heap window. Callers either hold the allocator lock or tolerate
    /// a torn watermark (same discipline as the lock-free census).
    fn page_is_free(&self, page_idx: usize) -> bool {
        debug_assert!(page_idx < BUDDY_MAX_PAGES);
        (self.free_bits[page_idx / 64].load(Ordering::Relaxed) >> (page_idx % 64)) & 1 == 1
    }

    /// Set/clear the free bits of `npages` pages starting at `page_idx`
    /// (INV-HEAP-3). Word-strided masked RMW: whole words take the
    /// full-mask fast path, at most two partial edge words per call.
    /// Order-18 (whole-heap) fill = 4096 word ops.
    fn fill_free_bits(&self, page_idx: usize, npages: usize, free: bool) {
        if npages == 0 {
            return;
        }
        assert!(
            page_idx + npages <= BUDDY_MAX_PAGES,
            "buddy fill_free_bits out of range ({}+{} > MAX_PAGES={})",
            page_idx,
            npages,
            BUDDY_MAX_PAGES
        );
        let last = page_idx + npages; // exclusive
        let mut w = page_idx / 64;
        let end_word = (last + 63) / 64;
        while w < end_word {
            let base = w * 64;
            let lo = if page_idx > base { page_idx - base } else { 0 };
            let hi = (last - base).min(64);
            let mask: u64 = if hi - lo == 64 {
                !0u64
            } else {
                ((1u64 << (hi - lo)) - 1) << lo
            };
            if free {
                self.free_bits[w].fetch_or(mask, Ordering::Relaxed);
            } else {
                self.free_bits[w].fetch_and(!mask, Ordering::Relaxed);
            }
            w += 1;
        }
    }

    /// Fill the INTERIOR pages' `order` bytes for a block (leader is
    /// written by the caller). Tight raw-pointer byte loop, no per-page
    /// bounds call — this replaces the per-page `get_mut` interior marking
    /// that made order-scale add/remove the ww-v2 wedge grind. The order
    /// bytes keep the R9-14 ORDER_MISMATCH arm and report fidelity for
    /// aliased interior frees; the load-bearing free bit is the bitmap.
    fn fill_interior_order(&self, page_idx: usize, order: usize) {
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

    /// Initialize block metadata
    fn init_block(&self, page_idx: usize, order: usize, free: bool) {
        let meta = self.meta.get_mut(page_idx);
        meta.order = order as u8;
        meta.free = if free { 1 } else { 0 };
        meta.prev = 0;
        meta.next = 0;
        // R17-A2: keep EVERY page of the block consistent — the split path
        // calls init_block(.., false) on the retained half; stale interior
        // marks let the R9-14 tripwire swallow legitimate frees (leak →
        // alloc failures under fork pressure: create/P2b/rename flakiness).
        // R17-A3 (session 5): the marking itself is now the bitmap + tight
        // order fill (see INV-HEAP-3) — same coverage, 2^order/64 words
        // plus a call-free byte loop instead of 2^order `get_mut` calls.
        self.fill_interior_order(page_idx, order);
        self.fill_free_bits(page_idx, 1usize << order, free);
    }

    /// Add block to free list
    fn add_to_free_list(&self, page_idx: usize, order: usize) {
        // Boundary check
        if order > BUDDY_MAX_ORDER {
            return;  // Cannot handle blocks exceeding maximum order
        }

        {
            let meta = self.meta.get_mut(page_idx);
            meta.order = order as u8;
            meta.free = 1;
        }
        // R17-A: mark EVERY page of the block free — interior pages kept
        // stale free=0 after merges/splits, letting second frees pass
        // silently (heap analog of the zone OnFreelist bug). R17-A3: the
        // mark is the word-strided bitmap + tight order fill (INV-HEAP-3),
        // not the per-page `get_mut` loop that ground order-scale adds.
        self.fill_interior_order(page_idx, order);
        self.fill_free_bits(page_idx, 1usize << order, true);

        // Get current free list head
        let list_head = self.free_lists[order].load(Ordering::Acquire);

        // Insert block at list head
        if list_head != EMPTY_LIST && list_head < BUDDY_MAX_PAGES {
            self.meta.get_mut(list_head).prev = page_idx as u32;
        }
        {
            let meta = self.meta.get_mut(page_idx);
            meta.next = if list_head == EMPTY_LIST { u32::MAX } else { list_head as u32 };
            meta.prev = u32::MAX;  // u32::MAX means null
        }

        // Update list head
        self.free_lists[order].store(page_idx, Ordering::Release);
    }

    /// Remove block from free list
    fn remove_from_free_list(&self, page_idx: usize, order: usize) {
        // Boundary check
        if order > BUDDY_MAX_ORDER {
            return;  // Cannot handle blocks exceeding maximum order
        }

        // R17-A: clear the free mark on EVERY page of the block
        // (R17-A3: via the bitmap — INV-HEAP-3).
        {
            let meta = self.meta.get_mut(page_idx);
            meta.free = 0;
        }
        self.fill_free_bits(page_idx, 1usize << order, false);

        let prev_idx = self.meta.get(page_idx).prev as usize;
        let next_idx = self.meta.get(page_idx).next as usize;

        if prev_idx != u32::MAX as usize && prev_idx < BUDDY_MAX_PAGES {
            self.meta.get_mut(prev_idx).next = next_idx as u32;
        } else {
            // This is the list head, update global list head
            let new_head = if next_idx == u32::MAX as usize { EMPTY_LIST } else { next_idx };
            self.free_lists[order].store(new_head, Ordering::Release);
        }

        if next_idx != u32::MAX as usize && next_idx < BUDDY_MAX_PAGES {
            self.meta.get_mut(next_idx).prev = prev_idx as u32;
        }

        self.meta.get_mut(page_idx).free = 0;
    }

    /// Calculate order for heap size (O(1) bit manipulation)
    fn heap_size_to_order(&self, size: usize) -> usize {
        if size <= PAGE_SIZE {
            return 0;
        }
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let order = (usize::BITS - (pages - 1).leading_zeros()) as usize;
        if order > BUDDY_MAX_ORDER { BUDDY_MAX_ORDER } else { order }
    }

    /// Convert size to order (O(1) bit manipulation)
    ///
    /// R75 (GNOME scribbles): the old clamp `order > BUDDY_MAX_ORDER → BUDDY_MAX_ORDER`
    /// made every allocation larger than the largest buddy block
    /// (PAGE_SIZE << BUDDY_MAX_ORDER, 16 MB) come back as a BUDDY_MAX_ORDER block — a
    /// 64 MB Vec growth received 16 MB and then overwrote the following
    /// 48 MB of kernel heap (the R57 pick-state garbage / R46 phantoms /
    /// R74 slotless-TP / PREEMPT-UNDERFLOW family all match that
    /// fingerprint). Callers must never receive a block smaller than the
    /// requested layout: `alloc` rejects oversized requests with null
    /// (the GlobalAlloc failure contract) so the damage stays a loud,
    /// scoped ENOMEM kill instead of silent corruption.
    fn size_to_order(&self, size: usize) -> usize {
        if size <= PAGE_SIZE {
            return 0;
        }
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        (usize::BITS - (pages - 1).leading_zeros()) as usize
    }

    /// Get buddy page index for a block
    fn get_buddy_idx(&self, page_idx: usize, order: usize) -> usize {
        let block_size_pages = 1usize << order;  // Number of pages in block
        page_idx ^ block_size_pages
    }

    /// Convert page index to address (uses stored heap_start)
    fn page_idx_to_addr(&self, page_idx: usize) -> usize {
        let heap_start = self.heap_start.load(Ordering::Acquire);
        heap_start + page_idx * PAGE_SIZE
    }

    /// Convert address to page index (uses stored heap_start)
    /// Returns None if the address is outside the heap range
    fn addr_to_page_idx_checked(&self, addr: usize) -> Option<usize> {
        let heap_start = self.heap_start.load(Ordering::Acquire);
        let heap_end = self.heap_end.load(Ordering::Acquire);
        if addr < heap_start || addr >= heap_end {
            return None;
        }
        let idx = (addr - heap_start) / PAGE_SIZE;
        if idx >= BUDDY_MAX_PAGES {
            return None;
        }
        Some(idx)
    }

    /// Allocate memory
    fn alloc_blocks(&self, order: usize) -> *mut u8 {
        // Search starting from specified order
        for mut current_order in order..=BUDDY_MAX_ORDER {
            let list_head = self.free_lists[current_order].load(Ordering::Acquire);

            if list_head != EMPTY_LIST && list_head < BUDDY_MAX_PAGES {
                // Found free block
                self.remove_from_free_list(list_head, current_order);

                // Split block if needed
                let mut page_idx = list_head;
                while current_order > order {
                    let block_size_pages = 1usize << current_order;
                    let buddy_idx = page_idx + (block_size_pages / 2);

                    // Safety check: buddy must be within valid range
                    if buddy_idx >= BUDDY_MAX_PAGES {
                        break;
                    }

                    // Publish the buddy half at the smaller order.
                    // add_to_free_list performs the FULL marking (leader
                    // meta, interior order bytes, free bitmap) — the old
                    // init_block(.., true) in front of it repeated that
                    // marking byte-for-byte, and the retained half's
                    // init_block(.., false) re-marked 2^(co-1) pages that
                    // the NEXT iteration's add_to_free_list (split-off
                    // suffixes) and the loop's final init_block (kept
                    // prefix) fully rewrote anyway. The split loop runs
                    // under the heap irqsave lock, so the transient stale
                    // larger-order marks on the retained half are
                    // unobservable (session-5 follow-up: both dead-write
                    // fills folded out).
                    self.add_to_free_list(buddy_idx, current_order - 1);
                    current_order -= 1;
                }

                // Ensure final block is marked as allocated
                self.init_block(page_idx, order, false);
                NET_PAGES[order].fetch_add(1usize << order, Ordering::Relaxed);

                let addr = self.page_idx_to_addr(page_idx);
                // R18-1 (alloc-side handoff probe): if this page is already
                // marked as a LIVE Task page and this allocation is NOT the
                // Task allocator itself, the page just received its SECOND
                // owner — the long-hunted first handoff, named at the
                // moment it happens. ra identifies the taker.
                // R34/R43: never print here (BUDDY lock held; console would
                // nest UART under BUDDY — the A-form wedge ingredient).
                // R69: the ownership bitmap is actually populated now
                // (task_page_mark from alloc_task_slot — before R69 the
                // probe read a table nobody wrote and never fired), so the
                // event is also recorded with a caller backtrace for the
                // deferred audit printer in rux:scribble (retired debug=scribble) mode.
                if !crate::sched::hotpath::IN_TASK_ALLOC.load(core::sync::atomic::Ordering::Acquire)
                    && crate::sched::hotpath::task_page_is_owned(addr as *const u8)
                {
                    // R69: also record the event with a caller backtrace
                    // for the deferred audit printer (rux:scribble (retired debug=scribble),
                    // x86_64 — the debug scribble module is x86-only).
                    #[cfg(feature = "x86_64")]
                    if crate::trace::EVENT_SCRIBBLE.armed() {
                        let frames = crate::debug::scribble::audit_frames();
                        crate::debug::scribble::audit_record(
                            crate::debug::scribble::AUDIT_KIND_HANDOFF,
                            addr as u64,
                            order as u32,
                            &frames,
                        );
                    }
                    crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE); // R18-1 double-handoff
                }
                return addr as *mut u8;
            }
        }

        // Not enough memory
        core::ptr::null_mut()
    }

    /// Free memory (the merge loop). The body is safe: metadata access
    /// goes through the bounds-checked MetaArray::get and the free-list
    /// ops are safe methods — this was signed only by declaration
    /// granularity.
    fn free_blocks(&self, ptr: *mut u8, order: usize) {
        let addr = ptr as usize;
        let mut page_idx = match self.addr_to_page_idx_checked(addr) {
            Some(idx) => idx,
            None => return,
        };
        // R9-14: double-free tripwire. The buddy previously accepted any
        // free unconditionally — one double free anywhere put a LIVE page
        // on a free list and the next two allocations shared it (the
        // "linked Task page wiped by a zeroing allocation" corruption).
        // Refuse the free and report instead of corrupting the heap.
        // R17-A3: the free arm consults the per-page bitmap (exact for
        // aliased interior pages too), the order arm the `order` byte
        // (leaders exact, interiors filled by fill_interior_order).
        {
            let meta = self.meta.get(page_idx);
            if self.page_is_free(page_idx) || meta.order != order as u8 {
                // R43: taint-only (SBI ecall under the heap lock was an
                // A-form wedge ingredient — see the R18-1 note above).
                // R69: also record the event with a caller backtrace for
                // the deferred audit printer (rux:scribble (retired debug=scribble), x86_64).
                #[cfg(feature = "x86_64")]
                if crate::trace::EVENT_SCRIBBLE.armed() {
                    let frames = crate::debug::scribble::audit_frames();
                    crate::debug::scribble::audit_record(
                        crate::debug::scribble::AUDIT_KIND_DOUBLE_FREE,
                        addr as u64,
                        order as u32,
                        &frames,
                    );
                }
                crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE); // R9-14 double-free/hoard
                // R17-A3: kind follows the bitmap (the authoritative free
                // state), not the leader-only `free` byte.
                let kind = if self.page_is_free(page_idx) { FR_DOUBLE_FREE } else { FR_ORDER_MISMATCH };
                refused_free_note(kind, addr, order, meta.order, meta.free);
                return;
            }
        }
        let mut current_order = order;
        NET_PAGES[order].fetch_sub(1usize << order, Ordering::Relaxed);

        loop {
            // Merge ceiling (page-buddy parity): stop coalescing AT the
            // maximum order. The old `> BUDDY_MAX_ORDER` guard let two
            // BUDDY_MAX_ORDER buddies merge first and only then registered the
            // oversized block at BUDDY_MAX_ORDER — the buddy half's free bits
            // were cleared by remove_from_free_list but its pages were on
            // no list: 2^BUDDY_MAX_ORDER pages orphaned per event, invisible to
            // both the allocator and the census. Unreachable with the
            // shipped config (BUDDY_MAX_ORDER=20 covers the whole heap),
            // but latent for any smaller max order.
            if current_order >= BUDDY_MAX_ORDER {
                // At the ceiling already: add directly to BUDDY_MAX_ORDER list.
                self.add_to_free_list(page_idx, BUDDY_MAX_ORDER);
                break;
            }

            let buddy_idx = self.get_buddy_idx(page_idx, current_order);

            // Check if buddy is in valid range
            if buddy_idx >= BUDDY_MAX_PAGES {
                // Buddy out of range, cannot merge
                self.add_to_free_list(page_idx, current_order);
                break;
            }

            // Check if buddy is free and size matches
            let buddy_meta = self.meta.get(buddy_idx);
            if buddy_meta.free == 0 || buddy_meta.order != current_order as u8 {
                // Buddy not free or size doesn't match, cannot merge
                self.add_to_free_list(page_idx, current_order);
                break;
            }

            // Buddy is free, remove from list
            self.remove_from_free_list(buddy_idx, current_order);

            // Merge: select smaller index as base address
            if page_idx > buddy_idx {
                page_idx = buddy_idx;
            }

            current_order += 1;
        }
    }
}

unsafe impl GlobalAlloc for BuddyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // DEBUG memwatch: own s0, captured unconditionally at the top of the
        // body. s0 is callee-saved so the value stays valid for the whole
        // body even across helper calls; the actual frame walk runs just
        // before the note call (see below). -O0 riscv64 frames keep
        // s0 = frame TOP: [s0-8] = own saved ra, [s0-0x10] = caller's s0,
        // so walking from s0 yields [ra-into-__rust_alloc, callers...].
        let mw_s0: u64 = crate::debug::backtrace::current_frame_pointer();

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
        let _aw = crate::debug::allocwatch::guard(
            crate::debug::allocwatch::SITE_BUDDY,
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
        if !ptr.is_null() && crate::debug::memwatch::armed() {
            let mut mw_frames: [u64; crate::debug::memwatch::SITE_FRAMES] =
                [0; crate::debug::memwatch::SITE_FRAMES];
            // f[0] = own saved ra (== the ra at entry, before any helper
            // could clobber the register), f[1..] = the s0 chain above.
            // s0 is callee-saved, so the walk stays valid even here, after
            // alloc_blocks().
            unsafe {
                crate::debug::memwatch::walk_fp_chain(mw_s0, &mut mw_frames);
            }
            crate::debug::memwatch::note_alloc(
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
        let mut mw_frames: [u64; crate::debug::memwatch::SITE_FRAMES] =
            [0; crate::debug::memwatch::SITE_FRAMES];
        if crate::debug::memwatch::armed() {
            let mut fp: u64 = crate::debug::backtrace::current_frame_pointer();
            // f[0] = return address out of this method (into __rust_realloc);
            // f[1..] ascend the caller chain through the growth machinery.
            unsafe {
                crate::debug::memwatch::walk_fp_chain(fp, &mut mw_frames);
            }
            crate::debug::memwatch::note_realloc(new_size, &mw_frames);
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
#[inline]
fn trace_kmalloc_at(s0: u64, ptr: u64, bytes_req: usize, bytes_alloc: usize) {
    if !crate::trace::EVENT_KMALLOC.armed() {
        return;
    }
    let mut frames = [0u64; 8];
    // SAFETY: s0 is GlobalAlloc::alloc's own fp (captured at entry,
    // callee-saved); the walk reads only kernel frames.
    unsafe {
        crate::debug::memwatch::walk_fp_chain(s0, &mut frames);
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
#[inline]
fn trace_kfree_at(ptr: u64) {
    if !crate::trace::EVENT_KFREE.armed() {
        return;
    }
    let s0 = crate::debug::backtrace::current_frame_pointer();
    let mut frames = [0u64; 8];
    // SAFETY: s0 is current_frame_pointer() read in THIS frame, the
    // dealloc twin of the alloc capture.
    unsafe {
        crate::debug::memwatch::walk_fp_chain(s0, &mut frames);
    }
    crate::trace::trace_kfree(crate::trace::alloc_call_site(&frames), ptr);
}

/// Free through the live global allocator, bypassing the deferred-free
/// interception (drainer entry point, mm/deferred_free.rs).
pub fn dealloc_untracked_pub(ptr: *mut u8, layout: Layout) {
    GLOBAL_ALLOCATOR.dealloc_untracked(ptr, layout);
}

impl BuddyAllocator {
    /// Raw order-0 page for the small-object packing layer (mm/
    /// small_alloc.rs): same accounting and locking as the GlobalAlloc
    /// path (NET_PAGES, tripwires) but no memwatch note and no
    /// size-to-order walk. Callers must NOT hold the buddy lock; the
    /// small-alloc class lock MAY be held (class -> buddy order only).
    pub fn alloc_page_raw(&self) -> *mut u8 {
        if self.magic.load(Ordering::Acquire) != 0xDEADBEEF
            || self.initialized.load(Ordering::Acquire) == 0
        {
            return core::ptr::null_mut();
        }
        let _guard = self.lock.lock_irqsave();
        // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
        // Runs NESTED under the small_alloc class lock (class -> buddy
        // order) — a stamp pair names the whole frozen chain.
        let _aw = crate::debug::allocwatch::guard(
            crate::debug::allocwatch::SITE_BUDDY,
            0,
        );
        let ptr = self.alloc_blocks(0);
        drop(_guard);
        ptr
    }

    /// Raw order-0 free, the matching twin of alloc_page_raw.
    pub fn free_page_raw(&self, ptr: *mut u8) {
        if self.initialized.load(Ordering::Acquire) == 0 {
            return;
        }
        let addr = ptr as usize;
        let heap_start = self.heap_start.load(Ordering::Acquire);
        let heap_end = self.heap_end.load(Ordering::Acquire);
        if addr < heap_start || addr >= heap_end {
            refused_free_note(FR_OUT_OF_WINDOW, addr, 0, 0, 0);
            return;
        }
        let _guard = self.lock.lock_irqsave();
        // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
        let _aw = crate::debug::allocwatch::guard(
            crate::debug::allocwatch::SITE_BUDDY,
            0,
        );
        self.free_blocks(ptr, 0);
        drop(_guard);
    }
}

impl BuddyAllocator {
    /// The real buddy free path, unintercepted — the deferred-free
    /// drainer (mm/deferred_free.rs) calls this so a held-back entry can
    /// never be re-deferred.
    fn dealloc_untracked(&self, ptr: *mut u8, layout: Layout) {
        // Small-object packing layer FIRST (alloc-crash r12-16): slab-page
        // objects free through their page's freelist, never the buddy —
        // and this must run before the R69 task-page probe, whose
        // page-granular ownership bitmap would refuse frees of non-task
        // objects sharing a (now packed) slab page with a live Task.
        if super::hotpath::free(ptr as usize) {
            return;
        }

        // R17-B withdrawn from the refuse path: the bitmap index aliases
        // for pages outside the heap's 8192-page window (stacks, page
        // tables), refusing legitimate frees and bleeding the machine to
        // the crashes seen in the r17 gate. The bitmap itself proved the
        // free side clean (never fired while corruption reproduced), so
        // the guard bought nothing; mark/unmark kept for the alloc-side
        // hunt next round.

        let size = layout.size();
        let align = layout.align();

        let ptr_addr = ptr as usize;
        let heap_start = self.heap_start.load(Ordering::Acquire);
        let heap_end = self.heap_end.load(Ordering::Acquire);

        if ptr_addr < heap_start || ptr_addr >= heap_end {
            crate::debug::memwatch::DEALLOC_OUTSIDE.fetch_add(1, Ordering::Relaxed);
            refused_free_note(FR_OUT_OF_WINDOW, ptr_addr, self.size_to_order(size.max(align)), 0, 0);
            return;
        }

        let order = self.size_to_order(size.max(align));

        // R69 free-side probe (x86_64): a free targeting a page that
        // still carries a live Task (ownership bitmap; alloc/
        // free_task_slot maintain it) is the double-handoff engine's
        // fuel — the page would return to the free lists while its Task
        // still runs, and the next two allocations share it. Refuse
        // under rux:scribble (retired debug=scribble) (the bitmap is exact since R69; the old
        // R17-B refusal was withdrawn because the pre-Batch-8 bitmap
        // aliased) and record with a caller backtrace either way.
        // IN_TASK_FREE marks the legitimate release path.
        #[cfg(feature = "x86_64")]
        if crate::trace::EVENT_SCRIBBLE.armed()
            && !crate::sched::hotpath::IN_TASK_FREE.load(core::sync::atomic::Ordering::Acquire)
            && crate::sched::hotpath::task_page_is_owned(ptr)
        {
            let frames = crate::debug::scribble::audit_frames();
            crate::debug::scribble::audit_record(
                crate::debug::scribble::AUDIT_KIND_FREE_LIVE,
                ptr_addr as u64,
                order as u32,
                &frames,
            );
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE); // R69 free-of-live
            refused_free_note(FR_TASK_LIVE, ptr_addr, order, 0, 0);
            return;
        }

        // Check if order exceeds BUDDY_MAX_ORDER
        if order > BUDDY_MAX_ORDER {
            crate::debug::memwatch::DEALLOC_BIGORDER.fetch_add(1, Ordering::Relaxed);
            refused_free_note(FR_ORDER_TOO_BIG, ptr_addr, order, 0, 0);
            return;
        }

        // R21-3: block-alignment validation — a free of an interior or
        // offset pointer (off-by-slack / partial-unwind bugs) used to
        // pass the leader-only tripwire after R17-A2 made interior pages
        // carry consistent metadata, splicing bogus blocks into the
        // doubly-linked freelist.
        if self.addr_to_page_idx_checked(ptr as usize).map(|idx| idx % (1usize << order) != 0).unwrap_or(true) {
            crate::debug::memwatch::DEALLOC_MISALIGNED.fetch_add(1, Ordering::Relaxed);
            refused_free_note(FR_UNALIGNED, ptr_addr, order, 0, 0);
            return;
        }

        let _guard = self.lock.lock_irqsave();
        // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
        let _aw = crate::debug::allocwatch::guard(
            crate::debug::allocwatch::SITE_BUDDY,
            0,
        );
        self.free_blocks(ptr, order);
        drop(_guard);
        crate::debug::memwatch::note_free(
            ptr,
            self.heap_start.load(Ordering::Relaxed),
            size,
        );
    }
}

/// Global allocator (Buddy System)
/// Note: This is the only allocator instance, used for kernel heap allocation and #[global_allocator]
pub static GLOBAL_ALLOCATOR: BuddyAllocator = BuddyAllocator::new();

/// rux:allocwatch (retired debug=allocwatch) probe: main buddy lock word state at sample time;
/// see debug/allocwatch.rs (STUCK detection).
pub fn audit_lock_held() -> bool {
    GLOBAL_ALLOCATOR.lock.is_locked()
}


/// Compatibility alias
pub use GLOBAL_ALLOCATOR as HEAP_ALLOCATOR;

/// The actual #[global_allocator]: small (<=4KB, <=8-aligned) allocations
/// pack into slab pages (16-byte header + 8-multiple object grids); larger
/// or stricter layouts go to the page-granular buddy. Previously the raw
/// buddy served everything and a 9-byte Box pinned a whole 4KB page —
/// small-object-heavy workloads (LTP inode02/ftest family) exhausted the
/// 128MB kernel heap in tens of seconds.
#[global_allocator]
pub static GLOBAL_COMBINED: CombinedAllocator = CombinedAllocator;

pub fn init_heap() {
    GLOBAL_ALLOCATOR.init();
}

/// init_heap with a RAM-clamped window size (see BuddyAllocator::init_sized;
/// the -m 512M panic family). The early boot block computes the effective
/// size from the discovered memory map.
pub fn init_heap_sized(size: usize) {
    GLOBAL_ALLOCATOR.init_sized(size);
}

/// LOCK-FREE rough heap census for the alloc-failure (ALLOCTHROW) print.
///
/// `buddy_stats()` takes the allocator irqsave lock, which is exactly
/// what the OOM-moment print must not rely on (the failure can arrive
/// with the caller holding unrelated locks; a wedged print would lose
/// the panic). This walks the free lists through the atomics only and
/// tolerates concurrent mutation: bounds-check every index, cap the
/// per-order walk, and let a torn read terminate the chain. The numbers
/// are a watermark, not an audit.
///
/// Returns (heap_size, free_bytes, blocks_at_order_0..=BUDDY_MAX_ORDER
/// truncated to u32 counts) — kept as a plain tuple to avoid any
/// formatting on the hot path.
pub fn buddy_free_census_lockfree() -> (usize, usize, [u32; BUDDY_MAX_ORDER + 1]) {
    let mut blocks = [0u32; BUDDY_MAX_ORDER + 1];
    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return (0, 0, blocks);
    }
    let start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
    let end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);
    let heap_size = end.saturating_sub(start);
    let mut free_pages = 0usize;
    for order in 0..=BUDDY_MAX_ORDER {
        let mut page_idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);
        let mut hops = 0usize;
        while page_idx != EMPTY_LIST && page_idx < BUDDY_MAX_PAGES && hops < BUDDY_MAX_PAGES {
            blocks[order] = blocks[order].saturating_add(1);
            free_pages += 1usize << order;
            let next = HEAP_ALLOCATOR.meta.get(page_idx).next as usize;
            if next == u32::MAX as usize || next >= BUDDY_MAX_PAGES {
                break;
            }
            page_idx = next;
            hops += 1;
        }
    }
    (heap_size, free_pages * PAGE_SIZE, blocks)
}

/// Get Buddy allocator statistics
pub fn buddy_stats() -> BuddyStats {    let mut stats = BuddyStats::default();

    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return stats;
    }

    let _guard = HEAP_ALLOCATOR.lock.lock_irqsave();

    stats.heap_start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
    stats.heap_end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);
    stats.heap_size = stats.heap_end - stats.heap_start;

    // Count free blocks per order
    let mut total_free_pages = 0usize;
    for order in 0..=BUDDY_MAX_ORDER {
        let mut count = 0usize;
        let mut page_idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);

        while page_idx != EMPTY_LIST && page_idx < BUDDY_MAX_PAGES {
            count += 1;
            total_free_pages += 1usize << order;
            let next = HEAP_ALLOCATOR.meta.get(page_idx).next as usize;
            if next == u32::MAX as usize || next >= BUDDY_MAX_PAGES {
                break;
            }
            page_idx = next;
        }
        stats.free_blocks[order] = count;
    }

    stats.free_bytes = total_free_pages * PAGE_SIZE;
    stats.used_bytes = stats.heap_size - stats.free_bytes;

    stats
}

// ============================================================================
// Meta shadow audit (rux:metawatch (retired debug=metawatch), wildwrite-hunt line session 13)
//
// The unified-engine hypothesis says the wild writer's second downstream
// face is buddy-meta corruption (the ww-v2 order-18 grind: an impossible
// coalescing scale with only ~229 MB in use). The gdb watchpoint chain
// can only cover kernel STATIC victims; the buddy metadata (free-list
// links, per-page free bitmap) is megabytes of heap-adjacent BSS and
// cannot be watchpointed. This audit derives a shadow free bitmap from
// the free lists and cross-checks the linkage invariants:
//   - free_lists[order] heads are EMPTY_LIST or a valid, 2^order-aligned
//     page index whose leader meta says (order, free=1)     (INV-HEAP-1/2)
//   - prev/next links mirror each other along each list
//   - the derived per-page free set equals `free_bits` exactly
//     (INV-HEAP-3), with no page listed twice.
//
// Pass 1 runs LOCK-FREE from the timer cadence (racy per-field reads are
// tolerated, same discipline as buddy_free_census_lockfree), so it can
// observe transient mid-mutation states; any dirty pass 1 is re-audited
// UNDER the heap irqsave lock before being reported — only a persistent
// mismatch is corruption. Callers never print on pass-1 evidence alone.
// ============================================================================
//
// The audit's own types (MetaFinding, MetaFindKind, AuditScratch,
// MetaScanOutcome), the BuddyStats snapshot and the CombinedAllocator
// front are defined in mm/types.rs; BuddyAllocator and its metadata
// stay here. Re-exported so the `mm::buddy_allocator::…` paths are
// unchanged.

/// The audit's empty finding (const constructor for stack buffers).
pub const EMPTY_FINDING: MetaFinding = MetaFinding {
    kind: MetaFindKind::LinkOob,
    order: 0,
    idx: 0,
    prev: 0,
    next: 0,
    word: 0,
    bits: 0,
    derived: 0,
};

/// Scratch shadow bitmap for the audit, plus the single-CPU claim that
/// makes it safe: pass 1 and its under-lock confirmation run inside one
/// claim, so the scratch is never shared by two concurrent audits.
static META_AUDIT_BUSY: AtomicU64 = AtomicU64::new(0);

// SAFETY: see `AuditScratch` doc — all access is claim-serialized.
unsafe impl Send for AuditScratch {}
unsafe impl Sync for AuditScratch {}

static META_AUDIT_SCRATCH: AuditScratch = AuditScratch {
    data: UnsafeCell::new([0; FREE_BITMAP_WORDS]),
};

fn record(
    findings: &mut [MetaFinding],
    total: &mut usize,
    f: MetaFinding,
) {
    if *total < findings.len() {
        findings[*total] = f;
    }
    *total = (*total).saturating_add(1);
}

/// The audit walk itself. Caller provides exclusivity over the scratch
/// (the META_AUDIT_BUSY claim) — see buddy_meta_shadow_scan.
///
/// SAFETY-of-use note: reads `meta` fields without the allocator lock
/// (census precedent — aligned u8/u32 field loads; a torn combination
/// surfaces as a transient pass-1 finding that the under-lock pass
/// clears). Returns the TOTAL number of findings; at most
/// `findings.len()` are stored.
fn meta_audit_inner(findings: &mut [MetaFinding]) -> usize {
    let mut total = 0usize;
    // SAFETY: caller holds the META_AUDIT_BUSY claim (single audit at a
    // time across CPUs); the borrow lives only inside this function.
    let scratch = unsafe { &mut (*META_AUDIT_SCRATCH.data.get()) };
    for w in scratch.iter_mut() {
        *w = 0;
    }

    let mut hop_budget = BUDDY_MAX_PAGES + BUDDY_MAX_ORDER + 1;
    'orders: for order in 0..=BUDDY_MAX_ORDER {
        let npages = 1usize << order;
        let mut idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);
        let mut prev_link = u32::MAX as usize; // head's prev must be null
        loop {
            if idx == EMPTY_LIST {
                break;
            }
            if idx >= BUDDY_MAX_PAGES || idx + npages > BUDDY_MAX_PAGES {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::LinkOob,
                        order,
                        idx,
                        prev: 0,
                        next: 0,
                        word: 0,
                        bits: 0,
                        derived: 0,
                    },
                );
                break;
            }
            if hop_budget == 0 {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::HopOverflow,
                        order,
                        idx,
                        prev: 0,
                        next: 0,
                        word: 0,
                        bits: 0,
                        derived: 0,
                    },
                );
                break 'orders;
            }
            hop_budget -= 1;

            // Racy leader-meta snapshot (tolerated; see function doc).
            let m = HEAP_ALLOCATOR.meta.get(idx);
            let (m_order, m_free, m_prev, m_next) = (m.order, m.free, m.prev, m.next);

            if m_order as usize != order {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::OrderMismatch,
                        order,
                        idx,
                        prev: m_prev,
                        next: m_next,
                        word: 0,
                        bits: m_order as u64,
                        derived: order as u64,
                    },
                );
            }
            if m_free != 1 {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::FreeByteClear,
                        order,
                        idx,
                        prev: m_prev,
                        next: m_next,
                        word: 0,
                        bits: m_free as u64,
                        derived: 1,
                    },
                );
            }
            if idx & (npages - 1) != 0 {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::Unaligned,
                        order,
                        idx,
                        prev: m_prev,
                        next: m_next,
                        word: 0,
                        bits: 0,
                        derived: 0,
                    },
                );
            }
            if m_prev as usize != prev_link {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::PrevBroken,
                        order,
                        idx,
                        prev: m_prev,
                        next: m_next,
                        word: 0,
                        bits: m_prev as u64,
                        derived: prev_link as u64,
                    },
                );
            }

            // Set the span's shadow bits; overlap = double listing.
            let w0 = idx / 64;
            let w1 = (idx + npages - 1) / 64;
            let mut double = false;
            for w in w0..=w1 {
                let lo = if w == w0 { idx % 64 } else { 0 };
                let hi = if w == w1 { (idx + npages - 1) % 64 } else { 63 };
                let mask = if lo == 0 && hi == 63 {
                    !0u64
                } else {
                    ((1u64 << (hi - lo + 1)) - 1) << lo
                };
                let old = scratch[w];
                if old & mask != 0 {
                    double = true;
                }
                scratch[w] = old | mask;
            }
            if double {
                record(
                    findings,
                    &mut total,
                    MetaFinding {
                        kind: MetaFindKind::DoubleListed,
                        order,
                        idx,
                        prev: m_prev,
                        next: m_next,
                        word: 0,
                        bits: 0,
                        derived: 0,
                    },
                );
            }

            prev_link = idx;
            let next = m_next as usize;
            if next == u32::MAX as usize {
                break;
            }
            idx = next;
        }
    }

    // Cross-check the shadow against the per-page bitmap (INV-HEAP-3).
    for w in 0..FREE_BITMAP_WORDS {
        let d = scratch[w];
        let b = HEAP_ALLOCATOR.free_bits[w].load(Ordering::Relaxed);
        if b & !d != 0 {
            record(
                findings,
                &mut total,
                MetaFinding {
                    kind: MetaFindKind::BitmapSetUnlisted,
                    order: BUDDY_MAX_ORDER + 1,
                    idx: 0,
                    prev: 0,
                    next: 0,
                    word: w,
                    bits: b,
                    derived: d,
                },
            );
        }
        if d & !b != 0 {
            record(
                findings,
                &mut total,
                MetaFinding {
                    kind: MetaFindKind::BitmapClearListed,
                    order: BUDDY_MAX_ORDER + 1,
                    idx: 0,
                    prev: 0,
                    next: 0,
                    word: w,
                    bits: b,
                    derived: d,
                },
            );
        }
    }

    total
}

/// One shadow scan: lock-free pass, then — only if pass 1 was dirty —
/// an under-lock re-audit. Returns None if another CPU's scan is in
/// flight (the claim serializes audits; the next cadence will run).
pub fn buddy_meta_shadow_scan(findings: &mut [MetaFinding]) -> Option<MetaScanOutcome> {
    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return Some(MetaScanOutcome { lockfree_findings: 0, confirmed_findings: 0 });
    }
    if META_AUDIT_BUSY
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    let n_free = meta_audit_inner(findings);
    let n_conf = if n_free > 0 {
        // Re-audit under the heap lock, overwriting the findings buffer
        // with the authoritative (persistent) set.
        let _g = HEAP_ALLOCATOR.lock.lock_irqsave();
        meta_audit_inner(findings)
    } else {
        0
    };
    META_AUDIT_BUSY.store(0, Ordering::Release);
    Some(MetaScanOutcome {
        lockfree_findings: n_free,
        confirmed_findings: n_conf,
    })
}

/// Selftest (`metawatch-selftest=1`): under claim + heap lock, clear one
/// free_bits bit of a listed block, expect the audit to flag it, restore,
/// and expect a clean re-audit. Proves the detector end-to-end without
/// leaving damage (all under one irqsave section — no allocator traffic
/// can interleave).
pub fn buddy_meta_selftest() -> bool {
    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return false;
    }
    if META_AUDIT_BUSY
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let mut findings = [EMPTY_FINDING; 4];
    let _g = HEAP_ALLOCATOR.lock.lock_irqsave();

    // Pre-audit must be clean.
    if meta_audit_inner(&mut findings) != 0 {
        META_AUDIT_BUSY.store(0, Ordering::Release);
        return false;
    }

    // Find any listed block to corrupt.
    let mut victim: Option<(usize, usize)> = None; // (order, idx)
    for order in 0..=BUDDY_MAX_ORDER {
        let head = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);
        if head != EMPTY_LIST && head < BUDDY_MAX_PAGES {
            victim = Some((order, head));
            break;
        }
    }
    let (order, idx) = match victim {
        Some(v) => v,
        None => {
            META_AUDIT_BUSY.store(0, Ordering::Release);
            return false;
        }
    };

    let w = idx / 64;
    let mask = 1u64 << (idx % 64);
    let saved = HEAP_ALLOCATOR.free_bits[w].load(Ordering::Relaxed);
    HEAP_ALLOCATOR.free_bits[w].fetch_and(!mask, Ordering::Relaxed);

    let flagged = meta_audit_inner(&mut findings);
    let saw_kind = flagged > 0
        && findings[..flagged.min(findings.len())]
            .iter()
            .any(|f| f.kind == MetaFindKind::BitmapClearListed && f.word == w);

    // Restore and require a clean re-audit.
    HEAP_ALLOCATOR.free_bits[w].fetch_or(mask, Ordering::Relaxed);
    let clean = meta_audit_inner(&mut findings) == 0;

    META_AUDIT_BUSY.store(0, Ordering::Release);
    saw_kind && clean
}

/// The real (unintercepted) free path: slab-region or buddy-heap by
/// pointer range. `CombinedAllocator::dealloc` routes here after the
/// deferred-free check declines; the deferred-free drainer calls this
/// directly so re-deferring is impossible.
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


// ════════════════ allocator ═══════════════════════════
// (was allocator.rs — the buddy allocator's re-export module. Nothing of it
// survives the merge but the deprecated BumpAllocator alias, which is in
// types.rs.)
// Memory Allocator Module
//
// This module re-exports the Buddy System allocator's public interface.
// The Buddy System is an efficient memory allocation algorithm that supports O(log n) allocation and deallocation.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

// The old BumpAllocator has been deprecated, but the type alias is kept
// to avoid breaking changes. Its definition lives in mm/types.rs;
// re-exported so the `mm::allocator::BumpAllocator` path is unchanged.


// ════════════════ layout ══════════════════════════════
// (was layout.rs)
// Memory Layout Management
//
// This module manages kernel memory layout, providing dynamic calculation
// of memory regions instead of hardcoding addresses.
//
// The is part of Phase 0 of the memory management refactoring plan.

/// Physical memory base address (QEMU virt platform)
/// This is the start of physical memory on RISC-V QEMU virt
pub const LAYOUT_PHYS_MEMORY_BASE: usize = 0x80000000;

/// Kernel entry point (after OpenSBI)
/// This is where the kernel is loaded by the bootloader
pub const KERNEL_ENTRY: usize = 0x80200000;

// (DEFAULT_HEAP_SIZE removed — crate::config::KERNEL_HEAP_SIZE is the
// single source for the heap size; a second 32MB constant here silently
// drifted from the config, review 4.20.)

/// Default slab size. 16MB: the slab serves every small kernel allocation,
/// and the old 4MB filled under LTP fs-churn workloads (SlabPages is bounded
/// by this static region — it does not grow from buddy). RAM clamping in
/// `effective_static_regions` drops the slab to 0 on machines without room.
pub const DEFAULT_SLAB_SIZE: usize = 16 * 1024 * 1024;

// EffectiveStaticRegions (the clamped carve this file's functions
// compute; KernelMemoryLayout itself stays here — this file is its own)
// is defined in mm/types.rs. Re-exported so the path is unchanged.

/// End of the CONTIGUOUS USABLE region containing `heap_start`, or 0 when
/// the base sits in no usable region.
///
/// The static carve (heap + slab) must stay inside one usable region:
/// with memory holes (x86_64 q35/KVM 4G: usable RAM ends at 0x7ffcd000,
/// a 208KB reserved hole follows up to 0x80000000, more usable RAM sits
/// above 4G) a bound derived from the GLOBAL span max let the heap tail
/// and the slab that follows it land in hole DRAM. The linear map only
/// maps usable regions, so writes there page-fault with no fixup — the
/// rs-egl r12/r13/r15 physmap-top crashes (kernel fault at
/// 0xffff88807ffcd020 inside `Vec::push_mut` / `ext4_readdir`: a
/// getdents64 Vec whose backing chunk was carved from the unmapped heap
/// tail, exactly as the heap reached its end for the first time under
/// the composition working set).
///
/// `regions` is the usable-region list (`cmdline::MemoryRegion`); the
/// tightest containing region wins (regions should not nest, but the
/// min() makes the bound safe if they do).
pub fn heap_region_end(regions: &[crate::cmdline::MemoryRegion], heap_start: usize) -> usize {
    regions
        .iter()
        .filter(|r| r.base <= heap_start && heap_start < r.base.saturating_add(r.size))
        .map(|r| r.base.saturating_add(r.size))
        .min()
        .unwrap_or(0)
}

/// Effective static-region sizing for machines smaller than the config
/// assumes (the -m 512M boot panic family).
///
/// `KERNEL_HEAP_PHYS` (+ `KERNEL_HEAP_SIZE`) and the slab window that
/// follows it are COMPILE-TIME addresses: on a machine whose discovered
/// RAM ends before `heap_start + heap_size` the old code (a) reserved a
/// heap range extending past RAM in memblock, (b) underflowed in
/// `init_from_memblock` (`phys_base + phys_size - slab_start - slab_size`
/// panics "attempt to subtract with overflow" at boot), and (c) let the
/// heap buddy cover nonexistent DRAM — its single MAX_ORDER seed block
/// made allocations past the RAM end fault (KERNPANIC on first touch).
///
/// This helper is the ONE place the static regions are clamped to the
/// discovered memory map:
///  - the heap NEVER extends past `phys_end`, and always leaves room for
///    the slab plus a minimum user zone (`MIN_USER_ZONE`), shrinking the
///    heap (config size is a CAP, not an entitlement);
///  - the slab follows the (possibly shrunk) heap end, clamped to RAM;
///  - when RAM is too small even for that, regions come back empty
///    rather than wrapped — boot fails with a clear "no memory" instead
///    of an arithmetic panic.
///
/// Machines with RAM ≥ KERNEL_HEAP_PHYS-base + KERNEL_HEAP_SIZE +
/// slab + MIN_USER_ZONE get EXACTLY the historical layout (the clamp is
/// a no-op) — 2G/4G boots are unchanged.
pub fn effective_static_regions(
    phys_base: usize,
    phys_size: usize,
    heap_start: usize,
    heap_region_end: usize,
) -> EffectiveStaticRegions {
    /// Smallest user zone we are willing to hand the buddy when the heap
    /// must shrink to fit (16 MiB — enough for the minimal rootfs shell).
    const MIN_USER_ZONE: usize = 16 * 1024 * 1024;
    /// Heap shrink granularity (keeps the window a power-of-two multiple
    /// so the buddy can seed it with whole blocks).
    const HEAP_ALIGN: usize = 16 * 1024 * 1024;
    /// Never hand the kernel heap less than this, even if it means a tiny
    /// user zone (the heap hosts every Task/file object; below this the
    /// kernel cannot boot at all).
    const MIN_HEAP: usize = 32 * 1024 * 1024;

    // Clamp the carve to the containing usable region — never past a
    // hole (see heap_region_end), and never past the global span either.
    let span_end = phys_base.saturating_add(phys_size);
    let phys_end = if heap_region_end >= phys_base {
        span_end.min(heap_region_end)
    } else {
        // Heap base in no usable region: carve nothing; boot fails with
        // the existing "no heap window" print instead of phantom DRAM.
        phys_base
    };
    let config_size = crate::config::KERNEL_HEAP_SIZE;
    let slab_size = DEFAULT_SLAB_SIZE;

    // Heap: cap at RAM end minus slab minus the minimum user zone...
    let headroom = phys_end
        .saturating_sub(heap_start)
        .saturating_sub(slab_size)
        .saturating_sub(MIN_USER_ZONE);
    let mut heap_size = config_size.min(headroom);
    // ...but keep at least MIN_HEAP when the machine is that tight (the
    // user zone shrinks below MIN_USER_ZONE instead of the heap), and on
    // machines where the config does not fit at all, leave the heap at
    // most half the RAM (a 464MB-of-512MB heap left a 30MB user zone —
    // the machine booted but 16MB of anonymous mmap OOM'd; the heap
    // hosts every Task/file object, so half the machine is already a
    // generous entitlement for it).
    let max_fit = phys_end
        .saturating_sub(heap_start)
        .saturating_sub(slab_size);
    let small_machine_cap = phys_size / 2;
    if heap_size < MIN_HEAP {
        heap_size = MIN_HEAP.min(max_fit);
    } else if config_size > headroom {
        heap_size = heap_size.min(small_machine_cap);
    }
    // Align DOWN so buddy seeding covers whole power-of-two blocks
    // without the last block poking past the window.
    heap_size = heap_size / HEAP_ALIGN * HEAP_ALIGN;

    let heap_end = heap_start.saturating_add(heap_size);
    let slab_fit = phys_end.saturating_sub(heap_end);
    let slab_size = if slab_fit >= slab_size { slab_size } else { 0 };
    EffectiveStaticRegions {
        heap_start,
        heap_size,
        slab_start: heap_end,
        slab_size,
    }
}


impl KernelMemoryLayout {
    /// Create a new empty layout
    pub const fn new() -> Self {
        Self {
            phys_base: LAYOUT_PHYS_MEMORY_BASE,
            phys_size: 0,
            kernel_start: 0,
            kernel_end: 0,
            heap_start: 0,
            heap_size: crate::config::KERNEL_HEAP_SIZE,
            slab_start: 0,
            slab_size: DEFAULT_SLAB_SIZE,
            user_phys_start: 0,
            user_phys_size: 0,
            frame_alloc_start: 0,
            frame_alloc_size: 0,
        }
    }

    /// Initialize from memblock
    ///
    /// This should be called after memblock is initialized with memory regions.
    ///
    /// `heap_region_end` is the end of the contiguous USABLE region
    /// containing the heap window base (see [`heap_region_end`]): the
    /// heap+slab carve is clamped to it while `phys_size` stays the full
    /// discovered span (the PFN-span consumers — user-address-space
    /// validation, above-4G pages — must keep the global bound).
    #[allow(clippy::too_many_arguments)]
    pub fn init_from_memblock(
        phys_base: usize,
        phys_size: usize,
        kernel_start: usize,
        kernel_end: usize,
        heap_region_end: usize,
    ) -> Self {
        // Calculate heap region (after kernel)
        let _kernel_size = kernel_end - kernel_start;
        let heap_start = (kernel_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        // Static regions clamped to the DISCOVERED memory map (the -m 512M
        // panic family — see effective_static_regions). The kernel_end
        // argument is KERNEL_HEAP_PHYS: the heap window base.
        let eff = effective_static_regions(phys_base, phys_size, heap_start, heap_region_end);
        let heap_size = eff.heap_size;

        // Calculate slab region (after heap)
        let slab_start = eff.slab_start;
        let slab_size = eff.slab_size;

        // Calculate user physical memory region.
        //
        // P0-7 (2026-09): the old `min(25% of remaining, 64MB)` cap starved
        // every resident-heavy program (JVM/CPython/rustc — anything above
        // 64MB resident hit a hard OOM). This region is ACCOUNTING ONLY —
        // the zone allocator (mm/page_alloc.rs) hands out all memblock-free
        // memory regardless of this split; user_phys has no allocator
        // consumer (audited: only print_kernel_layout and the verify crate
        // read it). New policy: 75% of the memory remaining after
        // heap+slab is user-physical, 25% stays reserved for kernel
        // structures (page tables, page descriptors, slab growth). No
        // 64MB truncation. Boot order is unchanged (layout init runs at the
        // same main.rs point as before). Saturating: on machines smaller
        // than the static regions the remainder is simply zero (the
        // effective_* clamps already shrank them to fit).
        let remaining_after_slab = (phys_base + phys_size)
            .saturating_sub(slab_start)
            .saturating_sub(slab_size);
        let user_phys_size = remaining_after_slab * 3 / 4;
        let user_phys_start = slab_start + slab_size;

        // Frame allocator starts after user physical region
        let frame_alloc_start = user_phys_start + user_phys_size;
        let frame_alloc_size = (phys_base + phys_size).saturating_sub(frame_alloc_start);

        Self {
            phys_base,
            phys_size,
            kernel_start,
            kernel_end,
            heap_start,
            heap_size,
            slab_start,
            slab_size,
            user_phys_start,
            user_phys_size,
            frame_alloc_start,
            frame_alloc_size,
        }
    }
}

// Global memory layout instance.
//
// KERNEL_LAYOUT is written once during early boot by kernel_layout_init() and
// read-only afterwards.  KERNEL_LAYOUT_INIT acts as a guard so that readers
// can check initialization without accessing the MaybeUninit.
static KERNEL_LAYOUT_INIT: AtomicBool = AtomicBool::new(false);
pub(super) static mut KERNEL_LAYOUT: MaybeUninit<KernelMemoryLayout> =
    MaybeUninit::uninit();

/// Initialize kernel memory layout
///
/// This should be called once during boot after memblock is initialized.
pub fn kernel_layout_init(layout: KernelMemoryLayout) {
    if KERNEL_LAYOUT_INIT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        // First and only write — guarded by compare_exchange above
        // (INV-MM-11 init arm).
        super::r#unsafe::kernel_layout_write(layout);
    }
}

/// Get kernel memory layout
///
/// Panics if layout is not initialized.
pub fn kernel_layout() -> &'static KernelMemoryLayout {
    if !KERNEL_LAYOUT_INIT.load(Ordering::Acquire) {
        panic!("Kernel layout not initialized");
    }
    // KERNEL_LAYOUT_INIT is true, so kernel_layout_init() has completed
    // and the value is fully initialized (INV-MM-11 shared arm).
    super::r#unsafe::kernel_layout_ref()
}

/// Check if kernel layout is initialized
pub fn is_kernel_layout_initialized() -> bool {
    KERNEL_LAYOUT_INIT.load(Ordering::Acquire)
}

// ==================== Accessor Functions ====================

/// Get physical memory base address
#[inline]
pub fn phys_memory_base() -> usize {
    kernel_layout().phys_base
}

/// Get physical memory size
#[inline]
pub fn phys_memory_size() -> usize {
    kernel_layout().phys_size
}

/// Get kernel start address
#[inline]
pub fn kernel_start() -> usize {
    kernel_layout().kernel_start
}

/// Get kernel end address
#[inline]
pub fn kernel_end() -> usize {
    kernel_layout().kernel_end
}

/// Get heap start address
#[inline]
pub fn heap_start() -> usize {
    kernel_layout().heap_start
}

/// Get heap size
#[inline]
pub fn heap_size() -> usize {
    kernel_layout().heap_size
}

/// Get slab start address
#[inline]
pub fn slab_start() -> usize {
    kernel_layout().slab_start
}

/// Get slab size
#[inline]
pub fn slab_size() -> usize {
    kernel_layout().slab_size
}

/// Get user physical memory start address
#[inline]
pub fn user_phys_start() -> usize {
    kernel_layout().user_phys_start
}

/// Get user physical memory size
#[inline]
pub fn user_phys_size() -> usize {
    kernel_layout().user_phys_size
}

/// Get frame allocator start address
#[inline]
pub fn frame_alloc_start() -> usize {
    kernel_layout().frame_alloc_start
}

/// Get frame allocator size
#[inline]
pub fn frame_alloc_size() -> usize {
    kernel_layout().frame_alloc_size
}

/// Get heap end address
#[inline]
pub fn heap_end() -> usize {
    heap_start() + heap_size()
}

/// Get slab end address
#[inline]
pub fn slab_end() -> usize {
    slab_start() + slab_size()
}

/// Get user physical memory end address
#[inline]
pub fn user_phys_end() -> usize {
    user_phys_start() + user_phys_size()
}

// ==================== Debug/Info Functions ====================

/// Boot-time consistency assertion between the kernel layout and memblock
/// (review 4.20).
///
/// The heap and slab regions this layout advertises must:
///  - lie inside physical memory,
///  - be exactly adjacent (slab starts where the heap ends), and
///  - be fully covered by memblock RESERVED regions — anything else means
///    the zone allocator (which seeds from memory minus reserved) would
///    hand those pages to a second owner.
///
    /// Panics at boot (deterministic, single-threaded) on violation.
    pub fn assert_memblock_consistency(layout: &KernelMemoryLayout) {
        let phys_end = layout.phys_base.saturating_add(layout.phys_size);

        assert!(
            layout.heap_start >= layout.phys_base
                && layout.heap_start.saturating_add(layout.heap_size) <= phys_end,
            "layout: heap {:#x}+{:#x} outside physical memory {:#x}-{:#x}",
            layout.heap_start,
            layout.heap_size,
            layout.phys_base,
            phys_end
        );
        assert!(
            layout.slab_start.saturating_add(layout.slab_size) <= phys_end,
            "layout: slab {:#x}+{:#x} outside physical memory",
            layout.slab_start,
            layout.slab_size
        );
        assert!(
            layout.slab_start == layout.heap_start + layout.heap_size,
            "layout: slab {:#x} not adjacent to heap end {:#x}",
            layout.slab_start,
            layout.heap_start + layout.heap_size
        );

        let mb = super::hotpath::memblock();
        let regions = [
            ("heap", layout.heap_start, layout.heap_size),
            ("slab", layout.slab_start, layout.slab_size),
        ];
        for (name, base, size) in regions {
            if size == 0 {
                continue;
            }
            let first = base;
            let last = base + size - PAGE_SIZE;
            assert!(
                mb.is_reserved(first) && mb.is_reserved(last),
                "layout: {} region {:#x}-{:#x} not fully reserved in memblock \
                 (first_reserved={} last_reserved={}) — double-ownership risk",
                name,
                base,
                base + size,
                mb.is_reserved(first),
                mb.is_reserved(last)
            );
            // Hole guard (rs-egl r12/13/15 family): the static carve must
            // sit inside USABLE DRAM. "Inside the span" is not enough on
            // machines with holes (x86_64 q35: usable ends 0x7ffcd000,
            // reserved hole to 0x80000000, more RAM above 4G) — the old
            // span-based bound put the heap tail and the slab in unmapped
            // hole DRAM and the first write page-faulted with no fixup.
            assert!(
                mb.is_memory(first) && mb.is_memory(last),
                "layout: {} region {:#x}-{:#x} not inside usable RAM \
                 (first_mem={} last_mem={}) — static carve crosses a memory hole",
                name,
                base,
                base + size,
                mb.is_memory(first),
                mb.is_memory(last)
            );
        }
    }

/// Print memory layout information
pub fn print_kernel_layout() {
    if !is_kernel_layout_initialized() {
        crate::println!("kernel_layout: not initialized");
        return;
    }

    let layout = kernel_layout();

    crate::println!("Memory Layout:");
    crate::println!("  Physical Memory: {:#x} - {:#x} ({:?} MB)",
        layout.phys_base,
        layout.phys_base + layout.phys_size,
        layout.phys_size / (1024 * 1024),
    );
    crate::println!("  Kernel:          {:#x} - {:#x} ({:?} MB)",
        layout.kernel_start,
        layout.kernel_end,
        (layout.kernel_end - layout.kernel_start) / (1024 * 1024),
    );
    crate::println!("  Heap:           {:#x} - {:#x} ({:?} MB)",
        layout.heap_start,
        layout.heap_start + layout.heap_size,
        layout.heap_size / (1024 * 1024),
    );
    crate::println!("  Slab:            {:#x} - {:#x} ({:?} MB)",
        layout.slab_start,
        layout.slab_start + layout.slab_size,
        layout.slab_size / (1024 * 1024),
    );
    crate::println!("  User Phys:      {:#x} - {:#x} ({:?} MB)",
        layout.user_phys_start,
        layout.user_phys_start + layout.user_phys_size,
        layout.user_phys_size / (1024 * 1024),
    );
    crate::println!("  Frame Alloc:    {:#x} - {:#x} ({:?} MB)",
        layout.frame_alloc_start,
        layout.frame_alloc_start + layout.frame_alloc_size,
        layout.frame_alloc_size / (1024 * 1024),
    );
}


// ════════════════ page ════════════════════════════════
// (was page.rs)
// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

// Page Frame Management
//
// Basic types for physical and virtual page management.
// Page allocation is handled by the zone allocator (page_alloc.rs).

pub const PAGE_MASK: usize = PAGE_SIZE - 1;


impl PhysAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn frame_number(&self) -> PhysFrameNr {
        self.0 / PAGE_SIZE
    }

    /// Get physical page number (PPN)
    pub fn ppn(&self) -> usize {
        self.0 / PAGE_SIZE
    }
}

impl VirtAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn page_number(&self) -> VirtPageNr {
        self.0 / PAGE_SIZE
    }
}

impl PhysFrame {
    pub const fn new(number: PhysFrameNr) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: PhysAddr) -> Self {
        Self::new(addr.frame_number())
    }

    pub fn start_address(&self) -> PhysAddr {
        PhysAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<PhysAddr> {
        let start = self.start_address();
        let end = PhysAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

impl VirtPage {
    pub const fn new(number: VirtPageNr) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: VirtAddr) -> Self {
        Self::new(addr.page_number())
    }

    pub fn start_address(&self) -> VirtAddr {
        VirtAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<VirtAddr> {
        let start = self.start_address();
        let end = VirtAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

/// Initialize page descriptors
///
/// This must be called after memblock is initialized.
/// Page allocation uses the zone allocator (init_zone_system).
pub fn init_page_descriptors(start_frame: PhysFrameNr, nr_pages: usize) {
    // Initialize page descriptor array
    super::hotpath::init_mem_map(start_frame, nr_pages);
}

// page_frame_to_page / page_frame_to_page_mut were page.rs's forwarders to
// page_desc's frame_to_page / frame_to_page_mut. The two files declared each
// name; the implementation keeps it, the forwarder carries the page_ prefix.
/// Get Page descriptor for a frame
pub fn page_frame_to_page(frame: PhysFrame) -> *const super::types::Page {
    super::hotpath::frame_to_page(frame)
}

/// Get mutable Page descriptor for a frame
pub fn page_frame_to_page_mut(frame: PhysFrame) -> *mut super::types::Page {
    super::hotpath::frame_to_page_mut(frame)
}


// ════════════════ page_desc ═══════════════════════════
// (was page_desc.rs)
// Page Descriptor
//
// Maintains metadata for each physical page frame, including:
// - Reference count (_refcount)
// - Page flags (flags)
// - Map count (_mapcount)
// - Other metadata
//
// # Safety Invariants
//
// The following invariants must hold at all times for every `Page` descriptor:
//
// - **INV-REF-1**: `_refcount` must never be negative.
//   `put_page()` restores the old value on underflow and warns.
//
// - **INV-REF-2**: `_refcount == 0` ⟺ page is on a buddy free list (or otherwise free).
//
// - **INV-REF-3**: `_refcount > 0` ⟺ page is in use by at least one owner.
//
// - **INV-REF-4**: `_mapcount == -1` (PAGE_MAPCOUNT_BIAS) ⟺ page is not mapped
//   by any page table entry.
//
// - **INV-REF-5**: `_mapcount > -1` ⟺ page is mapped in `(_mapcount + 1)` page tables.
//
// - **INV-REF-6**: If `Cow` flag is set ⟹ `_refcount >= 2` AND the PTE `W` bit is clear.
//   This is the COW pre-condition: the page is shared read-only among multiple mappings.
//

// (INV-PAGEDESC-1: the vmemmap family — descriptors are addressed by
// pfn through the linear vmemmap array (bounds by the managed-range
// check); the flag words are the same lock-free atomics every reader
// uses.)

/// Flag-bit window [18:20] storing the LRU list index (0..NR_LRU_LISTS)
/// of a page carrying `PageFlag::Lru`. Storing the index on the page makes
/// LRU unlink O(1) with the doubly-linked lru_next/lru_prev pair (review
/// 4.11) — no list walk to find which of the 5 lists owns the page.
pub const LRU_LIST_SHIFT: u32 = 18;
pub const LRU_LIST_MASK: u32 = 0x7 << LRU_LIST_SHIFT;

impl PageFlags {
    /// Create empty flags collection
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Create from raw value
    pub const fn from_raw(flags: u32) -> Self {
        Self(AtomicU32::new(flags))
    }

    /// Get raw value
    pub fn raw(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Test if flag is set
    pub fn test(&self, flag: PageFlag) -> bool {
        self.0.load(Ordering::Relaxed) & (flag as u32) != 0
    }

    /// Set flag
    pub fn set(&self, flag: PageFlag) {
        self.0.fetch_or(flag as u32, Ordering::Release);
    }

    /// Clear flag
    pub fn clear(&self, flag: PageFlag) {
        self.0.fetch_and(!(flag as u32), Ordering::Release);
    }

    /// Test and set flag (returns old value)
    pub fn test_and_set(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_or(bit, Ordering::AcqRel) & bit) != 0
    }

    /// Test and clear flag (returns old value)
    pub fn test_and_clear(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_and(!bit, Ordering::AcqRel) & bit) != 0
    }

    /// Clear all flags
    pub fn clear_all(&self) {
        self.0.store(0, Ordering::Release);
    }
}

/// Map count initial offset value (-1 means unmapped)
pub const PAGE_MAPCOUNT_BIAS: i32 = -1;

// ==================== SIGILL text-frame stamp table (gnome-sigill) ====================
//
// Records every frame handed to a user mapping by the file-backed demand
// fault path (those frames carry no mapcount/rmap/LRU state). Teardown
// walkers skip stamped frames in the diagnostic build (leak on purpose);
// free_pages() on a stamped frame trips a loud line naming the owner.
//
// Entry value: (pid << 32) | (va >> 12); 0 = empty.

/// Number of stamp slots (power of two). Sized so a GNOME-boot worth of
/// live file-fill frames (~40k) loads the table under ~30% — the linear
/// probe then essentially never reaches its eviction fallback.
pub const TEXT_STAMP_SLOTS: usize = 131072;

static TEXT_STAMPS: [AtomicU64; TEXT_STAMP_SLOTS] =
    [const { AtomicU64::new(0) }; TEXT_STAMP_SLOTS];

fn text_stamp_slot(pfn: usize) -> usize {
    // cheap multiplicative hash; linear probe on lookup/insert
    (pfn.wrapping_mul(0x9E37_79B9)) & (TEXT_STAMP_SLOTS - 1)
}

/// Record `pfn` as a live file-backed fill frame owned by (pid, va).
/// Stamp layout (must fit 64 bits): [63:36] va>>12 (28b) | [35:24] pid
/// (12b) | [23:0] pfn (24b). The pfn is part of the value so lookups can
/// reject hash collisions (no false positives).
///
/// Returns the stamp it displaced (if any). A displaced stamp means the
/// frame reached the allocator (or was double-handed out) while still
/// tracked as LIVE for its previous owner — the callers report that as
/// the FILL-DOUBLE family (gnome-sigill).
pub fn text_stamp_insert(pfn: usize, pid: u32, va: usize) -> Option<(u32, usize)> {
    debug_assert!(pfn < (1 << 24));
    debug_assert!((pid as usize) < (1 << 12));
    let val = ((va >> 12) as u64 & 0x0FFF_FFFF) << 36
        | ((pid as u64 & 0xFFF) << 24)
        | (pfn as u64 & 0xFF_FFFF);
    let base = text_stamp_slot(pfn);
    let mut displaced: Option<(u32, usize)> = None;
    for i in 0..8 {
        let slot = (base + i) & (TEXT_STAMP_SLOTS - 1);
        let cur = TEXT_STAMPS[slot].load(Ordering::Relaxed);
        if cur == 0 {
            TEXT_STAMPS[slot].store(val, Ordering::Release);
            return displaced;
        }
        if (cur & 0xFF_FFFF) == (pfn as u64 & 0xFF_FFFF) {
            // Same frame re-filled: the previous stamp names who owned it
            // when it was last live — hand that back for the report.
            let p = ((cur >> 24) & 0xFFF) as u32;
            let v = (((cur >> 36) & 0x0FFF_FFFF) as usize) << 12;
            displaced = Some((p, v));
            TEXT_STAMPS[slot].store(val, Ordering::Release);
            return displaced;
        }
    }
    TEXT_STAMPS[base].store(val, Ordering::Release);
    displaced
}

/// Remove the stamp for `pfn` (frame is being legitimately released).
pub fn text_stamp_remove(pfn: usize) {
    let want = pfn as u64 & 0xFF_FFFF;
    for i in 0..8 {
        let slot = (text_stamp_slot(pfn) + i) & (TEXT_STAMP_SLOTS - 1);
        let val = TEXT_STAMPS[slot].load(Ordering::Relaxed);
        if val != 0 && val & 0xFF_FFFF == want {
            TEXT_STAMPS[slot].store(0, Ordering::Release);
            return;
        }
    }
}

/// Remove the stamp for `pfn` iff its current owner is `pid`
/// (gnome-sigill): used when `pid`'s MAPPING of the frame goes away
/// without a free — the COW resolver redirects the PTE to a private copy.
/// Without this the stamp survives as a false "foreign owner" and the
/// fork peer's later legal final free is refused as a leak.
pub fn text_stamp_remove_if_owner(pfn: usize, pid: u32) {
    if let Some((owner, _)) = text_stamp_get(pfn) {
        if owner == pid {
            text_stamp_remove(pfn);
        }
    }
}

// TextFrameEvent (the ring's record) and the PageDescStats snapshot are
// defined in mm/types.rs (Page itself, its flags and its type enum stay
// here — this file is Page's own). Re-exported so the
// `mm::page_desc::…` paths are unchanged.

impl TextFrameEvent {
    const fn new() -> Self {
        Self {
            pfn: AtomicU64::new(0),
            pid: AtomicU32::new(0),
            site: AtomicU64::new(0),
            refc: AtomicI32::new(0),
        }
    }
}
const TEXT_EVT_NEW: TextFrameEvent = TextFrameEvent::new();
pub static TEXT_EVT_RING: [TextFrameEvent; 8192] = [TEXT_EVT_NEW; 8192];
pub static TEXT_EVT_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// Record one lifecycle event for a stamped frame (no-op for unstamped
/// frames — the table IS the filter). `refc` is the refcount AFTER the
/// operation (0 when not applicable).
pub fn text_stamp_note(pfn: usize, pid: u32, site: char, refc: i32) {
    if text_stamp_get(pfn).is_none() {
        return;
    }
    let idx = TEXT_EVT_CURSOR.fetch_add(1, Ordering::Relaxed) % TEXT_EVT_RING.len();
    let e = &TEXT_EVT_RING[idx];
    e.pfn.store(pfn as u64, Ordering::Relaxed);
    e.pid.store(pid, Ordering::Relaxed);
    e.site.store(site as u64, Ordering::Relaxed);
    e.refc.store(refc, Ordering::Relaxed);
}

/// Look up the stamp for `pfn`; returns Some((pid, va_page)). The stored
/// pfn must match — a slot hit with a different pfn is a hash collision.
pub fn text_stamp_get(pfn: usize) -> Option<(u32, usize)> {
    let want = pfn as u64 & 0xFF_FFFF;
    for i in 0..8 {
        let slot = (text_stamp_slot(pfn) + i) & (TEXT_STAMP_SLOTS - 1);
        let val = TEXT_STAMPS[slot].load(Ordering::Acquire);
        if val == 0 {
            return None;
        }
        if val & 0xFF_FFFF == want {
            let pid = ((val >> 24) & 0xFFF) as u32;
            let va = (((val >> 36) & 0x0FFF_FFFF) as usize) << 12;
            return Some((pid, va));
        }
    }
    None
}

impl Page {
    /// Create new page descriptor (initialized to free state)
    pub const fn new() -> Self {
        Self {
            flags: PageFlags::new(),
            _mapcount: AtomicI32::new(PAGE_MAPCOUNT_BIAS),
            _refcount: AtomicI32::new(0),
            private: AtomicUsize::new(0),
            mapping: AtomicUsize::new(0),
            index: AtomicUsize::new(0),
            rmap_alt: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; 6]
            },
            _type: AtomicU32::new(PageType::Normal as u32),
            next_free: AtomicUsize::new(usize::MAX),  // FREE_LIST_NULL
            lru_next: AtomicUsize::new(0),
            lru_prev: AtomicUsize::new(0),
        }
    }

    /// Initialize as reserved page (kernel code, device memory, etc.)
    pub fn init_reserved(&self) {
        self.flags.set(PageFlag::Reserved);
        self._refcount.store(1, Ordering::Release);
    }

    /// Initialize as normal available page
    pub fn init_free(&self) {
        self.flags.clear_all();
        self._mapcount.store(PAGE_MAPCOUNT_BIAS, Ordering::Release);
        self._refcount.store(0, Ordering::Release);
        self.private.store(0, Ordering::Release);
        self.mapping.store(0, Ordering::Release);
        self.index.store(0, Ordering::Release);
        for slot in self.rmap_alt.iter() {
            slot.store(0, Ordering::Release);
        }
        self.lru_next.store(0, Ordering::Release);
        self.lru_prev.store(0, Ordering::Release);
        // R15-5 (probe false-positive root cause): init_free used to skip
        // next_free. Descriptor memory powers on as 0, so a page that had
        // never been on a freelist carried next_free=0 instead of
        // FREE_LIST_NULL — the TDF probe (next_free != MAX) then reported
        // every first release as a double free (idle 193 / nettest 483
        // hits were all false positives). Reset it here; the criterion is
        // reliable again.
        self.next_free.store(usize::MAX, Ordering::Release);
        self.flags.clear(PageFlag::OnFreelist);
    }

    // ========== Flag operations ==========

    /// Raw flag-set read (hp89 forensics: the flag word carries the
    /// frame's recent history — reclaim, COW, LRU, rmap state).
    #[inline]
    pub fn flags(&self) -> &PageFlags {
        &self.flags
    }

    /// Test flag
    #[inline]
    pub fn test_flag(&self, flag: PageFlag) -> bool {
        self.flags.test(flag)
    }

    /// Set flag
    #[inline]
    pub fn set_flag(&self, flag: PageFlag) {
        self.flags.set(flag);
    }

    /// Clear all flags
    #[inline]
    pub fn clear_all_flags(&self) {
        self.flags.clear_all();
    }

    /// Clear flag
    #[inline]
    pub fn clear_flag(&self, flag: PageFlag) {
        self.flags.clear(flag);
    }

    /// Test and set flag
    #[inline]
    pub fn test_and_set_flag(&self, flag: PageFlag) -> bool {
        self.flags.test_and_set(flag)
    }

    /// Test and clear flag
    #[inline]
    pub fn test_and_clear_flag(&self, flag: PageFlag) -> bool {
        self.flags.test_and_clear(flag)
    }

    /// Check if page is locked
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.test_flag(PageFlag::Locked)
    }

    /// Check if page is reserved
    #[inline]
    pub fn is_reserved(&self) -> bool {
        self.test_flag(PageFlag::Reserved)
    }

    /// Check if page is dirty
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.test_flag(PageFlag::Dirty)
    }

    /// Check if page is copy-on-write
    #[inline]
    pub fn is_cow(&self) -> bool {
        self.test_flag(PageFlag::Cow)
    }

    /// Check if page is anonymous
    #[inline]
    pub fn is_anonymous(&self) -> bool {
        self.test_flag(PageFlag::Anonymous)
    }

    /// Check if page data is valid
    #[inline]
    pub fn is_uptodate(&self) -> bool {
        self.test_flag(PageFlag::UpToDate)
    }

    // ========== Reference count operations ==========

    /// Get reference count
    #[inline]
    pub fn refcount(&self) -> i32 {
        self._refcount.load(Ordering::Acquire)
    }

    /// Increment reference count
    /// Returns the value after increment
    #[inline]
    pub fn get_page(&self) -> i32 {
        let r = self._refcount.fetch_add(1, Ordering::AcqRel) + 1;
        // mmforensics (early-put hunt): record the take on cache frames —
        // a no-op (one relaxed load) unless the switch is armed.
        crate::debug::mmforensics::note_ref(self as *const Page, 0, 0, r - 1);
        r
    }

    /// Decrement reference count atomically
    /// Returns the value after decrement; if it becomes 0, caller should free the page
    /// On underflow (refcount was already 0), returns -1 without modifying refcount
    #[inline]
    pub fn put_page(&self) -> i32 {
        loop {
            let old = self._refcount.load(Ordering::Acquire);
            if old <= 0 {
                // Would underflow — refuse to decrement
                // Note: intentionally no log here to avoid printk during page table
                // teardown, which can deadlock on mm/ring buffer/ext4 locks.
                crate::debug::driftwatch::PUT_UNDERFLOW
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                return -1;
            }
            match self._refcount.compare_exchange_weak(
                old,
                old - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return old - 1,
                Err(_) => continue, // CAS failed, retry
            }
        }
    }

    /// `put_page()` with drift attribution: a transition to 0 is the exact
    /// moment a frame becomes orphan-able — count it per call site
    /// (debug::driftwatch) so the MEMLEDGER census can attribute drift to the
    /// release path that dropped it. Same return contract as `put_page`.
    #[inline]
    pub fn put_page_site(&self, site: usize) -> i32 {
        let r = self.put_page();
        if r == 0 {
            crate::debug::driftwatch::note_zero_put(site);
        }
        // mmforensics (early-put hunt): record the drop on cache frames —
        // a no-op (one relaxed load) unless the switch is armed.
        crate::debug::mmforensics::note_ref(self as *const Page, 1, site as u8, r + 1);
        r
    }

    /// Try to increment reference count (only if refcount > 0)
    /// Returns true on success
    #[inline]
    pub fn try_get_page(&self) -> bool {
        loop {
            let old = self._refcount.load(Ordering::Acquire);
            if old <= 0 {
                return false;
            }
            match self._refcount.compare_exchange_weak(
                old,
                old + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// Set reference count (only for initialization)
    #[inline]
    pub fn set_refcount(&self, count: i32) {
        self._refcount.store(count, Ordering::Release);
    }

    // ========== Map count operations ==========

    /// Get map count (-1 means unmapped)
    #[inline]
    pub fn mapcount(&self) -> i32 {
        self._mapcount.load(Ordering::Acquire)
    }

    /// Increment map count
    /// Returns the value after increment
    #[inline]
    pub fn add_mapcount(&self) -> i32 {
        self._mapcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement map count
    /// Returns the value after decrement
    #[inline]
    pub fn sub_mapcount(&self) -> i32 {
        self._mapcount.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// Check if page is mapped
    #[inline]
    pub fn is_mapped(&self) -> bool {
        self._mapcount.load(Ordering::Acquire) > PAGE_MAPCOUNT_BIAS
    }

    /// Increment map count (alias for add_mapcount)
    #[inline]
    pub fn inc_mapcount(&self) -> i32 {
        self.add_mapcount()
    }

    /// Decrement map count (alias for sub_mapcount)
    #[inline]
    pub fn dec_mapcount(&self) -> i32 {
        self.sub_mapcount()
    }

    /// Reset map count
    #[inline]
    pub fn reset_mapcount(&self) {
        self._mapcount.store(PAGE_MAPCOUNT_BIAS, Ordering::Release);
    }

    // ========== Private data operations ==========

    /// Get private data
    #[inline]
    pub fn private(&self) -> usize {
        self.private.load(Ordering::Acquire)
    }

    /// Set private data
    #[inline]
    pub fn set_private(&self, value: usize) {
        self.private.store(value, Ordering::Release);
    }

    // ========== Mapping info operations ==========

    /// Get associated address_space
    #[inline]
    pub fn mapping(&self) -> *mut core::ffi::c_void {
        self.mapping.load(Ordering::Acquire) as *mut core::ffi::c_void
    }

    /// Set associated address_space
    #[inline]
    pub fn set_mapping(&self, mapping: *mut core::ffi::c_void) {
        self.mapping.store(mapping as usize, Ordering::Release);
    }

    /// Get page index
    #[inline]
    pub fn index(&self) -> usize {
        self.index.load(Ordering::Acquire)
    }

    /// Set page index
    #[inline]
    pub fn set_index(&self, index: usize) {
        self.index.store(index, Ordering::Release);
    }

    // ========== SIGILL text-frame stamps (gnome-sigill diagnostic) ==========
    //
    // See the module-level TEXT_STAMPS table below the Page impl for the
    // rationale; the accessors are free functions.

    // ========== Page type operations ==========

    /// Get page type
    #[inline]
    pub fn page_type(&self) -> PageType {
        match self._type.load(Ordering::Acquire) {
            0 => PageType::Normal,
            1 => PageType::Buddy,
            2 => PageType::Slab,
            3 => PageType::PageCache,
            4 => PageType::Anonymous,
            _ => PageType::Normal,
        }
    }

    /// Set page type
    #[inline]
    pub fn set_page_type(&self, page_type: PageType) {
        self._type.store(page_type as u32, Ordering::Release);
    }

    // ========== LRU operations ==========

    /// Get LRU next PFN (0 = end of list)
    #[inline]
    pub fn lru_next(&self) -> usize {
        self.lru_next.load(Ordering::Acquire)
    }

    /// Set LRU next PFN
    #[inline]
    pub fn set_lru_next(&self, pfn: usize) {
        self.lru_next.store(pfn, Ordering::Release);
    }

    /// Get LRU prev PFN (0 = list head / none)
    #[inline]
    pub fn lru_prev(&self) -> usize {
        self.lru_prev.load(Ordering::Acquire)
    }

    /// Set LRU prev PFN
    #[inline]
    pub fn set_lru_prev(&self, pfn: usize) {
        self.lru_prev.store(pfn, Ordering::Release);
    }

    /// Record which LRU list (0..NR_LRU_LISTS) the page is linked on.
    /// Only meaningful while PageFlag::Lru is set.
    #[inline]
    pub fn set_lru_list(&self, idx: usize) {
        self.flags.0.fetch_and(!LRU_LIST_MASK, Ordering::Release);
        self.flags.0
            .fetch_or(((idx as u32) & 0x7) << LRU_LIST_SHIFT, Ordering::Release);
    }

    /// Read the recorded LRU list index (0 if never set).
    #[inline]
    pub fn lru_list(&self) -> usize {
        ((self.flags.0.load(Ordering::Acquire) >> LRU_LIST_SHIFT) & 0x7) as usize
    }

    /// Clear the recorded LRU list index.
    #[inline]
    pub fn clear_lru_list(&self) {
        self.flags.0.fetch_and(!LRU_LIST_MASK, Ordering::Release);
    }

    // ========== rmap multi-mapping slots (review 4.10) ==========

    /// Record an additional (mm, virtual page number) mapping.
    ///
    /// Returns false when all slots are in use — the caller should then
    /// set `RmapOverflow` and fall back to the scan-based reverse map.
    /// Slots are NOT deduplicated per (mm, vpn); the rmap add path must
    /// not record the same mapping twice.
    pub fn rmap_alt_add(&self, mm: usize, vpn: usize) -> bool {
        for k in 0..3 {
            let mm_slot = &self.rmap_alt[2 * k];
            let vpn_slot = &self.rmap_alt[2 * k + 1];
            if mm_slot.load(Ordering::Acquire) == 0 {
                vpn_slot.store(vpn, Ordering::Release);
                mm_slot.store(mm, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Remove the slot matching (mm, vpn) if present.
    pub fn rmap_alt_remove(&self, mm: usize, vpn: usize) {
        for k in 0..3 {
            if self.rmap_alt[2 * k].load(Ordering::Acquire) == mm
                && self.rmap_alt[2 * k + 1].load(Ordering::Acquire) == vpn
            {
                self.rmap_alt[2 * k].store(0, Ordering::Release);
                self.rmap_alt[2 * k + 1].store(0, Ordering::Release);
                return;
            }
        }
    }

    /// Clear all alternate rmap slots.
    pub fn rmap_alt_clear(&self) {
        for slot in self.rmap_alt.iter() {
            slot.store(0, Ordering::Release);
        }
        self.flags.clear(PageFlag::RmapOverflow);
    }

    /// Call `f(mm, vpn)` for every recorded alternate mapping.
    pub fn rmap_alt_for_each<F: FnMut(usize, usize)>(&self, mut f: F) {
        for k in 0..3 {
            let mm = self.rmap_alt[2 * k].load(Ordering::Acquire);
            if mm != 0 {
                let vpn = self.rmap_alt[2 * k + 1].load(Ordering::Acquire);
                f(mm, vpn);
            }
        }
    }

    // ========== Free list operations (allocator internal use) ==========

    /// Get next free page's PFN
    #[inline]
    pub(crate) fn next_free(&self) -> usize {
        self.next_free.load(Ordering::Acquire)
    }

    /// Set next free page's PFN
    #[inline]
    pub(crate) fn set_next_free(&self, pfn: usize) {
        self.next_free.store(pfn, Ordering::Release);
    }

    // ========== Buddy allocator operations ==========

    /// Get order (stored in private field for buddy pages)
    #[inline]
    pub fn order(&self) -> u8 {
        (self.private.load(Ordering::Acquire) & 0xFF) as u8
    }

    /// Set order (stored in private field for buddy pages)
    #[inline]
    pub fn set_order(&self, order: u8) {
        self.private.store(order as usize, Ordering::Release);
    }

    /// Check if page is free (refcount == 0)
    #[inline]
    pub fn is_free(&self) -> bool {
        self._refcount.load(Ordering::Acquire) == 0
    }
}

// ========== Page content operations ==========

/// Copy contents of one physical page to another.
///
/// Both `src_pfn` and `dst_pfn` must be valid page frame numbers.
/// The pages must not overlap.
///
/// # Safety
/// - src_pfn and dst_pfn must be valid
/// - The source page must contain valid data
/// - The caller must ensure no concurrent writes to either page
pub unsafe fn copy_page_contents(src_pfn: usize, dst_pfn: usize) {
    use crate::arch::mm::memory_layout::{phys_to_virt, PhysAddr};
    // pfn_to_phys yields a physical address; it must go through the linear
    // mapping (phys_to_virt) before dereference — the raw PA has no mapping
    // once the MMU is on.
    let src = phys_to_virt(PhysAddr(super::hotpath::pfn_to_phys(src_pfn) as u64)).0 as usize as *const u8;
    let dst = phys_to_virt(PhysAddr(super::hotpath::pfn_to_phys(dst_pfn) as u64)).0 as usize as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
}

// ========== Global page array (mem_map) ==========

/// Physical memory constants
// Physical memory base: riscv64 QEMU virt RAM starts at 0x80000000; x86_64
// RAM starts at 0 (the PFN formulas below are base-relative by design).
#[cfg(feature = "riscv64")]
pub const PHYS_MEMORY_BASE: usize = 0x8000_0000;
#[cfg(feature = "aarch64")]
pub const PHYS_MEMORY_BASE: usize = 0x4000_0000;
#[cfg(feature = "x86_64")]
pub const PHYS_MEMORY_BASE: usize = 0x0000_0000;

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
//
// NOMINAL/default size only. The actual per-boot size comes from the
// hardware memory map (riscv64 FDT / x86_64 e820) and is published as the
// runtime descriptor count via set_nr_descs() — see nr_descs(). The config
// value remains the pre-vmemmap fallback.

/// Compile-time CEILING on managed page descriptors.
///
/// Fake-OOM family root cause (KVM OH rounds): MAX_PAGES used to be derived
/// from the Kernel.toml nominal size (2 GiB = 524288 pages), so on any
/// machine with more RAM (KVM q35 4 GiB: e820 reported 1048428 pages) both
/// vmemmap and the zone allocator silently TRUNCATED to the first 2 GiB —
/// every page above the line was invisible to the allocator while the boot
/// banner happily reported the full size. The allocator then "ran out" at
/// ~1.4 GiB usable on a 4 GiB box (allocator ENOMEM storm, fork=ENOMEM,
/// task-context ALLOCTHROW kills at PID 485+, foundation/accountmgr era).
///
/// MAX_PAGES now sizes only descriptor-backed STATIC structures (the
/// PT-ledger exact bitmaps: MAX_PAGES/64 * 8 B = 512 KiB) and the final
/// sanity clamp; the actual runtime bound is nr_descs(), set at boot from
/// the discovered memory map. A 16 GiB ceiling = 4 Mi descriptors = 256 MiB
/// of vmemmap backing in the worst case (Linux pays the same memmap cost).
pub const MAX_PAGES: usize = 16 * 1024 * 1024 * 1024 / PAGE_SIZE;

/// Runtime descriptor count (0 = not yet set by init_vmemmap).
static NR_DESCS: AtomicUsize = AtomicUsize::new(0);

/// Whether the runtime descriptor count has been published (init_vmemmap
/// done). The PT walk guards key on this so pre-init boot-time walks are
/// never vetoed by phys_valid's config-derived fallback (a machine with
/// more RAM than the config ceiling would otherwise fail walks above it).
pub fn descs_published() -> bool {
    NR_DESCS.load(Ordering::Acquire) != 0
}

/// Publish the per-boot descriptor count (clamped to MAX_PAGES).
/// Called once from init_vmemmap after the vmemmap mapping succeeds.
pub fn set_nr_descs(n: usize) {
    NR_DESCS.store(n.min(MAX_PAGES), Ordering::Release);
}

/// Number of page descriptors actually instantiated this boot.
///
/// Before init_vmemmap runs this returns the config-derived default; the
/// zone allocator, vmemmap bounds checks, and forensic scans all run after
/// init, so they see the discovered value.
#[inline]
pub fn nr_descs() -> usize {
    match NR_DESCS.load(Ordering::Acquire) {
        0 => PHYS_MEMORY_SIZE / PAGE_SIZE,
        n => n,
    }
}

/// Maximum Page Frame Number (nominal, config-derived; runtime users
/// should prefer max_pfn()).
pub const MAX_PFN: usize = (PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE) / PAGE_SIZE;

/// Runtime last-valid-PFN + 1 (base + discovered descriptors).
#[inline]
pub fn max_pfn() -> usize {
    MIN_PFN + nr_descs()
}

/// Minimum Page Frame Number (physical memory starts here)
pub const MIN_PFN: usize = PHYS_MEMORY_BASE / PAGE_SIZE;

/// Check if a Page Frame Number is valid (within managed physical memory)
#[inline]
pub fn pfn_valid(pfn: usize) -> bool {
    pfn >= MIN_PFN && pfn < MIN_PFN + nr_descs()
}

/// Check if a physical address is valid (within managed physical memory)
#[inline]
pub fn phys_valid(phys: usize) -> bool {
    phys >= PHYS_MEMORY_BASE && phys < PHYS_MEMORY_BASE + nr_descs() * PAGE_SIZE
}

/// Present (usable RAM) pages this boot — the SUM of the discovered
/// regions, holes excluded. Distinct from nr_descs(), which sizes the
/// linear PFN span (max region end) including non-RAM holes. MemTotal and
/// friends must use this (fake-OOM family: span-counted totals overstated
/// RAM by the MMIO-hole size on split-map machines).
static PRESENT_PAGES: AtomicUsize = AtomicUsize::new(0);

/// Publish the present-page count (called once from boot after the
/// memory map is parsed).
pub fn set_present_pages(n: usize) {
    PRESENT_PAGES.store(n, Ordering::Release);
}

/// Present (usable RAM) pages: nr_descs() if the boot never published a
/// count (contiguous-map fallback), else the sum of usable regions.
#[inline]
pub fn present_pages() -> usize {
    match PRESENT_PAGES.load(Ordering::Acquire) {
        0 => nr_descs(),
        n => n,
    }
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

/// Whether page array is initialized
static MEM_MAP_INIT: AtomicUsize = AtomicUsize::new(0);

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
pub fn init_mem_map(start_pfn: PhysFrameNr, nr_pages: usize) {
    // Prevent duplicate initialization
    if MEM_MAP_INIT.swap(1, Ordering::AcqRel) != 0 {
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

// ========== PFN <-> Page conversion (vmemmap-style) ==========

/// vmemmap base address for page descriptors
/// This is defined in arch/riscv64/mm/base.rs
pub const VMEMMAP_START: usize = crate::arch::mm::VMEMMAP_START;

/// PFN (Page Frame Number) to Page pointer
///
/// Uses vmemmap addressing:
///   page_addr = VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)
///
/// This is O(1).
///
/// # Safety
/// Caller must ensure pfn is in valid range
#[inline]
pub fn pfn_to_page(pfn: PhysFrameNr) -> *const Page {
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;

    // Check if pfn is in valid range
    if pfn < base_pfn {
        return core::ptr::null();
    }

    let idx = pfn - base_pfn;
    if idx >= nr_descs() {
        return core::ptr::null();
    }

    // vmemmap: VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)
    let vaddr = VMEMMAP_START + idx * core::mem::size_of::<Page>();
    vaddr as *const Page
}

/// PFN to mutable Page pointer
#[inline]
pub fn pfn_to_page_mut(pfn: PhysFrameNr) -> *mut Page {
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;

    // Check if pfn is in valid range
    if pfn < base_pfn {
        return core::ptr::null_mut();
    }

    let idx = pfn - base_pfn;
    if idx >= nr_descs() {
        return core::ptr::null_mut();
    }

    // vmemmap: VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)
    let vaddr = VMEMMAP_START + idx * core::mem::size_of::<Page>();
    vaddr as *mut Page
}

/// Page pointer to PFN
///
/// Uses vmemmap-style addressing:
///   idx = (page_addr - VMEMMAP_START) / sizeof(Page)
///   pfn = idx + base_pfn
///
/// # Safety
/// Caller must ensure page pointer is valid
#[inline]
pub fn page_to_pfn(page: *const Page) -> PhysFrameNr {
    let page_addr = page as usize;
    let idx = (page_addr - VMEMMAP_START) / core::mem::size_of::<Page>();
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;
    idx + base_pfn
}

/// Physical address to Page pointer
#[inline]
pub fn phys_to_page(phys: PhysAddr) -> *const Page {
    pfn_to_page(phys.frame_number())
}

/// Physical address to mutable Page pointer
#[inline]
pub fn phys_to_page_mut(phys: PhysAddr) -> *mut Page {
    pfn_to_page_mut(phys.frame_number())
}

/// Physical frame to Page pointer
#[inline]
pub fn frame_to_page(frame: PhysFrame) -> *const Page {
    pfn_to_page(frame.number)
}

/// Physical frame to mutable Page pointer
#[inline]
pub fn frame_to_page_mut(frame: PhysFrame) -> *mut Page {
    pfn_to_page_mut(frame.number)
}

// ========== Helper functions ==========

/// Get total number of pages (runtime descriptor count)
#[inline]
pub fn total_pages() -> usize {
    nr_descs()
}

/// Get Page structure size (bytes)
#[inline]
pub fn page_size() -> usize {
    core::mem::size_of::<Page>()
}

/// Get page descriptor statistics
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
#[inline]
pub unsafe fn get_page(pfn: PhysFrameNr) -> &'static Page {
    &*pfn_to_page(pfn)
}

/// Get mutable Page reference for a physical frame
///
/// # Safety
/// Caller must ensure pfn is in valid range and there are no other references
#[inline]
pub unsafe fn get_page_mut(pfn: PhysFrameNr) -> &'static mut Page {
    &mut *pfn_to_page_mut(pfn)
}

/// Safe shared view of a frame's page descriptor (unsafe-slim round 2).
///
/// INV-FRAME-1: the vmemmap descriptor array covers every physical frame
/// and is never reallocated; `Page` mutates only through atomics (the
/// whole struct is interior-mutable by construction), so a shared
/// `&'static Page` from a `&self` method is sound no matter how many
/// holders alias the same frame. This is the typed replacement for the
/// `let p = pfn_to_page_mut(pfn); if !p.is_null() { unsafe { (*p)... } }`
/// idiom — out-of-range pfns return None (the null check, lifted into
/// the type).
#[inline]
pub fn page_desc(pfn: PhysFrameNr) -> Option<&'static Page> {
    let p = pfn_to_page(pfn);
    if p.is_null() {
        return None;
    }
    // SAFETY: INV-FRAME-1 — vmemmap descriptors exist for the whole
    // machine lifetime; non-null check above mirrors pfn_to_page's
    // range validation.
    Some(unsafe { &*p })
}


// ════════════════ vma ═════════════════════════════════
// (was vma.rs)
// Virtual Memory Area Management - Platform-Independent Part
//
//
// VMA represents a contiguous virtual memory region in a process address space,
// with the same access permissions and mapping attributes.
//
// This module only contains platform-independent data structures:
// - VmaFlags: VMA flags
// - Vma: VMA struct
// - VmaManager: VMA manager
// - AddressSpace: Platform-independent address space abstraction
//
// Architecture-specific implementations (such as page table management,
// mmap/munmap/brk system calls) should be in arch/*/mm.rs
//
// # Safety Invariants — VMA Layout
//
// - **INV-VMA-1**: No two VMAs in the same address space overlap
//   (their `[start, end)` intervals are pairwise disjoint).
//
// - **INV-VMA-2**: VMAs are sorted by start address, enforced by the BTreeMap key.
//
// - **INV-VMA-3**: `max_end` (if maintained) equals `max(end)` across all VMAs
//   in the address space.
//
// - **INV-VMA-4**: Every VMA's `start` and `end` are `PAGE_SIZE`-aligned.

impl VmaFlags {
    /// Readable (VM_READ)
    pub const READ: u32 = 0x00000001;
    /// Writable (VM_WRITE)
    pub const WRITE: u32 = 0x00000002;
    /// Executable (VM_EXEC)
    pub const EXEC: u32 = 0x00000004;
    /// Shared mapping (VM_SHARED)
    pub const SHARED: u32 = 0x00000008;
    /// Private mapping (VM_PRIVATE)
    pub const PRIVATE: u32 = 0x00000010;
    /// May extend to heap (VM_GROWSDOWN)
    pub const GROWSDOWN: u32 = 0x00000100;
    /// May extend to stack (VM_GROWSUP)
    pub const GROWSUP: u32 = 0x00000200;
    /// Deny rmap (VM_DENYWRITE)
    pub const DENYWRITE: u32 = 0x00000800;
    /// Executable control/heap (VM_EXECUTABLE)
    pub const EXECUTABLE: u32 = 0x00001000;
    /// Locked memory (VM_LOCKED)
    pub const LOCKED: u32 = 0x00002000;
    /// I/O mapping (VM_IO)
    pub const IO: u32 = 0x00004000;
    /// Do not copy this VMA across fork (Linux VM_DONTCOPY,
    /// 0x04000000). The binder driver stamps this on its mmap so a
    /// forked child never inherits the parent's transaction buffers —
    /// the child must open /dev/binder itself (Linux binder_mmap sets
    /// VM_DONTCOPY unconditionally, which is what keeps forked OH
    /// appspawn children from sharing the daemon's binder_proc).
    pub const DONTFORK: u32 = 0x04000000;

    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }

    #[inline]
    pub fn contains(&self, flags: u32) -> bool {
        self.0 & flags == flags
    }

    #[inline]
    pub fn insert(&mut self, flags: u32) {
        self.0 |= flags;
    }

    #[inline]
    pub fn remove(&mut self, flags: u32) {
        self.0 &= !flags;
    }

    /// Check if readable
    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::READ != 0
    }

    /// Check if writable
    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::WRITE != 0
    }

    /// Check if executable
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::EXEC != 0
    }

    /// Check if shared
    #[inline]
    pub fn is_shared(&self) -> bool {
        self.0 & Self::SHARED != 0
    }

    /// Check if private
    #[inline]
    pub fn is_private(&self) -> bool {
        self.0 & Self::PRIVATE != 0
    }

    /// Convert to page permissions (Perm)
    ///
    /// Infer page table permissions from VMA flags
    ///
    /// Mapping:
    /// - No READ/WRITE/EXEC -> Perm::None
    /// - READ only -> Perm::Read
    /// - READ + WRITE -> Perm::ReadWrite
    /// - READ + WRITE + EXEC -> Perm::ReadWriteExec
    /// - READ + EXEC -> Perm::Read (no ReadExec option, use Read)
    /// - WRITE + EXEC -> Perm::ReadWrite (no WriteExec option, use ReadWrite)
    ///
    pub fn to_page_perm(&self) -> super::types::Perm {
        use super::types::Perm;

        let readable = self.is_readable();
        let writable = self.is_writable();
        let executable = self.is_executable();

        match (readable, writable, executable) {
            (false, false, false) => Perm::None,
            (true, false, false) => Perm::Read,
            (true, true, false) => Perm::ReadWrite,
            (true, true, true) => Perm::ReadWriteExec,
            (true, false, true) => Perm::ReadExec,    // Read + Execute
            (false, true, false) => Perm::ReadWrite,   // Write-only (unusual)
            (false, true, true) => Perm::ReadWriteExec, // Write + Execute
            (false, false, true) => Perm::Exec,         // Execute-only (Sv39 supports X=1,R=0)
        }
    }
}

impl Default for VmaFlags {
    fn default() -> Self {
        Self::new()
    }
}

impl VmaOrigin {
    /// Unstamped (default for paths not yet annotated).
    pub const UNKNOWN: u8 = 0;
    /// ELF loader: PT_LOAD / interpreter segments / fillers.
    pub const EXEC: u8 = 1;
    /// Main-thread stack created at exec.
    pub const EXEC_STACK: u8 = 2;
    /// mmap(2) install (fresh placement and MAP_FIXED replacement).
    pub const MMAP: u8 = 3;
    /// Covered piece of an mprotect(2) — the piece whose flags changed.
    pub const MPROTECT: u8 = 4;
    /// brk(2) heap growth.
    pub const BRK: u8 = 5;
    /// mremap(2) derived mapping.
    pub const MREMAP: u8 = 6;
    /// SysV shared-memory attach.
    pub const SHM: u8 = 7;
    /// binder(4) mmap (the transaction-buffer mapping).
    pub const BINDER: u8 = 8;
    /// Misc kernel-established mapping (io_uring, ...).
    pub const OTHER: u8 = 9;
    /// Modifier bit: this record was copied into a new mm by fork.
    pub const FORKED: u8 = 0x80;

    #[inline]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Stamp a creator (clears any previous creator, keeps FORKED).
    #[inline]
    pub fn set_creator(&mut self, creator: u8) {
        self.0 = (self.0 & Self::FORKED) | (creator & !Self::FORKED);
    }

    /// Mark this record as fork-copied (adds the modifier bit).
    #[inline]
    pub fn mark_forked(&mut self) {
        self.0 |= Self::FORKED;
    }

    #[inline]
    pub fn bits(&self) -> u8 {
        self.0
    }

    /// Creator name (without the fork modifier).
    #[inline]
    pub fn creator_name(&self) -> &'static str {
        match self.0 & !Self::FORKED {
            Self::UNKNOWN => "unknown",
            Self::EXEC => "exec",
            Self::EXEC_STACK => "exec-stack",
            Self::MMAP => "mmap",
            Self::MPROTECT => "mprotect",
            Self::BRK => "brk",
            Self::MREMAP => "mremap",
            Self::SHM => "shm",
            Self::BINDER => "binder",
            Self::OTHER => "other",
            _ => "?",
        }
    }

    /// True when the record was copied into this mm by fork.
    #[inline]
    pub fn is_forked(&self) -> bool {
        self.0 & Self::FORKED != 0
    }
}

impl Default for VmaOrigin {
    fn default() -> Self {
        Self(Self::UNKNOWN)
    }
}

impl Vma {
    /// Create new VMA
    pub fn new(start: VirtAddr, end: VirtAddr, flags: VmaFlags) -> Self {
        assert!(start.as_usize() < end.as_usize(), "Invalid VMA range");
        assert!(start.as_usize() % PAGE_SIZE == 0, "VMA start not page aligned");
        assert!(end.as_usize() % PAGE_SIZE == 0, "VMA end not page aligned");

        Self {
            start,
            end,
            flags,
            offset: 0,
            vma_type: VmaType::Anonymous,
            file_fd: -1,
            file_size: 0,
            origin: VmaOrigin::default(),
        }
    }

    /// Get start address
    #[inline]
    pub fn start(&self) -> VirtAddr {
        self.start
    }

    /// Get end address
    #[inline]
    pub fn end(&self) -> VirtAddr {
        self.end
    }

    /// Get VMA size (bytes)
    #[inline]
    pub fn size(&self) -> usize {
        self.end.as_usize() - self.start.as_usize()
    }

    /// Get VMA size (page count)
    #[inline]
    pub fn page_count(&self) -> usize {
        self.size() / PAGE_SIZE
    }

    /// Get flags
    #[inline]
    pub fn flags(&self) -> VmaFlags {
        self.flags
    }

    /// Set flags
    pub fn set_flags(&mut self, flags: VmaFlags) {
        self.flags = flags;
    }

    /// Get type
    #[inline]
    pub fn vma_type(&self) -> VmaType {
        self.vma_type
    }

    /// Set type
    pub fn set_type(&mut self, vma_type: VmaType) {
        self.vma_type = vma_type;
    }

    /// Check if address is within VMA range
    #[inline]
    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr.as_usize() >= self.start.as_usize() && addr.as_usize() < self.end.as_usize()
    }

    /// Check if two VMAs overlap
    pub fn overlaps(&self, other: &Vma) -> bool {
        self.start.as_usize() < other.end.as_usize()
            && other.start.as_usize() < self.end.as_usize()
    }

    /// Set file offset (for file mapping)
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
    }

    /// Get file offset
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Set file descriptor (for file-backed mapping)
    pub fn set_file_fd(&mut self, fd: i32) {
        self.file_fd = fd;
    }

    /// Get file descriptor
    #[inline]
    pub fn file_fd(&self) -> i32 {
        self.file_fd
    }

    /// Set file size (for file-backed mapping)
    pub fn set_file_size(&mut self, size: u64) {
        self.file_size = size;
    }

    /// Get file size
    #[inline]
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Get origin stamp (which path created this Vma record).
    #[inline]
    pub fn origin(&self) -> VmaOrigin {
        self.origin
    }

    /// Set origin stamp.
    #[inline]
    pub fn set_origin(&mut self, origin: VmaOrigin) {
        self.origin = origin;
    }

    /// Split VMA at specified address
    ///
    /// Returns (first half, second half) or None if address not in range
    pub fn split(&self, addr: VirtAddr) -> Option<(Vma, Vma)> {
        if !self.contains(addr) {
            return None;
        }

        // Ensure split address is page aligned
        let aligned_addr = VirtAddr::new(addr.as_usize() & !(PAGE_SIZE - 1));
        if aligned_addr.as_usize() <= self.start.as_usize()
            || aligned_addr.as_usize() >= self.end.as_usize()
        {
            return None;
        }

        let first = Vma {
            start: self.start,
            end: aligned_addr,
            flags: self.flags,
            offset: self.offset,
            vma_type: self.vma_type,
            file_fd: self.file_fd,
            file_size: self.file_size,
            origin: self.origin,
        };

        let second = Vma {
            start: aligned_addr,
            end: self.end,
            flags: self.flags,
            offset: self.offset + (aligned_addr.as_usize() - self.start.as_usize()),
            vma_type: self.vma_type,
            file_fd: self.file_fd,
            file_size: self.file_size,
            origin: self.origin,
        };

        Some((first, second))
    }

    /// Can merge with another VMA?
    ///
    /// Adjacent with identical attributes.  For anonymous VMAs the file
    /// offset is irrelevant — Linux (can_vma_merge_before/after) merges on
    /// offset *continuity* (`self.offset + size == other.offset` style),
    /// which for anonymous mappings is always satisfiable; requiring exact
    /// offset equality fragmented anonymous heaps (mmap chains after brk).
    /// For file-backed VMAs the offset must be continuous with the
    /// preceding mapping's extent (merging two mappings of DIFFERENT
    //  files/offsets corrupted reads through the first fd — review MM-H1).
    pub fn can_merge(&self, other: &Vma) -> bool {
        if self.end.as_usize() != other.start.as_usize() {
            return false;
        }
        if self.flags.bits() != other.flags.bits()
            || self.vma_type != other.vma_type
            || self.file_fd != other.file_fd
            || self.file_size != other.file_size
        {
            return false;
        }
        // Backing continuity: for file-backed mappings the new VMA must
        // start exactly where this one's file extent ends; anonymous
        // mappings have no backing, so any (contiguous) offset is fine.
        match self.vma_type {
            VmaType::Anonymous | VmaType::SharedMemory => true,
            _ => {
                let self_pages = (self.end.as_usize() - self.start.as_usize()) / PAGE_SIZE;
                self.offset + self_pages * PAGE_SIZE == other.offset
            }
        }
    }

    /// Merge with another VMA
    pub fn merge(&mut self, other: Vma) -> bool {
        if self.can_merge(&other) {
            self.end = other.end;
            true
        } else {
            false
        }
    }

    /// Extend end to cover an adjacent VMA (for forward merge in VmaManager::add)
    pub fn merge_at_end(&mut self, new_end: VirtAddr) {
        self.end = new_end;
    }
}

impl core::fmt::Debug for Vma {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Vma")
            .field("range", &format_args!("0x{:x}-0x{:x}", self.start.as_usize(), self.end.as_usize()))
            .field("size", &self.size())
            .field("flags", &self.flags)
            .field("type", &self.vma_type)
            .field("origin", &self.origin.creator_name())
            .finish()
    }
}

// VmaManager and the KernelAddressSpaceLayout names are defined in
// mm/types.rs (the Vma record and its flag/origin types stay here).
// Re-exported so the `mm::vma::…` paths are unchanged.

impl VmaManager {
    /// Create new VMA manager
    pub fn new() -> Self {
        Self {
            vmas: BTreeMap::new(),
            max_end: VirtAddr::new(0),
        }
    }

    /// Add VMA
    ///
    /// Follows Linux `vma_merge()`: the new VMA is merged with the
    /// PREDECESSOR and (chained) with any SUCCESSORS that are adjacent and
    /// compatible, so [A][new][B] collapses to a single VMA when all three
    /// are mergeable.  The old implementation merged at most one side.
    ///
    /// # Parameters
    /// - `vma`: VMA to add
    ///
    /// # Returns
    /// - `Ok(())`: Added successfully
    /// - `Err(VmaError::Overlap)`: Overlaps with existing VMA
    ///
    /// # Performance
    /// O(log n) overlap check + O(log n) insert + O(k log n) chain merge
    pub fn add(&mut self, vma: Vma) -> Result<(), VmaError> {
        // mmprof (cmdline mmprof=1): tree-add attempt counter (merges and
        // rejects visible from the delta against the other mmprof slots).
        if crate::debug::mmprof::on() {
            crate::debug::mmprof::bump(crate::debug::mmprof::S_VMAADD, 1);
        }
        let start = vma.start();
        let end = vma.end();

        // TRIPWIRE: inserting a VMA whose start key already exists used to
        // silently REPLACE the incumbent (BTreeMap insert) — two owners for
        // one range. Linux never adds an existing start; every caller
        // removes first. (heapstress/gnome-shell corruption hunt.) The add
        // is now REJECTED with Overlap by construction: the prev-merge
        // branch is skipped (has_vma_starting_within sees the incumbent's
        // key) and the overlap check below catches key == start — so this
        // line names the caller that tried, and its Err path is the
        // evidence to chase.
        if self.vmas.contains_key(&start) {
            static DUP_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if DUP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                crate::pr_err!(
                    "VMA-ADD-DUP start={:#x} end={:#x} pid={} — add will be REJECTED with Overlap (incumbent kept)",
                    start.as_usize(),
                    end.as_usize(),
                    crate::process::hotpath::current_pid()
                );
            }
        }

        // Check if previous VMA overlaps
        if let Some((_, prev_vma)) = self.vmas.range(..start).next_back() {
            if prev_vma.end().as_usize() > start.as_usize() {
                return Err(VmaError::Overlap);
            }

            // Try to merge with previous VMA (same flags, adjacent).
            // R7-5: the merge must not swallow a VMA starting inside
            // [start, end) — can_merge only compares prev and the new vma,
            // so an enclosed successor (different flags/backing) would be
            // silently overlapped (verify proptest: 0xf000-0x13000 vs
            // 0x12000-0x13000).
            if prev_vma.can_merge(&vma) && !self.has_vma_starting_within(start, end) {
                let prev_start = prev_vma.start();

                // Chain-merge successors of the incoming VMA (Linux
                // vma_merge merges prev + new + next in one pass).
                let mut merged = vma;
                loop {
                    let cur_end = merged.end();
                    let next = self.vmas.range(cur_end..).next().map(|(_, nv)| *nv);
                    match next {
                        Some(nv) if nv.start() == cur_end && merged.can_merge(&nv) => {
                            merged.merge_at_end(nv.end());
                            self.vmas.remove(&nv.start());
                        }
                        _ => break,
                    }
                }

                if let Some(prev) = self.vmas.get_mut(&prev_start) {
                    prev.merge_at_end(merged.end());
                    if prev.end().as_usize() > self.max_end.as_usize() {
                        self.max_end = prev.end();
                    }
                }
                return Ok(());
            }
        }

        // Reject overlaps: any VMA starting inside [start, end)
        if let Some((_, next_vma)) = self.vmas.range(start..=end).next() {
            if next_vma.start().as_usize() < end.as_usize() {
                return Err(VmaError::Overlap);
            }
        }

        // Fresh insert; chain-merge adjacent successors into it.
        let mut merged = vma;
        let mut swallowed = 0usize;
        loop {
            let cur_end = merged.end();
            let next = self.vmas.range(cur_end..).next().map(|(_, nv)| *nv);
            match next {
                Some(nv) if nv.start() == cur_end && merged.can_merge(&nv) => {
                    merged.merge_at_end(nv.end());
                    self.vmas.remove(&nv.start());
                    swallowed += 1;
                }
                _ => break,
            }
        }

        let final_end = merged.end();
        self.vmas.insert(start, merged);

        if final_end.as_usize() > self.max_end.as_usize() {
            self.max_end = final_end;
        }
        Ok(())
    }

    /// Find VMA containing specified address
    ///
    /// # Performance
    /// O(log n) using BTreeMap range lookup
    pub fn find(&self, addr: VirtAddr) -> Option<&Vma> {
        // Fast path: if address >= maximum end address, cannot find
        if addr.as_usize() >= self.max_end.as_usize() {
            return None;
        }

        // Use BTreeMap range lookup
        // Find largest VMA with start address <= addr
        // range(..=addr) returns all elements with key <= addr
        if let Some((_, vma)) = self.vmas.range(..=addr).next_back() {
            // Check if address is within this VMA range
            if vma.contains(addr) {
                return Some(vma);
            }
        }

        None
    }

    /// Find VMA containing specified address (mutable reference)
    pub fn find_mut(&mut self, addr: VirtAddr) -> Option<&mut Vma> {
        // Fast path
        if addr.as_usize() >= self.max_end.as_usize() {
            return None;
        }

        // Find VMA that may contain this address
        let start_addr = if let Some((&key, _)) = self.vmas.range(..=addr).next_back() {
            key
        } else {
            return None;
        };

        // Get mutable reference and check
        let vma = self.vmas.get_mut(&start_addr)?;
        if vma.contains(addr) {
            Some(vma)
        } else {
            None
        }
    }

    /// Remove VMA
    ///
    /// # Parameters
    /// - `start`: VMA start address
    pub fn remove(&mut self, start: VirtAddr) -> Result<(), VmaError> {
        if let Some(removed) = self.vmas.remove(&start) {
            // If removed VMA had maximum end address, need to recalculate
            if removed.end() == self.max_end {
                self.max_end = self.vmas.values()
                    .map(|v| v.end())
                    .max()
                    .unwrap_or(VirtAddr::new(0));
            }
            Ok(())
        } else {
            Err(VmaError::NotFound)
        }
    }

    /// Get iterator over all VMAs
    pub fn iter(&self) -> impl Iterator<Item = &Vma> {
        self.vmas.values()
    }

    /// Iterate VMAs whose start is strictly greater than `after`
    /// (leg-26 maps chunking: resume a chunked walk after dropping and
    /// re-taking the reader lock between batches).
    pub fn iter_after(
        &self,
        after: VirtAddr,
    ) -> impl Iterator<Item = &Vma> {
        use core::ops::Bound::{Excluded, Unbounded};
        self.vmas.range((Excluded(after), Unbounded)).map(|(_, v)| v)
    }

    /// Last VMA whose start is at or before `addr` — the candidate for
    /// the VMA *containing* `addr`. iter_after only yields VMAs that
    /// start after a key, so a walk that must begin coverage at `addr`
    /// (mprotect's range check) has to fetch the containing VMA
    /// separately or a strict sub-range of a larger VMA looks like a
    /// gap (geekbench's 8MB RWX arena carve died exactly there).
    pub fn last_at_or_before(&self, addr: VirtAddr) -> Option<&Vma> {
        use core::ops::Bound::{Included, Unbounded};
        self.vmas
            .range((Unbounded, Included(addr)))
            .next_back()
            .map(|(_, v)| v)
    }

    /// True if any VMA starts within [start, end) — used by add() to
    /// reject forward merges that would swallow a successor VMA (R7-5).
    fn has_vma_starting_within(&self, start: VirtAddr, end: VirtAddr) -> bool {
        self.vmas.range(start..end).next().is_some()
    }

    /// Get VMA count
    #[inline]
    pub fn count(&self) -> usize {
        self.vmas.len()
    }

    /// Find VMA at specified start address
    pub fn get(&self, start: VirtAddr) -> Option<&Vma> {
        self.vmas.get(&start)
    }

    /// Find VMA at specified start address (mutable reference)
    pub fn get_mut(&mut self, start: VirtAddr) -> Option<&mut Vma> {
        self.vmas.get_mut(&start)
    }

    /// Find first VMA
    pub fn first(&self) -> Option<&Vma> {
        self.vmas.values().next()
    }

    /// Find last VMA
    pub fn last(&self) -> Option<&Vma> {
        self.vmas.values().next_back()
    }

    /// Find first VMA with start address >= addr
    pub fn find_vma_after(&self, addr: VirtAddr) -> Option<&Vma> {
        self.vmas.range(addr..).next().map(|(_, vma)| vma)
    }

    /// Clear all VMAs
    pub fn clear(&mut self) {
        self.vmas.clear();
        self.max_end = VirtAddr::new(0);
    }

    /// Get maximum end address
    #[inline]
    pub fn max_end(&self) -> VirtAddr {
        self.max_end
    }

    /// Expand VMA downward (for stack expansion)
    ///
    /// This is used for stack growth when accessing an address below the current VMA start.
    /// Expand VMA downwards.
    ///
    /// # Arguments
    /// - `vma_start`: Start address of the VMA to expand
    /// - `new_start`: New start address (must be <= current start, page-aligned)
    ///
    /// # Returns
    /// - `Ok(())`: Expanded successfully
    /// - `Err(VmaError::NotFound)`: VMA not found at given start address
    /// - `Err(VmaError::Overlap)`: Would overlap with another VMA
    /// - `Err(VmaError::Invalid)`: Invalid new_start (not page aligned or above current start)
    pub fn expand_downwards(&mut self, vma_start: VirtAddr, new_start: VirtAddr) -> Result<(), VmaError> {
        // Validate new_start is page aligned
        if new_start.as_usize() % PAGE_SIZE != 0 {
            return Err(VmaError::Invalid);
        }

        // Get the VMA to expand
        let cur_start = {
            let vma = self.vmas.get(&vma_start).ok_or(VmaError::NotFound)?;
            // new_start must be below current start
            if new_start.as_usize() >= vma.start.as_usize() {
                return Err(VmaError::Invalid);
            }
            vma.start
        };

        // Full-interval overlap check (review 4.1): the extension covers
        // [new_start, cur_start) — the OLD code only looked at the VMA
        // preceding new_start, so a mapping living strictly INSIDE the
        // extension window was silently swallowed and the expanded stack
        // overlapped it.
        if self.has_vma_starting_within(new_start, cur_start) {
            return Err(VmaError::Overlap);
        }
        // The extension must also not be covered by a predecessor that
        // reaches into the window.
        if let Some((_, prev_vma)) = self.vmas.range(..new_start).next_back() {
            if prev_vma.end().as_usize() > new_start.as_usize() {
                // Would overlap with previous VMA
                return Err(VmaError::Overlap);
            }
        }

        // Remove old VMA from BTreeMap (keyed by old start)
        let mut vma = self.vmas.remove(&vma_start).ok_or(VmaError::NotFound)?;

        // Expand the VMA
        vma.start = new_start;

        // Re-insert with new key (new start address)
        self.vmas.insert(new_start, vma);

        Ok(())
    }

    /// Find stack VMA (VMA with GROWSDOWN flag) containing or below the address
    ///
    /// Lookup is O(log n) via the BTreeMap: the candidate is either the VMA
    /// containing `addr`, or — for a fault BELOW the stack bottom — the
    /// first VMA starting above `addr`.  The caller (page fault path)
    /// enforces the growth limit (`mm.stack_limit`, from RLIMIT_STACK);
    /// unlike the old 1-page window this allows growing to any depth below
    /// the current bottom until the limit or a collision — matching
    /// Linux's `expand_downwards()` behavior (guard-gap omitted).
    ///
    /// # Arguments
    /// - `addr`: Address to check
    ///
    /// # Returns
    /// - `Some((start_addr, vma))`: Stack VMA found, returns its start address for expansion
    /// - `None`: No stack VMA found
    pub fn find_stack_vma(&self, addr: VirtAddr) -> Option<(VirtAddr, &Vma)> {
        // Case 1: addr falls inside a VMA — return it if growable.
        if addr.as_usize() < self.max_end.as_usize() {
            if let Some((_, vma)) = self.vmas.range(..=addr).next_back() {
                if vma.contains(addr) && vma.flags().contains(VmaFlags::GROWSDOWN) {
                    return Some((vma.start(), vma));
                }
            }
        }

        // Case 2: addr is below the first VMA starting above it — that VMA
        // is the downward-growth candidate.
        if let Some((start, vma)) = self.vmas.range(addr..).next() {
            if vma.flags().contains(VmaFlags::GROWSDOWN) {
                return Some((*start, vma));
            }
        }

        None
    }
}

impl Default for VmaManager {
    fn default() -> Self {
        Self::new()
    }
}

// Send/Sync proofs live in mm::unsafe (same contracts the inline impls
// documented).


// ============================================================================
// Address Space Platform-Specific Parameter Interface
// ============================================================================

/// Address Space Platform-Specific Parameters
///
/// Different architectures need to provide their specific address space layout parameters
///
pub trait AddressSpaceLayout {
    /// User address space start address
    fn user_start() -> usize;

    /// User address space end address
    fn user_end() -> usize;

    /// Default stack size
    fn default_stack_size() -> usize;

    /// Default stack top (from user space top, going down)
    fn default_stack_top() -> usize;

    /// Heap start address
    fn heap_start() -> usize;

    /// Heap end address (maximum heap value)
    fn heap_end() -> usize;
}

// ============================================================================
// Address Space Layout Implementation
// ============================================================================

impl AddressSpaceLayout for KernelAddressSpaceLayout {
    /// User space start address (0, but null page protected)
    #[inline]
    fn user_start() -> usize {
        crate::arch::mm::user_addr::USER_START
    }

    /// User space end address = TASK_SIZE = 256GB for Sv39
    #[inline]
    fn user_end() -> usize {
        crate::arch::mm::user_addr::TASK_SIZE
    }

    /// Default stack size (8MB)
    #[inline]
    fn default_stack_size() -> usize {
        crate::arch::mm::user_addr::STACK_MAX_SIZE
    }

    /// Default stack top (TASK_SIZE, stack grows down)
    #[inline]
    fn default_stack_top() -> usize {
        crate::arch::mm::user_addr::STACK_TOP
    }

    /// Heap start address (brk default)
    #[inline]
    fn heap_start() -> usize {
        crate::arch::mm::user_addr::BRK_DEFAULT
    }

    /// Heap end address (maximum brk can grow to)
    #[inline]
    fn heap_end() -> usize {
        crate::arch::mm::user_addr::BRK_MAX
    }
}


// ════════════════ pagemap ═════════════════════════════
// (was pagemap.rs)
// Platform-agnostic Address Space Interface
//
// This module re-exports platform-specific AddressSpace implementations:
// - RISC-V: arch/riscv64/mm.rs
//
// High-level VMA operations (brk, mmap, munmap) are provided in platform implementations

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

// Platform-specific AddressSpace re-export (canonical type: crate::mm)

// Re-export common types

// VMA related types


// Implement From<VmaError> to support ? operator
impl From<VmaError> for MapError {
    fn from(err: VmaError) -> Self {
        match err {
            VmaError::Overlap => MapError::AlreadyMapped,
            VmaError::NoSpace => MapError::OutOfMemory,
            VmaError::NotFound => MapError::NotMapped,
            VmaError::Invalid => MapError::Invalid,
        }
    }
}


// ════════════════ slab ════════════════════════════════
// (was slab.rs)
// Slab Allocator
//
// Efficient memory allocation for small objects, reducing buddy allocator fragmentation.
//
// # Design
// - SlabCache: Cache for managing specific size objects
// - Slab: Memory pages containing multiple same-size objects
// - kmalloc/kfree: Public allocation interface
//
// # Supported Object Sizes
// 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 bytes

/// Minimum object size (8 bytes)
const MIN_OBJECT_SIZE: usize = 8;

/// Maximum object size (one page)
const MAX_OBJECT_SIZE: usize = PAGE_SIZE;


impl SlabHeader {
    const fn new() -> Self {
        Self {
            cache_idx: 0,
            object_size: 0,
            total_objects: 0,
            free_objects: 0,
            free_index: 0,
            next: 0,
            prev: 0,
            _pad: 0,
        }
    }
}

impl SlabCache {
    /// Create new slab cache
    pub const fn new(object_size: usize) -> Self {
        // Calculate objects per slab
        // Reserve header space
        let header_size = core::mem::size_of::<SlabHeader>();
        let usable_size = PAGE_SIZE - header_size;
        let objects_per_slab = if object_size == 0 {
            0
        } else {
            usable_size / object_size
        };

        Self {
            object_size,
            objects_per_slab,
            free_list: 0,
            partial_list: 0,
            full_list: 0,
            alloc_count: AtomicUsize::new(0),
            free_count: AtomicUsize::new(0),
        }
    }

    /// Allocate an object from cache
    pub fn alloc(&mut self, slab_pages: &SlabPages) -> *mut u8 {
        // Prioritize allocation from partial list
        if self.partial_list != 0 {
            let ptr = self.alloc_from_slab(self.partial_list, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        // If partial is empty, allocate from free list
        if self.free_list != 0 {
            let slab_idx = self.free_list;
            // Move slab from free to partial
            self.free_list = slab_pages.get_next(slab_idx);
            if self.free_list != 0 {
                slab_pages.set_prev(self.free_list, 0);
            }
            slab_pages.set_prev(slab_idx, 0);
            slab_pages.set_next(slab_idx, 0);
            self.partial_list = slab_idx;

            let ptr = self.alloc_from_slab(slab_idx, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        // No available slab, need to create new one
        if let Some(slab_idx) = self.create_slab(slab_pages) {
            self.partial_list = slab_idx;
            let ptr = self.alloc_from_slab(slab_idx, slab_pages);
            if !ptr.is_null() {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                return ptr;
            }
        }

        core::ptr::null_mut()
    }

    /// Allocate object from specified slab
    fn alloc_from_slab(&mut self, slab_idx: u16, slab_pages: &SlabPages) -> *mut u8 {
        let header = slab_pages.get_header_mut(slab_idx);

        if header.free_objects == 0 {
            return core::ptr::null_mut();
        }

        // Get free object index
        let obj_idx = header.free_index;

        // Calculate object address (checked to prevent usize overflow)
        let header_size = core::mem::size_of::<SlabHeader>();
        let obj_offset = match header_size.checked_add(
            (obj_idx as usize).checked_mul(self.object_size).unwrap_or(usize::MAX)
        ) {
            Some(off) => off,
            None => return core::ptr::null_mut(),
        };
        let page_addr = slab_pages.get_page_addr(slab_idx);
        let obj_ptr = (page_addr + obj_offset) as *mut u8;

        // Update header info
        // Read next free index (stored in object memory)
        // obj_ptr points within a slab page allocated from SlabPages;
        // we hold the cache lock so no concurrent alloc/free on this
        // cache (INV-MM-4).
        let next_free = if self.object_size >= 2 {
            super::r#unsafe::slab_read_freelist_link(obj_ptr)
        } else {
            obj_idx + 1
        };

        // Validate free-list index: detect corruption by checking the value is
        // either the sentinel (0xFFFF = end-of-list) or a valid object index.
        let max_objects = (PAGE_SIZE - header_size) / self.object_size;
        if next_free != 0xFFFF && next_free as usize >= max_objects {
            // free-list corrupted — treat slab as full. Do NOT return the
            // out-of-range object (review 4.19: returning the pointer hands
            // the caller memory past the slab's object array); fail instead.
            header.free_objects = 0;
            self.move_slab_to_full(slab_idx, slab_pages);
            return core::ptr::null_mut();
        }

        header.free_index = next_free;
        header.free_objects -= 1;

        // Check if slab is now full
        if header.free_objects == 0 {
            // Move from partial to full
            self.move_slab_to_full(slab_idx, slab_pages);
        }

        obj_ptr
    }

    /// Free object to cache
    ///
    /// Returns false (and leaves the cache untouched) when the pointer does
    /// not name a valid object of THIS cache (review 4.19): the object index
    /// must land inside the slab's object array, be object-aligned, and the
    /// slab header must declare this cache's object size. Without these
    /// checks a kfree fallback loop linked foreign pointers into the wrong
    /// size class' freelist.
    pub fn free(&mut self, ptr: *mut u8, slab_pages: &SlabPages) -> bool {
        // Find slab containing the object
        let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
        let slab_idx = match slab_pages.find_slab_by_addr(page_addr) {
            Some(idx) => idx,
            None => return false,
        };

        let header = slab_pages.get_header_mut(slab_idx);

        // Validate cache index
        if header.cache_idx as usize >= NUM_CACHES {
            return false;
        }

        // Cache-ownership check: the slab must belong to this size class.
        // header.object_size is written by create_slab() and kmalloc().
        if header.object_size as usize != self.object_size {
            return false;
        }

        // Calculate object index (guarding the header region)
        let header_size = core::mem::size_of::<SlabHeader>();
        let off = ptr as usize;
        if off < page_addr + header_size {
            // Pointer into the slab header — never a valid object.
            return false;
        }
        let obj_offset = off - page_addr - header_size;

        // Alignment: objects start at header_size + i * object_size.
        if obj_offset % self.object_size != 0 {
            return false;
        }

        let obj_idx_u = obj_offset / self.object_size;
        // Bounds: index must be inside this slab's object array (also
        // rejects pointers past the last object, e.g. into the next page).
        if obj_idx_u >= header.total_objects as usize {
            return false;
        }
        let obj_idx = obj_idx_u as u16;

        // Double-free guard: freeing into a slab that already reports every
        // object free would corrupt the freelist head chain.
        if header.free_objects >= header.total_objects {
            return false;
        }

        // Write object index to object memory (as free list)
        // ptr points within a slab page; we hold the cache lock
        // (INV-MM-4).
        if self.object_size >= 2 {
            super::r#unsafe::slab_write_freelist_link(ptr, header.free_index);
        }

        header.free_index = obj_idx;
        header.free_objects += 1;

        self.free_count.fetch_add(1, Ordering::Relaxed);

        // Check slab state change
        let was_full = header.free_objects == 1;
        let is_empty = header.free_objects == header.total_objects;

        if was_full {
            // Move from full to partial
            self.move_slab_from_full(slab_idx, slab_pages);
        } else if is_empty {
            // Move from partial to free (optional: release slab)
            // Keep in partial for now to avoid frequent create/destroy
        }

        true
    }

    /// Create new slab
    fn create_slab(&mut self, slab_pages: &SlabPages) -> Option<u16> {
        // Guard: if objects_per_slab is 0, this cache cannot hold any objects.
        if self.objects_per_slab == 0 {
            return None;
        }

        // Allocate a page from buddy allocator
        let page = slab_pages.alloc_page()?;

        // Initialize slab header
        let header = slab_pages.get_header_mut(page);
        header.cache_idx = 0; // Will be set after return
        header.object_size = self.object_size as u16;
        header.total_objects = self.objects_per_slab as u16;
        header.free_objects = self.objects_per_slab as u16;
        header.free_index = 0;
        header.next = 0;
        header.prev = 0;

        // Initialize free list (each object stores next free index)
        let header_size = core::mem::size_of::<SlabHeader>();
        let page_addr = slab_pages.get_page_addr(page);

        for i in 0..self.objects_per_slab - 1 {
            let obj_offset = header_size + i * self.object_size;
            let obj_ptr = (page_addr + obj_offset) as *mut u16;
            // obj_ptr is within a freshly allocated slab page; no
            // concurrent access — this slab is not yet in any list
            // (INV-MM-4 fresh-slab arm).
            super::r#unsafe::slab_write_freelist_link(obj_ptr as *mut u8, (i + 1) as u16);
        }

        // Last object's next is 0xFFFF (list end marker)
        if self.objects_per_slab > 0 {
            let last_offset = header_size + (self.objects_per_slab - 1) * self.object_size;
            let last_ptr = (page_addr + last_offset) as *mut u16;
            super::r#unsafe::slab_write_freelist_link(last_ptr as *mut u8, 0xFFFF);
        }

        Some(page)
    }

    /// Move slab from partial to full
    fn move_slab_to_full(&mut self, slab_idx: u16, slab_pages: &SlabPages) {
        // Remove from partial list
        let next = slab_pages.get_next(slab_idx);
        let prev = slab_pages.get_prev(slab_idx);

        if prev == 0 {
            self.partial_list = next;
        } else {
            slab_pages.set_next(prev, next);
        }

        if next != 0 {
            slab_pages.set_prev(next, prev);
        }

        // Add to full list head
        slab_pages.set_next(slab_idx, self.full_list);
        slab_pages.set_prev(slab_idx, 0);
        if self.full_list != 0 {
            slab_pages.set_prev(self.full_list, slab_idx);
        }
        self.full_list = slab_idx;
    }

    /// Move slab from full to partial
    fn move_slab_from_full(&mut self, slab_idx: u16, slab_pages: &SlabPages) {
        // Remove from full list
        let next = slab_pages.get_next(slab_idx);
        let prev = slab_pages.get_prev(slab_idx);

        if prev == 0 {
            self.full_list = next;
        } else {
            slab_pages.set_next(prev, next);
        }

        if next != 0 {
            slab_pages.set_prev(next, prev);
        }

        // Add to partial list head
        slab_pages.set_next(slab_idx, self.partial_list);
        slab_pages.set_prev(slab_idx, 0);
        if self.partial_list != 0 {
            slab_pages.set_prev(self.partial_list, slab_idx);
        }
        self.partial_list = slab_idx;
    }
}

impl SlabPages {
    /// Create new slab pages manager
    pub const fn new(base_addr: usize, max_pages: usize) -> Self {
        Self {
            base_addr,
            allocated_pages: AtomicUsize::new(0),
            max_pages,
        }
    }

    /// Get base address
    pub fn base_addr(&self) -> usize {
        self.base_addr
    }

    /// Get max pages
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// Allocate a new page
    fn alloc_page(&self) -> Option<u16> {
        let idx = self.allocated_pages.fetch_add(1, Ordering::AcqRel);
        if idx >= self.max_pages {
            self.allocated_pages.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some((idx + 1) as u16) // Use 1-based index, 0 means null
    }

    /// Get page address
    fn get_page_addr(&self, idx: u16) -> usize {
        self.base_addr + (idx as usize - 1) * PAGE_SIZE
    }

    /// Get slab header
    fn get_header_mut(&self, idx: u16) -> &mut SlabHeader {
        let addr = self.get_page_addr(idx);
        // addr is page-aligned within the slab region; caller holds
        // the cache lock ensuring exclusive access to this slab
        // (INV-MM-4).
        super::r#unsafe::slab_header_mut(addr)
    }

    /// Get next slab
    fn get_next(&self, idx: u16) -> u16 {
        self.get_header_mut(idx).next
    }

    /// Set next slab
    fn set_next(&self, idx: u16, next: u16) {
        self.get_header_mut(idx).next = next;
    }

    /// Get previous slab
    fn get_prev(&self, idx: u16) -> u16 {
        self.get_header_mut(idx).prev
    }

    /// Set previous slab
    fn set_prev(&self, idx: u16, prev: u16) {
        self.get_header_mut(idx).prev = prev;
    }

    /// Find slab by address
    fn find_slab_by_addr(&self, addr: usize) -> Option<u16> {
        if addr < self.base_addr {
            return None;
        }
        let offset = addr - self.base_addr;
        if offset >= self.max_pages * PAGE_SIZE {
            return None;
        }
        let idx = (offset / PAGE_SIZE) as u16 + 1;
        if idx as usize > self.allocated_pages.load(Ordering::Acquire) {
            return None;
        }
        Some(idx)
    }
}

/// The allocator's `pages` cell. The state doc (which fields are `Sync`
/// and why `SLAB_ALLOCATOR` can be a plain `static`) sits on
/// `SlabAllocator` in mm/types.rs.
pub(super) fn slab_pages_cell(
) -> &'static core::cell::UnsafeCell<SlabPages> {
    &SLAB_ALLOCATOR.pages
}

/// Static Slab allocator instance.
static SLAB_ALLOCATOR: SlabAllocator = SlabAllocator {
    caches: [
        Spinlock::new(SlabCache::new(8)),
        Spinlock::new(SlabCache::new(16)),
        Spinlock::new(SlabCache::new(32)),
        Spinlock::new(SlabCache::new(64)),
        Spinlock::new(SlabCache::new(128)),
        Spinlock::new(SlabCache::new(256)),
        Spinlock::new(SlabCache::new(512)),
        Spinlock::new(SlabCache::new(1024)),
        Spinlock::new(SlabCache::new(2048)),
        Spinlock::new(SlabCache::new(4096)),
    ],
    pages: core::cell::UnsafeCell::new(SlabPages::new(0, 0)),
    initialized: AtomicUsize::new(0),
};

impl SlabAllocator {
    /// Initialize Slab allocator.
    ///
    /// # Safety
    /// `init()` must be called exactly once, before any concurrent `kmalloc` /
    /// `kfree` calls.  It writes through a raw pointer to initialize the
    /// `SlabPages` fields that are immutable afterwards.  No other CPU may
    /// access `SLAB_ALLOCATOR` concurrently during this call.
    pub fn init(base_addr: usize, max_pages: usize) {
        // init() runs once during boot before secondary CPUs start and
        // before any kmalloc/kfree call (INV-MM-4 init arm).
        super::r#unsafe::slab_pages_init(base_addr, max_pages);
        SLAB_ALLOCATOR.initialized.store(1, Ordering::Release);
    }

    /// Check if initialized
    fn is_initialized() -> bool {
        SLAB_ALLOCATOR.initialized.load(Ordering::Acquire) == 1
    }

    /// Get shared reference to pages (safe after init).
    fn pages() -> &'static SlabPages {
        // Pages are only written once during init(); after init all
        // accesses are reads or atomic operations (INV-MM-4 shared arm).
        super::r#unsafe::slab_pages()
    }

    /// Find appropriate cache index
    pub(crate) fn find_cache_index(size: usize) -> Option<usize> {
        if size == 0 || size > MAX_OBJECT_SIZE {
            return None;
        }

        for (i, &obj_size) in OBJECT_SIZES.iter().enumerate() {
            if size <= obj_size {
                return Some(i);
            }
        }
        None
    }
}

/// Allocate memory
///
/// # Arguments
/// - `size`: Requested memory size
///
/// # Returns
/// Memory pointer on success, null on failure
/// rux:allocwatch (retired debug=allocwatch) probe: live lock-word state of every slab cache lock
/// (bit i = caches[i] held at sample time); see debug/allocwatch.rs.
pub fn audit_cache_locks() -> u16 {
    let mut m = 0u16;
    for (i, c) in SLAB_ALLOCATOR.caches.iter().enumerate() {
        if c.is_locked() {
            m |= 1 << i;
        }
    }
    m
}

pub fn kmalloc(size: usize) -> *mut u8 {
    if !SlabAllocator::is_initialized() {
        return core::ptr::null_mut();
    }

    // Find appropriate cache
    let cache_idx = match SlabAllocator::find_cache_index(size) {
        Some(idx) => idx,
        None => return core::ptr::null_mut(),
    };

    // Use lock_irqsave: kmalloc can be called from interrupt context,
    // and an interrupt firing while we hold the slab lock would self-deadlock
    // if the handler also allocates memory.
    let mut cache = SLAB_ALLOCATOR.caches[cache_idx].lock_irqsave();
    // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
    let _aw = crate::debug::allocwatch::guard(
        crate::debug::allocwatch::SITE_SLAB,
        cache_idx as u8,
    );
    let ptr = cache.alloc(SlabAllocator::pages());
    if !ptr.is_null() {
        // Store cache_idx in slab header for O(1) kfree lookup
        let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
        let header = page_addr as *mut SlabHeader;
        // page_addr is page-aligned and within the slab region; we
        // hold the cache lock (INV-MM-4).
        super::r#unsafe::slab_header_mut(page_addr).cache_idx = cache_idx as u8;
    }
    ptr
}

/// Free memory
///
/// # Arguments
/// - `ptr`: Memory pointer to free
pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() || !SlabAllocator::is_initialized() {
        return;
    }

    // O(1) lookup: read cache_idx from slab header
    let page_addr = (ptr as usize) & !(PAGE_SIZE - 1);
    let base = SlabAllocator::pages().base_addr();
    let max_pages = SlabAllocator::pages().max_pages();
    let slab_end = base + max_pages * PAGE_SIZE;

    // Check if pointer is within slab region
    if page_addr >= base && page_addr < slab_end {
        let header = page_addr as *const SlabHeader;
        // page_addr is within the slab region (checked above); the slab
        // header was written by kmalloc and is stable (INV-MM-4).
        let idx = super::r#unsafe::slab_header_cache_idx(page_addr) as usize;
        if idx < NUM_CACHES {
            let mut cache = SLAB_ALLOCATOR.caches[idx].lock_irqsave();
            if cache.free(ptr, SlabAllocator::pages()) {
                return;
            }
        }
    }

    // Fallback: linear search for non-slab pointers or corrupted header
    for i in 0..NUM_CACHES {
        let mut cache = SLAB_ALLOCATOR.caches[i].lock_irqsave();
        // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
        let _aw = crate::debug::allocwatch::guard(
            crate::debug::allocwatch::SITE_SLAB,
            i as u8,
        );
        if cache.free(ptr, SlabAllocator::pages()) {
            return;
        }
    }
}

/// Allocate and zero memory
///
/// # Arguments
/// - `size`: Requested memory size
///
/// # Returns
/// Zeroed memory pointer on success, null on failure
pub fn kzalloc(size: usize) -> *mut u8 {
    let ptr = kmalloc(size);
    if !ptr.is_null() {
        // ptr was returned by kmalloc, size matches the allocation
        // (INV-MM-4 owned arm).
        super::r#unsafe::slab_zero(ptr, size);
    }
    ptr
}

/// Initialize Slab allocator
///
/// # Arguments
/// - `base_addr`: Slab memory region start address
/// - `size`: Slab memory region size
pub fn init_slab(base_addr: usize, size: usize) {
    let max_pages = size / PAGE_SIZE;
    SlabAllocator::init(base_addr, max_pages);
}

/// Check if Slab allocator is initialized
pub fn is_slab_initialized() -> bool {
    SlabAllocator::is_initialized()
}

/// Get the slab region's actual bounds as configured by init_slab()
/// — (base_addr, base + max_pages * PAGE_SIZE), or None before init.
///
/// Consumers that must distinguish slab pointers from heap pointers
/// (CombinedAllocator::dealloc) should use this instead of guessing
/// "heap_end + hardcoded 4MB" (review 4.8: dual-source drifted).
pub fn slab_region() -> Option<(usize, usize)> {
    if !SlabAllocator::is_initialized() {
        return None;
    }
    let pages = SlabAllocator::pages();
    let base = pages.base_addr();
    let end = base + pages.max_pages() * PAGE_SIZE;
    Some((base, end))
}

/// Get Slab statistics
pub fn slab_stats() -> SlabStats {
    let mut stats = SlabStats::default();

    if !SlabAllocator::is_initialized() {
        return stats;
    }

    // Use lock_irqsave: interrupts must be disabled while holding slab locks.
    // A timer interrupt firing mid-lock can invoke kmalloc (which uses
    // lock_irqsave on the same cache), causing a self-deadlock on the
    // current CPU since the IRQ handler spins forever waiting for the
    // lock held by the preempted context.
    for i in 0..NUM_CACHES {
        let mut cache = SLAB_ALLOCATOR.caches[i].lock_irqsave();
        stats.cache_stats[i] = CacheStats {
            object_size: cache.object_size,
            alloc_count: cache.alloc_count.load(Ordering::Relaxed),
            free_count: cache.free_count.load(Ordering::Relaxed),
        };
    }
    stats.total_pages = SlabAllocator::pages().allocated_pages.load(Ordering::Relaxed);

    stats
}


// ════════════════ meminfo ═════════════════════════════
// (was meminfo.rs)
// Kernel Memory Statistics
//
// Provides memory statistics functionality similar to /proc/meminfo to track system memory usage.
//
// # Statistics Content
// - Physical memory usage (Frame Allocator)
// - Heap memory usage (Buddy Allocator)
// - Slab allocator usage
// - Per-CPU Pages cache
// - Page descriptor status

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

impl Default for MemoryInfo {
    fn default() -> Self {
        Self {
            mem_total: 0,
            mem_free: 0,
            mem_available: 0,
            mem_used: 0,
            heap_total: 0,
            heap_used: 0,
            heap_free: 0,
            slab_pages: 0,
            slab_allocs: 0,
            slab_frees: 0,
            pcp_pages: [0; crate::config::MAX_CPUS],
            pages_free: 0,
            pages_used: 0,
            pages_reserved: 0,
            pages_mapped: 0,
            pages_dirty: 0,
            pages_cow: 0,
            pages_anon: 0,
        }
    }
}

impl MemoryInfo {
    /// Format as human-readable string
    pub fn format(&self) -> MemoryInfoFormatter {
        MemoryInfoFormatter { info: self }
    }
}

// The MemorySummary snapshot and the Display helper are defined in
// mm/types.rs (MemoryInfo itself stays here — this file is its own).
// Re-exported so the `mm::meminfo::…` paths are unchanged.

impl<'a> core::fmt::Display for MemoryInfoFormatter<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "Memory Info:")?;
        writeln!(f, "  MemTotal:       {:>10} kB ({} MB)", self.info.mem_total / 1024, self.info.mem_total / 1024 / 1024)?;
        writeln!(f, "  MemFree:        {:>10} kB ({} MB)", self.info.mem_free / 1024, self.info.mem_free / 1024 / 1024)?;
        writeln!(f, "  MemAvailable:   {:>10} kB ({} MB)", self.info.mem_available / 1024, self.info.mem_available / 1024 / 1024)?;
        writeln!(f, "  MemUsed:        {:>10} kB ({} MB)", self.info.mem_used / 1024, self.info.mem_used / 1024 / 1024)?;
        writeln!(f)?;
        writeln!(f, "  HeapTotal:      {:>10} kB ({} MB)", self.info.heap_total / 1024, self.info.heap_total / 1024 / 1024)?;
        writeln!(f, "  HeapUsed:       {:>10} kB ({} MB)", self.info.heap_used / 1024, self.info.heap_used / 1024 / 1024)?;
        writeln!(f, "  HeapFree:       {:>10} kB ({} MB)", self.info.heap_free / 1024, self.info.heap_free / 1024 / 1024)?;
        writeln!(f)?;
        writeln!(f, "  SlabPages:      {:>10} pages", self.info.slab_pages)?;
        writeln!(f, "  SlabAllocs:     {:>10}", self.info.slab_allocs)?;
        writeln!(f, "  SlabFrees:      {:>10}", self.info.slab_frees)?;
        writeln!(f)?;
        writeln!(f, "  PCP Pages:      CPU0={} CPU1={} CPU2={} CPU3={}",
            self.info.pcp_pages[0], self.info.pcp_pages[1],
            self.info.pcp_pages[2], self.info.pcp_pages[3])?;
        writeln!(f)?;
        writeln!(f, "  PagesFree:      {:>10}", self.info.pages_free)?;
        writeln!(f, "  PagesUsed:      {:>10}", self.info.pages_used)?;
        writeln!(f, "  PagesReserved:  {:>10}", self.info.pages_reserved)?;
        writeln!(f, "  PagesMapped:    {:>10}", self.info.pages_mapped)?;
        writeln!(f, "  PagesDirty:     {:>10}", self.info.pages_dirty)?;
        writeln!(f, "  PagesCOW:       {:>10}", self.info.pages_cow)?;
        writeln!(f, "  PagesAnon:      {:>10}", self.info.pages_anon)
    }
}

/// Get basic memory statistics (O(1), for hot paths like /proc/meminfo)
///
/// Only computes mem_total/mem_free/mem_available/mem_used from the zone
/// allocator's nr_free counter. Skips expensive buddy_stats, slab_stats,
/// and page_desc_stats scans.
pub fn get_basic_memory_info() -> MemoryInfo {
    let mut info = MemoryInfo::default();

    // Runtime total: the discovered USABLE RAM (present pages), not the
    // Kernel.toml nominal size and not the PFN-span descriptor count —
    // MemTotal must not lie on machines whose RAM differs from the config
    // value (fake-OOM family: the 4G box reported 2G while claiming to
    // manage all of it) nor on split maps where the span includes MMIO
    // holes (span-counted totals overstated RAM by the hole size).
    let total = super::hotpath::present_pages() * PAGE_SIZE;
    if let Some(node) = first_online_node() {
        let free_pages = node.free_pages();
        info.mem_free = free_pages * PAGE_SIZE;
        info.mem_available = free_pages * PAGE_SIZE;
        info.mem_total = total;
        info.mem_used = info.mem_total.saturating_sub(info.mem_free);
    } else {
        info.mem_total = total;
        info.mem_free = 0;
        info.mem_used = info.mem_total;
        info.mem_available = 0;
    }

    info
}

/// Get complete memory statistics (includes expensive full scans)
pub fn get_memory_info() -> MemoryInfo {
    let mut info = get_basic_memory_info();

    // Heap memory statistics
    let buddy_stats = buddy_stats();
    info.heap_total = buddy_stats.heap_size;
    info.heap_used = buddy_stats.used_bytes;
    info.heap_free = buddy_stats.free_bytes;

    // Slab statistics
    let slab_stats = slab_stats();
    info.slab_pages = slab_stats.total_pages;
    info.slab_allocs = slab_stats.cache_stats.iter().map(|c| c.alloc_count).sum();
    info.slab_frees = slab_stats.cache_stats.iter().map(|c| c.free_count).sum();

    // Per-CPU Pages: real counts from the pcp rows (INV-PCP-2: these
    // frames are already included in the zone free_pages counters).
    info.pcp_pages = super::hotpath::cached_pages_per_cpu();

    // Page descriptor statistics
    let page_stats = page_desc_stats();
    info.pages_free = page_stats.free_pages;
    info.pages_used = page_stats.used_pages;
    info.pages_reserved = page_stats.reserved_pages;
    info.pages_mapped = page_stats.mapped_pages;
    info.pages_dirty = page_stats.dirty_pages;
    info.pages_cow = page_stats.cow_pages;
    info.pages_anon = page_stats.anonymous_pages;

    info
}

/// Print memory statistics
pub fn print_memory_info() {
    let info = get_memory_info();
    crate::println!("{}", info.format());
}

/// Get memory usage summary
pub fn get_memory_summary() -> MemorySummary {
    let info = get_basic_memory_info();

    let total_mb = info.mem_total / 1024 / 1024;
    let used_mb = info.mem_used / 1024 / 1024;
    let free_mb = info.mem_free / 1024 / 1024;

    let heap_usage_percent = if info.heap_total > 0 {
        info.heap_used * 100 / info.heap_total
    } else {
        0
    };

    MemorySummary {
        total_mb,
        used_mb,
        free_mb,
        heap_usage_percent,
    }
}

/// Check if memory is low (for OOM warning)
pub fn is_memory_low() -> bool {
    let info = get_basic_memory_info();

    // If free memory is less than 5% of total, consider memory low
    if info.mem_total > 0 {
        info.mem_free * 100 / info.mem_total < 5
    } else {
        false
    }
}

/// Check if OOM should be triggered
pub fn should_trigger_oom() -> bool {
    let info = get_basic_memory_info();

    // If free memory is less than 1% of total, trigger OOM
    if info.mem_total > 0 {
        info.mem_free * 100 / info.mem_total < 1
    } else {
        false
    }
}


// ════════════════ mm_struct ═══════════════════════════
// (was mm_struct.rs)
// Memory Descriptor
//
// This module implements the mm_struct abstraction for describing process address spaces.
//
// Key fields:
// - pgd: Page table base address
// - mmap: VMA manager
// - start_code/end_code: Code segment range
// - start_data/end_data: Data segment range
// - start_brk/brk: Heap region
// - start_stack: Stack start address
// - arg_start/arg_end: Command line arguments
// - env_start/env_end: Environment variables
// - total_vm: Total virtual memory page count
// - locked_vm: Locked memory page count
//
// # Architecture Design
//
// `MmStruct` is a platform-independent data structure containing all mm_struct fields.
// Architecture-specific operations (such as page table mapping) are implemented through:
//
// 1. Platform-independent methods defined in `impl MmStruct` (e.g., field accessors)
// 2. Architecture-specific methods added in `arch/*/mm.rs` via extension traits or impl blocks
//
// This design follows a layered architecture:
// - mm_struct is the generic memory descriptor
// - Architecture-specific pte/pmd/pud/p4d/pgd operations are implemented in arch directory

// AT_VECTOR_SIZE, GlobalPteLock, PteLock, PteGuard, PteIrqGuard, MmStruct,
// MmFlags and AddressSpace — the definitions live in mm/types.rs; every
// impl and function that uses them stays in this file. Re-exported so the
// existing `mm::mm_struct::…` paths are unchanged.

// ============================================================================
// Leaf-PTE exclusion (INV-LOCK-7 — see docs/architecture/lock-ordering.md)
// ============================================================================

impl GlobalPteLock {
    pub const fn new() -> Self {
        Self { raw: RwSpinlock::new(()) }
    }

    /// Whole-domain exclusive PTE section (the old global lock).
    #[inline]
    pub fn lock(&self) -> crate::sync::rwlock::RwSpinlockWriteGuard<'_, ()> {
        self.raw.write()
    }

    /// Whole-domain exclusive PTE section, IRQs saved.
    #[inline]
    pub fn lock_irqsave(&self) -> crate::sync::rwlock::RwSpinlockIrqWriteGuard<'_, ()> {
        self.raw.write_irqsave()
    }
}

impl PteLock {
    pub const fn new() -> Self {
        Self { spin: Spinlock::new(()) }
    }

    /// Per-mm PTE section: global epoch SHARED + this mm's spin.
    /// Writers of this mm serialize on the spin; writers of other mms
    /// run concurrently (shared epoch); global-mode writers still
    /// exclude (they need the epoch exclusive).
    ///
    /// Split mode on ALL architectures since the 2026-10-31 perf review:
    /// riscv64 (the primary target) previously degraded to the global
    /// exclusive section, serializing every PTE writer of every mm
    /// system-wide — demand faults, COW faults, brk, unmap and the fork
    /// walk all queued on one spinlock. The arch hooks that still need
    /// whole-domain exclusion (map_user_region, foreign IPC installs)
    /// keep calling GlobalPteLock::lock* directly.
    #[inline]
    pub fn lock(&self) -> PteGuard<'_> {
        let read = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.raw.read();
        let spin = self.spin.lock();
        PteGuard { _spin: spin, _epoch: read }
    }

    /// `lock()` with IRQs saved for the whole section.
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> PteIrqGuard<'_> { // track_caller: syscost site attribution
        let read = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.raw.read_irqsave();
        let spin = self.spin.lock_irqsave();
        PteIrqGuard { _spin: spin, _epoch: read }
    }
}

/// PTE-section handle for page-table trees that have NO owning MmStruct:
/// exec's not-yet-published error-path guard tree and the synthetic test
/// trees. Those trees have no concurrent mm-side writers by construction;
/// the handle still gives the teardown chunks a real section (and on
/// non-x86_64, the full global exclusion).
pub static FRESH_TREE_PTE_LOCK: PteLock = PteLock::new();

impl MmFlags {
    /// Core dump in progress
    pub const MMF_DUMP_CORE: u64 = 0x00000001;
    /// Skip shared mappings
    pub const MMF_DUMP_SKIP_SHARED: u64 = 0x00000002;
    /// Skip private mappings
    pub const MMF_DUMP_SKIP_PRIVATE: u64 = 0x00000004;
    /// Dumped
    pub const MMF_DUMPED: u64 = 0x00000008;
    /// OOM notification disabled
    pub const MMF_OOM_DISABLE: u64 = 0x00000010;
    /// OOM score adjustment
    pub const MMF_OOM_SCORE_ADJ: u64 = 0x00000020;
}

impl MmStruct {
    /// Create a new memory descriptor
    ///
    /// # Arguments
    /// - `pgd`: Page table root PPN
    /// - `space_type`: Address space type
    ///
    /// Contract: the caller must ensure `pgd` points to a valid page
    /// table (the constructor only records it — no dereference happens
    /// here; the old unsafe marker was vestigial).
    pub fn new(pgd: u64, space_type: PageTableType) -> Self {
        use super::types::RiscVAddressSpaceLayout;
        use super::hotpath::AddressSpaceLayout;

        // Teardown-race guard (mm/deferred_free.rs): register this
        // struct's size class so its heap frees are time-deferred — an
        // unaccounted &MmStruct borrower preempted mid-syscall must not
        // resume on heap-reused memory. The size that matters is the ARC
        // ALLOCATION's: alloc::sync::Arc<MmStruct> frees a block holding
        // ArcInner { strong, weak, MmStruct } — 16 bytes of refcount
        // header before the struct (rounding to the 8-byte header align).
        // Registering bare size_of::<MmStruct>() matched nothing and the
        // freed slot was reused as string data within microseconds
        // (exitwalk: PT-WALK-STALE roots spelled ASCII). First writer
        // wins; both sizes are compile-time constants, so idempotent.
        super::hotpath::register_deferred_size(
            (2 * core::mem::size_of::<usize>()) + core::mem::size_of::<MmStruct>(),
        );

        // Leg-17 leak census: creation-side twin of memwatch::MM_DROPS —
        // both gated on User spaces so live_mm = MM_NEWS - MM_DROPS.
        if space_type == PageTableType::User {
            crate::debug::memwatch::MM_NEWS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }

        let vma_manager = VmaManager::new();
        let brk_default = if space_type == PageTableType::User {
            super::types::RiscVAddressSpaceLayout::heap_start()
        } else {
            0
        };

        let mmap_base = if space_type == PageTableType::User {
            // mmap region starts after heap region
            super::types::RiscVAddressSpaceLayout::heap_end()
        } else {
            0
        };

        Self {
            pgd,
            asid: AtomicU16::new(0),  // Will be allocated on first use
            pte_lock: PteLock::new(),
            mm_surgery: Rwsem::new("mm-surgery", ()),
            vma_manager: Rwsem::new("vma-lock", vma_manager),
            vma_files: RwSpinlock::new(alloc::collections::BTreeMap::new()),
            space_type,
            // Segment ranges
            start_code: AtomicUsize::new(0),
            end_code: AtomicUsize::new(0),
            start_data: AtomicUsize::new(0),
            end_data: AtomicUsize::new(0),
            // Heap management
            start_brk: AtomicUsize::new(brk_default),
            brk: AtomicUsize::new(brk_default),
            brk_lock: Spinlock::new(()),
            // Stack management
            start_stack: AtomicUsize::new(0),
            stack_limit: AtomicUsize::new(0),
            // Arguments and environment variables
            arg_start: AtomicUsize::new(0),
            arg_end: AtomicUsize::new(0),
            env_start: AtomicUsize::new(0),
            env_end: AtomicUsize::new(0),
            saved_auxv: RwSpinlock::new([0; AT_VECTOR_SIZE]),
            // Virtual memory statistics
            total_vm: AtomicU64::new(0),
            locked_vm: AtomicU64::new(0),
            pinned_vm: AtomicU64::new(0),
            data_vm: AtomicU64::new(0),
            exec_vm: AtomicU64::new(0),
            stack_vm: AtomicU64::new(0),
            rss: AtomicU64::new(0),
            // mmap region
            mmap_base: AtomicUsize::new(mmap_base),
            mmap_legacy_base: AtomicUsize::new(mmap_base),
            highest_vm_end: AtomicUsize::new(0),
            // Reference counting
            mm_users: AtomicI32::new(1),
            mm_count: AtomicI32::new(1),
            // Other
            flags: AtomicU64::new(0),
            owner_pid: AtomicI32::new(-1),
            exe_text_key: Spinlock::new(None),
            cpu_mask: AtomicU64::new(0),
        }
    }

    /// Create a shared page table memory descriptor (for fork)
    pub fn new_shared(pgd: u64, space_type: PageTableType, brk: VirtAddr) -> Self {
        let mut mm = Self::new(pgd, space_type);
        mm.start_brk.store(brk.as_usize(), Ordering::Release);
        mm.brk.store(brk.as_usize(), Ordering::Release);
        mm
    }

    /// Create a memory descriptor with specified type
    ///
    /// This is an alias for `new()`, providing an interface compatible with the old `AddressSpace::new_with_type`
    pub fn new_with_type(pgd: u64, space_type: PageTableType) -> Self {
        Self::new(pgd, space_type)
    }

    /// Create kernel address space (convenience method)
    pub fn new_kernel(pgd: u64) -> Self {
        Self::new(pgd, PageTableType::Kernel)
    }

    /// Create user address space (convenience method)
    pub fn new_user(pgd: u64) -> Self {
        Self::new(pgd, PageTableType::User)
    }

    /// Exec commit (do_execve success on THIS mm): record the executed
    /// image's text hold. The caller has already taken the hold
    /// (fs::locks::inode_text_hold_acquire_key); while it is live a
    /// write-mode open/creat of the image fails ETXTBSY (reference
    /// deny_write_access held via mm->exe_file; LTP creat07). Re-exec
    /// overwrites: the previous mm (with its hold) is dropped separately.
    pub fn set_exe_text_key(&self, key: (u64, u64)) {
        *self.exe_text_key.lock() = Some(key);
    }

    /// copy_mm (fork without CLONE_VM): the child's mm duplicates the
    /// exe text reference — reference dup_mm's get_file(mm->exe_file) —
    /// so the write denial lives until the last fork descendant exits.
    pub fn inherit_exe_text(&self, parent: &MmStruct) {
        if let Some(key) = *parent.exe_text_key.lock() {
            crate::fs::locks::inode_text_hold_acquire_key(key);
            *self.exe_text_key.lock() = Some(key);
        }
    }

    // ==================== Basic Accessors ====================

    /// Get page table root PPN
    #[inline]
    pub fn pgd(&self) -> u64 {
        self.pgd
    }

    /// Get page table root PPN (compatible alias)
    #[inline]
    pub fn root_ppn(&self) -> u64 {
        self.pgd
    }

    /// Get ASID (Address Space ID)
    #[inline]
    pub fn asid(&self) -> u16 {
        self.asid.load(Ordering::Acquire)
    }

    /// Set ASID
    #[inline]
    pub fn set_asid(&self, asid: u16) {
        self.asid.store(asid, Ordering::Release);
    }

    /// Record that THIS cpu is about to load (or has loaded) this mm's
    /// root — the TLB-shootdown mask (see `cpu_mask`). Must be called
    /// BEFORE the CR3 write so a concurrent shootdown that has already
    /// cleared PTEs cannot miss this CPU (see field doc).
    #[inline]
    pub fn note_tlb_cpu(&self, cpu: usize) {
        if cpu >= 64 {
            return;
        }
        self.cpu_mask.fetch_or(1u64 << cpu, Ordering::Release);
    }

    /// Drop THIS cpu from the shootdown mask on switching AWAY from the
    /// mm (the mask's lazy half, x86_64 PCID-off discipline). Valid ONLY
    /// when the switch-away's CR3 write flushes the whole TLB (PCID off:
    /// every plain CR3 write is a full flush) — with PCID on the caller
    /// must NOT clear (cached entries survive the switch under the mm's
    /// PCID tag; the grow-only mask stays conservative there).
    ///
    /// Ordering (why a concurrently-faulting CPU cannot be missed):
    /// - clear-side: the clear runs in context_switch with IRQs off,
    ///   BEFORE the outgoing CR3 write; no user translation can occur
    ///   between them, and the write itself flushes every entry.
    /// - set-side: `note_tlb_cpu` precedes the incoming CR3 write, and
    ///   MOV-to-CR3 is a serializing instruction (SDM Vol 3 §8.3), so
    ///   the bit-set is globally performed before any entry the load
    ///   caches exists; a flush whose PTE-clear precedes its Acquire
    ///   mask read therefore sees every CPU that cached old entries.
    #[inline]
    pub fn clear_tlb_cpu(&self, cpu: usize) {
        if cpu >= 64 {
            return;
        }
        self.cpu_mask.fetch_and(!(1u64 << cpu), Ordering::Release);
    }

    /// Snapshot of the ever-loaded CPU mask for shootdown filtering.
    /// Callers that have just cleared PTEs read this with Acquire: every
    /// CPU whose CR3 write preceded the clear is guaranteed present.
    #[inline]
    pub fn tlb_cpu_mask(&self) -> u64 {
        self.cpu_mask.load(Ordering::Acquire)
    }

    /// Allocate ASID for this address space
    /// Returns the allocated ASID
    ///
    /// Atomic (review 4.2): the old check-then-set let two CPUs racing on
    /// the same mm both observe `asid == 0`, both draw from the arch pool
    /// and the loser's store leaked its ASID permanently (pool exhaustion).
    /// Now a compare_exchange decides the single winner; the loser returns
    /// the already-installed value and frees its speculative allocation.
    pub fn alloc_asid(&self) -> Option<u16> {
        // Only user address spaces need ASIDs
        if self.space_type != PageTableType::User {
            return Some(0);  // Kernel uses ASID 0
        }

        // Fast path: already allocated
        let current = self.asid.load(Ordering::Acquire);
        if current != 0 {
            return Some(current);
        }

        // Draw a candidate, then CAS 0 -> candidate. Only the winner keeps it.
        let asid = crate::arch::mm::alloc_asid()?;
        match self.asid.compare_exchange(0, asid, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Some(asid),
            Err(winner) => {
                // Lost the race: another thread installed an ASID first.
                crate::arch::mm::free_asid(asid);
                Some(winner)
            }
        }
    }

    /// Free ASID for this address space
    pub fn free_asid(&self) {
        let asid = self.asid.swap(0, Ordering::AcqRel);
        if asid != 0 && self.space_type == PageTableType::User {
            crate::arch::mm::free_asid(asid);
        }
    }

    // ==================== Per-mm PTE Lock ====================

    /// The mm's leaf-PTE exclusion lock (INV-LOCK-7).
    ///
    /// A PTE section = global epoch SHARED + this spinlock (x86_64 split
    /// mode): two writers of the SAME mm serialize here, writers of
    /// DIFFERENT mms run concurrently, and the remaining global-mode
    /// users (ipc installs, kernel-root/unpublished-tree maps, every
    /// riscv64 site) hold the epoch EXCLUSIVE so they still exclude
    /// against everyone. See docs/architecture/lock-ordering.md.
    #[inline]
    pub fn pte_lock(&self) -> &PteLock {
        &self.pte_lock
    }

    /// Whole-syscall mm-surgery section (see the field doc): acquired by
    /// the mm syscall layer around multi-step surgery so sibling
    /// CLONE_VM threads cannot interleave into the gaps.
    ///
    /// oh-crash 91st: this is a SLEEPABLE Rwsem write section, not a
    /// Spinlock. The Spinlock shape was illegal by this crate's own
    /// rules (spinlock.rs: "a Spinlock section must stay short by
    /// construction" / "every lock that may be held across blocking is
    /// a SleepMutex, never a Spinlock"): every mm syscall holds
    /// mm_surgery across vma_manager read/write acquisition, and the
    /// vma rwsem PARKS on contention — so the surgery holder slept
    /// while 7 sibling CPUs spun on mm_surgery non-preemptibly, waking
    /// nobody. hp91a (pristine userdata, first boot): all-CPU spin at
    /// 51.5s on the surgery lock + the vma wait_lock, six PREEMPT-HEAL
    /// events un-stranding parked threads carrying count=0x10001 (the
    /// parked-with-the-surgery-preempt-disable shape), launcher never
    /// spawns. As the reference mmap_lock(write) role, the correct
    /// primitive is a rwsem: waiters park (no CPU burn, no wake-IPI
    /// starvation), and the grant machinery is the same one
    /// vma_manager already uses inside a Box'd MmStruct.
    pub fn mm_surgery_guard(
        &self,
    ) -> crate::sync::rwsem::RwsemWriteGuard<'_, ()> {
        self.mm_surgery.write()
    }

    /// Get address space type
    #[inline]
    pub fn space_type(&self) -> PageTableType {
        self.space_type
    }

    // ==================== Segment Range Accessors ====================

    /// Get code segment start address
    #[inline]
    pub fn start_code(&self) -> usize {
        self.start_code.load(Ordering::Acquire)
    }

    /// Set code segment start address
    #[inline]
    pub fn set_start_code(&self, addr: usize) {
        self.start_code.store(addr, Ordering::Release);
    }

    /// Get code segment end address
    #[inline]
    pub fn end_code(&self) -> usize {
        self.end_code.load(Ordering::Acquire)
    }

    /// Set code segment end address
    #[inline]
    pub fn set_end_code(&self, addr: usize) {
        self.end_code.store(addr, Ordering::Release);
    }

    /// Get data segment start address
    #[inline]
    pub fn start_data(&self) -> usize {
        self.start_data.load(Ordering::Acquire)
    }

    /// Set data segment start address
    #[inline]
    pub fn set_start_data(&self, addr: usize) {
        self.start_data.store(addr, Ordering::Release);
    }

    /// Get data segment end address
    #[inline]
    pub fn end_data(&self) -> usize {
        self.end_data.load(Ordering::Acquire)
    }

    /// Set data segment end address
    #[inline]
    pub fn set_end_data(&self, addr: usize) {
        self.end_data.store(addr, Ordering::Release);
    }

    // ==================== Heap Management ====================

    /// Get heap start address
    #[inline]
    pub fn start_brk(&self) -> usize {
        self.start_brk.load(Ordering::Acquire)
    }

    /// Set heap start address
    #[inline]
    pub fn set_start_brk(&self, addr: usize) {
        self.start_brk.store(addr, Ordering::Release);
    }

    /// Get current brk value
    ///
    /// EXACT bytes, never page-masked: the reference kernel keeps
    /// mm->brk at the byte address the last brk() request named (pages
    /// are rounded at fault time), and brk(2) must read back exactly
    /// what it stored — LTP brk01 sets the break to pagesize*2-1 past
    /// the current one and fails the test on any masked read-back
    /// (VirtAddr::new masks low bits; the tuple constructor does not).
    #[inline]
    pub fn brk(&self) -> VirtAddr {
        VirtAddr(self.brk.load(Ordering::Acquire))
    }

    /// Set brk value
    #[inline]
    pub fn set_brk_val(&self, addr: usize) {
        self.brk.store(addr, Ordering::Release);
    }

    /// Per-mm break lock (GNOME trim face, 2026-11-07): serializes the
    /// whole sys_brk read-modify-write. Held OUTERMOST (before the
    /// mm-surgery section and the VMA write lock); nothing takes
    /// brk_lock while holding those.
    #[inline]
    pub fn brk_lock(&self) -> &Spinlock<()> {
        &self.brk_lock
    }

    // ==================== Stack Management ====================

    /// Get stack start address
    #[inline]
    pub fn start_stack(&self) -> usize {
        self.start_stack.load(Ordering::Acquire)
    }

    /// Set stack start address
    #[inline]
    pub fn set_start_stack(&self, addr: usize) {
        self.start_stack.store(addr, Ordering::Release);
    }

    /// Get stack limit (minimum stack address)
    #[inline]
    pub fn stack_limit(&self) -> usize {
        self.stack_limit.load(Ordering::Acquire)
    }

    /// Set stack limit (minimum stack address)
    #[inline]
    pub fn set_stack_limit(&self, addr: usize) {
        self.stack_limit.store(addr, Ordering::Release);
    }

    // ==================== Arguments and Environment Variables ====================

    /// Get command line arguments start address
    #[inline]
    pub fn arg_start(&self) -> usize {
        self.arg_start.load(Ordering::Acquire)
    }

    /// Set command line arguments start address
    #[inline]
    pub fn set_arg_start(&self, addr: usize) {
        self.arg_start.store(addr, Ordering::Release);
    }

    /// Get command line arguments end address
    #[inline]
    pub fn arg_end(&self) -> usize {
        self.arg_end.load(Ordering::Acquire)
    }

    /// Set command line arguments end address
    #[inline]
    pub fn set_arg_end(&self, addr: usize) {
        self.arg_end.store(addr, Ordering::Release);
    }

    /// Get environment variables start address
    #[inline]
    pub fn env_start(&self) -> usize {
        self.env_start.load(Ordering::Acquire)
    }

    /// Set environment variables start address
    #[inline]
    pub fn set_env_start(&self, addr: usize) {
        self.env_start.store(addr, Ordering::Release);
    }

    /// Get environment variables end address
    #[inline]
    pub fn env_end(&self) -> usize {
        self.env_end.load(Ordering::Acquire)
    }

    /// Set environment variables end address
    #[inline]
    pub fn set_env_end(&self, addr: usize) {
        self.env_end.store(addr, Ordering::Release);
    }

    // ==================== Virtual Memory Statistics ====================

    /// Get total virtual memory page count
    #[inline]
    pub fn total_vm(&self) -> u64 {
        self.total_vm.load(Ordering::Acquire)
    }

    /// Add to total virtual memory page count
    #[inline]
    pub fn add_total_vm(&self, pages: u64) {
        self.total_vm.fetch_add(pages, Ordering::AcqRel);
    }

    /// Subtract from total virtual memory page count
    ///
    /// Saturating at 0 (like sub_rss): the counter is an accounting
    /// mirror of the live mapping, and an accounting gap must never wrap
    /// it around — a wrapped counter would make every later mapping look
    /// far past RLIMIT_AS.
    #[inline]
    pub fn sub_total_vm(&self, pages: u64) {
        let _ = self
            .total_vm
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(pages));
    }

    /// Release the virtual-size charge of the pages that leave the mapping.
    ///
    /// The reference kernel keeps `mm->total_vm` equal to the LIVE mapping
    /// size: every page that is unmapped leaves the count (kernel/mm/mmap.c:
    /// vm_stat_account(mm, vma->vm_flags, -nrpages) for each removed VMA).
    /// The VMA surgery removes whole VMAs and re-adds the preserved head
    /// and tail pieces, so only the part of each VMA that is actually
    /// unmapped is released — the preserved pieces stay charged.
    ///
    /// Without this the counter only ever grows: RLIMIT_AS (checked
    /// against mm->total_vm) would refuse mappings that fit, and the
    /// reported VmSize/vsize would describe every mapping the process ever
    /// made.
    pub fn release_unmapped_pages(
        &self,
        vma_start: usize,
        vma_end: usize,
        range_start: usize,
        range_end: usize,
    ) {
        let cut_start = vma_start.max(range_start);
        let cut_end = vma_end.min(range_end);
        if cut_end > cut_start {
            let pages = ((cut_end - cut_start) / super::types::PAGE_SIZE) as u64;
            self.sub_total_vm(pages);
        }
    }

    /// Get locked memory page count
    #[inline]
    pub fn locked_vm(&self) -> u64 {
        self.locked_vm.load(Ordering::Acquire)
    }

    /// Get pinned memory page count
    #[inline]
    pub fn pinned_vm(&self) -> u64 {
        self.pinned_vm.load(Ordering::Acquire)
    }

    /// Get data segment page count
    #[inline]
    pub fn data_vm(&self) -> u64 {
        self.data_vm.load(Ordering::Acquire)
    }

    /// Get executable segment page count
    #[inline]
    pub fn exec_vm(&self) -> u64 {
        self.exec_vm.load(Ordering::Acquire)
    }

    /// Get stack page count
    #[inline]
    pub fn stack_vm(&self) -> u64 {
        self.stack_vm.load(Ordering::Acquire)
    }

    // ==================== RSS (resident set) ====================

    /// Get resident set size in pages.
    #[inline]
    pub fn rss(&self) -> u64 {
        self.rss.load(Ordering::Acquire)
    }

    /// Add pages to the resident set (page installed by a fault).
    #[inline]
    pub fn add_rss(&self, pages: u64) {
        self.rss.fetch_add(pages, Ordering::AcqRel);
    }

    /// Subtract pages from the resident set (PTE zapped by unmap/exit).
    /// Saturating at 0 to keep the counter meaningful on accounting gaps.
    #[inline]
    pub fn sub_rss(&self, pages: u64) {
        let _ = self
            .rss
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_sub(pages)
            });
    }

    // ==================== mmap Region ====================

    /// Get mmap base address
    #[inline]
    pub fn mmap_base(&self) -> usize {
        self.mmap_base.load(Ordering::Acquire)
    }

    /// Set mmap base address
    #[inline]
    pub fn set_mmap_base(&self, addr: usize) {
        self.mmap_base.store(addr, Ordering::Release);
    }

    /// Get highest virtual memory end address
    #[inline]
    pub fn highest_vm_end(&self) -> usize {
        self.highest_vm_end.load(Ordering::Acquire)
    }

    /// Update highest virtual memory end address
    ///
    /// Atomic max (review 4.2): the load/store pair lost updates when two
    /// threads mapped past the old ceiling concurrently.
    #[inline]
    pub fn update_highest_vm_end(&self, addr: usize) {
        let mut current = self.highest_vm_end.load(Ordering::Acquire);
        while addr > current {
            match self.highest_vm_end.compare_exchange_weak(
                current,
                addr,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    // ==================== Reference Counting ====================

    /// Increment user count (mm_users)
    /// Returns the value after increment
    #[inline]
    pub fn mm_users_inc(&self) -> i32 {
        self.mm_users.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement user count (mm_users)
    /// Returns the value after decrement.
    ///
    /// Underflow-safe (review 4.2): the old fetch_sub decremented first and
    /// only warned, letting mm_users go negative and mask later double-free
    /// bugs. Like Linux's atomic_dec_and_test consumers, a decrement at
    /// <= 0 is refused outright.
    #[inline]
    pub fn mm_users_dec(&self) -> i32 {
        loop {
            let old = self.mm_users.load(Ordering::Acquire);
            if old <= 0 {
                crate::pr_warn!("mm_users underflow: mm_users_dec called at {}", old);
                return old;
            }
            match self.mm_users.compare_exchange_weak(
                old,
                old - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return old - 1,
                Err(_) => continue,
            }
        }
    }

    /// Decrement user count and return true if it reached zero.
    #[inline]
    pub fn mm_users_dec_and_test(&self) -> bool {
        self.mm_users_dec() == 0
    }

    /// Get user count
    #[inline]
    pub fn mm_users(&self) -> i32 {
        self.mm_users.load(Ordering::Acquire)
    }

    /// Increment reference count (mm_count)
    #[inline]
    pub fn mm_count_inc(&self) -> i32 {
        self.mm_count.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement reference count (mm_count)
    ///
    /// Underflow-safe, mirroring mm_users_dec above.
    #[inline]
    pub fn mm_count_dec(&self) -> i32 {
        loop {
            let old = self.mm_count.load(Ordering::Acquire);
            if old <= 0 {
                crate::pr_warn!("mm_count underflow: mm_count_dec called at {}", old);
                return old;
            }
            match self.mm_count.compare_exchange_weak(
                old,
                old - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return old - 1,
                Err(_) => continue,
            }
        }
    }

    /// Decrement reference count and return true if it reached zero.
    #[inline]
    pub fn mm_count_dec_and_test(&self) -> bool {
        self.mm_count_dec() == 0
    }

    /// Get reference count
    #[inline]
    pub fn mm_count(&self) -> i32 {
        self.mm_count.load(Ordering::Acquire)
    }

    // ==================== Flags ====================

    /// Get flags
    #[inline]
    pub fn flags(&self) -> u64 {
        self.flags.load(Ordering::Acquire)
    }

    /// Set flags
    #[inline]
    pub fn set_flags(&self, flags: u64) {
        self.flags.store(flags, Ordering::Release);
    }

    /// Check if specified flag is set
    #[inline]
    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags.load(Ordering::Acquire) & flag != 0
    }

    // ==================== Owner ====================

    /// Get owner PID
    #[inline]
    pub fn owner_pid(&self) -> i32 {
        self.owner_pid.load(Ordering::Acquire)
    }

    /// Set owner PID
    #[inline]
    pub fn set_owner_pid(&self, pid: i32) {
        self.owner_pid.store(pid, Ordering::Release);
    }

    // ==================== VMA Operations ====================

    /// Acquire VMA read lock. SLEEPABLE (rwsem, the mmap_read_lock
    /// shape): on contention the caller parks instead of spinning
    /// preempt-disabled; a held section is preemptible. Task/syscall
    /// context only.
    #[inline]
    pub fn vma_read(&self) -> crate::sync::rwsem::RwsemReadGuard<'_, VmaManager> {
        self.vma_manager.read()
    }

    /// Try-acquire VMA read lock (None when contended; never sleeps).
    /// Diagnostic callers in softirq context (the fuxstuck census)
    /// skip their decode on contention instead of blocking against a
    /// writer. Refuses while any waiter is queued or handoff is
    /// pending (down_read_trylock semantics).
    #[inline]
    pub fn vma_try_read(&self) -> Option<crate::sync::rwsem::RwsemReadGuard<'_, VmaManager>> {
        self.vma_manager.try_read()
    }

    /// 78th (ohw115): census-only snapshot of the VMA rwsem's raw
    /// owner word + count. When a thread is observed stuck mid-brk
    /// (sys=45 for minutes — ohw115 tid 344), the owner word names
    /// the founder: a WRITER pid (owner low bits) or READER-owned
    /// (bit set; count's reader units). Relaxed loads, no locking.
    #[inline]
    pub fn vma_rwsem_diag(&self) -> (usize, i64) {
        self.vma_manager.diag()
    }

    /// Acquire VMA write lock
    #[inline]
    /// Pin a file reference for a file-backed VMA (Linux vm_file hold).
    pub fn pin_vma_file(&self, vma_start: usize, file: alloc::sync::Arc<crate::fs::file::File>) {
        self.vma_files.write().insert(vma_start, file);
    }

    /// Drop the pinned file reference for a VMA being removed.
    pub fn unpin_vma_file(&self, vma_start: usize) {
        self.vma_files.write().remove(&vma_start);
    }

    /// Fetch the pinned file for a VMA (demand-fault fill source). The pin
    /// keeps the file alive even after the mapping fd was closed.
    pub fn get_vma_file(&self, vma_start: usize) -> Option<alloc::sync::Arc<crate::fs::file::File>> {
        self.vma_files.read().get(&vma_start).cloned()
    }

    /// Non-blocking `get_vma_file` for diagnostic callers that run under
    /// locks they must not spin against (pid-hash buckets, softirq): a
    /// busy map defers the decode instead of blocking the caller.
    pub fn try_get_vma_file(
        &self,
        vma_start: usize,
    ) -> Option<alloc::sync::Arc<crate::fs::file::File>> {
        let m = self.vma_files.try_read()?;
        m.get(&vma_start).cloned()
    }

    /// Acquire VMA write lock. SLEEPABLE (rwsem, the mmap_write_lock
    /// shape): a contended writer first tries bounded optimistic
    /// spinning (only while the owner is on-CPU) and then parks
    /// uninterruptibly — it never spins the CPU away. Task/syscall
    /// context only.
    pub fn vma_write(&self) -> crate::sync::rwsem::RwsemWriteGuard<'_, VmaManager> {
        self.vma_manager.write()
    }

    /// Find VMA
    pub fn find_vma(&self, addr: VirtAddr) -> Option<Vma> {
        let vma_mgr = self.vma_read();
        vma_mgr.find(addr).cloned()
    }

    /// Add VMA and update statistics
    pub fn add_vma(&self, vma: Vma) -> Result<(), MapError> {
        let pages = vma.page_count() as u64;

        let mut vma_mgr = self.vma_write();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Update statistics
        self.add_total_vm(pages);

        // Update highest end address
        self.update_highest_vm_end(vma.end().as_usize());

        Ok(())
    }

    /// Remove VMA and update statistics
    pub fn remove_vma(&self, start: VirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();
        let vma = vma_mgr.get(start).cloned();

        if let Some(vma) = vma {
            let pages = vma.page_count() as u64;
            vma_mgr.remove(start).map_err(|_| MapError::NotMapped)?;
            self.sub_total_vm(pages);
            Ok(())
        } else {
            Err(MapError::NotMapped)
        }
    }

    // ==================== ELF Loading Helpers ====================

    /// Set code/data segment ranges based on ELF segment type
    ///
    /// Called during ELF loading to set start_code, end_code, start_data, end_data
    pub fn setup_segment_layout(
        &self,
        code_start: usize,
        code_end: usize,
        data_start: usize,
        data_end: usize,
        entry_point: usize,
    ) {
        self.set_start_code(code_start);
        self.set_end_code(code_end);
        self.set_start_data(data_start);
        self.set_end_data(data_end);

        // Update executable segment page count
        let code_pages = ((code_end.saturating_sub(code_start) + 4095) / 4096) as u64;
        self.exec_vm.store(code_pages, Ordering::Release);

        // Update data segment page count
        let data_pages = ((data_end.saturating_sub(data_start) + 4095) / 4096) as u64;
        self.data_vm.store(data_pages, Ordering::Release);

        // Update total page count
        self.add_total_vm(code_pages + data_pages);
    }

    /// Set up stack layout
    ///
    /// Set stack address and statistics
    pub fn setup_stack(&self, stack_top: usize, stack_size: usize) {
        self.set_start_stack(stack_top);

        let stack_pages = ((stack_size + 4095) / 4096) as u64;
        self.stack_vm.store(stack_pages, Ordering::Release);
        self.add_total_vm(stack_pages);

        // Update highest end address
        self.update_highest_vm_end(stack_top);
    }

    /// Set up command line arguments layout
    pub fn setup_argv(&self, arg_start: usize, arg_end: usize) {
        self.set_arg_start(arg_start);
        self.set_arg_end(arg_end);
    }

    /// Set up environment variables layout
    pub fn setup_envp(&self, env_start: usize, env_end: usize) {
        self.set_env_start(env_start);
        self.set_env_end(env_end);
    }

    /// Save the exec-time auxiliary vector for a later core dump
    /// (NT_AUXV source). `words` is (type, value) pairs ending with the
    /// AT_NULL pair; anything past the terminator or past
    /// AT_VECTOR_SIZE words is dropped.
    pub fn set_saved_auxv(&self, words: &[u64]) {
        let n = words.len().min(AT_VECTOR_SIZE);
        let mut auxv = self.saved_auxv.write();
        auxv[..n].copy_from_slice(&words[..n]);
    }

    /// Read back the saved auxiliary vector as (type, value) pairs up to
    /// and including the AT_NULL terminator. Returns an empty Vec when
    /// no auxv was ever saved.
    pub fn saved_auxv(&self) -> alloc::vec::Vec<u64> {
        let auxv = self.saved_auxv.read();
        // AT_NULL == 0 terminates the pair list.
        let mut n = 0;
        while n + 1 < AT_VECTOR_SIZE && auxv[n] != 0 {
            n += 2;
        }
        if n == 0 {
            return alloc::vec::Vec::new();
        }
        // Include the AT_NULL pair itself (n points AT it), clamped to the
        // array for a completely full vector with no terminator.
        let end = (n + 2).min(AT_VECTOR_SIZE);
        auxv[..end].to_vec()
    }
}

// Send/Sync proofs live in mm::unsafe (INV-MM-2 family) — the Arc'd mm
// crosses threads only through the pinned-reference discipline the
// edges document.

impl Drop for MmStruct {
    fn drop(&mut self) {
        // Exec text hold (reference allow_write_access at the final mmput):
        // this mm dying releases its share of the write denial on the
        // exec'd image — the last drop re-enables write opens of the file.
        if let Some(key) = self.exe_text_key.lock().take() {
            crate::fs::locks::inode_text_hold_release(key);
        }

        // Free ASID
        self.free_asid();

        // Free page tables when the last reference is dropped
        // Only free for user address spaces (not kernel)
        //
        // Per-VMA teardown note (review 4.2, "Drop does not walk the VMA tree"): the actual
        // page teardown walks the PAGE TABLES (free_user_page_tables visits
        // every leaf PTE, does page_remove_rmap + put_page and frees the
        // page-table pages). Driving VmaManager::unmap_pages per VMA here
        // would put_page every resident page a SECOND time (double free).
        // The VMA tree itself is released with the struct. RSS/LRU stats
        // are already drained by that PTE walk.
        if self.space_type == PageTableType::User {
            crate::debug::memwatch::MM_DROPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

            // forkprof (cmdline forkprof=1): drop-phase split — VMA
            // release / grace wait / page-table free.
            let fp = crate::debug::forkprof::on();
            if fp {
                crate::debug::forkprof::bump(crate::debug::forkprof::S_MM_DROPS, 1);
            }
            let fp_t_vma = if fp { crate::debug::forkprof::now() } else { 0 };

            // ===== SysV shm detach (exactly once, Linux exit_mmap parity) =====
            // nattch must drop exactly once per mm, at the point its address
            // space goes away — NOT per task exit (CLONE_VM siblings share
            // one Arc) and not twice. do_exit's old pre-drop
            // `Arc::strong_count == 1` check was racy: a concurrently
            // cloned walker reference (ptrace/OOM/rmap) could make the
            // check read >1 at exit and 1 by drop time (nattch leak), or
            // two sibling exits could both observe 1 (double detach). The
            // Drop impl runs exactly once by construction. Runs before the
            // VMA tree is released with the struct.
            {
                let vma_mgr = self.vma_read();
                for vma in vma_mgr.iter() {
                    if vma.vma_type() == super::types::VmaType::SharedMemory {
                        let shmid = vma.file_fd();
                        if shmid >= 0 {
                            crate::api::ipc::shm_detach_vma(shmid);
                        }
                    }
                }
            }

            // ===== VMA tree release under vma_write (walker exclusion) =====
            // procfs generate_maps/smaps/status hold `vma_read()` across
            // their whole btree iteration on an UNPINNED &MmStruct borrow
            // of a foreign task. Freeing the btree nodes with the struct
            // while such a reader navigates them is the #GP-in-btree-
            // navigate family (reference OH log pid 554; exitwalk pid 314:
            // atomic_load inside the node comparison chasing a
            // non-canonical child pointer). Dropping the tree under its
            // OWN write lock mutually excludes every vma_read holder — a
            // reader that armed before us blocks the write acquisition
            // until it finishes; readers after see an empty (dying) tree.
            // Same treatment for the pinned-file table (get_vma_file
            // readers). The struct's own heap slot still follows the
            // grace-period wait below.
            //
            // (sysmm-2x lane) The VMA range list is snapshotted under the
            // SAME write guard, before the tree is released: the teardown
            // walk below uses it to bound its leaf-table scans (see
            // free_user_page_tables).
            let vma_ranges: alloc::vec::Vec<(u64, u64)> = {
                let mut mgr = self.vma_write();
                let ranges = mgr
                    .iter()
                    .map(|vma| (vma.start().0 as u64, vma.end().0 as u64))
                    .collect();
                // old tree's nodes are freed HERE, under the write lock
                *mgr = VmaManager::new();
                self.vma_files.write().clear();
                ranges
            };
            if fp {
                crate::debug::forkprof::add(crate::debug::forkprof::P_DROP_VMAFREE, fp_t_vma);
            }

            // ===== Grace-period wait: hold off the free under raw borrowers =====
            // procfs generate_maps/smaps/status take `task.address_space()` —
            // an UNACCOUNTED &MmStruct borrow of a foreign task. A CPU
            // mid-syscall never reports an RCU quiescent state (quiescence
            // needs a context switch, idle entry, or user return), so one
            // grace period after the LAST task slot went None guarantees
            // every such in-flight borrow has finished before we free the
            // page tables and (with the struct) the VMA btree. This is the
            // same idea as Linux freeing mm_struct via call_rcu in mmdrop.
            // Skipped when we cannot schedule (spinlock held / IRQs off /
            // softirq — e.g. an OOM-scan Arc dropped under a pid-hash
            // bucket lock): those callers are restructured to drop outside
            // their locks, this is the belt-and-braces guard.
            let fp_t_rcu = if fp { crate::debug::forkprof::now() } else { 0 };
            if crate::interrupt::preempt::preempt_count() == 0
                && crate::arch::cpu::get_interrupts_state()
            {
                // BOUNDED (lb125 decode): the unbounded wait froze every
                // exit — and init, and the whole service graph — behind
                // monopolist windows; see synchronize_rcu_bounded. The
                // bound matches the deferred_free wall-clock discipline
                // (1.5 s) that still holds the struct free afterwards.
                crate::sync::rcu::synchronize_rcu_bounded_pollfirst(1500, "mm-drop");
            }
            if fp {
                crate::debug::forkprof::add(crate::debug::forkprof::P_DROP_RCU, fp_t_rcu);
            }

            // Teardown-context witness (FUT-STALE family): a teardown in a
            // context that is not the (still-live) owner names a
            // deferred/wrong-context drop — the shape the PTLEDGER
            // DOUBLE-FREE ring then attributes. The owner counts as
            // still-live ONLY when it can still reach THIS mm: not
            // PF_EXITING (do_exit's first store — exit_mm clears the
            // slot long before the ZOMBIE transition, and the exitwalk
            // churn showed every MM-DROP-CTX hit was an owner inside
            // exactly that window while a /proc maps walker's pinned Arc
            // dropped last in the walker's context — the designed
            // deferred drop, not a UAF) and its slot still names this
            // MmStruct (identity compare; a slot-held Arc keeps the
            // refcount above zero, so a live slot here would mean the
            // refcount itself is corrupt — the real UAF signal).
            {
                let owner = self.owner_pid();
                let cur = crate::sched::hotpath::get_current_pid() as i32;
                let owner_live = crate::process::hotpath::find_task_by_pid(owner as u32)
                    .map(|t| {
                        let st = t.state();
                        !st.is_dead()
                            && !st.contains(crate::process::types::TaskState::ZOMBIE)
                            && t.ti_flags()
                                & crate::process::types::TaskFlags::PF_EXITING
                                    .bits()
                                == 0
                            && t.mm_ptr_peek()
                                == Some(self as *const MmStruct as *const AddressSpace)
                    })
                    .unwrap_or(false);
                if owner > 0 && owner != cur && owner_live {
                    static DEFER_REPORTS: AtomicUsize = AtomicUsize::new(0);
                    if DEFER_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                        crate::pr_warn!(
                            "MM-DROP-CTX: mm root={:#x} owner pid={} still holds this mm, dropped in pid={} context",
                            self.pgd, owner, cur
                        );
                    }
                }
            }

            let fp_t_pt = if fp { crate::debug::forkprof::now() } else { 0 };
            // The dying mm's OWN PteLock (INV-LOCK-7): uncontended by
            // construction here (last-Arc + synchronize_rcu drain), so
            // the teardown chunks never queue behind a live mm's
            // sections (free_user_page_tables is a safe fn — the old
            // wrapper was vestigial).
            crate::arch::mm::free_user_page_tables(self.pte_lock(), self.pgd, &vma_ranges);
            if fp {
                crate::debug::forkprof::add(crate::debug::forkprof::P_DROP_PTFREE, fp_t_pt);
            }
        }
    }
}

// ============================================================================
// Helper type aliases (backward compatibility)
// ============================================================================


// ════════════════ memblock ════════════════════════════
// (was memblock.rs)
// Memblock Early Memory Allocator
//
// This module implements a memblock-style early memory allocator for
// memblock subsystem. It is used during early boot before the buddy allocator is
// initialized.
//
// # Overview
//
// Memblock manages memory in terms of regions, with four types:
// - memory: Available physical memory regions (from device tree)
// - reserved: Regions that are already in use (kernel, initrd, dtb, etc.)
// - nomap: Memory regions that should not be mapped (e.g., persistent memory)
//
// # Alignment
//
// This implementation follows standard memblock design:
// 1. Memory regions are discovered from device tree (/memory nodes)
// 2. Reserved regions are added for kernel, initrd, dtb, etc.
// 3. Frame allocator is initialized from available memory (memory - reserved)

impl MemBlockFlags {
    /// No special flags
    pub const NONE: MemBlockFlags = MemBlockFlags(0);
    /// Region should not be mapped (MEMBLOCK_NOMAP)
    pub const NOMAP: MemBlockFlags = MemBlockFlags(1 << 0);
    /// Region is mirror of another region
    pub const MIRROR: MemBlockFlags = MemBlockFlags(1 << 1);
}

impl MemBlockRegion {
    pub const fn new(base: usize, size: usize) -> Self {
        Self {
            base,
            size,
            flags: MemBlockFlags::NONE,
            nid: 0,
        }
    }

    pub const fn with_flags(base: usize, size: usize, flags: MemBlockFlags) -> Self {
        Self {
            base,
            size,
            flags,
            nid: 0,
        }
    }

    /// Get end address (exclusive)
    #[inline]
    pub fn end(&self) -> usize {
        self.base + self.size
    }

    /// Check if region contains address
    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.end()
    }

    /// Get page frame number of base
    #[inline]
    pub fn base_pfn(&self) -> usize {
        self.base / PAGE_SIZE
    }

    /// Get page frame number of end (exclusive)
    #[inline]
    pub fn end_pfn(&self) -> usize {
        (self.base + self.size) / PAGE_SIZE
    }

    /// Get number of pages in region
    #[inline]
    pub fn page_count(&self) -> usize {
        self.size / PAGE_SIZE
    }
}

impl MemBlockType {
    pub const fn new() -> Self {
        Self {
            regions: [MemBlockRegion::new(0, 0); MAX_MEMBLOCK_REGIONS],
            cnt: 0,
            total_size: 0,
        }
    }

    /// Insert [base, base+size) keeping the region array SORTED by base and
    /// fully merged (Linux memblock_insert_region + memblock_merge_regions).
    ///
    /// The old add/add_reserved merged with the FIRST adjacent entry only,
    /// so a range that BRIDGED two existing regions left one of them
    /// overlapping the new extent: total_size double-counted the overlap
    /// and the region was handed to the zone allocator twice (review 4.17).
    /// This helper folds EVERY overlapping or touching region into one
    /// extent (union) and re-inserts it at its sorted position.
    fn insert_and_merge_all(&mut self, base: usize, size: usize, flags: MemBlockFlags) -> Result<(), ()> {
        let mut m_base = base;
        let mut m_end = match base.checked_add(size) {
            Some(e) => e,
            None => return Err(()),
        };
        let mut m_flags = flags;

        // Fold pass (to fixed point): absorb every region that overlaps or
        // touches the extent. A bridging insert can pull in several regions
        // one after another; iterating to a fixed point also catches a
        // region that only becomes adjacent after the extent grew.
        loop {
            let mut changed = false;
            for i in 0..self.cnt {
                let r = self.regions[i];
                let r_end = r.base + r.size;
                if m_base <= r_end && m_end >= r.base {
                    if m_base > r.base {
                        m_base = r.base;
                        changed = true;
                    }
                    if m_end < r_end {
                        m_end = r_end;
                        changed = true;
                    }
                    // Absorbed regions donate their flags if the incoming
                    // range has none (keeps NOMAP sticky across merges).
                    if m_flags == MemBlockFlags::NONE && r.flags != MemBlockFlags::NONE {
                        m_flags = r.flags;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // Compact pass: drop absorbed regions (subtract their size from the
        // total), keeping the survivors in sorted order.
        let mut write = 0usize;
        for read in 0..self.cnt {
            let r = self.regions[read];
            let r_end = r.base + r.size;
            if m_base <= r_end && m_end >= r.base {
                self.total_size = self.total_size.saturating_sub(r.size);
            } else {
                self.regions[write] = r;
                write += 1;
            }
        }
        let live = write;

        // If nothing was absorbed we need one extra slot for the new region.
        if live == self.cnt && self.cnt >= MAX_MEMBLOCK_REGIONS {
            return Err(());
        }

        // Sorted insertion index among the live prefix.
        let mut at = 0usize;
        while at < live && self.regions[at].base < m_base {
            at += 1;
        }

        // Shift [at, live) right by one and place the merged extent.
        let mut idx = live;
        while idx > at {
            self.regions[idx] = self.regions[idx - 1];
            idx -= 1;
        }
        self.regions[at] = MemBlockRegion::with_flags(m_base, m_end - m_base, m_flags);
        self.cnt = live + 1;
        self.total_size += m_end - m_base;

        Ok(())
    }

    /// Add a region (memory type): page-aligned, sorted, fully merged.
    pub fn add(&mut self, base: usize, size: usize) -> Result<(), ()> {
        // Align region to page boundaries using base+end (not base+size independently)
        let aligned_base = (base + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let aligned_end = (base + size) & !(PAGE_SIZE - 1);

        if aligned_end <= aligned_base {
            return Err(());
        }

        self.insert_and_merge_all(aligned_base, aligned_end - aligned_base, MemBlockFlags::NONE)
    }

    /// Add a reserved region (no alignment, sorted, fully merged)
    pub fn add_reserved(&mut self, base: usize, size: usize, flags: MemBlockFlags) -> Result<(), ()> {
        self.insert_and_merge_all(base, size, flags)
    }

    /// Remove a region by index
    pub fn remove(&mut self, idx: usize) {
        if idx >= self.cnt {
            return;
        }

        self.total_size -= self.regions[idx].size;

        // Shift remaining regions
        for i in idx..self.cnt - 1 {
            self.regions[i] = self.regions[i + 1];
        }
        self.cnt -= 1;
    }

    /// Iterate over regions
    pub fn iter(&self) -> impl Iterator<Item = &MemBlockRegion> {
        self.regions[..self.cnt].iter()
    }

    /// Get region count
    #[inline]
    pub fn len(&self) -> usize {
        self.cnt
    }

    /// Check if empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.cnt == 0
    }

    /// Get total size
    #[inline]
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Find region containing address
    pub fn find(&self, addr: usize) -> Option<&MemBlockRegion> {
        self.iter().find(|r| r.contains(addr))
    }

    /// Find first available region (for frame allocator)
    pub fn find_first_available(&self) -> Option<&MemBlockRegion> {
        self.iter().find(|r| r.flags == MemBlockFlags::NONE)
    }
}

impl MemBlock {
    pub const fn new() -> Self {
        Self {
            memory: MemBlockType::new(),
            reserved: MemBlockType::new(),
            initialized: AtomicBool::new(false),
            bottom: 0,
            top: 0,
            current: 0,
        }
    }

    /// Initialize memblock (can only be called once)
    pub fn init(&mut self) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }
        self.initialized.store(true, Ordering::Release);
    }

    /// Add memory region (from device tree)
    pub fn add_memory(&mut self, base: usize, size: usize) -> Result<(), ()> {
        self.memory.add(base, size)?;

        // Update top
        if base + size > self.top {
            self.top = base + size;
        }

        Ok(())
    }

    /// Reserve a memory region
    pub fn reserve(&mut self, base: usize, size: usize) -> Result<(), ()> {
        self.reserved.add_reserved(base, size, MemBlockFlags::NONE)?;

        // Update bottom if this is the highest reserved region
        if base + size > self.bottom {
            self.bottom = base + size;
        }

        Ok(())
    }

    /// Reserve a memory region with NOMAP flag
    pub fn reserve_nomap(&mut self, base: usize, size: usize) -> Result<(), ()> {
        self.reserved.add_reserved(base, size, MemBlockFlags::NOMAP)
    }

    /// Get available memory regions (memory - reserved)
    /// Returns the first region that can be used for frame allocation
    pub fn get_available_region(&self) -> Option<MemBlockRegion> {
        // Find the highest reserved region end
        let mut highest_reserved_end = 0usize;
        for res_region in self.reserved.iter() {
            if res_region.end() > highest_reserved_end {
                highest_reserved_end = res_region.end();
            }
        }

        // Find the first memory region that starts at or after the highest reserved end
        for mem_region in self.memory.iter() {
            // Check if this memory region contains addressable space after reserved regions
            if mem_region.end() > highest_reserved_end {
                let available_start = highest_reserved_end.max(mem_region.base);
                let available_end = mem_region.end();

                // Align to page boundary
                let aligned_start = (available_start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                let aligned_end = available_end & !(PAGE_SIZE - 1);

                if aligned_end > aligned_start {
                    return Some(MemBlockRegion::new(aligned_start, aligned_end - aligned_start));
                }
            }
        }

        None
    }

    /// Get total memory size
    #[inline]
    pub fn total_memory(&self) -> usize {
        self.memory.total_size()
    }

    /// Get total reserved size
    #[inline]
    pub fn total_reserved(&self) -> usize {
        self.reserved.total_size()
    }

    /// Get available memory size
    pub fn available_memory(&self) -> usize {
        self.memory.total_size().saturating_sub(self.reserved.total_size())
    }

    /// Get memory regions
    #[inline]
    pub fn memory(&self) -> &MemBlockType {
        &self.memory
    }

    /// Get reserved regions
    #[inline]
    pub fn reserved(&self) -> &MemBlockType {
        &self.reserved
    }

    /// Check if address is reserved
    pub fn is_reserved(&self, addr: usize) -> bool {
        self.reserved.find(addr).is_some()
    }

    /// Check if address is in memory
    pub fn is_memory(&self, addr: usize) -> bool {
        self.memory.find(addr).is_some()
    }

    /// Find a free region of given size (for early boot allocations)
    pub fn find_in_range(&self, size: usize, min_addr: usize, max_addr: usize) -> Option<usize> {
        // Align size to page boundary
        let size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        for mem_region in self.memory.iter() {
            let region_start = mem_region.base.max(min_addr);
            let region_end = (mem_region.base + mem_region.size).min(max_addr);

            if region_start >= region_end || region_end - region_start < size {
                continue;
            }

            // Check if region is free (not reserved)
            let mut addr = region_start;
            while addr + size <= region_end {
                let mut is_free = true;
                for res_region in self.reserved.iter() {
                    if addr < res_region.end() && addr + size > res_region.base {
                        // Overlaps with reserved region, skip to after it
                        addr = res_region.end();
                        is_free = false;
                        break;
                    }
                }
                if is_free {
                    return Some(addr);
                }
            }
        }

        None
    }

    /// Dump memblock state (for debugging)
    pub fn dump(&self) {
        crate::println!("memblock: memory regions:");
        for (i, region) in self.memory.iter().enumerate() {
            crate::println!("  [{:2}] {:#018x} - {:#018x} ({:?})",
                i, region.base, region.end(), region.size / (1024 * 1024));
        }
        crate::println!("memblock: reserved regions:");
        for (i, region) in self.reserved.iter().enumerate() {
            crate::println!("  [{:2}] {:#018x} - {:#018x} ({:?})",
                i, region.base, region.end(), region.size / (1024 * 1024));
        }
        crate::println!("memblock: total: {:?} MB, reserved: {:?} MB, available: {:?} MB",
            self.total_memory() / (1024 * 1024),
            self.total_reserved() / (1024 * 1024),
            self.available_memory() / (1024 * 1024));
    }

    /// Iterate over free memory ranges (memory - reserved)
    pub fn for_each_free_range<F>(&self, min_addr: usize, max_addr: usize, mut f: F)
    where
        F: FnMut(usize, usize),
    {
        // Collect reserved regions for sorting
        let reserved: Vec<_> = self.reserved.iter().collect();
        let mut reserved_sorted: Vec<_> = reserved.into_iter().collect();
        reserved_sorted.sort_by_key(|r| r.base);

        for mem_region in self.memory.iter() {
            let region_start = mem_region.base.max(min_addr);
            let region_end = (mem_region.base + mem_region.size).min(max_addr);

            if region_start >= region_end {
                continue;
            }

            // Walk through this memory region, skipping reserved parts
            let mut current = region_start;
            for res in &reserved_sorted {
                if res.base >= region_end {
                    break;
                }
                if res.end() <= region_start {
                    continue;
                }

                // Free range before this reserved region
                if current < res.base {
                    let free_start = current;
                    let free_end = res.base.min(region_end);
                    if free_start < free_end {
                        f(free_start, free_end);
                    }
                }

                // Skip past the reserved region
                current = current.max(res.end());
            }

            // Free range after all reserved regions
            if current < region_end {
                f(current, region_end);
            }
        }
    }
}

// Global memblock instance.
//
// MEMBLOCK is accessed from early boot (single-threaded) through the memblock_init /
// memblock_add / memblock_reserve helpers, and later read-only via memblock().
// All mutation must happen before the scheduler starts; after that, only
// `memblock()` (shared ref) is safe to call.
static mut MEMBLOCK: MemBlock = MemBlock::new();

/// Raw pointer to the memblock static — the ONLY bridge the
/// mm::unsafe (INV-MM-3) use to reach it; never called from elsewhere
/// (forming the pointer is inert; the borrow discipline lives in
/// edges::memblock_ref/memblock_mut).
#[allow(static_mut_refs)]
pub(super) fn memblock_raw() -> *mut MemBlock {
    &raw mut MEMBLOCK
}

/// Init guard for MEMBLOCK — ensures init() runs exactly once.
static MEMBLOCK_INIT: AtomicBool = AtomicBool::new(false);

/// Initialize memblock
pub fn memblock_init() {
    // Use compare_exchange so that even if two early-boot callers race, only
    // one proceeds with initialization.
    if MEMBLOCK_INIT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        // Early boot (single-threaded), before any concurrent access to
        // MEMBLOCK (INV-MM-3 exclusive arm).
        super::r#unsafe::memblock_mut().init();
    }
}

/// Add memory region
pub fn memblock_add(base: usize, size: usize) -> Result<(), ()> {
    // Early boot before concurrent access (INV-MM-3 exclusive arm).
    super::r#unsafe::memblock_mut().add_memory(base, size)
}

/// Reserve memory region
pub fn memblock_reserve(base: usize, size: usize) -> Result<(), ()> {
    // Early boot before concurrent access (INV-MM-3 exclusive arm).
    super::r#unsafe::memblock_mut().reserve(base, size)
}

/// Reserve memory region with NOMAP
pub fn memblock_reserve_nomap(base: usize, size: usize) -> Result<(), ()> {
    // Early boot before concurrent access (INV-MM-3 exclusive arm).
    super::r#unsafe::memblock_mut().reserve_nomap(base, size)
}

/// Get available region for frame allocator
pub fn memblock_get_available_region() -> Option<MemBlockRegion> {
    // Read-only access; MEMBLOCK is initialized before this is called
    // (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().get_available_region()
}

/// Get total memory size
pub fn memblock_total_memory() -> usize {
    // Read-only access (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().total_memory()
}

/// Get available memory size
pub fn memblock_available_memory() -> usize {
    // Read-only access (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().available_memory()
}

/// Check if address is reserved
pub fn memblock_is_reserved(addr: usize) -> bool {
    // Read-only access (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().is_reserved(addr)
}

/// Iterate over free memory ranges (memory - reserved)
pub fn memblock_for_each_free_range<F>(min_addr: usize, max_addr: usize, f: F)
where
    F: FnMut(usize, usize),
{
    // Read-only iteration over regions (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().for_each_free_range(min_addr, max_addr, f)
}

/// Find free region in range
pub fn memblock_find_in_range(size: usize, min_addr: usize, max_addr: usize) -> Option<usize> {
    // Read-only query (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().find_in_range(size, min_addr, max_addr)
}

/// Dump memblock state
pub fn memblock_dump() {
    // Read-only access for diagnostics (INV-MM-3 shared arm).
    super::r#unsafe::memblock_ref().dump()
}

/// Get reference to memblock (for reading).
///
/// # Safety
/// Caller must ensure MEMBLOCK has been initialized (memblock_init called).
/// After the scheduler starts this is safe to call from any context because
/// MEMBLOCK is no longer mutated.
pub fn memblock() -> &'static MemBlock {
    // MEMBLOCK is initialized before any call to this function; after
    // early boot the data is effectively immutable (INV-MM-3).
    super::r#unsafe::memblock_ref()
}

/// Get mutable reference to memblock (for initialization).
///
/// # Safety
/// Must only be called during early boot before the scheduler starts.
/// The caller must ensure no other thread can access MEMBLOCK concurrently.
pub fn memblock_mut() -> &'static mut MemBlock {
    // Exclusive access only during early boot (INV-MM-3 exclusive arm).
    super::r#unsafe::memblock_mut()
}

/// Allocate a physical page from memblock.
/// Physical memory allocation from memblock.
/// Returns physical address of allocated page, or None if allocation fails.
pub fn memblock_phys_alloc() -> Option<usize> {
    // Early boot (single-threaded) — INV-MM-3 exclusive arm.
    {
        let mb = super::r#unsafe::memblock_mut();
        // Find a free page in available memory
        let phys = mb.find_in_range(PAGE_SIZE, 0, usize::MAX)?;
        // SCRIBBLE HUNT: the kernel image occupies the low ~12MB; a page
        // handed out from there becomes a live page table over .text
        // (the x86 text-scribbler). Catch it red-handed.
        if phys < 0x220_0000 {
            crate::pr_err!(
                "MEMBLOCK-LOW-HIT phys={:#x} — kernel-image page handed out!",
                phys
            );
        }
        // Reserve it
        mb.reserve(phys, PAGE_SIZE).ok()?;
        Some(phys)
    }
}


// ════════════════ zone ════════════════════════════════
// (was zone.rs)
// Zone Allocator Infrastructure
//
// This module implements memory zones for physical page management.
// Zones are used to group pages with similar characteristics or constraints.
//
// # Safety Invariants — Buddy Free List Integrity
//
// The zone buddy allocator (FreeArea free lists) must maintain these invariants:
//
// - **INV-BUDDY-1**: Every page on a free list has `_refcount == 0`.
//
// - **INV-BUDDY-2**: A page's `private` order field matches the order of the
//   free list it resides on.
//
// - **INV-BUDDY-3**: Each order's free list forms a valid singly-linked list
//   (no cycles, no nodes outside the zone's PFN range).
//
// - **INV-BUDDY-4**: `free_count[order]` matches the actual list length for
//   every order.
//
// - **INV-BUDDY-5**: The buddy of a free page is either free (same order),
//   allocated, or outside the zone boundaries — never free at a different order.

// ==================== Zone Type Definitions ====================


impl ZoneType {
    /// Get zone name as string
    pub fn name(&self) -> &'static str {
        match self {
            ZoneType::ZoneDma => "DMA",
            ZoneType::ZoneDma32 => "DMA32",
            ZoneType::ZoneNormal => "Normal",
            ZoneType::ZoneMovable => "Movable",
            ZoneType::ZoneCount => "Invalid",
        }
    }

    /// Get zone index
    pub fn index(&self) -> usize {
        *self as usize
    }
}

// ==================== Free Area (Buddy System) ====================

/// Null pointer for free list
pub const FREE_LIST_NULL: usize = usize::MAX;

impl FreeArea {
    pub const fn new() -> Self {
        Self {
            free_list: AtomicUsize::new(FREE_LIST_NULL),
            nr_free: AtomicUsize::new(0),
        }
    }

    /// Check if free list is empty
    pub fn is_empty(&self) -> bool {
        self.free_list.load(Ordering::Acquire) == FREE_LIST_NULL
    }

    /// Get number of free blocks
    pub fn nr_free(&self) -> usize {
        self.nr_free.load(Ordering::Acquire)
    }

    /// Increment free count
    pub fn inc_free(&self) {
        self.nr_free.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement free count
    pub fn dec_free(&self) {
        self.nr_free.fetch_sub(1, Ordering::Relaxed);
    }
}

// ==================== Watermark Constants ====================

// ==================== Zone Structure ====================

// Static array of free areas
const fn new_free_areas() -> [FreeArea; MAX_ORDER + 1] {
    let mut arr: [FreeArea; MAX_ORDER + 1] = [
        FreeArea::new(), FreeArea::new(), FreeArea::new(), FreeArea::new(),
        FreeArea::new(), FreeArea::new(), FreeArea::new(), FreeArea::new(),
        FreeArea::new(), FreeArea::new(), FreeArea::new(),
    ];
    arr
}

impl Zone {
    /// Create a new uninitialized zone
    pub const fn new(zone_type: ZoneType, zone_id: usize, node_id: usize) -> Self {
        Self {
            zone_type,
            zone_id,
            node_id,
            zone_start_pfn: AtomicUsize::new(0),
            zone_end_pfn: AtomicUsize::new(0),
            spanned_pages: AtomicUsize::new(0),
            present_pages: AtomicUsize::new(0),
            managed_pages: AtomicUsize::new(0),
            free_pages: AtomicUsize::new(0),
            free_area: new_free_areas(),
            lock: Spinlock::new(()),
            initialized: AtomicBool::new(false),
            _watermark: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_WMARK]
            },
        }
    }

    /// Initialize the zone
    ///
    /// # Arguments
    /// - `start_pfn`: Start page frame number
    /// - `end_pfn`: End page frame number (exclusive)
    pub fn init(&self, start_pfn: usize, end_pfn: usize) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        let _guard = self.lock.lock();

        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        let spanned = end_pfn.saturating_sub(start_pfn);

        self.zone_start_pfn.store(start_pfn, Ordering::Release);
        self.zone_end_pfn.store(end_pfn, Ordering::Release);
        self.spanned_pages.store(spanned, Ordering::Release);
        self.present_pages.store(spanned, Ordering::Release);
        self.managed_pages.store(spanned, Ordering::Release);
        // Don't set free_pages here - it will be updated as pages are added

        self.initialized.store(true, Ordering::Release);
    }

    /// Check if zone is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Get zone type
    pub fn zone_type(&self) -> ZoneType {
        self.zone_type
    }

    /// Get zone ID
    pub fn zone_id(&self) -> usize {
        self.zone_id
    }

    /// Get node ID
    pub fn node_id(&self) -> usize {
        self.node_id
    }

    /// Get start PFN
    pub fn start_pfn(&self) -> usize {
        self.zone_start_pfn.load(Ordering::Acquire)
    }

    /// Get end PFN (exclusive)
    pub fn end_pfn(&self) -> usize {
        self.zone_end_pfn.load(Ordering::Acquire)
    }

    /// Get spanned pages
    pub fn spanned_pages(&self) -> usize {
        self.spanned_pages.load(Ordering::Acquire)
    }

    /// Get present pages
    pub fn present_pages(&self) -> usize {
        self.present_pages.load(Ordering::Acquire)
    }

    /// Get managed pages
    pub fn managed_pages(&self) -> usize {
        self.managed_pages.load(Ordering::Acquire)
    }

    /// Get free pages count
    pub fn nr_free(&self) -> usize {
        self.free_pages.load(Ordering::Acquire)
    }

    /// Add n to the free-page counter (pcp cache bookkeeping, INV-PCP-2:
    /// free_pages counts buddy-list frames AND pcp-cached frames).
    pub fn free_pages_add(&self, n: usize) {
        self.free_pages.fetch_add(n, Ordering::Release);
    }

    /// Subtract n from the free-page counter (pcp cache bookkeeping).
    pub fn free_pages_sub(&self, n: usize) {
        self.free_pages.fetch_sub(n, Ordering::Release);
    }

    /// Set managed pages (boot-time only): the count of pages actually
    /// handed to the buddy (span minus reserved/hole pages), mirroring
    /// Linux zone->managed_pages vs spanned_pages.
    pub fn set_managed_pages(&self, n: usize) {
        self.managed_pages.store(n, Ordering::Release);
    }

    /// Get watermark value for the given level (WMARK_MIN/LOW/HIGH).
    pub fn watermark(&self, wmark: usize) -> usize {
        self._watermark[wmark].load(Ordering::Relaxed)
    }

    /// Set watermark value.
    pub fn set_watermark(&self, wmark: usize, val: usize) {
        self._watermark[wmark].store(val, Ordering::Relaxed);
    }

    /// Check whether this zone has enough free pages to satisfy an
    /// allocation of the given order above the specified watermark level.
    ///
    /// Following `__zone_watermark_ok()` in mm/page_alloc.c:
    ///   min = watermark[level] + (1 << order) - 1
    ///   return free_pages >= min
    pub fn watermark_ok(&self, order: usize, level: usize) -> bool {
        let min = self._watermark[level].load(Ordering::Relaxed)
            .saturating_add((1usize << order).saturating_sub(1));
        self.free_pages.load(Ordering::Acquire) >= min
    }

    /// Get free pages at a specific order
    pub fn free_pages_order(&self, order: usize) -> usize {
        if order > MAX_ORDER {
            return 0;
        }
        self.free_area[order].nr_free()
    }

    /// Allocate pages from this zone
    ///
    /// # Arguments
    /// - `order`: Order of allocation (2^order pages)
    ///
    /// # Returns
    /// - PFN of allocated block, or None if allocation fails
    pub fn alloc_pages(&self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        let _guard = self.lock.lock();
        self.alloc_pages_locked(order)
    }

    /// Pop up to `n` ORDER-0 pages under ONE lock acquisition.
    ///
    /// Page-cache batch inserts used to pay one zone lock round trip (plus
    /// descriptor init) per 4 KiB page — the dominant cold-read cost once
    /// the block layer was batched. Order-0 only: eviction frees these
    /// pages individually, so no compound/allocation-group semantics are
    /// involved. Returns the number of pages popped; their PFNs are
    /// written to `out`.
    ///
    /// BUFW-PARITY (2026-11-18): the per-page body inside the lock also
    /// paid one locked RMW on EACH shared line per page — free_pages,
    /// free_area[0].nr_free, the ZTRACE_ALLOC ring cursor and the vmstat
    /// pgalloc counter — plus the full order-scan and a double descriptor
    /// init (alloc_from_order stamps refcount/order/next_free, then
    /// alloc_pages_locked's loop re-stamps flags/mapcount/refcount). The
    /// TSC phase probe measured ~188 c/page here (~48k cycles per 1 MiB
    /// buffered write, the row's largest non-copy term after the
    /// mandatory cold-frame fill). This bulk form pops the order-0 free
    /// list directly and claims every shared counter ONCE PER BATCH:
    /// per page only the forensic seed-check loads, the head-pointer
    /// store and ONE descriptor init remain (MM-H2 holds — descriptor
    /// init stays inside the zone lock). When the order-0 list empties
    /// mid-batch the existing single-page path (alloc_pages_locked(0),
    /// which splits a higher-order block and re-seeds the order-0 list)
    /// serves the remainder with its original exact accounting; the
    /// ZTRACE ring cursor is claimed once per batch for the bulk-popped
    /// segment only, so every entry still names a real page (the ring is
    /// a best-effort relaxed forensic ledger — see mmforensics).
    pub fn alloc_pages_order0_batch(&self, n: usize, out: &mut [usize]) -> usize {
        let _guard = self.lock.lock();
        let want = n.min(out.len());
        let mut got = 0usize;
        let mut bulk = 0usize; // pages served off the zone fast path:
        // list-drain pops + full-shatter hand-outs (free_pages/vmstat/
        // ZTRACE are claimed once for all of them below).
        let mut drained = 0usize; // subset popped off the order-0 free
        // list (free_area[0].nr_free covers exactly these; shatter
        // hand-outs were counted at nr_free[shard_order], not here).
        let mut bulk_pfns: [usize; 64] = [0; 64]; // ZTRACE entries (capped segments)
        const BULK_PFNS_MAX: usize = 64;

        while got < want {
            // Bulk segment: drain the order-0 free list directly.
            let mut head = self.free_area[0].free_list.load(Ordering::Acquire);
            while head != FREE_LIST_NULL && got < want {
                let page = pfn_to_page(head);
                if page.is_null() {
                    // Defensive: descriptor missing — treat as list end
                    // (same stance as remove_from_free_list).
                    self.free_area[0].free_list.store(FREE_LIST_NULL, Ordering::Release);
                    head = FREE_LIST_NULL;
                    break;
                }
                // head came from the order-0 free list under the zone
                // lock; descriptor access is exclusive here
                // (INV-MM-2 locked-chain arm).
                let p = super::r#unsafe::page_ref_of(head).unwrap();
                let (rc, linked, next) = (
                    p.refcount(),
                    p.test_flag(PageFlag::OnFreelist),
                    p.next_free(),
                );
                // SEED-ALLOC-BAD-POP twin (fake-OOM family): a popped block
                // must be refcount==0 and carry OnFreelist — same forensic
                // as alloc_from_order, kept per page.
                if rc != 0 || !linked {
                    if SEED_ALARMS.fetch_add(1, Ordering::Relaxed) < 8 {
                        crate::pr_err!(
                            "SEED-ALLOC-BAD-POP pfn={} bulk order=0: refcount={} on_freelist={} zone_free_cnt={}",
                            head, rc, linked,
                            self.free_pages.load(Ordering::Relaxed)
                        );
                    }
                }
                // Pop: head := next (Release pairs with the free-side push).
                self.free_area[0].free_list.store(next, Ordering::Release);
                // Descriptor init INSIDE the zone lock (review MM-H2):
                // leaving refcount=0 (free-looking) outside the lock let a
                // concurrent free path treat these pages as buddies and
                // corrupt the free lists. One pass — flags, mapcount,
                // refcount, order, and the stale next_free severed.
                {
                    let p = super::r#unsafe::page_of(head).unwrap();
                    p.clear_all_flags();
                    p.reset_mapcount();
                    p.set_refcount(1);
                    p.set_order(0);
                    p.set_next_free(FREE_LIST_NULL);
                }
                out[got] = head;
                if bulk < BULK_PFNS_MAX {
                    bulk_pfns[bulk] = head;
                }
                got += 1;
                bulk += 1;
                drained += 1;
                head = next;
            }
            if got >= want {
                break;
            }
            // Order-0 exhausted. Full-shatter arm (BUFW-PARITY): when the
            // remainder still wants a run of pages, pop the SMALLEST
            // available higher-order block ONCE and hand its members out
            // directly — one list unlink, one batched set of shared-line
            // claims, per page only the in-use descriptor init. The old
            // per-page split path (alloc_pages_locked(0)) remains for
            // small remainders: it leaves buddies at every intermediate
            // order, which keeps fragmentation coarser when only a page
            // or two are needed. Frees re-merge shattered order-0 runs,
            // so the choice is a policy, not an invariant.
            let remaining = want - got;
            if remaining >= 8 {
                let mut shard_order = 0usize;
                for m in 1..=MAX_ORDER {
                    if self.free_area[m].free_list.load(Ordering::Acquire) != FREE_LIST_NULL {
                        shard_order = m;
                        break;
                    }
                }
                if shard_order > 0 {
                    let block = self.free_area[shard_order].free_list.load(Ordering::Acquire);
                    let page = pfn_to_page(block);
                    if !page.is_null() {
                        // block came from the shard_order free list
                        // under the zone lock (INV-MM-2).
                        let p = super::r#unsafe::page_ref_of(block).unwrap();
                        let (rc, linked) = (
                            p.refcount(),
                            p.test_flag(PageFlag::OnFreelist),
                        );
                        if rc != 0 || !linked {
                            if SEED_ALARMS.fetch_add(1, Ordering::Relaxed) < 8 {
                                crate::pr_err!(
                                    "SEED-ALLOC-BAD-POP pfn={} shatter order={}: refcount={} on_freelist={} zone_free_cnt={}",
                                    block, shard_order, rc, linked,
                                    self.free_pages.load(Ordering::Relaxed)
                                );
                            }
                        }
                    }
                    // Unlink the block from its list (one unlink;
                    // remove_from_free_list owns the nr_free decrement
                    // and clears the head's OnFreelist bit).
                    self.remove_from_free_list(block, shard_order);
                    let block_n = 1usize << shard_order;
                    let n_take = block_n.min(remaining);
                    // Hand out the first n_take members as order-0 pages.
                    for i in 0..n_take {
                        if let Some(p) = page_desc(block + i) {
                            // SAFETY: member of the just-popped block;
                            // zone lock held (MM-H2).
                                                    p.clear_all_flags();
                            p.reset_mapcount();
                            p.set_refcount(1);
                            p.set_order(0);
                            p.set_next_free(FREE_LIST_NULL);
                        }
                        out[got + i] = block + i;
                        if bulk < BULK_PFNS_MAX {
                            bulk_pfns[bulk] = block + i;
                        }
                        bulk += 1;
                    }
                    got += n_take;
                    // Excess members go to the order-0 free list (they
                    // stay "free" for accounting — only the handed-out
                    // n_take leave it; the batched free_pages sub below
                    // covers exactly those).
                    let excess = block_n - n_take;
                    if excess > 0 {
                        let mut h = self.free_area[0].free_list.load(Ordering::Acquire);
                        for j in (0..excess).rev() {
                            if let Some(p) = page_desc(block + n_take + j) {
                                // SAFETY: member of the just-popped block;
                                // zone lock held.
                                                            p.set_refcount(0);
                                p.set_order(0);
                                p.set_next_free(h);
                                p.set_flag(PageFlag::OnFreelist);
                            }
                            h = block + n_take + j;
                        }
                        self.free_area[0].free_list.store(h, Ordering::Release);
                        self.free_area[0].nr_free.fetch_add(excess, Ordering::Relaxed);
                    }
                    continue;
                }
            }
            // Small remainder: the original per-page path (its
            // free_pages/nr_free/vmstat/ZTRACE accounting stays per-page
            // and exact — the counters below only cover the bulk segment).
            match self.alloc_pages_locked(0) {
                Some(pfn) => {
                    out[got] = pfn;
                    got += 1;
                }
                None => break,
            }
        }

        // Batched shared-line accounting for the bulk segment only.
        if bulk > 0 {
            self.free_pages.fetch_sub(bulk, Ordering::Relaxed);
            self.free_area[0].nr_free.fetch_sub(drained, Ordering::Relaxed);
            super::hotpath::note_alloc(self.zone_type, bulk as u64);
            let claimed = bulk.min(BULK_PFNS_MAX);
            let base = ZALLOC_CUR.fetch_add(claimed, Ordering::Relaxed);
            for i in 0..claimed {
                ZTRACE_ALLOC[(base + i) % ZTRACE_ALLOC.len()]
                    .store((bulk_pfns[i] as u64) << 4, Ordering::Relaxed);
            }
        }
        got
    }

    /// alloc_pages body WITHOUT taking the lock — caller holds self.lock.
    fn alloc_pages_locked(&self, order: usize) -> Option<usize> {
        if order > MAX_ORDER {
            return None;
        }

        // Find a free block at this order or higher
        for current_order in order..=MAX_ORDER {
            let head = self.free_area[current_order].free_list.load(Ordering::Acquire);
            if head != FREE_LIST_NULL {
                // Found a block, remove it and split if needed
                let result = self.alloc_from_order(current_order, order);
                if let Some(pfn) = result {
                    // Initialize ALL page descriptors of the block while still
                    // holding the zone lock: leaving refcount=0 (free-looking)
                    // outside the lock let a concurrent free path treat these
                    // pages as buddies and corrupt the free lists
                    // (review MM-H2).
                    //
                    // NOTE on the compound-page convention (review 4.4):
                    // Linux keeps tail refcounts at 0 with only the head
                    // counted.  Rux CANNOT adopt that: the arch teardown
                    // path (free_user_page_tables) walks every PTE and
                    // put_page()s EACH member page individually, so every
                    // member must own a count.  Blocks here are "allocation
                    // groups", not Linux compound pages.  The corresponding
                    // protection is the member-state check in free_pages()
                    // (an order>0 block is only released when every member
                    // is unreferenced).
                    // FORENSIC alloc ledger (atomic ring; crash-time replay).
                    ZTRACE_ALLOC[ZALLOC_CUR.fetch_add(1, Ordering::Relaxed) % ZTRACE_ALLOC.len()]
                        .store((pfn as u64) << 4 | (order as u64 & 0xF), Ordering::Relaxed);
                    let count = 1usize << order;
                    // /proc/vmstat pgalloc_* counters.
                    super::hotpath::note_alloc(self.zone_type, count as u64);
                    for i in 0..count {
                        if let Some(page) = page_desc(pfn + i) {
                            // SAFETY: pfn+i from alloc_from_order is valid; lock held.
                                                    page.clear_all_flags();
                            page.reset_mapcount();
                            page.set_refcount(1);
                        }
                    }
                    if let Some(leader) = page_desc(pfn) {
                        // SAFETY: leader pfn valid; lock held.
                                            leader.set_order(order as u8);
                        // SP-7 (stress-port r7): NO Referenced flag
                        // here. The reference kernel's prep_new_page
                        // hands out flag-clear pages — PG_referenced
                        // exists only after mark_page_accessed(). The
                        // old plant made every freshly allocated page
                        // look hot to reclaim: measured on the
                        // exhaustion class, 46% of all examined anon
                        // candidates rotated away on flags that no
                        // access ever set (the SP-7 VSCAN-CENSUS
                        // baseline; nothing in Rux sets Referenced on
                        // anon pages legitimately — lru_note_refs has
                        // no callers and the hardware accessed bit is
                        // never propagated).
                    }
                }
                return result;
            }
        }

        None
    }

    /// Allocate a block from a specific order level
    fn alloc_from_order(&self, current_order: usize, target_order: usize) -> Option<usize> {
        let head = self.free_area[current_order].free_list.load(Ordering::Acquire);
        if head == FREE_LIST_NULL {
            return None;
        }

        // FORENSIC seed detector (fake-OOM family): a block popped from a
        // free list must be refcount==0 AND carry OnFreelist. A live
        // refcount here means a live frame was wired into the buddy (it is
        // about to be handed out a second time); a clear OnFreelist means
        // the head is not actually linked (counter/list desync). Either is
        // a primary corruption event — print the first few.
        {
            // Read-only descriptor access under the zone lock
            // (INV-MM-2 locked-chain arm).
            if let Some(p) = super::r#unsafe::page_ref_of(head) {
                let (rc, linked) = (
                    p.refcount(),
                    p.test_flag(PageFlag::OnFreelist),
                );
                if rc != 0 || !linked {
                    if SEED_ALARMS.fetch_add(1, Ordering::Relaxed) < 8 {
                        crate::pr_err!(
                            "SEED-ALLOC-BAD-POP pfn={} from order={} (want {}): refcount={} on_freelist={} zone_free_cnt={}",
                            head, current_order, target_order, rc, linked,
                            self.free_pages.load(Ordering::Relaxed)
                        );
                    }
                }
            }
        }

        // Remove from free list
        self.remove_from_free_list(head, current_order);

        let mut pfn = head;
        let mut order = current_order;

        // Split blocks until we reach target order
        while order > target_order {
            order -= 1;
            let buddy_pfn = pfn + (1usize << order);

            // Add buddy to free list
            self.add_to_free_list(buddy_pfn, order);
        }

        // Update free pages count
        self.free_pages.fetch_sub(1usize << target_order, Ordering::Relaxed);

        // Update page descriptor
        if let Some(page) = page_desc(pfn) {
            // SAFETY: pfn is within zone range (validated above), lock is held.
                    page.set_refcount(1);
            page.set_order(target_order as u8);
            // CRITICAL: Clear next_free to prevent stale pointer corruption
            page.set_next_free(FREE_LIST_NULL);
        }

        Some(pfn)
    }

    /// Free pages to this zone
    ///
    /// # Arguments
    /// - `pfn`: Page frame number of the block to free
    /// - `order`: Order of the block
    pub fn free_pages(&self, pfn: usize, order: usize) {
        // /proc/vmstat pgfree counter (the pages really go back to the
        // free lists further down, after the range check).
        super::hotpath::note_free(1u64 << order);
        // FORENSIC free ledger.
        ZTRACE_FREE[ZFREE_CUR.fetch_add(1, Ordering::Relaxed) % ZTRACE_FREE.len()]
            .store((pfn as u64) << 4 | (order as u64 & 0xF), Ordering::Relaxed);

        if order > MAX_ORDER {
            return;
        }

        // Check if pfn is within zone range
        let start = self.zone_start_pfn.load(Ordering::Acquire);
        let end = self.zone_end_pfn.load(Ordering::Acquire);
        if pfn < start || pfn >= end {
            return;
        }

        // FORENSIC (fake-OOM family) RAW-FREE WATCHPOINT: a frame carrying a
        // LIVE page-table ledger stamp is about to be freed through the RAW
        // path (not free_page_table_checked). That stamps-out a live table
        // tree — the seed of the stale-tree/cascade corruption. The stamp
        // stays live, so the alloc side can also catch the frame being
        // handed out again ("DOUBLE-ALLOC still live"). Print with a
        // frame-pointer chain to name the offender.
        if crate::arch::mm::mmu_init::alloc_stage_is_late()
            && crate::arch::mm::mmu_init::PtLedger::is_live_table(pfn as u64)
        {
            static RAWFREE_REPORTS: AtomicUsize = AtomicUsize::new(0);
            if RAWFREE_REPORTS.fetch_add(1, Ordering::Relaxed) < 6 {
                crate::pr_err!(
                    "RAW-FREE-OF-LIVE-TABLE pfn={:#x} order={} by pid={} — stack:",
                    pfn, order,
                    crate::process::hotpath::current_pid()
                );
                let mut frames: [u64; 10] = [0; 10];
                // Reads the frame-pointer chain of the current stack
                // (INV-MM-2 diag arm: this CPU's own stack).
                {
                    let s0 = crate::debug::backtrace::current_frame_pointer();
                    super::r#unsafe::walk_fp_chain(s0, &mut frames);
                }
                for f in frames.iter() {
                    if *f == 0 {
                        break;
                    }
                    crate::pr_err!("  RAW-FREE frame {:#x}", f);
                }
            }
            // mmforensics (pcachewedge §6 residual): deep capture — full
            // tracking state, the frame's incarnation history, and the
            // bounded reverse scan of every live tree for links naming
            // this pfn (the stale-pointer origin). Runs BEFORE the zone
            // lock is taken; internally gated + rate-limited.
            crate::debug::mmforensics::capture_frame(pfn, "raw-free-live-table");
            // mmforensics fix (2026-11-15): REFUSE the free and QUARANTINE
            // the frame. The capture evidence (pfn 0x6bd1 boot) showed the
            // old print-then-free behavior is the double-availability
            // engine itself: the frame re-enters the buddy while its
            // previous owners (live table stamp, cache entry, stale PTEs)
            // all still reference it, and every re-allocation re-collides
            // (observed: one frame re-stamped by five trees in one boot,
            // ending in a SIGSEGV cascade). A live-stamped frame has NO
            // legitimate raw free (legit table frees clear the stamp via
            // PtLedger::take() first) — leak it and mark it poisoned so
            // no other free path ever picks it up either.
            crate::arch::mm::mmu_init::PtLedger::poison_table(pfn as u64);
            return;
        }

        let _guard = self.lock.lock();

        // R21-2: reset the descriptor — every raw-free caller's comments
        // claim this happens here, but no store existed: pages landed on
        // the freelist with refcount==1, is_buddy_free rejected every
        // merge involving them (order-0 accumulation, INV-REF-2 violation)
        // and meminfo miscounted them as used.
        {
            if let Some(page) = page_desc(pfn) {
                            page.set_refcount(0);
                page.clear_flag(super::types::PageFlag::Referenced);
            }
        }

        // R15-3: TRUE double-free tripwire on the primary allocator path.
        // OnFreelist is the authoritative linked-state bit: if it is still
        // set, the page is STILL LINKED in a freelist, so this is a genuine
        // double free. Proceeding would relink an already-linked block
        // (two lists share its next_free) and corrupt both — leak it
        // instead, like Linux's bad_page() path.
        //
        // pcachewedge (2026-11-15): OnPcp is the SAME class of linked
        // state — the frame sits on a per-CPU pageset row. A raw zone
        // free of a pcp-cached frame (the teardown echo of a stale tree
        // reference: the frame was freed once before, cached, and the
        // stale walk frees it again) links it into the buddy WHILE the
        // row still hands it out — one frame, two future owners, and
        // the re-alloc stamps it as a live page table in between (the
        // observed RAW-FREE-OF-LIVE-TABLE mid-flight re-stamp). Refuse
        // identically: leak + taint, never double-link.
        {
            if let Some(page) = page_desc(pfn) {
                            if page.test_flag(PageFlag::OnFreelist)
                    || page.test_flag(PageFlag::OnPcp)
                {
                    // Taint-only reporting: this runs under the zone
                    // lock; raw putchar / printk here can wedge the
                    // console path (same discipline as the heap
                    // buddy's R43 tripwires).
                    crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
                    DOUBLE_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
        // QUARANTINE refusal (mmforensics lane): a poisoned frame (ever
        // double-tracked — see PtLedger::poison_table) must never re-enter
        // the allocator; its stale references make every future owner a
        // collision. Silent + taint + counter (zone lock held — same
        // discipline as R15-3 above).
        if crate::arch::mm::mmu_init::PtLedger::is_poisoned(pfn as u64) {
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            crate::debug::driftwatch::MM_POISON_FREES.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // Member-state invariant for high-order frees (review 4.4):
        // the block's members are reset by add_to_free_list() when the
        // (possibly merged) block joins the free list (R25-4 zeroes every
        // member).  A caller freeing an order>0 block therefore must not
        // have outstanding put_page-able references on any member —
        // members carry per-page refcounts in Rux (see alloc_pages note);
        // freeing a block whose members are still PTE-mapped is a caller
        // bug that no metadata here can detect.
        let mut current_pfn = pfn;
        let mut current_order = order;

        // Try to merge with buddy
        while current_order < MAX_ORDER {
            let buddy_pfn = self.find_buddy(current_pfn, current_order);

            // Check if buddy is free
            if !self.is_buddy_free(buddy_pfn, current_order) {
                break;
            }

            // Remove buddy from free list
            self.remove_from_free_list(buddy_pfn, current_order);

            // Merge: take lower address
            current_pfn = current_pfn.min(buddy_pfn);
            current_order += 1;
        }

        // Add merged block to free list
        self.add_to_free_list(current_pfn, current_order);

        // Update free pages count
        self.free_pages.fetch_add(1usize << order, Ordering::Relaxed);
    }

    /// Find buddy PFN
    fn find_buddy(&self, pfn: usize, order: usize) -> usize {
        pfn ^ (1usize << order)
    }

    /// Check if buddy is free
    ///
    /// Mirrors `page_is_buddy()` in mm/page_alloc.c: the candidate must
    /// actually be ON the free list (PageBuddy/OnFreelist), not merely
    /// look free.  Checking only `refcount == 0 && order == order`
    /// misjudged pages that are free-looking but not linked (PCP-resident
    /// pages, freshly-allocated-but-not-yet-initialized pages, reserved
    /// pages outside memblock tracking) as mergeable — merging such a page
    /// gave one PFN two owners (review 4.5).
    fn is_buddy_free(&self, buddy_pfn: usize, order: usize) -> bool {
        // Check if buddy is in zone range
        let start = self.zone_start_pfn.load(Ordering::Acquire);
        let end = self.zone_end_pfn.load(Ordering::Acquire);
        if buddy_pfn < start || buddy_pfn >= end {
            return false;
        }

        // buddy_pfn is within zone range; read-only access — the zone
        // lock is held so the descriptor's linked-state is stable
        // (INV-MM-2 locked-chain arm).
        match super::r#unsafe::page_ref_of(buddy_pfn) {
            // A buddy is only suitable for merging if ALL of:
            // 1. refcount == 0 (free)
            // 2. OnFreelist is set (authoritative linked-state bit)
            // 3. order matches the free-list order being merged at
            // 4. not a reserved page
            Some(page) => {
                page.refcount() == 0
                    && page.test_flag(PageFlag::OnFreelist)
                    && page.order() == order as u8
                    && !page.test_flag(PageFlag::Reserved)
            }
            None => false,
        }
    }

    /// Add block to free list
    fn add_to_free_list(&self, pfn: usize, order: usize) {
        // R25-4: every page joining a freelist takes refcount 0 — split
        // buddies previously re-entered with the parent allocation's
        // refcount==1, permanently disabling merges (INV-BUDDY-1).
        {
            let count = 1usize << order;
            for i in 0..count {
                if let Some(pg) = super::r#unsafe::page_of(pfn + i) {
                    pg.set_refcount(0);
                }
            }
        }
        let head = self.free_area[order].free_list.load(Ordering::Acquire);

        // Update Page descriptor's free list pointers
        if let Some(page) = page_desc(pfn) {
            // SAFETY: pfn is from alloc_pages (valid), lock is held.
                    page.set_next_free(head);
            page.set_flag(PageFlag::OnFreelist);
            page.set_order(order as u8);
        }

        // Set new head
        self.free_area[order].free_list.store(pfn, Ordering::Release);
        self.free_area[order].inc_free();
    }

    /// Add block to free list during initialization (no buddy merging)
    /// This is used during zone initialization when we know pages are not in any list
    pub fn add_to_free_list_init(&self, pfn: usize, order: usize) {
        let head = self.free_area[order].free_list.load(Ordering::Acquire);

        // Update Page descriptor's free list pointers
        if let Some(page) = page_desc(pfn) {
            // SAFETY: pfn is within zone range, called during init (single-threaded).
                    page.set_next_free(head);
            page.set_order(order as u8);
            page.set_flag(PageFlag::OnFreelist);
        }

        // Set new head
        self.free_area[order].free_list.store(pfn, Ordering::Release);
        self.free_area[order].inc_free();

        // Update free pages count
        self.free_pages.fetch_add(1usize << order, Ordering::Relaxed);
    }

    /// Remove block from free list
    fn remove_from_free_list(&self, pfn: usize, order: usize) {
        let head = self.free_area[order].free_list.load(Ordering::Acquire);

        if head == pfn {
            // Removing head, get next from Page descriptor
            let next = {
                // pfn is from the free list, lock held (INV-MM-2).
                match super::r#unsafe::page_ref_of(pfn) {
                    Some(page) => page.next_free(),
                    None => FREE_LIST_NULL,
                }
            };
            self.free_area[order].free_list.store(next, Ordering::Release);
        } else {
            // Need to walk the list to find and remove
            let mut prev = head;
            loop {
                if prev == FREE_LIST_NULL {
                    break;
                }

                // prev_page from the chain, lock held (INV-MM-2).
                let Some(prev_page) = super::r#unsafe::page_of(prev) else {
                    break;
                };

                let next = prev_page.next_free();
                if next == pfn {
                    // Found it, update prev's next pointer
                    let new_next = match super::r#unsafe::page_ref_of(pfn) {
                        Some(target) => target.next_free(),
                        None => FREE_LIST_NULL,
                    };
                    prev_page.set_next_free(new_next);
                    break;
                }
                prev = next;
            }
        }

        self.free_area[order].dec_free();
        // R15-6: the removed page leaves the freelist — clear the
        // authoritative linked bit.
        if let Some(rp) = super::r#unsafe::page_of(pfn) {
            rp.clear_flag(PageFlag::OnFreelist);
        }
    }

    /// Allocate a single page from the buddy free list (for compaction).
    ///
    /// Unlike `alloc_pages()`, this does not update page descriptor refcount
    /// or flags — the caller is responsible for that.
    pub fn alloc_single_page(&self) -> Option<usize> {
        // R31-6: serialize like alloc_pages — the sole caller (compaction)
        // runs at peak zone pressure; the unlocked twin corrupted freelists.
        let _guard = self.lock.lock();
        self.alloc_from_order(0, 0)
    }

    /// Check if a free block of the given order exists.
    pub fn has_free_block(&self, order: usize) -> bool {
        if order > MAX_ORDER {
            return false;
        }
        !self.free_area[order].is_empty()
    }

    /// FORENSIC (fake-OOM family): compare each order's DECLARED free-block
    /// count with the length actually reachable by walking the `next_free`
    /// chain. A declared count above the walked length means the chain is
    /// broken (a linked page's `next_free` no longer points into the list —
    /// every page after the break is unreachable to the allocator while the
    /// counters still claim it). Walk is bounded and cycle-guarded; it runs
    /// WITHOUT the zone lock (diagnostic path only).
    ///
    /// Returns (declared_blocks, walked_blocks, head_pfn) per order.
    pub fn forensic_free_report(&self) -> [(usize, usize, usize); MAX_ORDER + 1] {
        let mut out = [(0usize, 0usize, 0usize); MAX_ORDER + 1];
        for order in 0..=MAX_ORDER {
            let declared = self.free_area[order].nr_free();
            let mut cur = self.free_area[order].free_list.load(Ordering::Acquire);
            let head = cur;
            let mut walked = 0usize;
            let mut guard = 0usize;
            while cur != FREE_LIST_NULL {
                walked += 1;
                guard += 1;
                if guard > 4_000_000 {
                    break; // runaway/cyclic chain
                }
                // Read-only descriptor access on the diagnostic path
                // (INV-MM-2 chain arm).
                let Some(page) = super::r#unsafe::page_ref_of(cur) else {
                    break;
                };
                let next = page.next_free();
                if next == cur {
                    break; // self-loop
                }
                cur = next;
            }
            out[order] = (declared, walked, if head == FREE_LIST_NULL { 0 } else { head });
        }
        out
    }

    /// Get zone statistics
    pub fn stats(&self) -> ZoneStats {
        ZoneStats {
            zone_type: self.zone_type,
            zone_id: self.zone_id,
            node_id: self.node_id,
            start_pfn: self.start_pfn(),
            end_pfn: self.end_pfn(),
            spanned_pages: self.spanned_pages(),
            present_pages: self.present_pages(),
            managed_pages: self.managed_pages(),
            free_pages: self.nr_free(),
            free_blocks: [
                self.free_area[0].nr_free(),
                self.free_area[1].nr_free(),
                self.free_area[2].nr_free(),
                self.free_area[3].nr_free(),
                self.free_area[4].nr_free(),
                self.free_area[5].nr_free(),
                self.free_area[6].nr_free(),
                self.free_area[7].nr_free(),
                self.free_area[8].nr_free(),
                self.free_area[9].nr_free(),
                self.free_area[10].nr_free(),
            ],
        }
    }
}


// ==================== GFP Flags ====================

impl GfpFlags {
    /// Normal kernel allocation
    pub const GFP_KERNEL: GfpFlags = GfpFlags(0x01);

    /// User space allocation
    pub const GFP_USER: GfpFlags = GfpFlags(0x02);

    /// High priority allocation (can use atomic reserves)
    pub const GFP_ATOMIC: GfpFlags = GfpFlags(0x04);

    /// DMA compatible allocation
    pub const GFP_DMA: GfpFlags = GfpFlags(0x08);

    /// DMA32 compatible allocation
    pub const GFP_DMA32: GfpFlags = GfpFlags(0x10);

    /// Zero pages on allocation
    pub const __GFP_ZERO: GfpFlags = GfpFlags(0x100);

    /// High memory allocation (not used on RISC-V)
    pub const __GFP_HIGHMEM: GfpFlags = GfpFlags(0x200);

    /// Movable allocation
    pub const __GFP_MOVABLE: GfpFlags = GfpFlags(0x400);

    /// Get zone type for this GFP mask
    pub fn zone_type(&self) -> ZoneType {
        if self.0 & Self::GFP_DMA.0 != 0 {
            ZoneType::ZoneDma
        } else if self.0 & Self::GFP_DMA32.0 != 0 {
            ZoneType::ZoneDma32
        } else if self.0 & Self::__GFP_MOVABLE.0 != 0 {
            ZoneType::ZoneMovable
        } else {
            ZoneType::ZoneNormal
        }
    }
}

// ==================== Helper Functions ====================

/// Integer square root (Newton's method).
/// Following int_sqrt() in lib/math/int_sqrt.c.
fn int_sqrt(n: usize) -> usize {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Setup per-zone watermarks following `__setup_per_zone_wmarks()` in
/// mm/page_alloc.c.
///
/// # Arguments
/// - `zones`: iterator over mutable references to initialised zones
/// - `total_managed`: sum of managed_pages across all zones
pub fn setup_per_zone_wmarks(
    zones: &[&Zone],
    total_managed: usize,
) {
    if total_managed == 0 {
        return;
    }

    // min_free_kbytes = int_sqrt(lowmem_kbytes * 16)
    // clamped to [128, 262144]
    // (mm/page_alloc.c __setup_per_zone_wmarks(); the historical "/ 4" here
    // made every watermark four times smaller than Linux's)
    let managed_kb = total_managed * PAGE_SIZE / 1024;
    let mut min_free_kbytes = int_sqrt(managed_kb * 16);
    if min_free_kbytes < 128 {
        min_free_kbytes = 128;
    }
    if min_free_kbytes > 262144 {
        min_free_kbytes = 262144;
    }

    // Convert to pages (PAGE_SIZE = 4096 = 4 KB)
    let pages_min = min_free_kbytes * 1024 / PAGE_SIZE;

    for zone in zones.iter() {
        let zone_managed = zone.managed_pages.load(Ordering::Acquire);
        if zone_managed == 0 {
            continue;
        }

        // WMARK_MIN = max(pages_min, pages_min * zone_managed / total_managed)
        let wmark_min = core::cmp::max(
            pages_min,
            pages_min * zone_managed / total_managed,
        );

        // watermark_gap = max(wmark_min / 4, zone_managed * 10 / 10000)
        let gap = core::cmp::max(
            wmark_min / 4,
            zone_managed * 10 / 10000,
        );

        let wmark_low = wmark_min + gap;
        let wmark_high = wmark_low + gap;

        zone.set_watermark(WMARK_MIN, wmark_min);
        zone.set_watermark(WMARK_LOW, wmark_low);
        zone.set_watermark(WMARK_HIGH, wmark_high);
    }
}

/// Convert PFN to physical address
/// Returns 0 if the multiplication would overflow
pub fn pfn_to_phys(pfn: usize) -> usize {
    pfn.checked_mul(PAGE_SIZE).unwrap_or(0)
}

/// Convert physical address to PFN
pub fn phys_to_pfn(phys: usize) -> usize {
    phys / PAGE_SIZE
}

/// Print zone information
pub fn print_zone_info(zone: &Zone) {
    crate::println!("Zone {} (Node {}):", zone.zone_type().name(), zone.node_id());
    crate::println!("  Start PFN:    {:#x}", zone.start_pfn());
    crate::println!("  End PFN:      {:#x}", zone.end_pfn());
    crate::println!("  Spanned:      {} pages ({} MB)",
        zone.spanned_pages(),
        zone.spanned_pages() * PAGE_SIZE / (1024 * 1024));
    crate::println!("  Present:      {} pages", zone.present_pages());
    crate::println!("  Managed:      {} pages", zone.managed_pages());
    crate::println!("  Free:         {} pages ({} MB)",
        zone.nr_free(),
        zone.nr_free() * PAGE_SIZE / (1024 * 1024));
    crate::println!("  Watermarks:   min={} low={} high={}",
        zone.watermark(WMARK_MIN),
        zone.watermark(WMARK_LOW),
        zone.watermark(WMARK_HIGH));
    crate::println!("  Free blocks per order:");
    for order in 0..=MAX_ORDER {
        let nr = zone.free_pages_order(order);
        if nr > 0 {
            crate::println!("    Order {}: {} blocks ({} pages each)",
                order, nr, 1usize << order);
        }
    }
}

// FORENSIC zone alloc/free ledgers for crash-time replay.
pub static ZTRACE_ALLOC: [core::sync::atomic::AtomicU64; 4096] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 4096];
pub static ZALLOC_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub static ZTRACE_FREE: [core::sync::atomic::AtomicU64; 4096] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 4096];
pub static ZFREE_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// FORENSIC (fake-OOM family): rate-limit counters for the seed detectors —
// SEED-ALLOC-BAD-POP (allocating a block that was live or unlinked) and the
// OnFreelist double-free tripwire in free_pages().
pub static SEED_ALARMS: AtomicUsize = AtomicUsize::new(0);
pub static DOUBLE_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);


// ════════════════ vmstat ══════════════════════════════
// (was vmstat.rs)
// Kernel VM statistics — the counters behind Linux's /proc/vmstat.
//
// The counters live here (the memory manager owns them, like Linux's
// mm/vmstat.c); `fs/procfs/vmstat.rs` renders them in Linux's
// key/value format.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// Page faults taken (Linux's `pgfault`).
pub static PGFAULT: AtomicU64 = AtomicU64::new(0);

/// Page faults that had to go to the backing store (Linux's
/// `pgmajfault`).
///
/// Rux populates file-backed pages from the page cache rather than by
/// reading from the block device inside the fault handler, so there is
/// no fault-time disk read to classify yet — the counter stays 0 until
/// that path exists. It is wired (`note_major_fault`) so the plumbing
/// is in place when it does.
pub static PGMAJFAULT: AtomicU64 = AtomicU64::new(0);

/// Pages returned to the free lists (Linux's `pgfree`).
pub static PGFREE: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the Normal zone (Linux's `pgalloc_normal`).
pub static PGALLOC_NORMAL: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the Movable zone (Linux's `pgalloc_movable`).
pub static PGALLOC_MOVABLE: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the DMA32 zone (Linux's `pgalloc_dma32`).
pub static PGALLOC_DMA32: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the DMA zone (Linux's `pgalloc_dma`).
pub static PGALLOC_DMA: AtomicU64 = AtomicU64::new(0);

/// Count one page fault.
pub fn note_fault() {
    PGFAULT.fetch_add(1, Ordering::Relaxed);
    // perf per-CPU page-fault accounting (PERF_COUNT_SW_PAGE_FAULTS;
    // one relaxed load when no perf event wants it).
    crate::perf::note_pfault();
}

/// Count one major page fault.
pub fn note_major_fault() {
    PGMAJFAULT.fetch_add(1, Ordering::Relaxed);
}

/// Count `pages` pages freed back to the buddy free lists.
pub fn note_free(pages: u64) {
    PGFREE.fetch_add(pages, Ordering::Relaxed);
}

/// Count `pages` pages allocated from `zone`.
pub fn note_alloc(zone: ZoneType, pages: u64) {
    let counter = match zone {
        ZoneType::ZoneNormal => &PGALLOC_NORMAL,
        ZoneType::ZoneMovable => &PGALLOC_MOVABLE,
        ZoneType::ZoneDma32 => &PGALLOC_DMA32,
        ZoneType::ZoneDma => &PGALLOC_DMA,
        // ZoneCount is a sentinel, not a real zone.
        _ => return,
    };
    counter.fetch_add(pages, Ordering::Relaxed);
}


// ════════════════ pglist ══════════════════════════════
// (was pglist.rs)
// Page List Data (pglist_data) - NUMA Node Management
//
// This module implements the pglist_data structure which represents
// a NUMA node's memory. On UMA systems, there is a single node.

// ==================== Pglist Data (NUMA Node) ====================


/// Maximum number of NUMA nodes
pub const MAX_NUMNODES: usize = 1;  // UMA system, single node

// ==================== LRU Constants ====================

/// DEF_PRIORITY — starting priority for kswapd reclaim loop.
pub const DEF_PRIORITY: i32 = 12;

impl PglistData {
    /// Create a new uninitialized pglist_data
    pub const fn new(node_id: usize) -> Self {
        Self {
            node_id,
            zones: [None, None, None, None],
            nr_zones: AtomicUsize::new(0),
            node_start_pfn: AtomicUsize::new(0),
            node_spanned_pages: AtomicUsize::new(0),
            node_present_pages: AtomicUsize::new(0),
            total_reserved_pages: AtomicUsize::new(0),
            initialized: AtomicBool::new(false),
            lru_heads: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_LRU_LISTS]
            },
            lru_tails: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_LRU_LISTS]
            },
            lru_sizes: {
                const INIT: AtomicUsize = AtomicUsize::new(0);
                [INIT; NR_LRU_LISTS]
            },
            lru_lock: Spinlock::new(()),
        }
    }

    /// Initialize the node with memory regions
    ///
    /// # Arguments
    /// - `start_pfn`: Start page frame number
    /// - `spanned_pages`: Total pages including holes
    /// - `present_pages`: Present pages (excluding holes)
    pub fn init(&mut self, start_pfn: usize, spanned_pages: usize, present_pages: usize) {
        self.node_start_pfn.store(start_pfn, Ordering::Release);
        self.node_spanned_pages.store(spanned_pages, Ordering::Release);
        self.node_present_pages.store(present_pages, Ordering::Release);
        self.initialized.store(true, Ordering::Release);
    }

    /// Add a zone to this node
    pub fn add_zone(&mut self, zone_type: ZoneType, zone: Zone) {
        let idx = zone_type as usize;
        if idx < MAX_NR_ZONES {
            self.zones[idx] = Some(zone);
            self.nr_zones.fetch_add(1, Ordering::Release);
        }
    }

    /// Get zone by type
    pub fn zone(&self, zone_type: ZoneType) -> Option<&Zone> {
        self.zones[zone_type as usize].as_ref()
    }

    /// Get mutable zone by type
    pub fn zone_mut(&mut self, zone_type: ZoneType) -> Option<&mut Zone> {
        self.zones[zone_type as usize].as_mut()
    }

    /// Get zone by index
    pub fn zone_at(&self, idx: usize) -> Option<&Zone> {
        if idx < MAX_NR_ZONES {
            self.zones[idx].as_ref()
        } else {
            None
        }
    }

    /// Get node ID
    pub fn node_id(&self) -> usize {
        self.node_id
    }

    /// Get number of zones
    pub fn nr_zones(&self) -> usize {
        self.nr_zones.load(Ordering::Acquire)
    }

    /// Get node start PFN
    pub fn start_pfn(&self) -> usize {
        self.node_start_pfn.load(Ordering::Acquire)
    }

    /// Get node spanned pages
    pub fn spanned_pages(&self) -> usize {
        self.node_spanned_pages.load(Ordering::Acquire)
    }

    /// Get node present pages
    pub fn present_pages(&self) -> usize {
        self.node_present_pages.load(Ordering::Acquire)
    }

    /// Get total free pages across all zones
    pub fn free_pages(&self) -> usize {
        let mut total = 0;
        for zone_opt in &self.zones {
            if let Some(zone) = zone_opt {
                if zone.is_initialized() {
                    total += zone.nr_free();
                }
            }
        }
        total
    }

    /// Check if node is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Get node statistics
    pub fn stats(&self) -> NodeStats {
        let mut zone_stats = Vec::new();
        for zone_opt in &self.zones {
            if let Some(zone) = zone_opt {
                if zone.is_initialized() {
                    zone_stats.push(zone.stats());
                }
            }
        }

        NodeStats {
            node_id: self.node_id,
            start_pfn: self.start_pfn(),
            spanned_pages: self.spanned_pages(),
            present_pages: self.present_pages(),
            total_free_pages: self.free_pages(),
            zones: zone_stats,
        }
    }
}


// ==================== Global Node Management ====================

/// Global node array (static storage)
/// For UMA systems, we only have node 0
pub(super) static mut NODE_DATA: [Option<PglistData>; MAX_NUMNODES] = [None];


/// Node count
static NODE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Initialize node data
///
/// Early boot only, before other CPUs are online (INV-MM-12 init arm
/// via the edge).
pub fn init_node_data() {
    super::r#unsafe::node_data_init();
    NODE_COUNT.store(1, Ordering::Release);
}

/// Get node by ID
///
/// # Safety
/// Returns reference to static data
pub fn node_data(node_id: usize) -> Option<&'static PglistData> {
    if node_id >= MAX_NUMNODES {
        return None;
    }
    // SAFETY: node_id is bounds-checked above; NODE_DATA is initialized by
    // init_node_data() before any call here.
    unsafe { NODE_DATA[node_id].as_ref() }
}

/// Get mutable node by ID
///
/// # Safety
/// Must be called during initialization only
pub fn node_data_mut(node_id: usize) -> Option<&'static mut PglistData> {
    if node_id >= MAX_NUMNODES {
        return None;
    }
    // SAFETY: node_id is bounds-checked above; caller must ensure exclusive
    // access (called only during init or with appropriate locking).
    unsafe { NODE_DATA[node_id].as_mut() }
}

/// Get first (and only, on UMA) node
pub fn first_online_node() -> Option<&'static PglistData> {
    node_data(0)
}

/// Get first mutable node.
///
/// # Safety
/// Caller must ensure exclusive access to the node.  On a single-node UMA
/// system this means no other thread may hold a mutable or shared reference
/// to the same `PglistData` simultaneously.  Typically this is safe only
/// during early boot or when the caller holds a lock that prevents
/// concurrent page allocation / reclaim on the same node.
pub unsafe fn first_online_node_mut() -> Option<&'static mut PglistData> {
    node_data_mut(0)
}

/// Get node count
pub fn num_online_nodes() -> usize {
    NODE_COUNT.load(Ordering::Acquire)
}

// ==================== Zone Selection ====================

/// Select zone for allocation based on GFP flags
///
/// # Arguments
/// - `gfp_flags`: GFP flags for the allocation
/// - `node`: Node to allocate from
///
/// # Returns
/// - The appropriate zone for this allocation
pub fn select_zone(gfp_flags: super::types::GfpFlags, node: &PglistData) -> Option<&Zone> {
    let zone_type = gfp_flags.zone_type();
    node.zone(zone_type)
}

/// Select mutable zone for allocation
pub fn select_zone_mut(gfp_flags: super::types::GfpFlags, node: &mut PglistData) -> Option<&mut Zone> {
    let zone_type = gfp_flags.zone_type();
    node.zone_mut(zone_type)
}

// ==================== Buddyinfo ====================

/// Print buddyinfo
pub fn print_buddyinfo() {
    for node_id in 0..num_online_nodes() {
        if let Some(node) = node_data(node_id) {
            crate::println!("Node {}, zone{}", node_id, "");

            // Print header
            crate::print!("  Order: ");
            for order in 0..=MAX_ORDER {
                crate::print!("{:4} ", order);
            }
            crate::println!("");

            // Print free count per zone
            for zone_type in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
                if let Some(zone) = node.zone(zone_type) {
                    if zone.is_initialized() {
                        crate::print!("  {:6} ", zone_type.name());
                        for order in 0..=MAX_ORDER {
                            crate::print!("{:4} ", zone.free_pages_order(order));
                        }
                        crate::println!("");
                    }
                }
            }
        }
    }
}

/// Print memory layout
pub fn print_zoneinfo() {
    for node_id in 0..num_online_nodes() {
        if let Some(node) = node_data(node_id) {
            crate::println!("Node {}", node_id);
            crate::println!("  spanned pages: {}", node.spanned_pages());
            crate::println!("  present pages: {}", node.present_pages());

            for zone_type in [ZoneType::ZoneDma, ZoneType::ZoneDma32, ZoneType::ZoneNormal, ZoneType::ZoneMovable] {
                if let Some(zone) = node.zone(zone_type) {
                    if zone.is_initialized() {
                        crate::println!("");
                        crate::println!("  zone {}", zone_type.name());
                        crate::println!("    start_pfn:     {:#x}", zone.start_pfn());
                        crate::println!("    spanned:       {}", zone.spanned_pages());
                        crate::println!("    present:       {}", zone.present_pages());
                        crate::println!("    managed:       {}", zone.managed_pages());
                        crate::println!("    free:          {}", zone.nr_free());
                        crate::println!("    buddy pages:");
                        for order in 0..=MAX_ORDER {
                            crate::println!("      order {}: {} blocks",
                                order, zone.free_pages_order(order));
                        }
                    }
                }
            }
        }
    }
}


// ════════════════ page_alloc ══════════════════════════
// (was page_alloc.rs)
// Physical Page Buddy Allocator
//
// This module implements a unified buddy system for physical page allocation.
// It provides APIs like __get_free_pages() and free_pages().

// ==================== Page Allocation API ====================

/// Allocation statistics for debugging
static ZONE_ALLOCS: AtomicUsize = AtomicUsize::new(0);
static LEGACY_ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// Direct-reclaim recursion guard (equivalent of Linux PF_MEMALLOC):
/// set while a task is inside vmscan's reclaim loop, so allocations made
/// BY reclaim (swap I/O paths) fail fast instead of recursing into
/// another try_to_free_pages round.
///
/// Leg-52 item 0 (fleet s19/s24-r30, stress-lane A/B verdict): the value
/// is the HOLDER'S PID (0 = free), not a bare 0/1 flag. The old flag had
/// exits only at the reclaim block's own returns (below); a task KILLED
/// while inside the block (the pagefault-OOM path can select the
/// sleeping holder itself, or any fatal SIGKILL lands mid-writeback
/// wait) funnels into do_exit without unwinding them, leaving the gate
/// stuck at 1 — every later allocation then takes the no-drain skip
/// path (the fake-OOM family; every fleet forensic block read
/// in_direct_reclaim=1, including each boot's first dump). do_exit now
/// clears the gate BY IDENTITY (reclaim_gate_exit_release), so the leak
/// is closed deterministically instead of hoping the block's exits ran.
static IN_DIRECT_RECLAIM: AtomicUsize = AtomicUsize::new(0);

/// How many times the exit path has cleared a gate the dying task still
/// held (each occurrence was a would-be permanent leak on the old
/// flag). Printed in the OOM-FORENSIC block; non-zero on a repro run is
/// the verification handle that the fix fires where the old kernel
/// wedged.
static RECLAIM_GATE_EXIT_RELEASES: AtomicUsize = AtomicUsize::new(0);

/// Leg-52 item 0: called from do_exit with the dying task's pid. Clears
/// the direct-reclaim gate IFF this task is still the recorded holder —
/// a gate already released (or held by another task) is untouched, so
/// the hook is a no-op on every normal exit (one failed CAS).
pub fn reclaim_gate_exit_release(pid: u32) {
    let holder = if pid == 0 { usize::MAX } else { pid as usize };
    if IN_DIRECT_RECLAIM
        .compare_exchange(holder, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let n = RECLAIM_GATE_EXIT_RELEASES.fetch_add(1, Ordering::Relaxed) + 1;
        // Bounded by actual mid-block deaths (rare by construction), so
        // each one is worth a serial line on the repro runs.
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_info!(
            "RECLAIM-GATE: pid {} died holding IN_DIRECT_RECLAIM; cleared ({} releases total)",
            pid,
            n
        );
        GATE_PHASE.store(GATE_PHASE_FREE, Ordering::Relaxed);
    }
}

// ==================== SP-5 gate census =============//
// stress-port round 3 (memory-fix1): the gate can be held for minutes
// while the zone refills around the holder (workers died and freed
// 122k pages; in_direct_reclaim stayed = holder pid across 4+ OOM
// dumps; vmscan freed 268->278 pages in ~40 s). The old evidence could
// not name WHERE the holder's time went. These stamps answer that:
// the holder's context notes its reclaim phase (one atomic store per
// phase transition), and the 1 Hz timer heartbeat prints a census line
// when the gate has been held >= GATE_STRAND_J — i.e. only on strands.
// Standard boots never hold the gate 2 s and print nothing; the
// always-on cost is one atomic load per heartbeat.
//
// Waiter side: tasks spinning in the bounded 64-attempt gate-wait loop
// are counted live; the ones that burned all 64 attempts WITHOUT the
// gate (their allocation then fails even though the zone may refill a
// moment later) are counted cumulatively, and a nested allocation made
// BY the holder itself (current == gate value — reclaim I/O paths
// re-entering alloc_pages while holding the gate) is counted as a
// self-fight: the holder CAS-looping against itself.
pub const GATE_PHASE_FREE: usize = 0;
pub const GATE_PHASE_ENTERED: usize = 1;
pub const GATE_PHASE_ROUND: usize = 2;
pub const GATE_PHASE_SHRINK: usize = 3;
pub const GATE_PHASE_ANON: usize = 4;
pub const GATE_PHASE_SWAPW: usize = 5;
pub const GATE_PHASE_DEEP: usize = 6;
pub const GATE_PHASE_RETRY: usize = 7;

/// Jiffies stamp of the successful gate CAS (reset per hold).
static GATE_TAKEN_J: AtomicU64 = AtomicU64::new(0);
/// Current holder phase (one of GATE_PHASE_*).
static GATE_PHASE: AtomicUsize = AtomicUsize::new(GATE_PHASE_FREE);
/// Jiffies stamp of the last phase transition.
static GATE_PHASE_J: AtomicU64 = AtomicU64::new(0);
/// try_to_free_pages rounds completed by the current holder.
static GATE_ROUNDS: AtomicU64 = AtomicU64::new(0);
/// Pages freed by the current holder's reclaim (rounds + deep drain).
static GATE_FREED: AtomicU64 = AtomicU64::new(0);
/// Deep-drain passes executed by the current holder.
static GATE_DEEP: AtomicU64 = AtomicU64::new(0);
/// Tasks currently inside the 64-attempt gate-wait loop.
static GATE_WAITERS: AtomicUsize = AtomicUsize::new(0);
/// Cumulative: waiters that exhausted all 64 attempts without the gate.
static GATE_EXHAUSTED: AtomicU64 = AtomicU64::new(0);
/// Cumulative: gate-wait loop iterations where the spinning task was
/// ITSELF the holder (nested alloc self-fight).
static GATE_SELF: AtomicU64 = AtomicU64::new(0);
/// Last exhaustion witness (pid + jiffies).
static GATE_LAST_EXH_PID: AtomicU64 = AtomicU64::new(0);
static GATE_LAST_EXH_J: AtomicU64 = AtomicU64::new(0);
/// Cumulative successful gate acquisitions (SP-5 round 4: distinguishes
/// one long hold from release+reacquire churn — the round-4 sweep boot 1
/// showed the SP-5 face at the OOM dumps with ZERO 1 Hz census lines,
/// which is either heartbeat starvation or short-hold churn).
static GATE_ACQS: AtomicU64 = AtomicU64::new(0);
/// Cumulative holds whose age at release crossed the strand bar (2 s).
static GATE_LONGHOLDS: AtomicU64 = AtomicU64::new(0);
/// Cumulative pages freed by ANY reclaim (kswapd rounds + direct-reclaim
/// holder rounds + the deep pre-OOM drain) — the SP-5 waiter-side
/// progress witness. The reference kernel's __alloc_pages_slowpath
/// retries the allocation while system-wide reclaim is making progress
/// (should_reclaim_retry consulting pgsteal deltas; no_progress only
/// accumulates across progress-free passes) — an allocation must not
/// FAIL just because a different task is mid-reclaim. Bumped from
/// balance_pgdat (covers both kswapd and the gate holder).
pub static RECLAIM_PROGRESS: AtomicU64 = AtomicU64::new(0);

/// Release the gate from the holder's allocation path. Every
/// alloc_pages_inner release site funnels through here so the SP-5
/// audit is uniform: a hold that aged past the strand bar prints one
/// GATE-LONGHOLD line AT RELEASE (process context — immune to timer
/// softirq starvation, unlike the 1 Hz census).
#[inline]
fn gate_release(holder: usize) {
    let now = crate::timer::get_jiffies();
    let age = now.saturating_sub(GATE_TAKEN_J.load(Ordering::Relaxed));
    if age >= 200 {
        GATE_LONGHOLDS.fetch_add(1, Ordering::Relaxed);
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_info!(
            "GATE-LONGHOLD holder={} age={}j phase={} phase_age={}j rounds={} freed={} deep={} acqs={} waiters={} exhausted={} selffight={}",
            holder,
            age,
            GATE_PHASE.load(Ordering::Relaxed),
            now.saturating_sub(GATE_PHASE_J.load(Ordering::Relaxed)),
            GATE_ROUNDS.load(Ordering::Relaxed),
            GATE_FREED.load(Ordering::Relaxed),
            GATE_DEEP.load(Ordering::Relaxed),
            GATE_ACQS.load(Ordering::Relaxed),
            GATE_WAITERS.load(Ordering::Relaxed),
            GATE_EXHAUSTED.load(Ordering::Relaxed),
            GATE_SELF.load(Ordering::Relaxed)
        );
    }
    IN_DIRECT_RECLAIM.store(0, Ordering::Release);
    GATE_PHASE.store(GATE_PHASE_FREE, Ordering::Relaxed);
}

#[inline]
fn gate_holder_is_me() -> bool {
    let holder = IN_DIRECT_RECLAIM.load(Ordering::Relaxed);
    if holder == 0 {
        return false;
    }
    let pid = crate::process::hotpath::current_pid();
    let me = if pid == 0 { usize::MAX } else { pid as usize };
    holder == me
}

/// Phase stamp — called from reclaim code paths; only the gate holder's
/// context writes (kswapd shares the vmscan entry points but never
/// holds the gate, and its stamps would otherwise overwrite the
/// holder's).
#[inline]
pub fn gate_note_phase(p: usize) {
    if gate_holder_is_me() {
        GATE_PHASE.store(p, Ordering::Relaxed);
        GATE_PHASE_J.store(crate::timer::get_jiffies(), Ordering::Relaxed);
    }
}

/// Round/progress stamp: a completed reclaim round that freed >0 pages.
/// Kept separate from gate_note_phase so the phase after a round
/// (RETRY, stamped by the caller) is not overwritten.
#[inline]
pub fn gate_note_round_done(freed: u64) {
    if gate_holder_is_me() {
        GATE_ROUNDS.fetch_add(1, Ordering::Relaxed);
        GATE_FREED.fetch_add(freed, Ordering::Relaxed);
    }
}

/// Deep-drain pass stamp: called once per drain_file_cache_before_oom
/// pass; also accumulates what the drain freed.
#[inline]
pub fn gate_note_deep_pass(freed: u64) {
    if gate_holder_is_me() {
        GATE_DEEP.fetch_add(1, Ordering::Relaxed);
        if freed > 0 {
            GATE_FREED.fetch_add(freed, Ordering::Relaxed);
        }
    }
}

/// SP-5 census: called from the 1 Hz timer heartbeat. Prints one line
/// when the gate has been held >= GATE_STRAND_J (2 s), naming the
/// holder, its phase + phase age, progress counters, the holder task's
/// scheduler state/CPU (cross-CPU safe read via the pid hash), and the
/// waiter-side counters. Rate-limited by construction (1 Hz call).
pub fn reclaim_gate_census() {
    let holder = IN_DIRECT_RECLAIM.load(Ordering::Relaxed);
    if holder == 0 || holder == usize::MAX {
        return;
    }
    let now = crate::timer::get_jiffies();
    let taken = GATE_TAKEN_J.load(Ordering::Relaxed);
    // Taken==0 can mean "never" only when the gate is also free; a
    // holder implies a successful CAS wrote a nonzero stamp (jiffies
    // starts at 0 only pre-timer, where no reclaim runs).
    let age = now.saturating_sub(taken);
    const GATE_STRAND_J: u64 = 200;
    if age < GATE_STRAND_J {
        return;
    }
    let phase = GATE_PHASE.load(Ordering::Relaxed);
    let phase_age = now.saturating_sub(GATE_PHASE_J.load(Ordering::Relaxed));
    // Best-effort holder task read (cross-CPU; the task may be exiting
    // concurrently — same discipline as the parkwalk census).
    let (tstate, tcpu) = {
        let t = crate::sched::hotpath::find_task_by_pid(holder as u32);
        if t.is_null() {
            (0xFFFFFFFFu32, 0xFFFFu32)
        } else {
            // INV-PROC-BORROW-1 census arm (racy best-effort read).
            let t = crate::process::r#unsafe::borrow_task(t);
            (t.state().bits(), t.ti_cpu() as u32)
        }
    };
    let zone_free = super::r#unsafe::first_node()
        .and_then(|n| n.zone(super::types::ZoneType::ZoneNormal))
        .map(|z| z.nr_free())
        .unwrap_or(0);
    // falloc-wedge census addendum: the holder's last tick-interrupted RIP
    // (TICK_ENTRY_RIP is per-CPU, sampled every tick) — names where a
    // stranded holder is actually spinning, sampled once per second of
    // strand instead of guessing from the phase enum.
    let holder_rip = if tcpu != 0xFFFF {
        crate::interrupt::preempt::TICK_ENTRY_RIP[tcpu as usize].load(Ordering::Relaxed)
    } else {
        0
    };
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::pr_info!(
        "GATE-CENSUS holder={} age={}j phase={} phase_age={}j rounds={} freed={} deep={} task_state={:#x} tcpu={} zone_free={} waiters={} exhausted={} selffight={} last_exh_pid={} last_exh_age={}j holder_rip={:#x}",
        holder,
        age,
        phase,
        phase_age,
        GATE_ROUNDS.load(Ordering::Relaxed),
        GATE_FREED.load(Ordering::Relaxed),
        GATE_DEEP.load(Ordering::Relaxed),
        tstate,
        tcpu,
        zone_free,
        GATE_WAITERS.load(Ordering::Relaxed),
        GATE_EXHAUSTED.load(Ordering::Relaxed),
        GATE_SELF.load(Ordering::Relaxed),
        GATE_LAST_EXH_PID.load(Ordering::Relaxed),
        now.saturating_sub(GATE_LAST_EXH_J.load(Ordering::Relaxed)),
        holder_rip,
    );
}

/// Translate Rux's private GfpFlags namespace onto the reference gfp
/// mask values (the kmem:mm_page_alloc gfp_flags column carries the
/// reference mask — its print-fmt flag table names those values, not
/// Rux's internal bits).
fn gfp_reference_mask(g: GfpFlags) -> u64 {
    let mut m = 0u64;
    if g.0 & GfpFlags::GFP_KERNEL.0 != 0 {
        m |= 0xcc0;
    }
    if g.0 & GfpFlags::GFP_USER.0 != 0 {
        m |= 0x100cc0;
    }
    if g.0 & GfpFlags::GFP_ATOMIC.0 != 0 {
        m |= 0x820;
    }
    if g.0 & GfpFlags::GFP_DMA.0 != 0 {
        m |= 0x1;
    }
    if g.0 & GfpFlags::GFP_DMA32.0 != 0 {
        m |= 0x4;
    }
    if g.0 & GfpFlags::__GFP_ZERO.0 != 0 {
        m |= 0x100;
    }
    if g.0 & GfpFlags::__GFP_HIGHMEM.0 != 0 {
        m |= 0x2;
    }
    if g.0 & GfpFlags::__GFP_MOVABLE.0 != 0 {
        m |= 0x8;
    }
    m
}

/// The reference's gfp_migratetype(): __GFP_MOVABLE (Rux carries no
/// __GFP_RECLAIMABLE) selects MIGRATE_MOVABLE, everything else is
/// MIGRATE_UNMOVABLE.
fn gfp_migratetype(g: GfpFlags) -> i32 {
    if g.0 & GfpFlags::__GFP_MOVABLE.0 != 0 {
        2
    } else {
        0
    }
}

/// Allocate 2^order contiguous physical pages
///
/// DEBUG memwatch wrapper: captures the caller chain (frame-pointer walk,
/// valid at -O0) and feeds the page-level site table in debug::memwatch —
/// net growth per call-site is the physical-page leak signature.
pub fn alloc_pages(gfp_flags: GfpFlags, order: usize) -> usize {
    let phys = if !crate::debug::memwatch::armed() {
        alloc_pages_inner(gfp_flags, order)
    } else {
        let mw_s0: u64 = crate::debug::backtrace::current_frame_pointer();
        let phys = alloc_pages_inner(gfp_flags, order);
        if phys != 0 {
            let mut mw_frames: [u64; crate::debug::memwatch::SITE_FRAMES] =
                [0; crate::debug::memwatch::SITE_FRAMES];
            super::r#unsafe::walk_fp_chain(mw_s0, &mut mw_frames);
            crate::debug::memwatch::note_page_alloc(phys, order, &mw_frames);
        }
        phys
    };
    // Perf ring premature-free detector (hp88a hunt, TASK 87.11): one
    // relaxed load while no perf ring exists; while one does, verify
    // handed-out frames against the live ring set — a hit means a ring
    // frame was returned to the allocator while its event still lists
    // it (the sampler would write into the new owner).
    if phys != 0 && crate::perf::ring_frames_live() {
        crate::perf::ring_frame_check(phys, order);
    }
    // kmem:mm_page_alloc — the reference's __alloc_pages return record
    // (fired once per allocation, pfn -1 on failure exactly like the
    // reference's page==NULL path).
    if crate::trace::EVENT_MM_PAGE_ALLOC.armed() {
        crate::trace::trace_mm_page_alloc(
            if phys != 0 {
                phys_to_pfn(phys) as u64
            } else {
                u64::MAX
            },
            order as u32,
            gfp_reference_mask(gfp_flags),
            gfp_migratetype(gfp_flags),
        );
    }
    phys
}

/// Allocate 2^order contiguous physical pages
///
/// # Arguments
/// - `gfp_flags`: GFP flags controlling allocation behavior
/// - `order`: Order of allocation (2^order pages)
///
/// # Returns
/// - Physical address of the first page, or 0 if allocation fails
fn alloc_pages_inner(gfp_flags: GfpFlags, order: usize) -> usize {
    if order > MAX_ORDER {
        return 0;
    }

    // Try to allocate from the Zone system first
    // SAFETY: early boot or under allocator lock — exclusive node access.
    if let Some(node) = super::r#unsafe::first_node_mut() {
        let zone_type = gfp_flags.zone_type();
        if let Some(zone) = node.zone_mut(zone_type) {
            if zone.is_initialized() {
                // Per-CPU pageset fast path (perf review 2026-10-31):
                // order-0 allocations — the page-fault / COW / page-cache
                // bulk class — are served from this CPU's pcp row with no
                // zone-lock acquisition; the lock is paid once per
                // PCP_BATCH refill. ZoneNormal only: the pcp rows cache
                // frames from that zone, and GFP-forced alternate zones
                // keep their exact historical semantics.
                if order == 0 && zone_type == ZoneType::ZoneNormal {
                    if let Some(pfn) = super::hotpath::alloc_order0(zone) {
                        ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                        return pfn_to_phys(pfn);
                    }
                    // pcp miss with an exhausted row falls through to
                    // zone.alloc_pages below (which refills the row next
                    // time); a zone failure reaches the slow path as before.
                }
                if let Some(pfn) = zone.alloc_pages(order) {
                    ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                    // Page descriptors (refcount/mapcount/flags/order) were
                    // initialized by zone.alloc_pages INSIDE the zone lock —
                    // doing it here used to leave a lock-free window where
                    // the pages still looked free (review MM-H2).
                    return pfn_to_phys(pfn);
                }
                // Zone allocator failed — wake kswapd if below low watermark
                if !zone.watermark_ok(order, WMARK_LOW) {
                    crate::api::mm::wakeup_kswapd(order as i32);
                }
                // Leg-15 pace: one tick per slowpath entry (zone fastpath
                // failed), rux:schb (retired debug=schb)-gated counter only.
                crate::debug::schb::note_slow();

                // Synchronous direct reclaim (Linux __alloc_pages_slowpath):
                // waking kswapd alone loses the race — the faulting task
                // returns OOM before the daemon has reclaimed anything.
                // GFP_KERNEL-style callers reclaim pages themselves (clean
                // page cache first, then anonymous pages to swap) and retry.
                // Skipped for GFP_ATOMIC (IRQ contexts must not block on I/O)
                // and while already inside reclaim (PF_MEMALLOC equivalent).
                if gfp_flags.0 & GfpFlags::GFP_ATOMIC.0 == 0 {
                    // GATE CONTENTION (the at-cap crawl): the gate used to be
                    // take-it-or-skip — when another task was mid-reclaim,
                    // THIS allocation skipped reclaim entirely and could
                    // return 0 while the reclaimer was freeing a 32-page
                    // batch (page-cache inserts bailed mid-batch, faults
                    // OOM-killed loaders; faultlat3 at-cap: 10-20 s single-
                    // page stalls). Wait for the gate instead: retry the
                    // fastpath (the reclaimer's freed pages may already
                    // cover us) and yield the CPU between attempts so the
                    // reclaimer and kswapd actually run. Bounded — a stuck
                    // holder falls through to the old skip path.
                    let mut gate_attempts = 0u32;
                    let mut held_gate = false;
                    // Item 0: stamp the holder's pid into the gate (0 is
                    // "free", so a pre-sched context claims usize::MAX) —
                    // do_exit clears a gate its dying task still holds.
                    let holder = {
                        let pid = crate::process::hotpath::current_pid();
                        if pid == 0 { usize::MAX } else { pid as usize }
                    };
                    // PF_MEMALLOC parity (reference): an allocation made
                    // BY the current gate holder (reclaim's I/O paths
                    // re-entering alloc_pages mid-round) gets
                    // ALLOC_NO_WATERMARKS-style access instead of
                    // queueing — it must never wait on its own gate.
                    // Serve it from the zone fastpath; on failure fall
                    // through WITHOUT the wait loop (the holder's own
                    // round continues; a retry after release may serve
                    // it). Counted as the self-fight face by the census.
                    let nested_holder_alloc = holder != usize::MAX
                        && IN_DIRECT_RECLAIM.load(Ordering::Relaxed) == holder;
                    if nested_holder_alloc {
                        GATE_SELF.fetch_add(1, Ordering::Relaxed);
                        if let Some(pfn) = zone.alloc_pages(order) {
                            ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                            return pfn_to_phys(pfn);
                        }
                    } else {
                    GATE_WAITERS.fetch_add(1, Ordering::Relaxed);
                    // SP-5 fix (reference parity): track system-wide
                    // reclaim progress while waiting. The reference's
                    // allocation stalls RETRY as long as reclaim is
                    // freeing pages anywhere (should_reclaim_retry +
                    // pgsteal deltas); only progress-free passes
                    // accumulate toward giving up. A fixed 64-attempt
                    // cap converted a holder's slow-but-working swap
                    // reclaim into allocation FAILURE for every waiter
                    // — the fake-OOM kill chain (sweep boots 1-3:
                    // exhausted=4..8 vs acqs=2, holder crawling in
                    // SWAPW while the zone refilled around it).
                    let mut prog_last = RECLAIM_PROGRESS.load(Ordering::Relaxed);
                    let mut fatal_bail = false;
                    while gate_attempts < 64 {
                        // Reference fatal_signal_pending discipline: a
                        // dying allocator stops waiting (its fault
                        // owner is the kill path, not this loop).
                        if crate::sched::hotpath::current_is_dying() {
                            fatal_bail = true;
                            break;
                        }
                        if IN_DIRECT_RECLAIM
                            .compare_exchange(0, holder, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            held_gate = true;
                            break;
                        }
                        // SP-5: an allocation made BY the current holder
                        // (reclaim's own I/O paths re-entering alloc_pages)
                        // fights its own gate here — count it; the census
                        // quantifies the self-fight face.
                        if IN_DIRECT_RECLAIM.load(Ordering::Relaxed) == holder
                            && holder != usize::MAX
                        {
                            GATE_SELF.fetch_add(1, Ordering::Relaxed);
                        }
                        if let Some(pfn) = zone.alloc_pages(order) {
                            ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                            GATE_WAITERS.fetch_sub(1, Ordering::Relaxed);
                            return pfn_to_phys(pfn);
                        }
                        // Progress reset: pages were freed by the holder
                        // or kswapd since the last check — keep waiting
                        // (the reference retries the allocation on
                        // exactly this evidence).
                        let prog = RECLAIM_PROGRESS.load(Ordering::Relaxed);
                        if prog != prog_last {
                            prog_last = prog;
                            gate_attempts = 0;
                        }
                        crate::sched::hotpath::yield_cpu();
                        gate_attempts += 1;
                    }
                    GATE_WAITERS.fetch_sub(1, Ordering::Relaxed);
                    if !held_gate && gate_attempts >= 64 && !fatal_bail {
                        // SP-5: this allocation is about to FAIL because it
                        // never got the gate — the zone may refill moments
                        // later (the round-3 #3/#4 forensics: alloc failed
                        // while the holder's victims were being torn down).
                        GATE_EXHAUSTED.fetch_add(1, Ordering::Relaxed);
                        if holder != usize::MAX {
                            GATE_LAST_EXH_PID.store(holder as u64, Ordering::Relaxed);
                            GATE_LAST_EXH_J
                                .store(crate::timer::get_jiffies(), Ordering::Relaxed);
                        }
                    }
                    } // !nested_holder_alloc
                    if held_gate {
                        // SP-5 census: reset the per-hold progress stamps,
                        // then follow the holder's phases.
                        GATE_ACQS.fetch_add(1, Ordering::Relaxed);
                        GATE_TAKEN_J.store(
                            crate::timer::get_jiffies(),
                            Ordering::Relaxed,
                        );
                        GATE_ROUNDS.store(0, Ordering::Relaxed);
                        GATE_FREED.store(0, Ordering::Relaxed);
                        GATE_DEEP.store(0, Ordering::Relaxed);
                        gate_note_phase(GATE_PHASE_ENTERED);
                        for _ in 0..8 {
                            // Reference fatal_signal_pending: a dying
                            // holder stops reclaiming — its pages return
                            // at exit and the gate must not be carried
                            // through swap I/O it will never use.
                            if crate::sched::hotpath::current_is_dying() {
                                break;
                            }
                            let freed = crate::api::mm::try_to_free_pages(order as i32);
                            gate_note_phase(GATE_PHASE_RETRY);
                            if let Some(pfn) = zone.alloc_pages(order) {
                                ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                                gate_release(holder);
                                return pfn_to_phys(pfn);
                            }
                            if freed == 0 {
                                break; // no progress — do not spin
                            }
                        }
                        // DEEP FINAL ROUND (stress-fleet s17, leg 49B;
                        // design in perf-report.md 8.10.19): the 8
                        // steady-state rounds above drained at most
                        // ~8x32 = 256 pages — measured against kills
                        // that fired while 175-204 MB of clean file
                        // cache sat on the LRUs, two orders of magnitude
                        // short (the bounds are steady-state pace
                        // measures; nothing deliberately limits the
                        // pre-OOM effort). Before giving up: ONE deep
                        // drain sized to the actual shortfall, with the
                        // bulk active-list demotion that makes the
                        // never-scanned active pool reachable. Still
                        // inside the IN_DIRECT_RECLAIM gate (the drain
                        // uses shrink_reclaim, the direct-reclaim
                        // variant with dirty writeback). Standard boots
                        // never reach here (the rounds above satisfy
                        // them); the drain's counters read zero on the
                        // Tier-1 census.
                        let wmark_low = zone.watermark(super::types::WMARK_LOW);
                        let shortfall = wmark_low
                            .saturating_add(1usize << order)
                            .saturating_sub(zone.nr_free());
                        let deep_target = shortfall.clamp(32, 8192);
                        // fatal_signal_pending parity (see the rounds
                        // loop): a dying holder skips the deep drain too.
                        if crate::sched::hotpath::current_is_dying() {
                            gate_release(holder);
                        } else {
                        gate_note_phase(GATE_PHASE_DEEP);
                        let deep_freed =
                            crate::api::mm::drain_file_cache_before_oom(deep_target);
                        gate_note_phase(GATE_PHASE_RETRY);
                        if deep_freed > 0 {
                            if let Some(pfn) = zone.alloc_pages(order) {
                                ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                                gate_release(holder);
                                return pfn_to_phys(pfn);
                            }
                        }
                        gate_release(holder);
                        }
                    }
                }

                // High-order allocation failed: try compaction to reduce fragmentation
                if order > 0 {
                    // Convert to raw pointer before passing to compact_zone.
                    // Use raw pointer for the subsequent alloc_pages call too,
                    // avoiding the aliasing UB of re-creating &mut from a raw
                    // pointer while the original &mut binding is still live.
                    let zone_ptr: *mut Zone = zone;
                    // zone_ptr is a valid pointer to an initialized zone
                    // (compact_zone is now a safe fn over the same pointer).
                    let cr = crate::api::mm::compact_zone(zone_ptr, order);
                    if matches!(cr, crate::api::mm::CompactResult::Success) {
                        // compact_zone does not destroy the zone; the
                        // pointer remains valid for the lifetime of the
                        // node (INV-SYSC-2 reachability arm).
                        let pfn =
                            crate::syscall::r#unsafe::boxed_mut(zone_ptr).alloc_pages(order);
                        if let Some(pfn) = pfn {
                            ZONE_ALLOCS.fetch_add(1, Ordering::Relaxed);
                            // Descriptors initialized under the zone lock.
                            return pfn_to_phys(pfn);
                        }
                    }
                }

                // FORENSIC (fake-OOM family): the allocator is about to fail
                // this allocation while the zone may still CLAIM free pages.
                // Dump the state once so the serial log shows whether the
                // free lists are genuinely empty or just unreachable.
                if order == 0 && zone.nr_free() > 0 {
                    oom_forensic_dump("alloc_pages(order=0) failed with free_cnt>0");
                }

                return 0;
            }
        }
    }

    // Zone system not initialized yet, use memblock
    // This should only happen during early boot before zone is set up
    LEGACY_ALLOCS.fetch_add(1, Ordering::Relaxed);
    // Flag any post-zone-init memblock page handout: memblock's bump
    // cursor overlaps the zone's seeded free lists, so a handout here
    // past zone init is a double-allocation in the making.
    if super::hotpath::memblock_available_memory() > 0 {
        static LEG_WARN: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if LEG_WARN.fetch_add(1, Ordering::Relaxed) < 5 {
            // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
            crate::pr_err!("page_alloc: legacy memblock alloc order={} after zone init", order);
        }
    }
    super::hotpath::memblock_phys_alloc().unwrap_or(0)
}

/// Allocate a single page
pub fn alloc_page(gfp_flags: GfpFlags) -> usize {
    alloc_pages(gfp_flags, 0)
}

/// FORENSIC (fake-OOM family): one-shot allocator-state dump, printed when
/// the page allocator fails a demand-fault allocation (the kernel then kills
/// the faulting task with SIGKILL — "pagefault: Out of memory").
///
/// Distinguishes the failure classes that are indistinguishable from the
/// OOM message alone:
///  - genuine exhaustion: free counters AND walkable lists both ~0
///  - broken free lists: counters claim free pages, chain walk finds none
///    (a linked page's next_free was clobbered — everything past the break
///    is unreachable while still refcount==0)
///  - orphaned frames: descriptors say refcount==0 but the page is not on
///    any free list (freed through a path that never linked it, or unlinked
///    without relinking)
///  - inverse corruption: page is ON a free list but carries refcount>0
///    (a live frame wired into the buddy — will be handed out twice)
///
/// Rate-limited to the first 4 calls per boot (alloc failure storms would
/// otherwise flood the UART).
pub fn oom_forensic_dump(site: &str) {
    static DUMPS: AtomicUsize = AtomicUsize::new(0);
    if DUMPS.fetch_add(1, Ordering::Relaxed) >= 4 {
        return;
    }

    // Full accounting ledger first (debug/ledger.rs): where every managed
    // page went at this moment — the census + counter reconciliation that
    // attributes the "out of memory" to a category (or to drift).
    super::super::debug::ledger::dump(&alloc::format!("oom: {}", site));

    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!("==== OOM-FORENSIC #{} @ {} ====", DUMPS.load(Ordering::Relaxed), site);
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: in_direct_reclaim={} (holder pid; 0=free) gate_exit_releases={}",
        IN_DIRECT_RECLAIM.load(Ordering::Relaxed),
        RECLAIM_GATE_EXIT_RELEASES.load(Ordering::Relaxed)
    );
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: gate phase={} rounds={} freed={} deep={} waiters={} exhausted={} selffight={} taken_age={}j acqs={} longholds={} now_j={} taken_j={} prog={}",
        GATE_PHASE.load(Ordering::Relaxed),
        GATE_ROUNDS.load(Ordering::Relaxed),
        GATE_FREED.load(Ordering::Relaxed),
        GATE_DEEP.load(Ordering::Relaxed),
        GATE_WAITERS.load(Ordering::Relaxed),
        GATE_EXHAUSTED.load(Ordering::Relaxed),
        GATE_SELF.load(Ordering::Relaxed),
        crate::timer::get_jiffies()
            .saturating_sub(GATE_TAKEN_J.load(Ordering::Relaxed)),
        GATE_ACQS.load(Ordering::Relaxed),
        GATE_LONGHOLDS.load(Ordering::Relaxed),
        crate::timer::get_jiffies(),
        GATE_TAKEN_J.load(Ordering::Relaxed),
        RECLAIM_PROGRESS.load(Ordering::Relaxed)
    );
    // SP-5 clock discrimination: raw timebase + tick IRQ counts —
    // separates "read_time broke backward" from "timer IRQs stopped"
    // (the two ways now_j can lag or freeze).
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: clock tb={} irq0={} irq1={}",
        crate::timer::read_time(),
        crate::timer::WW_TICK_IRQ[0].load(core::sync::atomic::Ordering::Relaxed),
        crate::timer::WW_TICK_IRQ[1].load(core::sync::atomic::Ordering::Relaxed),
    );

    // SP-5 round 5: frozen-tick four-way discrimination (LAPIC LVT/TMICT/
    // TMCCT/IRR/ISR live snapshot, self + peer-via-IPI, arm trail). The
    // dump context still executes during the cascade by construction.
    crate::timer::tick_trap_report("oom");

    // SP-5 round 4: the reclaim round's per-page cost driver.
    {
        let (n, slow, max_ns) = crate::api::mm::swap_write_census();
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::println!(
            "OOM-FORENSIC: swapwrite n={} slow={} max_ms={}",
            n,
            slow,
            max_ns / 1_000_000
        );
    }
    // SP-7: per-page skip reasons — why anon candidates never reach the
    // swap write above.
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!("OOM-FORENSIC: {}", crate::api::mm::vscan_census_line());
    // falloc-wedge census: the page-cache reclaim/evict skip faces (same
    // SP-7 shape — names the filter that holds the file LRU hostage).
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: {}",
        crate::fs::page_cache::pcache_reclaim_census_line()
    );
    // Zone state: counters vs walkable chains
    // SAFETY: diagnostic read-only node access.
    if let Some(node) = super::r#unsafe::first_node_mut() {
        for zt in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma, ZoneType::ZoneMovable] {
            if let Some(zone) = node.zone(zt) {
                if !zone.is_initialized() {
                    continue;
                }
                let report = zone.forensic_free_report();
                // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                crate::println!(
                    "OOM-FORENSIC: zone {} free_cnt={} managed={} wmark min/low/high={}/{}/{}",
                    zone.zone_type().name(),
                    zone.nr_free(),
                    zone.managed_pages(),
                    zone.watermark(super::types::WMARK_MIN),
                    zone.watermark(super::types::WMARK_LOW),
                    zone.watermark(super::types::WMARK_HIGH),
                );
                let mut walk_total = 0usize;
                for order in 0..=MAX_ORDER {
                    let (declared, walked, head) = report[order];
                    walk_total += walked << order;
                    if declared != 0 || walked != 0 {
                        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                        crate::println!(
                            "OOM-FORENSIC:   order {:>2}: declared={} walked={} head_pfn={}",
                            order, declared, walked, head
                        );
                    }
                }
                // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                crate::println!(
                    "OOM-FORENSIC:   walkable pages total={} vs free_cnt={}{}",
                    walk_total,
                    zone.nr_free(),
                    if walk_total == zone.nr_free() { "" } else { "  <== MISMATCH" }
                );
            }
        }
        // LRU sizes
        let mut l0 = 0usize;
        for lru_i in 0..super::types::NR_LRU_LISTS {
            let sz = node.lru_sizes[lru_i].load(Ordering::Relaxed);
            if sz > 0 {
                // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                crate::println!("OOM-FORENSIC: lru[{}] size={}", lru_i, sz);
            }
            l0 += sz;
        }
        if l0 == 0 {
            // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
            crate::println!("OOM-FORENSIC: all LRU lists empty");
        }
        // session-9 (r51/r58 follow-up): clean/dirty breakdown of the
        // FILE LRUs. ~200MB of UNMAPPED file cache sits on these lists
        // at the OOM point (free_cnt=13, mapped ~= anon) while direct
        // reclaim OOMs — the split decides between "reclaim never
        // reached them" (mostly CLEAN here -> scan/index drift or
        // pinned-by-refcount) and "dirty and unwriteable" (mostly
        // DIRTY/WRITEBACK here -> writeback starved under pressure).
        // Best-effort walk WITHOUT lru_lock (OOM forensic context;
        // single-page races acceptable — counters, not invariants).
        // Bound: 300k pages (~1.2G, more than any list can hold).
        for lru_i in [super::types::LRU_INACTIVE_FILE, super::types::LRU_ACTIVE_FILE] {
            let mut pfn = node.lru_heads[lru_i].load(Ordering::Relaxed);
            let (mut n, mut dirty, mut wb, mut refd, mut unev, mut locked) =
                (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
            while pfn != 0 && n < 300_000 {
                // SAFETY: forensic read-only flag access via the
                // descriptor array; pfn came off an LRU link chain.
                // INV-MM-2 chain arm.
                let p = match super::r#unsafe::page_ref_of(pfn) {
                    Some(p) => p,
                    None => break,
                };
                if p.test_flag(super::types::PageFlag::Dirty) {
                    dirty += 1;
                }
                if p.test_flag(super::types::PageFlag::Writeback) {
                    wb += 1;
                }
                if p.test_flag(super::types::PageFlag::Referenced) {
                    refd += 1;
                }
                if p.test_flag(super::types::PageFlag::Unevictable) {
                    unev += 1;
                }
                if p.is_locked() {
                    locked += 1;
                }
                n += 1;
                let next = p.lru_next();
                if next == pfn {
                    break; // self-loop guard (torn concurrent unlink)
                }
                pfn = next;
            }
            // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
            crate::println!(
                "OOM-FORENSIC: file-lru[{}] walked={} dirty={} writeback={} referenced={} unevictable={} locked={}",
                lru_i, n, dirty, wb, refd, unev, locked
            );
        }
    }

    // Page-descriptor census
    let st = super::hotpath::page_desc_stats();
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: desc used={} free={} reserved={} anon={} mapped={}",
        st.used_pages, st.free_pages, st.reserved_pages, st.anonymous_pages, st.mapped_pages
    );

    // Swap
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: swap active={}",
        crate::api::mm::nr_active_swap()
    );

    // Seed-detector counters (see zone.rs)
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "OOM-FORENSIC: seed_alarms={} double_free_tripwires={}",
        super::hotpath::SEED_ALARMS.load(Ordering::Relaxed),
        super::hotpath::DOUBLE_FREE_COUNT.load(Ordering::Relaxed)
    );

    // Orphan / inverse-corruption scan over the whole mem_map (bounded by
    // the descriptors vmemmap actually mapped — see page_desc_stats for the
    // MAX_PAGES-vs-RAM clamp rationale; ~0.5M iterations, acceptable on the
    // diagnostic path).
    {
        use super::{hotpath::pfn_to_page, types::PageFlag, hotpath::MIN_PFN, hotpath::MAX_PAGES};
        let nr_descs =
            MAX_PAGES.min(super::hotpath::vmemmap_stats().nr_pages);
        let mut orphan = 0usize;
        let mut inverse = 0usize;
        let mut orphan_first: [usize; 8] = [0; 8];
        let mut inverse_first: [usize; 8] = [0; 8];
        for i in 0..nr_descs {
            // Read-only descriptor access on the diagnostic path
            // (INV-MM-2 chain arm).
            let (rc, linked, reserved) = match super::r#unsafe::page_ref_of(MIN_PFN + i) {
                Some(p) => (p.refcount(), p.test_flag(PageFlag::OnFreelist), p.is_reserved()),
                None => continue,
            };
            if rc == 0 && !linked && !reserved {
                orphan += 1;
                if orphan <= 8 {
                    orphan_first[orphan - 1] = MIN_PFN + i;
                }
            } else if rc > 0 && linked {
                inverse += 1;
                if inverse <= 8 {
                    inverse_first[inverse - 1] = MIN_PFN + i;
                }
            }
        }
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::println!(
            "OOM-FORENSIC: orphan(unlinked,ref0)={} first={:?}",
            orphan, orphan_first
        );
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::println!(
            "OOM-FORENSIC: inverse(linked,ref>0)={} first={:?}",
            inverse, inverse_first
        );

        // Raw-UART copy of the decisive numbers. printk may be wedged
        // during the very storms this dump targets (the pt1 audit lost
        // every println line of this dump while the raw MEMLEDGER-COMPACT
        // line survived): zone counter-vs-walk mismatch for ZoneNormal
        // plus the corruption indices, one raw line.
        let mut zf = 0usize;
        let mut zm = 0usize;
        let mut zw = 0usize;
        // SAFETY: diagnostic read-only node access.
        if let Some(node) = super::hotpath::first_online_node() {
            if let Some(zone) = node.zone(super::types::ZoneType::ZoneNormal) {
                if zone.is_initialized() {
                    zf = zone.nr_free();
                    zm = zone.managed_pages();
                    let report = zone.forensic_free_report();
                    for order in 0..=super::types::MAX_ORDER {
                        zw += report[order].1 << order;
                    }
                }
            }
        }
        crate::debug::ledger::oomfx_line(
            zf as u64,
            zw as u64,
            zm as u64,
            orphan as u64,
            inverse as u64,
            super::hotpath::SEED_ALARMS.load(Ordering::Relaxed) as u64,
            super::hotpath::DOUBLE_FREE_COUNT.load(Ordering::Relaxed) as u64,
        );
    }
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!("==== OOM-FORENSIC end ====");
}

/// Allocate up to `n` single (order-0) pages under ONE zone lock.
///
/// Batched companion to `alloc_page` for the page-cache bulk-insert path:
/// one lock acquisition for n pages instead of n. PHYSICAL addresses are
/// written to `out`; the return value is how many were actually allocated
/// (callers fall back to `alloc_pages` — with its reclaim slowpath — for
/// any remainder).
pub fn alloc_page_batch(gfp_flags: GfpFlags, n: usize, out: &mut [usize]) -> usize {
    if n == 0 || out.is_empty() {
        return 0;
    }
    // SAFETY: exclusive node access — same discipline as alloc_pages_inner.
    if let Some(node) = super::r#unsafe::first_node_mut() {
        let zone_type = gfp_flags.zone_type();
        if let Some(zone) = node.zone_mut(zone_type) {
            if zone.is_initialized() {
                let want = core::cmp::min(n, out.len());
                // BUFW-PARITY: per-CPU row first (the reference's
                // per-cpu-pageset handout shape) — frames a recent free
                // cached on this CPU are handed out with no zone round
                // trip at all; only the remainder pays the (bulk) zone
                // path. ZoneNormal only, matching the row's discipline.
                let mut got = 0usize;
                // (alloc_page_batch is order-0-only by definition.)
                if zone_type == ZoneType::ZoneNormal {
                    got = super::hotpath::alloc_order0_batch(zone, want, out);
                }
                if got < want {
                    got += zone.alloc_pages_order0_batch(want - got, &mut out[got..]);
                }
                if got > 0 {
                    ZONE_ALLOCS.fetch_add(got, Ordering::Relaxed);
                }
                // pfn -> phys for API symmetry with alloc_page.
                for p in out.iter_mut().take(got) {
                    *p = pfn_to_phys(*p);
                }
                return got;
            }
        }
    }
    0
}

/// Allocate a page and zero it
pub fn get_zeroed_page(gfp_flags: GfpFlags) -> usize {
    let addr = alloc_page(gfp_flags);
    if addr != 0 {
        // SAFETY: addr is a valid physical address just allocated by alloc_page.
        // phys_to_virt converts it to the corresponding virtual address in the
        // kernel linear mapping region, which is safe to write to.
        // Writing PAGE_SIZE bytes is within the allocated page.
        let virt = crate::arch::mm::phys_to_virt(
            crate::arch::mm::PhysAddr::new(addr as u64),
        );
        // Fresh allocation, exclusively owned (INV-MM-7 page-slice arm).
        super::r#unsafe::page_slice_mut(virt.0 as usize).fill(0);
    }
    addr
}

/// Count of user leaf PTEs skipped by PTE walkers because the mapped
/// frame is foreign (boot-reserved). Readable from memwatch-style dumps;
/// nonzero after drm GEM activity is expected and healthy.
pub static FOREIGN_LEAF_SKIPS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// True when `phys` is a boot-reserved frame (kernel image, kernel heap,
/// slab, vmemmap, dtb). A USER leaf PTE mapping such a frame points at
/// FOREIGN memory: drm GEM dumb buffers and the virtio-gpu framebuffer
/// backing are kernel-heap allocations that get mmap(2)'d into userspace
/// (see drm::create_dumb + gem_mmap_resolve). No mapping-side refcount
/// was ever taken and the page_desc belongs to whichever subsystem owns
/// the frame, so every PTE walk (fork copy, unmap/zap, full teardown)
/// must SKIP these frames entirely: no get_page, no rmap, no put_page,
/// no free — the owner releases the frame through its own path
/// (GemObject::drop -> heap dealloc).
///
/// History: before this discriminator, fork's COW copy get_page()'d the
/// foreign GEM frames and the LAST teardown's put hit 0 and called
/// free_pages on kernel-heap phys — one loud pr_err PER FRAME from a
/// free context (sc83: a 7442-line REFUSED flood sweeping ~24 MB of
/// heap phys while the walk monopolized its locks; the printk storm
/// itself wedged the box). Generalizes the older framebuffer-range
/// special cases in both arches' walkers.
#[inline]
pub fn is_foreign_user_frame(phys: usize) -> bool {
    let foreign = super::hotpath::memblock().is_reserved(phys);
    if foreign {
        FOREIGN_LEAF_SKIPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    foreign
}

/// Rate-limited evidence printer for free_pages refusals/drops: the
/// first `DETAIL_BUDGET` events print in full, afterwards only one line
/// per `SUMMARY_EVERY` events. A single corrupted PTE tree can sweep
/// thousands of frames through here — per-frame printk from a free
/// context (console locks + UART at 115200) is itself a wedge vector
/// (sc83), so the flood is capped while the evidence survives.
fn report_free_refusal(detail_msg: &core::fmt::Arguments) {
    use core::sync::atomic::Ordering::Relaxed;
    const DETAIL_BUDGET: usize = 8;
    const SUMMARY_EVERY: usize = 4096;
    static COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let n = COUNT.fetch_add(1, Relaxed);
    if n < DETAIL_BUDGET {
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_err!("{}", detail_msg);
    } else if (n + 1) % SUMMARY_EVERY == 0 {
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_err!(
            "free_pages: refused/dropped {} frames so far (per-frame prints suppressed)",
            n + 1
        );
    }
}

/// mm-side release of a PCACHE (page-cache-backed) mapping leaf whose
/// descriptor put just returned 0 (pcachewedge lane, 2026-11-15).
///
/// A non-RO PCACHE leaf's frame ALWAYS belongs to the page cache while
/// the cache entry lives: the cache holds the frame's base descriptor
/// reference (the allocator's grant at insert, never dropped by the
/// cache — its release path frees through `release_page_frame` without
/// consulting the descriptor refcount), and every mapping adds its own
/// `get_page` at install (filemap_bookkeep / fork's copy walk). A
/// descriptor put reaching 0 here therefore means the mapping's own
/// reference was already consumed somewhere else — a PHANTOM put.
///
/// Freeing would hand a frame the page cache still owns (map entry +
/// LRU link) to the zone allocator; the frame is then re-allocated
/// (observed: as a page table, PT_LEDGER-stamped) while the cache still
/// lists it, and the cache's own later release (eviction, invalidate,
/// final put) frees the LIVE new owner: the RAW-FREE-OF-LIVE-TABLE
/// double-hand behind the fammm-window corruption cascade (PTE values
/// scribbled into recycled task memory, oops-killed lock holders, the
/// fdreapd starvation wedge). Reproduced and stack-attributed in the
/// pcachewedge fammm boot: evict_one_locked → release_page_frame freeing
/// pfn 0x7e41d while the ledger still stamped it live.
///
/// Discipline (same shape as the REUSE-WHILE-MAPPED tripwire in
/// free_pages): LEAK the frame — bounded, one frame per phantom put —
/// and name it. There is no legitimate mm-side zero-put free of a
/// cache-backed leaf: the cache's release paths never run through
/// put_page.
pub fn pcache_leaf_phantom_zero_put(phys: usize, site: &str) {
    use crate::debug::driftwatch;
    use core::sync::atomic::Ordering::Relaxed;
    driftwatch::PCACHE_LEAF_PHANTOM_PUT.fetch_add(1, Relaxed);
    static REPORTS: core::sync::atomic::AtomicUsize =
        core::sync::atomic::AtomicUsize::new(0);
    let n = REPORTS.fetch_add(1, Relaxed);
    if n < 8 {
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_err!(
            "pcache-leaf phantom zero-put: frame {:#x} still cache-owned (site {}) — leaking instead of freeing",
            phys,
            site
        );
        crate::debug::backtrace::dump_stack();
    } else if (n + 1) % 4096 == 0 {
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_err!(
            "pcache-leaf phantom zero-put: {} frames leaked so far (prints suppressed)",
            n + 1
        );
    }
    // mmforensics (early-put hunt): the PRODUCER-side capture — this is
    // the moment the mapping's descriptor reference is proven missing.
    // The ref-transition ring replay in the capture names the path that
    // consumed it early. Takes no locks (PTE-lock context is fine).
    crate::debug::mmforensics::capture_frame(phys >> 12, "pcache-phantom-put");
    // Intentionally NO free: the page cache owns the frame.
    // QUARANTINE (mmforensics lane): the mapping's reference was consumed
    // early — the frame is (or is about to be) double-tracked. Mark it so
    // no free path can ever hand it to a new owner while the surviving
    // references (cache entry, stale PTEs) still name it.
    crate::arch::mm::mmu_init::PtLedger::poison_table((phys >> 12) as u64);
    crate::debug::driftwatch::MM_POISONED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Free contiguous physical pages
///
/// # Arguments
/// - `addr`: Physical address of the first page
/// - `order`: Order of the allocation
pub fn free_pages(addr: usize, order: usize) {
    use crate::debug::driftwatch as dw;
    if addr == 0 {
        dw::FREE_NULL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return;
    }

    // kmem:mm_page_free — the free-side record at entry (the
    // reference's __free_pages_ok fires before the page meets the PCP
    // lists / the buddy; Rux's single free funnel carries every order —
    // see the event's divergence note).
    if crate::trace::EVENT_MM_PAGE_FREE.armed() {
        crate::trace::trace_mm_page_free(phys_to_pfn(addr) as u64, order as u32);
    }

    // REUSE-WHILE-MAPPED tripwire (b88 samgr heap corruption hunt): a
    // frame whose descriptor still says "mapped" must never re-enter the
    // zone — the surviving PTE would alias the next allocation and the
    // two owners scribble over each other (observed as mallocng meta
    // corruption and a zeroed binder write buffer in samgr, with NO
    // PTE-OVERWRITE at map time because the old leaf simply stayed).
    // Refuse the free (bounded leak) and name the frame; the mapping
    // keeps working, so the boot survives to reveal more.
    {
        let pd_ref = super::r#unsafe::page_ref_of(phys_to_pfn(addr));
        if pd_ref.is_some_and(|p| p.is_mapped()) {
            static MAPPED_FREES: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            dw::FREE_REFUSED_MAPPED
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if MAPPED_FREES.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                crate::pr_err!(
                    "free_pages: REFUSED frame {:#x} order {} still MAPPED (mapcount {}) pid {} — leaking instead of aliasing",
                    addr,
                    order,
                    pd_ref.map(|p| p.mapcount()).unwrap_or(0),
                    crate::process::hotpath::current_pid()
                );
                crate::debug::backtrace::dump_stack();
            }
            // mmforensics (early-put hunt): a free of a still-mapped
            // frame is the premature-free face of the same producer
            // family — capture names the frame and its referees.
            crate::debug::mmforensics::capture_frame(phys_to_pfn(addr), "free-while-mapped");
            return;
        }
    }

    // mmforensics (early-put hunt, 2026-11-15): an mm-side free of a
    // frame the page cache still owns. Every legitimate cache-side free
    // funnels through page_cache::release_page_frame, which clears the
    // PageCache type and the eviction keys (mapping/index) BEFORE the
    // free_page call — a frame arriving HERE still PageCache-typed with
    // its mapping key live is being freed by a foreign path (observed:
    // a teardown/unmap leaf put reaching 0 on a cache-owned frame — the
    // phantom-put's free face; first boot's capture #1 ring: PUT
    // old=1->0 with NO matching get since the frame's cache insert).
    // Under rux:mmforensics (retired debug=mmforensics): capture AT this moment (the rings still
    // hold the fresh evidence — the install sites, the transition
    // sequence) and REFUSE the free (quarantine-leak), so the frame
    // never re-enters circulation and scrambles the trail.
    if crate::trace::EVENT_MMFORENSICS.armed() {
        // Read-only descriptor access on the diagnostic path
        // (INV-MM-2).
        if let Some(p) = super::r#unsafe::page_ref_of(phys_to_pfn(addr)) {
            if p.page_type() == super::types::PageType::PageCache
                && !p.mapping().is_null()
            {
                static CACHE_FREES: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = CACHE_FREES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 8 {
                    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                    crate::pr_err!(
                        "free_pages: REFUSED frame {:#x} still CACHE-OWNED (type PageCache, mapping key live, refcount {}) pid {} — phantom-put free face",
                        addr,
                        p.refcount(),
                        crate::process::hotpath::current_pid()
                    );
                    crate::debug::backtrace::dump_stack();
                }
                crate::debug::mmforensics::capture_frame(
                    phys_to_pfn(addr),
                    "free-of-live-cache-entry",
                );
                crate::debug::driftwatch::MM_POISON_FREES
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    }

    // Sentinel: a kernel VMA (or any address outside the managed physical
    // range) entering the zone poisons the freelist — the next alloc
    // returns it as a wild phys. (x86_64 diagnostic from the feature
    // branch; kept as a cheap guard.)
    //
    // Fake-OOM fix: the bound used to be a hard-coded 4 GiB ceiling, but
    // qemu virt maps guest DRAM starting at 0x8000_0000, so on -m 4G HALF
    // the RAM (0x1_0000_0000..0x1_8000_0000) is above it — every free of
    // an above-4G page was dropped and the frame leaked (oomchurn anon
    // 2600 MB on 4G: exactly 524288 WILD lines = 2 GiB leaked on one
    // munmap; rerunning the workload then fails with a page allocator
    // "out of memory" while most of the RAM is only leaked, not used).
    // The managed-range check rejects genuinely wild addresses (kernel
    // VMAs are far outside it) while accepting every page the zone can
    // hand out — the two sets must agree by construction.
    if !super::hotpath::phys_valid(addr) {
        dw::FREE_DROPPED_WILD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // Caller diagnostic is x86-only (frame-pointer walk via att_syntax
        // asm); the riscv64 build has no equivalent and just drops it.
        // Rate-limited (see report_free_refusal): a wild sweep hits this
        // per frame.
        #[cfg(feature = "x86_64")]
        {
            let ret: u64 = super::r#unsafe::x86_caller_return_address();
            report_free_refusal(&format_args!(
                "free_pages: WILD addr={:#x} order={} caller={:#x}",
                addr, order, ret
            ));
        }
        #[cfg(not(feature = "x86_64"))]
        {
            report_free_refusal(&format_args!(
                "free_pages: WILD addr={:#x} order={}",
                addr, order
            ));
        }

        return;
    }

    // NEVER return a boot-reserved frame (kernel image, kernel heap, slab,
    // vmemmap, dtb) to the zone allocator. The kernel heap lives in a
    // memblock-reserved region and hosts every Task/file object; a forged
    // or stale PTE walking through the mm-teardown path used to be able to
    // put_page+free_pages an arbitrary physical frame — if that frame was
    // heap memory, the next zone allocation handed kernel objects to a
    // user mapping (observed: corrupted task children lists, do_wait
    // panics). Refuse loudly and keep the frame leaked instead.
    if super::hotpath::memblock().is_reserved(addr) {
        dw::FREE_REFUSED_RESERVED
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // Rate-limited (see report_free_refusal): sc83 showed a 7442-line
        // flood of this exact line from one teardown walk of drm-GEM-mapped
        // kernel-heap frames; the printk storm from the free context was
        // itself a wedge vector. The walker-side is_foreign_user_frame skip
        // now prevents the GEM case from reaching here at all — this
        // refusal remains the last-line defense for genuinely forged frees.
        report_free_refusal(&format_args!(
            "free_pages: REFUSED boot-reserved frame {:#x} order {} — not returned to zone",
            addr, order
        ));
        return;
    }

    // SIGILL-stamp tripwire (gnome-sigill diagnostic): a STAMPED frame
    // (file-fill page of a user mapping) reached the allocator from a
    // path the teardown adjudicator (mmu_init::release_user_leaf) does
    // not cover.
    //   owner == dying pid : legal final put of the owner's own frame —
    //       drop the stamp and allow the free (counted, silent: printk
    //       from free contexts wedges the box).
    //   owner != dying pid : foreign free — the frame may still be mapped
    //       by a live process (its r-x text); recycling it is the SIGILL
    //       mechanism. Under rux:mmforensics (retired debug=mmforensics) refuse the free (leak) and
    //       print the evidence; with the switch off, clear the stamp and
    //       keep the historical allocator behavior.
    // The walk-path (exit/unmap) frees are adjudicated properly in
    // release_user_leaf — owner-map scan, lifecycle replay, refusal.
    static TEXT_OWN_FREES: core::sync::atomic::AtomicUsize =
        core::sync::atomic::AtomicUsize::new(0);
    {
        let pfn = phys_to_pfn(addr);
        let stamped = if order == 0 {
            super::hotpath::text_stamp_get(pfn)
        } else {
            // block free: check the first and last member
            match super::hotpath::text_stamp_get(pfn) {
                Some(s) => Some(s),
                None => super::hotpath::text_stamp_get(
                    pfn + (1usize << order) - 1,
                ),
            }
        };
        if let Some((owner_pid, owner_va)) = stamped {
            let dying = crate::process::hotpath::current_pid();
            if owner_pid == dying {
                super::hotpath::text_stamp_remove(pfn);
                // Silent by design: printing from free_pages contexts (any
                // spinlock-holding path can free pages) floods printk and
                // perturbs timing enough to soft-lockup the box. The count
                // is readable from the FOREIGN report below.
                TEXT_OWN_FREES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            } else if crate::trace::EVENT_MMFORENSICS.armed() {
                super::hotpath::text_stamp_remove(pfn);
                #[cfg(feature = "riscv64")]
                {
                    // Attribution: the dying context's page-table ROOT (to
                    // correlate with the FUT teardown ring / EARLY-ROOT-FREE
                    // witnesses) plus a symbolizable kernel backtrace via
                    // the debug unwinder (printk-free console writer, safe
                    // under the locks this path can hold).
                    let root = crate::sched::hotpath::current()
                        .and_then(|t| t.address_space().map(|mm| mm.root_ppn()))
                        .unwrap_or(0);
                    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                    crate::pr_err!(
                        "TEXTFREE-FOREIGN pfn={:#x} order={} owner_pid={} owner_va={:#x} dying_pid={} dying_root={:#x} ownfrees={} — free REFUSED (leaked)",
                        pfn, order, owner_pid, owner_va, dying, root,
                        TEXT_OWN_FREES.load(core::sync::atomic::Ordering::Relaxed)
                    );
                    crate::debug::backtrace::dump_stack();
                    dw::FREE_REFUSED_TEXT
                        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
                #[cfg(not(feature = "riscv64"))]
                {
                    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
                    crate::pr_err!(
                        "TEXTFREE-FOREIGN pfn={:#x} order={} owner_pid={} owner_va={:#x} dying_pid={} — free REFUSED (leaked)",
                        pfn, order, owner_pid, owner_va, dying
                    );
                    dw::FREE_REFUSED_TEXT
                        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
                return;
            } else {
                // Switch off: keep historical behavior, just drop the stamp.
                super::hotpath::text_stamp_remove(pfn);
            }
        }
    }

    // DEBUG memwatch: attribute the free back to the site that stamped this
    // block (page-level leak accounting — see debug::memwatch).
    crate::debug::memwatch::note_page_free(addr, order);

    let pfn = phys_to_pfn(addr);

    // Try to free to the Zone system first
    // SAFETY: exclusive node access — caller must ensure no concurrent mutation.
    if let Some(node) = super::r#unsafe::first_node_mut() {
        // Try each zone type to find which one contains this PFN.
        // ZoneMovable included (review 4.7): pages allocated with
        // __GFP_MOVABLE live there and silently leaked when their free
        // fell through this list.
        for zone_type in [
            ZoneType::ZoneNormal,
            ZoneType::ZoneMovable,
            ZoneType::ZoneDma32,
            ZoneType::ZoneDma,
        ] {
            if let Some(zone) = node.zone_mut(zone_type) {
                if zone.is_initialized() {
                    let start_pfn = zone.start_pfn();
                    let end_pfn = zone.end_pfn();
                    if pfn >= start_pfn && pfn < end_pfn {
                        // Per-CPU pageset fast path (perf review
                        // 2026-10-31): eligible order-0 frames cache on
                        // this CPU with no zone-lock round trip; every
                        // tripwire above (mapped/wild/reserved/text) has
                        // already passed, and pcp::free_order0 owns the
                        // OnPcp double-free tripwire. Cache full -> the
                        // drain inside free_order0 returns frames to the
                        // buddy in bulk.
                        if order == 0
                            && zone_type == ZoneType::ZoneNormal
                            && super::hotpath::free_order0(pfn, zone)
                        {
                            dw::FREE_OK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                            return;
                        }
                        zone.free_pages(pfn, order);
                        dw::FREE_OK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                }
            }
        }
    }

    // Page doesn't belong to any zone - this shouldn't happen in normal operation
    // Pages allocated via memblock during early boot are not tracked and don't need freeing
    dw::FREE_NO_ZONE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Free a single page
pub fn free_page(addr: usize) {
    free_pages(addr, 0);
}

// ==================== Page Helper Functions ====================

/// Get the page descriptor for a kernel VIRTUAL address (linear mapping).
///
/// Review 4.x: the old implementation treated the argument as a PHYSICAL
/// address (`addr / PAGE_SIZE`) while every caller-visible name says
/// "virt" — for any kernel-linear VA the PFN came out ~PAGE_OFFSET/PAGE_SIZE
/// too large, yielding a wild descriptor. Now the address is properly
/// translated through virt_to_phys() (identity-mapped legacy region
/// included); addresses outside both fall through to virt_to_phys's
/// warned identity behavior.
pub fn virt_to_page(addr: usize) -> *mut Page {
    pfn_to_page_mut(virt_to_pfn(addr))
}

/// Get page frame number from a kernel virtual address.
pub fn virt_to_pfn(addr: usize) -> usize {
    let phys = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(addr as u64),
    );
    (phys.bits() as usize) / PAGE_SIZE
}

/// Get physical address from page descriptor
pub fn page_to_phys(page: &Page) -> usize {
    super::hotpath::page_to_pfn(page) * PAGE_SIZE
}

/// Get the kernel-linear virtual address of a page described by `page`.
pub fn page_to_virt(page: &Page) -> usize {
    let phys = page_to_phys(page);
    crate::arch::mm::phys_to_virt(
        crate::arch::mm::PhysAddr::new(phys as u64),
    )
    .bits() as usize
}

// ==================== Dead-code removal note (review 4.7) ====================
// The THIRD buddy implementation that used to live here (struct
// BuddyAllocator + static KERNEL_BUDDY + init_kernel_buddy/buddy_alloc/
// buddy_free/buddy_stats) had ZERO callers: the kernel heap uses
// mm/buddy_allocator.rs (GLOBAL_ALLOCATOR) and physical pages go through
// the zone system in this file. Three coexisting buddy implementations
// tripled the audit surface for the same invariants; this one is deleted.
// The remaining two: zone.rs (Zone::alloc_pages/free_pages) and
// buddy_allocator.rs (#[global_allocator]).
// ==================== Zone System Initialization ====================

/// Initialize the zone system with physical memory
///
/// This replaces the separate user_phys_allocator with a unified zone system.
/// All physical page allocation should go through the zone system after this.
///
/// # Arguments
/// - `phys_start`: Physical memory start address
/// - `phys_size`: SIZE OF THE PFN SPAN (max region end − phys_start),
///   including MMIO holes — matching the descriptor/vmemmap sizing. The
///   zone spans the whole discovered map; only memblock-FREE pages are
///   seeded, so holes and static regions contribute no free pages.
/// - `kernel_end`: legacy, unused (the zone no longer starts after the
///   heap/slab window — that clamp is what left ~980 MB below
///   KERNEL_HEAP_PHYS permanently unmanaged on x86_64).
pub fn init_zone_system(phys_start: usize, phys_size: usize, _kernel_end: usize) {
    // Initialize node data structure: called once during early boot
    // before any page allocation (init_node_data is now a safe fn).
    init_node_data();

    // Get mutable node
    let node = match node_data_mut(0) {
        Some(n) => n,
        None => {
            // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
            crate::println!("page_alloc: Failed to get node 0 for zone initialization");
            return;
        }
    };

    // Validate phys_size against the descriptor ceiling (fake-OOM fix:
    // the old limit here was the Kernel.toml NOMINAL size, so >2GiB
    // machines silently lost every page above the line — see the
    // MAX_PAGES comment in page_desc.rs). The runtime descriptor count is
    // published by init_vmemmap from the same discovered memory map, so
    // vmemmap bounds stay consistent with the zone span.
    let max_pages = super::hotpath::MAX_PAGES;
    let max_phys_size = max_pages * PAGE_SIZE;
    let effective_phys_size = phys_size.min(max_phys_size);
    if effective_phys_size != phys_size {
        // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
        crate::pr_err!(
            "page_alloc: phys_size {}MB exceeds MAX_PAGES limit {}MB, truncating",
            phys_size / (1024 * 1024), max_phys_size / (1024 * 1024));
    }

    // Initialize node with total memory range
    let start_pfn = phys_start / PAGE_SIZE;
    let total_pages = effective_phys_size / PAGE_SIZE;
    node.init(start_pfn, total_pages, total_pages);

    // ZONE_NORMAL spans the WHOLE discovered PFN space (fake-OOM family):
    // the old span started at `kernel_end` (heap+slab end), so on x86_64
    // every memblock-free page below it — ~980 MB between the kernel
    // image and the heap at 0x40000000 — was never seeded and stayed
    // permanently unallocatable while the OOM killer fired (census:
    // 237,521 of 524,140 descriptors rc==0, unlinked, unmanaged on one
    // 2G boot). Linux parity: the zone spans present memory; reserved
    // ranges and non-RAM holes inside the span are simply never seeded,
    // and the memblock_is_reserved / PageFlag::Reserved guards on the
    // free path refuse anything forged against them.
    let alloc_start_pfn = start_pfn;
    let alloc_end_pfn = start_pfn + total_pages;
    let alloc_start = alloc_start_pfn * PAGE_SIZE;
    let alloc_end = alloc_end_pfn * PAGE_SIZE;

    // Create and initialize ZONE_NORMAL
    let mut zone = Zone::new(ZoneType::ZoneNormal, 0, 0);
    zone.init(alloc_start_pfn, alloc_end_pfn);

    // Add pages to the zone's buddy allocator
    // Use memblock_for_each_free_range to iterate over FREE memory ranges only
    // Free all memblock-reserved pages to the buddy allocator
    let mut total_added = 0usize;

    super::hotpath::memblock_for_each_free_range(alloc_start, alloc_end, |free_start, free_end| {
        // Convert to PFNs. Round the start UP and the end DOWN: a
        // PARTIALLY-reserved page must never reach the buddy allocator —
        // memblock reservations are byte ranges (e.g. the FDT /chosen
        // initrd range), and truncating them onto page boundaries handed
        // the tail page of a >4KB initrd to the allocator, whose first
        // allocations overwrote the image (gzip "corrupt deflate
        // stream" for every initrd larger than one page).
        let range_start_pfn = (free_start + PAGE_SIZE - 1) / PAGE_SIZE;
        let range_end_pfn = free_end / PAGE_SIZE;
        let range_pages = range_end_pfn.saturating_sub(range_start_pfn);

        if range_pages == 0 {
            return;
        }

        // Add pages in this free range to the zone
        let mut remaining = range_pages;
        let mut current_pfn = range_start_pfn;

        while remaining > 0 {
            // Find highest order that fits and is aligned
            let mut order = 0;
            for o in (0..=MAX_ORDER).rev() {
                let block_size = 1usize << o;
                if current_pfn % block_size == 0 && remaining >= block_size {
                    order = o;
                    break;
                }
            }

            // Add directly to zone's free list without buddy merging
            // During initialization, we know pages are not in any list yet
            zone.add_to_free_list_init(current_pfn, order);
            total_added += 1usize << order;

            current_pfn += 1usize << order;
            remaining -= 1usize << order;
        }
    });

    // Managed pages = what the buddy actually owns (span minus reserved
    // ranges and non-RAM holes), like Linux zone->managed_pages vs
    // spanned_pages. The old span-equals-managed accounting overstated
    // availability and sized watermarks off phantom pages.
    zone.set_managed_pages(total_added);

    // Add zone to node
    node.add_zone(ZoneType::ZoneNormal, zone);

    // Setup per-zone watermarks
    if let Some(zone_ref) = node.zone(ZoneType::ZoneNormal) {
        let total_managed = zone_ref.managed_pages();
        let refs: [&Zone; 1] = [zone_ref];
        super::hotpath::setup_per_zone_wmarks(&refs, total_managed);
    }

    // One-line boot banner: span vs seeded vs holes, so any future
    // structural loss is visible immediately.
    let span_pages = alloc_end_pfn - alloc_start_pfn;
    // HOTPATH-PRINT: allocator diagnostic — gated/budgeted (static-N cap, trace-event, or error-path arm; see the guard above)
    crate::println!(
        "page_alloc: zone Normal span={} pages ({} MB), seeded={} ({} MB), unmanaged-in-span={} ({} MB)",
        span_pages, span_pages * PAGE_SIZE / (1024 * 1024),
        total_added, total_added * PAGE_SIZE / (1024 * 1024),
        span_pages.saturating_sub(total_added),
        span_pages.saturating_sub(total_added) * PAGE_SIZE / (1024 * 1024)
    );
}

// ==================== Page Allocation APIs ====================

/// __get_free_pages - Allocate contiguous pages
///
/// # Safety
/// Caller must ensure the returned physical address is used only by the kernel
/// and freed exactly once via `__free_pages` with the correct order.
pub fn __get_free_pages(gfp_flags: GfpFlags, order: usize) -> usize {
    alloc_pages(gfp_flags, order)
}

/// __get_free_page - Allocate a single page
///
/// # Safety
/// Caller must ensure the returned physical address is used only by the kernel
/// and freed exactly once via `__free_page`.
pub fn __get_free_page(gfp_flags: GfpFlags) -> usize {
    alloc_page(gfp_flags)
}

/// __get_zeroed_page - Allocate and zero a page
///
/// # Safety
/// Caller must ensure the returned physical address is used only by the kernel
/// and freed exactly once via `__free_page`.
pub fn __get_zeroed_page(gfp_flags: GfpFlags) -> usize {
    get_zeroed_page(gfp_flags)
}

/// __free_pages - Free pages
///
/// # Safety
/// `addr` must be a physical address previously returned by `__get_free_pages`
/// with the same `order`, and must not have been freed already.
pub fn __free_pages(addr: usize, order: usize) {
    free_pages(addr, order);
}

/// __free_page - Free a single page
///
/// # Safety
/// `addr` must be a physical address previously returned by `__get_free_page`
/// or `__get_zeroed_page`, and must not have been freed already.
pub fn __free_page(addr: usize) {
    free_page(addr);
}


// ════════════════ rmap ════════════════════════════════
// (was rmap.rs)
// Reverse Mapping (rmap) Infrastructure
//
// This module implements reverse mapping, which allows finding all
// virtual addresses that map a given physical page. This is essential
// for page migration, memory compaction, and page reclamation.

// (INV-RMAP-1: the reverse-map family — the anon/file rmap walks
// hold the map's lock; the descriptor index words are read under the
// rmap lock's Release publication.)

// ==================== AnonVma ====================

// AnonVma, its chain alias and the RmapStats snapshot are defined in
// mm/types.rs; every impl and the rmap operations stay here.
// Re-exported so the `mm::rmap::…` paths are unchanged.

impl AnonVma {
    /// Create a new anon_vma
    pub fn new() -> Self {
        Self {
            refcount: AtomicUsize::new(1),
            root: AtomicPtr::new(core::ptr::null_mut()),
            children: RwSpinlock::new(Vec::new()),
            vma: AtomicUsize::new(0),
        }
    }

    /// Increment reference count
    pub fn get(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement reference count
    /// Returns true if this was the last reference
    pub fn put(&self) -> bool {
        self.refcount.fetch_sub(1, Ordering::AcqRel) == 1
    }

    /// Get reference count
    pub fn refcount(&self) -> usize {
        self.refcount.load(Ordering::Acquire)
    }

    /// Set associated VMA
    pub fn set_vma(&self, vma: *const Vma) {
        self.vma.store(vma as usize, Ordering::Release);
    }

    /// Get associated VMA
    pub fn get_vma(&self) -> Option<&Vma> {
        let ptr = self.vma.load(Ordering::Acquire) as *const Vma;
        if ptr.is_null() {
            None
        } else {
            // ptr was stored by set_vma() and the VMA outlives the page
            // (INV-MM-2 reachability arm).
            Some(crate::syscall::r#unsafe::boxed_ref(ptr))
        }
    }
}

// ==================== Page Rmap Operations ====================

/// Add reverse mapping for an anonymous page
///
/// # Arguments
/// - `page`: Page descriptor
/// - `vma`: VMA containing the mapping
/// - `address`: Virtual address of the mapping
/// - `exclusive`: Whether this is an exclusive mapping
pub fn page_add_anon_rmap(page: &Page, _vma: &Vma, address: usize, _exclusive: bool) {
    // page is exclusively owned (refcount == 1) when adding rmap
    // (Page's mutators are interior-mutable &self — the old wrappers
    // were vestigial).
    {
        // Set anonymous and swap-backed flags
        page.set_flag(super::types::PageFlag::Anonymous);
        page.set_flag(super::types::PageFlag::SwapBacked);

        // Set index (virtual page offset) for rmap
        let index = address / super::types::PAGE_SIZE;
        page.set_index(index);

        // Increment map count
        page.inc_mapcount();

        // Add to LRU_INACTIVE_ANON on first mapping
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        if page.mapcount() == 0 {
            crate::api::mm::page_add_anon_lru(page);
        }
    }
}

/// Add reverse mapping for a file-backed page
///
/// # Arguments
/// - `page`: Page descriptor
/// - `mapping`: Address space (file mapping)
/// - `index`: Page offset in the file
pub fn page_add_file_rmap(page: &Page, mapping: usize, index: usize) {
    // page is exclusively owned when adding file rmap (interior-
    // mutable mutators; vestigial wrapper removed).
    {
        // Set mapping and index (rmap only; LRU uses dedicated field)
        page.set_mapping(mapping as *mut core::ffi::c_void);
        page.set_index(index);

        // Increment map count
        page.inc_mapcount();

        // Add to LRU_INACTIVE_FILE on first mapping
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        if page.mapcount() == 0 {
            crate::api::mm::page_add_file_lru(page);
        }
    }
}

/// Remove reverse mapping for a page
///
/// # Arguments
/// - `page`: Page descriptor
pub fn page_remove_rmap(page: &Page) {
    // caller holds page lock (or page is unshared).
    {
        // R21-1: dec_mapcount returns the POST-decrement value; the
        // convention is -1 (PAGE_MAPCOUNT_BIAS) = unmapped, 0 = one
        // mapping. The old `== 0` check fired one mapping EARLY (a
        // fork-COW page lost Anonymous/LRU at 2->1) and never on the true
        // last unmap (page freed to buddy still LRU-linked).
        let new_count = page.dec_mapcount();

        // If last mapping, clear flags and remove from LRU.
        // Safe now: LRU uses dedicated lru_next field, not mapping/index
        //
        // PageCache frames (mm filemap_fault, x86_64): the file LRU
        // membership and the mapping/index fields belong to the PAGE
        // CACHE — the last PTE unmap must NOT unlink the frame from the
        // cache's LRU (eviction would lose sight of a still-cached page)
        // and must not clear bookkeeping the cache still needs. The
        // mapping's own release (descriptor put + cache pin put) happens
        // in the unmap/teardown callers keyed off the PTE's PCACHE bit.
        if new_count == -1 && page.page_type() != super::types::PageType::PageCache {
            page.clear_flag(super::types::PageFlag::Anonymous);
            page.clear_flag(super::types::PageFlag::SwapBacked);
            crate::api::mm::page_remove_lru(page);
        }
    }
}

/// Check if a page is mapped (has at least one PTE)
pub fn page_mapped(page: &Page) -> bool {
    page.mapcount() >= 0
}

/// Get all virtual addresses mapping a page
///
/// This is used for page migration and memory compaction.
///
/// # Returns
/// Vector of (mm_struct_ptr, virtual_address) pairs
pub fn page_get_mappings(_page: &Page) -> Vec<(usize, usize)> {
    // In a full implementation, this would:
    // 1. Check if page is anonymous or file-backed
    // 2. For anonymous pages: walk anon_vma chain
    // 3. For file pages: walk address_space i_mmap tree
    // 4. Return all (mm, address) pairs

    // Placeholder: return empty vector
    Vec::new()
}

/// Check if page was recently referenced
///
/// Used by page reclamation to determine if a page is still
/// being accessed.
pub fn page_referenced(page: &Page) -> bool {
    // Check referenced flag
    page.test_flag(super::types::PageFlag::Referenced)
}

/// Clear referenced flag on a page
pub fn page_clear_referenced(page: &Page) {
    page.clear_flag(super::types::PageFlag::Referenced);
}

/// Try to unmap a page from all processes.
///
/// Used during page reclamation (vmscan) to remove all PTEs mapping
/// a given physical page so it can be freed back to the zone allocator.
///
/// # Returns
/// Number of PTEs successfully unmapped.
pub fn try_to_unmap(page: &Page) -> i32 {
    try_to_unmap_inner(page, 0)
}

/// Try to unmap a page from all processes, replacing PTEs with a swap entry.
///
/// Like `try_to_unmap()` but writes `swap_entry` into each PTE instead of
/// zeroing it. Used by the swap-out path in vmscan.
///
/// # Returns
/// Number of PTEs successfully replaced with swap entries.
pub fn try_to_unmap_with_swap(page: &Page, swap_entry: u64) -> i32 {
    try_to_unmap_inner(page, swap_entry)
}

/// Record a (mm, virtual page number) mapping on the page (review 4.10).
///
/// Slot 0 (the `index` field) holds the FIRST mapping; up to three more
/// fit in `rmap_alt`. Beyond that the page keeps only slot 0 and the
/// `RmapOverflow` flag sends reverse-map lookups through the full task
/// scan (degraded single-value recording).
///
/// Called from the fault paths that install a PTE (page_fault.rs /
/// mm_ops.rs) — `page_add_anon_rmap` has no live callers.
pub fn page_record_mapping(page: &Page, mm_ptr: usize, vaddr: usize) {
    let vpn = vaddr / super::types::PAGE_SIZE;
    if page.index() == 0 {
        // First mapping — slot 0.
        page.set_index(vpn);
        return;
    }
    if page.index() == vpn {
        return; // re-recording the same mapping (index only) — no-op
    }

    // TRIPWIRE (heapstress/gnome-shell corruption hunt): a SECOND mapping
    // of the same frame into the SAME mm at a different va. In the
    // anonymous single-mm workloads that reproduce the heap corruption
    // this never happens legitimately — it means the frame was handed to
    // two faults (allocator double-handout, or freed while still mapped
    // and reused). Slot 0 carries no mm; only alt slots are checkable.
    {
        static DMAP_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let mut same_mm = false;
        page.rmap_alt_for_each(|m, _v| {
            if m == mm_ptr {
                same_mm = true;
            }
        });
        if same_mm && DMAP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            crate::pr_err!(
                "FRAME-DOUBLE-MM page={:#x} slot0_vpn={} new_vpn={} mm={:#x} pid={} mapcount={}",
                page as *const _ as usize,
                page.index(),
                vpn,
                mm_ptr,
                crate::process::hotpath::current_pid(),
                page.mapcount()
            );
        }
    }

    if !page.rmap_alt_add(mm_ptr, vpn) {
        // All 3 alternate slots busy: degrade to single-value recording.
        page.set_flag(super::types::PageFlag::RmapOverflow);
    }
}

/// Try to unmap a page from all processes, replacing PTEs with a
/// migration-entry marker (compaction, review 4.16).
///
/// Like `try_to_unmap()` but writes a migration marker into each PTE
/// instead of zeroing it: a fault on the marker WAITS for the migration to
/// complete instead of installing a zero page (which the subsequent remap
/// would overwrite — silent user-write loss).
///
/// # Returns
/// Number of PTEs successfully replaced with migration markers.
pub fn try_to_unmap_migration(page: &Page) -> i32 {
    try_to_unmap_inner(page, crate::api::mm::make_migration_entry())
}

/// Shared implementation for try_to_unmap and try_to_unmap_with_swap.
///
/// When `swap_entry == 0`, PTEs are zeroed (unmap).
/// When `swap_entry != 0`, PTEs are replaced with the swap entry (swap-out).
fn try_to_unmap_inner(page: &Page, swap_entry: u64) -> i32 {
    if !page_mapped(page) {
        return 0;
    }

    if !page.is_anonymous() {
        // File-backed pages: not yet supported (needs address_space walk).
        return 0;
    }

    let target_pfn = super::hotpath::page_to_pfn(page as *const Page);
    let target_index = page.index();
    if target_index == 0 && !page.test_flag(super::types::PageFlag::RmapOverflow) {
        return 0;
    }

    // Candidate virtual addresses: slot 0 (page.index) plus every alternate
    // rmap slot (multi-mapping pages — MAP_FIXED re-maps of the same frame).
    // Review 4.10: with only slot 0, unmapping a double-mapped page left the
    // second PTE in place while the page was freed/reused (stale-PTE UAF).
    let mut target_vaddrs: Vec<usize> = Vec::new();
    if target_index != 0 {
        target_vaddrs.push(target_index * (super::types::PAGE_SIZE as usize));
    }
    page.rmap_alt_for_each(|_mm, vpn| {
        let vaddr = vpn * (super::types::PAGE_SIZE as usize);
        if !target_vaddrs.contains(&vaddr) {
            target_vaddrs.push(vaddr);
        }
    });

    let unmapped_count = Cell::new(0i32);

    // mm-teardown vs spinlocks: for_each_task holds pid-hash bucket locks
    // across the callback; an mm Arc dropped there could be the LAST
    // reference, running the page-table teardown (which may schedule an
    // RCU grace-period wait) under the spinlock. Collect the pinned Arcs
    // under the locks (moved into the vector — never dropped there), then
    // walk them with no locks held. RefCell: the walker takes F: Fn.
    let mm_pins = Cell::new(Vec::<alloc::sync::Arc<super::types::MmStruct>>::new());
    crate::sched::hotpath::for_each_task(|task| {
        // for_each_task hands out &Task references; the task struct is
        // protected by the task list lock inside for_each_task.
        {
            let task = task;

            // Skip tasks without an address space (kernel threads)
            // R25-2: PIN the address space — do_exit's set_address_space(None)
            // + Drop(MmStruct) free the page tables without the VMA lock;
            // a raw borrow raced that free (NEW2-shaped UAF-read). The
            // clone is mm-slot-lock serialized (cannot resurrect a freed
            // Arc).
            if let Some(mm_arc) = task.address_space_arc() {
                let mut pins = mm_pins.take();
                pins.push(mm_arc);
                mm_pins.set(pins);
            }
        }
    });

    for mm_arc in mm_pins.take() {
        // The Arc pins this mm (page tables and VMA tree stay
        // allocated); everything below is safe API except the single
        // leaf_pte_replace edge (audited at its call).
        {
        let mm = mm_arc.as_ref();

            for target_vaddr in target_vaddrs.iter() {
                let target_vaddr = *target_vaddr;

                // Hold VMA lock across both the check and page table walk
                // to prevent concurrent munmap from freeing page tables (fixes F03-07).
                //
                // SP-8 (stress-port r8): admit FileBacked VMAs too. A
                // PRIVATE file VMA hosts anonymous COW pages (the fault
                // paths' FileBacked write-fault arm flags the exclusive
                // copy Anonymous+SwapBacked and links it on the anon LRU
                // — do_cow_fault parity), and the reference kernel swaps
                // those pages: page_add_new_anon_rmap chains them into
                // the VMA's anon_vma and rmap_walk_anon never looks at
                // the VMA type. Our old Anonymous-only filter made every
                // private-COW page (an exec'd binary's .data/.bss under
                // fork churn) permanently unswappable: vmscan passed all
                // its filters, the walk found no Anonymous VMA at the
                // recorded address, unmap0 freed the slot, and the page
                // was re-skipped on every later scan (the r7 unmap0
                // residual bucket). The PTE ppn==target_pfn match below
                // stays the authority — widening the VMA gate can only
                // find MORE real mappings of the target frame; shared
                // file VMAs never host Anonymous-flagged frames, and
                // shmem pages (Anonymous but not SwapBacked) never reach
                // this walk from vmscan.
                let vma_mgr = mm.vma_read();
                let vma_matches = vma_mgr.iter().any(|vma| {
                    vma.contains(super::types::VirtAddr::new(target_vaddr))
                        && matches!(
                            vma.vma_type(),
                            super::types::VmaType::Anonymous
                                | super::types::VmaType::FileBacked
                        )
                });

                if !vma_matches {
                    continue;
                }
                // vma_mgr still held — protects page table walk below

                let root_ppn = mm.pgd();
                // SAFETY: root_ppn is the Arc-pinned mm's live root;
                // the walker only reads page-table pages.
                // INV-SY-1 walk edge (Arc-pinned mm's live root).
                let walk_result =
                    crate::sync::r#unsafe::pt_walk(root_ppn, target_vaddr as u64);

                if let Some((ppn, _pte_bits)) = walk_result {
                    if ppn as usize == target_pfn {
                        // FD-1: the leaf replace (lock, ppn re-validation,
                        // raw write, unlock) is the arch's leaf_pte_replace
                        // — the manual Sv39 descent that used to live here
                        // indexed the root with the wrong bits on the
                        // 4-level arches and silently no-oped, so swap-out
                        // unmap and migration never unmapped anything on
                        // x86_64/aarch64. TLB flush stays with the caller
                        // (R10-6: hart-local; the batch shootdown below).
                        // FD-1 — the arch's audited leaf replace: mm is
                        // Arc-pinned, root_ppn is this mm's live root,
                        // the ppn was just re-validated by the walk
                        // above (leaf_pte_replace is a safe fn — the
                        // old wrapper was vestigial).
                        if crate::arch::mm::mm_ops::leaf_pte_replace(
                            mm,
                            root_ppn,
                            target_vaddr as u64,
                            target_pfn as u64,
                            swap_entry,
                        ) {
                            super::hotpath::flush_tlb_page_local(target_vaddr as u64);

                            // Decrement mapcount
                            page.dec_mapcount();
                            unmapped_count.set(unmapped_count.get() + 1);
                        }
                    }
                }
            }
        }
    }

    // R-SC-TLB (abi-review session 4, class-a): every PTE this pass
    // replaced (zero / swap entry / migration marker) may still sit in
    // ANOTHER CPU's TLB. The per-address flush inside the loop is
    // hart-local (R9-18: a "global" sfence buys nothing locally — but
    // it also reaches no other hart), and unlike the munmap/mprotect/
    // fork paths — where mm_ops issues arch::ipi::flush_tlb_others for
    // exactly this reason — the reclaim/migration callers never had a
    // shootdown. A remote CPU holding a stale entry keeps TRANSLATING
    // through it after this pass: for compaction that is a write into
    // the source frame AFTER migrate_page freed it back to the buddy
    // (cross-frame corruption; the stale entry survives until an
    // unrelated flush evicts it), and for swap-out a write into a frame
    // the swap layer already captured (silent lost write + stale-data
    // readback). One range-agnostic broadcast per pass (the same coarse
    // form mm_ops uses) closes the window: the fault path's
    // Migrating-marker wait covers fresh faults in the migration window,
    // and for swap there is no re-create race (the PTE now holds the
    // swap entry).
    if unmapped_count.get() > 0 {
        #[cfg(feature = "riscv64")]
        crate::arch::ipi::flush_tlb_others();
        #[cfg(feature = "x86_64")]
        crate::arch::ipi::flush_tlb_others(0, 0);
        #[cfg(feature = "aarch64")]
        crate::arch::ipi::flush_tlb_others();
    }

    // hp89 hunt: rmap-driven unmap (reclaim swap-out / migration) —
    // record for post-hoc #GP attribution (mm/unwatch).
    if unmapped_count.get() > 0 {
        super::hotpath::note(
            super::hotpath::KIND_TRY_TO_UNMAP,
            target_pfn as u64,
            swap_entry,
            unmapped_count.get() as u64,
        );
    }

    unmapped_count.get()
}

// ==================== Rmap Statistics ====================

/// Get rmap statistics
pub fn rmap_stats() -> RmapStats {
    // Placeholder: would need to track these globally
    RmapStats::default()
}


// ════════════════ vmemmap ═════════════════════════════
// (was vmemmap.rs)
// Virtual Memory Map (vmemmap) Management
//
// This module implements vmemmap for page descriptor mapping.
// Instead of using a static array, page descriptors are mapped at a fixed
// virtual address region (VMEMMAP_START).
//
// Advantages:
// - No large static array in kernel image
// - Dynamically mapped based on actual physical memory
// - O(1) PFN to page conversion via simple arithmetic

/// Page descriptor size (64 bytes for struct Page)
pub const STRUCT_PAGE_SIZE: usize = core::mem::size_of::<Page>();

/// Number of struct Page entries per page
/// One 4KB page can hold 4096 / 64 = 64 page descriptors
pub const PAGES_PER_VMEMMAP_PAGE: usize = PAGE_SIZE / STRUCT_PAGE_SIZE;

/// vmemmap initialized flag
static VMEMMAP_INIT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// vmemmap statistics
pub(super) static mut VMEMMAP_STATS: VmemmapStats = VmemmapStats {
    initialized: false,
    vmemmap_base: VMEMMAP_START,
    start_pfn: 0,
    nr_pages: 0,
    vmemmap_pages: 0,
};

/// Convert PFN to vmemmap virtual address
///
/// vmemmap_addr = VMEMMAP_START + (pfn - vmemmap_start_pfn) * sizeof(Page)
///
/// vmemmap = VMEMMAP_START - vmemmap_start_pfn
/// So pfn_to_page(pfn) = vmemmap + pfn = VMEMMAP_START + (pfn - vmemmap_start_pfn) * sizeof(Page)
///
/// Review 4.20: `pfn - start_pfn` used to UNDERFLOW (wrap) for PFNs below
/// the managed base, yielding a wild descriptor address — clamp instead.
#[inline]
pub fn pfn_to_vmemmap(pfn: usize) -> usize {
    // VMEMMAP_STATS is initialized by init_vmemmap() before any
    // pfn_to_vmemmap() call; start_pfn is a plain usize field (no
    // mutation concurrent with vmemmap reads) — INV-MM-9.
    let start_pfn = super::r#unsafe::vmemmap_stats_read().start_pfn;
    VMEMMAP_START + pfn.saturating_sub(start_pfn) * STRUCT_PAGE_SIZE
}

/// Convert vmemmap virtual address to PFN
///
/// pfn = (vmemmap_addr - VMEMMAP_START) / sizeof(Page) + vmemmap_start_pfn
///
/// Clamped on the low side, mirroring pfn_to_vmemmap (review 4.20).
#[inline]
pub fn vmemmap_to_pfn(vaddr: usize) -> usize {
    // Same as pfn_to_vmemmap — VMEMMAP_STATS is initialized before use
    // (INV-MM-9).
    let start_pfn = super::r#unsafe::vmemmap_stats_read().start_pfn;
    let vaddr = vaddr.clamp(VMEMMAP_START, usize::MAX - STRUCT_PAGE_SIZE + 1);
    start_pfn + vaddr.saturating_sub(VMEMMAP_START) / STRUCT_PAGE_SIZE
}

/// Check if vmemmap is initialized
#[inline]
pub fn is_vmemmap_initialized() -> bool {
    VMEMMAP_INIT.load(core::sync::atomic::Ordering::Acquire)
}

/// Initialize vmemmap mapping
///
/// This function maps the vmemmap region to physical pages.
/// Each physical page can hold 64 page descriptors.
///
/// # Arguments
/// - `start_pfn`: Start PFN of physical memory (e.g., 0x80000 for 0x80000000)
/// - `nr_pages`: Number of physical pages to map
///
/// # Returns
/// - Ok(()) on success
/// - Err(()) if mapping fails
pub fn init_vmemmap(start_pfn: usize, nr_pages: usize) -> Result<(), ()> {
    if VMEMMAP_INIT.swap(true, core::sync::atomic::Ordering::AcqRel) {
        // Already initialized
        return Ok(());
    }

    // Review 4.20: the swap(true) above is only the re-entrancy guard; on
    // every failure path below it must be rolled back so that
    // is_vmemmap_initialized() does not report a half-initialized vmemmap
    // (the old code "initialized" on error and lied to every later check).
    macro_rules! vmemmap_fail {
        () => {{
            VMEMMAP_INIT.store(false, core::sync::atomic::Ordering::Release);
            return Err(());
        }};
    }

    // Validate nr_pages against MAX_PAGES from page_desc
    // This ensures pfn_to_page() bounds check is consistent with vmemmap mapping
    let max_pages = super::hotpath::MAX_PAGES;
    let effective_nr_pages = nr_pages.min(max_pages);
    if effective_nr_pages != nr_pages {
        crate::pr_err!(
            "vmemmap: nr_pages {} exceeds MAX_PAGES {} ({} GiB ceiling) — truncating; raise MAX_PAGES",
            nr_pages, max_pages, max_pages * PAGE_SIZE / (1024 * 1024 * 1024));
    }

    // Calculate how many vmemmap pages we need
    // Each vmemmap page (4KB) can hold 64 page descriptors
    let vmemmap_pages = (effective_nr_pages + PAGES_PER_VMEMMAP_PAGE - 1) / PAGES_PER_VMEMMAP_PAGE;

    // vmemmap starts at VMEMMAP_START
    // page descriptors are accessed via: VMEMMAP_START + (pfn - start_pfn) * sizeof(Page)
    let vmemmap_start = VMEMMAP_START;
    let vmemmap_end = VMEMMAP_START + effective_nr_pages * STRUCT_PAGE_SIZE;

    // Check if vmemmap range is valid
    if vmemmap_end > VMEMMAP_END {
        vmemmap_fail!();
    }

    // Use memblock to find a contiguous region for vmemmap pages
    let vmemmap_size = vmemmap_pages * PAGE_SIZE;

    // Calculate the actual physical memory end address (arch-aware base:
    // riscv64 RAM starts at 0x80000000, x86_64 at 0 — the old hardcoded
    // base made the search range land outside RAM on x86 and init fail)
    let phys_base = super::hotpath::PHYS_MEMORY_BASE;
    let phys_end = phys_base + effective_nr_pages * PAGE_SIZE;
    let vmemmap_phys = super::hotpath::memblock_find_in_range(
        vmemmap_size,
        phys_base,
        phys_end,
    );

    let vmemmap_phys = match vmemmap_phys {
        Some(addr) => addr,
        None => vmemmap_fail!(),
    };

    // Reserve the memory for vmemmap
    if super::hotpath::memblock_reserve(vmemmap_phys, vmemmap_size).is_err() {
        vmemmap_fail!();
    }

    // Zero the vmemmap pages using linear mapping
    // Must use virtual address since MMU is enabled
    // (the arch's PhysAddr: `PhysAddr` alone is types.rs's in this file)
    let vmemmap_virt = phys_to_virt(crate::arch::mm::PhysAddr::new(vmemmap_phys as u64));
    {
        let mut w = crate::debug::backtrace::ConsoleWriter::new();
        use core::fmt::Write;
    }
    // vmemmap_virt points to the reserved physical region returned by
    // memblock_find_in_range; vmemmap_size is the exact allocation
    // size (INV-MM-9 init arm).
    super::r#unsafe::vmemmap_zero_region(vmemmap_virt.bits() as usize, vmemmap_size);

    // Map each vmemmap page
    let flags = PageTableEntry::V | PageTableEntry::R |
               PageTableEntry::W | PageTableEntry::A |
               PageTableEntry::D;

    for i in 0..vmemmap_pages {
        let vaddr = vmemmap_start + i * PAGE_SIZE;
        let paddr = vmemmap_phys + i * PAGE_SIZE;

        // vaddr is within [VMEMMAP_START, VMEMMAP_END) (checked
        // above); paddr is the reserved physical page from memblock
        // (INV-MM-9 init arm).
        super::r#unsafe::vmemmap_map_kernel_page(vaddr, paddr, flags);
    }

    // Final TLB flush after all mappings - MUST flush before accessing!
    crate::arch::mm::flush_tlb_all();

    // Store statistics: the VMEMMAP_INIT guard ensures single
    // initialization; no concurrent readers until init_vmemmap()
    // returns (INV-MM-9 init arm).
    super::r#unsafe::vmemmap_stats_write(VmemmapStats {
        initialized: true,
        vmemmap_base: VMEMMAP_START,
        start_pfn,
        nr_pages: effective_nr_pages,
        vmemmap_pages,
    });

    // Publish the runtime descriptor count (fake-OOM fix): every later
    // bounds check (pfn_to_page, phys_valid, forensic scans) uses this
    // instead of the Kernel.toml nominal size, so machines with more RAM
    // than the config value manage all of it.
    super::hotpath::set_nr_descs(effective_nr_pages);

    Ok(())
}

/// Get vmemmap statistics
pub fn vmemmap_stats() -> VmemmapStats {
    // VMEMMAP_STATS is initialized before any call to this function;
    // VmemmapStats is Copy, so this is a plain read of a static
    // (INV-MM-9).
    super::r#unsafe::vmemmap_stats_read()
}

// The VmemmapStats snapshot is defined in mm/types.rs. Re-exported so
// the `mm::vmemmap::VmemmapStats` path (the stats edges in mm::unsafe
// name it) is unchanged.


// ════════════════ deferred_free ═══════════════════════
// (was deferred_free.rs)
// Time-deferred heap frees for teardown-raced objects.
//
// Rationale (exitwalk probe, exit/teardown family): procfs walkers
// (generate_maps/smaps/status) take `task.address_space()` — an
// unaccounted `&MmStruct` borrow of a foreign task. Even with the
// teardown's vma_write walker-exclusion and the RCU grace-period wait,
// a reader preempted mid-syscall can resume AFTER the last Arc drop
// freed the MmStruct and the heap reused its memory: the reader then
// locks a garbage rwlock word and iterates a garbage tree pointer —
// the #GP-in-btree-navigate family (reference OH log pid 554,
// exitwalk pid 314/315: atomic_load inside the node comparison
// chasing a non-canonical child pointer).
//
// Mitigation: frees of allocations whose size matches a registered
// "deferred size" are held back for DEFER_MS before returning to the
// heap. By then the teardown has left the object in a coherent
// post-teardown state (empty VMA tree — late readers navigate a valid
// EMPTY BTreeMap, not freed memory) and any in-flight borrower of the
// old object has long finished. This is the same idea as Linux freeing
// mm_struct via call_rcu, but wall-clock based so kernel-preempted
// readers are covered too (our RCU counts preemption points as
// quiescent states, which a mid-syscall borrower is not).
//
// The remaining window (a reader parked >DEFER_MS mid-syscall) is
// negligible; the proper fix — pinning the mm with address_space_arc()
// in the procfs generators — is a one-line-per-site change in fs/ that
// this lane does not touch.
//
// Bounded: the pool is a fixed ring; on overflow the free is immediate
// (normal semantics). Reclamation is driven opportunistically from the
// alloc path (the same churn that fills the pool drains it), so an
// idle system never accumulates entries.

/// Pool capacity — bounded memory exposure (entries are 32 bytes each).
pub(super) const POOL_CAP: usize = 16384;
/// How long a matched free is held back (ms of jiffies).
const DEFER_MS: u64 = 1500;

// The pool's Entry is defined in mm/types.rs. Re-exported so the
// `mm::deferred_free::Entry` path (the raw-pool edge in mm::unsafe uses
// it) is unchanged.

/// Registered size classes whose frees are deferred (first writer wins
/// per slot). Two live classes: the MmStruct ArcInner block and the Task
/// page itself (procfs walkers also borrow the TASK unpinned — the reaper
/// on another CPU can free it mid-read; free_task_slot poisons the page
/// before dealloc, so late readers see poison values, never heap-reuse
/// garbage pointers).
static DEFER_SIZES: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
/// Number of slots ever registered (Release on register, Acquire in
/// try_defer). The size poll runs on EVERY large-object dealloc; this
/// bounds the Acquire-load scan to the live slots instead of always
/// walking all four (106th micro-lever; the walk was 0.8% of a boot
/// record — most deallocs pay one load now, and the pre-registration
/// phase pays zero).
static DEFER_N: AtomicUsize = AtomicUsize::new(0);
pub(super) static mut POOL: [Entry; POOL_CAP] = [Entry {
    ptr: 0,
    size: 0,
    align: 0,
    deadline: 0,
}; POOL_CAP];
static POOL_HEAD: AtomicUsize = AtomicUsize::new(0); // next slot to fill
static POOL_DRAINED: AtomicUsize = AtomicUsize::new(0); // oldest live entry
/// Earliest deadline currently in the pool — relaxed peek for the alloc
/// path so the common case costs one load, no lock.
static EARLIEST_DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);
static POOL_LOCK: crate::sync::spinlock::Spinlock<()> = crate::sync::spinlock::Spinlock::new(());

/// Register the size class whose frees are deferred (first writer wins;
/// call from the type's constructor). Only ONE size class is supported —
/// enough for the MmStruct teardown race this exists for.
pub fn register_deferred_size(size: usize) {
    if size == 0 {
        return;
    }
    // NOTE: no printk here — registration runs at the FIRST raw Task-slot
    // init (idle-task creation), earlier than printk bring-up on riscv64;
    // a pr_err there hung the boot at the banner (final2/final3 runs).
    // Engagement (first successful defer) is witnessed later, post-boot.
    for (i, slot) in DEFER_SIZES.iter().enumerate() {
        match slot.compare_exchange(0, size, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                DEFER_N.store(i + 1, Ordering::Release);
                return;
            }
            Err(v) if v == size => return, // already registered
            Err(_) => continue,
        }
    }
}

#[inline]
pub fn deferred_size() -> usize {
    DEFER_SIZES[0].load(Ordering::Acquire)
}

/// Maybe hold back this free. Returns true when the entry was queued
/// (the caller must NOT free); false when it should free normally
/// (size mismatch or pool full).
///
/// Safe in any context (irqsave spinlock; no allocation).
pub fn try_defer(ptr: *mut u8, layout: &Layout) -> bool {
    let size = layout.size();
    if size == 0 {
        return false;
    }
    let mut matched = false;
    let live = DEFER_N.load(Ordering::Acquire);
    if live > 0 {
        for slot in DEFER_SIZES.iter().take(live) {
            if slot.load(Ordering::Acquire) == size {
                matched = true;
                break;
            }
        }
    }
    if !matched {
        return false;
    }
    let now = crate::timer::get_jiffies();
    let deadline = now.saturating_add(DEFER_MS);
    let _g = POOL_LOCK.lock_irqsave();
    // INV-MM-8: the lock is held.
    let pool = super::r#unsafe::defer_pool_mut();
    let head = POOL_HEAD.load(Ordering::Relaxed);
    let drained = POOL_DRAINED.load(Ordering::Relaxed);
    let used = head.wrapping_sub(drained);
    if used >= POOL_CAP {
        return false; // pool full — fall back to immediate free
    }
    let idx = head % POOL_CAP;
    pool[idx] = Entry {
        ptr: ptr as usize,
        size,
        align: layout.align(),
        deadline,
    };
    POOL_HEAD.store(head.wrapping_add(1), Ordering::Relaxed);
    let earliest = EARLIEST_DEADLINE.load(Ordering::Relaxed);
    if deadline < earliest {
        EARLIEST_DEADLINE.store(deadline, Ordering::Relaxed);
    }
    // One-time engagement witness: proves the size class matched a real
    // free (the first iteration of this guard registered the WRONG size
    // — bare size_of::<MmStruct> instead of the ArcInner block — and
    // silently deferred nothing).
    static ENGAGED: AtomicUsize = AtomicUsize::new(0);
    if ENGAGED.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::pr_debug!(
                            "DEFERFREE: engaged (first MmStruct free held back {}ms)", DEFER_MS);
    }
    true
}

/// Cheap alloc-path hook: drain only when the earliest pending deadline
/// has passed (one relaxed load otherwise).
#[inline]
pub fn drain_if_due() {
    let now = crate::timer::get_jiffies();
    if now >= EARLIEST_DEADLINE.load(Ordering::Relaxed) {
        drain(now);
    }
}

/// Free every entry whose deadline has passed. Returns the number freed.
/// Entry payloads are copied out under the lock; the actual dealloc runs
/// OUTSIDE the pool lock and routes by region exactly like
/// `buddy_allocator::dealloc_direct` (slab region -> kfree, heap ->
/// untracked buddy) — never through the intercepted GlobalAlloc path, so
/// a drained entry cannot be re-deferred.
pub fn drain(now: u64) -> usize {
    let mut batch: [(usize, usize, usize); 64] = [(0, 0, 0); 64]; // ptr,size,align
    let mut batch_n = 0usize;
    let mut new_earliest = u64::MAX;
    {
        let _g = POOL_LOCK.lock_irqsave();
        // INV-MM-8: the lock is held.
        let pool = super::r#unsafe::defer_pool_mut();
        let mut drained = POOL_DRAINED.load(Ordering::Relaxed);
        let head = POOL_HEAD.load(Ordering::Relaxed);
        while drained < head && batch_n < batch.len() {
            let idx = drained % POOL_CAP;
            if pool[idx].deadline > now {
                new_earliest = new_earliest.min(pool[idx].deadline);
                break; // ring is deadline-ordered by construction
            }
            batch[batch_n] = (pool[idx].ptr, pool[idx].size, pool[idx].align);
            batch_n += 1;
            drained = drained.wrapping_add(1);
        }
        // Batch-limit truncation: more DUE entries remain behind us. If the
        // first survivor's deadline were not folded in, EARLIEST_DEADLINE
        // would go to u64::MAX and the alloc-path drain hook would sleep
        // until the NEXT deferred free lowers it again — every due entry
        // behind the batch stalls for up to DEFER_MS (plus forever, on a
        // pool that only ever drains). Same fixup the break path does.
        if drained < head {
            let idx = drained % POOL_CAP;
            new_earliest = new_earliest.min(pool[idx].deadline);
        }
        POOL_DRAINED.store(drained, Ordering::Relaxed);
        EARLIEST_DEADLINE.store(new_earliest, Ordering::Relaxed);
    }
    for i in 0..batch_n {
        let (ptr, size, align) = batch[i];
        if let Ok(layout) = Layout::from_size_align(size, align) {
            // Route by REGION like buddy_allocator::dealloc_direct: a
            // registered size class (Arc<MmStruct>, Arc<FdTable>,
            // Arc<SignalStruct>) is <= 4 KiB and 8-aligned, so the
            // ORIGINAL allocation usually came from the STATIC slab
            // region (CombinedAllocator prefers kmalloc for those
            // layouts), not the buddy heap. The old unconditional
            // dealloc_untracked_pub refused out-of-heap pointers
            // (FR_OUT_OF_WINDOW / DEALLOC_OUTSIDE) and permanently
            // LEAKED every slab-resident entry — the drain comment
            // claimed "calls dealloc_direct" but the code never did.
            // kfree never re-defers, so the no-re-defer contract holds
            // (INV-MM-8 drain arm).
            super::r#unsafe::defer_dealloc(ptr, layout);
        }
    }
    batch_n
}


// ════════════════ small_alloc ═════════════════════════
// (was small_alloc.rs)
// Small-object packing layer over the kernel heap buddy (SLUB-style).
//
// alloc-crash r12-16 root cause: the kernel heap's minimum block is one
// page, and an OpenHarmony first-frame boot holds ~213,000 net-live
// objects in the 1-byte..1-KB classes (measured live on a4 via the
// CR3-walked counters: 23.2M allocs / 23.0M frees; nets b4=23K, b5=37K,
// b6=26K, b7=62K, b8=38K, b9=6.5K). Each object pinned a whole 4 KiB
// buddy block — 852 MB of the 992 MB window — and the allocator died
// (a2/a3/a4: census `free=0K`, ALLOCTHROW -> panic) exactly when
// first-frame composition started. Linux answers this with kmalloc
// slabs; this module gives the buddy heap the same packing.
//
// # Design
//
// - Eight power-of-two classes, S = 8..1024. A layout is served here
//   when `size <= SMALL_MAX && align <= SMALL_MAX`; class S is the
//   power of two >= max(size, align). Objects sit at
//   page + FIRST_OFF(S) + i*S where FIRST_OFF(S) = max(16, S) — a
//   multiple of S — so every object is S-aligned, and S % align == 0
//   makes it `align`-aligned. The 16-byte header occupies the gap.
// - A slab page is an order-0 block drawn from the buddy RAW (under the
//   buddy lock, never through GlobalAlloc::alloc — no recursion).
//   16-byte in-page header; objects from offset 16.
// - Free routing: one static bit per heap page (`SLAB_PAGE_BITMAP`)
//   marks slab pages. `GlobalAlloc::dealloc` checks the bit first;
//   slab frees never reach the buddy free path. The bit is cleared
//   BEFORE an empty page returns to the buddy, so a stale pointer into
//   a reclaimed page falls through to the buddy tripwires (loud)
//   instead of corrupting the slab.
// - Per-class spinlock (irqsave). Active-page lists are singly linked
// through the page headers (page-index linkage, 0 = end). A page that
// fills up is unlinked immediately (the scan carries its prev); an
// EMPTY page is physically unlinked (walk from head) and returned to
// the buddy while still under the class lock — no other CPU can be
// scanning it at that moment, so there is no use-after-reclaim race.
// - Per-CPU magazines (pa16) front the class locks: alloc pops / free
// pushes run under LOCAL preempt-off only (106th: irq-off became
// preempt-off — interrupt context is routed away from the rows, so
// the fast paths keep IRQs enabled); the class lock is taken once
// per batch refill/flush (see the magazine block comment below).
//
// Lock order: class lock -> buddy lock (page create/reclaim). The
// reverse nesting never occurs (the buddy knows nothing of classes).

// (INV-SMALLALLOC-1: the small allocator family — the magazine
// discipline's raw spans (class pages, freelist links inside free
// blocks) are owned by the class lock; the arena base words are
// installed once at init.)

const HEAP_MAX_PAGES: usize = 262144; // 1 GiB window / 4 KiB

/// Maximum layout served by this layer (both size and align).
pub const SMALL_MAX: usize = 1024;

/// Classes: 8, 16, 32, 64, 128, 256, 512, 1024.
const CLASSES: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 1024];

const STATE_ACTIVE: u8 = 1; // has free objects, linked into the active list
const STATE_FULL: u8 = 2; // no free objects (or being reclaimed), unlinked

const HDR_MAGIC: u16 = 0x5142;


#[inline]
fn hdr_of(page: usize) -> &'static mut PageHdr {
    // Callers hold the class lock (or are initializing a page no other
    // CPU can reach yet — still under the class lock) — INV-MM-13.
    super::r#unsafe::sa_hdr_mut(page)
}

/// Class index for a layout already known to fit (size,align <= 1024).
/// First object offset in a class-S page: a multiple of S that also
/// clears the 16-byte header.
#[inline]
fn first_off(s: usize) -> usize {
    if s < 16 { 16 } else { s }
}

#[inline]
fn class_of(size: usize, align: usize) -> usize {
    let need = if size > align { size } else { align };
    let mut idx = 0usize;
    while CLASSES[idx] < need {
        idx += 1;
    }
    idx
}

/// The effective allocated size of a layout this layer would serve:
/// its size class (kmalloc-8 .. kmalloc-1024 — the same bin family the
/// reference's kmalloc caches use). The kmem:kmalloc tracepoint's
/// bytes_alloc column; read-only arithmetic over the same table
/// class_of uses.
pub fn effective_size(size: usize, align: usize) -> usize {
    if size == 0 || size > SMALL_MAX || align > SMALL_MAX {
        return size.max(align);
    }
    CLASSES[class_of(size, align)]
}

// ---------------------------------------------------------------------------
// Per-heap-page "is a slab page" bitmap: 1 bit per page of the max 1 GiB
// heap window = 32 KiB of .bss. Indexed by (addr - heap_start) / 4096.
// ---------------------------------------------------------------------------
const BMP_WORDS: usize = HEAP_MAX_PAGES / 64;
static SLAB_PAGE_BITMAP: [AtomicU64; BMP_WORDS] = [const { AtomicU64::new(0) }; BMP_WORDS];

#[inline]
fn bmp_set(page: usize, heap_start: usize) {
    let idx = (page - heap_start) / PAGE_SIZE;
    SLAB_PAGE_BITMAP[idx / 64].fetch_or(1u64 << (idx % 64), Ordering::AcqRel);
}

#[inline]
fn bmp_clear(page: usize, heap_start: usize) {
    let idx = (page - heap_start) / PAGE_SIZE;
    SLAB_PAGE_BITMAP[idx / 64].fetch_and(!(1u64 << (idx % 64)), Ordering::AcqRel);
}

/// Fast routing check used by the dealloc path — no locks taken.
#[inline]
pub fn is_slab_ptr(addr: usize, heap_start: usize) -> bool {
    let idx = (addr - heap_start) / PAGE_SIZE;
    if idx >= HEAP_MAX_PAGES {
        return false;
    }
    (SLAB_PAGE_BITMAP[idx / 64].load(Ordering::Acquire) >> (idx % 64)) & 1 == 1
}

/// oh-crash 91st (holder hunt): map an arbitrary kernel-heap address to
/// its containing small-object allocation, if any. Returns
/// `(object_base, class_size)` when `addr` lies inside a live slab page
/// (the in-page header names the class; the object base follows from the
/// page's geometry). Read-only over the page header and the slab bitmap,
/// no locks, safe from any context — built for forensic prints (the
/// page may be concurrently mutating; a torn read prints a torn span,
/// acceptable for a diagnostic).
pub fn object_span(addr: usize) -> Option<(usize, usize)> {
    let (heap_start, _heap_end) = super::hotpath::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 || addr < heap_start {
        return None;
    }
    if !is_slab_ptr(addr, heap_start) {
        return None;
    }
    let page = addr & !(PAGE_SIZE - 1);
    // SAFETY: read-only access to a slab page's in-page header; the
    // page lies inside the kernel heap direct map (bitmap bit set) so
    // the mapping is valid. Fields are plain u8/u16 loads — worst case
    // a torn value, filtered by the magic check below.
    let hdr = super::r#unsafe::sa_hdr_ref(page);
    if hdr.magic != HDR_MAGIC {
        return None;
    }
    let cls = (hdr.cls as usize).min(CLASSES.len() - 1);
    let s = CLASSES[cls];
    let off = addr - page;
    if off < first_off(s) {
        return None; // inside the header gap, not an object
    }
    let obj_idx = (off - first_off(s)) / s;
    let base = page + first_off(s) + obj_idx * s;
    Some((base, s))
}

impl SmallCache {
    const fn new() -> Self {
        SmallCache {
            lock: Spinlock::new(()),
            head: AtomicUsize::new(0),
            pages_created: AtomicUsize::new(0),
            pages_reclaimed: AtomicUsize::new(0),
            obj_allocs: AtomicUsize::new(0),
            obj_frees: AtomicUsize::new(0),
            lock_holds: AtomicUsize::new(0),
            walk_cycles: AtomicUsize::new(0),
            slow_max_cyc: AtomicU64::new(0),
            slow_over_10ms: AtomicU64::new(0),
        }
    }
}

static CACHES: [SmallCache; 8] = [
    SmallCache::new(), SmallCache::new(), SmallCache::new(), SmallCache::new(),
    SmallCache::new(), SmallCache::new(), SmallCache::new(), SmallCache::new(),
];

// ---------------------------------------------------------------------------
// Per-CPU magazines (pa16, 2026-10-24: the perf line measured the class
// locks at 12.6% of the whole machine — 732M irqsave sections/boot at
// ~250 ns each, the single biggest IF=0 contributor). Fast paths pop /
// push a per-CPU, per-class array of free object pointers under LOCAL
// irq-off only — no lock, no cross-CPU contention, no preempt-count
// traffic; the class lock is taken once per magazine refill/flush, i.e.
// amortized over ~MAG_DEPTH objects.
//
// CPU affinity: cpu_id() is read INSIDE each irq-off window (or inside
// the class irqsave hold, which also disables preemption). With IF=0
// neither an IRQ nor a preempting tick can migrate the task between the
// read and the row access; no pointer to a row is ever held across two
// windows. The flush path snapshots the whole row out under ONE window
// and then works on the local copy.
//
// Correctness of deferred frees (a magazine-resident object is NOT yet
// reflected in its page header's free_count):
// - Reclaim needs free_count == objs_per_page UNDER the class lock; a
//   magazine entry for page P holds one of P's slots un-accounted, so
//   free_count can never reach objs_per_page while such an entry
//   exists — reclaim is blocked by construction and the slab bitmap
//   stays set. This also holds inside a batch flush: a later entry on
//   the same page keeps that page's free_count short of full, so no
//   entry's page is reclaimed mid-batch while a same-page entry is
//   still pending.
// - Push-time screening: the fast push accepts an address only after
//   re-reading the page header inside the irq-off window (live magic,
//   matching class, in-range aligned slot). Anything doubtful —
//   misaligned/foreign pointers, full-looking pages, header garbage —
//   is routed to the locked body, which consumes and taints it exactly
//   as the pre-magazine allocator did. A bad pointer therefore NEVER
//   parks in a magazine (a later alloc pop would hand it out; the
//   consume-and-taint hardening is load-bearing for the wild-write
//   hunt line).
// - Double frees of a VALID slot corrupt the page freelist exactly as
//   they did before (silently for partially-free pages, taint at the
//   full-page boundary); the magazine only defers the moment, it adds
//   no new failure mode.
// - Touching a magazine from IRQ context (106th restructure): rows
//   are ONLY ever touched in task context inside preempt-off windows
//   (IRQs enabled). Interrupt-context allocs/frees are ROUTED to the
//   class-lock bodies before any row access, so an IRQ can interrupt
//   a row access but never race one; the preempt window forbids the
//   migration that would touch a foreign row.
// - Counters (obj_allocs/obj_frees) are bumped ONLY in the locked
//   bodies — at page-freelist removal/insertion — so every object is
//   counted exactly once per lifecycle; magazine-resident objects skew
//   the live estimate by at most MAX_CPUS*8*MAG_DEPTH (bounded).
// ---------------------------------------------------------------------------

// D-7 (rust-design-headroom §3): the magazine bank is the first
// CpuLocal conversion. The raw `static mut` + review-enforced "caller
// must have IF=0" discipline (the r8final and 7a6e1bf4 incident
// family) is now type-enforced: rows are reachable only through
// CpuLocal::with, whose guard exists exactly inside the closure and
// restores the saved IRQ state at drop. The r8final len/MAG_DEPTH
// guards stay — they defend against row CORRUPTION, which the type
// cannot prevent — but the aliasing-by-construction hazard is gone.
static MAGAZINES: crate::sync::percpu::CpuLocal<[Magazine; 8], { crate::config::MAX_CPUS }> =
    crate::sync::percpu::CpuLocal::new([Magazine { buf: [0; MAG_DEPTH], len: 0 }; 8]);

/// r8final forensics: times a magazine row was found with len >
/// MAG_DEPTH (transient two-CPU entry via a cpu_id()/ti_cpu divergence,
/// or a wild write into the row) and healed by the bounded drain. Zero
/// on a healthy boot; every nonzero value names a real divergence event
/// for the scheduler/wild-write hunt lanes. Printed by census_line().
static MAG_ALIAS_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Allocate a small object. Returns null when the layout does not fit
/// this layer OR the backing page could not be created — the caller
/// falls through to the buddy either way.
pub fn alloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 || size > SMALL_MAX || align > SMALL_MAX {
        return core::ptr::null_mut();
    }
    let (heap_start, _heap_end) = super::hotpath::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 {
        return core::ptr::null_mut(); // heap not up yet
    }
    let ci = class_of(size, align);

    // Interrupt context NEVER touches a magazine row (106th
    // restructure): rows are only ever mutated inside task-context
    // preempt-off windows whose IRQs stay ENABLED, so an IRQ-context
    // alloc that popped a row would race an interrupted task-context
    // access on the same cpu. The class lock (irqsave) serializes
    // everything the locked body touches, and the refill below only
    // fills the CURRENT cpu's row — an irq-context refill would write
    // the interrupted task's row concurrently with its own pop. So the
    // irq-context arm serves ONE object under the lock and returns,
    // no row interaction at all.
    if crate::interrupt::preempt::in_interrupt() {
        let cache = &CACHES[ci];
        let _g = cache.lock.lock_irqsave();
        let _aw = crate::debug::allocwatch::guard(
            crate::debug::allocwatch::SITE_SMALL_ALLOC,
            ci as u8,
        );
        let p = alloc_locked(ci, heap_start);
        drop(_aw);
        drop(_g);
        cache.lock_holds.fetch_add(1, Ordering::Relaxed);
        return p;
    }

    // Fast path: pop from THIS CPU's magazine. The preempt guard (106th)
    // disables preemption for exactly the closure body and reads cpu_id
    // inside the window — the task cannot be migrated between the read
    // and the row access, while IRQs stay enabled (interrupt-context
    // allocs are routed above, so an IRQ can interrupt but never race
    // the row access). Before the 106th this was a CpuLocal::with
    // irq-off window: ~46M cli/sti pairs per boot on the fast paths
    // alone, a top deferred-PMI (`enable_irq+0x1d`) sample producer.
    //
    // r8final guard (2026-10-25): runs/gnome-r8final died at t=515s in
    // "index out of bounds: the len is 16 but the index is 16" here and
    // in the free() snapshot loop. len > MAG_DEPTH means the row was
    // entered by two CPUs at once (a transient cpu_id()/ti_cpu divergence
    // — the R55/R74 scheduler hunt family; every in-tree steer is
    // gated but residual windows remain), and the racing
    // check-then-increment pushes both ran with len == 15, leaving len
    // == 17 with no out-of-bounds write at push time. A pop at 17 would
    // read buf[16], one past the row. Survive it: skip the fast path
    // (the locked body still serves), and let free()'s bounded drain
    // heal the row. Counted + tainted there, once per heal.
    if let Some(ptr) = MAGAZINES
        .with_preempt(|mut row| {
            let mag = &mut row[ci];
            (mag.len > 0 && mag.len <= MAG_DEPTH).then(|| {
                mag.len -= 1;
                mag.buf[mag.len]
            })
        })
        .flatten()
    {
        return ptr as *mut u8;
    }

    // Slow path: ONE class-lock hold refills the magazine in batch and
    // returns the first object. The irqsave guard has IF=0 + preemption
    // disabled; the nested magazine access below uses the preempt form
    // (preempt_count nesting is plain arithmetic under the already-IF=0
    // section) and its re-entry tripwire is vacuous here — no outer
    // window is live (the fast-path guard closed before we locked).
    let cache = &CACHES[ci];
    let mut first: *mut u8 = core::ptr::null_mut();
    let _g = cache.lock.lock_irqsave();
    // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (frozen-holder naming; the
    // holder's own tick is dead at IF=0 — see debug/allocwatch.rs) + the
    // section-duration meter (lb268: one bounded section held 300 ms of
    // real KVM execution — name the cost class).
    let _aw = crate::debug::allocwatch::guard(
        crate::debug::allocwatch::SITE_SMALL_ALLOC,
        ci as u8,
    );
    let _t0 = crate::arch::cpu::read_time();
    let mut refill = |mag: &mut Magazine| -> bool {
        while mag.len < MAG_DEPTH {
            let p = alloc_locked(ci, heap_start);
            if p.is_null() {
                break;
            }
            if first.is_null() {
                first = p;
            } else {
                mag.buf[mag.len] = p as usize;
                mag.len += 1;
            }
        }
        true
    };
    if MAGAZINES.with_preempt(|mut row| refill(&mut row[ci])).is_none() {
        // Interrupt context (routed here by the entry check's race
        // window — an IRQ hit between the check and the lock) or cpu id
        // out of bank range (divergence family): serve one object
        // directly under the lock, no row touched.
        first = alloc_locked(ci, heap_start);
    }
    // rux:allocwatch (retired debug=allocwatch) section meter (see _t0 above). lock_holds is
    // counted AFTER the guard drops (106th): it is a shared-line RMW —
    // inside the irqsave window it also extended every section's IF=0
    // span; the count itself only needs the section to have happened.
    drop(_aw);
    drop(_g);
    cache.lock_holds.fetch_add(1, Ordering::Relaxed);
    if crate::debug::allocwatch::armed() {
        let d = crate::arch::cpu::read_time().wrapping_sub(_t0);
        let prev = cache.slow_max_cyc.load(Ordering::Relaxed);
        if d > prev {
            cache.slow_max_cyc.store(d, Ordering::Relaxed);
        }
        if d > 25_000_000 {
            cache.slow_over_10ms.fetch_add(1, Ordering::Relaxed);
        }
    }
    first
}

/// The pre-magazine allocation body. Caller holds the class lock.
fn alloc_locked(ci: usize, heap_start: usize) -> *mut u8 {
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs_per_page: u16 = ((PAGE_SIZE - fo) / s) as u16;

    // 1. Serve from an active page; lazy-skip dead heads.
    //
    // WALK BOUND (the OH allocator-freeze, lb260/lb261): the unlink-skip
    // below re-links around dead pages, but a page-list CYCLE (wild-write
    // corruption: A->B->A with both dead) revisits forever — head=B,
    // then head=B.next=A, then head=A.next=B ... every iteration under
    // the class lock at IF=0, so the holder's own tick dies and the
    // whole machine freezes (os_ds_service t548 held small_alloc class
    // locks for the rest of the boot on both boots; ALLOC-HOLD named
    // it). A legit class list is bounded by the heap's page count — cap
    // the walk at 8192 pages (no legitimate single-class list approaches
    // that); on cap, taint loudly and fall through to a FRESH page
    // (path 2): the corrupt cycle is leaked like every other corrupt
    // page here — "loud beats corrupt", and a leaked cycle beats a
    // frozen machine.
    let mut walked: u32 = 0;
    let mut prev: u32 = 0; // page index of the previous live link point (0 = head)
    let mut cur = cache.head.load(Ordering::Relaxed) as u32;
    while cur != 0 {
        walked += 1;
        if walked > 8192 {
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            cache.walk_cycles.fetch_add(1, Ordering::Relaxed);
            break;
        }
        let page_addr = heap_start + cur as usize * PAGE_SIZE;
        let hdr = hdr_of(page_addr);
        if hdr.magic != HDR_MAGIC || hdr.state != STATE_ACTIVE || hdr.free_count == 0 {
            let next = hdr.next_page;
            if prev == 0 {
                cache.head.store(next as usize, Ordering::Relaxed);
            } else {
                hdr_of(heap_start + prev as usize * PAGE_SIZE).next_page = next;
            }
            cur = next;
            continue;
        }
        let obj = hdr.free_head as usize;
        if obj >= objs_per_page as usize {
            // Corrupt freelist: retire the page (leak it — loud beats corrupt).
            hdr.state = STATE_FULL;
            let next = hdr.next_page;
            if prev == 0 {
                cache.head.store(next as usize, Ordering::Relaxed);
            } else {
                hdr_of(heap_start + prev as usize * PAGE_SIZE).next_page = next;
            }
            cur = next;
            continue;
        }
        let ptr = page_addr + fo + obj * s;
        let next_free = super::r#unsafe::sa_read_u16(ptr) as usize;
        hdr.free_head = if next_free >= objs_per_page as usize { 0xFFFF } else { next_free as u16 };
        hdr.free_count -= 1;
        if hdr.free_count == 0 {
            hdr.state = STATE_FULL;
            let next = hdr.next_page;
            if prev == 0 {
                cache.head.store(next as usize, Ordering::Relaxed);
            } else {
                hdr_of(heap_start + prev as usize * PAGE_SIZE).next_page = next;
            }
        }
        cache.obj_allocs.fetch_add(1, Ordering::Relaxed);
        return ptr as *mut u8;
    }

    // 2. Create a fresh page from the buddy (raw — no GlobalAlloc recursion).
    let page = super::hotpath::GLOBAL_ALLOCATOR.alloc_page_raw() as usize;
    if page == 0 {
        return core::ptr::null_mut();
    }
    bmp_set(page, heap_start);
    let pi = ((page - heap_start) / PAGE_SIZE) as u32;
    let hdr = hdr_of(page);
    hdr.magic = HDR_MAGIC;
    hdr.cls = ci as u8;
    hdr.state = STATE_ACTIVE;
    hdr.free_head = 0;
    hdr.free_count = objs_per_page;
    hdr.next_page = cache.head.load(Ordering::Relaxed) as u32;
    cache.head.store(pi as usize, Ordering::Relaxed);
    cache.pages_created.fetch_add(1, Ordering::Relaxed);
    // Freelist chain through the free objects' first u16.
    let mut i = 0usize;
    while i < objs_per_page as usize {
        let val: u16 = if i + 1 >= objs_per_page as usize { 0xFFFF } else { (i + 1) as u16 };
        super::r#unsafe::sa_write_u16(page + fo + i * s, val);
        i += 1;
    }
    // Serve object 0.
    let ptr = page + fo;
    let hdr = hdr_of(page);
    let next_free = super::r#unsafe::sa_read_u16(ptr) as usize;
    hdr.free_head = if next_free >= objs_per_page as usize { 0xFFFF } else { next_free as u16 };
    hdr.free_count -= 1;
    if hdr.free_count == 0 {
        hdr.state = STATE_FULL;
        cache.head.store(hdr.next_page as usize, Ordering::Relaxed);
    }
    cache.obj_allocs.fetch_add(1, Ordering::Relaxed);
    ptr as *mut u8
}

/// Free a small object. Returns false ONLY when the page is not a slab
/// page (the caller then falls through to the buddy free path). Once
/// the bitmap says "slab page", the free is CONSUMED here no matter what:
/// the page holds up to 510 other objects, so a foreign or stale pointer
/// must never reach the buddy (whose page-granular free would hand the
/// live slab page to a second owner). Invalid frees taint and count.
pub fn free(addr: usize) -> bool {
    let (heap_start, heap_end) = super::hotpath::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 || addr < heap_start || addr >= heap_end {
        return false;
    }
    if !is_slab_ptr(addr, heap_start) {
        return false;
    }
    let page = addr & !(PAGE_SIZE - 1);
    // Pre-lock peek only to pick the class/magazine bin; everything is
    // re-validated under the lock at flush time (the page may have been
    // emptied, reclaimed and re-created as a different class between
    // the reads — the re-check catches that).
    let peek = hdr_of(page);
    let ci_guess = peek.cls as usize;
    if ci_guess >= CLASSES.len() {
        // Header garbage (transient or corrupt): consume, taint.
        crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
        return true;
    }

    // Interrupt context never touches a row (106th restructure — see
    // alloc()'s routing comment): the screen-then-push below runs in a
    // preempt-off window with IRQs ENABLED, so an irq-context push
    // would race an interrupted task-context access on the same cpu.
    // Flush this one entry directly through the locked body.
    if crate::interrupt::preempt::in_interrupt() {
        flush_batch(core::slice::from_ref(&addr), ci_guess, heap_start);
        return true;
    }

    // Fast path: screen the pointer against the page header INSIDE the
    // CpuLocal preempt window (106th: preempt-off, IRQs enabled — the
    // fresh magic/cls read, alignment and free_count head room are the
    // same advisory screen as before; the flush re-validates under the
    // class lock), then push into THIS CPU's magazine. Anything the
    // locked body would consume-and-taint is routed there instead — a
    // bad pointer must never park in a magazine, because a later alloc
    // pop on this CPU would hand it out as a live object.
    enum Step {
        Pushed,
        SnapFlush { snap: [usize; MAG_DEPTH], n: usize },
        Locked,
    }
    let step = MAGAZINES
        .with_preempt(|mut row| {
            let s = CLASSES[ci_guess];
            let fo = first_off(s);
            let objs_per_page = (PAGE_SIZE - fo) / s;
            let off = addr - page - fo;
            let h = hdr_of(page); // re-read under IF=0: another CPU may have
            // changed the header since the peek; the flush re-validates
            // under the class lock regardless.
            let screen = h.magic == HDR_MAGIC
                && h.cls as usize == ci_guess
                && addr != page
                && off % s == 0
                && off / s < objs_per_page
                && (h.free_count as usize) < objs_per_page;
            if !screen {
                return Step::Locked;
            }
            let mag = &mut row[ci_guess];
            if mag.len < MAG_DEPTH {
                mag.buf[mag.len] = addr;
                mag.len += 1;
                return Step::Pushed;
            }
            // Magazine full: snapshot the whole row out under THIS
            // window (the guard ends with the closure — no row pointer
            // survives it), then flush the snapshot and push addr
            // after — the batch flush below empties the row, so the
            // push always has room.
            //
            // r8final guard (2026-10-25): the drain is bounded on BOTH
            // sides. A row raced to len == MAG_DEPTH+1 (two pushes that
            // both saw len == 15 — see the alloc() fast-path comment)
            // would otherwise read buf[MAG_DEPTH] here and panic the
            // kernel; a wild-written len (any value) would read past
            // the row outright. Indices >= MAG_DEPTH are fictitious
            // entries that were never written by any push (a push
            // would have panicked at its own store first) — dropping
            // them is always safe, and flush_batch re-validates every
            // surviving entry against the page header anyway. The heal
            // is counted and tainted so the divergence is named, not
            // silently absorbed.
            if mag.len > 2 * MAG_DEPTH {
                // A racing double-entry adds single increments past the
                // cap; anything beyond 2x the depth is a scribble, not a
                // race product. Draining it one decrement at a time under
                // IF=0 would stall the CPU for billions of iterations —
                // drop the row wholesale instead. Up to MAG_DEPTH pending
                // frees leak (bounded); every surviving entry the row ever
                // hands out is page-header-validated at flush time.
                mag.len = 0;
                MAG_ALIAS_EVENTS.fetch_add(1, Ordering::Relaxed);
                crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            }
            let mut snap: [usize; MAG_DEPTH] = [0; MAG_DEPTH];
            let mut n = 0usize;
            let mut dropped = 0usize;
            while mag.len > 0 && n < MAG_DEPTH {
                mag.len -= 1;
                if mag.len < MAG_DEPTH {
                    snap[n] = mag.buf[mag.len];
                    n += 1;
                } else {
                    dropped += 1;
                }
            }
            if dropped > 0 {
                MAG_ALIAS_EVENTS.fetch_add(1, Ordering::Relaxed);
                crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            }
            Step::SnapFlush { snap, n }
        })
        .unwrap_or(Step::Locked); // cpu id out of bank range or refused re-entry
    match step {
        Step::Pushed => return true,
        Step::SnapFlush { snap, n } => {
            flush_batch(&snap[..n], ci_guess, heap_start);
            // Push addr into the (now empty) row. A FRESH preempt
            // window reads cpu_id again: the task may have been
            // preempted and migrated during the flush, and must not
            // touch a stale row. An interrupt-context free that ran
            // meanwhile is routed to flush_batch (never a row), so the
            // row can only hold FEWER than MAG_DEPTH entries — the
            // check still guards; the locked fallback covers the
            // impossible rest.
            let pushed = MAGAZINES
                .with_preempt(|mut row| {
                    let mag = &mut row[ci_guess];
                    (mag.len < MAG_DEPTH).then(|| {
                        mag.buf[mag.len] = addr;
                        mag.len += 1;
                    })
                })
                .unwrap_or(None);
            if pushed.is_none() {
                flush_batch(core::slice::from_ref(&addr), ci_guess, heap_start);
            }
            return true;
        }
        Step::Locked => {}
    }
    // Screen failed or cpu id out of range: the pre-magazine locked
    // body (consume-and-taint semantics preserved exactly).
    flush_batch(core::slice::from_ref(&addr), ci_guess, heap_start);
    true
}

/// The pre-magazine free body (validation + push to page freelist +
/// empty-page reclaim), run for a BATCH of same-class entries under ONE
/// class-lock hold (the snapshot's per-entry locking made the flush as
/// lock-heavy as the pre-magazine path; the batch is the actual
/// amortization). Empty pages are unlinked and marked reclaimed under
/// the lock, then returned to the buddy AFTER the guard drops — the
/// buddy free never runs inside the class lock (pre-magazine behavior).
/// Every entry is CONSUMED by the slab layer no matter what (returns
/// nothing, always accounted); invalid entries taint.
fn flush_batch(entries: &[usize], ci: usize, heap_start: usize) {
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs_per_page = (PAGE_SIZE - fo) / s;
    let mut ret: [usize; MAG_DEPTH + 1] = [0; MAG_DEPTH + 1]; // pages to buddy-free after unlock
    let mut nret = 0usize;
    let _g = cache.lock.lock_irqsave();
    // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
    let _aw = crate::debug::allocwatch::guard(
        crate::debug::allocwatch::SITE_SMALL_ALLOC,
        ci as u8,
    );
    for &addr in entries {
        let page = addr & !(PAGE_SIZE - 1);
        let hdr = hdr_of(page);
        if hdr.magic != HDR_MAGIC || hdr.cls as usize != ci {
            // Reclaimed/re-created under us — no valid object can exist.
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            continue;
        }
        let off = addr - page - fo;
        if addr == page || off % s != 0 || off / s >= objs_per_page {
            // Foreign/misaligned pointer into a live slab page: consume
            // it — the buddy free would destroy the page's 500-odd
            // neighbours.
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            continue;
        }
        if hdr.free_count as usize >= objs_per_page {
            // Double free — refuse, but consume it (do not also
            // buddy-free).
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            continue;
        }
        super::r#unsafe::sa_write_u16(addr, hdr.free_head);
        hdr.free_head = (off / s) as u16;
        hdr.free_count += 1;
        cache.obj_frees.fetch_add(1, Ordering::Relaxed);

        if hdr.free_count as usize == objs_per_page {
            // Page empty: physically unlink (walk from head) and mark it
            // reclaimed under the class lock, so no other CPU can be
            // mid-scan on this page; the buddy free itself runs after
            // the unlock below. A later batch entry on this page cannot
            // exist here — its slot would be un-accounted, so the page
            // could not have reached free_count == objs_per_page.
            let pi = ((page - heap_start) / PAGE_SIZE) as u32;
            let mut prev: u32 = 0;
            let mut cur = cache.head.load(Ordering::Relaxed) as u32;
            // Cycle cap (see alloc_locked's WALK BOUND — a corrupted
            // list must never loop under the class lock at IF=0).
            let mut steps: u32 = 0;
            while cur != 0 {
                steps += 1;
                if steps > 8192 {
                    crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
                    cache.walk_cycles.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                let a = heap_start + cur as usize * PAGE_SIZE;
                if cur == pi {
                    let next = hdr_of(a).next_page;
                    if prev == 0 {
                        cache.head.store(next as usize, Ordering::Relaxed);
                    } else {
                        hdr_of(heap_start + prev as usize * PAGE_SIZE).next_page = next;
                    }
                    break;
                }
                prev = cur;
                cur = hdr_of(a).next_page;
            }
            hdr.state = STATE_FULL;
            hdr.magic = 0;
            bmp_clear(page, heap_start);
            cache.pages_reclaimed.fetch_add(1, Ordering::Relaxed);
            if nret < ret.len() {
                ret[nret] = page;
                nret += 1;
            }
            continue;
        }
        if hdr.state == STATE_FULL {
            // Became partially free again: relink at the list head.
            hdr.state = STATE_ACTIVE;
            let pi = ((page - heap_start) / PAGE_SIZE) as u32;
            hdr.next_page = cache.head.load(Ordering::Relaxed) as u32;
            cache.head.store(pi as usize, Ordering::Relaxed);
        }
    }
    drop(_aw);
    drop(_g);
    // lock_holds AFTER the window (106th): shared-line RMW out of the
    // IF=0 section — see alloc()'s slow path.
    cache.lock_holds.fetch_add(1, Ordering::Relaxed);
    let mut i = 0usize;
    while i < nret {
        super::hotpath::GLOBAL_ALLOCATOR.free_page_raw(ret[i] as *mut u8);
        i += 1;
    }
}

/// Leg-16 kernel-heap anatomy: (live small-object bytes, slab pages
/// actually held) in KiB, from the forensic counters — the difference
/// between the two is the packing slack. Object bytes round every live
/// object up to its class size and ignore magazine residency (bounded
/// skew of MAX_CPUS*8*MAG_DEPTH entries); page bytes are exact.
pub fn live_stats_kib() -> (u64, u64) {
    let mut obj_bytes: u64 = 0;
    let mut pages: u64 = 0;
    for ci in 0..8 {
        let c = &CACHES[ci];
        let a = c.obj_allocs.load(Ordering::Relaxed) as i64;
        let f = c.obj_frees.load(Ordering::Relaxed) as i64;
        let live = (a - f).max(0) as u64;
        obj_bytes += live * CLASSES[ci] as u64;
        pages += c
            .pages_created
            .load(Ordering::Relaxed)
            .saturating_sub(c.pages_reclaimed.load(Ordering::Relaxed)) as u64;
    }
    (obj_bytes / 1024, pages * 4)
}

/// Leg-16 census peek: dump the first two qwords of up to `per_page`
/// objects from each active page of class `ci` (raw UART, allocation-
/// free). Object headers carry vtables/magic/locks that identify the
/// owning type — the census counts say WHICH CLASS holds the heap, this
/// says WHICH TYPE fills the class. Caller must not hold the class lock.
/// rux:allocwatch (retired debug=allocwatch) probe: per-class slow-path section durations (max ms
/// and count of sections >10 ms) — separates a genuinely long section
/// from a stale stamp. Reset on read.
pub fn audit_section_peek() -> [(u64, u64); 8] {
    let mut out = [(0u64, 0u64); 8];
    for (i, c) in CACHES.iter().enumerate() {
        out[i] = (
            c.slow_max_cyc.load(Ordering::Relaxed),
            c.slow_over_10ms.load(Ordering::Relaxed),
        );
    }
    out
}

/// rux:allocwatch (retired debug=allocwatch) probe: live lock-word state of every class lock
/// (bit i = CACHES[i] held at sample time). A sample is not a snapshot;
/// the caller filters transients (see debug/allocwatch.rs STUCK detection
/// — a bit set at every tick for >1 s with no matching holder stamp is
/// a stuck/corrupt lock word, the wild-write producer's freeze face).
pub fn audit_class_locks() -> u8 {
    let mut m = 0u8;
    for (i, c) in CACHES.iter().enumerate() {
        if c.lock.is_locked() {
            m |= 1 << i;
        }
    }
    m
}

pub fn census_peek(ci: usize, per_page: usize) {
    use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
    if ci >= CLASSES.len() {
        return;
    }
    let (heap_start, _heap_end) =
        super::hotpath::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 {
        return;
    }
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs = (PAGE_SIZE - fo) / s;
    // Header HISTOGRAM over up to 32 active pages (the first run's
    // 8x8 sample hit mostly-free head pages): count DISTINCT first-qword
    // values — a handful of dominant values = the type population.
    // Buffer-then-print (leg rule: no UART under locks).
    let mut vals: [u64; 24] = [0; 24];
    let mut counts: [u64; 24] = [0; 24];
    let mut nvals = 0usize;
    let mut seen_total: u64 = 0;
    let _g = cache.lock.lock_irqsave();
    // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
    let _aw = crate::debug::allocwatch::guard(
        crate::debug::allocwatch::SITE_SMALL_ALLOC,
        ci as u8,
    );
    let mut page = cache.head.load(Ordering::Relaxed) as u32;
    let mut pages = 0usize;
    while page != 0 && pages < 32 {
        let page_addr = heap_start + page as usize * PAGE_SIZE;
        let hdr = hdr_of(page_addr);
        if hdr.magic != HDR_MAGIC || hdr.state != STATE_ACTIVE {
            break;
        }
        // Sample from the TOP of the page (most recently allocated
        // slots sit past the freelist churn) and only slots that look
        // populated.
        let mut i = 0usize;
        while i < objs && i < per_page.max(16) {
            // SAFETY: live slab page of this class; slot in bounds.
            let p = page_addr + fo + i * s;
            let a0 = super::r#unsafe::sa_read_u64(p);
            if a0 != 0 {
                let mut k = 0usize;
                while k < nvals {
                    if vals[k] == a0 {
                        counts[k] += 1;
                        break;
                    }
                    k += 1;
                }
                if k == nvals && nvals < vals.len() {
                    vals[nvals] = a0;
                    counts[nvals] = 1;
                    nvals += 1;
                }
                seen_total += 1;
            }
            i += 1;
        }
        page = hdr.next_page;
        pages += 1;
    }
    drop(_g);
    taskdump_raw_line(b"PEEK c");
    taskdump_dec(s as u64);
    taskdump_raw_line(b" sampled=");
    taskdump_dec(seen_total);
    taskdump_raw_line(b" distinct=");
    taskdump_dec(nvals as u64);
    taskdump_raw_line(b"\n");
    let mut k = 0usize;
    while k < nvals {
        taskdump_raw_line(b"  hdr ");
        taskdump_dec(vals[k]);
        taskdump_raw_line(b" x");
        taskdump_dec(counts[k]);
        taskdump_raw_line(b"\n");
        k += 1;
    }
}

/// Leg-17 census peek v3: STRUCTURAL classification of class `ci`
/// slots. The v2 histogram showed the c1024 population is pointer-first
/// heterogeneous (every first qword unique), so value histograms cannot
/// name it — this buckets slots by layout signatures instead (raw UART,
/// allocation-free, count-under-lock then print):
///   mm-arc    : a0<=4096 (strong) && a1<=4096 (weak) && a2<2^24 (pgd)
///               — ArcInner<MmStruct> { strong, weak, MmStruct{pgd..} }
///   vma-node  : (a0==0 || kheap) && u16 len@+10 in 1..=11 && userva
///               at +16 — BTree node { parent, parent_idx, len, keys }
///               with VirtAddr keys (VmaManager tree)
///   work-tag  : high-32 of a1 == 0x4b524f57 ("WORK" — the dominant
///               c256 record family from the sc238 census)
///   userva/kheap/small-first : first-qword range buckets
pub fn census_peek3(ci: usize) {
    use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
    if ci >= CLASSES.len() {
        return;
    }
    let (heap_start, _heap_end) =
        super::hotpath::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 {
        return;
    }
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs = (PAGE_SIZE - fo) / s;

    #[inline]
    fn is_kheap(a: u64) -> bool {
        // x86_64 linear map (PAGE_OFFSET 0xffff8880_00000000). The
        // constants hold for the launcher-line target; other arches
        // simply classify fewer slots.
        a >= 0xffff_8880_0000_0000
    }
    #[inline]
    fn is_userva(a: u64) -> bool {
        (0x0000_7f00_0000_0000..0x0000_8000_0000_0000).contains(&a)
    }

    let (mut n_arc, mut n_vma, mut n_work) = (0u64, 0u64, 0u64);
    let (mut n_uv, mut n_kh, mut n_sm, mut n_ot) = (0u64, 0u64, 0u64, 0u64);
    let mut total: u64 = 0;
    let _g = cache.lock.lock_irqsave();
    // rux:allocwatch (retired debug=allocwatch): cross-CPU holder stamp (see debug/allocwatch.rs).
    let _aw = crate::debug::allocwatch::guard(
        crate::debug::allocwatch::SITE_SMALL_ALLOC,
        ci as u8,
    );
    let mut page = cache.head.load(Ordering::Relaxed) as u32;
    let mut pages = 0usize;
    while page != 0 && pages < 32 {
        let page_addr = heap_start + page as usize * PAGE_SIZE;
        let hdr = hdr_of(page_addr);
        if hdr.magic != HDR_MAGIC || hdr.state != STATE_ACTIVE {
            break;
        }
        let mut i = 0usize;
        while i < objs {
            // SAFETY: live slab page of this class; slot in bounds.
            let p = page_addr + fo + i * s;
            let a0 = super::r#unsafe::sa_read_u64(p);
            let a1 = super::r#unsafe::sa_read_u64(p + 8);
            let a2 = super::r#unsafe::sa_read_u64(p + 16);
            if a0 == 0 && a1 == 0 && a2 == 0 {
                i += 1;
                continue;
            }
            total += 1;
            // BTree node length: u16 at +10 (parent 8B, parent_idx u16).
            let blen = super::r#unsafe::sa_read_u16(p + 10) as u64;
            if a0 <= 4096 && a1 <= 4096 && a2 != 0 && a2 < (1 << 24) {
                n_arc += 1;
            } else if (a0 == 0 || is_kheap(a0)) && (1..=11).contains(&blen) && is_userva(a2) {
                n_vma += 1;
            } else if (a1 >> 32) == 0x4b52_4f57 {
                n_work += 1;
            } else if is_userva(a0) {
                n_uv += 1;
            } else if is_kheap(a0) {
                n_kh += 1;
            } else if a0 <= 64 {
                n_sm += 1;
            } else {
                n_ot += 1;
            }
            i += 1;
        }
        page = hdr.next_page;
        pages += 1;
    }
    drop(_g);
    taskdump_raw_line(b"PEEK3 c");
    taskdump_dec(s as u64);
    taskdump_raw_line(b": n=");
    taskdump_dec(total);
    taskdump_raw_line(b" mmArc=");
    taskdump_dec(n_arc);
    taskdump_raw_line(b" vmaNode=");
    taskdump_dec(n_vma);
    taskdump_raw_line(b" work=");
    taskdump_dec(n_work);
    taskdump_raw_line(b" uv=");
    taskdump_dec(n_uv);
    taskdump_raw_line(b" kh=");
    taskdump_dec(n_kh);
    taskdump_raw_line(b" small=");
    taskdump_dec(n_sm);
    taskdump_raw_line(b" other=");
    taskdump_dec(n_ot);
    taskdump_raw_line(b"\n");
}

/// One-line census for the ALLOCTHROW dump (raw UART, allocation-free).
pub fn census_line() {
    use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
    taskdump_raw_line(b"SMALLALLOC cls objs-alloc objs-free pages-made pages-reclaimed\n");
    let alias = MAG_ALIAS_EVENTS.load(Ordering::Relaxed);
    if alias > 0 {
        taskdump_raw_line(b" mag-alias-heals=");
        taskdump_dec(alias as u64);
        taskdump_raw_line(b"\n");
    }
    for ci in 0..8 {
        let c = &CACHES[ci];
        let a = c.obj_allocs.load(Ordering::Relaxed);
        let f = c.obj_frees.load(Ordering::Relaxed);
        if a == 0 && f == 0 {
            continue;
        }
        taskdump_raw_line(b" c");
        taskdump_dec(CLASSES[ci] as u64);
        taskdump_raw_line(b" a=");
        taskdump_dec(a as u64);
        taskdump_raw_line(b" f=");
        taskdump_dec(f as u64);
        taskdump_raw_line(b" made=");
        taskdump_dec(c.pages_created.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" recl=");
        taskdump_dec(c.pages_reclaimed.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" locks=");
        taskdump_dec(c.lock_holds.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b"\n");
    }
}


// ════════════════ vdso ════════════════════════════════
// (was vdso.rs)
// vDSO (virtual dynamic shared object) — P2.
//
// FRAMEKERNEL CLASSIFICATION (unsafe-slim L7, 2026-11-21): this file is
// mm's SECOND edge file (with mm/unsafe.rs — the doctrine's ≤2 per
// subsystem), NOT a service file to de-sign. It is one self-contained
// timekeeping primitive — the data-page seqlock, the volatile word
// pairs, and the rdtsc/rdcycle counter asm — whose unsafe is the
// audited core itself (the LTP clock_gettime04 protocol: base_tsc/
// base_mono_ns must pair under the seq word or the vDSO clock runs
// ahead of the syscall clock). Relocating these blocks into unsafe.rs
// would move the same audited bytes without shrinking anything.
//
// INV-MM-VDSO (the invariants this primitive owns):
// - The data page (`VDSO_DATA_PAGE`) is written ONLY by the timer-tick
//   / clock-set refresh paths under the seqlock protocol
//   (seq |= 1 → write fields → seq += 1), and read by user code that
//   validates the seq word before trusting the fields.
// - Word accesses are VOLATILE by contract (fields may be concurrently
//   read by user mappings; coherence is the seq word's job).
// - On x86_64 the page is the UNIFIED timekeeping anchor: `read_time`'s
//   TSC branch and the vDSO functions compute the same value from the
//   same words — the kernel/user bit-identity contract.
// - Counter reads are architecture-ordered (`lfence;rdtsc` on x86_64)
//   so the stamp sits inside the seq window.
//
// A two-page user mapping installed by every execve:
//
// ```text
//   VDSO_BASE        +0    data page  (RW)  kernel-updated time base
//   VDSO_BASE+0x1000 +1    code page  (RX)  minimal ELF with 3 symbols
// ```
//
// `AT_SYSINFO_EHDR` points at the code page; musl/glibc parse the ELF's
// dynamic symbol table and call the functions directly, making
// clock_gettime/gettimeofday/clock_getres user-space fast paths.
//
// Time source: the 10 MHz `time` CSR (100 ns per cycle exactly — no
// mult/shift fixed-point needed, `ns = base_ns + (cycle - base_cycle) *
// 100`). The kernel refreshes {seq, base_cycle, base_ns} from the timer
// tick under a seqlock; the vDSO functions interpolate from the current
// `rdcycle` and retry when the seqlock moved mid-read.
//
// Data page layout (offsets in bytes):
//   0   u32  seq            (odd = write in progress)
//   8   u64  base_cycles    time CSR at the last refresh
//   16  u64  base_mono_ns   monotonic ns at the last refresh
//   24  u64  wall_epoch_ns  REALTIME = monotonic + this (ns)
//   32  u32  tz_minuteswest (always 0: no timezone model)
//   36  u32  tz_dsttime
//
// (x86_64 differs: the page is the UNIFIED timekeeping anchor —
// see the `x86_track` module for the layout and the kernel/user
// bit-identity contract.)
//
// Functions (RV64 code, built below by a mini assembler):
//   __vdso_clock_gettime(clk, struct timespec*)       0 / -ENOSYS
//   __vdso_gettimeofday(struct timeval*, struct tz*)  0
//   __vdso_clock_getres(clk, struct timespec*)        0 / -ENOSYS
//
// Supported clocks: REALTIME(0), MONOTONIC(1), MONOTONIC_RAW(4),
// BOOTTIME(7), TAI(11 = REALTIME + 37). x86_64: every other clockid
// (the COARSE clocks, CPUTIME ids, garbage) falls back INSIDE the vDSO
// to the real syscall — the reference's shape (see mod x86 dispatch).

// (INV-VDSO-DATA-1: the vDSO data page family — the page is a fixed
// static mapping whose seqlock/fields are accessed at page-constant
// offsets (the seq/data readers' contract); the tick/wr rows hold the
// seqlock write discipline.)

// The two page types are defined in mm/types.rs; the pages themselves
// (the statics below) are this primitive's own state.

/// Fixed user mapping address (INTERP_BASE - 4 MiB, clear of the dynamic
/// linker's base).
pub const VDSO_BASE: u64 = 0x3FBE_C000_00;

/// auxv AT_SYSINFO_EHDR value: the code (ELF) page.
pub fn vdso_ehdr() -> u64 {
    VDSO_BASE + 4096
}

// ============================================================================
// Data page
// ============================================================================

static mut VDSO_DATA_PAGE: VdsoDataPage = VdsoDataPage([0; 4096]);

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
unsafe fn vdso_seq() -> &'static AtomicU32 {
    unsafe { &*(VDSO_DATA_PAGE.0.as_ptr() as *const AtomicU32) }
}

/// Refresh the data page snapshot (timer tick / clock-set paths).
///
/// Seqlock protocol: seq |= 1 (begin) → write fields → seq += 1 (end,
/// even). Readers retry while seq is odd or changed across the read.
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
        let base = VDSO_DATA_PAGE.0.as_mut_ptr();
        core::ptr::write_volatile(base.add(8) as *mut u64, cycles);
        core::ptr::write_volatile(base.add(16) as *mut u64, mono_ns);
        core::ptr::write_volatile(base.add(24) as *mut u64, epoch_ns);
        // tz fields (32/36) stay zero.
        seq.fetch_add(1, Ordering::Release);
    }
}

/// x86_64 unified-track timekeeping (vDSO re-arm, 2026-11-15).
///
/// THE ANCHOR LIVES IN THIS PAGE — it is the single timekeeping state
/// for BOTH consumers. `read_time()`'s TSC branch (the clock behind
/// every syscall) and the vDSO functions compute the SAME value from
/// the SAME words with the SAME integer ops:
///
///   nom(t) = base_mono_nom + ((tsc(t) - cycle_last) * mult) >> 32
///
/// read under the seq protocol [seq1 → fields → lfence;rdtsc → seq2,
/// retry on odd/changed], then ×100 to ns. Integer-associative, so the
/// two tracks are BIT-IDENTICAL — this is what LTP clock_gettime04
/// demands: it interleaves libc/vDSO reads with raw-syscall reads
/// 10000x per clock and requires STRICT non-negative adjacent-pair
/// deltas in both directions; adjacent reads can sit <1 µs apart, so
/// any per-track bias (even 100 ns of rounding) eventually pairs
/// negative. The pre-re-arm publication (base sampled from the
/// per-CPU-corrected, ratcheted `read_time()`, read back by a raw-TSC
/// interpolation) carried the TSC_CPU_OFF spread between the publisher
/// and the reader — µs-class steady state, ms-class while APs
/// calibrated (tsleep2: -356 ms at t<1 s; cgt_probe5: syscall-vs-vdso
/// spread to +1.2 ms; clock_gettime04 iso4: -4582 ns residual) — and no
/// user-side fix could remove it: replicating per-CPU offsets via
/// rdtscp+TSC_AUX reproduces the spread, it does not erase it.
///
/// Design consequences:
/// - base_mono_nom is NOMINAL-domain (10 MHz ticks) and carries ALL
///   corrections folded in (boot continuity offset, 1 Hz resync
///   steps). It only ever moves FORWARD: refreshes add the measured
///   TSC delta, the resync adds its clamped step, so the track cannot
///   step back by construction (the kernel's TSC_LAST ratchet stays as
///   anomaly insurance and then never engages on a healthy track).
/// - Per-CPU TSC_CPU_OFF corrections are GONE. They existed for KVM
///   per-vCPU TSC offsets, which under this rig's invariant TSC are
///   synchronized (the sc110-114 "skews" were the shift-12 mult bug);
///   residual per-CPU divergence is measured at AP bringup and
///   reported (>1 ms trips a boot-log line) instead of corrected —
///   a correction spread across CPUs is exactly what the strict
///   adjacent-pair tests cannot tolerate.
/// - The seq word doubles as the WRITER lock: cmpxchg even→odd, a
///   losing writer skips this refresh (next tick catches up), so the
///   fetch_or(1) multi-writer tear of the old publication (two tick
///   CPUs racing both-begin → both-write → seq lands odd) is closed.
///
/// Data page x86 layout (offsets; mult is nominal-domain here, NOT
/// the old ×100 ns-domain value — the emitter multiplies by 100 after
/// the nominal sum, matching `monotonic_time()`'s split exactly):
///   8   u64 cycle_last      raw TSC at the last refresh
///   16  u64 base_mono_nom   nominal-domain mono at cycle_last
///   24  u64 wall_epoch_ns   REALTIME = mono + this (ns)
///   40  u64 mult            TSC→nominal mult (== TSC_MULSHIFT 32;
///                           write-once at promote; 44 stays RESERVED
///                           — the old overlap published mult|(32<<32))
///   48  u32 res_ns          clock_getres answer (1)
///   52  u64 max_interp_tsc  interpolation bound; beyond it the emitter
///                           falls back to the real syscall
#[cfg(feature = "x86_64")]
pub mod x86_track {
    use core::sync::atomic::Ordering;

    use super::vdso_seq;

    // ---- lane instrumentation (vdso-clock): fallback/skip counters ----
    pub static TRACK_READ_NONE: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    pub static PEEK_NONE: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    pub static TICK_SKIP: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    #[inline]
    fn bump(c: &core::sync::atomic::AtomicU64, _tag: &str) {
        // Counters only, no console prints: the hunt prints fired every
        // 100 events and flooded the serial on SMP8 boots (ub2404:
        // 22k+ track-none events = 220+ lines/boot interleaved with
        // systemd output, burying getty and every diagnostic read-back).
        // Observability policy: instruments print nothing unless asked.
        c.fetch_add(1, Ordering::Relaxed);
    }
    /// Public bump for callers outside this module (vdso_data_tick).
    pub fn bump_pub(c: &core::sync::atomic::AtomicU64, tag: &str) {
        bump(c, tag)
    }

    // ------------------------------------------------------------------
    // field accessors (the page static — plain volatile loads/stores;
    // coherence is the seq word's job)
    // ------------------------------------------------------------------

    #[inline]
    pub(super) unsafe fn rd(off: usize) -> u64 {
        unsafe {
            core::ptr::read_volatile(
                super::VDSO_DATA_PAGE.0.as_ptr().add(off) as *const u64,
            )
        }
    }
    #[inline]
    pub(super) unsafe fn wr(off: usize, v: u64) {
        unsafe {
            core::ptr::write_volatile(
                super::VDSO_DATA_PAGE.0.as_mut_ptr().add(off) as *mut u64,
                v,
            )
        }
    }

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

    /// (delta_tsc * mult) >> 32 — full-precision nominal mapping,
    /// MULTIPLY FIRST, SHIFT SECOND.
    ///
    /// sc204 history (the shift-first bug): `(delta >> 32) * mult` left
    /// the kernel clock with a granularity of `mult` nominal ticks — at
    /// 4.29 GHz that is ONE STEP EVERY 2^32 TSC ticks = 1.0008 SECONDS;
    /// every timed wait in the kernel (nanosleep expiry checks,
    /// scheduler deltas, binder slices, the OH boot chain) coarsened to
    /// 1 s steps. Multiply-then-shift keeps the ~3 ppm rate accuracy
    /// AND ~0.23 ns granularity; the u128 product of delta (< uptime ×
    /// 4.3 GHz) and mult (~10M) cannot overflow. Same expression the
    /// emitter's mul+shrd computes.
    #[inline]
    pub fn tsc_delta_nominal(delta: u64, mult: u64) -> u64 {
        (((delta as u128) * (mult as u128)) >> 32) as u64
    }

    // ------------------------------------------------------------------
    // writer side (all under cmpxchg even→odd; losers skip)
    // ------------------------------------------------------------------

    /// Take the page for writing. Returns false when another writer
    /// holds it (seq odd) — the caller skips this round.
    #[inline]
    pub(super) unsafe fn seq_try_begin() -> bool {
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
    #[inline]
    pub(super) unsafe fn seq_end() {
        unsafe { vdso_seq().fetch_add(1, Ordering::Release); }
    }

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

    // ------------------------------------------------------------------
    // reader side
    // ------------------------------------------------------------------

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
                    bump(&PEEK_NONE, "peek-none-mult0");
                    return None; // not promoted
                }
                return Some((cl, base, mult));
            }
            bump(&PEEK_NONE, "peek-none-retry");
            None
        }
    }

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
                return Some(base.wrapping_add(tsc_delta_nominal(delta, mult)));
            }
            bump(&TRACK_READ_NONE, "track-none");
            None
        }
    }
}

/// x86_64 per-tick refresh: advance the anchor to NOW under the seq
/// word, and republish the epoch and interpolation bound. Forward-only
/// by construction (the measured TSC delta is non-negative); skipped
/// entirely while the TSC timekeeper is not promoted (mult == 0 keeps
/// userspace on the syscall fallback — the HPET/jiffies clock then
/// serves every read).
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
        let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
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

/// Physical addresses of the two vDSO pages (data, code) for the exec
/// mapping. `vdso_init()` must have run (code page built).
#[cfg(not(feature = "x86_64"))]
pub fn vdso_pages_phys() -> (u64, u64) {
    let data_va = core::ptr::addr_of!(VDSO_DATA_PAGE) as *const u8 as u64;
    let code_va = core::ptr::addr_of!(VDSO_CODE_PAGE) as *const u8 as u64;
    let d = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(data_va),
    );
    let c = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(code_va),
    );
    (d.bits(), c.bits())
}

/// x86_64 twin: the two statics live in the kernel image mapping,
/// whose link identity `VMA - KERNEL_LINK_BASE == LMA` holds for the
/// whole image (the __START_KERNEL_map trick — see memory_layout.rs),
/// so the physical address is plain arithmetic, no linear-map lookup
/// (the riscv64 blocker — statics outside the linear map — does not
/// exist here; see the disabled riscv block in exec.rs).
///
/// The frames are memblock-reserved (the boot code reserves [0, real
/// image end] on x86_64), so every mm teardown skips them via
/// `is_foreign_user_frame` — the same machinery that keeps the GEM/fb
/// aliases alive. `map_page`'s LEAF-LOW-HIT scribble tripwire allows
/// exactly these two frames (mmu_init.rs).
#[cfg(feature = "x86_64")]
pub fn vdso_pages_phys() -> (u64, u64) {
    let data_va = core::ptr::addr_of!(VDSO_DATA_PAGE) as *const u8 as u64;
    let code_va = core::ptr::addr_of!(VDSO_CODE_PAGE) as *const u8 as u64;
    let base = crate::arch::mm::memory_layout::KERNEL_LINK_BASE as u64;
    (data_va - base, code_va - base)
}

/// The two user-aliased vDSO physical frames (mmu_init's LEAF-LOW-HIT
/// exclusion test).
#[cfg(feature = "x86_64")]
pub fn is_vdso_phys(phys: u64) -> bool {
    let (d, c) = vdso_pages_phys();
    phys == d || phys == c
}

// ============================================================================
// Code page — mini RV64 assembler
// ============================================================================

static mut VDSO_CODE_PAGE: VdsoCodePage = VdsoCodePage([0; 4096]);

#[cfg(not(feature = "x86_64"))]
mod rv {
    // The original RV64 mini-assembler + function emitters. On
    // aarch64 the vDSO mapping stays disabled, so the RV encoding
    // rides along inert (same state as before the x86_64 port).
    use alloc::vec::Vec;

    /// Tiny two-pass label assembler (u32 words, forward/backward branches).
    struct Asm {
        words: alloc::vec::Vec<u32>,
        labels: [Option<u32>; 16],
        /// (insn index, label id, is_b_type)
        fixups: alloc::vec::Vec<(usize, usize, bool)>,
    }

    impl Asm {
        fn new() -> Self {
            Self {
                words: alloc::vec::Vec::new(),
                labels: [None; 16],
                fixups: alloc::vec::Vec::new(),
            }
        }
        fn label(&mut self, l: usize) {
            self.labels[l] = Some(self.words.len() as u32);
        }
        fn emit(&mut self, w: u32) {
            self.words.push(w);
        }
        /// Emit a branch-format word (beq/bne skeleton) targeting `l`.
        fn br(&mut self, w: u32, l: usize) {
            self.fixups.push((self.words.len(), l, true));
            self.words.push(w);
        }
        /// Emit a `j label` targeting `l`.
        fn jal_rel(&mut self, l: usize) {
            self.fixups.push((self.words.len(), l, false));
            self.words.push((0u32 << 7) | 0x6F);
        }
        fn finish(mut self) -> alloc::vec::Vec<u32> {
            for (idx, l, is_b) in self.fixups.drain(..) {
                let target = self.labels[l].expect("vdso: undefined label") as i64;
                let off = (target - idx as i64) as u32;
                self.words[idx] |= if is_b { b_imm(off) } else { j_imm(off) };
            }
            self.words
        }
    }

    // ---- encoders (RV64I + M) ----

    fn r_type(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
        (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
    }
    fn i_type(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
        ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
    }
    fn s_type(imm: i32, rs2: u32, rs1: u32, f3: u32, op: u32) -> u32 {
        let imm = imm as u32 & 0xFFF;
        ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 0x1F) << 7) | op
    }
    /// B-type immediate bits to OR into a branch skeleton.
    fn b_imm(off: u32) -> u32 {
        ((off & 0x1000) << 19) | ((off & 0x7E0) << 20) | ((off & 0x1E) << 7) | ((off & 0x800) >> 4)
    }
    /// J-type immediate bits to OR into a jal skeleton.
    fn j_imm(off: u32) -> u32 {
        ((off & 0x100000) << 11) | ((off & 0x7FE) << 20) | ((off & 0x800) << 9) | (off & 0xFF000)
    }
    fn u_type(imm20: u32, rd: u32, op: u32) -> u32 {
        ((imm20 & 0xFFFFF) << 12) | (rd << 7) | op
    }
    fn beq_skel(rs1: u32, rs2: u32) -> u32 {
        r_type(0, rs2, rs1, 0, 0, 0x63)
    }
    fn bne_skel(rs1: u32, rs2: u32) -> u32 {
        r_type(0, rs2, rs1, 1, 0, 0x63)
    }

    // registers
    const ZERO: u32 = 0;
    const RA: u32 = 1;
    const T0: u32 = 5;
    const T1: u32 = 6;
    const T2: u32 = 7;
    const T3: u32 = 28;
    const T4: u32 = 29;
    const T5: u32 = 30;
    const T6: u32 = 31;
    const A0: u32 = 10;
    const A1: u32 = 11;
    const A6: u32 = 16;
    const A7: u32 = 17;
    const S2: u32 = 18;

    // labels (each used by exactly ONE label() site — no re-binding)
    const L_CGI_RETRY_W: usize = 0;
    const L_CGI_MONO: usize = 1;
    const L_CGI_WALL: usize = 2;
    const L_CGI_TAI: usize = 3;
    const L_CGI_SPLIT: usize = 4;
    const L_CGI_RETRY_M: usize = 5;
    const L_GV_TV: usize = 6;
    const L_GV_RETRY: usize = 7;
    const L_GV_STORE: usize = 8;
    const L_GV_TZ: usize = 9;
    const L_GV_TZF: usize = 10;
    const L_RES_OK: usize = 11;
    const L_RES_FILL: usize = 12;

    /// `li rd, imm` (any positive 32-bit value).
    fn emit_li(a: &mut Asm, rd: u32, v: u32) {
        let lo = ((v as i32) << 20) >> 20; // sign-extended low 12 bits
        let hi = ((v as i64 - lo as i64) >> 12) as u32;
        let mut used = false;
        if hi != 0 {
            a.emit(u_type(hi, rd, 0x37)); // lui
            used = true;
        }
        if lo != 0 || !used {
            if used {
                a.emit(i_type(lo, rd, 0, rd, 0x1B)); // addiw (32-bit, sign-ext)
            } else {
                a.emit(i_type(lo, ZERO, 0, rd, 0x13)); // addi from x0
            }
        }
    }

    /// Prologue: compute the DATA page base into t2. `pc_off` is the emitting
    /// function's offset inside the code page (auipc reads its own PC).
    fn emit_data_base(a: &mut Asm, pc_off: u32) {
        a.emit(u_type(0xFFFFF, T2, 0x17)); // auipc t2, -1 → data + pc_off
        emit_li(a, T6, pc_off);
        a.emit(r_type(0x20, T6, T2, 0, T2, 0x33)); // sub t2, t2, t6 → data base
    }

    /// Seqlock-read under `retry` label: t4 = base_cycles, t5 = base_mono_ns.
    /// Clobbers t3, a6.
    fn emit_seqlock_read(a: &mut Asm, retry: usize) {
        a.label(retry);
        a.emit(i_type(0, T2, 2, T3, 0x03)); // lw t3, 0(t2)     seq
        a.emit(i_type(1, T3, 7, T4, 0x13)); // andi t4, t3, 1
        a.br(bne_skel(T4, ZERO), retry); // bnez t4, retry
        a.emit(0x0FF0_000F); // fence iorw, iorw (acquire)
        a.emit(i_type(8, T2, 3, T4, 0x03)); // ld t4, 8(t2)    base_cycles
        a.emit(i_type(16, T2, 3, T5, 0x03)); // ld t5, 16(t2)  base_mono_ns
        a.emit(i_type(0, T2, 2, A6, 0x03)); // lw a6, 0(t2)    seq2
        a.br(bne_skel(T3, A6), retry); // bne t3, a6, retry
    }

    /// t5 += (rdtime - t4) * 100 (ns interpolation at 10 MHz).
    ///
    /// IMPORTANT: read the SAME counter the kernel snapshots into the data
    /// page (CLINT `time`, CSR 0xC01 — see vdso_data_tick). The old code used
    /// `rdcycle` (0xC00), a different counter whose rate only approximately
    /// matches the CLINT under QEMU TCG; the two drifted by seconds under
    /// host load and vdso time() ran away from kernel wall_secs() (mtime
    /// source) — LTP utime01/02/03 before/after windows failed. U-mode
    /// access is enabled by scounteren.TM set in arch init.
    fn emit_interpolate(a: &mut Asm) {
        a.emit(i_type(0xC01, ZERO, 2, A7, 0x73)); // rdtime a7
        a.emit(r_type(0x20, T4, A7, 0, A7, 0x33)); // sub a7, a7, t4
        a.emit(i_type(100, ZERO, 0, T1, 0x13)); // li t1, 100
        a.emit(r_type(1, T1, A7, 0, A7, 0x33)); // mul a7, a7, t1
        a.emit(r_type(0, A7, T5, 0, T5, 0x33)); // add t5, t5, a7
    }

    fn emit_ret(a: &mut Asm) {
        a.emit(i_type(0, RA, 0, ZERO, 0x67)); // ret
    }

    /// __vdso_clock_gettime(clk=a0, tp=a1).
    pub fn emit_clock_gettime(off: u32) -> alloc::vec::Vec<u32> {
        let mut a = Asm::new();
        emit_data_base(&mut a, off);
        a.emit(i_type(0, ZERO, 0, S2, 0x13)); // li s2, 0 (TAI extra secs)
        a.br(beq_skel(A0, ZERO), L_CGI_WALL); // REALTIME
        a.emit(i_type(1, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_MONO);
        a.emit(i_type(4, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_MONO);
        a.emit(i_type(7, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_MONO);
        a.emit(i_type(11, ZERO, 0, T0, 0x13));
        a.br(beq_skel(A0, T0), L_CGI_TAI);
        a.emit(i_type(-38, ZERO, 0, A0, 0x13)); // li a0, -ENOSYS
        emit_ret(&mut a);
        a.label(L_CGI_TAI);
        a.emit(i_type(37, ZERO, 0, S2, 0x13)); // li s2, 37
        a.label(L_CGI_WALL);
        emit_seqlock_read(&mut a, L_CGI_RETRY_W);
        emit_interpolate(&mut a);
        // Wall clock: add the ns-precise epoch offset to the interpolated
        // monotonic ns BEFORE the sec/nsec split (a seconds-only epoch lost
        // sub-second clock_settime deltas — LTP clock_settime01).
        a.emit(i_type(24, T2, 3, A6, 0x03)); // ld a6, 24(t2) epoch ns
        a.emit(r_type(0, A6, T5, 0, T5, 0x33)); // add t5, t5, a6
        a.emit(r_type(0, S2, ZERO, 0, A6, 0x33)); // add a6, s2, 0 (TAI secs)
        a.jal_rel(L_CGI_SPLIT);
        a.label(L_CGI_MONO);
        emit_seqlock_read(&mut a, L_CGI_RETRY_M);
        emit_interpolate(&mut a);
        a.emit(i_type(0, ZERO, 0, A6, 0x13)); // li a6, 0
        a.jal_rel(L_CGI_SPLIT);
        // L_SPLIT: ns → (sec + a6 extra, nsec), store timespec at (a1).
        a.label(L_CGI_SPLIT);
        emit_li(&mut a, T1, 1_000_000_000);
        a.emit(r_type(1, T1, T5, 5, A7, 0x33)); // divu a7, t5, t1
        a.emit(r_type(1, T1, T5, 7, T4, 0x33)); // remu t4, t5, t1
        a.emit(r_type(0, A6, A7, 0, A7, 0x33)); // add a7, a7, a6
        a.emit(s_type(0, A7, A1, 3, 0x23)); // sd a7, 0(a1)
        a.emit(s_type(8, T4, A1, 3, 0x23)); // sd t4, 8(a1)
        a.emit(i_type(0, ZERO, 0, A0, 0x13)); // li a0, 0
        emit_ret(&mut a);
        a.finish()
    }

    /// __vdso_gettimeofday(tv=a0, tz=a1).
    pub fn emit_gettimeofday(off: u32) -> alloc::vec::Vec<u32> {
        let mut a = Asm::new();
        emit_data_base(&mut a, off);
        a.br(bne_skel(A0, ZERO), L_GV_TV);
        a.jal_rel(L_GV_TZ);
        a.label(L_GV_TV);
        emit_seqlock_read(&mut a, L_GV_RETRY);
        emit_interpolate(&mut a);
        // Wall clock with ns-precise epoch: total ns → (sec, usec).
        a.emit(i_type(24, T2, 3, A6, 0x03)); // ld a6, 24(t2) epoch ns
        a.emit(r_type(0, A6, T5, 0, T5, 0x33)); // add t5, t5, a6
        emit_li(&mut a, T1, 1_000_000_000);
        a.emit(r_type(1, T1, T5, 5, T4, 0x33)); // divu t4, t5, t1 → sec
        a.emit(r_type(1, T1, T5, 7, A6, 0x33)); // remu a6, t5, t1 → rem ns
        a.emit(i_type(1000, ZERO, 0, T1, 0x13)); // li t1, 1000
        a.emit(r_type(1, T1, A6, 5, A6, 0x33)); // divu a6, a6, t1 → usec
        a.label(L_GV_STORE);
        a.emit(s_type(0, A6, A0, 3, 0x23)); // sd a6, 0(a0) tv_sec
        a.emit(s_type(8, T4, A0, 3, 0x23)); // sd t4, 8(a0) tv_usec
        a.label(L_GV_TZ);
        a.br(bne_skel(A1, ZERO), L_GV_TZF);
        a.emit(i_type(0, ZERO, 0, A0, 0x13)); // li a0, 0
        emit_ret(&mut a);
        a.label(L_GV_TZF);
        a.emit(i_type(32, T2, 2, T3, 0x03)); // lw t3, 32(t2) tz_minuteswest
        a.emit(s_type(0, T3, A1, 2, 0x23)); // sw t3, 0(a1)
        a.emit(s_type(4, ZERO, A1, 2, 0x23)); // sw x0, 4(a1)
        a.emit(i_type(0, ZERO, 0, A0, 0x13)); // li a0, 0
        emit_ret(&mut a);
        a.finish()
    }

    /// __vdso_clock_getres(clk=a0, res=a1): {0, 100ns}.
    pub fn emit_clock_getres(off: u32) -> alloc::vec::Vec<u32> {
        let mut a = Asm::new();
        emit_data_base(&mut a, off);
        for clk in [0u32, 1, 4, 7, 11] {
            a.emit(i_type(clk as i32, ZERO, 0, T0, 0x13));
            a.br(beq_skel(A0, T0), L_RES_OK);
        }
        a.emit(i_type(-38, ZERO, 0, A0, 0x13)); // li a0, -ENOSYS
        emit_ret(&mut a);
        a.label(L_RES_OK);
        a.br(bne_skel(A1, ZERO), L_RES_FILL);
        a.emit(i_type(0, ZERO, 0, A0, 0x13));
        emit_ret(&mut a);
        a.label(L_RES_FILL);
        a.emit(s_type(0, ZERO, A1, 3, 0x23)); // sd x0, 0(a1)  tv_sec = 0
        a.emit(i_type(100, ZERO, 0, T0, 0x13)); // li t0, 100
        a.emit(s_type(8, T0, A1, 3, 0x23)); // sd t0, 8(a1)  tv_nsec = 100
        a.emit(i_type(0, ZERO, 0, A0, 0x13));
        emit_ret(&mut a);
        a.finish()
    }
}

// ============================================================================
// x86_64 code page emitters (106th vDSO lever, re-armed 2026-11-15)
// ============================================================================
//
// Three position-dependent functions, hand-encoded System V AMD64:
//   __vdso_clock_gettime(rdi=clk, rsi=tp)   -> 0 / raw -errno (fallback)
//   __vdso_gettimeofday (rdi=tv,  rsi=tz)   -> 0
//   __vdso_clock_getres (rdi=clk, rsi=res)  -> 0 / -ENOSYS
//
// Register contract per function: r11 = data page base (RIP-relative
// LEA, so the code page works at whatever VDSO_BASE it is mapped at),
// r10 = TAI extra seconds, rax/rcx/rdx/r8/r9 scratch. Only
// caller-saved registers are touched — no stack frame; the sole stack
// access is seqlock_read's balanced push/pop rdx (seq1 across RDTSC;
// see there).
//
// Time math IS the kernel's TSC path (drivers/timer/x86_64.rs
// read_time → x86_track::tsc_track_now_nominal): the seq protocol
// [seq1 → cycle_last/base/mult → LFENCE;RDTSC → seq2, retry] then
//   nom = base_mono_nom + ((tsc - cycle_last) * mult) >> 32
//   ns  = nom * 100
// The kernel syscall path computes the identical expression from the
// identical page words (and splits ns with the same floor arithmetic
// as monotonic_time()), so a vDSO read and a syscall read at the same
// instant are bit-identical — the strict adjacent-pair requirement of
// LTP clock_gettime04 (see the x86_track doc above). mult is the
// nominal-domain TSC multiplier; shift is always 32, an immediate in
// MUL+SHRD. A delta beyond the published bound (offset 52) — anchor
// stale or TSC smashed — falls back to the real syscall.
#[cfg(feature = "x86_64")]
mod x86 {
    use alloc::vec::Vec;

    /// Byte assembler with rel8 label fixups (every branch in the three
    /// functions is short — the bodies are < 200 bytes) plus rel32 jumps
    /// for the few long hops (dispatch → syscall fallback).
    struct Asm {
        bytes: Vec<u8>,
        labels: [Option<usize>; 8],
        /// (insn index, label, byte position of the rel8 slot)
        fixups: Vec<(usize, usize, usize)>,
        /// (label, byte position of the rel32 slot)
        fixups32: Vec<(usize, usize)>,
    }

    impl Asm {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                labels: [None; 8],
                fixups: Vec::new(),
                fixups32: Vec::new(),
            }
        }
        fn b(&mut self, bytes: &[u8]) {
            self.bytes.extend_from_slice(bytes);
        }
        fn label(&mut self, l: usize) {
            self.labels[l] = Some(self.bytes.len());
        }
        fn jcc(&mut self, op: u8, l: usize) {
            self.fixups.push((usize::MAX, l, self.bytes.len() + 1));
            self.b(&[op, 0]);
        }
        /// Near jmp (E9 rel32).
        fn jmp32(&mut self, l: usize) {
            self.fixups32.push((l, self.bytes.len() + 1));
            self.b(&[0xE9, 0, 0, 0, 0]);
        }
        /// Near jcc (0F 8x rel32).
        fn jcc32(&mut self, op: u8, l: usize) {
            self.fixups32.push((l, self.bytes.len() + 2));
            self.b(&[0x0F, op, 0, 0, 0, 0]);
        }
        fn finish(mut self) -> Vec<u8> {
            for (_, l, at) in self.fixups.drain(..) {
                let target = self.labels[l].expect("vdso x86: undefined label") as i64;
                let rel = target - (at as i64 + 1);
                assert!(
                    (-128..=127).contains(&rel),
                    "vdso x86: rel8 out of range ({rel})"
                );
                self.bytes[at] = rel as u8;
            }
            for (l, at) in self.fixups32.drain(..) {
                let target = self.labels[l].expect("vdso x86: undefined label") as i64;
                let rel = target - (at as i64 + 4);
                assert!(
                    rel >= i32::MIN as i64 && rel <= i32::MAX as i64,
                    "vdso x86: rel32 out of range ({rel})"
                );
                self.bytes[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
            }
            self.bytes
        }
    }

    /// `lea r11, [rip - (0x1000 + off + 7)]` — r11 = the DATA page base
    /// (one page below this function's page).
    fn lea_data_base(a: &mut Asm, off: u32) {
        let d: i32 = -((off + 7 + 0x1000) as i32);
        a.b(&[0x4C, 0x8D, 0x1D]);
        a.b(&d.to_le_bytes());
    }

    // ---- shared instruction fragments (see the table above) ----
    //
    // Seqlock read WITH the TSC stamp inside the window — the protocol
    // the kernel's tsc_track_now_nominal() uses. The in-window rdtsc
    // guarantees cycle_last <= stamped TSC at seq2-check time (a
    // refresh landing between the field loads and the rdtsc bumps the
    // seq word and the whole read retries), so the delta never wraps.
    //   edx = seq1; r8 = cycle_last(+8); r9 = base_mono_nom(+16);
    //   rcx = mult(+40); lfence; rdtsc -> rax; seq2 recheck.
    //
    // seq1 must survive RDTSC, which clobbers rax/rdx — and every other
    // register is live (rdi/rsi are the caller's args, also the syscall
    // fallback ABI; rcx mult; r8/r9 fields; r10 TAI secs; r11 page). A
    // balanced PUSH/POP RDX around the pair is the only stack touch in
    // the three functions (8 bytes, transient, leaf-safe: any async
    // event sees a consistent rsp and the slot sits inside the red
    // zone). Without it the rebase `cmp [r11],edx` would compare the
    // seq word against rdx's POST-shl low half — always 0 — and the
    // retry loop would never exit once the first publication bumps the
    // seq word off 0 (every fast-path read would spin).
    fn seqlock_read(a: &mut Asm, retry: usize) {
        a.label(retry);
        a.b(&[0x41, 0x8B, 0x13]); // mov edx,[r11]      seq1
        a.b(&[0xF6, 0xC2, 0x01]); // test dl,1
        a.jcc(0x75, retry); //     jne retry
        a.b(&[0x4D, 0x8B, 0x43, 0x08]); // mov r8,[r11+8]   cycle_last
        a.b(&[0x4D, 0x8B, 0x4B, 0x10]); // mov r9,[r11+16]  base_mono_nom
        a.b(&[0x49, 0x8B, 0x4B, 0x28]); // mov rcx,[r11+40] mult
        a.b(&[0x52]); // push rdx — seq1 survives the rdtsc
        a.b(&[0x0F, 0xAE, 0xE8]); // lfence (ordered rdtsc)
        a.b(&[0x0F, 0x31]); //     rdtsc
        a.b(&[0x48, 0xC1, 0xE2, 0x20]); // shl rdx,32
        a.b(&[0x48, 0x09, 0xD0]); //     or rax,rdx        rax = raw tsc
        a.b(&[0x5A]); // pop rdx — seq1 back (rdx dead after the or)
        a.b(&[0x41, 0x39, 0x13]); //     cmp [r11],edx     seq2 recheck
        a.jcc(0x75, retry); //     jne retry
    }
    /// rax = interpolated ns from (rax raw tsc, r8 cycle_last,
    /// r9 base_mono_nom, rcx mult) — the unified-track arithmetic:
    /// nom = base + ((tsc - cycle_last) * mult) >> 32; ns = nom * 100.
    ///
    /// `fb` is the fallback label: when the raw-TSC delta since
    /// cycle_last exceeds the published bound (data page +52), the
    /// interpolation is untrustworthy — the anchor is stale (ticks
    /// stopped) or the TSC jumped — so issue the REAL syscall (rdi and
    /// rsi still carry the caller's arguments, which is the syscall ABI
    /// too). A wrapped (negative) delta also compares unsigned-above
    /// and takes the fallback.
    fn interpolate(a: &mut Asm, fb: usize) {
        a.b(&[0x4C, 0x29, 0xC0]); //     sub rax,r8       delta tsc
        a.b(&[0x49, 0x8B, 0x53, 0x34]); // mov rdx,[r11+52] max_interp_tsc
        a.b(&[0x48, 0x39, 0xD0]); //     cmp rax,rdx
        a.jcc(0x77, fb); //     ja  fallback (also catches wrapped delta)
        a.b(&[0x48, 0xF7, 0xE1]); //     mul rcx          rdx:rax
        a.b(&[0x48, 0x0F, 0xAC, 0xD0, 0x20]); // shrd rax,rdx,32   nom_delta
        a.b(&[0x4C, 0x01, 0xC8]); //     add rax,r9       + base_mono_nom
        a.b(&[0x48, 0x6B, 0xC0, 0x64]); // imul rax,rax,100  nominal -> ns
    }
    /// ns in rax -> timespec {rax+r10 sec, rdx nsec} stored at rsi.
    fn split_store(a: &mut Asm) {
        a.b(&[0xB9, 0x00, 0xCA, 0x9A, 0x3B]); // mov ecx,1000000000
        a.b(&[0x31, 0xD2]); //     xor edx,edx
        a.b(&[0x48, 0xF7, 0xF1]); // div rcx
        a.b(&[0x4C, 0x01, 0xD0]); // add rax,r10       TAI secs
        a.b(&[0x48, 0x89, 0x06]); // mov [rsi],rax
        a.b(&[0x48, 0x89, 0x56, 0x08]); // mov [rsi+8],rdx
        a.b(&[0x31, 0xC0]); //     xor eax,eax
        a.b(&[0xC3]); //     ret
    }
    /// Clock dispatch. REALTIME(0)/MONOTONIC(1)/RAW(4)/BOOTTIME(7)/TAI(11)
    /// take the fast path; EVERYTHING ELSE — the COARSE clocks (5/6, which
    /// the syscall serves grid-snapped from the same track), CPUTIME ids,
    /// and garbage — goes to the SYSCALL FALLBACK, the reference's shape:
    /// Linux's __vdso_clock_gettime never returns -ENOSYS itself, it
    /// internally falls back to the real syscall for every clock the
    /// fast path doesn't carry (coarse included). Returning the raw
    /// syscall's result (0, or -errno raw) instead of -ENOSYS also keeps
    /// RAW vDSO callers correct: LTP's vdso variants check `ret != 0`
    /// with errno==ENOSYS to skip, but a vDSO function cannot set errno,
    /// and the caller's errno is stale harness state (clock_gettime04's
    /// j=2 read ECHILD left by LTP's own wait() reaping — TFAIL on every
    /// COARSE clock with the -ENOSYS form).
    fn dispatch(a: &mut Asm, wall: usize, mono: usize, tai: usize, fb: usize) {
        a.b(&[0x48, 0x85, 0xFF]); // test rdi,rdi     REALTIME(0)
        a.jcc(0x74, wall);
        a.b(&[0x48, 0x83, 0xFF, 0x01]); // cmp rdi,1  MONOTONIC
        a.jcc(0x74, mono);
        a.b(&[0x48, 0x83, 0xFF, 0x04]); // cmp rdi,4  MONOTONIC_RAW
        a.jcc(0x74, mono);
        a.b(&[0x48, 0x83, 0xFF, 0x07]); // cmp rdi,7  BOOTTIME
        a.jcc(0x74, mono);
        a.b(&[0x48, 0x83, 0xFF, 0x0B]); // cmp rdi,11 TAI
        a.jcc(0x74, tai);
        a.jmp32(fb); // everything else: real syscall
    }

    const L_WALL: usize = 0;
    const L_MONO: usize = 1;
    const L_TAI: usize = 2;
    const L_RETRY_W: usize = 3;
    const L_RETRY_M: usize = 4;
    const L_SPLIT: usize = 5;
    const L_FB_CGI: usize = 6;

    /// `mov eax, nr; syscall; ret` — the bound-exceeded fallback. rdi/rsi
    /// still hold the caller's arguments in the SysV clock/gettimeofday
    /// register contract, which is exactly the syscall ABI too.
    fn syscall_fallback(a: &mut Asm, nr: u32) {
        a.b(&[0xB8]);
        a.b(&nr.to_le_bytes());
        a.b(&[0x0F, 0x05]); // syscall
        a.b(&[0xC3]); //     ret
    }

    pub fn emit_clock_gettime(off: u32) -> Vec<u8> {
        let mut a = Asm::new();
        lea_data_base(&mut a, off);
        a.b(&[0x45, 0x31, 0xD2]); // xor r10d,r10d   extra secs = 0
        dispatch(&mut a, L_WALL, L_MONO, L_TAI, L_FB_CGI);
        a.label(L_TAI);
        a.b(&[0x41, 0xBA, 0x25, 0x00, 0x00, 0x00]); // mov r10d,37
        a.label(L_WALL);
        seqlock_read(&mut a, L_RETRY_W);
        interpolate(&mut a, L_FB_CGI);
        a.b(&[0x49, 0x03, 0x43, 0x18]); // add rax,[r11+24]  + epoch ns
        a.b(&[0xEB, 0x00]); //     jmp L_SPLIT (fixup-free: patched below)
        // Patch the short jmp target manually (single use, no label
        // machinery needed): displacement to the split body.
        let split_at = a.bytes.len();
        a.label(L_MONO);
        seqlock_read(&mut a, L_RETRY_M);
        interpolate(&mut a, L_FB_CGI);
        let after_mono = a.bytes.len();
        let rel = (after_mono as i64 - split_at as i64) as i8;
        a.bytes[split_at - 1] = rel as u8;
        a.label(L_SPLIT);
        split_store(&mut a);
        a.label(L_FB_CGI);
        syscall_fallback(&mut a, 228); // __NR_clock_gettime
        a.finish()
    }

    const L_TV: usize = 0;
    const L_RETRY: usize = 1;
    const L_TAIL: usize = 2;
    const L_RET0: usize = 3;
    const L_FB_GV: usize = 4;

    pub fn emit_gettimeofday(off: u32) -> Vec<u8> {
        let mut a = Asm::new();
        lea_data_base(&mut a, off);
        a.b(&[0x48, 0x85, 0xFF]); // test rdi,rdi
        a.jcc(0x75, L_TV); //     jne L_TV
        // rdi == NULL: skip the tv store, do the tz tail.
        a.b(&[0xEB, 0x00]); //     jmp L_TAIL (patched below)
        let tail_jmp_at = a.bytes.len();
        a.label(L_TV);
        seqlock_read(&mut a, L_RETRY);
        interpolate(&mut a, L_FB_GV);
        a.b(&[0x49, 0x03, 0x43, 0x18]); // add rax,[r11+24]  + epoch ns
        a.b(&[0xB9, 0x00, 0xCA, 0x9A, 0x3B]); // mov ecx,1000000000
        a.b(&[0x31, 0xD2]); //     xor edx,edx
        a.b(&[0x48, 0xF7, 0xF1]); // div rcx          sec / rem-ns
        a.b(&[0x48, 0x89, 0x07]); // mov [rdi],rax    tv_sec
        a.b(&[0x48, 0x89, 0xD0]); // mov rax,rdx
        a.b(&[0x31, 0xD2]); //     xor edx,edx
        a.b(&[0xB9, 0xE8, 0x03, 0x00, 0x00]); // mov ecx,1000
        a.b(&[0x48, 0xF7, 0xF1]); // div rcx          usec
        a.b(&[0x48, 0x89, 0x47, 0x08]); // mov [rdi+8],rax tv_usec
        let tail_target = a.bytes.len();
        let rel = (tail_target as i64 - tail_jmp_at as i64) as i8;
        a.bytes[tail_jmp_at - 1] = rel as u8;
        a.label(L_TAIL);
        a.b(&[0x48, 0x85, 0xF6]); // test rsi,rsi
        a.jcc(0x74, L_RET0); //    je L_RET0
        a.b(&[0x48, 0xC7, 0x06, 0x00, 0x00, 0x00, 0x00]); // mov qword [rsi],0
        a.b(&[0x48, 0xC7, 0x46, 0x08, 0x00, 0x00, 0x00, 0x00]); // mov qword [rsi+8],0
        a.label(L_RET0);
        a.b(&[0x31, 0xC0]); //     xor eax,eax
        a.b(&[0xC3]); //     ret
        a.label(L_FB_GV);
        syscall_fallback(&mut a, 96); // __NR_gettimeofday
        a.finish()
    }

    const L_OK: usize = 0;
    const L_RET0G: usize = 1;

    pub fn emit_clock_getres(off: u32) -> Vec<u8> {
        let mut a = Asm::new();
        lea_data_base(&mut a, off);
        for clk in [0u8, 1, 4, 7, 11] {
            if clk == 0 {
                a.b(&[0x48, 0x85, 0xFF]); // test rdi,rdi
                a.jcc(0x74, L_OK);
            } else {
                a.b(&[0x48, 0x83, 0xFF, clk]); // cmp rdi,clk
                a.jcc(0x74, L_OK);
            }
        }
        a.b(&[0xB8, 0xDA, 0xFF, 0xFF, 0xFF]); // mov eax,-38
        a.b(&[0xC3]); //     ret
        a.label(L_OK);
        a.b(&[0x48, 0x85, 0xF6]); // test rsi,rsi
        a.jcc(0x74, L_RET0G); //    je L_RET0G
        a.b(&[0x48, 0xC7, 0x06, 0x00, 0x00, 0x00, 0x00]); // mov qword [rsi],0
        a.b(&[0x41, 0x8B, 0x43, 0x30]); // mov eax,[r11+48] res_ns
        a.b(&[0x48, 0x89, 0x46, 0x08]); // mov [rsi+8],rax
        a.label(L_RET0G);
        a.b(&[0x31, 0xC0]); //     xor eax,eax
        a.b(&[0xC3]); //     ret
        a.finish()
    }
}

// ============================================================================
// ELF construction
// ============================================================================

/// SysV ELF hash (DT_HASH chains).
fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in name {
        h = (h << 4).wrapping_add(b as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// Build the code page: ELF header + phdrs + dynamic + DT_HASH + dynsym
/// + dynstr + the three functions.
fn build_code_page() -> [u8; 4096] {
    let mut page = [0u8; 4096];

    // ---- function bodies (fixed offsets) ----
    const F_CLOCK_GETTIME: u32 = 0x800;
    const F_GETTIMEOFDAY: u32 = 0x920;
    const F_CLOCK_GETRES: u32 = 0x9C0;
    #[cfg(not(feature = "x86_64"))]
    {
        let f1 = rv::emit_clock_gettime(F_CLOCK_GETTIME);
        let f2 = rv::emit_gettimeofday(F_GETTIMEOFDAY);
        let f3 = rv::emit_clock_getres(F_CLOCK_GETRES);
        let put = |page: &mut [u8; 4096], off: u32, words: &[u32]| {
            for (i, w) in words.iter().enumerate() {
                let p = off as usize + i * 4;
                page[p..p + 4].copy_from_slice(&w.to_le_bytes());
            }
        };
        put(&mut page, F_CLOCK_GETTIME, &f1);
        put(&mut page, F_GETTIMEOFDAY, &f2);
        put(&mut page, F_CLOCK_GETRES, &f3);
    }
    #[cfg(feature = "x86_64")]
    {
        let f1 = x86::emit_clock_gettime(F_CLOCK_GETTIME);
        let f2 = x86::emit_gettimeofday(F_GETTIMEOFDAY);
        let f3 = x86::emit_clock_getres(F_CLOCK_GETRES);
        let putb = |page: &mut [u8; 4096], off: u32, bytes: &[u8]| {
            let p = off as usize;
            page[p..p + bytes.len()].copy_from_slice(bytes);
        };
        putb(&mut page, F_CLOCK_GETTIME, &f1);
        putb(&mut page, F_GETTIMEOFDAY, &f2);
        putb(&mut page, F_CLOCK_GETRES, &f3);
    }

    // ---- dynstr @ 0x240 ----
    const STRTAB_OFF: usize = 0x240;
    // x86_64: canonical names RESTORED (vDSO re-arm, 2026-11-15). The
    // inert-name export (a6ee2acd5) was the deliberate correctness
    // trade of the live fast path for LTP clock_gettime04: the old
    // publication carried the TSC_CPU_OFF spread between publisher and
    // reader, and the residual µs-scale bidirectional skew made one of
    // the two adjacent-pair directions read negative (observed −4582 ns
    // with the fast path live; the syscall track's per-CPU corrections
    // + ratchet are not reproducible by a raw-TSC interpolation). With
    // the unified track (x86_track above) the vDSO and the syscall
    // compute bit-identical values from the same page words, so the
    // strict adjacent-pair requirement holds structurally and the
    // names come back — glibc and LTP's parser take the fast path
    // again (the 21.3M clock_gettime syscalls per OH boot, ~6.6% of
    // the box, move back to userspace).
    #[cfg(feature = "x86_64")]
    const NAMES: [&str; 3] = [
        "__vdso_clock_gettime",
        "__vdso_gettimeofday",
        "__vdso_clock_getres",
    ];
    #[cfg(not(feature = "x86_64"))]
    const NAMES: [&str; 3] = [
        "__vdso_clock_gettime",
        "__vdso_gettimeofday",
        "__vdso_clock_getres",
    ];
    let mut name_offs = [0usize; 3];
    let mut strtab_len = 1usize; // leading NUL (null symbol name)
    // rwlock-livelock (Go vdso crash): the version strings live in the
    // same strtab — vda_name offsets into it.
    let mut vda_soname_off = 0usize;
    let mut vda_linux26_off = 0usize;
    {
        page[STRTAB_OFF] = 0;
        let mut p = STRTAB_OFF + 1;
        for (i, n) in NAMES.iter().enumerate() {
            name_offs[i] = p - STRTAB_OFF;
            for &b in n.as_bytes() {
                page[p] = b;
                p += 1;
            }
            page[p] = 0;
            p += 1;
        }
        vda_soname_off = p - STRTAB_OFF;
        for &b in b"linux-vdso.so.1" {
            page[p] = b;
            p += 1;
        }
        page[p] = 0;
        p += 1;
        vda_linux26_off = p - STRTAB_OFF;
        for &b in b"LINUX_2.6" {
            page[p] = b;
            p += 1;
        }
        page[p] = 0;
        p += 1;
        strtab_len = p - STRTAB_OFF;
    }

    // ---- SHT_GNU_versym @ 0x2A0: null sym -> 0, the 3 funcs -> ndx 2 ----
    // (LINUX_2.6, the only symbol version; mirrors the reference vDSO's
    // version-scripted dynsym.)
    const VERSYM_OFF: usize = 0x2A0;
    page[VERSYM_OFF..VERSYM_OFF + 2].copy_from_slice(&0u16.to_le_bytes());
    for i in 0..3 {
        let p = VERSYM_OFF + (i + 1) * 2;
        page[p..p + 2].copy_from_slice(&2u16.to_le_bytes());
    }

    // ---- SHT_GNU_verdef @ 0x2C0 (Go vdso crash fix, 2026-11-14) ----
    // Without DT_VERDEF, Go's runtime.vdsoFindVersion walks a NULL
    // verdef pointer and dies at its first clock read (snapd on the
    // ub2404 image: SEGV at vdsoFindVersion+0x33, cr2=0x2 — the
    // vd_flags load off def==nil). glibc is fine either way (an absent
    // DT_VERSYM makes check_match accept the symbol unversioned), but
    // Go REQUIRES the section to exist. Two entries, mirroring Linux
    // (arch/x86/entry/vdso): ndx 1 = BASE soname, ndx 2 = LINUX_2.6 —
    // the version vdsoFindVersion matches for its symbol lookups.
    const VERDEF_OFF: usize = 0x2C0;
    let h_linux26 = elf_hash(b"LINUX_2.6");
    let h_soname = elf_hash(b"linux-vdso.so.1");
    let mut put_vd = |page: &mut [u8; 4096], at: usize, ver: u16, flags: u16, ndx: u16, cnt: u16, hash: u32, aux: u32, next: u32, name: u32| {
        page[at..at + 2].copy_from_slice(&ver.to_le_bytes());
        page[at + 2..at + 4].copy_from_slice(&flags.to_le_bytes());
        page[at + 4..at + 6].copy_from_slice(&ndx.to_le_bytes());
        page[at + 6..at + 8].copy_from_slice(&cnt.to_le_bytes());
        page[at + 8..at + 12].copy_from_slice(&hash.to_le_bytes());
        page[at + 12..at + 16].copy_from_slice(&aux.to_le_bytes());
        page[at + 16..at + 20].copy_from_slice(&next.to_le_bytes());
        let vd_aux_at = at + aux as usize;
        page[vd_aux_at..vd_aux_at + 4].copy_from_slice(&name.to_le_bytes());
        page[vd_aux_at + 4..vd_aux_at + 8].copy_from_slice(&0u32.to_le_bytes());
    };
    // Entry 1 @ +0: BASE soname; aux (Verdaux) at +20; next entry at +28.
    put_vd(
        &mut page,
        VERDEF_OFF,
        1,    // vd_version
        1,    // vd_flags = VER_FLG_BASE
        1,    // vd_ndx
        1,    // vd_cnt
        h_soname,
        20,   // vd_aux: Verdaux directly after this Verdef
        28,   // vd_next: entry2 follows aux (20 + 8)
        vda_soname_off as u32,
    );
    // Entry 2 @ +28: LINUX_2.6 (the matchable version), last in chain.
    put_vd(
        &mut page,
        VERDEF_OFF + 28,
        1, // vd_version
        0, // vd_flags: no BASE — vdsoFindVersion matches this one
        2, // vd_ndx (what DT_VERSYM words above reference)
        1, // vd_cnt
        h_linux26,
        20, // vd_aux
        0,  // vd_next: end of chain
        vda_linux26_off as u32,
    );

    // ---- dynsym @ 0x1C0: null + 3 STT_GLOBAL FUNC syms (24B each) ----
    const SYMTAB_OFF: usize = 0x1C0;
    let sym_values = [F_CLOCK_GETTIME, F_GETTIMEOFDAY, F_CLOCK_GETRES];
    for i in 0..3 {
        let p = SYMTAB_OFF + (i + 1) * 24;
        page[p..p + 4].copy_from_slice(&(name_offs[i] as u32).to_le_bytes());
        page[p + 4] = 0x12; // (STB_GLOBAL << 4) | STT_FUNC
        page[p + 6..p + 8].copy_from_slice(&1u16.to_le_bytes()); // st_shndx
        page[p + 8..p + 16].copy_from_slice(&(sym_values[i] as u64).to_le_bytes());
        // st_size 0 (informational)
    }

    // ---- DT_HASH @ 0x140: nbucket=2, nchain=4, bucket[2], chain[4] ----
    // TERMINATOR IS 0, never 0xFFFFFFFF (106th fix): SysV hash lookups
    // walk `for (i = bucket[h % nbucket]; i; i = chain[i])` — 0 (STN_UNDEF)
    // is the empty-bucket and end-of-chain marker (musl ldso and glibc
    // both). The first cut used u32::MAX sentinels and pid 1 segfaulted
    // at its first vdso lookup: chain 0xFFFFFFFF * sizeof(Elf64_Sym)=24
    // = 0x17_FFFF_FFE8 past the symtab — the exact CR2 delta decoded
    // from the USEGV-FOR dump (ripbytes were musl's hash loop). The
    // u32::MAX convention had never been exercised: the riscv64 mapping
    // it was written for stayed disabled.
    const HASH_OFF: usize = 0x180;
    page[HASH_OFF..HASH_OFF + 4].copy_from_slice(&2u32.to_le_bytes());
    page[HASH_OFF + 4..HASH_OFF + 8].copy_from_slice(&4u32.to_le_bytes());
    // buckets and chains stay ZERO-initialized (0 = empty/end).
    for i in 0..3 {
        let b = (elf_hash(NAMES[i].as_bytes()) % 2) as usize;
        let bucket = u32::from_le_bytes(
            page[HASH_OFF + 8 + b * 4..HASH_OFF + 12 + b * 4].try_into().unwrap(),
        );
        if bucket == 0 {
            page[HASH_OFF + 8 + b * 4..HASH_OFF + 12 + b * 4]
                .copy_from_slice(&((i + 1) as u32).to_le_bytes());
        } else {
            let mut cur = bucket as usize;
            loop {
                let next = u32::from_le_bytes(
                    page[HASH_OFF + 16 + cur * 4..HASH_OFF + 20 + cur * 4].try_into().unwrap(),
                );
                if next == 0 {
                    page[HASH_OFF + 16 + cur * 4..HASH_OFF + 20 + cur * 4]
                        .copy_from_slice(&((i + 1) as u32).to_le_bytes());
                    break;
                }
                cur = next as usize;
            }
        }
    }

    // ---- PT_DYNAMIC @ 0xC0 (9 entries + NULL × 16B = 160B) ----
    // 0xC0+160 = 0x160, clear of HASH @ 0x180 (the 3 added version
    // tags pushed the old 96-byte table into the hash table's slot).
    const DYN_OFF: usize = 0xC0;
    // DT_STRTAB/DT_SYMTAB values were SWAPPED until 2026-11-14
    // (arena-mmap lane): DT_SYMTAB pointed at the string table, so
    // every consumer that resolves symbols through the dynamic table
    // (musl __vdsosym — clock_gettime/gettimeofday fast paths) parsed
    // string bytes as Elf64_Sym entries, matched nothing, and silently
    // fell back to the real syscalls. The clock_gettime fallback turned
    // the FFRT arena pattern's user-mode vDSO store (SEGV → handler
    // installs the page → retry succeeds, reference-kernel behavior)
    // into a plain syscall EFAULT instead — one root of the OH mmi
    // service death family (rip inside the libc time path storing to a
    // PROT_NONE arena page).
    let dyn_entries: [(u64, u64); 10] = [
        (4, HASH_OFF as u64),   // DT_HASH
        (5, STRTAB_OFF as u64), // DT_STRTAB
        (6, SYMTAB_OFF as u64), // DT_SYMTAB
        (10, strtab_len as u64), // DT_STRSZ
        (11, 24),               // DT_SYMENT
        // Version tags (Go vdso crash fix): Go's runtime.vdsoFindVersion
        // dereferences DT_VERDEF unconditionally — a missing section is a
        // nil pointer and every Go binary dies at its first clock read
        // (snapd: SEGV at vdsoFindVersion+0x33, cr2=0x2). glibc is
        // indifferent (unversioned symbols are accepted when DT_VERSYM
        // is absent; with it present the LINUX_2.6 match is exact).
        // Tag values (elf.h): VERDEF 0x6ffffffc, VERDEFNUM 0x6ffffffd,
        // VERSYM 0x6ffffff0. Boot H caught the first cut using 0x6ffffffe
        // (DT_VERNEED!) for VERSYM: ld.so parses the vDSO's dynamic table
        // during startup dependency processing, read the versym array as
        // a Verneed with vn_version garbage and refused to load init
        // ("unsupported version 0 of Verneed record" -> poweroff).
        (0x6ffffffc, VERDEF_OFF as u64),   // DT_VERDEF
        (0x6ffffffd, 2),                   // DT_VERDEFNUM
        (0x6ffffff0, VERSYM_OFF as u64),   // DT_VERSYM
        (0, 0),                            // DT_NULL
        (0, 0),                            // DT_NULL (padding keeps 160B)
    ];
    for (i, (tag, val)) in dyn_entries.iter().enumerate() {
        let p = DYN_OFF + i * 16;
        page[p..p + 8].copy_from_slice(&tag.to_le_bytes());
        page[p + 8..p + 16].copy_from_slice(&val.to_le_bytes());
    }

    // ---- program headers @ 0x40 (Elf64_Phdr = 56 bytes each) ----
    const PHDR_OFF: usize = 0x40;
    let mut put_ph = |page: &mut [u8; 4096], p: usize, t: u32, fl: u32, o: u64, v: u64, fs: u64, ms: u64, al: u64| {
        page[p..p + 4].copy_from_slice(&t.to_le_bytes());
        page[p + 4..p + 8].copy_from_slice(&fl.to_le_bytes());
        page[p + 8..p + 16].copy_from_slice(&o.to_le_bytes());
        page[p + 16..p + 24].copy_from_slice(&v.to_le_bytes());
        page[p + 24..p + 32].copy_from_slice(&v.to_le_bytes()); // paddr = vaddr
        page[p + 32..p + 40].copy_from_slice(&fs.to_le_bytes());
        page[p + 40..p + 48].copy_from_slice(&ms.to_le_bytes());
        page[p + 48..p + 56].copy_from_slice(&al.to_le_bytes());
    };
    // PT_LOAD: whole page, R+X, vaddr 0 (load bias = AT_SYSINFO_EHDR).
    put_ph(&mut page, PHDR_OFF, 1, 5, 0, 0, 4096, 4096, 4096);
    // PT_DYNAMIC: R, at DYN_OFF.
    put_ph(
        &mut page,
        PHDR_OFF + 56,
        2,
        4,
        DYN_OFF as u64,
        DYN_OFF as u64,
        160,
        160,
        8,
    );

    // ---- ELF64 header ----
    {
        let e = &mut page[0..64];
        e[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        e[4] = 2; // ELFCLASS64
        e[5] = 1; // ELFDATA2LSB
        e[6] = 1; // EV_CURRENT
        e[7] = 0; // ELFOSABI_NONE
        e[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
        // e_machine must match the target: loaders sanity-check it against
        // their own arch before trusting the vDSO image. Mirrors
        // fs::elf::Elf64Ehdr::check_machine.
        #[cfg(feature = "aarch64")]
        let e_machine: u16 = crate::fs::elf::ElfMachine::EM_AARCH64 as u16;
        #[cfg(feature = "riscv64")]
        let e_machine: u16 = crate::fs::elf::ElfMachine::EM_RISCV as u16;
        #[cfg(feature = "x86_64")]
        let e_machine: u16 = crate::fs::elf::ElfMachine::EM_X86_64 as u16;
        e[18..20].copy_from_slice(&e_machine.to_le_bytes());
        e[20..24].copy_from_slice(&1u32.to_le_bytes()); // version
        e[24..32].copy_from_slice(&(F_CLOCK_GETTIME as u64).to_le_bytes()); // entry
        e[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
        e[40..48].copy_from_slice(&0u64.to_le_bytes()); // shoff
        // e_flags 0 is honest on all three targets: riscv 0 = base ISA with
        // soft-float conventions (the emitted code is integer-only, so no
        // float-ABI byte matters), and aarch64/x86_64 define no e_flags
        // bits at all.
        e[48..52].copy_from_slice(&0u32.to_le_bytes()); // flags
        e[52..54].copy_from_slice(&64u16.to_le_bytes()); // ehsize
        e[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
        e[56..58].copy_from_slice(&2u16.to_le_bytes()); // phnum
        e[58..60].copy_from_slice(&64u16.to_le_bytes()); // shentsize
        e[60..62].copy_from_slice(&0u16.to_le_bytes()); // shnum
        e[62..64].copy_from_slice(&0u16.to_le_bytes()); // shstrndx
    }

    page
}

// ============================================================================
// Init
// ============================================================================

/// Build the vDSO ELF code page and prime the data page. Called once
/// during boot (single-threaded) before the first execve.
pub fn vdso_init() {
    // SAFETY: boot-time single-threaded init; the code page is only read
    // afterwards (user mappings are R+X), and the data page writes are
    // seqlock-guarded.
    unsafe {
        VDSO_CODE_PAGE.0 = build_code_page();
    }
    // Prime the snapshot so pre-first-tick readers see sane values.
    vdso_data_tick();
}


// ════════════════ unwatch ═════════════════════════════
// (was unwatch.rs)
// hp89 hunt: post-hoc attribution ring for every PTE-clearing event.
//
// The mallocng-corruption signature (hp88m/hp89a/hp89c: healthy victim
// PTEs, singly-referenced non-ring anon frames, ZEROED data, no mm
// syscall of the victim tgid covering the VA) needs SOMEONE to run the
// full unmap sequence (rmap-remove + put_page + PTE clear) against a
// live mapping — or a frame free+re-sale the descriptors cannot see.
// The syscall-driven paths are exonerated by the per-task mm_ops ring
// (GP-MMH prints); what remains are the non-syscall PTE clearers:
// `unmap_pages` (munmap/brk-shrink/mremap/madvise engine — the mm and
// range are known even when the caller is not a syscall of the victim
// tgid), the dying-mm teardown (`free_user_page_tables` — the fork
// child exit face), and the rmap walkers (`try_to_unmap*` — the
// reclaim/migration face).
//
// This ring records every such event (kind, pid, key addresses, leaf
// count) into a small fixed array under one spinlock — NO printk on
// the record path (serial spam provably closes the race being hunted,
// hp88f/i). The #GP forensics (trap.rs gp_mmhistory_dump) prints the
// tail at fault time; correlating the victim VAs/PFNs against the
// entries names the clearing path in one corrupting boot.

/// Event kinds.
pub const KIND_UNMAP_PAGES: u8 = 1;
pub const KIND_MM_TEARDOWN: u8 = 2;
pub const KIND_TRY_TO_UNMAP: u8 = 3;
pub const KIND_LEAF_REPLACE: u8 = 4;

const RING_LEN: usize = 256;

// The ring's record UnEvent is defined in mm/types.rs (plain atomics: a
// concurrent reader may see a partially-updated event under contention,
// which is acceptable for forensics — LOCK still serializes writers).

static RING: [UnEvent; RING_LEN] = [const {
    UnEvent {
        kind: AtomicU64::new(0),
        pid: AtomicU64::new(0),
        a: AtomicU64::new(0),
        b: AtomicU64::new(0),
        cnt: AtomicU64::new(0),
        j: AtomicU64::new(0),
    }
}; RING_LEN];

static CURSOR: AtomicUsize = AtomicUsize::new(0);
static LOCK: crate::sync::spinlock::Spinlock<()> = crate::sync::spinlock::Spinlock::new(());

/// Record one PTE-clearing event. Cheap (one short spinlock section);
/// never prints.
pub fn note(kind: u8, a: u64, b: u64, cnt: u64) {
    let pid = crate::process::hotpath::current_pid() as u64;
    let j = crate::timer::get_jiffies();
    let idx = CURSOR.fetch_add(1, Ordering::Relaxed) % RING_LEN;
    let _g = LOCK.lock_irqsave();
    let e = &RING[idx];
    e.kind.store(kind as u64, Ordering::Relaxed);
    e.pid.store(pid, Ordering::Relaxed);
    e.a.store(a, Ordering::Relaxed);
    e.b.store(b, Ordering::Relaxed);
    e.cnt.store(cnt, Ordering::Relaxed);
    e.j.store(j, Ordering::Relaxed);
}

/// Dump the ring's tail (newest first), bounded — called from #GP
/// forensics only (post-hoc; the hunt window has already closed).
pub fn dump_tail(n: usize) {
    let cur = CURSOR.load(Ordering::Relaxed);
    let total = cur.min(RING_LEN);
    let n = n.min(total);
    let _g = LOCK.lock_irqsave();
    for k in 0..n {
        let idx = (cur + RING_LEN - 1 - k) % RING_LEN;
        let e = &RING[idx];
        let kind = e.kind.load(Ordering::Relaxed);
        if kind == 0 {
            continue;
        }
        let name = match kind {
            1 => "unmap_pages",
            2 => "mm_teardown",
            3 => "try_to_unmap",
            4 => "leaf_replace",
            _ => "?",
        };
        crate::pr_err!(
            "UNWATCH: #{:<4} {} pid={} a={:#x} b={:#x} cnt={} j={}",
            cur - k,
            name,
            e.pid.load(Ordering::Relaxed),
            e.a.load(Ordering::Relaxed),
            e.b.load(Ordering::Relaxed),
            e.cnt.load(Ordering::Relaxed),
            e.j.load(Ordering::Relaxed)
        );
    }
}

/// Whether any recorded unmap_pages event's [a, b) range covers `va`,
/// and if so PRINT up to `max_show` of the covering events (kind, pid,
/// range, jiffies — the attribution the #GP forensics needs). Called
/// from #GP forensics for each victim pointer.
pub fn covered_by_unmap(va: u64) -> bool {
    let cur = CURSOR.load(Ordering::Relaxed);
    let total = cur.min(RING_LEN);
    let mut shown = 0;
    let mut hit = false;
    let _g = LOCK.lock_irqsave();
    for k in 0..total {
        let idx = (cur + RING_LEN - k) % RING_LEN;
        let e = &RING[idx];
        if e.kind.load(Ordering::Relaxed) == KIND_UNMAP_PAGES as u64
            && va >= e.a.load(Ordering::Relaxed)
            && va < e.b.load(Ordering::Relaxed)
        {
            hit = true;
            if shown < 4 {
                crate::pr_err!(
                    "UNWATCH-COVER: va={:#x} event #{} unmap_pages pid={} a={:#x} b={:#x} cnt={} j={} (age {}/100 s)",
                    va,
                    cur - k,
                    e.pid.load(Ordering::Relaxed),
                    e.a.load(Ordering::Relaxed),
                    e.b.load(Ordering::Relaxed),
                    e.cnt.load(Ordering::Relaxed),
                    e.j.load(Ordering::Relaxed),
                    crate::timer::get_jiffies().saturating_sub(e.j.load(Ordering::Relaxed))
                );
                shown += 1;
            }
        }
    }
    hit
}


// ════════════════ gup ═════════════════════════════════
// (was gup.rs)
// Pin user pages for direct I/O — the reference kernel's
// `pin_user_pages` + `bio_iov_iter_get_pages` shape (GUP-DIO lane,
// 2026-11-18; reference-audit missing-optimization B2).
//
// The audit's two-sided attribution put a +34.5 µs/op copyout on the
// 1 MiB direct read and ~56 µs staging on the write side: every O_DIRECT
// transfer staged through a kernel buffer (alloc + full-data copy in,
// DMA, full-data copy out). The reference instead pins the user pages
// backing the request and builds the bio segments directly from the
// pinned frames — zero copies. This module is that pin:
//
// - [`pin_user_pages`] walks the CURRENT task's page tables
//   (`PageTableWalker::walk` under the mm's PTE lock, INV-LOCK-7),
//   faulting each page in first through the mm fault handler (the same
//   discipline the ptrace uaccess path uses), and takes a page
//   refcount on every page that resolves cleanly (`Page::get_page`).
//   The refcount pin is what makes the frame DMA-safe: reclaim only
//   takes exclusively-owned pages (vmscan's `refcount() == 1` gate),
//   and an unmap racing the pin is serialized by the same PTE lock the
//   unmap path takes (munmap unlinks under INV-LOCK-7).
// - For write-side pins the fault is driven with WRITE (COW-resolved
//   through `handle_cow_fault`, exactly the kernel-uaccess trap path),
//   so the mapped frame is a private, writable instance: a later fork
//   write-protects the mapping and any sharer's store COWs away from
//   our frame — the DMA reads a stable snapshot, the reference's
//   FOLL_WRITE semantics.
// - A page that cannot be faulted/pinned (unmapped range, OOM,
//   protection) is recorded as a HOLE in the result; the caller's
//   per-page fallback discipline serves the bytes through the
//   existing staged paths for exactly those pages. Pinning never
//   fails a request that the staged path could have served.
//
// The frames are exposed to the block layer as kernel direct-map
// aliases (`phys_to_virt`): `virt_to_phys` in the virtio submit path
// translates them back to the user frames, and the existing SG chain
// machinery (bio's WriteBatch / the batched direct reader) DMAs
// straight into/out of user memory. The pin is dropped (put_page) at
// request completion — `PinnedUserPages::drop`.

impl PinnedUserPages {
    /// User VA of the first byte.
    pub fn va(&self) -> usize {
        self.va
    }

    /// Byte length of the pinned request.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Byte offset of the range start inside its first page.
    fn head_off(&self) -> usize {
        self.va & (PAGE_SIZE - 1)
    }

    /// Page index (into `pages`) covering request offset `off`.
    fn page_of(&self, off: usize) -> usize {
        (self.head_off() + off) / PAGE_SIZE
    }

    /// Kernel direct-map alias of the piece of page `pi` that intersects
    /// `[off, off+n)`, and the piece's length. `off..off+n` must lie
    /// within one page.
    fn piece(&self, pi: usize, off: usize, n: usize) -> (*mut u8, usize) {
        let start = self.head_off() + off;
        let in_page = start - pi * PAGE_SIZE;
        let frame = self.pages[pi].unwrap();
        let kva = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr(frame)).0 as usize;
        (kva.wrapping_add(in_page) as *mut u8, n)
    }

    /// Iterate the pinned pieces covering request range `[off, off+n)`:
    /// yields `(kernel_alias_ptr, piece_len)` for every pinned piece,
    /// skipping unpinned pages. A piece never crosses a page boundary.
    /// `visit` returning false stops the walk. Returns the number of
    /// PINNED bytes visited (callers use it to detect holes without a
    /// second pass).
    ///
    /// # Safety
    /// Callers must not hold the returned pointers beyond the pin's
    /// lifetime (the `PinnedUserPages` borrow).
    pub fn for_each_piece<F: FnMut(*mut u8, usize) -> bool>(
        &self,
        off: usize,
        n: usize,
        mut visit: F,
    ) -> usize {
        debug_assert!(off + n <= self.len);
        let end = off + n;
        let mut done = 0usize;
        let mut o = off;
        while o < end {
            let pi = self.page_of(o);
            // Request offset where the current page ends (clipped to
            // the range end): pieces never cross page boundaries.
            let page_first_off = pi * PAGE_SIZE - self.head_off();
            let o_page_end = (page_first_off + PAGE_SIZE).min(end);
            let piece_len = o_page_end - o;
            if self.pages[pi].is_some() {
                let (p, l) = self.piece(pi, o, piece_len);
                if !visit(p, l) {
                    return done;
                }
                done += l;
            }
            o = o_page_end;
        }
        done
    }

    /// True when every byte of `[off, off+n)` lies in pinned pages —
    /// the gate for a zero-copy DMA span.
    pub fn range_pinned(&self, off: usize, n: usize) -> bool {
        let p0 = self.page_of(off);
        let p1 = self.page_of(off + n - 1);
        self.pages[p0..=p1].iter().all(|p| p.is_some())
    }

    /// Next pinned piece at request offset `*off` (before `end`):
    /// returns `(alias_ptr, len)` and advances `*off` past the piece.
    /// Unpinned pages are skipped. CONSECUTIVE PHYSICALLY-ADJACENT
    /// pinned pages are merged into ONE piece — the SG segment count
    /// then tracks physical contiguity (a fresh buddy-backed 1 MiB
    /// window is a handful of segments, not 256), which is what keeps
    /// the descriptor footprint at the staging path's shape. Returns
    /// None when no pinned piece remains before `end`.
    ///
    /// # Safety
    /// Same lifetime contract as `for_each_piece`.
    pub fn next_piece(&self, off: &mut usize, end: usize) -> Option<(*mut u8, usize)> {
        // Skip unpinned pages.
        let mut o = *off;
        loop {
            if o >= end {
                return None;
            }
            let pi = self.page_of(o);
            if self.pages[pi].is_some() {
                break;
            }
            o = self.page_end_off(pi, end);
        }
        // First (possibly partial) page piece.
        let pi0 = self.page_of(o);
        let first_end = self.page_end_off(pi0, end);
        let (ptr, l0) = self.piece(pi0, o, first_end - o);
        let mut total = l0;
        let mut cur_phys = self.pages[pi0].unwrap();
        // Extend through whole pages that are pinned and physically
        // adjacent (direct-map aliases of adjacent frames are
        // contiguous, so the merged pointer covers the whole run).
        let mut o2 = first_end;
        while o2 < end {
            let pi = self.page_of(o2);
            let Some(next_phys) = self.pages[pi] else { break };
            if next_phys != cur_phys + PAGE_SIZE as u64 {
                break;
            }
            let pe = self.page_end_off(pi, end);
            cur_phys = next_phys;
            total += pe - o2;
            o2 = pe;
        }
        *off = o + total;
        Some((ptr, total))
    }

    /// Request offset where page `pi`'s intersection with `[.., end)`
    /// ends (pieces never cross page boundaries before merging).
    fn page_end_off(&self, pi: usize, end: usize) -> usize {
        let page_first_off = pi * PAGE_SIZE - self.head_off();
        (page_first_off + PAGE_SIZE).min(end)
    }

    /// Kernel alias of the first pinned piece at request offset `off`
    /// (the completion-token pointer for SG chains; `off` must be
    /// pinned).
    pub fn first_ptr(&self, off: usize) -> usize {
        let pi = self.page_of(off);
        self.piece(pi, off, 1).0 as usize
    }

    /// Copy `src` into the user frames at request offset `off`
    /// (read-side per-block serve and hole fills; unpinned pages are
    /// SKIPPED — the caller's fallback owns their bytes). All bytes
    /// must lie in pinned pages (check `range_pinned` first).
    pub fn copy_into_pinned(&self, off: usize, src: &[u8]) {
        let mut s = 0usize;
        self.for_each_piece(off, src.len(), |dst, l| {
            // The pin keeps the frame alive; the alias is a valid
            // kernel address for l bytes (INV-MM-6 pinned-alias arm).
            super::r#unsafe::copy_kva(dst, &src[s..][..l]);
            s += l;
            true
        });
        debug_assert_eq!(s, src.len());
    }

    /// Copy `n` bytes of the user frames at request offset `off` into
    /// `dst` (write-side per-block gather). Unpinned bytes read as
    /// ZERO — callers gate spans with `range_pinned` and serve
    /// unpinned blocks through the staged path instead.
    pub fn copy_from_pinned(&self, off: usize, dst: &mut [u8]) {
        let mut d = 0usize;
        self.for_each_piece(off, dst.len(), |src, l| {
            // Same alias validity; src is l readable bytes (INV-MM-6).
            super::r#unsafe::copy_kva_from(&mut dst[d..][..l], src);
            d += l;
            true
        });
        for b in dst[d..].iter_mut() {
            *b = 0;
        }
    }

    /// Number of pages pinned (holes excluded).
    pub fn pinned_pages(&self) -> usize {
        self.pinned
    }

    /// Number of pages the range covers (pinned + holes).
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Drop every pin. Idempotent (the drop guard runs it once).
    pub fn unpin(&mut self) {
        for p in self.pages.iter_mut() {
            if let Some(frame) = p.take() {
                // pfn came from the page-table walk of a mapped page;
                // each pin took exactly one reference (INV-MM-2).
                if let Some(page) = super::r#unsafe::page_of((frame >> 12) as usize) {
                    page.put_page();
                }
            }
        }
        self.pinned = 0;
    }
}

impl Drop for PinnedUserPages {
    fn drop(&mut self) {
        self.unpin();
    }
}

/// Pin the user pages backing `[va, va+len)` of the CURRENT task.
///
/// `write` selects the fault discipline: a write pin drives WRITE
/// faults (COW-resolved) and requires a writable PTE, so the pinned
/// frame is the mapper's private instance (the reference's FOLL_WRITE);
/// a read pin only requires a present PTE. Pages that cannot be
/// faulted or pinned become holes in the result — the call fails
/// outright (None) only when the task or its mm cannot be resolved.
///
/// The caller must have access_ok'd the range (the syscall layer's
/// contract) and must not run this under the ext4 big lock: the
/// fault-in path may allocate and take mm locks (see the module doc).
pub fn pin_user_pages(va: usize, len: usize, write: bool) -> Option<PinnedUserPages> {
    use crate::arch::mm::mm_ops::PageTableWalker;

    debug_assert!(len > 0);
    let task = crate::sched::hotpath::current()?;
    let mm = task.address_space_arc()?;

    let first_page = va & !(PAGE_SIZE - 1);
    let last_page = (va + len - 1) & !(PAGE_SIZE - 1);
    let npages = (last_page - first_page) / PAGE_SIZE + 1;
    let mut pages: alloc::vec::Vec<Option<u64>> = alloc::vec::Vec::with_capacity(npages);

    // ---- Pass 1 (fast path): ONE pte-lock section, walk every page ----
    // The DIO buffers of interest (fio's aligned 1 MiB windows) are
    // long-resident: the walk resolves them and the fault machinery
    // never runs. Pages that come back unwalkable here get Pass 2's
    // fault-in (below). One lock acquisition for the whole range —
    // the per-page lock/unlock pair of the first cut measured ~100 µs
    // per pinned MiB (256 pairs), the dominant GUP cost after the
    // copy savings.
    {
        use crate::arch::mm::PageTableEntry;
        let root = mm.root_ppn();
        let guard = mm.pte_lock().lock();
        for pi in 0..npages {
            let page_va = first_page + pi * PAGE_SIZE;
            // root is the pinned mm's live root; the walker
            // bounds-checks the root and every intermediate entry
            // (INV-SY-1 walk edge).
            let r = crate::sync::r#unsafe::pt_walk(root, page_va as u64);
            pages.push(match r {
                // Present but NOT writable = a COW-shared leaf (fork's
                // write-protected mapping). Pinning it directly would
                // alias the DMA with the fork child (reads into the
                // shared frame would change the child's buffer — the
                // reference COW-resolves before pinning, and so do we):
                // leave it to Pass 2, which drives the WRITE fault.
                // (A genuinely read-only VMA fails the fault there and
                // stays a hole — the staged fallback's semantics.)
                Some((ppn, bits)) if bits & PageTableEntry::W != 0 => {
                    // Descriptor of a machine frame (vmemmap-published);
                    // mapped frame (refcount >= 1) — one reference,
                    // dropped in unpin() (INV-MM-2).
                    match super::r#unsafe::page_of(ppn as usize) {
                        Some(page) => {
                            page.get_page();
                            Some(ppn << 12)
                        }
                        None => None,
                    }
                }
                _ => None,
            });
        }
        drop(guard);
    }

    // ---- Pass 2: fault-in + re-pin ONLY the holes ----
    // (Missing pages AND COW leaves — the reference's FOLL_WRITE
    // discipline applies to both DMA directions: a read DIO WRITES the
    // frames, so they must be the mapper's private instance too.)
    let _ = write;
    for pi in 0..npages {
        if pages[pi].is_some() {
            continue;
        }
        let page_va = first_page + pi * PAGE_SIZE;
        pages[pi] = pin_one_faulted(&mm, page_va);
    }

    let pinned = pages.iter().filter(|p| p.is_some()).count();
    Some(PinnedUserPages {
        va,
        len,
        pages,
        pinned,
    })
}

/// Fault a page in and pin it (the Pass-2 body for pages the walk
/// missed: demand-fault anon/file pages, COW entries). The fault is
/// driven as a WRITE fault — the pin is a DMA target (read DIO) or a
/// stable snapshot source (write DIO), and both demand the COW-resolved
/// private instance (FOLL_WRITE semantics); a genuinely read-only VMA
/// fails the fault and the page stays a hole (staged fallback).
fn pin_one_faulted(
    mm: &super::types::MmStruct,
    page_va: usize,
) -> Option<u64> {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::{handle_cow_fault, CowFaultResult, FaultFlags, MmFaultResult};

    let flags = FaultFlags::WRITE | FaultFlags::USER;
    for _ in 0..2 {
        match handle_mm_fault_checked(mm, page_va, flags) {
            MmFaultResult::Handled
            | MmFaultResult::Fixed
            | MmFaultResult::AlreadyMapped => break,
            MmFaultResult::CowPending => {
                // mm owns root_ppn; handle_cow_fault is a safe fn and
                // runs under the PTE section itself (INV-LOCK-7) — the
                // old wrapper was vestigial.
                let r = handle_cow_fault(
                    mm,
                    mm.root_ppn(),
                    crate::arch::mm::VirtAddr::new(page_va as u64),
                );
                match r {
                    CowFaultResult::Resolved | CowFaultResult::Retry => continue,
                    CowFaultResult::OutOfMemory => return None,
                }
            }
            // Segfault/PermissionDenied/OOM/BusError: leave the page a
            // hole — the staged fallback serves it (or reports the
            // same error it would have).
            _ => return None,
        }
    }

    // ---- Walk + pin under the PTE lock ----
    // The lock serializes against munmap's unlink and vmscan's
    // leaf_pte_replace; a present leaf here stays backed by its frame
    // until our get_page lands.
    //
    // Write-pin note: a WRITE fault that resolves to a read-only leaf
    // (a PROT_READ mapping) still pins the mapped frame — it is a
    // stable DMA SOURCE (a read-only mapping has no writable alias a
    // fork could COW away from). The reference's FOLL_WRITE would
    // fail here; we keep the staged path's more permissive semantics
    // instead of changing observable behavior.
    let root = mm.root_ppn();
    let guard = mm.pte_lock().lock();
    // root is the pinned mm's live root; the walker bounds
    // checks the root and every intermediate entry (INV-SY-1 walk
    // edge).
    let (ppn, _bits) = crate::sync::r#unsafe::pt_walk(root, page_va as u64)?;
    // Descriptor of a machine frame (vmemmap-published); the frame is
    // mapped (refcount >= 1) — the pin takes one reference, dropped in
    // unpin() (INV-MM-2).
    let page = super::r#unsafe::page_of(ppn as usize)?;
    page.get_page();
    drop(guard);
    Some(ppn << 12)
}

/// handle_mm_fault wrapper keeping the arch re-export surface uniform.
#[inline]
fn handle_mm_fault_checked(
    mm: &super::types::MmStruct,
    va: usize,
    flags: u32,
) -> crate::arch::mm::MmFaultResult {
    crate::api::mm::handle_mm_fault(mm, crate::arch::mm::VirtAddr::new(va as u64), flags)
}


// ════════════════ pcp ═════════════════════════════════
// (was pcp.rs)
// ==================== Per-CPU pagesets (pcp) ====================
//
// Order-0 frame caching per CPU, mirroring the reference kernel's
// zone->per_cpu_pageset: on the alloc/free fast path a frame moves
// between the pcp array and the caller with NO zone-lock acquisition;
// the zone lock is paid once per PCP_BATCH frames (refill/drain).
//
// Why this exists (perf review 2026-10-31): the wave-6 cleanup removed
// an earlier never-wired pcp module, leaving every order-0 alloc/free
// a full round trip on the (effectively global — one zone) zone lock,
// with the page-descriptor init loop of alloc_pages_locked inside it.
// Page faults, COW, page-cache fills and mm teardown all serialize on
// that single lock. This module re-introduces the pageset with a
// design that fits Rux's invariants, where the removed one did not:
//
// INV-PCP-1 (three-state frame lifecycle). Every order-0 managed frame
//   is in exactly one of: buddy freelist (OnFreelist, rc==0), pcp cache
//   (OnPcp, rc==0), or in use (neither bit, rc>=1). The pcp paths
//   maintain this by construction and refuse (leak + taint) any free
//   that would violate it.
//
// INV-PCP-2 (counting). Zone.free_pages counts ALL free frames — buddy
//   lists AND pcp caches (the reference-kernel semantics, and what
//   watermark checks want). pcp entry adds 1, pcp exit subtracts 1;
//   refill/drain round trips through the zone net to zero on the pcp
//   side, so the zone lock paths' existing +/- stay untouched.
//
// INV-PCP-3 (cpu locality = safety). A pcp row is touched only by its
//   own CPU with interrupts disabled; nested entry (interrupt handler
//   allocating/freeing while the interrupted context is mid-pcp) is
//   SAFE by design — same-CPU nesting is serial under irq-off, so
//   unlike CpuLocal this module has no in_use refusal and no taint on
//   nesting. The only atomicity needed is the row's own count, which
//   the irq-off window makes exclusive.
//
// SAFETY (static rows): rows are addressed only by cpu_id() with IRQs
// disabled (same-CPU exclusivity above); cross-CPU access happens only
// via the count peeks, which read a single usize through a raw pointer
// — never a &mut. No `&'static mut` escapes this module (the audit
// failure that killed the wave-6 module).

// PcpPageset, its storage row PcpRow and the row high-water mark are
// defined in mm/types.rs; the pageset's impl and the refill/drain
// functions stay here. Re-exported so the `mm::pcp::…` paths (the
// raw-row edges in mm::unsafe among them) are unchanged.

impl PcpPageset {
    const fn new() -> Self {
        Self {
            list: [0; PCP_HIGH],
            count: 0,
        }
    }
}

/// Sync proof + raw-row edges live in mm::unsafe (INV-MM-5).

pub(super) static PCP_ROWS: [PcpRow; MAX_CPUS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ROW: PcpRow = PcpRow(UnsafeCell::new(PcpPageset::new()));
    [ROW; MAX_CPUS]
};


// Observability: fast-path hit/miss and drain/refill round trips.
pub static PCP_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PCP_MISSES: AtomicUsize = AtomicUsize::new(0);
pub static PCP_REFILLS: AtomicUsize = AtomicUsize::new(0);
pub static PCP_DRAINS: AtomicUsize = AtomicUsize::new(0);
/// Double-free attempts against a pcp-cached frame (INV-PCP-1 tripwire).
pub static PCP_DOUBLE_FREE: AtomicUsize = AtomicUsize::new(0);

/// Run `f` with exclusive access to this CPU's row (IRQs off).
fn with_row<R>(f: impl FnOnce(&mut PcpPageset) -> R) -> Option<R> {
    // SAFETY: irq-off window grants same-CPU exclusivity (INV-PCP-3);
    // nested entry is serial and safe, so no in_use guard is needed.
    let flags = crate::arch::cpu::save_and_disable_irq();
    let c = crate::arch::cpu_id() as usize;
    let out = if c < MAX_CPUS {
        // UnsafeCell::get() hands out the raw row pointer; the &mut
        // derived from it is exclusive because only this CPU can touch
        // the row and interrupts are disabled (INV-PCP-3 / INV-MM-5
        // exclusive arm, via the edge).
        Some(super::r#unsafe::pcp_row_exclusive(c, f))
    } else {
        None
    };
    crate::arch::cpu::restore_irq(flags);
    out
}

/// Allocate one order-0 frame, pcp first.
///
/// `zone` is the caller's resolved ZoneNormal (from alloc_pages_inner).
/// On pcp miss this refills the row with one zone-lock batch and hands
/// out the first frame; on refill failure (zone exhausted) returns None
/// so the caller falls through to the slow path (kswapd / direct
/// reclaim) unchanged.
pub fn alloc_order0(zone: &Zone) -> Option<usize> {
    let got = with_row(|row| {
        if row.count > 0 {
            row.count -= 1;
            let pfn = row.list[row.count];
            row.list[row.count] = 0;
            Some(pfn)
        } else {
            None
        }
    });
    if let Some(Some(pfn)) = got {
        PCP_HITS.fetch_add(1, Ordering::Relaxed);
        // INV-PCP-2: this frame was counted free while cached.
        zone.free_pages_sub(1);
        to_in_use(pfn);
        return Some(pfn);
    }
    PCP_MISSES.fetch_add(1, Ordering::Relaxed);

    // Refill: one zone-lock acquisition initializes PCP_BATCH frames
    // (alloc_pages_locked semantics — the MM-H2 invariant holds because
    // init stays inside the zone lock).
    let mut batch = [0usize; PCP_BATCH];
    let n = zone.alloc_pages_order0_batch(PCP_BATCH, &mut batch);
    if n == 0 {
        return None;
    }
    PCP_REFILLS.fetch_add(1, Ordering::Relaxed);
    let pfn = batch[0];
    // Overflow list: frames that no longer fit (an interrupt-context
    // free_order0 storm between the miss window and this refill window
    // can leave count>0 — up to PCP_HIGH after the spill fixup). Rows
    // that fit go to the row; the rest return to the zone directly,
    // keeping every list index < PCP_HIGH (the wfG panic class).
    let mut overflow = [0usize; PCP_BATCH];
    let mut n_overflow = 0usize;
    let keep = n - 1;
    with_row(|row| {
        for i in 0..keep {
            let p = batch[1 + i];
            if row.count < PCP_HIGH {
                row.list[row.count] = p;
                row.count += 1;
                to_cached(p);
            } else {
                overflow[n_overflow] = p;
                n_overflow += 1;
            }
        }
    });
    if n > 1 {
        // INV-PCP-2: n-1 frames are back to "free" (cached) state; the
        // handed-out frame was already decremented by alloc_pages_locked.
        zone.free_pages_add(n - 1);
    }
    if n_overflow > 0 {
        // The overflow frames were counted free above; return them to
        // the buddy now (they were never cached in a row).
        zone.free_pages_sub(n_overflow);
        for i in 0..n_overflow {
            if let Some(pg) = page_desc(overflow[i]) {
                            pg.set_refcount(1);
                pg.clear_flag(PageFlag::OnPcp);
            }
            zone.free_pages(overflow[i], 0);
        }
    }
    to_in_use(pfn);
    Some(pfn)
}

/// Drain up to `n` frames from this CPU's pcp row in ONE irq-off window
/// (BUFW-PARITY: the batch twin of alloc_order0 for the page-cache bulk
/// insert path — a 1 MiB buffered write asks for 256 frames; serving the
/// row's contents from the row avoids the zone round trip for every
/// frame a recent free cached). Same state transitions and accounting as
/// N calls to alloc_order0: INV-PCP-2 (each handed-out frame was counted
/// free while cached -> free_pages_sub(k)) and to_in_use per frame.
/// Returns how many were served; the caller falls to the zone bulk path
/// for the remainder. No refill here — the single-frame path refills
/// lazily.
pub fn alloc_order0_batch(zone: &Zone, n: usize, out: &mut [usize]) -> usize {
    if n == 0 || out.is_empty() {
        return 0;
    }
    let want = n.min(out.len());
    let k = with_row(|row| {
        let avail = row.count.min(want);
        for i in 0..avail {
            out[i] = row.list[row.count - 1 - i];
        }
        row.count -= avail;
        avail
    })
    .unwrap_or(0);
    if k == 0 {
        PCP_MISSES.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    PCP_HITS.fetch_add(k, Ordering::Relaxed);
    // INV-PCP-2: the k frames were counted free while cached.
    zone.free_pages_sub(k);
    for pfn in out.iter_mut().take(k) {
        to_in_use(*pfn);
    }
    k
}

/// Free one order-0 frame into this CPU's pcp row.
///
/// Returns true when the frame was cached (caller is done); false means
/// "not eligible for the pcp" — the caller falls through to the zone
/// path unchanged. A detected double free is leaked + tainted here and
/// NOT passed to the zone (the buddy has no view of pcp frames).
pub fn free_order0(pfn: usize, zone: &Zone) -> bool {
    let page = pfn_to_page_mut(pfn);
    if page.is_null() {
        return false;
    }
    // Descriptor transitions below are the pcp ownership transfer for
    // this frame; exclusivity comes from INV-PCP-1: only one of the
    // three states owns the frame, and every state change validates
    // the previous state first (INV-MM-2 via the edge).
    {
        let page: &mut _ = super::r#unsafe::page_of(pfn).unwrap();
        // Textbook in-use state only; anything else goes to the zone
        // slow path whose tripwires own the diagnosis.
        if page.refcount() != 1
            || page.test_flag(PageFlag::OnFreelist)
            || page.order() != 0
        {
            return false;
        }
        // LIVE-STAMP REFUSAL (mmforensics lane, 2026-11-15): the pcp fast
        // path used to be the INVISIBLE first free — a frame still
        // carrying a LIVE page-table ledger stamp entered the row with no
        // zone-level check, and only its much-later drain printed the
        // RAW-FREE-OF-LIVE-TABLE echo (by then the frame had already been
        // re-handed to a second owner; capture evidence: pfn 0x6bd1,
        // fallocate06 SIGSEGV). A live-stamped frame has NO legitimate
        // raw free (legit table frees clear the stamp via
        // PtLedger::take() before reaching here), so refuse, quarantine,
        // and leak. Silent + taint + counter (free contexts hold locks;
        // printk here can wedge the console — INV-PCP-3's irq-off window
        // is near). The mmforensics capture (switch-gated) still names
        // the frame on forensic boots.
        if crate::arch::mm::mmu_init::alloc_stage_is_late() {
            let ppn = pfn as u64;
            if crate::arch::mm::mmu_init::PtLedger::is_live_table(ppn) {
                crate::arch::mm::mmu_init::PtLedger::poison_table(ppn);
                crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
                crate::debug::driftwatch::MM_POISONED.fetch_add(1, Ordering::Relaxed);
                crate::debug::mmforensics::capture_frame(pfn, "pcp-raw-free-live-table");
                return true; // handled: leaked, never cached
            }
            if crate::arch::mm::mmu_init::PtLedger::is_poisoned(ppn) {
                crate::debug::driftwatch::MM_POISON_FREES.fetch_add(1, Ordering::Relaxed);
                return true; // handled: quarantined frames never re-enter
            }
        }
        if page.test_flag(PageFlag::OnPcp) {
            // Double free of a pcp-cached frame. Leak it (stay cached,
            // never hand out twice) and name it, mirroring the buddy's
            // R15-3 stance. No printk from this path (free contexts hold
            // locks; floods wedge the console — sc83 discipline).
            PCP_DOUBLE_FREE.fetch_add(1, Ordering::Relaxed);
            crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::BAD_PAGE);
            return true;
        }
        // In use -> cached.
        page.set_refcount(0);
        page.clear_flag(PageFlag::Referenced);
        page.set_flag(PageFlag::OnPcp);
    }
    // INV-PCP-2: the frame is free again (cached).
    zone.free_pages_add(1);

    // Spill handling is ONE irq-off window: detect the high-water mark,
    // copy the drain batch out, and fix `count` up atomically. The old
    // two-window form (detect, then drain in a second with_row) left
    // `count == PCP_HIGH == list.len()` visible with IRQs enabled
    // between the windows; an interrupt-context free_order0 in that gap
    // executed `row.list[row.count] = pfn` with count==32 — the release
    // OH boot wfG panic "index out of bounds: the len is 32 but the
    // index is 32" at 108.6 s. Zone-side accounting (free_pages_sub +
    // the 16 zone.free_pages calls) stays OUTSIDE the window exactly as
    // before — only the 16 usize copies moved inside, and a nested
    // same-CPU entry remains serial (INV-PCP-3).
    //
    // lb250 (2026-11-02, verify roller): the 35336eb1 refill guard can
    // leave count == PCP_HIGH EXACTLY (its stores stop at the bound, so
    // a 31-count row refilled to full parks ON 32 with no drain pending
    // — the intended spill never runs because that store is not a
    // free_order0 entry). This entry store must therefore be bound
    // checked too: a full row spills the OLDEST PCP_BATCH out of the
    // list and sends the INCOMING frame straight to the buddy (its +1
    // accounting and OnPcp transition already happened above, so it
    // joins the drain accounting as +1/-1, not a row member).
    let mut pfns = [0usize; PCP_BATCH];
    let mut do_drain = false;
    let mut spill_direct = false;
    with_row(|row| {
        if row.count < PCP_HIGH {
            row.list[row.count] = pfn;
            row.count += 1;
        } else {
            // Row left full by a capped refill: the incoming frame
            // bypasses the list (spill_direct below).
            spill_direct = true;
        }
        if row.count >= PCP_HIGH {
            let first = row.count - PCP_BATCH;
            for i in 0..PCP_BATCH {
                pfns[i] = row.list[first + i];
            }
            row.count = first;
            do_drain = true;
        }
    });

    if do_drain {
        // Drain PCP_BATCH frames back to the buddy: undo the pcp-side
        // +1 per frame, then let zone.free_pages re-add them (its own
        // merge/list logic untouched).
        PCP_DRAINS.fetch_add(1, Ordering::Relaxed);
        zone.free_pages_sub(PCP_BATCH);
        for p in pfns.iter() {
            // SAFETY: frame was cached (OnPcp) — transition back through
            // the zone's own free path, which relinks and merges it.
            if let Some(pg) = page_desc(*p) {
                            pg.clear_flag(PageFlag::OnPcp);
            }
            zone.free_pages(*p, 0);
        }
    }
    if spill_direct {
        // The incoming frame never entered the row: undo its pcp-side
        // +1 and hand it to the zone directly (same transition shape as
        // the drain loop's frames).
        PCP_DRAINS.fetch_add(1, Ordering::Relaxed);
        zone.free_pages_sub(1);
        if let Some(pg) = page_desc(pfn) {
                    pg.clear_flag(PageFlag::OnPcp);
        }
        zone.free_pages(pfn, 0);
    }
    true
}

/// Descriptor transition: cached (OnPcp, rc==0). Caller guarantees the
/// frame is in the "in use" or "just allocated" state.
fn to_cached(pfn: usize) {
    if let Some(page) = page_desc(pfn) {
        // SAFETY: refill batch frames come from alloc_pages_locked in
        // the in-use state (rc==1, Referenced); this flips them to the
        // pcp-cached free state.
            page.set_refcount(0);
        page.clear_flag(PageFlag::Referenced);
        page.set_flag(PageFlag::OnPcp);
    }
}

/// Descriptor transition: in use — bit-identical to the state
/// alloc_pages_locked produces for an order-0 frame.
fn to_in_use(pfn: usize) {
    if let Some(page) = page_desc(pfn) {
        // SAFETY: pop from this CPU's row under irq-off; the frame was
        // cached (OnPcp, rc==0).
            page.clear_all_flags();
        page.reset_mapcount();
        page.set_refcount(1);
        // SP-7 (stress-port r7): no Referenced flag on handout (see
        // the zone.rs alloc-path note — the reference's prep_new_page
        // produces flag-clear pages; the plant made every PCP-served
        // order-0 fault page look hot to reclaim).
        page.set_order(0);
    }
}

/// Total frames cached across all pcp rows (meminfo / forensic dumps).
pub fn cached_pages_total() -> usize {
    let mut total = 0usize;
    for c in 0..MAX_CPUS {
        // Read-only peek of a single usize — no reference is formed; a
        // moment-stale value is acceptable for reporting (INV-MM-5 peek
        // arm).
        total += super::r#unsafe::pcp_row_count_peek(c);
    }
    total
}

/// Per-CPU cached counts for /proc meminfo's pcp_pages row.
pub fn cached_pages_per_cpu() -> [usize; MAX_CPUS] {
    let mut out = [0usize; MAX_CPUS];
    for c in 0..MAX_CPUS {
        // Read-only single-usize peek, see cached_pages_total
        // (INV-MM-5 peek arm).
        out[c] = super::r#unsafe::pcp_row_count_peek(c);
    }
    out
}

/// mmforensics capture support: which CPU rows currently hold `pfn`.
///
/// Racy by design (diagnostic only): rows are read through raw pointers
/// without the owning CPU's irq-off window, mirroring the
/// `cached_pages_total` peeks. Writes the cpu ids of hits into `out`
/// (capped) and returns the total number of rows holding the frame.
pub fn forensics_scan_rows(pfn: usize, out: &mut [usize]) -> usize {
    let mut total = 0usize;
    for c in 0..MAX_CPUS {
        // Read-only peek of the row's list/count — no reference escapes;
        // moment-stale values are fine for a forensic snapshot
        // (INV-MM-5 peek arm).
        let r = super::r#unsafe::pcp_row_ref_peek(c);
        let count = r.count.min(PCP_HIGH);
        let mut hit = false;
        for i in 0..count {
            if r.list[i] == pfn {
                hit = true;
                break;
            }
        }
        if hit {
            if total < out.len() {
                out[total] = c;
            }
            total += 1;
        }
    }
    total
}
