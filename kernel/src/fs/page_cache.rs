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
//! making them trackable by the page reclamation subsystem (kswapd).
//!
//! Physical pages are accessed through the linear mapping (phys_to_virt),
//! not through identity mapping, since the kernel only identity-maps a
//! small region around the kernel image.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::mm::page_alloc::{alloc_page, free_page};
use crate::mm::zone::GfpFlags;
use crate::mm::page_desc::{PageFlag, PageType, pfn_to_page_mut};
use crate::mm::{pfn_to_phys, PAGE_SIZE};
use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

/// Maximum cached pages across all inodes (512 x 4KB = 2MB)
const MAX_CACHED_PAGES: usize = 512;

/// A cached page of file data, backed by a zone-allocated physical page frame.
struct CachedPage {
    /// Physical frame number (allocated from zone allocator).
    pfn: usize,
    /// Reference count — pages with ref_count > 0 are not evicted.
    ref_count: AtomicU32,
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
    /// On hit: increments ref_count and returns pointer to page data.
    /// On miss: returns None.
    pub fn get(&self, ino: u32, page_index: u64) -> Option<*const u8> {
        let cache = self.inodes.lock();
        let inode_cache = cache.get(&ino)?;
        let page = inode_cache.pages.get(&page_index)?;
        page.ref_count.fetch_add(1, Ordering::AcqRel);
        let phys = pfn_to_phys(page.pfn);
        Some(phys_to_virt_ptr(phys) as *const u8)
    }

    /// Insert a newly-read page into the cache.
    /// If the page already exists, just increments ref_count.
    pub fn insert(&self, ino: u32, page_index: u64, _block_nr: u64, data: &[u8]) {
        let mut cache = self.inodes.lock();

        // Evict if needed
        while self.total_pages.load(Ordering::Relaxed) as usize >= MAX_CACHED_PAGES {
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

        // Allocate a physical page frame from zone allocator
        let phys_addr = alloc_page(GfpFlags::GFP_KERNEL);
        if phys_addr == 0 {
            return;
        }

        let pfn = phys_addr / PAGE_SIZE;

        // Mark page descriptor as page cache page
        let page_desc = pfn_to_page_mut(pfn);
        if !page_desc.is_null() {
            unsafe {
                (*page_desc).set_page_type(PageType::PageCache);
                (*page_desc).set_flag(PageFlag::UpToDate);
            }
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
            ref_count: AtomicU32::new(1),
        });
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
            let count = inode_cache.pages.len();
            for (_, page) in inode_cache.pages.iter() {
                release_page_frame(page.pfn);
            }
            self.total_pages.fetch_sub(count as u32, Ordering::Relaxed);
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
            // Quick check before acquiring the lock
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
                // Nothing was evicted — all remaining pages have ref_count > 0
                break;
            }
            freed += 1;
        }
        freed
    }

    /// Evict one page with ref_count == 0 from any inode (oldest first via BTreeMap order).
    fn evict_one(
        cache: &mut BTreeMap<u32, InodePageCache>,
        total_pages: &AtomicU32,
    ) {
        for (_ino, inode_cache) in cache.iter_mut() {
            let mut to_remove = None;
            for (&page_idx, page) in inode_cache.pages.iter() {
                if page.ref_count.load(Ordering::Acquire) == 0 {
                    to_remove = Some(page_idx);
                    break;
                }
            }
            if let Some(page_idx) = to_remove {
                if let Some(page) = inode_cache.pages.remove(&page_idx) {
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
            (*page_desc).set_page_type(PageType::Normal);
            (*page_desc).clear_flag(PageFlag::UpToDate);
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
