//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buffer I/O Layer - Block Cache Management
//!
//! Core concepts:
//! - `struct buffer_head`: Buffer head, represents a cached block
//! - Block cache: Caches disk blocks to improve performance
//! - Hash table with chaining: Fast lookup of cached blocks
//! - LRU eviction: Reclaim least recently used buffers when cache is full

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::drivers::blkdev;

// ============================================================================
// Buffer State
// ============================================================================

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct BufferState(u8);

impl BufferState {
    pub const BH_Uptodate: u8 = 0;  // Buffer contains valid data
    pub const BH_Dirty: u8 = 1;     // Buffer needs to be written to disk
    pub const BH_Lock: u8 = 2;      // Buffer is locked
    pub const BH_Req: u8 = 3;       // Buffer has been requested
    pub const BH_Mapped: u8 = 4;    // Buffer is mapped to a disk block

    pub fn new() -> Self {
        Self(0)
    }

    pub fn set(&mut self, bit: u8) {
        self.0 |= 1 << bit;
    }

    pub fn clear(&mut self, bit: u8) {
        self.0 &= !(1 << bit);
    }

    pub fn test(&self, bit: u8) -> bool {
        (self.0 & (1 << bit)) != 0
    }

    pub fn is_uptodate(&self) -> bool {
        self.test(Self::BH_Uptodate)
    }

    pub fn is_dirty(&self) -> bool {
        self.test(Self::BH_Dirty)
    }

    pub fn is_locked(&self) -> bool {
        self.test(Self::BH_Lock)
    }

    pub fn is_mapped(&self) -> bool {
        self.test(Self::BH_Mapped)
    }
}

// ============================================================================
// Buffer Head
// ============================================================================

pub struct BufferHead {
    /// Block device
    pub b_device: Option<*const blkdev::GenDisk>,
    /// Block number
    pub b_blocknr: u64,
    /// Block size
    pub b_size: u32,
    /// Buffer state
    pub b_state: Spinlock<BufferState>,
    /// Data
    pub b_data: Vec<u8>,
    /// Reference count
    b_count: AtomicU32,
}

unsafe impl Send for BufferHead {}
unsafe impl Sync for BufferHead {}

impl BufferHead {
    /// Create new buffer head
    pub fn new(blocknr: u64, size: u32) -> Self {
        Self {
            b_device: None,
            b_blocknr: blocknr,
            b_size: size,
            b_state: Spinlock::new(BufferState::new()),
            b_data: vec![0u8; size as usize],
            b_count: AtomicU32::new(1),
        }
    }

    /// Set block device
    pub fn set_device(&mut self, device: *const blkdev::GenDisk) {
        if device.is_null() {
            crate::console::puts("bio: set_device: NULL device!\n");
            return;
        }
        self.b_device = Some(device);
    }

    /// Get state
    pub fn get_state(&self) -> BufferState {
        let state = self.b_state.lock();
        *state
    }

    /// Set state bit
    pub fn set_state_bit(&self, bit: u8) {
        let mut state = self.b_state.lock();
        state.set(bit);
    }

    /// Clear state bit
    pub fn clear_state_bit(&self, bit: u8) {
        let mut state = self.b_state.lock();
        state.clear(bit);
    }

    /// Check if dirty
    pub fn is_dirty(&self) -> bool {
        let state = self.b_state.lock();
        state.is_dirty()
    }

    /// Increment reference count
    pub fn get(&self) {
        self.b_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn put(&self) -> u32 {
        self.b_count.fetch_sub(1, Ordering::AcqRel).wrapping_sub(1)
    }

    /// Get reference count
    pub fn count(&self) -> u32 {
        self.b_count.load(Ordering::Acquire)
    }

    /// Read data
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.b_size as usize {
            return 0;
        }
        let available = self.b_size as usize - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.b_data[offset..offset + to_read]);
        to_read
    }

    /// Write data
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if offset >= self.b_data.len() {
            return 0;
        }
        let available = self.b_data.len() - offset;
        let to_write = core::cmp::min(buf.len(), available);
        self.b_data[offset..offset + to_write].copy_from_slice(&buf[..to_write]);
        self.set_state_bit(BufferState::BH_Dirty);
        to_write
    }

    /// Sync to disk
    pub fn sync(&self) -> Result<(), i32> {
        if !self.is_dirty() {
            return Ok(());
        }

        if let Some(device) = self.b_device {
            unsafe {
                blkdev::blkdev_write(
                    device,
                    self.b_blocknr * (self.b_size as u64 / 512),
                    &self.b_data,
                )?;
                self.clear_state_bit(BufferState::BH_Dirty);
            }
        }
        Ok(())
    }
}

