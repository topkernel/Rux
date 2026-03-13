//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buffer I/O Layer - Block Cache Management
//!
//! Reference: Linux fs/buffer.c, include/linux/buffer_head.h
//!
//! Core concepts:
//! - `struct buffer_head`: Buffer head, represents a cached block
//! - Block cache: Caches disk blocks to improve performance
//! - Hash table with chaining: Fast lookup of cached blocks
//! - LRU eviction: Reclaim least recently used buffers when cache is full

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
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
    pub b_state: Mutex<BufferState>,
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
            b_state: Mutex::new(BufferState::new()),
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
        self.b_count.fetch_sub(1, Ordering::AcqRel) - 1
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
        // Reclaim the BufferHead
        if !self.bh.is_null() {
            unsafe {
                let _ = Box::from_raw(self.bh);
            }
        }
    }
}

// ============================================================================
// Block Cache with Chaining and LRU
// ============================================================================

/// Block cache with hash chaining and LRU eviction
///
/// # Design
/// - Hash table with chaining: Each bucket contains a linked list of entries
/// - LRU list: Global doubly-linked list for eviction policy
/// - Reference counting: Buffers with count > 0 cannot be evicted
///
/// Reference: Linux fs/buffer.c, fs/block_dev.c
struct BlockCache {
    /// Hash table (array of chain heads)
    hash_table: Mutex<Vec<Option<*mut CacheEntry>>>,
    /// Hash table size (must be power of 2)
    hash_size: usize,
    /// LRU list head (most recently used)
    lru_head: Mutex<Option<*mut CacheEntry>>,
    /// LRU list tail (least recently used)
    lru_tail: Mutex<Option<*mut CacheEntry>>,
    /// Current entry count
    count: Mutex<usize>,
    /// Maximum entries (cache capacity)
    max_entries: usize,
    /// Block size
    block_size: u32,
}

unsafe impl Send for BlockCache {}
unsafe impl Sync for BlockCache {}

impl BlockCache {
    /// Create new block cache
    ///
    /// # Arguments
    /// - `hash_size`: Number of hash buckets (must be power of 2)
    /// - `max_entries`: Maximum number of cached buffers
    /// - `block_size`: Size of each block in bytes
    fn new(hash_size: usize, max_entries: usize, block_size: u32) -> Self {
        let mut hash_table = Vec::with_capacity(hash_size);
        for _ in 0..hash_size {
            hash_table.push(None);
        }

        Self {
            hash_table: Mutex::new(hash_table),
            hash_size,
            lru_head: Mutex::new(None),
            lru_tail: Mutex::new(None),
            count: Mutex::new(0),
            max_entries,
            block_size,
        }
    }

    /// Calculate hash index
    fn hash_index(&self, device_major: u32, blocknr: u64) -> usize {
        let hash = (device_major as u64)
            .wrapping_mul(2654435761)  // Golden ratio prime
            .wrapping_add(blocknr);
        (hash as usize) & (self.hash_size - 1)
    }

    /// Lookup buffer in hash chain
    ///
    /// Returns the entry pointer if found, and moves it to LRU head
    fn lookup_entry(&self, device_major: u32, blocknr: u64) -> Option<*mut CacheEntry> {
        let index = self.hash_index(device_major, blocknr);
        let mut hash_table = self.hash_table.lock();

        let mut prev: Option<*mut CacheEntry> = None;
        let mut current = hash_table[index];

        while let Some(entry_ptr) = current {
            unsafe {
                let entry = &*entry_ptr;
                if entry.key == (device_major, blocknr) {
                    // Found! Move to front of hash chain for better locality
                    if prev.is_some() {
                        let prev_entry = &mut *prev.unwrap();
                        prev_entry.hash_next = entry.hash_next;
                        (*entry_ptr).hash_next = hash_table[index];
                        hash_table[index] = Some(entry_ptr);
                    }
                    // Move to LRU head
                    drop(hash_table);
                    self.move_to_lru_head(entry_ptr);
                    return Some(entry_ptr);
                }
                prev = Some(entry_ptr);
                current = entry.hash_next;
            }
        }

        None
    }

    /// Move entry to LRU head (most recently used)
    fn move_to_lru_head(&self, entry_ptr: *mut CacheEntry) {
        let mut lru_head = self.lru_head.lock();
        let mut lru_tail = self.lru_tail.lock();

        unsafe {
            let entry = &mut *entry_ptr;

            // Remove from current position in LRU list
            if let Some(prev) = entry.lru_prev {
                (*prev).lru_next = entry.lru_next;
            } else {
                // Already at head
            }

            if let Some(next) = entry.lru_next {
                (*next).lru_prev = entry.lru_prev;
            } else {
                // Was at tail
                *lru_tail = entry.lru_prev;
            }

            // Insert at head
            entry.lru_prev = None;
            entry.lru_next = *lru_head;

            if let Some(head) = *lru_head {
                (*head).lru_prev = Some(entry_ptr);
            }

            *lru_head = Some(entry_ptr);

            // If list was empty, this is also the tail
            if lru_tail.is_none() {
                *lru_tail = Some(entry_ptr);
            }
        }
    }

    /// Add entry to cache
    fn insert_entry(&self, entry_ptr: *mut CacheEntry) {
        unsafe {
            let entry = &mut *entry_ptr;
            let (device_major, blocknr) = entry.key;

            // Insert into hash chain at head
            let index = self.hash_index(device_major, blocknr);
            let mut hash_table = self.hash_table.lock();
            entry.hash_next = hash_table[index];
            hash_table[index] = Some(entry_ptr);
        }

        // Insert at LRU head
        self.move_to_lru_head(entry_ptr);

        // Increment count
        let mut count = self.count.lock();
        *count += 1;
    }

