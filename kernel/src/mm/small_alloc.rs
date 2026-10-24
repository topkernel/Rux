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
//! through the page headers (page-index linkage, 0 = end). A page that
//! fills up is unlinked immediately (the scan carries its prev); an
//! EMPTY page is physically unlinked (walk from head) and returned to
//! the buddy while still under the class lock — no other CPU can be
//! scanning it at that moment, so there is no use-after-reclaim race.
//! - Per-CPU magazines (pa16) front the class locks: alloc pops / free
//! pushes run under LOCAL irq-off only; the class lock is taken once
//! per batch refill/flush (see the magazine block comment below).
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
    /// Class-lock acquisitions (slow paths only — one per magazine
    /// refill/flush batch, pre-magazine one per operation). Printed by
    /// the census so a boot's lock-traffic win is directly readable
    /// against obj_allocs+obj_frees on the same line.
    lock_holds: AtomicUsize,
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
// - Touching a magazine from IRQ context is serialized: every access
//   sits between save_and_disable_irq/restore_irq (or under the class
//   irqsave lock), and IRQ-context allocs/frees take the same guard.
// - Counters (obj_allocs/obj_frees) are bumped ONLY in the locked
//   bodies — at page-freelist removal/insertion — so every object is
//   counted exactly once per lifecycle; magazine-resident objects skew
//   the live estimate by at most MAX_CPUS*8*MAG_DEPTH (bounded).
// ---------------------------------------------------------------------------
const MAG_DEPTH: usize = 16;

#[derive(Clone, Copy)]
struct Magazine {
    buf: [usize; MAG_DEPTH],
    len: usize,
}

static mut MAGAZINES: [[Magazine; 8]; crate::config::MAX_CPUS] =
    [[Magazine { buf: [0; MAG_DEPTH], len: 0 }; 8]; crate::config::MAX_CPUS];