// ============================================================================
// Cache Entry (for chaining and LRU)
// ============================================================================

/// Cache entry wrapper for hash chaining and LRU list
struct CacheEntry {
    /// The actual buffer head (raw pointer, owned by this entry)
    bh: *mut BufferHead,
    /// Key: (device_major, blocknr) for fast lookup
    key: (u32, u64),
    /// Next entry in hash chain
    hash_next: Option<*mut CacheEntry>,
    /// Previous entry in LRU list (more recent)
    lru_prev: Option<*mut CacheEntry>,
    /// Next entry in LRU list (less recent)
    lru_next: Option<*mut CacheEntry>,
}

impl CacheEntry {
    fn new(bh: Box<BufferHead>, device_major: u32, blocknr: u64) -> Self {
        Self {
            bh: Box::into_raw(bh),
            key: (device_major, blocknr),
            hash_next: None,
            lru_prev: None,
            lru_next: None,
        }
    }
}

impl Drop for CacheEntry {
    fn drop(&mut self) {
        if !self.bh.is_null() {
            unsafe {
                let _ = Box::from_raw(self.bh);
            }
        }
    }
}

// ============================================================================
// Block Cache with Per-Bucket Locking and LRU
// ============================================================================

/// A single hash bucket, protected by its own spinlock.
struct HashBucket {
    /// Head of the hash chain
    head: Option<*mut CacheEntry>,
}

/// Shared LRU list state (accessed under any bucket lock — each operation
/// touches only O(1) LRU nodes so contention is minimal).
struct LruState {
    head: Option<*mut CacheEntry>,
    tail: Option<*mut CacheEntry>,
}

/// Block cache with per-bucket spinlock, global LRU, and atomic count.
///
/// # Lock hierarchy
/// 1. **Bucket lock** (`Spinlock<HashBucket>`) — protects one hash chain
/// 2. **BufferState lock** (`Spinlock<BufferState>`) — per-buffer state flags
///
/// No global mutex is held during I/O. Eviction syncs dirty buffers
/// *after* releasing the bucket lock.
struct BlockCache {
    /// Per-bucket hash chains, each with its own lock
    buckets: Vec<Spinlock<HashBucket>>,
    /// Global LRU list (manipulated only while holding a bucket lock)
    lru: Spinlock<LruState>,
    /// Global entry count (atomic — no lock needed to check capacity)
    count: AtomicU32,
    /// Hash table size (must be power of 2)
    hash_size: usize,
    /// Maximum entries (cache capacity)
    max_entries: usize,
    /// Block size
    block_size: u32,
}

unsafe impl Send for BlockCache {}
unsafe impl Sync for BlockCache {}

impl BlockCache {
    fn new(hash_size: usize, max_entries: usize, block_size: u32) -> Self {
        let mut buckets = Vec::with_capacity(hash_size);
        for _ in 0..hash_size {
            buckets.push(Spinlock::new(HashBucket { head: None }));
        }

        Self {
            buckets,
            lru: Spinlock::new(LruState { head: None, tail: None }),
            count: AtomicU32::new(0),
            hash_size,
            max_entries,
            block_size,
        }
    }

    #[inline]
    fn hash_index(&self, device_major: u32, blocknr: u64) -> usize {
        let hash = (device_major as u64)
            .wrapping_mul(2654435761)
            .wrapping_add(blocknr);
        (hash as usize) & (self.hash_size - 1)
    }

    /// Move entry to LRU head. Caller must hold the lru lock.
    unsafe fn move_to_lru_head(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        let entry = &mut *entry_ptr;

        // Unlink from current position
        if let Some(prev) = entry.lru_prev {
            (*prev).lru_next = entry.lru_next;
        }
        if let Some(next) = entry.lru_next {
            (*next).lru_prev = entry.lru_prev;
        } else {
            lru.tail = entry.lru_prev;
        }

        // Insert at head
        entry.lru_prev = None;
        entry.lru_next = lru.head;
        if let Some(head) = lru.head {
            (*head).lru_prev = Some(entry_ptr);
        }
        lru.head = Some(entry_ptr);
        if lru.tail.is_none() {
            lru.tail = Some(entry_ptr);
        }
    }

