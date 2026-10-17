//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Full physical-accounting ledger (fake-OOM family, dfx=ledger).
//!
//! `oom_forensic_dump` answers "is the allocator's own state consistent?"
//! This module answers the question one level up: WHERE DID THE RAM GO?
//! Every managed page is classified into exactly one bucket (single
//! descriptor pass, precedence-ordered), the buckets are summed back
//! against the zone counters, and any gap is printed as an explicit
//! mismatch line. At the moment of a fake OOM the ledger shows which
//! category swallowed the "free" memory — anonymous, page tables, pinned
//! page cache, or accounting drift (pages that are nobody's).
//!
//! Buckets (first match wins):
//!   free_linked    refcount==0, OnFreelist          (should == zone free_cnt)
//!   free_orphan    refcount==0, not linked, not reserved   <= LEAK
//!   reserved       PageFlag::Reserved               (kernel image/heap/slab)
//!   page_tables    PT_LEDGER live stamp             (exact bitmap)
//!   anon           PageFlag::Anonymous
//!   cow            PageFlag::Cow, not anonymous
//!   mapped_file    mapcount>0, not anonymous        (page cache in a PTE)
//!   lru_file       PageFlag::Lru, not anon/mapped   (clean page cache)
//!   other_used     refcount>0, none of the above    (kernel objects,
//!                  stacks, pipe/skb buffers, pinned frames, ...)
//!
//! Output goes through printk (println!) — like oom_forensic_dump it is
//! meant for the OOM moment (task context, scheduler alive) and for the
//! on-demand UART magic "LEDGER!" (deferred to the timer softirq, which
//! already hosts the periodic dumps). It must NOT be called while the
//! caller holds the zone lock (the census only reads descriptors, but a
//! printk under the zone lock is the classic self-wedge).
//!
//! Zero heap allocation: counters are stack locals, and the compact line
//! rides the raw-UART taskdump writer with a fixed stack buffer (the
//! kernel heap cannot be trusted at the moment this dump exists for).

use core::sync::atomic::{AtomicBool, Ordering};

use super::taskdump::taskdump_raw_line;

/// Set by the UART magic ("LEDGER!"); consumed by the timer softirq.
pub static PENDING: AtomicBool = AtomicBool::new(false);

/// Decimal -> fixed stack buffer; returns the written slice.
fn dec_buf(n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    let mut n = n;
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    &buf[i..]
}

/// Append a decimal to a stack line buffer (silently truncated at the
/// cap — every field below is far shorter than the budget).
struct Line {
    buf: [u8; 160],
    len: usize,
}

impl Line {
    fn new(head: &[u8]) -> Self {
        let mut l = Line { buf: [0; 160], len: 0 };
        l.push(head);
        l
    }
    fn push(&mut self, s: &[u8]) {
        let room = self.buf.len() - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
        self.len += n;
    }
    fn dec(&mut self, n: u64) {
        let mut b = [0u8; 20];
        self.push(dec_buf(n, &mut b));
    }
    fn emit(self) {
        let mut me = self;
        me.buf[me.len] = b'\n';
        let end = me.len + 1;
        taskdump_raw_line(&me.buf[..end]);
    }
}

/// O(1) counters on the raw-UART path (printk-free, safe from any
/// context): zone free, PT pages, page cache, heap, slab, swap.
pub fn compact_line() {
    let mut l = Line::new(b"MEMLEDGER-COMPACT");
    if let Some(node) = crate::mm::pglist::first_online_node() {
        l.push(b" memfree_pages=");
        l.dec(node.free_pages() as u64);
    }
    #[cfg(any(feature = "riscv64", feature = "x86_64"))]
    {
        l.push(b" pt_pages=");
        l.dec(crate::arch::mm::PtLedger::live_count() as u64);
    }
    l.push(b" pagecache_pages=");
    l.dec(crate::fs::page_cache::page_cache_total_pages() as u64);
    let heap = crate::mm::buddy_allocator::buddy_stats();
    l.push(b" heapK=");
    l.dec((heap.used_bytes / 1024) as u64);
    l.push(b"/");
    l.dec((heap.heap_size / 1024) as u64);
    let slab = crate::mm::slab::slab_stats();
    l.push(b" slab_pages=");
    l.dec(slab.total_pages as u64);
    l.push(b" swap_active=");
    l.dec(crate::mm::swap::nr_active_swap() as u64);
    l.emit();
}

