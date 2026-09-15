//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Page Cache — per-inode file data cache layering on top of bio block cache.
//!
//! Caches 4KB file data pages keyed by (inode_number, page_index).
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
use crate::mm::page_alloc::{alloc_page, free_page};
use crate::mm::zone::GfpFlags;
use crate::mm::page_desc::{PageFlag, PageType, pfn_to_page_mut, Page};
use crate::mm::lru;
use crate::mm::pglist::{first_online_node_mut, LRU_INACTIVE_FILE, LRU_ACTIVE_FILE};
use crate::mm::{pfn_to_phys, PAGE_SIZE};
use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

/// Maximum cached pages across all inodes (512 x 4KB = 2MB)
const MAX_CACHED_PAGES: usize = 512;

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
}

/// Per-inode page cache.
struct InodePageCache {
    /// page_index → cached page.
    pages: BTreeMap<u64, CachedPage>,
}

/// Global page cache, keyed by inode number.
pub struct PageCache {
    /// Per-inode caches.
    inodes: Spinlock<BTreeMap<u32, InodePageCache>>,
    /// Total number of cached pages (for global limit).
    total_pages: AtomicU32,
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

    /// Lookup a cached page for (ino, page_index).
    /// On hit: increments ref_count, sets Referenced flag, returns pointer.
    /// On miss: returns None.
    pub fn get(&self, ino: u32, page_index: u64) -> Option<*const u8> {
        let cache = self.inodes.lock();
        let inode_cache = cache.get(&ino)?;
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
    pub fn insert(&self, ino: u32, page_index: u64, _block_nr: u64, data: &[u8]) {
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

        let inode_cache = cache.entry(ino).or_insert_with(|| InodePageCache {
            pages: BTreeMap::new(),
        });

        // If already cached and fresh, nothing to do. (R7-C2: this used to
        // bump ref_count with no matching put — a permanent pin leak that
        // defeated eviction and pinned stale data forever.)
        if let Some(page) = inode_cache.pages.get(&page_index) {
            if !page.invalidated {
                return;
            }
            // Invalidated but still pinned by a reader: replace the frame
            // now — the old frame is freed below via the invalidated flag's
            // owning path... instead swap directly: the reader's put() will
            // find the new entry and just decrement harmlessly.
            let old_pfn = page.pfn;
            inode_cache.pages.remove(&page_index);
            let old_desc = pfn_to_page_mut(old_pfn);
            if !old_desc.is_null() {
                unsafe { lru::page_remove_lru(&*old_desc); }
            }
            release_page_frame(old_pfn);
            self.total_pages.fetch_sub(1, Ordering::Relaxed);
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
                // Store reverse-lookup info for eviction
                (*page_desc).set_mapping(ino as usize as *mut core::ffi::c_void);
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
        });
        self.total_pages.fetch_add(1, Ordering::Relaxed);
    }

    /// Release a page reference (decrement ref_count).
    pub fn put(&self, ino: u32, page_index: u64) {
        let mut cache = self.inodes.lock();
        if let Some(inode_cache) = cache.get_mut(&ino) {
            if let Some(page) = inode_cache.pages.get_mut(&page_index) {
                // Floor at 0: a get() miss (invalidated page) never
                // incremented, so a stale put must not underflow.
                page.ref_count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                    v.checked_sub(1)
                }).ok();
                // R7-C2: invalidate_inode left this page in place because a
                // reader held a pin — now that the pin is gone, free it.
                if page.ref_count.load(Ordering::Acquire) == 0 && page.invalidated {
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
                cache.remove(&ino);
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
    pub fn invalidate_inode(&self, ino: u32) {
        let mut cache = self.inodes.lock();
        if let Some(inode_cache) = cache.get_mut(&ino) {
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
                self.total_pages.fetch_sub(freed_count, Ordering::Relaxed);
            }
            if inode_cache.pages.is_empty() {
                cache.remove(&ino);
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
        cache: &mut BTreeMap<u32, InodePageCache>,
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
                let ino = page.mapping() as u32;
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