    /// Unlink entry from LRU list. Caller must hold the lru lock.
    unsafe fn remove_from_lru(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        let entry = &*entry_ptr;
        if let Some(prev) = entry.lru_prev {
            (*prev).lru_next = entry.lru_next;
        } else {
            lru.head = entry.lru_next;
        }
        if let Some(next) = entry.lru_next {
            (*next).lru_prev = entry.lru_prev;
        } else {
            lru.tail = entry.lru_prev;
        }
        (*entry_ptr).lru_prev = None;
        (*entry_ptr).lru_next = None;
    }

    /// Evict one entry from the LRU tail.
    ///
    /// Scans LRU tail for an entry with refcount == 0, removes it from
    /// both the hash chain and LRU list, then syncs to disk **after**
    /// releasing all locks. Does not hold any bucket lock during I/O.
    fn evict_one(&self) -> bool {
        // Phase 1: Find a freeable entry (need lru lock to walk LRU)
        let victim = {
            let mut lru = self.lru.lock();
            let mut current = lru.tail;
            let mut found = None;

            while let Some(entry_ptr) = current {
                unsafe {
                    let entry = &*entry_ptr;
                    if (*entry.bh).count() == 0 {
                        found = Some(entry_ptr);
                        break;
                    }
                    current = entry.lru_prev;
                }
            }

            match found {
                Some(entry_ptr) => {
                    // Remove from LRU
                    unsafe { Self::remove_from_lru(&mut lru, entry_ptr); }
                    entry_ptr
                }
                None => return false, // all buffers in use
            }
        };
        // lru lock released here

        // Phase 2: Remove from hash chain (need the victim's bucket lock)
        let victim_key = unsafe { (*victim).key };
        let bucket_idx = self.hash_index(victim_key.0, victim_key.1);

        unsafe {
            let mut bucket = self.buckets[bucket_idx].lock();

            // Unlink from hash chain
            let mut prev: Option<*mut CacheEntry> = None;
            let mut current = bucket.head;
            while let Some(cp) = current {
                if cp == victim {
                    if let Some(pp) = prev {
                        (*pp).hash_next = (*cp).hash_next;
                    } else {
                        bucket.head = (*cp).hash_next;
                    }
                    break;
                }
                prev = Some(cp);
                current = (*cp).hash_next;
            }
        }
        // bucket lock released here

        // Phase 3: Sync if dirty (NO locks held — I/O is safe)
        unsafe {
            if (*(*victim).bh).is_dirty() {
                let _ = (*(*victim).bh).sync();
            }
        }

        // Phase 4: Free the entry
        unsafe {
            let _ = Box::from_raw(victim);
        }
        self.count.fetch_sub(1, Ordering::Release);
        true
    }

    /// Get or create buffer (synchronous read on cache miss).
    fn get(&self, device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
        unsafe {
            let device_major = (*device).major;
            let index = self.hash_index(device_major, blocknr);

            // Phase 1: Lookup under bucket lock
            {
                let mut bucket = self.buckets[index].lock();
                let mut prev: Option<*mut CacheEntry> = None;
                let mut current = bucket.head;

                while let Some(entry_ptr) = current {
                    let entry = &*entry_ptr;
                    if entry.key == (device_major, blocknr) {
                        // Found — move to hash chain head
                        if prev.is_some() {
                            let prev_entry = &mut *prev.unwrap();
                            prev_entry.hash_next = entry.hash_next;
                            (*entry_ptr).hash_next = bucket.head;
                            bucket.head = Some(entry_ptr);
                        }
                        // Move to LRU head
                        let mut lru = self.lru.lock();
                        Self::move_to_lru_head(&mut lru, entry_ptr);
                        (*entry.bh).get();
                        return Some(entry.bh);
                    }
                    prev = Some(entry_ptr);
                    current = entry.hash_next;
                }
            }

            // Phase 1.5: Evict if cache is full
            while self.count.load(Ordering::Acquire) as usize >= self.max_entries {
                if !self.evict_one() {
                    return None; // all buffers in use
                }
            }

            // Phase 2: Read from disk (no locks held)
            let mut bh = Box::new(BufferHead::new(blocknr, self.block_size));

            if let Err(_) = blkdev::blkdev_read(
                device,
                blocknr * (self.block_size as u64 / 512),
                &mut bh.b_data,
            ) {
                return None;
            }

            bh.set_device(device);
            bh.set_state_bit(BufferState::BH_Uptodate);

            // Create cache entry
            let entry = Box::new(CacheEntry::new(bh, device_major, blocknr));
            let entry_ptr = Box::into_raw(entry);

            // Phase 3: Insert into cache
            {
                let mut bucket = self.buckets[index].lock();

                // Double-check for duplicate inserted by another thread
                let mut current = bucket.head;
                while let Some(cp) = current {
                    if (*cp).key == (device_major, blocknr) {
                        (*(*cp).bh).get();
                        let mut lru = self.lru.lock();
                        Self::move_to_lru_head(&mut lru, cp);
                        let _ = Box::from_raw(entry_ptr);
                        return Some((*cp).bh);
                    }
                    current = (*cp).hash_next;
                }

                // Insert at hash chain head
                (*entry_ptr).hash_next = bucket.head;
                bucket.head = Some(entry_ptr);

                // Insert at LRU head
                let mut lru = self.lru.lock();
                Self::move_to_lru_head(&mut lru, entry_ptr);
            }

            self.count.fetch_add(1, Ordering::Release);
            Some((*entry_ptr).bh)
        }
    }

