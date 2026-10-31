//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Page Cache — per-inode file data cache layering on top of bio block cache.
//!
//! Caches 4KB file data pages keyed by ((fs_id, inode_number), page_index).
//! The fs_id component (review 5.3: 页缓存键 ino:u32 截断/跨文件系统串页)
//! separates inodes of different filesystem instances that would otherwise
//! collide on the same small inode number.
//! Reduces disk I/O for repeated reads and enables read-ahead population.
//!
//! Pages are allocated from the zone allocator as physical page frames,
//! placed on LRU_INACTIVE_FILE for proper reclaim integration.  Eviction
//! walks the LRU list in access-recency order, not BTreeMap key order.
//!
//! Physical pages are accessed through the linear mapping (phys_to_virt),
//! not through identity mapping, since the kernel only identity-maps a
//! small region around the kernel image.
//!
//! LOCKING (pcache Option B stage 1, perf-report 8.10.27/8.10.28 — the
//! fdtable lever-B shape, 8.10.22, applied to the inode map). The old
//! shape was ONE global `Spinlock<BTreeMap<u64, InodePageCache>>`: every
//! fault paid the global lock acquire + two BTreeMap walks, and under the
//! cold-start load phase (100MB of libs faulted page-by-page on 4 CPUs,
//! thousands of distinct inodes) every fault serialized on that lock.
//! Now:
//!
//! - **Global lookup is lock-free.** The inode map is a fixed-capacity
//!   open-addressed slot table (`slots: [PcSlot; PC_TABLE_CAP]`, linear
//!   probing on the mixed cache_key). Readers take NO lock: with irqs off
//!   they claim their CPU's reader bank (SeqCst RMW), probe the slots
//!   (Acquire loads), claim a strong count on the found entry, and drop
//!   the bank. CPU ids outside the bank range serialize through
//!   `slow_read` (the r8final discipline).
//! - **Per-inode lock.** Each `InodePageCache` carries its own
//!   `Spinlock<BTreeMap<u64, CachedPage>>` — the guard is the &mut access
//!   to the map and every CachedPage field — so cross-INODE fault
//!   contention is zero (the load phase faults many files at once; within
//!   one inode the faults serialize on the VFS/mm side anyway). The
//!   leg-54 locked-plain state-op discipline (86a458f9) transfers
//!   verbatim to the per-inode lock: pin/unpin/count/release helpers are
//!   plain accesses valid only under the owning entry's lock; read-only
//!   scan sites keep atomic loads (`dirty` is always touched through
//!   atomic ops and never here).
//! - **Writers serialize on `wlock`** (entry insert/remove, eviction,
//!   table iteration — rare and unchanged in shape from the old global
//!   lock): `wlock → per-inode` nesting ONLY, never the reverse
//!   (put-style paths drop the per-inode lock before taking wlock to
//!   re-verify emptiness). No allocation ever runs under either lock
//!   (R-WRITESTORM-1: the reclaim slowpath reaches shrink → wlock).
//! - **Whole-entry removal parks and drains** exactly like
//!   FdTable.retired (8.10.22): the slot is tombstoned under wlock, the
//!   slot's reference is parked in `retired`, and the drop happens only
//!   after a drain observes every reader bank at zero (SeqCst fence +
//!   Acquire scans + slow_read held across the take). A reader that
//!   already claimed the entry keeps it alive via its strong count; a
//!   reader that loads the tombstone sees a miss and re-fills from disk
//!   (the pages map was empty — that is WHY the entry was removed).
//!   A false miss is always safe: the miss path re-reads the file and
//!   `insert` dedups against the live entry.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use crate::sync::spinlock::{Spinlock, SpinlockGuard};
use crate::mm::page_alloc::{alloc_page, alloc_page_batch, free_page};
use crate::mm::zone::{GfpFlags, ZoneType};
use crate::mm::page_desc::{PageFlag, PageType, pfn_to_page_mut, Page};
use crate::mm::lru;
use crate::mm::pglist::{first_online_node, LRU_INACTIVE_FILE};
use crate::mm::{pfn_to_phys, PAGE_SIZE};
use crate::arch::mm::{phys_to_virt, PhysAddr};

/// Minimum cached pages across all inodes (8192 x 4KB = 32MB).
///
/// Was the WHOLE cap (a fixed `MAX_CACHED_PAGES = 8192`): smaller than one
/// Ubuntu binary's text+data working set, so every exec/read cycle
/// re-fetched from disk and a 10MB sequential read thrashed the LRU 5x
/// over. It is now the FLOOR of the dynamic cap below.
const MIN_CACHED_PAGES: usize = 8192;

/// Fraction of managed RAM the page cache may occupy before inserts evict
/// (soft cap — see `max_cached_pages`).
///
/// The fixed 32MB cap was the OH `.so`-load thrash: the image's shared
/// libraries alone are gigabytes, so with ~580 tasks faulting libraries
/// EVERY fault was a cache miss + eviction + disk refetch + ext4 big-lock
/// round trip, and measured ~1.5 s per fault (`SA:3002 OpenSo 802166ms`).
/// Linux scales the page cache to reclaimable memory; Rux now caps it at
/// half of managed RAM (reclaim still shrinks it below watermark via
/// `shrink`, so user allocations keep headroom).
const PAGE_CACHE_CAP_RAM_DIVISOR: usize = 2;

/// Cached dynamic cap (0 = not computed yet). Computed lazily on first
/// use; falls back to `MIN_CACHED_PAGES` before zone init.
static PAGE_CACHE_CAP: AtomicUsize = AtomicUsize::new(0);

/// Soft cap on total cached pages: half of managed RAM, at least 32MB.
///
/// Computed once (atomic compare-exchange); before the zone system is up
/// the old fixed floor applies. Reads managed pages from all initialized
/// zones — no allocation, no locks, safe from any insert path.
#[inline]
fn max_cached_pages() -> usize {
    let cached = PAGE_CACHE_CAP.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let mut total = 0usize;
    if let Some(node) = first_online_node() {
        for zt in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
            if let Some(zone) = node.zone(zt) {
                if zone.is_initialized() {
                    total += zone.managed_pages();
                }
            }
        }
    }
    if total == 0 {
        return MIN_CACHED_PAGES; // zones not up yet — old fixed cap
    }
    let cap = core::cmp::max(total / PAGE_CACHE_CAP_RAM_DIVISOR, MIN_CACHED_PAGES);
    let _ = PAGE_CACHE_CAP.compare_exchange(0, cap, Ordering::AcqRel, Ordering::Acquire);
    PAGE_CACHE_CAP.load(Ordering::Relaxed)
}

/// Page-cache observability counters (Rux extension, /proc/meminfo
/// `PageCacheOps:` line). Cheap relaxed atomics on the insert/evict paths;
/// the point is ratio visibility on OH boots (hits vs misses vs evictions
/// under the real service mixture).
static STAT_HITS: AtomicU64 = AtomicU64::new(0);
static STAT_MISSES: AtomicU64 = AtomicU64::new(0);
static STAT_EVICTIONS: AtomicU64 = AtomicU64::new(0);
static STAT_DEMOTIONS: AtomicU64 = AtomicU64::new(0);
/// Re-entrancy gate for shrink_reclaim's dirty-flush I/O (see there).
static WRITEBACK_GUARD: AtomicBool = AtomicBool::new(false);
/// Slot-table degradation counter: inserts refused because the probe
/// found no free slot. Correctness is never affected (the page is simply
/// not cached; the caller re-reads from disk next time); a non-zero count
/// on a standard boot names a workload with more live inodes than
/// PC_TABLE_CAP and is worth resizing the table for.
static PC_TABLE_FULL: AtomicUsize = AtomicUsize::new(0);
/// Drain give-up counter (fdtable parity, 8.10.22): a drain that could
/// not observe all reader banks at zero within its spin budget defers the
/// retired drop to the next writer. Correctness is never affected.
static PC_DRAIN_DEFERS: AtomicUsize = AtomicUsize::new(0);

/// A cached page of file data, backed by a zone-allocated physical page frame.
struct CachedPage {
    /// Physical frame number (allocated from zone allocator).
    pfn: usize,
    /// Reference count counting EXTERNAL pins (get..put windows). Pages are
    /// born at 0 — the cache's own ownership is not counted. (R7-C2: birth
    /// ref 1 with no matching put pinned every page forever, disabling
    /// eviction and making invalidate keep stale data.)
    ref_count: AtomicU32,
    /// Set when invalidate_inode found the page pinned: freed by the final
    /// put() instead of under the reader's feet.
    invalidated: bool,
    /// R14-17: one-shot release latch — put() and the evict/shrink paths
    /// can both reach the release for the same entry; the frame must be
    /// returned to the zone exactly once (the zone dblfree stream).
    released: core::sync::atomic::AtomicBool,
    /// Session 25 (MAP_SHARED writeback): the frame holds stores from a
    /// writable shared mapping that have NOT reached the file yet. Set
    /// conservatively at W-map install time (a writable mapping MAY write;
    /// flushing an unwritten page is harmless I/O), cleared by the
    /// msync/fsync flush. Dirty entries skip eviction — their content
    /// exists nowhere else but the frame.
    dirty: core::sync::atomic::AtomicBool,
}

impl CachedPage {
    /// Leg-54 item A (perf-report 8.10.24, sampler page 80473000 = 16.2%
    /// of launcher kernel time): every ref_count / released transition
    /// happens under a cache lock — the CAS-loop atomics were machinery
    /// the lock already subsumed. Since pcache Option B stage 1
    /// (8.10.27) that lock is the ENTRY's per-inode Spinlock (the old
    /// global inodes lock covered these same ops through leg-54). The
    /// STORAGE stays atomic for layout stability; these locked helpers
    /// do plain loads/stores through get_mut, which is sound ONLY while
    /// the owning entry's per-inode lock is held — new sites must keep
    /// that invariant (read-only scan sites elsewhere keep atomic loads;
    /// `dirty` is always touched through atomic ops and never here).
    #[inline]
    fn pin_locked(&mut self) {
        *self.ref_count.get_mut() += 1;
    }
    /// R14-16 floor discipline preserved: a put on an already-zero count
    /// (stale put) returns false and never underflows.
    #[inline]
    fn unpin_locked(&mut self) -> bool {
        let c = self.ref_count.get_mut();
        if *c == 0 {
            return false;
        }
        *c -= 1;
        true
    }
    #[inline]
    fn count_locked(&self) -> u32 {
        self.ref_count.load(Ordering::Relaxed)
    }
    /// R14-17 one-shot latch, locked form: true = this caller releases.
    #[inline]
    fn take_released_locked(&mut self) -> bool {
        let r = self.released.get_mut();
        if *r {
            return false;
        }
        *r = true;
        true
    }
}

