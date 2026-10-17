//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Page Descriptor
//!
//! Maintains metadata for each physical page frame, including:
//! - Reference count (_refcount)
//! - Page flags (flags)
//! - Map count (_mapcount)
//! - Other metadata
//!
//! # Safety Invariants
//!
//! The following invariants must hold at all times for every `Page` descriptor:
//!
//! - **INV-REF-1**: `_refcount` must never be negative.
//!   `put_page()` restores the old value on underflow and warns.
//!
//! - **INV-REF-2**: `_refcount == 0` ⟺ page is on a buddy free list (or otherwise free).
//!
//! - **INV-REF-3**: `_refcount > 0` ⟺ page is in use by at least one owner.
//!
//! - **INV-REF-4**: `_mapcount == -1` (PAGE_MAPCOUNT_BIAS) ⟺ page is not mapped
//!   by any page table entry.
//!
//! - **INV-REF-5**: `_mapcount > -1` ⟺ page is mapped in `(_mapcount + 1)` page tables.
//!
//! - **INV-REF-6**: If `Cow` flag is set ⟹ `_refcount >= 2` AND the PTE `W` bit is clear.
//!   This is the COW pre-condition: the page is shared read-only among multiple mappings.
//!

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::page::{PhysAddr, PhysFrame, PhysFrameNr, VirtAddr, PAGE_SIZE};

/// Page flags
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    /// Page is locked, not accessible
    Locked = 1 << 0,
    /// Page is being written back
    Writeback = 1 << 1,
    /// Page has been accessed (for LRU)
    Referenced = 1 << 2,
    /// Page data is valid (read from disk)
    UpToDate = 1 << 3,
    /// Page has been modified (needs writeback)
    Dirty = 1 << 4,
    /// Page is in LRU list
    Lru = 1 << 5,
    /// Head page of compound page
    Head = 1 << 6,
    /// Page has waiters
    Waiters = 1 << 7,
    /// Page is in active LRU list
    Active = 1 << 8,
    /// Reserved page (kernel use, not swappable)
    Reserved = 1 << 9,
    /// Page has private data (stored in private field)
    Private = 1 << 10,
    /// Page will be reclaimed
    Reclaim = 1 << 11,
    /// Page is backed by swap space
    SwapBacked = 1 << 12,
    /// Page is unevictable
    Unevictable = 1 << 13,
    /// Copy-on-write page (Rux extension)
    Cow = 1 << 14,
    /// Anonymous page (Rux extension)
    Anonymous = 1 << 15,
    /// Page is currently a free-list LEADER in its zone (R15-6: the
    /// authoritative linked-state bit — next_free is NOT reliable, it
    /// survives both power-on-0 and historical-leader residue).
    OnFreelist = 1 << 16,
    /// The page has more distinct (mm, vaddr) mappings than the fixed rmap
    /// slot array can record; reverse mapping falls back to slot 0 plus a
    /// full task scan (review 4.10).
    RmapOverflow = 1 << 17,
    /// Page is in the middle of a compaction migration (review 4.16): its
    /// PTEs hold migration-entry markers and the fault path must wait for
    /// the migration to finish instead of installing a zero page.
    Migrating = 1 << 21,
}

/// Flag-bit window [18:20] storing the LRU list index (0..NR_LRU_LISTS)
/// of a page carrying `PageFlag::Lru`. Storing the index on the page makes
/// LRU unlink O(1) with the doubly-linked lru_next/lru_prev pair (review
/// 4.11) — no list walk to find which of the 5 lists owns the page.
pub const LRU_LIST_SHIFT: u32 = 18;
pub const LRU_LIST_MASK: u32 = 0x7 << LRU_LIST_SHIFT;

/// Page flags collection
#[derive(Debug, Default)]
pub struct PageFlags(AtomicU32);

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

/// Page type
///
/// Used for special page identification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageType {
    /// Normal page
    Normal = 0,
    /// Buddy system free page
    Buddy = 1,
    /// Slab allocator page
    Slab = 2,
    /// Page cache page
    PageCache = 3,
    /// Anonymous page
    Anonymous = 4,
}

