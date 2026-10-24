//! Small-object packing layer over the kernel heap buddy (SLUB-style).
//!
//! alloc-crash r12-16 root cause: the kernel heap's minimum block is one
//! page, and an OpenHarmony first-frame boot holds ~213,000 net-live
//! objects in the 1-byte..1-KB classes (measured live on a4 via the
//! CR3-walked counters: 23.2M allocs / 23.0M frees; nets b4=23K, b5=37K,
//! b6=26K, b7=62K, b8=38K, b9=6.5K). Each object pinned a whole 4 KiB
//! buddy block — 852 MB of the 992 MB window — and the allocator died
//! (a2/a3/a4: census `free=0K`, ALLOCTHROW -> panic) exactly when
//! first-frame composition started. Linux answers this with kmalloc
//! slabs; this module gives the buddy heap the same packing.
//!
//! # Design
//!
//! - Eight power-of-two classes, S = 8..1024. A layout is served here
//!   when `size <= SMALL_MAX && align <= SMALL_MAX`; class S is the
//!   power of two >= max(size, align). Objects sit at
//!   page + FIRST_OFF(S) + i*S where FIRST_OFF(S) = max(16, S) — a
//!   multiple of S — so every object is S-aligned, and S % align == 0
//!   makes it `align`-aligned. The 16-byte header occupies the gap.
//! - A slab page is an order-0 block drawn from the buddy RAW (under the
//!   buddy lock, never through GlobalAlloc::alloc — no recursion).
//!   16-byte in-page header; objects from offset 16.
//! - Free routing: one static bit per heap page (`SLAB_PAGE_BITMAP`)
//!   marks slab pages. `GlobalAlloc::dealloc` checks the bit first;
//!   slab frees never reach the buddy free path. The bit is cleared
//!   BEFORE an empty page returns to the buddy, so a stale pointer into
//!   a reclaimed page falls through to the buddy tripwires (loud)
//!   instead of corrupting the slab.
//! - Per-class spinlock (irqsave). Active-page lists are singly linked
//!   through the page headers (page-index linkage, 0 = end). A page that
//!   fills up is unlinked immediately (the scan carries its prev); an
//!   EMPTY page is physically unlinked (walk from head) and returned to
//!   the buddy while still under the class lock — no other CPU can be
//!   scanning it at that moment, so there is no use-after-reclaim race.
//!
//! Lock order: class lock -> buddy lock (page create/reclaim). The
//! reverse nesting never occurs (the buddy knows nothing of classes).

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::spinlock::Spinlock;

const PAGE_SIZE: usize = 4096;
const HEAP_MAX_PAGES: usize = 262144; // 1 GiB window / 4 KiB

/// Maximum layout served by this layer (both size and align).
pub const SMALL_MAX: usize = 1024;

/// Classes: 8, 16, 32, 64, 128, 256, 512, 1024.
const CLASSES: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 1024];

const STATE_ACTIVE: u8 = 1; // has free objects, linked into the active list
const STATE_FULL: u8 = 2; // no free objects (or being reclaimed), unlinked

const HDR_MAGIC: u16 = 0x5142;

/// In-page header (16 bytes) at the start of every slab page.
#[repr(C)]
struct PageHdr {
    magic: u16,
    cls: u8,
    state: u8,
    free_head: u16, // first free object index, 0xFFFF = none
    free_count: u16,
    next_page: u32, // page INDEX of the next active page, 0 = end
    _pad: u32,
}

#[inline]
fn hdr_of(page: usize) -> &'static mut PageHdr {
    // SAFETY: callers hold the class lock (or are initializing a page
    // that no other CPU can reach yet — still under the class lock).
    unsafe { &mut *(page as *mut PageHdr) }
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