/// Number of per-CPU reader banks (pcache Option B stage 1). Kernel.toml's
/// SMP ceiling is 4; 8 leaves headroom. A cpu id at or above this bound
/// falls back to the slow-path lock (the r8final discipline — bank
/// selection happens with interrupts off, so the id is stable). Same
/// shape as FdTable's banks (8.10.22).
const PC_READER_BANKS: usize = 8;

/// One per-CPU reader-nesting counter, padded to its own cacheline so
/// the CPUs' counter RMWs never share a line.
#[repr(align(64))]
struct PcReaderBank(core::sync::atomic::AtomicUsize);

/// Fixed-capacity inode-entry slot table (2^15 slots = 256KB of .bss).
/// Load factors in practice are far below 10% (entries live only while
/// their inode has cached pages), so linear-probe chains stay 1-3 slots.
const PC_TABLE_CAP: usize = 32768;
const PC_TABLE_MASK: usize = PC_TABLE_CAP - 1;

/// One open-addressed slot: the Arc<InodePageCache> as a raw pointer.
/// null = never used (probe stop); PC_TOMBSTONE_BITS = entry removed
/// (probe continues — tombstoning instead of nulling keeps the OTHER
/// entries' probe chains intact). Single-word aligned stores, so a
/// lock-free load can never observe a torn value.
struct PcSlot(core::sync::atomic::AtomicPtr<InodePageCache>);

/// Removal sentinel stored in a vacated slot: non-null (probes keep
/// walking), never dereferenced (every deref site checks the bits first).
const PC_TOMBSTONE_BITS: usize = 1;

/// Bound on a lock-free READER probe walk (slots scanned before giving
/// up). Give-up = a miss, which is always safe (re-read + insert dedup);
/// at the load factors this table runs at, real chains are 1-3 slots and
/// the bound is never approached. Writers probe to the first empty slot
/// with no bound — correctness of the dedup depends on it.
const PC_READER_PROBE_MAX: usize = 64;

/// Parked-Arc batch carried out of a wlock section. Eviction may empty
/// entries; their slot references are parked into this fixed array under
/// wlock (Vec push would allocate — never under a pcache lock) and moved
/// into `retired` + drained right after the wlock drops.
const PC_PARK_BATCH: usize = 8;
type PcParkBatch = [Option<Arc<InodePageCache>>; PC_PARK_BATCH];

/// Per-inode page cache: the lock-free lookup unit.
struct InodePageCache {
    /// Identity for the slot-table probe compare (== cache_key). Written
    /// once at construction, never mutated — lock-free readers compare it
    /// inside their claim window.
    key: u64,
    /// Per-inode lock (Option B stage 1): the guard IS the &mut access to
    /// the pages map (every CachedPage field is reached through it — the
    /// leg-54 locked-plain discipline transfers verbatim). Nesting:
    /// `wlock → this` only, never the reverse.
    lock: Spinlock<BTreeMap<u64, CachedPage>>,
    /// Identity of the owning (filesystem, inode) — cache_key is a
    /// non-reversible hash, so reclaim-time writeback (shrink_reclaim)
    /// records the raw inputs here to resolve a live VFS Inode via
    /// icache_lookup and flush dirty entries through the normal
    /// ext4_flush_dirty_inode machinery.
    fs_id: u64,
    ino: u64,
}

impl InodePageCache {
    fn new(key: u64, fs_id: u64, ino: u64) -> Self {
        Self {
            key,
            lock: Spinlock::new(BTreeMap::new()),
            fs_id,
            ino,
        }
    }
}

/// Global page cache, keyed by the combined (fs_id, inode) cache key.
///
/// The inode map is the Option-B slot table; see the module LOCKING note.
pub struct PageCache {
    /// Lock-free inode-entry slot table (readers: bank claim + Acquire
    /// loads; writers: wlock + Release stores).
    slots: [PcSlot; PC_TABLE_CAP],
    /// Per-CPU reader-nesting banks (see `lookup_claimed` for the protocol).
    readers: [PcReaderBank; PC_READER_BANKS],
    /// Writer-side table structure lock: entry publish/remove, eviction,
    /// whole-table iteration. Ordering: wlock → per-inode lock, never
    /// reverse. No allocation under it.
    wlock: Spinlock<()>,
    /// Whole entries removed from the table, parked until a drain can
    /// drop them with no lock-free reader mid-claim (FdTable.retired).
    retired: Spinlock<alloc::vec::Vec<Arc<InodePageCache>>>,
    /// Slow-path exclusion for readers whose cpu id falls outside the
    /// bank range (r8final family); drains hold it across the drop.
    slow_read: Spinlock<()>,
    /// Total number of cached pages (for global limit).
    total_pages: AtomicU32,
}

/// Combine (fs_id, ino) into the single u64 map key. Distinct inputs map to
/// distinct keys for all practical purposes (splitmix-style mixing); the
/// fs_id for each filesystem is a stable instance tag (ext4: instance
/// pointer; rootfs/procfs/devfs: their FS_ID_* constants).
#[inline]
fn cache_key(fs_id: u64, ino: u64) -> u64 {
    fs_id
        .rotate_left(17)
        .wrapping_add(0x9E3779B97F4A7C15)
        ^ ino.wrapping_mul(0xC2B2AE3D27D4EB4F)
}

/// Convert a physical address to a kernel-virtual pointer via the linear mapping.
#[inline]
fn phys_to_virt_ptr(phys: usize) -> *mut u8 {
    phys_to_virt(PhysAddr::new(phys as u64)).0 as *mut u8
}

impl PageCache {
    /// Create a new empty page cache.
    pub const fn new() -> Self {
        Self {
            slots: [const {
                PcSlot(core::sync::atomic::AtomicPtr::new(
                    core::ptr::null_mut(),
                ))
            }; PC_TABLE_CAP],
            readers: [const {
                PcReaderBank(core::sync::atomic::AtomicUsize::new(0))
            }; PC_READER_BANKS],
            wlock: Spinlock::new(()),
            retired: Spinlock::new(alloc::vec::Vec::new()),
            slow_read: Spinlock::new(()),
            total_pages: AtomicU32::new(0),
        }
    }

    // ==================== slot-table primitives ====================

    /// Lock-free global lookup of the inode entry for `key` — the pcache
    /// twin of FdTable::get_file (8.10.22), at fault rate instead of
    /// syscall rate.
    ///
    /// With irqs off for the section, bump this CPU's reader bank (a
    /// SeqCst RMW — aqrl AMO on riscv, locked xadd on x86 — so the claim
    /// is globally visible before the slot loads below execute), probe
    /// the table (Acquire loads — pairs the publisher's Release store so
    /// the entry's fields are visible through the reconstructed Arc),
    /// claim the entry via a strong-count increment, and drop the claim.
    /// The per-inode work happens OUTSIDE the bank window, protected by
    /// the strong count alone.
    ///
    /// SAFETY of the increment (the reason the bank exists): a slot's Arc
    /// is owned by the table while published; any writer that removes it
    /// tombstones the slot, parks the Arc in `retired`, and the drain
    /// drops it only after observing EVERY bank at zero with the
    /// slow-path lock held. A reader can therefore only load a pointer
    /// whose reference is provably alive: either the slot still publishes
    /// it (the table's reference), or the removing writer's drain is
    /// waiting for THIS reader's bank to return to zero before its drop.
    /// The reader's SeqCst claim-before-load and the drain's
    /// SeqCst-fence-then-scan close the store-load ordering hole. The
    /// probe's `(*p).key` compare dereferences the entry for the same
    /// reason it is safe: within the claim window the entry cannot be
    /// freed (its allocation outlives the claim by the drain handshake),
    /// and `key` is write-once before publication.
    ///
    /// A probe that exhausts PC_READER_PROBE_MAX slots or hits the first
    /// empty slot reports a miss. Misses are always safe (the caller
    /// re-fills from disk and `insert` dedups against the live entry);
    /// the bound only bounds irqs-off latency.
    fn lookup_claimed(&self, key: u64) -> Option<Arc<InodePageCache>> {
        let flags = crate::arch::cpu::save_and_disable_irq();
        let cpu = crate::arch::cpu_id() as usize;
        let out = if cpu < PC_READER_BANKS {
            let bank = &self.readers[cpu];
            bank.0.fetch_add(1, Ordering::SeqCst);
            let p = self.probe_slot_ptr(key);
            let out = if !p.is_null() {
                // SAFETY: p was published by a writer from an owned Arc and
                // is kept alive by the slot or by `retired` until a drain
                // observes all banks zero — our bank claim is exactly what
                // that drain waits for.
                unsafe {
                    Arc::increment_strong_count(p);
                    Some(Arc::from_raw(p))
                }
            } else {
                None
            };
            bank.0.fetch_sub(1, Ordering::Release);
            out
        } else {
            // r8final family: cpu id outside the bank range. Serialize
            // with drains through slow_read instead of a bank claim.
            let _slow = self.slow_read.lock_irqsave();
            let p = self.probe_slot_ptr(key);
            if !p.is_null() {
                // SAFETY: drains hold slow_read across the drop of
                // retired Arcs (see drain_retired), so either the slot
                // or a parked reference keeps p alive here.
                unsafe {
                    Arc::increment_strong_count(p);
                    Some(Arc::from_raw(p))
                }
            } else {
                None
            }
        };
        crate::arch::cpu::restore_irq(flags);
        out
    }

    /// Probe the slot table for `key`; returns the raw entry pointer or
    /// null. Plain Acquire loads; LIVENESS is the caller's concern (bank
    /// claim window or slow_read). Stops at the first empty slot; walks
    /// over tombstones; bounded for readers, unbounded for writers.
    fn probe_slot_ptr(&self, key: u64) -> *mut InodePageCache {
        let h = (key as usize) & PC_TABLE_MASK;
        for n in 0..PC_READER_PROBE_MAX {
            let i = h.wrapping_add(n) & PC_TABLE_MASK;
            let p = self.slots[i].0.load(Ordering::Acquire);
            if p.is_null() {
                return core::ptr::null_mut();
            }
            if p as usize == PC_TOMBSTONE_BITS {
                continue;
            }
            // SAFETY: p is a published-or-parked live entry (see the
            // liveness contract of the callers); `key` is write-once.
            if unsafe { (*p).key } == key {
                return p;
            }
        }
        core::ptr::null_mut()
    }