/// Pointer to this CPU's class-ci magazine row. Callers must have IF=0
/// (own irq-off window or the class irqsave hold), making the row
/// private to this CPU for the duration of the access.
#[inline]
fn mag_ptr(cpu: usize, ci: usize) -> *mut Magazine {
    // SAFETY: addr_of_mut on the static avoids taking a reference to a
    // static mut; the flat index matches the [[Magazine; 8]; MAX_CPUS]
    // row-major layout. Aliasing is impossible under the caller's IF=0.
    unsafe { core::ptr::addr_of_mut!(MAGAZINES).cast::<Magazine>().add(cpu * 8 + ci) }
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

    // Fast path: pop from THIS CPU's magazine. cpu_id() is read inside
    // the irq-off window — with IF=0 the task cannot be preempted and
    // migrated between the read and the row access.
    {
        let flags = crate::arch::cpu::save_and_disable_irq();
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < crate::config::MAX_CPUS {
            let mag = unsafe { &mut *mag_ptr(cpu, ci) };
            if mag.len > 0 {
                mag.len -= 1;
                let ptr = mag.buf[mag.len];
                crate::arch::cpu::restore_irq(flags);
                return ptr as *mut u8;
            }
        }
        crate::arch::cpu::restore_irq(flags);
    }

    // Slow path: ONE class-lock hold refills the magazine in batch and
    // returns the first object. The irqsave guard has IF=0 + preemption
    // disabled, so the cpu_id() read below is stable for the hold.
    let cache = &CACHES[ci];
    let mut first: *mut u8 = core::ptr::null_mut();
    let _g = cache.lock.lock_irqsave();
    cache.lock_holds.fetch_add(1, Ordering::Relaxed);
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        let mag = unsafe { &mut *mag_ptr(cpu, ci) }; // private: IF=0
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
    // Pre-lock peek only to pick the class/magazine bin; everything is
    // re-validated under the lock at flush time (the page may have been
    // emptied, reclaimed and re-created as a different class between
    // the reads — the re-check catches that).
    let peek = hdr_of(page);
    let ci_guess = peek.cls as usize;
    if ci_guess >= CLASSES.len() {
        // Header garbage (transient or corrupt): consume, taint.
        crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
        return true;
    }

    // Fast path: screen the pointer against the page header INSIDE the
    // irq-off window (fresh magic/cls read, alignment, free_count head
    // room), then push into THIS CPU's magazine. Anything the locked
    // body would consume-and-taint is routed there instead — a bad
    // pointer must never park in a magazine, because a later alloc pop
    // on this CPU would hand it out as a live object.
    let flags = crate::arch::cpu::save_and_disable_irq();
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        let s = CLASSES[ci_guess];
        let fo = first_off(s);
        let objs_per_page = (PAGE_SIZE - fo) / s;
        let off = addr - page - fo;
        let h = hdr_of(page); // re-read under IF=0: another CPU may have
        // changed the header since the peek; the flush re-validates
        // under the class lock regardless.
        if h.magic == HDR_MAGIC
            && h.cls as usize == ci_guess
            && addr != page
            && off % s == 0
            && off / s < objs_per_page
            && (h.free_count as usize) < objs_per_page
        {
            let mag = unsafe { &mut *mag_ptr(cpu, ci_guess) };
            if mag.len < MAG_DEPTH {
                mag.buf[mag.len] = addr;
                mag.len += 1;
                crate::arch::cpu::restore_irq(flags);
                return true;
            }
            // Magazine full: snapshot the whole row out under THIS
            // window (no row pointer survives the window), then flush
            // the snapshot and push addr after — the batch flush below
            // empties the row, so the push always has room.
            let mut snap: [usize; MAG_DEPTH] = [0; MAG_DEPTH];
            let mut n = 0usize;
            while mag.len > 0 {
                mag.len -= 1;
                snap[n] = mag.buf[mag.len];
                n += 1;
            }
            crate::arch::cpu::restore_irq(flags);
            flush_batch(&snap[..n], ci_guess, heap_start);
            // Push addr into the (now empty) row. cpu_id re-read inside
            // the new window: the task may have been preempted and
            // migrated during the flush, and must not touch a stale
            // row. An IRQ-context free that ran meanwhile leaves the
            // row with fewer than MAG_DEPTH entries — the check still
            // guards; the locked fallback covers the impossible rest.
            let f2 = crate::arch::cpu::save_and_disable_irq();
            let c2 = crate::arch::cpu_id() as usize;
            if c2 < crate::config::MAX_CPUS {
                let mag = unsafe { &mut *mag_ptr(c2, ci_guess) };
                if mag.len < MAG_DEPTH {
                    mag.buf[mag.len] = addr;
                    mag.len += 1;
                    crate::arch::cpu::restore_irq(f2);
                    return true;
                }
            }
            crate::arch::cpu::restore_irq(f2);
            flush_batch(core::slice::from_ref(&addr), ci_guess, heap_start);
            return true;
        }
    }
    crate::arch::cpu::restore_irq(flags);
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
    cache.lock_holds.fetch_add(1, Ordering::Relaxed);
    for &addr in entries {
        let page = addr & !(PAGE_SIZE - 1);
        let hdr = hdr_of(page);
        if hdr.magic != HDR_MAGIC || hdr.cls as usize != ci {
            // Reclaimed/re-created under us — no valid object can exist.
            crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
            continue;
        }
        let off = addr - page - fo;
        if addr == page || off % s != 0 || off / s >= objs_per_page {
            // Foreign/misaligned pointer into a live slab page: consume
            // it — the buddy free would destroy the page's 500-odd
            // neighbours.
            crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
            continue;
        }
        if hdr.free_count as usize >= objs_per_page {
            // Double free — refuse, but consume it (do not also
            // buddy-free).
            crate::dfx::taint::add_taint(crate::dfx::taint::TaintFlags::BAD_PAGE);
            continue;
        }
        unsafe { core::ptr::write_volatile(addr as *mut u16, hdr.free_head) };
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
    drop(_g);
    let mut i = 0usize;
    while i < nret {
        unsafe {
            crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.free_page_raw(ret[i] as *mut u8)
        };
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
pub fn census_peek(ci: usize, per_page: usize) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    if ci >= CLASSES.len() {
        return;
    }
    let (heap_start, _heap_end) =
        crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.heap_bounds();
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
            let a0 = unsafe { core::ptr::read_volatile(p as *const u64) };
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
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    if ci >= CLASSES.len() {
        return;
    }
    let (heap_start, _heap_end) =
        crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.heap_bounds();
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
            let a0 = unsafe { core::ptr::read_volatile(p as *const u64) };
            let a1 = unsafe { core::ptr::read_volatile((p + 8) as *const u64) };
            let a2 = unsafe { core::ptr::read_volatile((p + 16) as *const u64) };
            if a0 == 0 && a1 == 0 && a2 == 0 {
                i += 1;
                continue;
            }
            total += 1;
            // BTree node length: u16 at +10 (parent 8B, parent_idx u16).
            let blen = unsafe { core::ptr::read_volatile((p + 10) as *const u16) } as u64;
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

/// Leg-17 census peek v4: the v3 roll proved the c1024 population is
/// SCATTERED (~3 live objects per held page — 9.9K live over 3.3K pages),
/// so the 32-page walk sampled 1% of it. This walks up to 512 pages and
/// DUMPS the first `dump_n` objects' 4 qwords per shape bucket (raw
/// UART, allocation-free; reads under the class lock, prints after).
pub fn census_peek4(ci: usize, dump_n: usize) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    if ci >= CLASSES.len() {
        return;
    }
    let (heap_start, _heap_end) =
        crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 {
        return;
    }
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs = (PAGE_SIZE - fo) / s;

    #[inline]
    fn is_kheap(a: u64) -> bool {
        a >= 0xffff_8880_0000_0000
    }
    #[inline]
    fn is_userva(a: u64) -> bool {
        (0x0000_7f00_0000_0000..0x0000_8000_0000_0000).contains(&a)
    }

    let mut dumped = 0usize;
    let mut pages = 0usize;
    let _g = cache.lock.lock_irqsave();
    let mut page = cache.head.load(Ordering::Relaxed) as u32;
    'walk: while page != 0 && pages < 512 {
        let page_addr = heap_start + page as usize * PAGE_SIZE;
        let hdr = hdr_of(page_addr);
        if hdr.magic != HDR_MAGIC || hdr.state != STATE_ACTIVE {
            break;
        }
        let mut i = 0usize;
        while i < objs {
            // SAFETY: live slab page of this class; slot in bounds.
            let p = page_addr + fo + i * s;
            let a0 = unsafe { core::ptr::read_volatile(p as *const u64) };
            let a1 = unsafe { core::ptr::read_volatile((p + 8) as *const u64) };
            let a2 = unsafe { core::ptr::read_volatile((p + 16) as *const u64) };
            let a3 = unsafe { core::ptr::read_volatile((p + 24) as *const u64) };
            if (a0 != 0 || a1 != 0 || a2 != 0)
                && (is_userva(a0) || (dumped & 1) == 1)
            {
                if dumped < dump_n {
                    taskdump_raw_line(b"  P4 ");
                    taskdump_dec(a0);
                    taskdump_raw_line(b" ");
                    taskdump_dec(a1);
                    taskdump_raw_line(b" ");
                    taskdump_dec(a2);
                    taskdump_raw_line(b" ");
                    taskdump_dec(a3);
                    taskdump_raw_line(b"\n");
                }
                dumped += 1;
            }
            if dumped >= dump_n {
                break 'walk;
            }
            i += 1;
        }
        page = hdr.next_page;
        pages += 1;
    }
    drop(_g);
    taskdump_raw_line(b"PEEK4 c");
    taskdump_dec(s as u64);
    taskdump_raw_line(b": walked=");
    taskdump_dec(pages as u64);
    taskdump_raw_line(b" dumped=");
    taskdump_dec(dumped as u64);
    taskdump_raw_line(b"\n");
}