    /// Release buffer (decrement refcount)
    fn put(&self, bh: *const BufferHead) {
        unsafe {
            (*bh).put();
        }
    }

    /// Sync all dirty buffers.
    ///
    /// Phase 1: Collect dirty buffer pointers under per-bucket locks.
    /// Phase 2: Sync each buffer without holding any lock.
    fn sync_all(&self) -> Result<(), i32> {
        // Phase 1: Collect dirty buffers (increment refcount to prevent eviction)
        let mut dirty_list: Vec<*mut BufferHead> = Vec::new();

        for i in 0..self.hash_size {
            let bucket = self.buckets[i].lock();
            let mut current = bucket.head;
            while let Some(entry_ptr) = current {
                unsafe {
                    let entry = &*entry_ptr;
                    if (*entry.bh).is_dirty() {
                        (*entry.bh).get();
                        dirty_list.push(entry.bh);
                    }
                    current = entry.hash_next;
                }
            }
        }

        // Phase 2: Sync without holding any lock
        let mut first_error: i32 = 0;
        for bh in &dirty_list {
            unsafe {
                if let Err(e) = (**bh).sync() {
                    if first_error == 0 {
                        first_error = e;
                    }
                }
            }
            self.put(*bh);
        }

        if first_error != 0 {
            Err(first_error)
        } else {
            Ok(())
        }
    }

    /// Invalidate all buffers (for device removal, etc.)
    fn invalidate(&self) {
        for i in 0..self.hash_size {
            let mut bucket = self.buckets[i].lock();
            let mut current = bucket.head;
            while let Some(entry_ptr) = current {
                unsafe {
                    let next = (*entry_ptr).hash_next;
                    let _ = Box::from_raw(entry_ptr);
                    current = next;
                }
            }
            bucket.head = None;
        }

        let mut lru = self.lru.lock();
        lru.head = None;
        lru.tail = None;
        self.count.store(0, Ordering::Release);
    }
}

impl Drop for BlockCache {
    fn drop(&mut self) {
        self.invalidate();
    }
}

// ============================================================================
// Public API
// ============================================================================

use core::sync::atomic::AtomicBool;

static CACHE_INIT: AtomicBool = AtomicBool::new(false);
static mut BLOCK_CACHE: Option<BlockCache> = None;

fn get_block_cache() -> &'static BlockCache {
    unsafe {
        if !CACHE_INIT.load(Ordering::Acquire) {
            // Use compare_exchange to ensure only one thread initializes
            // the cache on multi-core systems
            // Create cache:
            // - 64 hash buckets
            // - 256 max entries (1MB for 4KB blocks)
            // - 4KB block size
            let cache = BlockCache::new(64, 1024, 4096);
            if CACHE_INIT.compare_exchange(
                false,
                true,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                BLOCK_CACHE = Some(cache);
            }
        }
        BLOCK_CACHE.as_ref().unwrap()
    }
}

/// Read a block from cache (or disk if not cached)
///
/// Read a block from cache (or disk if not cached)
pub fn bread(device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
    get_block_cache().get(device, blocknr)
}