    /// WRITER probe (wlock held): find the slot publishing an entry with
    /// this key. Walks to the first empty slot — the dedup invariant
    /// ("never two live entries with one key") depends on seeing the
    /// whole cluster, so no bound here.
    fn table_find(&self, key: u64) -> Option<usize> {
        let h = (key as usize) & PC_TABLE_MASK;
        for n in 0..PC_TABLE_CAP {
            let i = h.wrapping_add(n) & PC_TABLE_MASK;
            let p = self.slots[i].0.load(Ordering::Acquire);
            if p.is_null() {
                return None;
            }
            if p as usize == PC_TOMBSTONE_BITS {
                continue;
            }
            // SAFETY: wlock excludes removal, so the slot's reference is
            // stable and the entry is alive; `key` is write-once.
            if unsafe { (*p).key } == key {
                return Some(i);
            }
        }
        None
    }

    /// WRITER probe (wlock held): find the slot publishing exactly this
    /// entry object. Pointer identity — the entry may have been removed
    /// and re-created under the same key since the caller claimed it.
    fn table_find_ptr(&self, key: u64, want: *const InodePageCache) -> Option<usize> {
        let h = (key as usize) & PC_TABLE_MASK;
        for n in 0..PC_TABLE_CAP {
            let i = h.wrapping_add(n) & PC_TABLE_MASK;
            let p = self.slots[i].0.load(Ordering::Acquire);
            if p.is_null() {
                return None;
            }
            if p as usize == PC_TOMBSTONE_BITS {
                continue;
            }
            if core::ptr::eq(p, want) {
                return Some(i);
            }
        }
        None
    }

