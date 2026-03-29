//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Page Cache — per-inode file data cache layering on top of bio block cache.
//!
//! Caches 4KB file data pages keyed by (inode_number, page_index).
//! Reduces disk I/O for repeated reads and enables read-ahead population.

use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// Maximum cached pages across all inodes (512 × 4KB = 2MB)
const MAX_PAGES: usize = 512;

/// Page size — always 4KB, matches filesystem block size.
const PAGE_SIZE: usize = 4096;

/// A cached page of file data.
struct CachedPage {
    /// Page data (copied from BufferHead).
    data: [u8; PAGE_SIZE],
    /// Reference count — pages with ref_count > 0 are not evicted.
    ref_count: AtomicU32,
}

/// Per-inode page cache.
struct InodePageCache {
    /// page_index → cached page data.
    pages: BTreeMap<u64, Box<CachedPage>>,
}

/// Global page cache, keyed by inode number.
pub struct PageCache {
    /// Per-inode caches.
    inodes: Mutex<BTreeMap<u32, InodePageCache>>,
    /// Total number of cached pages (for global limit).
    total_pages: AtomicU32,
}

impl PageCache {
    /// Create a new empty page cache.
    pub const fn new() -> Self {
        Self {
            inodes: Mutex::new(BTreeMap::new()),
            total_pages: AtomicU32::new(0),
        }
    }

    /// Lookup a cached page for (ino, page_index).
    /// On hit: increments ref_count and returns pointer to page data.
    /// On miss: returns None.
    pub fn get(&self, ino: u32, page_index: u64) -> Option<*const u8> {
        let cache = self.inodes.lock();
        let inode_cache = cache.get(&ino)?;
        let page = inode_cache.pages.get(&page_index)?;
        page.ref_count.fetch_add(1, Ordering::AcqRel);
        Some(page.data.as_ptr())
    }

    /// Insert a newly-read page into the cache.
    /// If the page already exists, just increments ref_count.
    pub fn insert(&self, ino: u32, page_index: u64, _block_nr: u64, data: &[u8]) {
        let mut cache = self.inodes.lock();

        // Evict if needed
        while self.total_pages.load(Ordering::Relaxed) as usize >= MAX_PAGES {
            Self::evict_one(&mut cache, &self.total_pages);
        }

        let inode_cache = cache.entry(ino).or_insert_with(|| InodePageCache {
            pages: BTreeMap::new(),
        });

        // If already cached, just bump ref
        if let Some(page) = inode_cache.pages.get(&page_index) {
            page.ref_count.fetch_add(1, Ordering::AcqRel);
            return;
        }

        // Create new cached page
        let mut page_data = [0u8; PAGE_SIZE];
        let copy_len = core::cmp::min(data.len(), PAGE_SIZE);
        page_data[..copy_len].copy_from_slice(&data[..copy_len]);

        inode_cache.pages.insert(page_index, Box::new(CachedPage {
            data: page_data,
            ref_count: AtomicU32::new(1),
        }));
        self.total_pages.fetch_add(1, Ordering::Relaxed);
    }

    /// Release a page reference (decrement ref_count).
    pub fn put(&self, ino: u32, page_index: u64) {
        let cache = self.inodes.lock();
        if let Some(inode_cache) = cache.get(&ino) {
            if let Some(page) = inode_cache.pages.get(&page_index) {
                page.ref_count.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    /// Invalidate all cached pages for a given inode.
    /// Called after writes or truncates to prevent stale data.
    pub fn invalidate_inode(&self, ino: u32) {
        let mut cache = self.inodes.lock();
        if let Some(inode_cache) = cache.remove(&ino) {
            self.total_pages.fetch_sub(inode_cache.pages.len() as u32, Ordering::Relaxed);
        }
    }

    /// Evict one page with ref_count == 0 from any inode (oldest first via BTreeMap order).
    fn evict_one(
        cache: &mut BTreeMap<u32, InodePageCache>,
        total_pages: &AtomicU32,
    ) {
        // Iterate inodes, try to evict first eligible page
        for (_ino, inode_cache) in cache.iter_mut() {
            // Find first page with ref_count == 0
            let mut to_remove = None;
            for (&page_idx, page) in inode_cache.pages.iter() {
                if page.ref_count.load(Ordering::Acquire) == 0 {
                    to_remove = Some(page_idx);
                    break;
                }
            }
            if let Some(page_idx) = to_remove {
                inode_cache.pages.remove(&page_idx);
                total_pages.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// Global page cache instance.
static PAGE_CACHE: PageCache = PageCache::new();

/// Get a reference to the global page cache.
pub fn get_page_cache() -> &'static PageCache {
    &PAGE_CACHE
}
