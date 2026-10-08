//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Heap-leak hunting instrumentation (dfx=memwatch).
//!
//! Two near-zero-cost layers, both fed from the `#[global_allocator]`:
//!
//! 1. Per-size-class alloc/free counters (always on while memwatch is
//!    enabled) — the DELTA between dumps gives the net growth per power-
//!    of-two size class, which pins the leaked object size.
//! 2. Call-site histogram for the "big object" class (2048 < size <=
//!    4096): the first 4 frame-pointer return addresses are hashed into a
//!    fixed table. Every 2560-byte-ish allocation leaves a count at its
//!    allocation site; the periodic DFX dump prints the hottest entries
//!    with raw PCs for offline addr2line.
//!
//! Output rides the taskdump raw-UART path (no printk, no allocation) so
//! it is safe from the timer softirq context that hosts the periodic dump.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use super::taskdump::{taskdump_dec, taskdump_raw_line};

/// Runtime switch state (set by `dfx=memwatch` on the cmdline).
pub static ENABLED: AtomicBool = AtomicBool::new(false);

/// Number of size-class buckets: sizes 1..=2^(NUM_BINS-1), plus overflow.
pub const NUM_BINS: usize = 13; // buckets: 1,2,4,...,4096 (>4096 clamps to last)

pub static ALLOC_BINS: [AtomicUsize; NUM_BINS] =
    [const { AtomicUsize::new(0) }; NUM_BINS];
pub static FREE_BINS: [AtomicUsize; NUM_BINS] =
    [const { AtomicUsize::new(0) }; NUM_BINS];

/// Histogram slots for allocation call sites.
pub const SITE_SLOTS: usize = 192;
/// Frames recorded per site: the raw_vec/alloc growth machinery at -O0 is
/// deeper than the kernel code (realloc: this method <- __rust_realloc <-
/// realloc_nonnull <- grow_impl <- Allocator::grow <- finish_grow <-
/// grow_amortized <- do_reserve <- reserve <- append_elements <- THE
/// KERNEL CODE), so 12 slots are needed before the kernel frame appears.
pub const SITE_FRAMES: usize = 12;

struct Site {
    key: AtomicUsize,
    count: AtomicUsize,
    bucket: AtomicUsize,
    /// First-seen exact byte size at this site (disambiguates within a
    /// power-of-two bucket — 129..256 vs exactly 256 matters).
    size: AtomicUsize,
    frames: [AtomicUsize; SITE_FRAMES],
}