    /// WRITER (wlock held): return the live entry for `key`, publishing
    /// the caller's pre-allocated `fresh` entry on first touch.
    ///
    /// The Arc allocation MUST have happened before the caller took
    /// wlock (R-WRITESTORM-1: alloc's reclaim slowpath reaches shrink →
    /// wlock). Returns None only when the table has no free slot (the
    /// insert degrades to no-cache; PC_TABLE_FULL counts it) or `fresh`
    /// was not provided.
    fn table_publish(
        &self,
        _w: &SpinlockGuard<'_, ()>,
        key: u64,
        fresh: &mut Option<Arc<InodePageCache>>,
    ) -> Option<Arc<InodePageCache>> {
        let h = (key as usize) & PC_TABLE_MASK;
        let mut first_tomb: Option<usize> = None;
        for n in 0..PC_TABLE_CAP {
            let i = h.wrapping_add(n) & PC_TABLE_MASK;
            let p = self.slots[i].0.load(Ordering::Acquire);
            if p.is_null() {
                let entry = fresh.take()?;
                // Publish at the first tombstone seen on the way (keeps
                // probe chains compact) or at this empty slot. The entry
                // is fully initialized BEFORE Arc::into_raw; the Release
                // store publishes its fields to lock-free readers.
                let slot = first_tomb.unwrap_or(i);
                let ptr = Arc::into_raw(entry) as *mut InodePageCache;
                // SAFETY: ptr's reference count is transferred to the
                // slot; we re-constitute a second owning Arc for the
                // caller via an increment + from_raw pair.
                unsafe { Arc::increment_strong_count(ptr) };
                self.slots[slot].0.store(ptr, Ordering::Release);
                return Some(unsafe { Arc::from_raw(ptr) });
            }
            if p as usize == PC_TOMBSTONE_BITS {
                if first_tomb.is_none() {
                    first_tomb = Some(i);
                }
                continue;
            }
            // SAFETY: wlock excludes removal (stable slot reference).
            if unsafe { (*p).key } == key {
                // SAFETY: the slot keeps its reference; ours is a new
                // owning claim established while wlock blocks removal.
                unsafe { Arc::increment_strong_count(p) };
                return Some(unsafe { Arc::from_raw(p) });
            }
        }
        PC_TABLE_FULL.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Remove an EMPTIED entry from the table and park its slot
    /// reference. The emptiness is re-verified under the entry's
    /// per-inode lock WITH wlock held: a concurrent insert may have
    /// re-populated the entry between the claimer's unlock and here —
    /// then the entry stays published (that insert's page would
    /// otherwise be orphaned in a parked entry).
    fn remove_entry(&self, key: u64, claimed: &Arc<InodePageCache>) {
        let w = self.wlock.lock();
        let Some(idx) = self.table_find_ptr(key, Arc::as_ptr(claimed)) else {
            return; // already removed (evict/another put) — our Arc dies with us
        };
        let parked = {
            let g = claimed.lock.lock();
            if !g.is_empty() {
                return; // re-populated — stays published
            }
            self.slots[idx]
                .0
                .store(PC_TOMBSTONE_BITS as *mut InodePageCache, Ordering::Release);
            // SAFETY: the slot's reference is handed over here (the slot
            // no longer owns it); the parking Vec keeps it until the
            // reader-quiescent drain.
            unsafe { Arc::from_raw(Arc::as_ptr(claimed)) }
        };
        drop(w);
        self.park_and_drain(parked);
    }

    /// Park a removed entry and attempt the drain (FdTable.park_and_drain
    /// shape: nothing spins under wlock, so parking happens AFTER it).
    fn park_and_drain(&self, entry: Arc<InodePageCache>) {
        {
            let mut r = self.retired.lock_irqsave();
            r.push(entry);
        }
        self.drain_retired();
    }

    /// Move a wlock section's parked slot references into `retired` and
    /// drain once. Callers run this right AFTER dropping wlock.
    fn flush_parked(&self, parked: &mut PcParkBatch) {
        let mut any = false;
        for p in parked.iter_mut() {
            if let Some(e) = p.take() {
                let mut r = self.retired.lock_irqsave();
                r.push(e);
                any = true;
            }
        }
        if any {
            self.drain_retired();
        }
    }

    /// Drop retired entries once no reader can be mid-claim.
    ///
    /// Quiescence: every bank reads 0 (each scan load Acquire — reading 0
    /// synchronizes with that reader's Release decrement, so its
    /// strong-count increment has completed) AND the slow-path lock is
    /// held across the take (excludes the bank-range fallback readers).
    /// Readers that START after a successful scan cannot load these Arcs:
    /// their slots were tombstoned before the park, the tombstone stores
    /// precede this drain's SeqCst fence, and a fresh reader's slot load
    /// follows its own SeqCst bank claim.
    fn drain_retired(&self) {
        {
            // Cheap emptiness check first: most drains find a previous
            // drain already took the vec.
            let r = self.retired.lock_irqsave();
            if r.is_empty() {
                return;
            }
        }
        let mut scans: usize = 0;
        loop {
            // The fence orders the writer's earlier slot-tombstone stores
            // before the bank reads below (the reader side of the
            // handshake is its SeqCst bank claim before its slot load —
            // see lookup_claimed).
            core::sync::atomic::fence(Ordering::SeqCst);
            let mut all_zero = true;
            for b in self.readers.iter() {
                if b.0.load(Ordering::Acquire) != 0 {
                    all_zero = false;
                    break;
                }
            }
            if all_zero {
                break;
            }
            scans += 1;
            if scans >= 8192 {
                // Defer, never fail: reader sections are irq-off and
                // bounded, so this is practically unreachable; the next
                // writer op retries. Only the removal TIMING can be
                // delayed, never correctness.
                PC_DRAIN_DEFERS.fetch_add(1, Ordering::Relaxed);
                return;
            }
            core::hint::spin_loop();
        }
        let taken = {
            let _slow = self.slow_read.lock_irqsave();
            let mut r = self.retired.lock_irqsave();
            core::mem::take(&mut *r)
        };
        // Drop OUTSIDE all locks: InodePageCache::drop frees BTreeMap
        // nodes (allocator locks only, no pcache locks — same shape as
        // FdTable's retired File drops).
        drop(taken);
    }

    // ==================== fault-side ops (claim + per-inode lock) ====================

    /// Lookup a cached page for ((fs_id, ino), page_index).
    /// On hit: increments ref_count, sets Referenced flag, returns pointer.
    /// On miss: returns None.
    pub fn get(&self, fs_id: u64, ino: u64, page_index: u64) -> Option<*const u8> {
        let key = cache_key(fs_id, ino);
        let entry = self.lookup_claimed(key)?;
        let out = {
            let mut g = entry.lock.lock();
            let page = g.get_mut(&page_index)?;
            if page.invalidated {
                // Stale (a write invalidated it while pinned) — serve a
                // miss so the caller re-reads current data from disk.
                return None;
            }
            STAT_HITS.fetch_add(1, Ordering::Relaxed);
            page.pin_locked();

            // Mark page as recently accessed for LRU rotation
            let page_desc = pfn_to_page_mut(page.pfn);
            if !page_desc.is_null() {
                unsafe {
                    (*page_desc).set_flag(PageFlag::Referenced);
                }
            }

            let phys = pfn_to_phys(page.pfn);
            Some(phys_to_virt_ptr(phys) as *const u8)
        };
        drop(entry);
        out
    }

    /// Same as `get` but returns the PHYSICAL address — for mapping the
    /// cached frame directly into a user page table (MAP_SHARED faults).
    /// The returned pin is owned by that PTE mapping for its lifetime.
    pub fn get_phys(&self, fs_id: u64, ino: u64, page_index: u64) -> Option<u64> {
        let key = cache_key(fs_id, ino);
        let entry = self.lookup_claimed(key)?;
        let out = {
            let mut g = entry.lock.lock();
            let page = g.get_mut(&page_index)?;
            if page.invalidated {
                return None;
            }
            STAT_HITS.fetch_add(1, Ordering::Relaxed);
            page.pin_locked();
            let page_desc = pfn_to_page_mut(page.pfn);
            if !page_desc.is_null() {
                unsafe {
                    (*page_desc).set_flag(PageFlag::Referenced);
                }
            }
            Some(pfn_to_phys(page.pfn) as u64)
        };
        drop(entry);
        out
    }

    /// Pin up to `out.len()` consecutive cached pages starting at
    /// `page_index`, filling `out` with (index, pointer) pairs.
    ///
    /// Stops at the first missing/invalidated page — the caller re-fills and
    /// retries. One claim + one per-inode lock + one BTreeMap range walk
    /// replaces a get()/put() pair PER PAGE: for a 1 MiB sequential read that
    /// is 2 map round trips instead of 512 (the dominant warm-read cost under
    /// TCG). The out-array API avoids a heap allocation per call (a Vec here
    /// measurably regressed the 4 KiB-read hot path). Returns the run
    /// length; 0 when `page_index` itself is not cached.
    ///
    /// CONTIGUITY CONTRACT: out[i] is exactly `page_index + i` for i < n.
    /// The caller (ext4_file_read_cached_dst) consumes the run POSITIONALLY
    /// — it ignores the returned index and serves run[i] as page_index+i.
    /// BTreeMap::range() iterates only EXISTING keys, silently stepping
    /// over missing ones, so a bare range walk violates that contract
    /// whenever a page inside the window is absent:
    ///   - eviction removed page_index itself while its neighbours stayed:
    ///     the run started at page_index+1 and its content was served as
    ///     page_index's; or
    ///   - eviction removed a middle page (or fill_page_cache_batch skipped
    ///     a hole): every later entry shifted one page early — VALID file
    ///     data delivered at the WRONG offset.
    /// That was the AC-2 CKSUM-MISMATCH family (300 MB read-only window
    /// returning wrong bytes with a pristine disk and boot-deterministic
    /// wrong content). The `expected` walk below stops at the first gap so
    /// the caller re-fills from the missing page instead.
    pub fn get_range_into(
        &self,
        fs_id: u64,
        ino: u64,
        page_index: u64,
        out: &mut [(u64, *const u8)],
    ) -> usize {
        let key = cache_key(fs_id, ino);
        if out.is_empty() {
            return 0;
        }
        let count = out.len();
        let Some(entry) = self.lookup_claimed(key) else {
            return 0;
        };
        let n = {
            let mut g = entry.lock.lock();
            let mut expected = page_index;
            let mut n = 0usize;
            for (&idx, page) in g.range_mut(page_index..page_index + count as u64) {
                if idx != expected {
                    break; // gap: page `expected` is not cached — stop here
                }
                if page.invalidated {
                    break;
                }
                STAT_HITS.fetch_add(1, Ordering::Relaxed);
                page.pin_locked();
                let page_desc = pfn_to_page_mut(page.pfn);
                if !page_desc.is_null() {
                    unsafe {
                        (*page_desc).set_flag(PageFlag::Referenced);
                    }
                }
                let phys = pfn_to_phys(page.pfn);
                out[n] = (idx, phys_to_virt_ptr(phys) as *const u8);
                n += 1;
                expected += 1;
            }
            n
        };
        drop(entry);
        n
    }

    /// Release a run of pages pinned by `get_range` (one claim + one
    /// per-inode lock for all).
    pub fn put_range(&self, fs_id: u64, ino: u64, pages: &[(u64, *const u8)]) {
        let key = cache_key(fs_id, ino);
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        let emptied = {
            let mut g = entry.lock.lock();
            for &(page_index, _) in pages {
                if let Some(page) = g.get_mut(&page_index) {
                    // Same release discipline as put(): only a put that
                    // actually decremented may release an invalidated
                    // page's frame.
                    let decremented = page.unpin_locked();
                    if decremented
                        && page.count_locked() == 0
                        && page.invalidated
                        && page.take_released_locked()
                    {
                        let pfn = page.pfn;
                        g.remove(&page_index);
                        let page_desc = pfn_to_page_mut(pfn);
                        if !page_desc.is_null() {
                            unsafe { lru::page_remove_lru(&*page_desc); }
                        }
                        release_page_frame(pfn);
                        self.total_pages.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
            g.is_empty()
        };
        if emptied {
            self.remove_entry(key, &entry);
        }
        drop(entry);
    }

    /// Release a page reference (decrement ref_count).
    pub fn put(&self, fs_id: u64, ino: u64, page_index: u64) {
        let key = cache_key(fs_id, ino);
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        let emptied = {
            let mut g = entry.lock.lock();
            if let Some(page) = g.get_mut(&page_index) {
                // Floor at 0: a get() miss (invalidated page) never
                // incremented, so a stale put must not underflow.
                // R14-16 (the dblfree engine): only a put that ACTUALLY
                // decremented (prev >= 1) may release — the old code
                // released whenever the count was seen 0+invalidated, so
                // two stale puts (or a stale put after the real one) each
                // released the SAME frame: the zone double-free stream
                // (pfn 544551/2/5...) behind the surviving corruption.
                let decremented = page.unpin_locked();
                // R7-C2: invalidate_inode left this page in place because a
                // reader held a pin — now that the pin is gone, free it.
                if decremented
                    && page.count_locked() == 0
                    && page.invalidated
                    && page.take_released_locked()
                {
                    let pfn = page.pfn;
                    g.remove(&page_index);
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(pfn);
                    self.total_pages.fetch_sub(1, Ordering::Relaxed);
                }
            }
            g.is_empty()
        };
        if emptied {
            self.remove_entry(key, &entry);
        }
        drop(entry);
    }

    /// mm-side release for a mapping-owned pin (x86_64 filemap_fault).
    ///
    /// The combined key and page_index come from the page DESCRIPTOR's
    /// mapping/index fields, which insert/insert_batch maintain as the
    /// eviction keys — the unmap/COW/fork paths only have the frame. Same
    /// release discipline as `put()` (R14-16: only a put that actually
    /// decremented may release an invalidated entry's frame).
    pub fn put_by_key(&self, key: u64, page_index: u64) {
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        let emptied = {
            let mut g = entry.lock.lock();
            if let Some(page) = g.get_mut(&page_index) {
                let decremented = page.unpin_locked();
                if decremented
                    && page.count_locked() == 0
                    && page.invalidated
                    && page.take_released_locked()
                {
                    let pfn = page.pfn;
                    g.remove(&page_index);
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(pfn);
                    self.total_pages.fetch_sub(1, Ordering::Relaxed);
                }
            }
            g.is_empty()
        };
        if emptied {
            self.remove_entry(key, &entry);
        }
        drop(entry);
    }

    /// mm-side pin acquisition for a frame the page tables are about to
    /// share (fork's COW walk copying a cache-mapped PTE into the child).
    ///
    /// Private cache mappings own exactly one pin per PTE (taken by
    /// `get_phys` at fault time, released by `put_by_key` at unmap/COW);
    /// the fork walk copies the PTE, so it must copy the pin ownership
    /// too — otherwise the parent's unmap would drop the last pin and
    /// eviction could free a frame the child still maps. Pins are also
    /// taken on `invalidated` entries (the R7-C2/R8-3 release accounting
    /// counts every holder; a refused pin would let a later put release
    /// another mapping's frame). Returns false when the entry is gone
    /// entirely (no pin possible; the frame is no longer cache-owned).
    pub fn pin_by_key(&self, key: u64, page_index: u64) -> bool {
        let Some(entry) = self.lookup_claimed(key) else {
            return false;
        };
        let out = {
            let mut g = entry.lock.lock();
            if let Some(page) = g.get_mut(&page_index) {
                page.pin_locked();
                let page_desc = pfn_to_page_mut(page.pfn);
                if !page_desc.is_null() {
                    unsafe {
                        (*page_desc).set_flag(PageFlag::Referenced);
                    }
                }
                true
            } else {
                false
            }
        };
        drop(entry);
        out
    }

    // ==================== MAP_SHARED writeback (session 25) ====================
    //
    // Writable shared mappings alias the cache frame with W, so their
    // stores land DIRECTLY in the frame (mapper coherence by construction).
    // Persistence is then the cache's problem: the frame is marked dirty
    // at W-map install (conservative v1), write(2) writes THROUGH pinned
    // frames instead of invalidating them, and msync/fsync flush dirty
    // frames back to the file. NO pcache lock (wlock or per-inode) is
    // EVER held across the block I/O of a flush (R-WRITESTORM-1):
    // collection and completion are separate locked passes, the flush
    // itself runs pinned but unlocked.

    /// Mark one cached page dirty (a writable shared mapping was just
    /// installed on it). Conservative: a writable mapping MAY write, and
    /// flushing an unwritten page is harmless I/O. Missing/invalidated
    /// entries are ignored (nothing to persist).
    pub fn mark_dirty(&self, fs_id: u64, ino: u64, page_index: u64) {
        self.mark_dirty_by_key(cache_key(fs_id, ino), page_index);
    }

    /// `mark_dirty` by combined cache key — for callers that only hold the
    /// descriptor's eviction keys (mapping/index).
    pub fn mark_dirty_by_key(&self, key: u64, page_index: u64) {
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        {
            let g = entry.lock.lock();
            if let Some(page) = g.get(&page_index) {
                if !page.invalidated {
                    page.dirty.store(true, Ordering::Release);
                }
            }
        }
        drop(entry);
    }

    /// `mark_dirty` by frame PFN — for PTE-rewrite sites (mprotect
    /// W-upgrades on SHARED cache leaves) that hold the leaf's PPN but not
    /// the file identity. Resolves the cache entry through the page
    /// descriptor's eviction keys (set by insert). No-op for non-cache
    /// frames (mapping() == 0) and foreign descriptors.
    pub fn mark_dirty_pfn(&self, pfn: usize) {
        use crate::mm::page_desc::pfn_to_page_mut;
        let page = pfn_to_page_mut(pfn);
        if page.is_null() {
            return;
        }
        // SAFETY: descriptor of a live frame; mapping/index are written
        // only by the cache's insert/release paths.
        let key = unsafe { (*page).mapping() as u64 };
        if key == 0 {
            return;
        }
        let index = unsafe { (*page).index() as u64 };
        self.mark_dirty_by_key(key, index);
    }

    /// Post-write(2) cache update: copy the written bytes INTO every fresh
    /// cached frame overlapping [file_off, file_off+data.len()) and leave
    /// the entries in place. This replaces the old whole-inode invalidate
    /// on the write path:
    /// - PINNED entries (shared or private mappings alias them) MUST be
    ///   updated, never invalidated — a pinned-but-invalidated entry serves
    ///   misses, so the next fault would re-read from disk into a SECOND
    ///   frame while live mappings keep writing the old one: permanent
    ///   mapper divergence. Updating the frame keeps every alias coherent
    ///   (Linux writes into the page cache the same way).
    /// - Unpinned entries stay cached and clean: the bytes just reached
    ///   the file, so the frame matches disk — cache stays hot instead of
    ///   being dropped (the old behavior re-fetched the whole inode on
    ///   every write).
    /// The dirty bit is deliberately untouched: a page holding UNFLUSHED
    /// shared-mapping stores outside the written range stays dirty (its
    /// data still exists only in the frame); a clean page written by
    /// write(2) is now on disk, hence clean.
    ///
    /// `skip_pfn`: the writeback path itself calls this with the FLUSH
    /// SOURCE frame (a dirty cache frame being written to disk) — copying
    /// it onto itself is a self-overlap (UB via copy_nonoverlapping, and
    /// pointless); passing its PFN skips exactly that entry.
    ///
    /// memcpy only — no I/O, safe under the per-inode lock.
    pub fn writeback_update_range(
        &self,
        fs_id: u64,
        ino: u64,
        file_off: u64,
        data: &[u8],
        skip_pfn: u64,
    ) {
        if data.is_empty() {
            return;
        }
        let key = cache_key(fs_id, ino);
        let first_index = file_off / PAGE_SIZE as u64;
        let last_index = (file_off + data.len() as u64 - 1) / PAGE_SIZE as u64;
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        {
            let g = entry.lock.lock();
            for idx in first_index..=last_index {
                let Some(page) = g.get(&idx) else {
                    continue;
                };
                if page.invalidated {
                    // Stale-pinned entry: serves misses, dies on its last put —
                    // leave it alone (its frame is invisible to new faults).
                    continue;
                }
                if skip_pfn != 0 && page.pfn as u64 == skip_pfn {
                    // The flush source frame itself (see the doc comment) —
                    // copying a frame onto itself is a self-overlap.
                    continue;
                }
                let page_off = (idx * PAGE_SIZE as u64).max(file_off);
                let page_end = ((idx + 1) * PAGE_SIZE as u64).min(file_off + data.len() as u64);
                if page_off >= page_end {
                    continue;
                }
                let src_skip = (page_off - file_off) as usize;
                let dst = phys_to_virt_ptr(pfn_to_phys(page.pfn));
                let n = (page_end - page_off) as usize;
                // SAFETY: dst is the linear map of the entry's frame (PAGE_SIZE
                // writable bytes); page_off%PAGE + n <= PAGE_SIZE by construction.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data.as_ptr().add(src_skip),
                        dst.add((page_off % PAGE_SIZE as u64) as usize),
                        n,
                    );
                }
            }
        }
        drop(entry);
    }

    /// Collect up to `out.len()` dirty pages in [first_index, last_index]
    /// (inclusive), PINNING each (ref++) so its frame cannot be freed
    /// while the caller flushes it OUTSIDE all pcache locks. Fills `out`
    /// with (page_index, pfn) pairs; returns the count collected. Callers
    /// MUST balance every collected entry with `flush_done`.
    ///
    /// PROGRESS RULE (session 26): a collected-and-flushed page whose
    /// mapping is still up STAYS dirty (flush_done clears the bit only at
    /// pin count zero), so a caller looping "collect until empty" would
    /// re-collect the same leading pages forever — the first msync of any
    /// file with >out.len() live-mapped dirty pages hung in an unbounded
    /// write loop. Callers therefore resume from the LAST COLLECTED index:
    /// pass `first_index = last_collected + 1` on the next call within one
    /// flush operation (the entries collected but not yet flushed keep
    /// their pins, and are skipped naturally by the advanced range).
    pub fn collect_dirty_range(
        &self,
        fs_id: u64,
        ino: u64,
        first_index: u64,
        last_index: u64,
        out: &mut [(u64, u64)],
    ) -> usize {
        let key = cache_key(fs_id, ino);
        if out.is_empty() {
            return 0;
        }
        let Some(entry) = self.lookup_claimed(key) else {
            return 0;
        };
        let n = {
            let mut g = entry.lock.lock();
            let mut n = 0usize;
            for (&idx, page) in g.range_mut(first_index..=last_index) {
                if n == out.len() {
                    break;
                }
                if page.invalidated || !page.dirty.load(Ordering::Acquire) {
                    continue;
                }
                page.pin_locked();
                out[n] = (idx, page.pfn as u64);
                n += 1;
            }
            n
        };
        drop(entry);
        n
    }

    /// Complete a flush cycle for entries collected by `collect_dirty_range`:
    /// drop the flush pin and clear the dirty bit ONLY when the count reached
    /// zero AND the flush's write verifiably reached the file (`ok`) — no live
    /// mapping remains that could store into the frame again (every mapping
    /// holds a pin). Concurrent mapper stores racing the flush are
    /// last-writer-wins (POSIX accepted for unsynchronized msync); while a
    /// mapping is up the bit stays set and the next sync rewrites the
    /// page — the conservative v1 substitute for hardware dirty tracking.
    /// An entry invalidated meanwhile (truncate/unlink raced the flush) has
    /// its data discarded instead.
    ///
    /// `entries`: (page_index, pfn, write_ok) — a FAILED or short write
    /// keeps the page dirty for the next sync even at pin count 0: the
    /// frame may be the only surviving copy (mapping already unmapped),
    /// and marking it clean would make it evictable and its data silently
    /// losable.
    pub fn flush_done(&self, fs_id: u64, ino: u64, entries: &[(u64, u64, bool)]) {
        if entries.is_empty() {
            return;
        }
        let key = cache_key(fs_id, ino);
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        let emptied = {
            let mut g = entry.lock.lock();
            for &(page_index, _pfn, write_ok) in entries {
                if let Some(page) = g.get_mut(&page_index) {
                    // Clear dirty ONLY when the flush put brought the pin
                    // count to zero — i.e. NO live mapping remains (every
                    // PCACHE mapping holds a pin) — AND the write succeeded.
                    // While any mapping is up, the page MAY receive new stores
                    // the flush did not see; keeping the dirty bit makes the
                    // next sync rewrite it (the conservative v1 substitute for
                    // hardware dirty tracking). The same rule keeps the
                    // entry dirty after the LAST unmap of an unsynced page:
                    // bounded, unevictable until the next sync on the file,
                    // and coherent for every reader through the frame (POSIX
                    // deferred-writeback latitude).
                    let mut clear_dirty = false;
                    let decremented = page.unpin_locked();
                    if decremented {
                        clear_dirty =
                            page.count_locked() == 0 && !page.invalidated && write_ok;
                    }
                    if clear_dirty {
                        page.dirty.store(false, Ordering::Release);
                    }
                    if decremented
                        && page.count_locked() == 0
                        && page.invalidated
                        && page.take_released_locked()
                    {
                        let pfn = page.pfn;
                        g.remove(&page_index);
                        let page_desc = pfn_to_page_mut(pfn);
                        if !page_desc.is_null() {
                            unsafe { lru::page_remove_lru(&*page_desc); }
                        }
                        release_page_frame(pfn);
                        self.total_pages.fetch_sub(1, Ordering::Relaxed);
                    }
                }
            }
            // An inode whose last entry was an invalidated frame released
            // here must not leave a permanent empty InodePageCache behind.
            g.is_empty()
        };
        if emptied {
            self.remove_entry(key, &entry);
        }
        drop(entry);
    }

    /// Invalidate all cached pages for a given inode.
    /// Called after writes or truncates to prevent stale data.
    ///
    /// R7-C2: pages with an active reader pin (ref_count > 0) are marked
    /// `invalidated` instead of being freed under the reader's feet —
    /// the final put() releases them, and get() serves a miss meanwhile
    /// so no stale data is ever returned.
    ///
    /// The per-pass doom list is a FIXED array: no allocation may run
    /// under the per-inode lock (R-WRITESTORM-1 — the reclaim slowpath
    /// could reach evict_one → the same entry's per-inode lock). A full
    /// pass loops until an under-full pass proves everything is done;
    /// every full pass frees 64 pages, so this terminates.
    pub fn invalidate_inode(&self, fs_id: u64, ino: u64) {
        let key = cache_key(fs_id, ino);
        let Some(entry) = self.lookup_claimed(key) else {
            return;
        };
        const PASS_MAX: usize = 64;
        loop {
            let mut doomed: [(u64, usize); PASS_MAX] = [(0, 0); PASS_MAX];
            let mut n = 0usize;
            let emptied = {
                let mut g = entry.lock.lock();
                for (&idx, page) in g.iter_mut() {
                    if n == PASS_MAX {
                        break;
                    }
                    if page.ref_count.load(Ordering::Acquire) == 0 {
                        page.invalidated = true; // marker; removed below
                        doomed[n] = (idx, page.pfn);
                        n += 1;
                    } else {
                        page.invalidated = true; // pinned: dies on its final put
                    }
                }
                // Remove the doomed entries while the lock still covers
                // the map; the frames are freed right here (release does
                // not allocate — it ran under the old global lock too).
                for &(idx, pfn) in &doomed[..n] {
                    g.remove(&idx);
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(pfn);
                }
                if n > 0 {
                    self.total_pages.fetch_sub(n as u32, Ordering::Relaxed);
                }
                g.is_empty()
            };
            if emptied {
                self.remove_entry(key, &entry);
            }
            if n < PASS_MAX {
                break;
            }
            // Full pass — more unreferenced pages may remain. Loop.
        }
        drop(entry);
    }

    // ==================== writer-side ops (wlock) ====================

    /// Insert a newly-read page into the cache.
    /// If the page already exists, just leaves it alone (its pin is the
    /// caller's next get).
    pub fn insert(&self, fs_id: u64, ino: u64, page_index: u64, _block_nr: u64, data: &[u8]) {
        let key = cache_key(fs_id, ino);

        // R-WRITESTORM-1: allocate the frame BEFORE taking any pcache
        // lock. alloc_page(GFP_KERNEL) can enter synchronous direct
        // reclaim (page_alloc.rs slowpath), which reaches
        // vmscan::try_to_free_pages -> PageCache::shrink -> wlock —
        // allocating under the lock self-deadlocks the CPU on its own
        // spinlock. Under the OH /data write storm (memory pressure makes
        // the zone slowpath routine) that wedged BOTH CPUs spinning on
        // PAGE_CACHE while the ext4 big-lock holder sat inside insert()
        // (oh b120/b121 deadlock watchdog: lock=...page_cache::PAGE_CACHE,
        // preceded by EXT4-BIGLOCK-STALL).
        let phys_addr = alloc_page(GfpFlags::GFP_KERNEL);
        if phys_addr == 0 {
            return;
        }

        STAT_MISSES.fetch_add(1, Ordering::Relaxed);

        // The first-touch Arc allocation ALSO stays outside wlock for the
        // same reason. Pre-allocated speculatively; dropped unused when
        // the entry already exists.
        let mut fresh = Some(Arc::new(InodePageCache::new(key, fs_id, ino)));
        let mut parked: PcParkBatch = [const { None }; PC_PARK_BATCH];

        {
            let w = self.wlock.lock();

            // Evict if needed (with progress check to prevent infinite loop
            // when all cached pages have ref_count > 0). The frame is already
            // in hand, so exceeding the soft cap by a page here is harmless —
            // the next insert evicts. The parked batch bounds the loop too
            // (emptied entries park there; they are drained right after the
            // wlock drops — the overshoot is the old soft-cap latitude).
            while self.total_pages.load(Ordering::Relaxed) as usize >= max_cached_pages() {
                if !self.evict_one_locked(&w, &mut parked) {
                    break; // Cannot evict — all pages in use
                }
            }

            let Some(entry) = self.table_publish(&w, key, &mut fresh) else {
                // Table full (or no fresh entry): degrade to no-cache —
                // the page is simply not cached; correctness unaffected.
                drop(w);
                self.flush_parked(&mut parked);
                free_page(phys_addr);
                return;
            };

            // Dedup verdict + install in ONE per-inode hold. A duplicate
            // never returns from inside the guards — the frame return is
            // scheduled here and executed after all locks drop.
            let mut duplicate = false;
            {
                let mut g = entry.lock.lock();

                // If already cached and fresh, nothing to do. (R7-C2:
                // insert used to bump ref_count with no matching put — a
                // permanent pin leak that defeated eviction and pinned
                // stale data forever.) R8-3: an invalidated-but-still-
                // pinned entry — the final put() owns the old frame; the
                // previous code freed the new frame HERE regardless of
                // ref_count, handing the pinned reader a freed physical
                // page (exactly what invalidate_inode promises not to
                // do). Skip caching this read; the old entry dies via
                // its refcount. The pre-allocated frame is unused on
                // both paths — returned below.
                if g.contains_key(&page_index) {
                    duplicate = true;
                } else {
                    let pfn = phys_addr / PAGE_SIZE;

                    // Mark page descriptor and add to LRU
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe {
                            (*page_desc).set_page_type(PageType::PageCache);
                            (*page_desc).set_flag(PageFlag::UpToDate);
                            // Store reverse-lookup info for eviction (the combined key)
                            (*page_desc).set_mapping(key as usize as *mut core::ffi::c_void);
                            (*page_desc).set_index(page_index as usize);
                        }
                        // Add to LRU_INACTIVE_FILE at the YOUNG end — must happen after
                        // setting flags. (Two-tier policy: young at head, reclaim from
                        // tail; linking at the tail made the eviction scan see the
                        // freshest page first.)
                        lru::page_add_file_lru_young(unsafe { &*page_desc });
                    }

                    // Copy data into the physical page via linear mapping
                    unsafe {
                        let dst = phys_to_virt_ptr(phys_addr);
                        let copy_len = core::cmp::min(data.len(), PAGE_SIZE);
                        core::ptr::copy_nonoverlapping(data.as_ptr(), dst, copy_len);
                        if copy_len < PAGE_SIZE {
                            core::ptr::write_bytes(dst.add(copy_len), 0, PAGE_SIZE - copy_len);
                        }
                    }

                    g.insert(page_index, CachedPage {
                        pfn,
                        ref_count: AtomicU32::new(0),
                        invalidated: false,
                        released: core::sync::atomic::AtomicBool::new(false),
                        dirty: core::sync::atomic::AtomicBool::new(false),
                    });
                    self.total_pages.fetch_add(1, Ordering::Relaxed);
                }
            }
            drop(w);
            self.flush_parked(&mut parked);
            drop(entry);
            if duplicate {
                free_page(phys_addr);
            }
        }
    }

    /// Bulk-insert a run of consecutive pages whose data lives in ONE
    /// contiguous buffer (`buf`, `block_size` bytes per page).
    ///
    /// The read-ahead path used to call `insert` per 4 KiB page: each call
    /// took the global lock, ran the eviction check, did a BTreeMap
    /// lookup+insert, allocated one order-0 frame, and copied 4 KiB —
    /// ~90µs/page under TCG, which was ~4x the cost of the underlying disk
    /// I/O (measured: 181ms insert vs 12ms device wait for an 8 MiB fill).
    /// This path amortizes all of it:
    /// - ONE wlock + ONE per-inode lock acquisition and ONE eviction pass
    ///   for the whole run;
    /// - power-of-two buddy allocations (up to 64 pages) instead of per-page
    ///   order-0 allocations;
    /// - ONE memcpy per buddy chunk instead of per page;
    /// - direct per-page inserts into the pre-resolved entry. Duplicate
    ///   frames never leak: a key already cached (or invalidated-but-pinned,
    ///   R8-3) is skipped and its freshly popped frame released right away.
    pub fn insert_batch(&self, fs_id: u64, ino: u64, first_index: u64, block_size: usize, buf: &[u8]) {
        if block_size == 0 || buf.len() < block_size {
            return;
        }
        let npages = buf.len() / block_size;
        if npages <= 1 || block_size != PAGE_SIZE {
            // Single page, or a block size that does not fill a frame (the
            // packed contiguous copy below would pack blocks without their
            // per-page zero padding) — per-page inserts keep that layout.
            for k in 0..npages {
                let end = core::cmp::min((k + 1) * block_size, buf.len());
                self.insert(fs_id, ino, first_index + k as u64, 0, &buf[k * block_size..end]);
            }
            return;
        }
        let key = cache_key(fs_id, ino);

        // R-WRITESTORM-1 (the insert twin): allocate EVERY frame BEFORE
        // taking any pcache lock. The old loop called
        // alloc_page_batch(GFP_KERNEL) under the lock; under memory
        // pressure that enters the page-allocator slowpath →
        // vmscan::try_to_free_pages → PageCache::shrink → wlock — the
        // SAME lock the caller already holds: a spinlock self-deadlock
        // with one CPU spinning forever (the OH b120/b121 wedge insert()
        // was fixed for, resurrected here at RAM-scale cache pressure —
        // every insert batch wedged a CPU, SA .so loads never completed).
        // Chunked 64 frames at a time: each chunk allocates unlocked, then
        // one locked section does eviction headroom + inserts for the
        // chunk.
        const ALLOC_CHUNK: usize = 64;
        let mut phys_buf: [usize; ALLOC_CHUNK] = [0; ALLOC_CHUNK];

        let mut done: usize = 0;
        while done < npages {
            let want = core::cmp::min(ALLOC_CHUNK, npages - done);
            let got = alloc_page_batch(GfpFlags::GFP_KERNEL, want, &mut phys_buf[..want]);
            if got == 0 {
                // Out of memory entirely: stop the batch here; the caller's
                // per-page fallback covers the tail.
                break;
            }

            // Fill and describe every frame BEFORE any pcache lock (perf
            // review 2026-10-31): the chunk's memcpy used to run under
            // wlock + the per-inode lock — up to 64x4 KiB of copying with
            // both serialization points held, stalling every reader and
            // every other inserter for the duration. The frames are still
            // private here (allocated above, not yet published), so the
            // copy and the descriptor stamping need no lock. A frame whose
            // index turns out to be a duplicate at insert time pays one
            // wasted copy (the rare dedup path) and is freed exactly as
            // the old in-lock order did.
            for k in 0..got {
                let phys = phys_buf[k];
                let pfn = phys / PAGE_SIZE;
                let page_index = first_index + done as u64 + k as u64;
                let src_off = (done + k) * block_size;
                let copy_len = core::cmp::min(PAGE_SIZE, buf.len() - src_off);
                // SAFETY: phys is a valid page frame from the zone
                // allocator; the linear mapping covers all RAM.
                unsafe {
                    let dst = phys_to_virt_ptr(phys);
                    core::ptr::copy_nonoverlapping(buf.as_ptr().add(src_off), dst, copy_len);
                }
                let page_desc = pfn_to_page_mut(pfn);
                if !page_desc.is_null() {
                    // SAFETY: pfn came from the zone allocator; its page
                    // descriptor is valid for the frame's lifetime.
                    unsafe {
                        (*page_desc).set_page_type(PageType::PageCache);
                        (*page_desc).set_flag(PageFlag::UpToDate);
                        (*page_desc).set_mapping(key as usize as *mut core::ffi::c_void);
                        (*page_desc).set_index(page_index as usize);
                    }
                }
            }

            // The first-touch Arc allocation stays outside wlock (same
            // R-WRITESTORM-1 rule as insert); dropped unused when the
            // entry already exists.
            let mut fresh = Some(Arc::new(InodePageCache::new(key, fs_id, ino)));
            let mut parked: PcParkBatch = [const { None }; PC_PARK_BATCH];
            let mut appended: u32 = 0;

            {
                let w = self.wlock.lock();

                // Eviction headroom for this chunk (soft cap — bounded
                // loop, the batch may proceed slightly over if everything
                // is pinned or the park batch fills).
                let mut evict_tries = 0u32;
                while self.total_pages.load(Ordering::Relaxed) as usize + got > max_cached_pages() {
                    if !self.evict_one_locked(&w, &mut parked) {
                        evict_tries += 1;
                        if evict_tries >= 32 {
                            break;
                        }
                    }
                    if parked.iter().all(|p| p.is_some()) {
                        break;
                    }
                }

                STAT_MISSES.fetch_add(1, Ordering::Relaxed);

                let Some(entry) = self.table_publish(&w, key, &mut fresh) else {
                    // Table full: drop this chunk's frames, stop the batch.
                    drop(w);
                    for &phys in &phys_buf[..got] {
                        free_page(phys);
                    }
                    self.flush_parked(&mut parked);
                    break;
                };

                // Copy per page (order-0 frames are not contiguous), describe,
                // link to the LRU in one locked batch, and insert into the
                // inode map directly. Order-0 deliberately: eviction frees
                // page-cache frames one page at a time, higher-order buddy
                // chunks failed expensively (reclaim + compaction slowpaths).
                let mut lru_batch: [*mut Page; ALLOC_CHUNK] =
                    [core::ptr::null_mut(); ALLOC_CHUNK];
                let mut lru_n: usize = 0;
                {
                    let mut g = entry.lock.lock();
                    for k in 0..got {
                        let phys = phys_buf[k];
                        let pfn = phys / PAGE_SIZE;
                        let page_index = first_index + done as u64 + k as u64;
                        // Duplicate-key discipline of insert(): an existing
                        // fresh entry is left alone (no frame clobber/leak);
                        // an invalidated-but-pinned one (R8-3) is skipped
                        // too. The frame was popped from the zone — release
                        // it right away.
                        if g.contains_key(&page_index) {
                            // Duplicate index: the frame was already filled
                            // and described in the unlocked stage above —
                            // release it exactly as the old in-lock order
                            // did (no clobber, no leak).
                            free_page(phys);
                            continue;
                        }
                        let page_desc = pfn_to_page_mut(pfn);
                        if !page_desc.is_null() {
                            lru_batch[lru_n] = page_desc;
                            lru_n += 1;
                        }
                        g.insert(
                            page_index,
                            CachedPage {
                                pfn,
                                ref_count: AtomicU32::new(0),
                                invalidated: false,
                                released: core::sync::atomic::AtomicBool::new(false),
                                dirty: core::sync::atomic::AtomicBool::new(false),
                            },
                        );
                        appended += 1;
                    }
                    if lru_n > 0 {
                        lru::page_add_file_lru_young_batch(&lru_batch[..lru_n]);
                    }
                    self.total_pages.fetch_add(appended, Ordering::Relaxed);
                }
            }
            self.flush_parked(&mut parked);
            done += got;
        }
    }

    /// Shrink the page cache by evicting up to `nr_to_scan` unreferenced pages.
    ///
    /// Called by the page reclaim engine (kswapd / direct reclaim) when zone
    /// free pages drop below watermarks.  Only pages with ref_count == 0 are
    /// evicted — pages actively being read are left alone.
    ///
    /// Returns the number of pages actually freed.
    pub fn shrink(&self, nr_to_scan: usize) -> usize {
        let mut freed = 0usize;
        while freed < nr_to_scan {
            if self.total_pages.load(Ordering::Relaxed) == 0 {
                break;
            }
            let mut parked: PcParkBatch = [const { None }; PC_PARK_BATCH];
            let evicted = {
                let w = self.wlock.lock();
                self.evict_one_locked(&w, &mut parked)
            };
            self.flush_parked(&mut parked);
            if !evicted {
                break;
            }
            freed += 1;
        }
        freed
    }

    /// Reclaim-time dirty writeback (the session-24 "v2" item). Direct
    /// reclaim ONLY: the plain shrink() evicts clean pages alone — a
    /// DIRTY entry (unflushed MAP_SHARED stores of a mapping that has
    /// since unmapped without msync) is skipped by evict_one by design,
    /// because evicting it would lose the only copy of that data. When
    /// nothing clean is left, such pages are a permanent, unevictable
    /// drain (the abandonment shape of the fake-OOM family). This
    /// variant makes progress where shrink() cannot: flush the dirty
    /// entries to their file first, then let the clean pass evict them.
    ///
    /// Safety shape (each point verified against the callers):
    /// - Runs from vmscan direct reclaim (page_alloc slowpath), which is
    ///   task context, holds no spinlocks, and is globally serialized by
    ///   IN_DIRECT_RECLAIM — never from the mmu_init PT-alloc retry,
    ///   which holds the per-mm PteLock (sleeping on I/O there is the
    ///   deadlock the session-19 comment documents; that caller keeps
    ///   plain shrink()).
    /// - NO pcache lock is EVER held across the flush I/O
    ///   (R-WRITESTORM-1): candidates are collected under wlock (+ the
    ///   per-inode locks), the locks are dropped, then
    ///   ext4_flush_dirty_inode does its own chunked collect → write →
    ///   complete cycle.
    /// - The filesystem is reached through a LIVE VFS Inode resolved by
    ///   icache_lookup (fs_id of an ext4 Inode IS the fs pointer, ext4
    ///   mod.rs inode setup) — the same liveness contract sys_fsync
    ///   uses. An inode no longer in icache is skipped: its dirty pages
    ///   stay as they were (bounded, coherent to every reader through
    ///   the frame); the next open() re-caches the inode and a later
    ///   retry can flush them.
    /// - WRITEBACK_GUARD stops re-entry: ext4's write path allocates
    ///   memory, and a nested reclaim arriving inside our own flush
    ///   must not start another flush (it degrades to clean-only).
    /// - A page whose mapping re-pinned during the flush stays dirty
    ///   (flush_done keeps the bit while pinned) and is simply picked
    ///   again by a later round — the bounded round count below keeps
    ///   one call from spinning on it.
    pub fn shrink_reclaim(&self, nr_to_scan: usize) -> usize {
        /// Max inode ranges flushed per round, and rounds per call. One
        /// shrink_reclaim thus performs at most 2*16 chunked flushes of
        /// I/O before returning — reclaim stays responsive while always
        /// making progress (each flushed-and-now-clean page is evicted
        /// by the follow-up clean pass).
        const MAX_WB_INODES: usize = 2;
        const MAX_WB_ROUNDS: usize = 4;
        /// Dirty-page window per flush: matches ext4_flush_dirty_inode's
        /// internal CHUNK (one collect/write/complete cycle).
        const WB_WINDOW: u64 = 16;

        let mut freed = 0usize;
        for _round in 0..MAX_WB_ROUNDS {
            let remaining = nr_to_scan.saturating_sub(freed);
            if remaining == 0 {
                break;
            }
            freed += self.shrink(remaining);

            if freed >= nr_to_scan {
                break;
            }
            // Shortfall: reclaim demand exceeds what clean eviction
            // supplied. Flush dirty, unreferenced entries NOW — while
            // clean pages may still exist elsewhere — rather than at
            // the exhausted wall: the flush's own ext4/bio allocations
            // (journal buffers, block mapping) must be able to allocate
            // while it runs, or every write fails and the pages stay
            // dirty forever (the Linux dirty-threshold insight: writeback
            // starts far below the memory wall).
            let mut work: [(u64, u64, u64, u64); MAX_WB_INODES] =
                [(0, 0, 0, 0); MAX_WB_INODES];
            let mut n_work = 0usize;
            {
                let _w = self.wlock.lock();
                'outer: for si in 0..PC_TABLE_CAP {
                    let p = self.slots[si].0.load(Ordering::Acquire);
                    if p.is_null() || p as usize == PC_TOMBSTONE_BITS {
                        continue;
                    }
                    // SAFETY: wlock excludes removal, so the slot's
                    // reference is stable and the entry is alive.
                    let entry = unsafe { &*p };
                    let g = entry.lock.lock();
                    let mut first: Option<u64> = None;
                    let mut last: u64 = 0;
                    for (&idx, page) in g.iter() {
                        if page.invalidated
                            || page.count_locked() != 0
                            || !page.dirty.load(Ordering::Acquire)
                        {
                            continue;
                        }
                        if first.is_none() {
                            first = Some(idx);
                        }
                        last = idx;
                        if idx - first.unwrap_or(idx) + 1 >= WB_WINDOW {
                            break;
                        }
                    }
                    if let Some(first) = first {
                        work[n_work] = (entry.fs_id, entry.ino, first, last);
                        n_work += 1;
                        if n_work == MAX_WB_INODES {
                            break 'outer;
                        }
                    }
                }
            }
            if n_work == 0 {
                break; // no flushable dirty pages anywhere
            }
            // Re-entrancy gate: nested reclaim (from inside ext4 write's
            // own allocations) degrades to clean-only.
            if WRITEBACK_GUARD
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                break;
            }
            for i in 0..n_work {
                let (fs_id, ino, first, last) = work[i];
                // Liveness: same contract as sys_fsync (Inode Arc keeps
                // the descriptor alive; private_data fs pointer is valid
                // while the mount lives). Miss = skip, retry next call.
                if let Some(inode) = crate::fs::inode::icache_lookup(ino, fs_id) {
                    crate::fs::ext4::file::ext4_flush_dirty_inode(
                        &inode,
                        first,
                        last.min(first + WB_WINDOW - 1),
                    );
                }
            }
            WRITEBACK_GUARD.store(false, Ordering::Release);
            // Loop: the clean pass at the top of the next round evicts
            // what the flush cleaned.
        }
        freed
    }

    /// Evict one unreferenced page, two-tier (Linux file-LRU shape).
    ///
    /// 1. Walk LRU_INACTIVE_FILE from the TAIL — the oldest end, walking
    ///    `lru_prev` toward younger pages. Pinned pages (ref_count > 0)
    ///    are skipped. Referenced pages get the flag cleared and are
    ///    activated onto the young end of LRU_ACTIVE_FILE. The first
    ///    clean unreferenced page is evicted.
    /// 2. If the inactive walk found nothing, demote the OLDEST active
    ///    pages back onto the young end of inactive (second chance) and
    ///    re-walk once. Without demotion every page that was ever served
    ///    accumulated on the active list forever (`lru_deactivate` had no
    ///    callers), eviction could not make progress, and inserts fell
    ///    through to the map-order fallback — semantically RANDOM
    ///    eviction, the OH multi-loader thrash amplifier.
    /// 3. Safety net: slot-table scan (hash-key order) for pages the LRU
    ///    walk cannot reach (descriptor-less frames, list corruption).
    ///
    /// History: the pre-two-tier walk started at the tail and followed
    /// `lru_next` — the terminator direction — so it examined exactly ONE
    /// page per call: the newest (fresh pages linked at the tail back
    /// then), i.e. "evict the page most likely to be needed next".
    ///
    /// `wlock` MUST be held by the caller (the guard parameter is the
    /// proof — it nests per-inode locks of VICTIMS inside it; the caller's
    /// own target entry, if any, is locked only AFTER this returns).
    /// Entries emptied by the eviction have their slot tombstoned and the
    /// slot's Arc parked into `parked` (the caller flushes after dropping
    /// wlock); the function refuses to evict when `parked` is full.
    ///
    /// Returns true when a page was actually evicted.
    fn evict_one_locked(
        &self,
        _w: &SpinlockGuard<'_, ()>,
        parked: &mut PcParkBatch,
    ) -> bool {
        const MAX_SCAN: usize = 64; // bound scan to limit latency
        const DEMOTE_BATCH: usize = 32;

        if parked.iter().all(|p| p.is_some()) {
            return false; // caller must flush first
        }

        for _round in 0..2 {
            let mut pfn = lru::lru_tail(LRU_INACTIVE_FILE);
            let mut scanned = 0usize;

            while pfn != 0 && scanned < MAX_SCAN {
                scanned += 1;
                let page_desc = pfn_to_page_mut(pfn);
                if page_desc.is_null() {
                    break;
                }

                unsafe {
                    let page = &*page_desc;
                    // Tail = oldest end; lru_prev walks toward the head
                    // (younger). Capture BEFORE any move of this page.
                    let next_pfn = page.lru_prev();

                    // Only evict PageCache pages
                    if page.page_type() != PageType::PageCache {
                        pfn = next_pfn;
                        continue;
                    }

                    // Check ref_count — pages being read are not evictable.
                    // Use mapping (inode key) and index (page_index) for the
                    // lookup. Verdict AND removal share ONE per-inode hold:
                    // dropping the lock between check and remove would let a
                    // concurrent get() pin the page and the eviction free a
                    // pinned frame.
                    let key = page.mapping() as u64;
                    let page_index = page.index() as u64;

                    enum Victim {
                        Skip,
                        SecondChance,
                        Evict,
                    }

                    let verdict = match self.table_find(key) {
                        Some(i) => {
                            let ptr = self.slots[i].0.load(Ordering::Acquire);
                            // SAFETY: wlock excludes removal; the entry is alive
                            // (this deref rides the enclosing LRU-walk unsafe block).
                            let entry = &*ptr;
                            let mut g = entry.lock.lock();
                            let verdict = match g.get(&page_index) {
                                Some(cached) => {
                                    // Session 25: dirty frames hold unflushed
                                    // shared-mapping stores — evicting one
                                    // would lose the only copy of that data.
                                    let evictable = cached.count_locked() == 0
                                        && !cached.dirty.load(Ordering::Acquire);
                                    if !evictable {
                                        Victim::Skip
                                    } else {
                                        let pg_desc = pfn_to_page_mut(cached.pfn);
                                        let referenced = if !pg_desc.is_null() {
                                            (*pg_desc).test_flag(PageFlag::Referenced)
                                        } else {
                                            false
                                        };
                                        if referenced {
                                            Victim::SecondChance
                                        } else {
                                            g.remove(&page_index);
                                            if g.is_empty() {
                                                // Whole entry gone: tombstone the
                                                // slot and park its reference
                                                // (drained by the caller after
                                                // wlock drops — never spin under
                                                // wlock). `parked` had a free
                                                // slot (checked up front).
                                                self.slots[i].0.store(
                                                    PC_TOMBSTONE_BITS as *mut InodePageCache,
                                                    Ordering::Release,
                                                );
                                                if let Some(slot) =
                                                    parked.iter_mut().find(|p| p.is_none())
                                                {
                                                    // SAFETY: the slot's reference
                                                    // is handed over here (see
                                                    // remove_entry; rides the
                                                    // enclosing unsafe block).
                                                    *slot = Some(Arc::from_raw(ptr));
                                                }
                                            }
                                            Victim::Evict
                                        }
                                    }
                                }
                                // Page not in the map — stale descriptor, clean up
                                None => Victim::Evict,
                            };
                            verdict
                        }
                        // Entry gone entirely — stale descriptor, clean up
                        None => Victim::Evict,
                    };

                    match verdict {
                        Victim::Skip => {
                            pfn = next_pfn;
                            continue;
                        }
                        Victim::SecondChance => {
                            // Referenced: clear the flag, promote to the young
                            // end of the active list, keep scanning.
                            page.clear_flag(PageFlag::Referenced);
                            lru::lru_activate_young(page);
                            pfn = next_pfn;
                            continue;
                        }
                        Victim::Evict => {
                            lru::page_remove_lru(page);
                            release_page_frame(pfn);
                            self.total_pages.fetch_sub(1, Ordering::Relaxed);
                            STAT_EVICTIONS.fetch_add(1, Ordering::Relaxed);
                            return true;
                        }
                    }
                }
            }

            // Inactive walk made no progress — demote the oldest ACTIVE
            // pages to the young end of inactive and retry once. An empty
            // active list (or nothing demotable) falls to the map scan.
            let demoted = lru::lru_demote_active_file_oldest(DEMOTE_BATCH);
            if demoted == 0 {
                break;
            }
            STAT_DEMOTIONS.fetch_add(demoted as u64, Ordering::Relaxed);
        }

        // LRU walk exhausted — fall back to slot-table scan (safety net)
        for si in 0..PC_TABLE_CAP {
            let ptr = self.slots[si].0.load(Ordering::Acquire);
            if ptr.is_null() || ptr as usize == PC_TOMBSTONE_BITS {
                continue;
            }
            // SAFETY: wlock excludes removal; the entry is alive.
            let entry = unsafe { &*ptr };
            let mut g = entry.lock.lock();
            // Same candidate rule as the LRU walk: skip pinned and dirty,
            // give Referenced pages their second chance (activated, scan
            // continues), evict the first clean unreferenced page.
            let mut victim: Option<(u64, usize)> = None;
            for (&page_idx, page) in g.iter() {
                if page.count_locked() != 0 || page.dirty.load(Ordering::Acquire) {
                    continue;
                }
                let page_desc = pfn_to_page_mut(page.pfn);
                if !page_desc.is_null() {
                    unsafe {
                        let p = &*page_desc;
                        if p.test_flag(PageFlag::Referenced) {
                            p.clear_flag(PageFlag::Referenced);
                            lru::lru_activate_young(p);
                            continue; // second chance — keep scanning
                        }
                    }
                }
                victim = Some((page_idx, page.pfn));
                break;
            }
            if let Some((page_idx, pfn)) = victim {
                g.remove(&page_idx);
                if g.is_empty() {
                    self.slots[si]
                        .0
                        .store(PC_TOMBSTONE_BITS as *mut InodePageCache, Ordering::Release);
                    if let Some(slot) = parked.iter_mut().find(|p| p.is_none()) {
                        // SAFETY: the slot's reference is handed over here.
                        *slot = Some(unsafe { Arc::from_raw(ptr) });
                    }
                }
                drop(g);
                let page_desc = pfn_to_page_mut(pfn);
                if !page_desc.is_null() {
                    unsafe { lru::page_remove_lru(&*page_desc); }
                }
                release_page_frame(pfn);
                self.total_pages.fetch_sub(1, Ordering::Relaxed);
                STAT_EVICTIONS.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }
}

/// Release a physical page frame back to the zone allocator.
fn release_page_frame(pfn: usize) {
    let phys_addr = pfn_to_phys(pfn);

    // Clear page cache metadata
    let page_desc = pfn_to_page_mut(pfn);
    if !page_desc.is_null() {
        unsafe {
            let page = &*page_desc;
            page.set_page_type(PageType::Normal);
            page.clear_flag(PageFlag::UpToDate);
            page.set_mapping(core::ptr::null_mut());
            page.set_index(0);
        }
    }

    free_page(phys_addr);
}

/// Global page cache instance.
static PAGE_CACHE: PageCache = PageCache::new();

/// Get a reference to the global page cache.
pub fn get_page_cache() -> &'static PageCache {
    &PAGE_CACHE
}

