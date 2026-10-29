//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buddy System Memory Allocator
//!
//! Improved version: Metadata and user data are stored separately to prevent BlockHeader corruption
//!
//! # Safety Invariants — Kernel Heap Buddy Free List
//!
//! This is the `#[global_allocator]` buddy for kernel heap allocations.
//! The same INV-BUDDY-1 through INV-BUDDY-5 invariants from `zone.rs` apply,
//! with `BlockMeta.free == 1` replacing `_refcount == 0` as the free indicator
//! and `BlockMeta.order` as the per-page order field.
//!
//! Additionally:
//!
//! - **INV-HEAP-1**: `BlockMeta.prev`/`next` are valid page indices (< MAX_PAGES)
//!   or `u32::MAX` (null sentinel). No truncation occurs — fields are `u32`.
//!
//! - **INV-HEAP-2**: `free_lists[order]` is either `EMPTY_LIST` or a valid
//!   page index within `[0, MAX_PAGES)`.
//!
//! - **INV-HEAP-3** (session 5, wedge fix): the per-page FREE bit lives in
//!   `free_bits`, a word-strided bitmap; `BlockMeta.free` is authoritative
//!   for LIST LEADERS only (interior pages' byte stays stale). The bitmap
//!   is the exact per-page free state and is what the R9-14 double-free
//!   check consults. Interior pages' `order` bytes are maintained by a
//!   tight raw-pointer fill (report fidelity for the ORDER_MISMATCH arm);
//!   they carry the order of the block they currently belong to.
//!
//!   Why: the old mark-every-page loops ran `meta.get_mut()` (bounds
//!   assert + UnsafeCell call) per page inside `add_to_free_list` /
//!   `remove_from_free_list` / `init_block`, all under the single
//!   irqsave heap lock. On a 1 GiB heap (2^18 pages, seeded as one
//!   order-18 block) an order-scale add/remove walked 262144 heavy
//!   iterations per level; gdb on the ww-v2 wedge caught cpu1 grinding
//!   exactly that loop under tcp_timer softirq while cpu0 spun on the
//!   same lock in the alloc path (terminal 15+ min system freeze,
//!   session-4 record). The bitmap fill is 4096 word ops for the same
//!   block; the order fill is a call-free byte loop.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::spinlock::Spinlock;

const PAGE_SIZE: usize = 4096;

// Use config value for max order
const MAX_ORDER: usize = crate::config::BUDDY_MAX_ORDER;

const MIN_ORDER: usize = 0;

// Hardcoded heap start address for early boot.
// Uses phys_to_virt() to get the virtual address in the linear mapping region.
// riscv64: PAGE_OFFSET 0xffffffd600000000 over PHYS base 0x80000000.
// x86_64: linear map at 0xffff888000000000 over phys 0; heap placed at 1GB
// (clear of the image window 0x200000..0xa00000 and the q35 ECAM hole).
#[cfg(feature = "riscv64")]
const KERNEL_HEAP_PHYS: usize = 0x8160_0000;
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
const HEAP_SIZE: usize = crate::config::KERNEL_HEAP_SIZE;

/// Maximum number of pages (for metadata array size)
const MAX_PAGES: usize = HEAP_SIZE / PAGE_SIZE;

/// Words in the per-page free bitmap (INV-HEAP-3).
const FREE_BITMAP_WORDS: usize = (MAX_PAGES + 63) / 64;

/// Empty list marker (use out-of-range value)
const EMPTY_LIST: usize = MAX_PAGES + 1;

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
pub static NET_PAGES: [AtomicUsize; MAX_ORDER + 1] =
    [const { AtomicUsize::new(0) }; MAX_ORDER + 1];

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


