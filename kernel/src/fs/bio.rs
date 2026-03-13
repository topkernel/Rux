//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Buffer I/O Layer - Block Cache Management
//!
//!
//! Core concepts:
//! - `struct buffer_head`: Buffer head, represents a cached block
//! - Block cache: Caches disk blocks to improve performance
//! - Hash table: Fast lookup of cached blocks

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::drivers::blkdev;

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
        // Add debug information
        if device.is_null() {
            crate::console::puts("bio: set_device: NULL device!\n");
            return;
        }
        self.b_device = Some(device);
        // Set state bit directly to avoid potential deadlock
        // let mut state = self.b_state.lock();
        // state.set(BufferState::BH_Mapped);
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
            blkdev::blkdev_write(
                device,
                self.b_blocknr * (self.b_size as u64 / 512),
                &self.b_data,
            )?;
            self.clear_state_bit(BufferState::BH_Dirty);
            Ok(())
        } else {
            Err(-6)  // ENXIO
        }
    }
}

struct BlockCache {
    /// Buffer hash table
    /// Index: (device major number, block number) % hash table size
    buffers: Mutex<Vec<Option<*mut BufferHead>>>,
    /// Hash table size (must be power of 2)
    hash_size: usize,
    /// Block size
    block_size: u32,
}

unsafe impl Send for BlockCache {}
unsafe impl Sync for BlockCache {}

impl BlockCache {
    /// Create new block cache
    fn new(hash_size: usize, block_size: u32) -> Self {
        // Use raw pointer initialization to avoid needing Clone trait
        let mut vec = Vec::with_capacity(hash_size);
        for _ in 0..hash_size {
            vec.push(None);
        }

        Self {
            buffers: Mutex::new(vec),
            hash_size,
            block_size,
        }
    }

    /// Calculate hash index
    fn hash_index(&self, device_major: u32, blocknr: u64) -> usize {
        // Use simple hash function
        let hash = (device_major as u64).wrapping_mul(31).wrapping_add(blocknr);
        (hash as usize) & (self.hash_size - 1)
    }

    /// Lookup buffer
    fn lookup(&self, device_major: u32, blocknr: u64) -> Option<*const BufferHead> {
        let index = self.hash_index(device_major, blocknr);
        let buffers = self.buffers.lock();

        if let Some(bh_ptr) = buffers[index] {
            unsafe {
                let bh = &*bh_ptr;
                if bh.b_blocknr == blocknr {
                    if let Some(device) = bh.b_device {
                        if (*device).major == device_major {
                            return Some(bh_ptr);
                        }
                    }
                }
            }
        }

        None
    }

    /// Get or create buffer
    fn get(&self, device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
        unsafe {
            let device_major = (*device).major;

            // First try to find existing buffer
            if let Some(bh) = self.lookup(device_major, blocknr) {
                let bh_ref = &*bh;
                bh_ref.get();
                return Some(bh as *mut u8 as *mut BufferHead);
            }

            // Create new buffer
            let bh = Box::new(BufferHead::new(blocknr, self.block_size));

            // Read data from disk
            let mut bh_owned = bh;
            if let Err(_e) = blkdev::blkdev_read(
                device,
                blocknr * (self.block_size as u64 / 512),
                &mut bh_owned.b_data,
            ) {
                return None;
            }

            bh_owned.set_device(device);
            bh_owned.set_state_bit(BufferState::BH_Uptodate);

            // Convert to raw pointer and leak
            let bh_ptr = Box::leak(bh_owned);

            // Insert into hash table
            let index = self.hash_index(device_major, blocknr);
            let mut buffers = self.buffers.lock();
            buffers[index] = Some(bh_ptr);

            Some(bh_ptr)
        }
    }

    /// Release buffer
    fn put(&self, _bh: *const BufferHead) {
        // Simplified implementation: don't actually release
        // In complete implementation, should decrement reference count,
        // and reclaim when count reaches 0
    }

    /// Sync all dirty buffers
    fn sync_all(&self) -> Result<(), i32> {
        let buffers = self.buffers.lock();

        for bh_opt in buffers.iter() {
            if let Some(bh_ptr) = *bh_opt {
                unsafe {
                    let bh = &*bh_ptr;
                    if bh.is_dirty() {
                        bh.sync()?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Invalidate all buffers
    fn invalidate(&self) {
        let mut buffers = self.buffers.lock();

        for i in 0..buffers.len() {
            if let Some(bh_ptr) = buffers[i] {
                unsafe {
                    // Reclaim ownership and release
                    let _ = Box::from_raw(bh_ptr);
                }
                buffers[i] = None;
            }
        }
    }
}

// Use lazy_static style initialization
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

static CACHE_INIT: AtomicBool = AtomicBool::new(false);
static mut BLOCK_CACHE: Option<BlockCache> = None;

fn get_block_cache() -> &'static BlockCache {
    unsafe {
        if !CACHE_INIT.load(AtomicOrdering::Acquire) {
            // Create cache with 16 entries (64KB)
            BLOCK_CACHE = Some(BlockCache::new(16, 4096));
            CACHE_INIT.store(true, AtomicOrdering::Release);
        }
        BLOCK_CACHE.as_ref().unwrap()
    }
}

pub fn bread(device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
    get_block_cache().get(device, blocknr)
}

pub fn brelse(bh: *const BufferHead) {
    get_block_cache().put(bh)
}

pub fn sync_dirty_buffer(bh: *const BufferHead) -> Result<(), i32> {
    unsafe {
        let bh_ref = &*bh;
        bh_ref.sync()
    }
}

pub fn sync_buffers() -> Result<(), i32> {
    get_block_cache().sync_all()
}

pub fn init() {
    // Cache will be auto-initialized on first use (lazy loading mode)
    // Don't initialize here to avoid panic from excessive memory allocation at boot
}