/// Get the total number of cached pages (for /proc/meminfo).
pub fn page_cache_total_pages() -> u32 {
    PAGE_CACHE.total_pages.load(Ordering::Relaxed)
}

/// Page-cache op counters (hits, misses, evictions, active demotions) for
/// the /proc/meminfo `PageCacheOps:` Rux-extension line. Under the OH
/// service mixture the hit/miss ratio is the direct read on whether the
/// cache still thrashes (miss ≈ every fault pays the ext4 big lock).
pub fn page_cache_stats() -> (u64, u64, u64, u64) {
    (
        STAT_HITS.load(Ordering::Relaxed),
        STAT_MISSES.load(Ordering::Relaxed),
        STAT_EVICTIONS.load(Ordering::Relaxed),
        STAT_DEMOTIONS.load(Ordering::Relaxed),
    )
}

/// Effective page-cache cap (dynamic: half of managed RAM, min 32MB).
/// Exposed for the meminfo observability line.
pub fn page_cache_cap_pages() -> usize {
    max_cached_pages()
}

/// Resolve a MAP_SHARED file-mapping fault to the page-cache frame for
/// (inode, file_offset/PAGE), populating the cache from the file on miss.
///
/// Returns the pinned physical address that the caller maps into the user
/// page table. Aliasing the cache frame (instead of a private copy) is
/// what makes the mapping genuinely shared: every mapper of the file —
/// across fork AND across a fresh mmap after exec — faults onto the same
/// physical page and observes the others' stores (LTP's IPC results file;
/// the execl01..execveat02 family used to TBROK "Test haven't reported
/// results" when each mapper got a private copy).
///
/// The `get_phys` pin is owned by the user PTE mapping for its lifetime;
/// there is deliberately no put() at unmap time, so a shared-mapped page
/// stays unevictable until its inode cache is dropped. Bounded in practice
/// by the number of simultaneously shared-mapped files (LTP: one 4KB page
/// per test). Known deviation from Linux, documented here: write(2) on the
/// file invalidates the inode cache, after which the mapped frame and new
/// disk reads can diverge — Linux keeps them coherent by writing through
/// the page cache; ext4 here writes disk-direct. Files used as shared
/// mappings (the POSIX shm-over-mmap pattern) are written via the mapping
/// itself, which stays coherent.
pub fn shared_file_fault_page(file: &crate::fs::file::File, file_offset: u64) -> Option<u64> {
    // SAFETY: inode is written once at open time and read-only afterwards.
    let inode = unsafe { &*file.inode.get() }.as_ref()?;
    let (fs_id, ino) = (inode.fs_id, inode.ino as u64);
    let cache = get_page_cache();
    let page_index = file_offset / PAGE_SIZE as u64;

    if let Some(phys) = cache.get_phys(fs_id, ino, page_index) {
        return Some(phys);
    }

    // Miss: read the page from the file into a bounce frame, insert it
    // into the cache, then take the pinning lookup path.
    let bounce_phys = alloc_page(GfpFlags::GFP_KERNEL);
    if bounce_phys == 0 {
        return None;
    }
    // SAFETY: bounce_phys is a freshly allocated page frame owned here.
    let bounce = unsafe {
        core::ptr::write_bytes(phys_to_virt(PhysAddr::new(bounce_phys as u64)).0 as *mut u8, 0,
                               PAGE_SIZE);
        phys_to_virt(PhysAddr::new(bounce_phys as u64)).0 as *mut u8
    };
    // SAFETY: read_at validates its own file references; bounce is a full
    // page of writable kernel memory.
    let got = unsafe { file.read_at(file_offset, bounce, PAGE_SIZE) };
    if got < 0 {
        free_page(bounce_phys);
        return None;
    }
    // SAFETY: bounce holds PAGE_SIZE readable bytes (zero-filled tail).
    unsafe {
        cache.insert(fs_id, ino, page_index, 0,
                     core::slice::from_raw_parts(bounce, PAGE_SIZE));
    }
    free_page(bounce_phys);
    cache.get_phys(fs_id, ino, page_index)
}