static SITES: [Site; SITE_SLOTS] = [const {
    Site {
        key: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
        bucket: AtomicUsize::new(0),
        size: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; SITE_SLOTS];

/// Bucket index for an allocation size (floor(log2(size)), clamped).
#[inline]
fn bucket(size: usize) -> usize {
    if size <= 1 {
        return 0;
    }
    let log2 = usize::BITS - 1 - size.leading_zeros();
    (log2 as usize).min(NUM_BINS - 1)
}

/// Whether memwatch is armed (cheaper than a Relaxed load? same — but keeps
/// the allocator prologue short and single-sourced).
#[inline]
pub fn armed() -> bool {
    ENABLED.load(Ordering::Relaxed)
}/// Realloc call-site histogram: (new_size bucket, frame chain) -> count.
/// Captured in the allocator's explicit realloc() via an 8-deep frame
/// walk (see buddy_allocator.rs) — deep enough to clear the whole
/// raw_vec/alloc growth machinery and land in the kernel caller.
static RSITES: [Site; 64] = [const {
    Site {
        key: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
        bucket: AtomicUsize::new(0),
        size: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; 64];

/// Ascend the -O0 frame-pointer chain starting at `fp` (s0 of the frame to
/// record first), storing each frame's saved return address into `out`:
/// out[0] = return address of the frame at `fp`, out[1] = its caller's,
/// etc. Stops at the first fp that is not a plausible kernel stack frame
/// (canonical kernel address, 8-aligned, strictly ascending). Only live
/// frames are touched, so validated reads cannot fault.
///
/// MUST be called with the frame at `fp` live on the current call stack
/// (it is: callers pass their own s0 before doing anything else).
#[inline(never)]
pub unsafe fn walk_fp_chain(mut fp: u64, out: &mut [u64]) {
    for slot in out.iter_mut() {
        *slot = 0;
        if !(fp >= 0x8000_0000_0000_0000 && fp & 7 == 0) {
            return;
        }
        let ra = core::ptr::read_volatile((fp - 8) as *const u64);
        let next = core::ptr::read_volatile((fp - 0x10) as *const u64);
        *slot = ra;
        if !(next >= 0x8000_0000_0000_0000 && next & 7 == 0 && next > fp) {
            return;
        }
        fp = next;
    }
}

fn chain_key(bucket: usize, frames: &[u64; SITE_FRAMES]) -> usize {
    // keep 0 as the "empty slot" sentinel
    let mut key = bucket.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    for (i, &f) in frames.iter().enumerate() {
        key ^= (f as usize).rotate_left((i as u32) * 13 + 3);
    }
    key
}

/// Record one realloc observation. `frames[0]` is the return address out
/// of GlobalAlloc::realloc (into __rust_realloc); deeper entries climb
/// the growth machinery into the kernel caller.
pub fn note_realloc(new_size: usize, frames: &[u64; SITE_FRAMES]) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let b = bucket(new_size);
    let key = chain_key(b, frames);
    let s = &RSITES[key % 64];
    if s.key.load(Ordering::Relaxed) == key {
        s.count.fetch_add(1, Ordering::Relaxed);
    } else if s.key.load(Ordering::Relaxed) == 0 {
        if s.key
            .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            s.bucket.store(b, Ordering::Relaxed);
            s.size.store(new_size, Ordering::Relaxed);
            for i in 0..SITE_FRAMES {
                s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
            }
            s.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Record one successful alloc observation. `frames[0]` is the caller's
/// return address captured from `ra` in the allocator prologue (before
/// any helper call can clobber it); `frames[1..]` come from the s0 chain.
/// `ptr`/`heap_start` feed the page->site table so the matching free can
/// be attributed back to THIS site (live counting).
pub fn note_alloc(ptr: *mut u8, heap_start: usize, size: usize, frames: &[u64; SITE_FRAMES]) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let b = bucket(size);
    ALLOC_BINS[b].fetch_add(1, Ordering::Relaxed);

    if b >= 7 {
        let key = chain_key(b, frames);
        let s = &SITES[key % SITE_SLOTS];
        if s.key.load(Ordering::Relaxed) == key {
            s.count.fetch_add(1, Ordering::Relaxed);
        } else if s.key.load(Ordering::Relaxed) == 0 {
            // claim the empty slot; a lost race only mis-files a count
            if s.key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                s.bucket.store(b, Ordering::Relaxed);
                s.size.store(size, Ordering::Relaxed);
                for i in 0..SITE_FRAMES {
                    s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
                }
                s.count.fetch_add(1, Ordering::Relaxed);
            }
        }
        // else: hash collision — count is dropped (diagnostic only)
    }

    // Live accounting: mark every heap page of this block with the site id
    // so the later free() of the block decrements the RIGHT site. Hunt-v3:
    // threshold lowered from b>=7 to b>=3. The buddy heap is PAGE-granular
    // (a 9-byte Box<[u8]> pins a whole 4096B block), so SMALL-object leaks
    // dominate real heap growth — the per-exec exe_path leak was a b3
    // (8..15-byte) site invisible under the old >=128B tracking. b0..b2
    // (1..4-byte) churn stays excluded to keep the 250-slot table from
    // saturating at boot (see LIVE_SLOTS comment).
    if b >= 3 {
        let sid = live_intern(b, size, frames);
        if sid != 0 {
            LIVE_SITES[sid - 1].allocs.fetch_add(1, Ordering::Relaxed);
            LIVE_BINS[b].fetch_add(1, Ordering::Relaxed);
            mark_pages(ptr as usize, heap_start, size, sid, true);
        }
    }
}

/// Live count per size class (tracked b>=7 blocks only).
pub static LIVE_BINS: [AtomicUsize; NUM_BINS] =
    [const { AtomicUsize::new(0) }; NUM_BINS];

// ---------------------------------------------------------------------------
// Live-object accounting (page -> site-id table)
//
// The histograms above count EVENTS, which cannot distinguish a 5246x/s
// alloc/free churn site from a 4.6x/s never-freed leak. Here every heap
// page carries the id of the site that allocated its block; on free the
// site's live counter drops. The dump then prints `live=` per site —
// a monotonic `live` growth IS the leak, regardless of churn.
// ---------------------------------------------------------------------------

/// 128MB heap / 4KB pages. Keep in sync with the heap size in Kernel.toml
/// (boot msg "heap region 128MB @ 0x80a00000"); out-of-range indices are
/// simply not tracked.
pub const TRACKED_PAGES: usize = 32768;
pub static PAGE_SITE: [AtomicU8; TRACKED_PAGES] = [const { AtomicU8::new(0) }; TRACKED_PAGES];

/// Extended per-site live counters. Index+1 is the site id stored in
/// PAGE_SITE (0 = untracked). LIVE_SLOTS <= 255.
///
/// Only classes >= 128B are interned: tiny-class churn interns thousands
/// of distinct chains within seconds and saturates the table before the
/// leak class ever gets a slot (observed: live table frozen at boot-time
/// entries while b8 leaked unnoticed).
pub const LIVE_SLOTS: usize = 250;
pub struct LiveSite {
    pub key: AtomicUsize,
    pub allocs: AtomicUsize,
    pub frees: AtomicUsize,
    pub size: AtomicUsize,
    pub bucket: AtomicUsize,
    pub frames: [AtomicUsize; SITE_FRAMES],
}
pub static LIVE_SITES: [LiveSite; LIVE_SLOTS] = [const {
    LiveSite {
        key: AtomicUsize::new(0),
        allocs: AtomicUsize::new(0),
        frees: AtomicUsize::new(0),
        size: AtomicUsize::new(0),
        bucket: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; LIVE_SLOTS];

/// Map a chain to a stable live-site id (1..=LIVE_SLOTS), or 0 if the
/// table is full / disabled. Same relaxed-races-are-fine discipline as
/// the histogram tables above.
fn live_intern(bucket: usize, size: usize, frames: &[u64; SITE_FRAMES]) -> usize {
    let key = chain_key(bucket, frames);
    let h = key % LIVE_SLOTS;
    // Check the home slot and one neighbor: linear probing keeps distinct
    // chains distinct without a full scan.
    for off in 0..2 {
        let s = &LIVE_SITES[(h + off) % LIVE_SLOTS];
        let k = s.key.load(Ordering::Relaxed);
        if k == key {
            s.size.store(size, Ordering::Relaxed);
            return (h + off) % LIVE_SLOTS + 1;
        }
        if k == 0 {
            if s.key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                s.bucket.store(bucket, Ordering::Relaxed);
                s.size.store(size, Ordering::Relaxed);
                for i in 0..SITE_FRAMES {
                    s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
                }
                return (h + off) % LIVE_SLOTS + 1;
            }
            // lost the race: re-read below
            if s.key.load(Ordering::Relaxed) == key {
                return (h + off) % LIVE_SLOTS + 1;
            }
        }
    }
    0
}

/// Set/clear the site id on every page covered by [ptr, ptr+size).
fn mark_pages(ptr: usize, heap_start: usize, size: usize, sid: usize, set: bool) {
    if heap_start == 0 || ptr < heap_start {
        return;
    }
    let first = (ptr - heap_start) / 4096;
    if first >= TRACKED_PAGES {
        return;
    }
    // span the whole order-rounded block: the buddy allocator hands out
    // 2^order pages; without rounding, a 2560B alloc marks 1 page but the
    // free below would try to clear by size again — consistent either way,
    // so plain page-count from size rounded up is enough.
    let pages = (size + 4095) / 4096;
    let mut i = first;
    while i < first + pages && i < TRACKED_PAGES {
        if set {
            PAGE_SITE[i].store(sid as u8, Ordering::Relaxed);
        } else {
            PAGE_SITE[i].store(0, Ordering::Relaxed);
        }
        i += 1;
    }
}

/// Attribute one free to the site that allocated the block.
pub fn note_free(ptr: *mut u8, heap_start: usize, size: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    FREE_BINS[bucket(size)].fetch_add(1, Ordering::Relaxed);
    if heap_start == 0 || (ptr as usize) < heap_start {
        return;
    }
    let first = (ptr as usize - heap_start) / 4096;
    if first >= TRACKED_PAGES {
        return;
    }
    let sid = PAGE_SITE[first].load(Ordering::Relaxed);
    if sid != 0 && sid as usize <= LIVE_SLOTS {
        mark_pages(ptr as usize, heap_start, size, sid as usize, false);
        let s = &LIVE_SITES[sid as usize - 1];
        s.frees.fetch_add(1, Ordering::Relaxed);
        let b = s.bucket.load(Ordering::Relaxed);
        if b < NUM_BINS {
            LIVE_BINS[b].fetch_sub(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Physical-page accounting (alloc_pages/free_pages funnel + alloc_page_table)
//
// The exec-leak signature (heap flat, page_desc used_pages +2.3MB/s) lives
// BELOW the GlobalAlloc heap: whole 4K pages. Same discipline as the heap
// live table: intern the allocation call-site chain, stamp every PFN of the
// block with the site id, un-stamp + count on free. A monotonic net growth
// per site in the dump IS the page leak.
//
// The PFN->site table is 512KB and CANNOT be static: the early-boot page
// tables map the kernel image only up to the 8MB mark and .bss sits within
// ~23KB of it (a static table panics the boot with a pfault at
// 0xffffffff80800000). It is lazily carved out of the kernel heap on first
// use instead.
// ---------------------------------------------------------------------------

/// 2GB / 4KB pages. PFNs outside are untracked.
pub const PPAGE_TABLE_ENTRIES: usize = 524288;

static PPAGE_SITE_PTR: AtomicUsize = AtomicUsize::new(0); // 0=none, 1=failed, else ptr

fn ppage_site_table() -> *mut [AtomicU8; PPAGE_TABLE_ENTRIES] {
    let p = PPAGE_SITE_PTR.load(Ordering::Acquire);
    if p == 1 {
        return core::ptr::null_mut();
    }
    if p != 0 {
        return p as *mut [AtomicU8; PPAGE_TABLE_ENTRIES];
    }
    // First use: carve the table out of the kernel heap (the GlobalAlloc
    // heap is a pre-reserved region — this never re-enters alloc_pages).
    let mut v: alloc::vec::Vec<u8> = alloc::vec![0u8; PPAGE_TABLE_ENTRIES];
    let ptr = v.as_mut_ptr();
    match PPAGE_SITE_PTR.compare_exchange(0, ptr as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            core::mem::forget(v);
            ptr as *mut [AtomicU8; PPAGE_TABLE_ENTRIES]
        }
        Err(cur) => {
            if cur == 1 {
                core::ptr::null_mut()
            } else {
                cur as *mut [AtomicU8; PPAGE_TABLE_ENTRIES]
            }
        }
    }
}

pub const PSITE_SLOTS: usize = 64;

/// Raw free_pages() funnel count while armed — 0 means the page free side
/// is entirely dead (no page ever returns to the zone allocator).
pub static PGFREE_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// MmStruct::drop invocations (user address spaces fully released).
pub static MM_DROPS: AtomicUsize = AtomicUsize::new(0);
/// free_user_page_tables entries (teardown walks started).
pub static FUT_CALLS: AtomicUsize = AtomicUsize::new(0);
/// page-table pages actually returned by free_page_table_checked.
pub static FUT_TABLES: AtomicUsize = AtomicUsize::new(0);
/// data pages actually returned by the teardown walk (put_page -> 0).
pub static FUT_PAGES: AtomicUsize = AtomicUsize::new(0);
pub struct PageSite {
    pub key: AtomicUsize,
    pub allocs: AtomicUsize,
    pub frees: AtomicUsize,
    pub order: AtomicUsize,
    pub frames: [AtomicUsize; SITE_FRAMES],
}
pub static PSITES: [PageSite; PSITE_SLOTS] = [const {
    PageSite {
        key: AtomicUsize::new(0),
        allocs: AtomicUsize::new(0),
        frees: AtomicUsize::new(0),
        order: AtomicUsize::new(0),
        frames: [const { AtomicUsize::new(0) }; SITE_FRAMES],
    }
}; PSITE_SLOTS];

fn psite_intern(frames: &[u64; SITE_FRAMES]) -> usize {
    let key = chain_key(0, frames);
    let h = key % PSITE_SLOTS;
    for off in 0..2 {
        let s = &PSITES[(h + off) % PSITE_SLOTS];
        let k = s.key.load(Ordering::Relaxed);
        if k == key {
            return (h + off) % PSITE_SLOTS + 1;
        }
        if k == 0 {
            if s.key
                .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                for i in 0..SITE_FRAMES {
                    s.frames[i].store(frames[i] as usize, Ordering::Relaxed);
                }
                return (h + off) % PSITE_SLOTS + 1;
            }
            if s.key.load(Ordering::Relaxed) == key {
                return (h + off) % PSITE_SLOTS + 1;
            }
        }
    }
    0
}

/// Record one physical-page allocation (order-sized block).
pub fn note_page_alloc(phys: usize, order: usize, frames: &[u64; SITE_FRAMES]) {
    if !ENABLED.load(Ordering::Relaxed) || phys == 0 {
        return;
    }
    let table = ppage_site_table();
    if table.is_null() {
        return;
    }
    let sid = psite_intern(frames);
    if sid == 0 {
        return;
    }
    PSITES[sid - 1].allocs.fetch_add(1, Ordering::Relaxed);
    PSITES[sid - 1].order.store(order, Ordering::Relaxed);
    let first = tracked_index(phys);
    if first == usize::MAX {
        return;
    }
    let pages = 1usize << order;
    let mut i = first;
    while i < first + pages && i < PPAGE_TABLE_ENTRIES {
        unsafe {
            (*table)[i].store(sid as u8, Ordering::Relaxed);
        }
        i += 1;
    }
}

/// Record one physical-page free: attribute it back to the site that
/// stamped the block's first PFN.
pub fn note_page_free(phys: usize, order: usize) {
    if !ENABLED.load(Ordering::Relaxed) || phys == 0 {
        return;
    }
    PGFREE_TOTAL.fetch_add(1, Ordering::Relaxed);
    let first = tracked_index(phys);
    if first == usize::MAX {
        return;
    }
    let table = ppage_site_table();
    if table.is_null() {
        return;
    }
    let sid = unsafe { (*table)[first].load(Ordering::Relaxed) };
    if sid == 0 || sid as usize > PSITE_SLOTS {
        return;
    }
    let pages = 1usize << order;
    let mut i = first;
    while i < first + pages && i < PPAGE_TABLE_ENTRIES {
        unsafe {
            (*table)[i].store(0, Ordering::Relaxed);
        }
        i += 1;
    }
    PSITES[sid as usize - 1].frees.fetch_add(1, Ordering::Relaxed);
}

/// DRAM-relative page index for the PFN->site table. Physical addresses
/// are absolute (0x80000000+ on qemu virt); the table indexes from the
/// DRAM base. usize::MAX = outside tracked DRAM.
fn tracked_index(phys: usize) -> usize {
    let base = if crate::mm::layout::is_kernel_layout_initialized() {
        crate::mm::layout::kernel_layout().phys_base
    } else {
        0x8000_0000
    };
    if phys < base {
        return usize::MAX;
    }
    (phys - base) / 4096
}

fn put_dec(v: u64) {
    taskdump_dec(v);
}

/// Print the kernel root's low-half (vpn2[0..1]) non-leaf population:
/// `l1` non-leaf PGD entries and `l0` non-leaf L1 entries beneath them.
/// Read-only walk; diagnostic noise under concurrency is acceptable.
fn kroot_pollution() {
    taskdump_raw_line(b"MEMKROOT l1=");
    let mut l1n = 0u64;
    let mut l0n = 0u64;
    let mut min_va = u64::MAX;
    let mut max_va = 0u64;
    let mut user_leaves = 0u64;
    unsafe {
        let root_ptr = &raw const crate::arch::mm::mmu_init::ROOT_PAGE_TABLE;
        let root = &*root_ptr;
        for i in 0..2usize {
            let pte2 = root.get(i);
            if !pte2.is_valid() || pte2.is_leaf() {
                continue;
            }
            l1n += 1;
            let t1 = crate::arch::mm::mmu_init::get_page_table_virt(
                pte2.ppn() << 12,
            );
            if t1.is_null() {
                continue;
            }
            for j in 0..512usize {
                let pte1 = (*t1).get(j);
                if !pte1.is_valid() {
                    continue;
                }
                let va = ((i as u64) << 30) | ((j as u64) << 21);
                if va < min_va { min_va = va; }
                if va + (1 << 21) > max_va { max_va = va + (1 << 21); }
                if pte1.is_leaf() {
                    if pte1.is_user() { user_leaves += 1; }
                    continue;
                }
                l0n += 1;
                let t0 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte1.ppn() << 12,
                );
                if t0.is_null() {
                    continue;
                }
                for k in 0..512usize {
                    let pte0 = (*t0).get(k);
                    if pte0.is_valid() && pte0.is_leaf() && pte0.is_user() {
                        user_leaves += 1;
                    }
                }
            }
        }
    }
    put_dec(l1n);
    taskdump_raw_line(b" l0=");
    put_dec(l0n);
    taskdump_raw_line(b" uleaf=");
    put_dec(user_leaves);
    if min_va != u64::MAX {
        taskdump_raw_line(b" va=0x");
        put_hex(min_va);
        taskdump_raw_line(b"-0x");
        put_hex(max_va);
    }
    taskdump_raw_line(b"\n");
}

fn put_hex(v: u64) {
    taskdump_raw_line(b"0x");
    let mut shift = 64;
    while shift > 0 {
        shift -= 4;
        let nib = (v >> shift) & 0xF;
        let c = if nib < 10 {
            b'0' + nib as u8
        } else {
            b'a' + (nib - 10) as u8
        };
        taskdump_raw_line(&[c]);
    }
}

/// One MEM line + the bin/site tables for the periodic DFX dump.
/// Raw UART, allocation-free, safe from softirq context.
pub fn dump_mem() {
    taskdump_raw_line(b"\nMEM heap ");
    let st = crate::mm::buddy_allocator::buddy_stats();
    put_dec((st.used_bytes / 1024) as u64);
    taskdump_raw_line(b"K/");
    put_dec((st.heap_size / 1024) as u64);
    // buddy_stats() does not maintain counters; use memwatch totals when
    // the switch is on (they started at dfx init, close enough for rates)
    let (mut at, mut ft) = (0u64, 0u64);
    if ENABLED.load(Ordering::Relaxed) {
        for i in 0..NUM_BINS {
            at += ALLOC_BINS[i].load(Ordering::Relaxed) as u64;
            ft += FREE_BINS[i].load(Ordering::Relaxed) as u64;
        }
    }
    taskdump_raw_line(b"K a=");
    put_dec(at);
    taskdump_raw_line(b" f=");
    put_dec(ft);
    let sl = crate::mm::slab::slab_stats();
    let mut sa = 0usize;
    let mut sf = 0usize;
    for c in sl.cache_stats.iter() {
        sa += c.alloc_count;
        sf += c.free_count;
    }
    taskdump_raw_line(b" | slab ");
    put_dec(sl.total_pages as u64);
    taskdump_raw_line(b"pg a=");
    put_dec(sa as u64);
    taskdump_raw_line(b" f=");
    put_dec(sf as u64);
    let pg = crate::mm::page_desc::page_desc_stats();
    taskdump_raw_line(b" | pages u=");
    put_dec(pg.used_pages as u64);
    taskdump_raw_line(b" r=");
    put_dec(pg.reserved_pages as u64);
    taskdump_raw_line(b"\n");

    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // net growth per size class since boot (allocs - frees)
    taskdump_raw_line(b"MEMBINS class:net\n");
    for i in 0..NUM_BINS {
        let a = ALLOC_BINS[i].load(Ordering::Relaxed);
        let f = FREE_BINS[i].load(Ordering::Relaxed);
        if a == 0 && f == 0 {
            continue;
        }
        let net = a.wrapping_sub(f) as i128;
        taskdump_raw_line(b" b");
        put_dec(i as u64);
        taskdump_raw_line(b"(size>");
        put_dec(if i == 0 { 0 } else { 1 << (i - 1) } as u64);
        taskdump_raw_line(b"):a=");
        put_dec(a as u64);
        taskdump_raw_line(b":f=");
        put_dec(f as u64);
        taskdump_raw_line(b":net=");
        if net < 0 {
            taskdump_raw_line(b"-");
            put_dec((-net) as u64);
        } else {
            put_dec(net as u64);
        }
        taskdump_raw_line(b"\n");
    }

    // hottest realloc sites (top 6): b=<new size bucket> sz=<first-seen
    // exact size> + the 8-deep return-address chain (deepest kernel frames
    // are the leaking code; frames inside __rust_* / alloc::* / raw_vec
    // machinery come first)
    taskdump_raw_line(b"MEMRSITES cnt bucket sz f1..f8\n");
    let mut rtaken = [false; 64];
    for _ in 0..6 {
        let mut best: isize = -1;
        let mut best_cnt = 0usize;
        for i in 0..64 {
            if rtaken[i] {
                continue;
            }
            let c = RSITES[i].count.load(Ordering::Relaxed);
            let k = RSITES[i].key.load(Ordering::Relaxed);
            if k != 0 && c > best_cnt {
                best_cnt = c;
                best = i as isize;
            }
        }
        if best < 0 {
            break;
        }
        let i = best as usize;
        rtaken[i] = true;
        taskdump_raw_line(b" rsite cnt=");
        put_dec(best_cnt as u64);
        taskdump_raw_line(b" b=");
        put_dec(RSITES[i].bucket.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" sz=");
        put_dec(RSITES[i].size.load(Ordering::Relaxed) as u64);
        for fr in RSITES[i].frames.iter() {
            taskdump_raw_line(b" ");
            put_hex(fr.load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }

    // LIVE sites, sorted by live count (top 12): live=allocs-frees per
    // call-site chain. A monotonically growing live value IS the leak;
    // churn-heavy sites show live ~0 despite huge alloc counts.
    taskdump_raw_line(b"MEMLIVE live allocs frees bucket sz f1..f12\n");
    taskdump_raw_line(b" livebins");
    for i in 0..NUM_BINS {
        if LIVE_BINS[i].load(Ordering::Relaxed) == 0 {
            continue;
        }
        taskdump_raw_line(b" b");
        put_dec(i as u64);
        taskdump_raw_line(b"=");
        put_dec(LIVE_BINS[i].load(Ordering::Relaxed) as u64);
    }
    taskdump_raw_line(b"\n");
    let mut ltaken = [false; LIVE_SLOTS];
    for _ in 0..12 {
        let mut best: isize = -1;
        let mut best_live: i64 = -1;
        for i in 0..LIVE_SLOTS {
            if ltaken[i] {
                continue;
            }
            let k = LIVE_SITES[i].key.load(Ordering::Relaxed);
            if k == 0 {
                continue;
            }
            let live = LIVE_SITES[i].allocs.load(Ordering::Relaxed) as i64
                - LIVE_SITES[i].frees.load(Ordering::Relaxed) as i64;
            if live > best_live {
                best_live = live;
                best = i as isize;
            }
        }
        if best < 0 {
            break;
        }
        let i = best as usize;
        ltaken[i] = true;
        taskdump_raw_line(b" lsite live=");
        put_dec(best_live as u64);
        taskdump_raw_line(b" a=");
        put_dec(LIVE_SITES[i].allocs.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" f=");
        put_dec(LIVE_SITES[i].frees.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" b=");
        put_dec(LIVE_SITES[i].bucket.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" sz=");
        put_dec(LIVE_SITES[i].size.load(Ordering::Relaxed) as u64);
        for fr in LIVE_SITES[i].frames.iter() {
            taskdump_raw_line(b" ");
            put_hex(fr.load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }

    // PHYSICAL-PAGE sites, sorted by net growth (top 10): net=allocs-frees
    // per allocation call-site chain. A monotonic net IS the page leak.
    taskdump_raw_line(b"MEMPSITES net allocs frees order f1..f12\n");
    taskdump_raw_line(b" pgfree_total=");
    put_dec(PGFREE_TOTAL.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" mm_drops=");
    put_dec(MM_DROPS.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" fut_calls=");
    put_dec(FUT_CALLS.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" fut_tables=");
    put_dec(FUT_TABLES.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b" fut_pages=");
    put_dec(FUT_PAGES.load(Ordering::Relaxed) as u64);
    taskdump_raw_line(b"\n");
    // Kernel-root low-half hygiene: valid non-leaf entries under
    // ROOT_PAGE_TABLE vpn2[0..1]. Every non-leaf L1 entry here costs one
    // L0-table copy per exec (copy_kernel_mappings), so growth in these
    // counters is the exec-leak amplifier.
    kroot_pollution();
    let mut ptaken = [false; PSITE_SLOTS];
    for _ in 0..10 {
        let mut best: isize = -1;
        let mut best_net: i64 = -1;
        for i in 0..PSITE_SLOTS {
            if ptaken[i] {
                continue;
            }
            let k = PSITES[i].key.load(Ordering::Relaxed);
            if k == 0 {
                continue;
            }
            let net = PSITES[i].allocs.load(Ordering::Relaxed) as i64
                - PSITES[i].frees.load(Ordering::Relaxed) as i64;
            if net > best_net {
                best_net = net;
                best = i as isize;
            }
        }
        if best < 0 {
            break;
        }
        let i = best as usize;
        ptaken[i] = true;
        taskdump_raw_line(b" psite net=");
        put_dec(best_net as u64);
        taskdump_raw_line(b" a=");
        put_dec(PSITES[i].allocs.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" f=");
        put_dec(PSITES[i].frees.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b" ord=");
        put_dec(PSITES[i].order.load(Ordering::Relaxed) as u64);
        for fr in PSITES[i].frames.iter() {
            taskdump_raw_line(b" ");
            put_hex(fr.load(Ordering::Relaxed) as u64);
        }
        taskdump_raw_line(b"\n");
    }
}