struct SmallCache {
    lock: Spinlock<()>,
    /// Page INDEX of the first ACTIVE page (0 = none). Under lock.
    head: AtomicUsize,
    /// Forensics.
    pages_created: AtomicUsize,
    pages_reclaimed: AtomicUsize,
    obj_allocs: AtomicUsize,
    obj_frees: AtomicUsize,
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
// traffic; the class lock is taken once per ~MAG_DEPTH misses/overflows
// to refill/flush in batch.
//
// Correctness of deferred frees (a magazine-resident object is NOT yet
// reflected in its page header's free_count):
// - Reclaim needs free_count == objs_per_page UNDER the class lock; a
//   magazine entry for page P holds one of P's slots un-accounted, so
//   free_count can never reach objs_per_page while such an entry
//   exists — reclaim is blocked by construction and the slab bitmap
//   stays set. Flush runs the exact pre-magazine locked free body
//   (magic/cls re-validation, alignment checks, BAD_PAGE taints).
// - Double frees corrupt the page freelist exactly as they did before
//   (silently for partially-free pages, taint at the reclaim boundary);
//   the magazine only defers the moment, it adds no new failure mode.
// - Touching a magazine from IRQ context is impossible: every access
//   sits between save_and_disable_irq/restore_irq (or under the class
//   irqsave lock), and IRQ-context allocs/frees take the same guard.
// ---------------------------------------------------------------------------
const MAG_DEPTH: usize = 16;

#[derive(Clone, Copy)]
struct Magazine {
    buf: [usize; MAG_DEPTH],
    len: usize,
}

static mut MAGAZINES: [[Magazine; 8]; crate::config::MAX_CPUS] =
    [[Magazine { buf: [0; MAG_DEPTH], len: 0 }; 8]; crate::config::MAX_CPUS];

#[inline]
fn mag_of(cpu: usize, ci: usize) -> &'static mut Magazine {
    // SAFETY: cpu < MAX_CPUS and ci < 8 are checked by callers; each CPU
    // touches only its own row, always under local irq-off (or the class
    // irqsave lock), so no two accesses to a slot can overlap.
    unsafe { &mut MAGAZINES[cpu][ci] }
}

/// Allocate a small object. Returns null when the layout does not fit
/// this layer OR the backing page could not be created — the caller
/// falls through to the buddy either way.
pub fn alloc(size: usize, align: usize) -> *mut u8 {
    if size == 0 || size > SMALL_MAX || align > SMALL_MAX {
        return core::ptr::null_mut();
    }
    let (heap_start, _heap_end) = crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 {
        return core::ptr::null_mut(); // heap not up yet
    }
    let ci = class_of(size, align);
    let cache = &CACHES[ci];

    // Fast path: pop from this CPU's magazine (local irq-off only).
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        let mag = mag_of(cpu, ci);
        let flags = crate::arch::cpu::save_and_disable_irq();
        if mag.len > 0 {
            mag.len -= 1;
            let ptr = mag.buf[mag.len];
            crate::arch::cpu::restore_irq(flags);
            cache.obj_allocs.fetch_add(1, Ordering::Relaxed);
            return ptr as *mut u8;
        }
        crate::arch::cpu::restore_irq(flags);
    }

    // Slow path: one class-lock hold refills the magazine in batch and
    // returns the first object.
    let mut first: *mut u8 = core::ptr::null_mut();
    let _g = cache.lock.lock_irqsave();
    if cpu < crate::config::MAX_CPUS {
        let mag = mag_of(cpu, ci); // private: IF is off (irqsave)
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
    } else {
        first = alloc_locked(ci, heap_start);
    }
    drop(_g);
    first
}