/// Block metadata (stored separately, not mixed with user data)
#[repr(C)]
#[derive(Clone, Copy)]
struct BlockMeta {
    /// Block size order (2^order * PAGE_SIZE)
    order: u8,
    /// Whether the block is free
    free: u8,
    /// Previous index (index in metadata array, u32::MAX means null)
    prev: u32,
    /// Next index (index in metadata array, u32::MAX means null)
    next: u32,
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

/// Metadata array wrapper (uses UnsafeCell for interior mutability)
struct MetaArray {
    data: UnsafeCell<[BlockMeta; MAX_PAGES]>,
}

// SAFETY: MetaArray is only accessed through BuddyAllocator, which holds a
// Spinlock.  All get/get_mut calls happen under that lock, preventing
// concurrent mutable access to the underlying array.
unsafe impl Send for MetaArray {}
unsafe impl Sync for MetaArray {}

impl MetaArray {
    const fn new() -> Self {
        Self {
            data: UnsafeCell::new([const { BlockMeta::new() }; MAX_PAGES]),
        }
    }

    /// Get metadata reference
    fn get(&self, idx: usize) -> &BlockMeta {
        assert!(idx < MAX_PAGES, "buddy allocator: metadata index {} out of range (MAX_PAGES={})", idx, MAX_PAGES);
        // SAFETY: idx is bounds-checked above; caller holds BuddyAllocator lock.
        unsafe { &(*self.data.get())[idx] }
    }

    /// Get mutable metadata reference
    fn get_mut(&self, idx: usize) -> &mut BlockMeta {
        assert!(idx < MAX_PAGES, "buddy allocator: metadata index {} out of range (MAX_PAGES={})", idx, MAX_PAGES);
        // SAFETY: idx is bounds-checked above; caller holds BuddyAllocator lock.
        unsafe { &mut (*self.data.get())[idx] }
    }
}

pub struct BuddyAllocator {
    /// Magic number (for corruption detection)
    magic: AtomicUsize,
    /// Heap start address (user data area)
    heap_start: AtomicUsize,
    /// Heap end address
    heap_end: AtomicUsize,
    /// Free block lists (one list per order, stores page indices)
    free_lists: [AtomicUsize; MAX_ORDER + 1],
    /// Whether initialized
    initialized: AtomicUsize,
    /// Metadata area (stores metadata for each page)
    meta: MetaArray,
    /// Per-page free bitmap (INV-HEAP-3): bit (page_idx % 64) of word
    /// (page_idx / 64). Maintained by the same locked paths that used to
    /// fill the per-page `BlockMeta.free` bytes; word-strided so an
    /// order-scale fill costs 2^order/64 word RMWs instead of 2^order
    /// per-page `get_mut` iterations under the irqsave lock.
    free_bits: [AtomicU64; FREE_BITMAP_WORDS],
    /// Lock for buddy operations
    lock: Spinlock<()>,
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

