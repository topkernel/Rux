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

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::mm::page_alloc::{alloc_page, alloc_page_batch, free_page};
use crate::mm::zone::GfpFlags;
use crate::mm::page_desc::{PageFlag, PageType, pfn_to_page_mut, Page};
use crate::mm::lru;
use crate::mm::pglist::{first_online_node_mut, LRU_INACTIVE_FILE, LRU_ACTIVE_FILE};
use crate::mm::{pfn_to_phys, PAGE_SIZE};
use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

/// Maximum cached pages across all inodes (8192 x 4KB = 32MB).
///
/// Was 512 pages (2MB): smaller than one Ubuntu binary's text+data working
/// set, so every exec/read cycle re-fetched from disk and a 10MB sequential
/// read thrashed the LRU 5x over. 32MB still leaves the 2GB guest plenty of
/// user memory and eviction/reclaim paths unchanged.
const MAX_CACHED_PAGES: usize = 8192;

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
}

/// Per-inode page cache.
struct InodePageCache {
    /// page_index → cached page.
    pages: BTreeMap<u64, CachedPage>,
}

/// Global page cache, keyed by the combined (fs_id, inode) cache key.
pub struct PageCache {
    /// Per-inode caches.
    inodes: Spinlock<BTreeMap<u64, InodePageCache>>,
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
            inodes: Spinlock::new(BTreeMap::new()),
            total_pages: AtomicU32::new(0),
        }
    }

    /// Lookup a cached page for ((fs_id, ino), page_index).
    /// On hit: increments ref_count, sets Referenced flag, returns pointer.
    /// On miss: returns None.
    pub fn get(&self, fs_id: u64, ino: u64, page_index: u64) -> Option<*const u8> {
        let key = cache_key(fs_id, ino);
        let cache = self.inodes.lock();
        let inode_cache = cache.get(&key)?;
        let page = inode_cache.pages.get(&page_index)?;
        if page.invalidated {
            // Stale (a write invalidated it while pinned) — serve a miss so
            // the caller re-reads current data from disk.
            return None;
        }
        page.ref_count.fetch_add(1, Ordering::AcqRel);

        // Mark page as recently accessed for LRU rotation
        let page_desc = pfn_to_page_mut(page.pfn);
        if !page_desc.is_null() {
            unsafe {
                (*page_desc).set_flag(PageFlag::Referenced);
            }
        }

        let phys = pfn_to_phys(page.pfn);
        Some(phys_to_virt_ptr(phys) as *const u8)
    }

    /// Insert a newly-read page into the cache.
    /// If the page already exists, just increments ref_count.
    pub fn insert(&self, fs_id: u64, ino: u64, page_index: u64, _block_nr: u64, data: &[u8]) {
        let key = cache_key(fs_id, ino);
        let mut cache = self.inodes.lock();

        // Evict if needed (with progress check to prevent infinite loop
        // when all cached pages have ref_count > 0).
        while self.total_pages.load(Ordering::Relaxed) as usize >= MAX_CACHED_PAGES {
            let before = self.total_pages.load(Ordering::Relaxed);
            Self::evict_one(&mut cache, &self.total_pages);
            let after = self.total_pages.load(Ordering::Relaxed);
            if after >= before {
                break; // Cannot evict — all pages in use
            }
        }

        let inode_cache = cache.entry(key).or_insert_with(|| InodePageCache {
            pages: BTreeMap::new(),
        });

        // If already cached and fresh, nothing to do. (R7-C2: this used to
        // bump ref_count with no matching put — a permanent pin leak that
        // defeated eviction and pinned stale data forever.)
        if let Some(page) = inode_cache.pages.get(&page_index) {
            if !page.invalidated {
                return;
            }
            // R8-3: invalidated-but-still-pinned — the final put() owns the
            // old frame. The previous code freed it HERE regardless of
            // ref_count, handing the pinned reader a freed physical page
            // (exactly what invalidate_inode promises not to do). Skip
            // caching this read; the old entry dies via its refcount.
            return;
        }

        // Allocate a physical page frame from zone allocator
        let phys_addr = alloc_page(GfpFlags::GFP_KERNEL);
        if phys_addr == 0 {
            return;
        }

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
            // Add to LRU_INACTIVE_FILE — must happen after setting flags
            lru::page_add_file_lru(unsafe { &*page_desc });
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

        inode_cache.pages.insert(page_index, CachedPage {
            pfn,
            ref_count: AtomicU32::new(0),
            invalidated: false,
            released: core::sync::atomic::AtomicBool::new(false),
        });
        self.total_pages.fetch_add(1, Ordering::Relaxed);
    }

    /// Bulk-insert a run of consecutive pages whose data lives in ONE
    /// contiguous buffer (`buf`, `block_size` bytes per page).
    ///
    /// The read-ahead path used to call `insert` per 4 KiB page: each call
    /// took the global inodes lock, ran the eviction check, did a BTreeMap
    /// lookup+insert, allocated one order-0 frame, and copied 4 KiB —
    /// ~90µs/page under TCG, which was ~4x the cost of the underlying disk
    /// I/O (measured: 181ms insert vs 12ms device wait for an 8 MiB fill).
    /// This path amortizes all of it:
    /// - ONE lock acquisition and ONE eviction pass for the whole run;
    /// - power-of-two buddy allocations (up to 64 pages) instead of per-page
    ///   order-0 allocations;
    /// - ONE memcpy per buddy chunk instead of per page;
    /// - a BTreeMap bulk `append` (O(log n) merge) instead of per-page
    ///   lookups. `append` keeps existing keys — pages already cached (or
    /// invalidated-but-pinned, R8-3) are left exactly as `insert` leaves
    /// them; the leftovers are dropped.
    /// Duplicate frames never leak: an appended entry always owns a fresh
    /// frame; dup keys are counted from the append leftovers.
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
        let mut cache = self.inodes.lock();

        // Eviction headroom for the whole run (soft cap — bounded loop, the
        // batch may proceed slightly over if everything is pinned).
        let mut evict_tries = 0u32;
        while self.total_pages.load(Ordering::Relaxed) as usize + npages > MAX_CACHED_PAGES {
            let before = self.total_pages.load(Ordering::Relaxed);
            Self::evict_one(&mut cache, &self.total_pages);
            let after = self.total_pages.load(Ordering::Relaxed);
            if after >= before {
                evict_tries += 1;
                if evict_tries >= 32 {
                    break;
                }
            }
        }

        let inode_cache = cache.entry(key).or_insert_with(|| InodePageCache {
            pages: BTreeMap::new(),
        });

        // Allocate frames in order-0 batches (one zone lock per 64 pages),
        // copy per page (order-0 frames are not contiguous), describe, link
        // to the LRU in one locked batch, and insert into the inode map
        // directly. Order-0 deliberately: eviction frees page-cache frames
        // one page at a time, higher-order buddy chunks failed expensively
        // (reclaim + compaction slowpaths), and a temp-map `append` proved
        // 3x slower than direct inserts under the debug build.
        const ALLOC_CHUNK: usize = 64;
        let mut phys_buf: [usize; ALLOC_CHUNK] = [0; ALLOC_CHUNK];
        let mut lru_batch: [*mut Page; ALLOC_CHUNK] = [core::ptr::null_mut(); ALLOC_CHUNK];
        let mut lru_n: usize = 0;
        let mut appended: u32 = 0;
        let mut done: usize = 0;
        while done < npages {
            let want = core::cmp::min(ALLOC_CHUNK, npages - done);
            let got = alloc_page_batch(GfpFlags::GFP_KERNEL, want, &mut phys_buf[..want]);
            if got == 0 {
                // Out of memory entirely: stop the batch here; the caller's
                // per-page fallback covers the tail.
                break;
            }
            for k in 0..got {
                let phys = phys_buf[k];
                let pfn = phys / PAGE_SIZE;
                let page_index = first_index + done as u64 + k as u64;
                // Duplicate-key discipline of insert(): an existing fresh
                // entry is left alone (no frame clobber/leak); an
                // invalidated-but-pinned one (R8-3) is skipped too. The
                // frame was popped from the zone — release it right away.
                if inode_cache.pages.contains_key(&page_index) {
                    free_page(phys);
                    continue;
                }
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
                    lru_batch[lru_n] = page_desc;
                    lru_n += 1;
                }
                inode_cache.pages.insert(
                    page_index,
                    CachedPage {
                        pfn,
                        ref_count: AtomicU32::new(0),
                        invalidated: false,
                        released: core::sync::atomic::AtomicBool::new(false),
                    },
                );
                appended += 1;
            }
            done += got;
            if lru_n > 0 {
                lru::page_add_file_lru_batch(&lru_batch[..lru_n]);
                lru_n = 0;
            }
        }

        self.total_pages.fetch_add(appended, Ordering::Relaxed);
    }

    /// Pin up to `out.len()` consecutive cached pages starting at
    /// `page_index`, filling `out` with (index, pointer) pairs.
    ///
    /// Stops at the first missing/invalidated page — the caller re-fills and
    /// retries. One lock acquisition + one BTreeMap range walk replaces a
    /// get()/put() pair PER PAGE: for a 1 MiB sequential read that is 2 map
    /// round trips instead of 512 (the dominant warm-read cost under TCG).
    /// The out-array API avoids a heap allocation per call (a Vec here
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
        let mut n = 0usize;
        let cache = self.inodes.lock();
        let Some(inode_cache) = cache.get(&key) else {
            return 0;
        };
        let mut expected = page_index;
        for (&idx, page) in inode_cache.pages.range(page_index..page_index + count as u64) {
            if idx != expected {
                break; // gap: page `expected` is not cached — stop here
            }
            if page.invalidated {
                break;
            }
            page.ref_count.fetch_add(1, Ordering::AcqRel);
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
    }

    /// Release a run of pages pinned by `get_range` (one lock for all).
    pub fn put_range(&self, fs_id: u64, ino: u64, pages: &[(u64, *const u8)]) {
        let key = cache_key(fs_id, ino);
        let mut cache = self.inodes.lock();
        let Some(inode_cache) = cache.get_mut(&key) else {
            return;
        };
        for &(page_index, _) in pages {
            if let Some(page) = inode_cache.pages.get_mut(&page_index) {
                // Same release discipline as put(): only a put that actually
                // decremented may release an invalidated page's frame.
                let decremented = page
                    .ref_count
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                        v.checked_sub(1)
                    })
                    .is_ok();
                if decremented
                    && page.ref_count.load(Ordering::Acquire) == 0
                    && page.invalidated
                    && !page.released.swap(true, Ordering::AcqRel)
                {
                    let pfn = page.pfn;
                    inode_cache.pages.remove(&page_index);
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(pfn);
                    self.total_pages.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Release a page reference (decrement ref_count).
    pub fn put(&self, fs_id: u64, ino: u64, page_index: u64) {
        let key = cache_key(fs_id, ino);
        let mut cache = self.inodes.lock();
        if let Some(inode_cache) = cache.get_mut(&key) {
            if let Some(page) = inode_cache.pages.get_mut(&page_index) {
                // Floor at 0: a get() miss (invalidated page) never
                // incremented, so a stale put must not underflow.
                // R14-16 (the dblfree engine): only a put that ACTUALLY
                // decremented (prev >= 1) may release — the old code
                // released whenever the count was seen 0+invalidated, so
                // two stale puts (or a stale put after the real one) each
                // released the SAME frame: the zone double-free stream
                // (pfn 544551/2/5...) behind the surviving corruption.
                let decremented = page
                    .ref_count
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                        v.checked_sub(1)
                    })
                    .is_ok();
                // R7-C2: invalidate_inode left this page in place because a
                // reader held a pin — now that the pin is gone, free it.
                if decremented
                    && page.ref_count.load(Ordering::Acquire) == 0
                    && page.invalidated
                    && !page.released.swap(true, Ordering::AcqRel)
                {
                    let pfn = page.pfn;
                    inode_cache.pages.remove(&page_index);
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(pfn);
                    self.total_pages.fetch_sub(1, Ordering::Relaxed);
                }
            }
            if inode_cache.pages.is_empty() {
                cache.remove(&key);
            }
        }
    }

    /// Invalidate all cached pages for a given inode.
    /// Called after writes or truncates to prevent stale data.
    ///
    /// R7-C2: pages with an active reader pin (ref_count > 0) are marked
    /// `invalidated` instead of being freed under the reader's feet —
    /// the final put() releases them, and get() serves a miss meanwhile
    /// so no stale data is ever returned.
    pub fn invalidate_inode(&self, fs_id: u64, ino: u64) {
        let key = cache_key(fs_id, ino);
        let mut cache = self.inodes.lock();
        if let Some(inode_cache) = cache.get_mut(&key) {
            let mut freed: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
            for (_, page) in inode_cache.pages.iter_mut() {
                if page.ref_count.load(Ordering::Acquire) == 0 {
                    freed.push(page.pfn);
                    page.invalidated = true; // marker; removed below
                } else {
                    page.invalidated = true;
                }
            }
            if !freed.is_empty() {
                let keys: alloc::vec::Vec<u64> = inode_cache
                    .pages
                    .iter()
                    .filter(|(_, p)| p.ref_count.load(Ordering::Acquire) == 0)
                    .map(|(k, _)| *k)
                    .collect();
                for k in keys {
                    inode_cache.pages.remove(&k);
                }
                let freed_count = freed.len() as u32;
                for pfn in freed {
                    let page_desc = pfn_to_page_mut(pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(pfn);
                }
                // (entries were removed from the map above; the released
                // latch lives on the moved-out entries)
                self.total_pages.fetch_sub(freed_count, Ordering::Relaxed);
            }
            if inode_cache.pages.is_empty() {
                cache.remove(&key);
            }
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
            let before = self.total_pages.load(Ordering::Relaxed);
            {
                let mut cache = self.inodes.lock();
                Self::evict_one(&mut cache, &self.total_pages);
            }
            let after = self.total_pages.load(Ordering::Relaxed);
            if after >= before {
                break;
            }
            freed += 1;
        }
        freed
    }

    /// Evict one page with ref_count == 0 from LRU_INACTIVE_FILE.
    ///
    /// Walks the LRU list from the tail (least recently used end) looking
    /// for a PageCache page with ref_count == 0 and no Referenced flag.
    /// Referenced pages are moved to the active list and skipped.
    fn evict_one(
        cache: &mut BTreeMap<u64, InodePageCache>,
        total_pages: &AtomicU32,
    ) {
        // Walk LRU_INACTIVE_FILE looking for an evictable page cache page
        let mut pfn = lru::lru_tail(LRU_INACTIVE_FILE);
        let mut scanned = 0usize;
        let max_scan = 64; // bound scan to limit latency

        while pfn != 0 && scanned < max_scan {
            scanned += 1;
            let page_desc = pfn_to_page_mut(pfn);
            if page_desc.is_null() {
                break;
            }

            unsafe {
                let page = &*page_desc;
                let next_pfn = page.lru_next();

                // Only evict PageCache pages
                if page.page_type() != PageType::PageCache {
                    pfn = next_pfn;
                    continue;
                }

                // Check ref_count — pages being read are not evictable.
                // We need to find the CachedPage in the BTreeMap to check.
                // Use mapping (inode) and index (page_index) for lookup.
                let ino = page.mapping() as u64;
                let page_index = page.index() as u64;

                let evictable = if let Some(inode_cache) = cache.get(&ino) {
                    if let Some(cached) = inode_cache.pages.get(&page_index) {
                        cached.ref_count.load(Ordering::Acquire) == 0
                    } else {
                        // Page not in BTreeMap — stale, should clean up
                        true
                    }
                } else {
                    true
                };

                if !evictable {
                    pfn = next_pfn;
                    continue;
                }

                // Check referenced flag — give recently accessed pages another chance
                if page.test_flag(PageFlag::Referenced) {
                    page.clear_flag(PageFlag::Referenced);
                    lru::lru_activate(page);
                    pfn = next_pfn;
                    continue;
                }

                // Evict: remove from BTreeMap, LRU, and free page
                if let Some(inode_cache) = cache.get_mut(&ino) {
                    inode_cache.pages.remove(&page_index);
                }
                lru::page_remove_lru(page);
                release_page_frame(pfn);
                total_pages.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        }

        // LRU walk exhausted — fall back to BTreeMap scan (safety net)
        for (_ino, inode_cache) in cache.iter_mut() {
            let mut to_remove = None;
            for (&page_idx, page) in inode_cache.pages.iter() {
                if page.ref_count.load(Ordering::Acquire) == 0 {
                    // Also check Referenced flag
                    let page_desc = pfn_to_page_mut(page.pfn);
                    let skip = if !page_desc.is_null() {
                        unsafe {
                            let p = &*page_desc;
                            if p.test_flag(PageFlag::Referenced) {
                                p.clear_flag(PageFlag::Referenced);
                                lru::lru_activate(p);
                                true
                            } else {
                                false
                            }
                        }
                    } else {
                        false
                    };
                    if !skip {
                        to_remove = Some(page_idx);
                        break;
                    }
                }
            }
            if let Some(page_idx) = to_remove {
                if let Some(page) = inode_cache.pages.remove(&page_idx) {
                    let page_desc = pfn_to_page_mut(page.pfn);
                    if !page_desc.is_null() {
                        unsafe { lru::page_remove_lru(&*page_desc); }
                    }
                    release_page_frame(page.pfn);
                }
                total_pages.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        }
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