/// The pre-magazine allocation body. Caller holds the class lock.
fn alloc_locked(ci: usize, heap_start: usize) -> *mut u8 {
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs_per_page: u16 = ((PAGE_SIZE - fo) / s) as u16;

    // 1. Serve from an active page; lazy-skip dead heads.
    let mut prev: u32 = 0; // page index of the previous live link point (0 = head)
    let mut cur = cache.head.load(Ordering::Relaxed) as u32;
    while cur != 0 {
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
        let next_free = unsafe { core::ptr::read_volatile(ptr as *const u16) } as usize;
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
    let page = unsafe { crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.alloc_page_raw() } as usize;
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
        unsafe { core::ptr::write_volatile((page + fo + i * s) as *mut u16, val) };
        i += 1;
    }
    // Serve object 0.
    let ptr = page + fo;
    let hdr = hdr_of(page);
    let next_free = unsafe { core::ptr::read_volatile(ptr as *const u16) } as usize;
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
    let (heap_start, heap_end) = crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 || addr < heap_start || addr >= heap_end {
        return false;
    }
    if !is_slab_ptr(addr, heap_start) {
        return false;
    }
    let page = addr & !(PAGE_SIZE - 1);
    // Pre-lock peek only to pick the class lock/magazine bin; everything
    // is re-validated under the lock at flush time (the page may have
    // been emptied, reclaimed and re-created as a different class
    // between the reads — the re-check catches that).
    let peek = hdr_of(page);
    let ci_guess = peek.cls as usize;
    if ci_guess >= CLASSES.len() {
        // Header garbage (transient or corrupt): consume, taint.
        crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
        return true;
    }
    let cache = &CACHES[ci_guess];

    // Fast path: push into this CPU's magazine (local irq-off only). A
    // magazine-resident object blocks its page's reclaim by construction
    // (see the magazine block comment), so no page-state check is due.
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        let mag = mag_of(cpu, ci_guess);
        let flags = crate::arch::cpu::save_and_disable_irq();
        if mag.len < MAG_DEPTH {
            mag.buf[mag.len] = addr;
            mag.len += 1;
            crate::arch::cpu::restore_irq(flags);
            cache.obj_frees.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        crate::arch::cpu::restore_irq(flags);
        // Magazine full: flush it entry by entry through the locked body
        // (self-locking, one hold per flushed object — 1/MAG_DEPTH of the
        // pre-magazine lock traffic), then this free the same way. Each
        // pop runs under its own irq-off window (an IRQ-context free on
        // this CPU may interleave between the free_locked calls).
        loop {
            let p = {
                let f = crate::arch::cpu::save_and_disable_irq();
                let r = if mag.len > 0 {
                    mag.len -= 1;
                    Some(mag.buf[mag.len])
                } else {
                    None
                };
                crate::arch::cpu::restore_irq(f);
                r
            };
            match p {
                Some(p) => {
                    free_locked(p, heap_start, heap_end, ci_guess);
                }
                None => break,
            }
        }
        return free_locked(addr, heap_start, heap_end, ci_guess);
    }

    // No magazine (cpu id out of range): the pre-magazine locked path.
    free_locked(addr, heap_start, heap_end, ci_guess)
}

/// The pre-magazine free body (validation + push to page freelist +
/// empty-page reclaim), self-locking on the ci_guess class lock (the
/// internal empty-page reclaim must drop the guard before the buddy
/// free, so the guard's ownership stays inside). Always returns true
/// (consumed by the slab layer).
fn free_locked(addr: usize, heap_start: usize, _heap_end: usize, ci_guess: usize) -> bool {
    let cache = &CACHES[ci_guess];
    let _g = cache.lock.lock_irqsave();
    let page = addr & !(PAGE_SIZE - 1);
    let hdr = hdr_of(page);
    if hdr.magic != HDR_MAGIC || hdr.cls as usize != ci_guess {
        // Reclaimed/re-created under us — no valid object can exist.
        crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
        return true;
    }
    let s = CLASSES[ci_guess];
    let fo = first_off(s);
    let objs_per_page = (PAGE_SIZE - fo) / s;
    let off = addr - page - fo;
    if addr == page || off % s != 0 || off / s >= objs_per_page {
        // Foreign/misaligned pointer into a live slab page: consume it —
        // the buddy free would destroy the page's 500-odd neighbours.
        crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
        return true;
    }
    if hdr.free_count as usize >= objs_per_page {
        // Double free — refuse, but consume it (do not also buddy-free).
        crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
        return true;
    }
    unsafe { core::ptr::write_volatile(addr as *mut u16, hdr.free_head) };
    hdr.free_head = (off / s) as u16;
    hdr.free_count += 1;
    cache.obj_frees.fetch_add(1, Ordering::Relaxed);

    if hdr.free_count as usize == objs_per_page {
        // Page empty: physically unlink (walk from head), then return it
        // to the buddy — all under the class lock, so no other CPU can be
        // mid-scan on this page.
        let pi = ((page - heap_start) / PAGE_SIZE) as u32;
        let mut prev: u32 = 0;
        let mut cur = cache.head.load(Ordering::Relaxed) as u32;
        while cur != 0 {
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
        drop(_g);
        unsafe { crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.free_page_raw(page as *mut u8) };
        return true;
    }
    if hdr.state == STATE_FULL {
        // Became partially free again: relink at the list head.
        hdr.state = STATE_ACTIVE;
        let pi = ((page - heap_start) / PAGE_SIZE) as u32;
        hdr.next_page = cache.head.load(Ordering::Relaxed) as u32;
        cache.head.store(pi as usize, Ordering::Relaxed);
    }
    true
}

/// One-line census for the ALLOCTHROW dump (raw UART, allocation-free).
pub fn census_line() {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    taskdump_raw_line(b"SMALLALLOC cls objs-alloc objs-free pages-made pages-reclaimed\n");
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
        taskdump_raw_line(b"\n");
    }
}