/// Async block read: submit I/O without blocking, return buffer head immediately.
///
/// On cache hit: returns buffer with `BH_Uptodate` set (no I/O needed).
/// On cache miss: creates a new BufferHead, submits async I/O via
/// `blkdev::blkdev_read_async`, inserts into cache, and returns the buffer.
/// The buffer data is **not** valid until `bread_wait()` completes.
pub fn bread_async(
    device: *const blkdev::GenDisk,
    blocknr: u64,
    completion: &crate::fs::io_completion::IoCompletion,
) -> Option<*mut BufferHead> {
    unsafe {
        let device_major = (*device).major;
        let cache = get_block_cache();
        let index = cache.hash_index(device_major, blocknr);

        // Phase 1: Lookup under bucket lock
        {
            let mut bucket = cache.buckets[index].lock();
            let mut prev: Option<*mut CacheEntry> = None;
            let mut current = bucket.head;

            while let Some(entry_ptr) = current {
                let entry = &*entry_ptr;
                if entry.key == (device_major, blocknr) {
                    // Cache hit
                    if prev.is_some() {
                        let prev_entry = &mut *prev.unwrap();
                        prev_entry.hash_next = entry.hash_next;
                        (*entry_ptr).hash_next = bucket.head;
                        bucket.head = Some(entry_ptr);
                    }
                    let mut lru = cache.lru.lock();
                    BlockCache::move_to_lru_head(&mut lru, entry_ptr);
                    (*entry.bh).get();
                    return Some(entry.bh);
                }
                prev = Some(entry_ptr);
                current = entry.hash_next;
            }
        }

        // Phase 1.5: Evict if cache is full
        while cache.count.load(Ordering::Acquire) as usize >= cache.max_entries {
            if !cache.evict_one() {
                return None;
            }
        }

        // Phase 2: Cache miss — submit async I/O (no lock held)
        let block_size = cache.block_size;
        let mut bh = Box::new(BufferHead::new(blocknr, block_size));
        bh.set_device(device);
        bh.set_state_bit(BufferState::BH_Req);

        let sectors_per_block = block_size as u64 / 512;
        if let Err(_) = blkdev::blkdev_read_async(
            device,
            blocknr * sectors_per_block,
            &mut bh.b_data,
            completion,
        ) {
            return None;
        }

        // Phase 3: Insert into cache
        let entry = Box::new(CacheEntry::new(bh, device_major, blocknr));
        let entry_ptr = Box::into_raw(entry);

        {
            let mut bucket = cache.buckets[index].lock();

            // Double-check for duplicate
            let mut current = bucket.head;
            while let Some(cp) = current {
                if (*cp).key == (device_major, blocknr) {
                    (*(*cp).bh).get();
                    let mut lru = cache.lru.lock();
                    BlockCache::move_to_lru_head(&mut lru, cp);
                    let _ = Box::from_raw(entry_ptr);
                    return Some((*cp).bh);
                }
                current = (*cp).hash_next;
            }

            (*entry_ptr).hash_next = bucket.head;
            bucket.head = Some(entry_ptr);

            let mut lru = cache.lru.lock();
            BlockCache::move_to_lru_head(&mut lru, entry_ptr);
        }

        cache.count.fetch_add(1, Ordering::Release);
        Some((*entry_ptr).bh)
    }
}

/// Wait for an async buffer read to complete.
///
/// Blocks until the IoCompletion signals done, then marks the buffer
/// as up-to-date and clears the in-flight flag.
pub fn bread_wait(bh: *mut BufferHead, completion: &crate::fs::io_completion::IoCompletion) {
    unsafe {
        let status = completion.wait();
        if status == 0 {
            (*bh).set_state_bit(BufferState::BH_Uptodate);
        }
        (*bh).clear_state_bit(BufferState::BH_Req);
    }
}

/// Release a buffer
pub fn brelse(bh: *const BufferHead) {
    get_block_cache().put(bh)
}

/// Sync a dirty buffer to disk
pub fn sync_dirty_buffer(bh: *const BufferHead) -> Result<(), i32> {
    unsafe {
        let bh_ref = &*bh;
        bh_ref.sync()
    }
}

/// Sync all dirty buffers
pub fn sync_buffers() -> Result<(), i32> {
    get_block_cache().sync_all()
}

/// Initialize block cache (lazy init on first use)
pub fn init() {
    // Cache auto-initializes on first use
}