/// One bucket row of the census.
#[derive(Default)]
struct Census {
    /// refcount==0 and OnFreelist — the LEADER page of a buddy free block
    /// (the flag marks leaders only, see PageFlag::OnFreelist / R15-6).
    free_leader: u64,
    /// refcount==0, not reserved, not a leader — the MEMBER pages of
    /// higher-order free blocks. Not a leak: the leader owns them.
    free_member: u64,
    reserved: u64,
    page_tables: u64,
    anon: u64,
    cow: u64,
    mapped_file: u64,
    lru_file: u64,
    other_used: u64,
    // refcount distribution of used pages (pinned/shared shape)
    rc_eq1: u64,
    rc_2_8: u64,
    rc_gt8: u64,
}

/// The full census + reconciliation. printk-based; see the module doc for
/// context restrictions. `site` names the trigger (OOM moment, UART magic).
pub fn dump(site: &str) {
    // One dump at a time (the timer-softirq trigger and an OOM can race;
    // interleaved ledgers are worse than a dropped one).
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    crate::println!("==== MEMLEDGER @ {} ====", site);
    compact_line();

    // Zone counters (what the allocator believes).
    let mut zone_free: u64 = 0;
    let mut zone_managed: u64 = 0;
    if let Some(node) = crate::mm::pglist::first_online_node() {
        for zt in [
            crate::mm::zone::ZoneType::ZoneNormal,
            crate::mm::zone::ZoneType::ZoneDma32,
            crate::mm::zone::ZoneType::ZoneDma,
            crate::mm::zone::ZoneType::ZoneMovable,
        ] {
            if let Some(zone) = node.zone(zt) {
                if !zone.is_initialized() {
                    continue;
                }
                zone_free += zone.nr_free() as u64;
                zone_managed += zone.managed_pages() as u64;
                crate::println!(
                    "MEMLEDGER zone {} free_cnt={} managed={} wmark={}/{}/{}",
                    zone.zone_type().name(),
                    zone.nr_free(),
                    zone.managed_pages(),
                    zone.watermark(crate::mm::zone::WMARK_MIN),
                    zone.watermark(crate::mm::zone::WMARK_LOW),
                    zone.watermark(crate::mm::zone::WMARK_HIGH),
                );
            }
        }
        // LRU sizes (counter view; the census below walks the pages).
        for lru_i in 0..crate::mm::pglist::NR_LRU_LISTS {
            let sz = node.lru_sizes[lru_i].load(Ordering::Relaxed);
            if sz > 0 {
                crate::println!("MEMLEDGER lru[{}] size={}", lru_i, sz);
            }
        }
    }

    // Full descriptor census (bounded by what vmemmap actually mapped —
    // same dereference-safety rule as page_desc_stats).
    use crate::mm::page_desc::{nr_descs, pfn_to_page, PageFlag, MIN_PFN};
    let scan = nr_descs().min(crate::mm::vmemmap::vmemmap_stats().nr_pages);
    #[cfg(any(feature = "riscv64", feature = "x86_64"))]
    let pt_count = crate::arch::mm::PtLedger::live_count() as u64;
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    let pt_count = 0u64;

    let mut c = Census::default();
    for i in 0..scan {
        let page = pfn_to_page(MIN_PFN + i);
        if page.is_null() {
            continue;
        }
        // SAFETY: read-only descriptor access inside the mapped vmemmap
        // span; the flags atomics are the same ones every other reader
        // uses lock-free.
        let (rc, linked, reserved) = unsafe {
            (
                (*page).refcount(),
                (*page).test_flag(PageFlag::OnFreelist),
                (*page).is_reserved(),
            )
        };
        if rc == 0 {
            if reserved {
                c.reserved += 1;
            } else if linked {
                c.free_leader += 1;
            } else {
                c.free_member += 1;
            }
            continue;
        }
        if rc == 1 {
            c.rc_eq1 += 1;
        } else if rc <= 8 {
            c.rc_2_8 += 1;
        } else {
            c.rc_gt8 += 1;
        }
        if reserved {
            c.reserved += 1;
            continue;
        }
        #[cfg(any(feature = "riscv64", feature = "x86_64"))]
        {
            if crate::arch::mm::PT_LEDGER.peek((MIN_PFN + i) as u64) != 0 {
                c.page_tables += 1;
                continue;
            }
        }
        let (anon, cow, mapped, lru) = unsafe {
            (
                (*page).is_anonymous(),
                (*page).is_cow(),
                (*page).is_mapped(),
                (*page).test_flag(PageFlag::Lru),
            )
        };
        if anon {
            c.anon += 1;
        } else if cow {
            c.cow += 1;
        } else if mapped {
            c.mapped_file += 1;
        } else if lru {
            c.lru_file += 1;
        } else {
            c.other_used += 1;
        }
    }

    let mb = |p: u64| p * 4 / 1024;
    let free_census = c.free_leader + c.free_member;
    crate::println!(
        "MEMLEDGER census scan={} descs",
        scan
    );
    crate::println!(
        "MEMLEDGER   free={}L+{}M={} ({} MB) reserved={} ({} MB) page_tables={} exact={}{}",
        c.free_leader, c.free_member, free_census, mb(free_census),
        c.reserved, mb(c.reserved), c.page_tables, pt_count,
        if c.page_tables != pt_count { "  <== PT-BITMAP MISMATCH" } else { "" }
    );
    crate::println!(
        "MEMLEDGER   anon={} ({} MB) cow={} mapped_file={} lru_file={} other_used={}",
        c.anon, mb(c.anon), c.cow, c.mapped_file, c.lru_file, c.other_used
    );
    crate::println!(
        "MEMLEDGER   refcounts(rc>0): ==1:{} 2..8:{} >8:{}",
        c.rc_eq1, c.rc_2_8, c.rc_gt8
    );

    // Reconciliation: every descriptor is in exactly one bucket.
    let sum = free_census
        + c.reserved
        + c.page_tables
        + c.anon
        + c.cow
        + c.mapped_file
        + c.lru_file
        + c.other_used;
    crate::println!(
        "MEMLEDGER sum={} vs descs={}{}",
        sum,
        nr_descs(),
        if sum != nr_descs() as u64 { "  <== CENSUS GAP" } else { "" }
    );
    // free census vs zone counter: both count free PAGES (members of free
    // blocks included). A census EXCESS means refcount==0 pages nobody
    // owns (true orphans / leak); a counter excess means the zone believes
    // pages free whose descriptors say otherwise (inverse corruption) —
    // or transient churn while the non-atomic census ran (a few thousand
    // pages under load is race noise; a monotonic gap is not).
    crate::println!(
        "MEMLEDGER free_census={} vs zone_free_cnt={} drift={}",
        free_census,
        zone_free,
        free_census as i64 - zone_free as i64
    );
    crate::println!(
        "MEMLEDGER zone_managed={} vs descs-reserved={} (heap/image/slab live outside the zone span)",
        zone_managed,
        nr_descs() as u64 - c.reserved
    );
    // Zero-put / free-refusal attribution (freelist-drift family): which
    // release path's refcount-0 transitions did NOT pair with a zone free.
    super::driftwatch::report();
    crate::println!("==== MEMLEDGER end ====");

    BUSY.store(false, Ordering::Release);
}

/// Deferred trigger: the UART RX magic sets PENDING; the timer softirq
/// (which already hosts the periodic DFX dumps and may printk) runs it.
pub fn run_pending() {
    if PENDING.swap(false, Ordering::AcqRel) {
        dump("uart-magic");
    }
}