/// Leg-17 census peek LIVE: the v2-v4 walks only reached ACTIVE-listed
/// pages (pages with free slots) — the population actually sits on FULL
/// pages (all-live, unlinked: c1024 held 3305 pages x ~3 slots = exactly
/// its 9.9K live census count) and free slots keep STALE data (only the
/// low u16 is the freelist link), so earlier peeks sampled garbage.
/// This scans the WHOLE heap for magic+cls slab pages and classifies
/// only LIVE slots: free_count is authoritative (a FULL page has no
/// chain to walk; an ACTIVE page's chain length == free_count).
pub fn census_peek_live(ci: usize, dump_n: usize) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    if ci >= CLASSES.len() {
        return;
    }
    let (heap_start, heap_end) =
        crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.heap_bounds();
    if heap_start == 0 {
        return;
    }
    let cache = &CACHES[ci];
    let s = CLASSES[ci];
    let fo = first_off(s);
    let objs = (PAGE_SIZE - fo) / s;
    if objs > 64 {
        return; // free_set cap; only used for c256/c512/c1024
    }

    #[inline]
    fn is_kheap(a: u64) -> bool {
        a >= 0xffff_8880_0000_0000
    }
    #[inline]
    fn is_userva(a: u64) -> bool {
        (0x0000_7f00_0000_0000..0x0000_8000_0000_0000).contains(&a)
    }

    let mut pages_seen: u64 = 0;
    let mut live: u64 = 0;
    let (mut n_arc, mut n_vma, mut n_work) = (0u64, 0u64, 0u64);
    let (mut n_uv, mut n_kh, mut n_sm, mut n_ot) = (0u64, 0u64, 0u64, 0u64);
    let mut dumped = 0usize;
    let _g = cache.lock.lock_irqsave();
    let heap_pages = (heap_end - heap_start) / PAGE_SIZE;
    let mut pi = 0usize;
    while pi < heap_pages {
        let page_addr = heap_start + pi * PAGE_SIZE;
        let hdr = hdr_of(page_addr);
        if hdr.magic == HDR_MAGIC
            && hdr.cls as usize == ci
            && (hdr.state == STATE_ACTIVE || hdr.state == STATE_FULL)
        {
            pages_seen += 1;
            // Free-slot set: walk the freelist chain ONLY on pages that
            // report free slots (free_count authoritative; on a FULL
            // page free_head is stale and every slot is live).
            let mut free_set = [false; 64];
            if hdr.free_count > 0 {
                let mut idx = hdr.free_head as usize;
                let mut steps = 0usize;
                while idx != 0xFFFF && idx < objs && steps <= objs as usize {
                    if idx < 64 {
                        free_set[idx] = true;
                    }
                    // SAFETY: slot of this page; first u16 is the link.
                    let p = page_addr + fo + idx * s;
                    idx = unsafe { core::ptr::read_volatile(p as *const u16) } as usize;
                    steps += 1;
                }
            }
            let mut i = 0usize;
            while i < objs {
                if !free_set[i] {
                    // SAFETY: live slab slot of this class, in bounds.
                    let p = page_addr + fo + i * s;
                    let a0 = unsafe { core::ptr::read_volatile(p as *const u64) };
                    let a1 = unsafe { core::ptr::read_volatile((p + 8) as *const u64) };
                    let a2 = unsafe { core::ptr::read_volatile((p + 16) as *const u64) };
                    let a3 = unsafe { core::ptr::read_volatile((p + 24) as *const u64) };
                    live += 1;
                    let blen =
                        unsafe { core::ptr::read_volatile((p + 10) as *const u16) } as u64;
                    if a0 <= 4096 && a1 <= 4096 && a2 != 0 && a2 < (1 << 24) {
                        n_arc += 1;
                    } else if (a0 == 0 || is_kheap(a0))
                        && (1..=11).contains(&blen)
                        && is_userva(a2)
                    {
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
                    if dumped < dump_n {
                        taskdump_raw_line(b"  PL ");
                        taskdump_dec(a0);
                        taskdump_raw_line(b" ");
                        taskdump_dec(a1);
                        taskdump_raw_line(b" ");
                        taskdump_dec(a2);
                        taskdump_raw_line(b" ");
                        taskdump_dec(a3);
                        taskdump_raw_line(b"\n");
                        dumped += 1;
                    }
                }
                i += 1;
            }
        }
        pi += 1;
    }
    drop(_g);
    taskdump_raw_line(b"PEEKLIVE c");
    taskdump_dec(s as u64);
    taskdump_raw_line(b": pages=");
    taskdump_dec(pages_seen);
    taskdump_raw_line(b" live=");
    taskdump_dec(live);
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
        taskdump_raw_line(b" locks=");
        taskdump_dec(c.lock_holds.load(Ordering::Relaxed) as u64);
        taskdump_raw_line(b"\n");
    }
}