/// Page descriptor
///
/// Each physical page frame corresponds to a Page structure, used to track page usage.
///
/// Memory layout (128 bytes, 64-byte aligned):
/// - flags: 4 bytes (atomic flags)
/// - _mapcount: 4 bytes (map count, -1 means unmapped)
/// - _refcount: 4 bytes (reference count)
/// - private: 8 bytes (private data)
/// - mapping: 8 bytes (rmap slot 0: mm ptr / address_space)
/// - index: 8 bytes (rmap slot 0: virtual page number)
/// - rmap_alt: 48 bytes (rmap slots 1..3: (mm ptr, vpn) pairs)
/// - _type: 4 bytes (page type)
/// - next_free: 8 bytes (free list pointer, for allocator)
/// - lru_next: 8 bytes (LRU next PFN)
/// - lru_prev: 8 bytes (LRU prev PFN — doubly-linked LRU, O(1) unlink)
///
#[repr(C, align(64))]
pub struct Page {
    /// Atomic flags
    flags: PageFlags,

    /// Map count: how many page table entries directly reference this page
    /// -1 means unmapped, 0 means mapped once, etc.
    /// Note: initial value is -1 (PAGE_MAPCOUNT_BIAS)
    _mapcount: AtomicI32,

    /// Reference count: number of references to this page
    /// 0 means free, > 0 means in use
    ///
    /// NOTE (compound convention, review 4.4): unlike Linux, Rux keeps a
    /// per-member refcount on every page of a high-order allocation — the
    /// arch teardown path put_page()s each member individually via its
    /// PTE.  Do not free an order>0 block while members are still mapped.
    _refcount: AtomicI32,

    /// Private data
    /// - Buddy system: stores order
    /// - Slab: stores slab management structure
    /// - File system: stores buffer_head
    private: AtomicUsize,

    /// rmap slot 0: associated address_space / owning mm pointer
    /// This field is rmap-only; LRU uses the dedicated lru_next/lru_prev.
    mapping: AtomicUsize,

    /// rmap slot 0: virtual page number in the owning mm
    /// This field is rmap-only; LRU uses the dedicated lru_next/lru_prev.
    index: AtomicUsize,

    /// rmap slots 1..3: (mm pointer, virtual page number) pairs for
    /// additional mappings (MAP_FIXED / shared anon).  `rmap_alt[2*k]` is
    /// the mm pointer, `rmap_alt[2*k+1]` the virtual page number.  All
    /// zero = unused slot.  When a 4th distinct mapping arrives the slots
    /// are flushed and the `RmapOverflow` flag is set — reverse-map
    /// lookups then fall back to the slot-0 + full-task-scan path.
    rmap_alt: [AtomicUsize; 6],

    /// Page type (for special pages)
    _type: AtomicU32,

    /// Free list pointer (for allocator internal use)
    next_free: AtomicUsize,

    /// LRU next pointer (PFN of next page in LRU list, 0 = end of list)
    /// Used for doubly-linked LRU lists; separate from mapping/index (rmap).
    lru_next: AtomicUsize,

    /// LRU prev pointer (PFN of previous page in LRU list, 0 = head/end)
    /// Companion to lru_next — makes unlink O(1) instead of a full list
    /// walk per reclaim (review 4.11).
    lru_prev: AtomicUsize,
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

/// Lifecycle event ring for STAMPED frames (gnome-sigill hunt): one entry
/// per interesting event on a stamped frame, so the report printed when a
/// foreign free fires can replay the frame's whole history — who filled
/// it, which forks took references, and every put that dropped one.
/// Site chars: 'F' fault-fill, 'K' fork get, 'T' teardown put,
/// 'U' unmap put, 'C' cow put, 'R' refused(leak), 'S' stale-stamp freed.
pub struct TextFrameEvent {
    pub pfn: AtomicU64,
    pub pid: AtomicU32,
    pub site: AtomicU64,
    pub refc: AtomicI32,
}
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
        // R15-5 (探针误报根因): init_free 漏重置 next_free。描述符内存
        // 上电为 0，未进过空闲链表的页 next_free=0 而非 FREE_LIST_NULL
        // ——TDF 探针(next_free!=MAX)把每个首次释放都误报成双重释放
        // (idle 193/nettest 483 次全是误报)。补上重置，判据恢复可靠。
        self.next_free.store(usize::MAX, Ordering::Release);
        self.flags.clear(PageFlag::OnFreelist);
    }

    // ========== Flag operations ==========

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
        self._refcount.fetch_add(1, Ordering::AcqRel) + 1
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
                crate::dfx::driftwatch::PUT_UNDERFLOW
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
    /// (dfx::driftwatch) so the MEMLEDGER census can attribute drift to the
    /// release path that dropped it. Same return contract as `put_page`.
    #[inline]
    pub fn put_page_site(&self, site: usize) -> i32 {
        let r = self.put_page();
        if r == 0 {
            crate::dfx::driftwatch::note_zero_put(site);
        }
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
    let src = phys_to_virt(PhysAddr(super::zone::pfn_to_phys(src_pfn) as u64)).0 as usize as *const u8;
    let dst = phys_to_virt(PhysAddr(super::zone::pfn_to_phys(dst_pfn) as u64)).0 as usize as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
}