    /// Evict least recently used entry
    ///
    /// Returns true if an entry was evicted
    fn evict_lru(&self) -> bool {
        let mut lru_head = self.lru_head.lock();
        let mut lru_tail = self.lru_tail.lock();
        let mut hash_table = self.hash_table.lock();
        let mut count = self.count.lock();

        // Find a freeable entry (count == 0) starting from tail
        let mut current = *lru_tail;
        while let Some(entry_ptr) = current {
            unsafe {
                let entry = &*entry_ptr;

                // Check if buffer is free (refcount == 0)
                if (*entry.bh).count() == 0 {
                    // Remove from LRU list
                    if let Some(prev) = entry.lru_prev {
                        (*prev).lru_next = entry.lru_next;
                    } else {
                        *lru_head = entry.lru_next;
                    }

                    if let Some(next) = entry.lru_next {
                        (*next).lru_prev = entry.lru_prev;
                    } else {
                        *lru_tail = entry.lru_prev;
                    }

                    // Remove from hash chain
                    let (device_major, blocknr) = entry.key;
                    let index = self.hash_index(device_major, blocknr);
                    let mut prev: Option<*mut CacheEntry> = None;
                    let mut curr = hash_table[index];

                    while let Some(curr_ptr) = curr {
                        let curr_entry = &*curr_ptr;
                        if curr_ptr == entry_ptr {
                            if let Some(p) = prev {
                                (*p).hash_next = curr_entry.hash_next;
                            } else {
                                hash_table[index] = curr_entry.hash_next;
                            }
                            break;
                        }
                        prev = Some(curr_ptr);
                        curr = curr_entry.hash_next;
                    }

                    // Sync if dirty before dropping
                    if (*entry.bh).is_dirty() {
                        let _ = (*entry.bh).sync();
                    }

                    // Free the entry (Drop will free the BufferHead)
                    let _ = Box::from_raw(entry_ptr);
                    *count -= 1;
                    return true;
                }

                current = entry.lru_prev;
            }
        }

        false
    }

    /// Get or create buffer
    fn get(&self, device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
        unsafe {
            let device_major = (*device).major;

            // Try to find existing buffer
            if let Some(entry_ptr) = self.lookup_entry(device_major, blocknr) {
                let entry = &*entry_ptr;
                (*entry.bh).get();
                // Return the BufferHead pointer directly
                return Some(entry.bh);
            }

            // Need to create new buffer - check if cache is full
            {
                let count = self.count.lock();
                if *count >= self.max_entries {
                    drop(count);
                    // Try to evict LRU entry
                    if !self.evict_lru() {
                        // Cannot evict anything, all buffers are in use
                        return None;
                    }
                }
            }

            // Create new buffer
            let mut bh = Box::new(BufferHead::new(blocknr, self.block_size));

            // Read data from disk
            if let Err(_) = blkdev::blkdev_read(
                device,
                blocknr * (self.block_size as u64 / 512),
                &mut bh.b_data,
            ) {
                return None;
            }

            bh.set_device(device);
            bh.set_state_bit(BufferState::BH_Uptodate);

            // Create cache entry (takes ownership of bh)
            let entry = Box::new(CacheEntry::new(bh, device_major, blocknr));
            let entry_ptr = Box::into_raw(entry);

            // Insert into cache
            self.insert_entry(entry_ptr);

            // Return the BufferHead pointer
            Some((*entry_ptr).bh)
        }
    }

    /// Release buffer (decrement refcount)
    fn put(&self, bh: *const BufferHead) {
        unsafe {
            let bh_ref = &*bh;
            bh_ref.put();
        }
    }

    /// Sync all dirty buffers
    fn sync_all(&self) -> Result<(), i32> {
        let hash_table = self.hash_table.lock();

        for bucket in hash_table.iter() {
            let mut current = *bucket;
            while let Some(entry_ptr) = current {
                unsafe {
                    let entry = &*entry_ptr;
                    if (*entry.bh).is_dirty() {
                        (*entry.bh).sync()?;
                    }
                    current = entry.hash_next;
                }
            }
        }

        Ok(())
    }

    /// Invalidate all buffers (for device removal, etc.)
    fn invalidate(&self) {
        let mut hash_table = self.hash_table.lock();
        let mut lru_head = self.lru_head.lock();
        let mut lru_tail = self.lru_tail.lock();
        let mut count = self.count.lock();

        for i in 0..hash_table.len() {
            let mut current = hash_table[i];
            while let Some(entry_ptr) = current {
                unsafe {
                    let entry = &*entry_ptr;
                    let next = entry.hash_next;
                    let _ = Box::from_raw(entry_ptr);
                    current = next;
                }
            }
            hash_table[i] = None;
        }

        *lru_head = None;
        *lru_tail = None;
        *count = 0;
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
            // Create cache:
            // - 64 hash buckets
            // - 256 max entries (1MB for 4KB blocks)
            // - 4KB block size
            BLOCK_CACHE = Some(BlockCache::new(64, 256, 4096));
            CACHE_INIT.store(true, Ordering::Release);
        }
        BLOCK_CACHE.as_ref().unwrap()
    }
}

/// Read a block from cache (or disk if not cached)
///
/// Reference: Linux bread() in fs/buffer.c
pub fn bread(device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
    get_block_cache().get(device, blocknr)
}

/// Release a buffer
///
/// Reference: Linux brelse() in fs/buffer.c
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