    pub const fn new() -> Self {
        Self {
            magic: AtomicUsize::new(0xDEADBEEF),
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            free_lists: [const { AtomicUsize::new(0) }; MAX_ORDER + 1],
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
            for i in 0..=MAX_ORDER {
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
                for o in (0..=MAX_ORDER).rev() {
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
        debug_assert!(page_idx < MAX_PAGES);
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
            page_idx + npages <= MAX_PAGES,
            "buddy fill_free_bits out of range ({}+{} > MAX_PAGES={})",
            page_idx,
            npages,
            MAX_PAGES
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
            last <= MAX_PAGES,
            "buddy fill_interior_order out of range ({}+2^{} > MAX_PAGES={})",
            page_idx,
            order,
            MAX_PAGES
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
        if order > MAX_ORDER {
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
        if list_head != EMPTY_LIST && list_head < MAX_PAGES {
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
        if order > MAX_ORDER {
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

        if prev_idx != u32::MAX as usize && prev_idx < MAX_PAGES {
            self.meta.get_mut(prev_idx).next = next_idx as u32;
        } else {
            // This is the list head, update global list head
            let new_head = if next_idx == u32::MAX as usize { EMPTY_LIST } else { next_idx };
            self.free_lists[order].store(new_head, Ordering::Release);
        }

        if next_idx != u32::MAX as usize && next_idx < MAX_PAGES {
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
        if order > MAX_ORDER { MAX_ORDER } else { order }
    }

    /// Convert size to order (O(1) bit manipulation)
    ///
    /// R75 (GNOME scribbles): the old clamp `order > MAX_ORDER → MAX_ORDER`
    /// made every allocation larger than the largest buddy block
    /// (PAGE_SIZE << MAX_ORDER, 16 MB) come back as a MAX_ORDER block — a
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
        if idx >= MAX_PAGES {
            return None;
        }
        Some(idx)
    }

    /// Allocate memory
    fn alloc_blocks(&self, order: usize) -> *mut u8 {
        // Search starting from specified order
        for mut current_order in order..=MAX_ORDER {
            let list_head = self.free_lists[current_order].load(Ordering::Acquire);

            if list_head != EMPTY_LIST && list_head < MAX_PAGES {
                // Found free block
                self.remove_from_free_list(list_head, current_order);

                // Split block if needed
                let mut page_idx = list_head;
                while current_order > order {
                    let block_size_pages = 1usize << current_order;
                    let buddy_idx = page_idx + (block_size_pages / 2);

                    // Safety check: buddy must be within valid range
                    if buddy_idx >= MAX_PAGES {
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
                // deferred audit printer in dfx=scribble mode.
                if !crate::sched::sched::IN_TASK_ALLOC.load(core::sync::atomic::Ordering::Acquire)
                    && crate::sched::sched::task_page_is_owned(addr as *const u8)
                {
                    // R69: also record the event with a caller backtrace
                    // for the deferred audit printer (dfx=scribble,
                    // x86_64 — the dfx scribble module is x86-only).
                    #[cfg(feature = "x86_64")]
                    if crate::dfx::scribble::ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
                        let frames = crate::dfx::scribble::audit_frames();
                        crate::dfx::scribble::audit_record(
                            crate::dfx::scribble::AUDIT_KIND_HANDOFF,
                            addr as u64,
                            order as u32,
                            &frames,
                        );
                    }
                    crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE); // R18-1 double-handoff
                }
                return addr as *mut u8;
            }
        }

        // Not enough memory
        core::ptr::null_mut()
    }

    /// Free memory
    unsafe fn free_blocks(&self, ptr: *mut u8, order: usize) {
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
                // the deferred audit printer (dfx=scribble, x86_64).
                #[cfg(feature = "x86_64")]
                if crate::dfx::scribble::ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
                    let frames = crate::dfx::scribble::audit_frames();
                    crate::dfx::scribble::audit_record(
                        crate::dfx::scribble::AUDIT_KIND_DOUBLE_FREE,
                        addr as u64,
                        order as u32,
                        &frames,
                    );
                }
                crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE); // R9-14 double-free/hoard
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
            // Boundary check: order cannot exceed MAX_ORDER
            if current_order > MAX_ORDER {
                // Exceeds maximum order, add directly to MAX_ORDER list
                self.add_to_free_list(page_idx, MAX_ORDER);
                break;
            }

            let buddy_idx = self.get_buddy_idx(page_idx, current_order);

            // Check if buddy is in valid range
            if buddy_idx >= MAX_PAGES {
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
        // DFX memwatch: own s0, captured unconditionally at the top of the
        // body. s0 is callee-saved so the value stays valid for the whole
        // body even across helper calls; the actual frame walk runs just
        // before the note call (see below). -O0 riscv64 frames keep
        // s0 = frame TOP: [s0-8] = own saved ra, [s0-0x10] = caller's s0,
        // so walking from s0 yields [ra-into-__rust_alloc, callers...].
        let mw_s0: u64 = crate::dfx::backtrace::current_frame_pointer();

        // Opportunistic reclamation of time-deferred frees (see
        // mm/deferred_free.rs): one relaxed load on the fast path; the
        // same allocation churn that fills the pool drains it.
        crate::mm::deferred_free::drain_if_due();

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
        // come back as an undersized MAX_ORDER block (see size_to_order).
        if order > MAX_ORDER {
            if size > 4 * 1024 * 1024 {
                crate::pr_err!(
                    "bigalloc: REJECTED size={:#x} (> max block {:#x})",
                    size,
                    PAGE_SIZE << MAX_ORDER
                );
            }
            return core::ptr::null_mut();
        }

        // Small-object packing layer (alloc-crash r12-16): layouts that
        // fit a <=1 KiB class are served from packed slab pages — one
        // buddy page holds dozens of objects instead of one. Falls
        // through to the plain buddy path if the layer cannot serve
        // (uninitialized heap, out of pages).
        if size <= crate::mm::small_alloc::SMALL_MAX
            && align <= crate::mm::small_alloc::SMALL_MAX
        {
            let p = crate::mm::small_alloc::alloc(size, align);
            if !p.is_null() {
                return p;
            }
        }

        let _guard = self.lock.lock_irqsave();
        let ptr = self.alloc_blocks(order);
        if ptr.is_null() && size > 4 * 1024 * 1024 {
            crate::pr_err!("bigalloc: FAILED size={:#x}", size);
        }
        drop(_guard);
        if !ptr.is_null() && crate::dfx::memwatch::armed() {
            let mut mw_frames: [u64; crate::dfx::memwatch::SITE_FRAMES] =
                [0; crate::dfx::memwatch::SITE_FRAMES];
            // f[0] = own saved ra (== the ra at entry, before any helper
            // could clobber the register), f[1..] = the s0 chain above.
            // s0 is callee-saved, so the walk stays valid even here, after
            // alloc_blocks().
            unsafe {
                crate::dfx::memwatch::walk_fp_chain(mw_s0, &mut mw_frames);
            }
            crate::dfx::memwatch::note_alloc(
                ptr,
                self.heap_start.load(Ordering::Relaxed),
                size,
                &mw_frames,
            );
        }
        ptr
    }

    // DFX memwatch leak hunt: explicit realloc (instead of the default
    // trait method) so the frame-pointer chain can be walked past the whole
    // raw_vec/alloc growth machinery into the allocating kernel code. This
    // is how the GNOME-oom leak was run to ground (the "129..256-byte
    // buffer reallocated ~4.6x/s" signature was BTreeMap node growth in
    // timer add_timer_wakeup). At -O0 the chain is: this method <-
    // __rust_realloc <- realloc_nonnull <- grow_impl <- Allocator::grow <-
    // finish_grow <- grow_amortized <- do_reserve <- reserve <-
    // append_elements <- THE KERNEL CODE, so 12 frames are captured.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // DFX memwatch leak hunt (see the method-level comment below for the
        // chain shape). The walk is gated: disabled = zero added work on the
        // realloc hot path; enabled = 12-frame pointer chase per realloc.
        let mut mw_frames: [u64; crate::dfx::memwatch::SITE_FRAMES] =
            [0; crate::dfx::memwatch::SITE_FRAMES];
        if crate::dfx::memwatch::armed() {
            let mut fp: u64 = crate::dfx::backtrace::current_frame_pointer();
            // f[0] = return address out of this method (into __rust_realloc);
            // f[1..] ascend the caller chain through the growth machinery.
            unsafe {
                crate::dfx::memwatch::walk_fp_chain(fp, &mut mw_frames);
            }
            crate::dfx::memwatch::note_realloc(new_size, &mw_frames);
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

        // Teardown-race guard (mm/deferred_free.rs): frees of the
        // registered size class (the MmStruct ArcInner block) are held
        // back briefly so unaccounted &MmStruct borrowers (procfs
        // generators) preempted mid-syscall resume against coherent
        // post-teardown memory instead of heap-reused garbage (the
        // #GP-in-btree-navigate family). NOTE: this impl is the live
        // #[global_allocator] (GLOBAL_ALLOCATOR: BuddyAllocator) — the
        // interception must live HERE, not in CombinedAllocator.
        if crate::mm::deferred_free::try_defer(ptr, &layout) {
            return;
        }

        self.dealloc_untracked(ptr, layout);
    }
}

/// Free through the live global allocator, bypassing the deferred-free
/// interception (drainer entry point, mm/deferred_free.rs).
pub unsafe fn dealloc_untracked_pub(ptr: *mut u8, layout: Layout) {
    GLOBAL_ALLOCATOR.dealloc_untracked(ptr, layout);
}

impl BuddyAllocator {
    /// Raw order-0 page for the small-object packing layer (mm/
    /// small_alloc.rs): same accounting and locking as the GlobalAlloc
    /// path (NET_PAGES, tripwires) but no memwatch note and no
    /// size-to-order walk. Callers must NOT hold the buddy lock; the
    /// small-alloc class lock MAY be held (class -> buddy order only).
    pub unsafe fn alloc_page_raw(&self) -> *mut u8 {
        if self.magic.load(Ordering::Acquire) != 0xDEADBEEF
            || self.initialized.load(Ordering::Acquire) == 0
        {
            return core::ptr::null_mut();
        }
        let _guard = self.lock.lock_irqsave();
        let ptr = self.alloc_blocks(0);
        drop(_guard);
        ptr
    }

    /// Raw order-0 free, the matching twin of alloc_page_raw.
    pub unsafe fn free_page_raw(&self, ptr: *mut u8) {
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
        self.free_blocks(ptr, 0);
        drop(_guard);
    }
}

impl BuddyAllocator {
    /// The real buddy free path, unintercepted — the deferred-free
    /// drainer (mm/deferred_free.rs) calls this so a held-back entry can
    /// never be re-deferred.
    unsafe fn dealloc_untracked(&self, ptr: *mut u8, layout: Layout) {
        // Small-object packing layer FIRST (alloc-crash r12-16): slab-page
        // objects free through their page's freelist, never the buddy —
        // and this must run before the R69 task-page probe, whose
        // page-granular ownership bitmap would refuse frees of non-task
        // objects sharing a (now packed) slab page with a live Task.
        if crate::mm::small_alloc::free(ptr as usize) {
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
            crate::dfx::memwatch::DEALLOC_OUTSIDE.fetch_add(1, Ordering::Relaxed);
            refused_free_note(FR_OUT_OF_WINDOW, ptr_addr, self.size_to_order(size.max(align)), 0, 0);
            return;
        }

        let order = self.size_to_order(size.max(align));

        // R69 free-side probe (x86_64): a free targeting a page that
        // still carries a live Task (ownership bitmap; alloc/
        // free_task_slot maintain it) is the double-handoff engine's
        // fuel — the page would return to the free lists while its Task
        // still runs, and the next two allocations share it. Refuse
        // under dfx=scribble (the bitmap is exact since R69; the old
        // R17-B refusal was withdrawn because the pre-Batch-8 bitmap
        // aliased) and record with a caller backtrace either way.
        // IN_TASK_FREE marks the legitimate release path.
        #[cfg(feature = "x86_64")]
        if crate::dfx::scribble::ENABLED.load(core::sync::atomic::Ordering::Relaxed)
            && !crate::sched::sched::IN_TASK_FREE.load(core::sync::atomic::Ordering::Acquire)
            && crate::sched::sched::task_page_is_owned(ptr)
        {
            let frames = crate::dfx::scribble::audit_frames();
            crate::dfx::scribble::audit_record(
                crate::dfx::scribble::AUDIT_KIND_FREE_LIVE,
                ptr_addr as u64,
                order as u32,
                &frames,
            );
            crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE); // R69 free-of-live
            refused_free_note(FR_TASK_LIVE, ptr_addr, order, 0, 0);
            return;
        }

        // Check if order exceeds MAX_ORDER
        if order > MAX_ORDER {
            crate::dfx::memwatch::DEALLOC_BIGORDER.fetch_add(1, Ordering::Relaxed);
            refused_free_note(FR_ORDER_TOO_BIG, ptr_addr, order, 0, 0);
            return;
        }

        // R21-3: block-alignment validation — a free of an interior or
        // offset pointer (off-by-slack / partial-unwind bugs) used to
        // pass the leader-only tripwire after R17-A2 made interior pages
        // carry consistent metadata, splicing bogus blocks into the
        // doubly-linked freelist.
        if self.addr_to_page_idx_checked(ptr as usize).map(|idx| idx % (1usize << order) != 0).unwrap_or(true) {
            crate::dfx::memwatch::DEALLOC_MISALIGNED.fetch_add(1, Ordering::Relaxed);
            refused_free_note(FR_UNALIGNED, ptr_addr, order, 0, 0);
            return;
        }

        let _guard = self.lock.lock_irqsave();
        self.free_blocks(ptr, order);
        drop(_guard);
        crate::dfx::memwatch::note_free(
            ptr,
            self.heap_start.load(Ordering::Relaxed),
            size,
        );
    }
}

/// Global allocator (Buddy System)
/// Note: This is the only allocator instance, used for kernel heap allocation and #[global_allocator]
pub static GLOBAL_ALLOCATOR: BuddyAllocator = BuddyAllocator::new();

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

/// Buddy allocator statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct BuddyStats {
    /// Heap start address
    pub heap_start: usize,
    /// Heap end address
    pub heap_end: usize,
    /// Heap total size (bytes)
    pub heap_size: usize,
    /// Used size (bytes)
    pub used_bytes: usize,
    /// Free size (bytes)
    pub free_bytes: usize,
    /// Number of free blocks per order
    pub free_blocks: [usize; MAX_ORDER + 1],
    /// Total allocation count
    pub alloc_count: usize,
    /// Total free count
    pub free_count: usize,
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
pub fn buddy_free_census_lockfree() -> (usize, usize, [u32; MAX_ORDER + 1]) {
    let mut blocks = [0u32; MAX_ORDER + 1];
    if HEAP_ALLOCATOR.initialized.load(Ordering::Acquire) == 0 {
        return (0, 0, blocks);
    }
    let start = HEAP_ALLOCATOR.heap_start.load(Ordering::Acquire);
    let end = HEAP_ALLOCATOR.heap_end.load(Ordering::Acquire);
    let heap_size = end.saturating_sub(start);
    let mut free_pages = 0usize;
    for order in 0..=MAX_ORDER {
        let mut page_idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);
        let mut hops = 0usize;
        while page_idx != EMPTY_LIST && page_idx < MAX_PAGES && hops < MAX_PAGES {
            blocks[order] = blocks[order].saturating_add(1);
            free_pages += 1usize << order;
            let next = HEAP_ALLOCATOR.meta.get(page_idx).next as usize;
            if next == u32::MAX as usize || next >= MAX_PAGES {
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
    for order in 0..=MAX_ORDER {
        let mut count = 0usize;
        let mut page_idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);

        while page_idx != EMPTY_LIST && page_idx < MAX_PAGES {
            count += 1;
            total_free_pages += 1usize << order;
            let next = HEAP_ALLOCATOR.meta.get(page_idx).next as usize;
            if next == u32::MAX as usize || next >= MAX_PAGES {
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
// Meta shadow audit (dfx=metawatch, wildwrite-hunt line session 13)
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

/// One shadow-audit mismatch. `kind` classifies it; `idx`/`prev`/`next`
/// carry the offending block's leader meta, `word`/`bits`/`derived` the
/// offending free_bits word and the audit-derived shadow value for the
/// bitmap kinds.
#[derive(Clone, Copy, PartialEq)]
pub struct MetaFinding {
    pub kind: MetaFindKind,
    pub order: usize,
    pub idx: usize,
    pub prev: u32,
    pub next: u32,
    pub word: usize,
    pub bits: u64,
    pub derived: u64,
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MetaFindKind {
    /// A list head or next-link escaped [0, MAX_PAGES), or a block's
    /// span ran past the end of the page array.
    LinkOob,
    /// Listed block's leader order byte disagrees with the list order.
    OrderMismatch,
    /// Listed block's leader free byte is not 1.
    FreeByteClear,
    /// Listed block leader is not 2^order-aligned.
    Unaligned,
    /// prev link does not mirror the walk position.
    PrevBroken,
    /// free_bits has set bits where the lists say nothing is free.
    BitmapSetUnlisted,
    /// A listed free block's span has clear bits in free_bits.
    BitmapClearListed,
    /// The same page is reachable from two list positions.
    DoubleListed,
    /// Walk exceeded the page-count budget (list cycle / mega-list).
    HopOverflow,
}

/// Scratch shadow bitmap for the audit, plus the single-CPU claim that
/// makes it safe: pass 1 and its under-lock confirmation run inside one
/// claim, so the scratch is never shared by two concurrent audits.
static META_AUDIT_BUSY: AtomicU64 = AtomicU64::new(0);

/// MetaArray-style interior-mutable scratch (see MetaArray's SAFETY
/// comments): only touched by `meta_audit_inner`, which every caller
/// reaches while holding the META_AUDIT_BUSY claim.
struct AuditScratch {
    data: UnsafeCell<[u64; FREE_BITMAP_WORDS]>,
}

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

    let mut hop_budget = MAX_PAGES + MAX_ORDER + 1;
    'orders: for order in 0..=MAX_ORDER {
        let npages = 1usize << order;
        let mut idx = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);
        let mut prev_link = u32::MAX as usize; // head's prev must be null
        loop {
            if idx == EMPTY_LIST {
                break;
            }
            if idx >= MAX_PAGES || idx + npages > MAX_PAGES {
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
                    order: MAX_ORDER + 1,
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
                    order: MAX_ORDER + 1,
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

/// Outcome of one shadow scan: what pass 1 (lock-free) saw and what the
/// under-lock confirmation made of it. `confirmed > 0` is corruption.
#[derive(Clone, Copy)]
pub struct MetaScanOutcome {
    pub lockfree_findings: usize,
    pub confirmed_findings: usize,
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
    for order in 0..=MAX_ORDER {
        let head = HEAP_ALLOCATOR.free_lists[order].load(Ordering::Acquire);
        if head != EMPTY_LIST && head < MAX_PAGES {
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

/// Combined allocator - prioritizes Slab for small objects, falls back to Buddy for large objects
///
/// This design can reduce memory fragmentation and improve small object allocation efficiency
pub struct CombinedAllocator;

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
    let in_slab = crate::mm::slab::slab_region()
        .map(|(s, e)| ptr_addr >= s && ptr_addr < e)
        .unwrap_or(false);

    if in_slab {
        // In Slab area, use kfree
        crate::mm::kfree(ptr);
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
        crate::mm::deferred_free::drain_if_due();

        // Small objects (<= 4096 bytes) try Slab allocator. The align <= 8
        // guard is load-bearing: slab objects are at least 8-aligned (16-byte
        // header + 8-multiple object sizes) but NOT 16-aligned in general, so
        // higher alignments must fall through to the buddy, which hands out
        // naturally aligned power-of-two blocks. Routing an align-16 layout
        // through the slab would hand back a misaligned pointer.
        // Without the slab, EVERY small Rust allocation (Box/Arc/String/Vec)
        // pinned a whole 4KB buddy page — a 30K-object workload exhausted the
        // 128MB heap (LTP inode02 ALLOCTHROW storm).
        if size <= 4096 && layout.align() <= 8 && crate::mm::slab::is_slab_initialized() {
            let ptr = crate::mm::kmalloc(size);
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
        if crate::mm::deferred_free::try_defer(ptr, &layout) {
            return;
        }

        dealloc_direct(ptr, layout);
    }
}