// ========== Global page array (mem_map) ==========

/// Physical memory constants
// Physical memory base: riscv64 QEMU virt RAM starts at 0x80000000; x86_64
// RAM starts at 0 (the PFN formulas below are base-relative by design).
#[cfg(feature = "riscv64")]
pub const PHYS_MEMORY_BASE: usize = 0x8000_0000;
#[cfg(feature = "x86_64")]
pub const PHYS_MEMORY_BASE: usize = 0x0000_0000;

// Use physical memory size from config (Kernel.toml: memory.physical_memory)
//
// NOMINAL/default size only. The actual per-boot size comes from the
// hardware memory map (riscv64 FDT / x86_64 e820) and is published as the
// runtime descriptor count via set_nr_descs() — see nr_descs(). The config
// value remains the pre-vmemmap fallback.
pub const PHYS_MEMORY_SIZE: usize = crate::config::PHYS_MEMORY_SIZE;

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
    use crate::mm::PAGE_SIZE;
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
    super::memblock::memblock().reserved().iter().for_each(|region| {
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

/// Page descriptor statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct PageDescStats {
    /// Total page count
    pub total_pages: usize,
    /// Free page count (refcount == 0)
    pub free_pages: usize,
    /// In-use page count (refcount > 0)
    pub used_pages: usize,
    /// Reserved page count (Reserved flag)
    pub reserved_pages: usize,
    /// Mapped page count (mapcount > PAGE_MAPCOUNT_BIAS)
    pub mapped_pages: usize,
    /// Dirty page count (Dirty flag)
    pub dirty_pages: usize,
    /// COW page count (Cow flag)
    pub cow_pages: usize,
    /// Anonymous page count (Anonymous flag)
    pub anonymous_pages: usize,
}

/// Get page descriptor statistics
pub fn page_desc_stats() -> PageDescStats {
    // F12: iterate only the pages the RUNNING machine actually has.
    // MAX_PAGES is the compile-time 2 GB worst case; on a 1 GB QEMU the
    // vmemmap pages past the real RAM are unmapped, and the dfx periodic
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
    // vmemmap, and dereferencing them panics the periodic DFX dump
    // (observed: KERNPANIC in page_desc_stats 8s into a -m 1G boot with
    // dfx=periodic,taskdump). The vmemmap-mapped count is the direct
    // dereference-safety bound; F12's phys_memory_size estimate is kept
    // for total_pages above.
    let live_pages = live_pages.min(crate::mm::vmemmap::vmemmap_stats().nr_pages);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_page_flags() {
        let flags = PageFlags::new();

        assert!(!flags.test(PageFlag::Locked));
        assert!(!flags.test(PageFlag::Dirty));

        flags.set(PageFlag::Locked);
        assert!(flags.test(PageFlag::Locked));

        flags.set(PageFlag::Dirty);
        assert!(flags.test(PageFlag::Dirty));

        flags.clear(PageFlag::Locked);
        assert!(!flags.test(PageFlag::Locked));
        assert!(flags.test(PageFlag::Dirty));
    }

    #[test_case]
    fn test_page_refcount() {
        let page = Page::new();

        assert_eq!(page.refcount(), 0);

        page.get_page();
        assert_eq!(page.refcount(), 1);

        page.get_page();
        assert_eq!(page.refcount(), 2);

        page.put_page();
        assert_eq!(page.refcount(), 1);

        page.put_page();
        assert_eq!(page.refcount(), 0);
    }

    #[test_case]
    fn test_page_mapcount() {
        let page = Page::new();

        // Initial map count is -1 (unmapped)
        assert_eq!(page.mapcount(), -1);
        assert!(!page.is_mapped());

        page.add_mapcount();
        assert_eq!(page.mapcount(), 0);
        assert!(page.is_mapped());

        page.add_mapcount();
        assert_eq!(page.mapcount(), 1);

        page.sub_mapcount();
        assert_eq!(page.mapcount(), 0);
    }
}
