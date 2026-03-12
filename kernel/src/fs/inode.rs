//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Index Node (Inode) Management
//!
//!
//! Core concepts:
//! - `struct inode`: Index node, represents an object in the filesystem
//! - `struct super_block`: Superblock, represents a filesystem
//! - `struct inode_operations`: Inode operation function pointers

use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::buffer::FileBuffer;

/// Inode number type
pub type Ino = u64;

/// Inode mode (file type and permissions)
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct InodeMode(u32);

impl InodeMode {
    /// File type mask
    pub const S_IFMT: u32 = 0o0170000;

    /// Regular file
    pub const S_IFREG: u32 = 0o0100000;
    /// Directory
    pub const S_IFDIR: u32 = 0o0040000;
    /// Character device
    pub const S_IFCHR: u32 = 0o0020000;
    /// Block device
    pub const S_IFBLK: u32 = 0o0060000;
    /// FIFO (named pipe)
    pub const S_IFIFO: u32 = 0o0010000;
    /// Symbolic link
    pub const S_IFLNK: u32 = 0o0120000;
    /// Socket
    pub const S_IFSOCK: u32 = 0o0140000;

    /// Permission bits
    pub const S_IRWXU: u32 = 0o0700;  // User permissions
    pub const S_IRUSR: u32 = 0o0400;  // User read
    pub const S_IWUSR: u32 = 0o0200;  // User write
    pub const S_IXUSR: u32 = 0o0100;  // User execute
    pub const S_IRWXG: u32 = 0o0070;  // Group permissions
    pub const S_IRGRP: u32 = 0o0040;  // Group read
    pub const S_IWGRP: u32 = 0o0020;  // Group write
    pub const S_IXGRP: u32 = 0o0010;  // Group execute
    pub const S_IRWXO: u32 = 0o0007;  // Others permissions
    pub const S_IROTH: u32 = 0o0004;  // Others read
    pub const S_IWOTH: u32 = 0o0002;  // Others write
    pub const S_IXOTH: u32 = 0o0001;  // Others execute

    pub fn new(mode: u32) -> Self {
        Self(mode)
    }

    pub fn is_regular_file(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFREG
    }

    pub fn is_directory(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFDIR
    }

    pub fn is_char_device(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFCHR
    }

    pub fn is_block_device(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFBLK
    }

    pub fn is_fifo(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFIFO
    }

    pub fn is_symlink(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFLNK
    }

    pub fn is_socket(&self) -> bool {
        (self.0 & Self::S_IFMT) == Self::S_IFSOCK
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Inode operation function pointer table
///
#[repr(C)]
pub struct INodeOps {
    /// Create new node
    pub mkdir: Option<unsafe fn(&mut Inode, &[u8]) -> i32>,
    /// Lookup node
    pub lookup: Option<unsafe fn(&mut Inode, &[u8]) -> Option<*mut Inode>>,
    /// Create link
    pub link: Option<unsafe fn(&mut Inode, &mut Inode, &[u8]) -> i32>,
    /// Remove link
    pub unlink: Option<unsafe fn(&mut Inode, &[u8]) -> i32>,
    /// Create symbolic link
    pub symlink: Option<unsafe fn(&mut Inode, &[u8], &[u8]) -> i32>,
    /// Create directory
    pub mkdir2: Option<unsafe fn(&mut Inode, &[u8], InodeMode) -> i32>,
    /// Remove directory
    pub rmdir: Option<unsafe fn(&mut Inode, &[u8]) -> i32>,
    /// Rename
    pub rename: Option<unsafe fn(&mut Inode, &mut Inode, &[u8], &[u8]) -> i32>,
    /// Read link
    pub readlink: Option<unsafe fn(&mut Inode, &mut [u8]) -> isize>,
}

/// Inode state
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum InodeState {
    /// Newly allocated inode
    INew = 0,
    /// Inode exists
    IExisting = 1,
    /// Inode being deleted
    IDying = 2,
}

/// Index node
///
#[repr(C)]
pub struct Inode {
    /// Inode number
    pub ino: Ino,
    /// Inode mode (file type and permissions)
    pub mode: InodeMode,
    /// File size
    pub size: AtomicU64,
    /// Device number
    pub rdev: u64,
    /// Inode state
    pub state: Mutex<InodeState>,
    /// Inode operations
    pub ops: Option<&'static INodeOps>,
    /// Private data
    pub private_data: Option<*mut u8>,
    /// File data (used for regular files)
    pub data: Mutex<Option<FileBuffer>>,
    /// Reference count
    ref_count: AtomicU64,
}

unsafe impl Send for Inode {}
unsafe impl Sync for Inode {}

impl Inode {
    /// Create new inode
    pub fn new(ino: Ino, mode: InodeMode) -> Self {
        Self {
            ino,
            mode,
            size: AtomicU64::new(0),
            rdev: 0,
            state: Mutex::new(InodeState::INew),
            ops: None,
            private_data: None,
            data: Mutex::new(None),
            ref_count: AtomicU64::new(1),
        }
    }

    /// Read file data
    pub fn read_data(&self, offset: usize, buf: &mut [u8]) -> usize {
        if let Some(ref data) = *self.data.lock() {
            data.read(offset, buf)
        } else {
            0
        }
    }

    /// Write file data
    pub fn write_data(&self, offset: usize, buf: &[u8]) -> usize {
        let mut data_guard = self.data.lock();
        if data_guard.is_none() {
            *data_guard = Some(FileBuffer::new());
        }
        if let Some(ref mut data) = *data_guard {
            let written = data.write(offset, buf);
            // Update file size
            let new_size = data.len() as u64;
            self.size.store(new_size, Ordering::Release);
            written
        } else {
            0
        }
    }

    /// Load file content from bytes
    pub fn load_from_bytes(&self, bytes: &[u8]) {
        let mut data_guard = self.data.lock();
        *data_guard = Some(FileBuffer::from_bytes(bytes));
        self.size.store(bytes.len() as u64, Ordering::Release);
    }

    /// Set inode operations
    pub fn set_ops(&mut self, ops: &'static INodeOps) {
        self.ops = Some(ops);
    }

    /// Set private data
    pub fn set_private_data(&mut self, data: *mut u8) {
        self.private_data = Some(data);
    }

    /// Get file size
    pub fn get_size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    /// Set file size
    pub fn set_size(&self, size: u64) {
        self.size.store(size, Ordering::Release);
    }

    /// Increment reference count
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn dec_ref(&self) -> u64 {
        self.ref_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// Get reference count
    pub fn get_ref(&self) -> u64 {
        self.ref_count.load(Ordering::Acquire)
    }
}

/// Create character device inode
pub fn make_char_inode(ino: Ino, rdev: u64) -> Inode {
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFCHR | 0o666));
    inode.rdev = rdev;
    inode
}

/// Create regular file inode
pub fn make_reg_inode(ino: Ino, size: u64) -> Inode {
    let inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o666));
    inode.set_size(size);
    inode
}

/// Create regular file inode with data
pub fn make_reg_inode_with_data(ino: Ino, data: &[u8]) -> Inode {
    let inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o666));
    inode.load_from_bytes(data);
    inode
}

/// Create directory inode
pub fn make_dir_inode(ino: Ino) -> Inode {
    Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o755))
}

/// Create FIFO inode
pub fn make_fifo_inode(ino: Ino) -> Inode {
    Inode::new(ino, InodeMode::new(InodeMode::S_IFIFO | 0o666))
}

// ============================================================================
// Inode cache (icache)
// ============================================================================

/// Inode cache size - from config
const ICACHE_SIZE: usize = crate::config::ICACHE_SIZE;

/// Inode cache statistics
#[derive(Debug)]
pub struct InodeCacheStats {
    /// Cache hit count
    pub hits: AtomicU64,
    /// Cache miss count
    pub misses: AtomicU64,
    /// Eviction count
    pub evictions: AtomicU64,
}

impl InodeCacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            (hits as f64) / (total as f64)
        }
    }
}

/// Hash table bucket
struct InodeHashBucket {
    /// inode pointer
    inode: Option<Arc<Inode>>,
    /// inode number (for quick comparison)
    ino: Ino,
    /// LRU timestamp (for eviction)
    access_time: AtomicU64,
}

impl Clone for InodeHashBucket {
    fn clone(&self) -> Self {
        Self {
            inode: self.inode.clone(),
            ino: self.ino,
            access_time: AtomicU64::new(self.access_time.load(Ordering::Relaxed)),
        }
    }
}

/// Inode hash table
struct InodeCache {
    /// Hash table
    buckets: [InodeHashBucket; ICACHE_SIZE],
    /// Number of entries in cache
    count: usize,
    /// Global timestamp (for LRU)
    global_time: AtomicU64,
    /// Statistics
    stats: InodeCacheStats,
}

unsafe impl Send for InodeCache {}
unsafe impl Sync for InodeCache {}

/// Global Inode cache
static ICACHE: spin::Mutex<Option<InodeCache>> = spin::Mutex::new(None);

/// Initialize Inode cache
fn icache_init() {
    let mut cache = ICACHE.lock();
    if cache.is_some() {
        return;  // Already initialized
    }

    // Create empty bucket array
    let buckets: [InodeHashBucket; ICACHE_SIZE] = core::array::from_fn(|_| InodeHashBucket {
        inode: None,
        ino: 0,
        access_time: AtomicU64::new(0),
    });

    *cache = Some(InodeCache {
        buckets,
        count: 0,
        global_time: AtomicU64::new(1),
        stats: InodeCacheStats::new(),
    });
}

/// Calculate hash value
///
/// Uses simple FNV-1a hash algorithm
fn inode_hash(ino: Ino) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis

    // Mix inode number
    hash ^= ino;
    hash = hash.wrapping_mul(0x100000001b3);

    hash
}

/// Lookup in Inode cache
///
pub fn icache_lookup(ino: Ino) -> Option<Arc<Inode>> {
    // Ensure cache is initialized
    icache_init();

    let mut cache = ICACHE.lock();
    let cache_inner = cache.as_mut()?;

    // Calculate hash value
    let hash = inode_hash(ino);
    let index = (hash as usize) % ICACHE_SIZE;

    // Find matching entry
    let bucket = &cache_inner.buckets[index];

    if let Some(ref inode) = bucket.inode {
        // Compare inode number
        if bucket.ino == ino {
            // Update access time (for LRU)
            let current_time = cache_inner.global_time.fetch_add(1, Ordering::Relaxed);
            bucket.access_time.store(current_time, Ordering::Relaxed);

            // Record hit
            cache_inner.stats.record_hit();

            return Some(inode.clone());
        }
    }

    // Record miss
    cache_inner.stats.record_miss();

    None
}

/// Add Inode to cache
///
pub fn icache_add(inode: Arc<Inode>) {
    // Ensure cache is initialized
    icache_init();

    // Get inode number (outside cache lock)
    let ino = inode.ino;

    // Calculate hash value
    let hash = inode_hash(ino);

    let mut cache = ICACHE.lock();
    let inner = cache.as_mut().expect("icache not initialized");

    let index = (hash as usize) % ICACHE_SIZE;

    // Check if already exists
    if let Some(ref _existing) = inner.buckets[index].inode {
        if inner.buckets[index].ino == ino {
            return;  // Already in cache
        }

        // Use LRU policy: find and evict least recently used entry
        icache_evict_lru(inner);
    }

    // Get current timestamp
    let current_time = inner.global_time.fetch_add(1, Ordering::Relaxed);

    // Add to cache
    inner.buckets[index] = InodeHashBucket {
        inode: Some(inode.clone()),
        ino,
        access_time: AtomicU64::new(current_time),
    };
    inner.count += 1;
}

/// LRU eviction policy: evict least recently used entry
///
fn icache_evict_lru(cache: &mut InodeCache) {
    // Find least recently used entry (minimum access time)
    let mut lru_index = 0;
    let mut lru_time = u64::MAX;
    let mut found = false;

    for (i, bucket) in cache.buckets.iter().enumerate() {
        if bucket.inode.is_some() {
            let access_time = bucket.access_time.load(Ordering::Relaxed);
            if access_time < lru_time {
                lru_time = access_time;
                lru_index = i;
                found = true;
            }
        }
    }

    // Evict LRU entry
    if found {
        cache.buckets[lru_index].inode = None;
        cache.buckets[lru_index].ino = 0;
        cache.buckets[lru_index].access_time.store(0, Ordering::Relaxed);
        cache.count -= 1;

        // Record eviction
        cache.stats.record_eviction();
    }
}

/// Remove from Inode cache
///
pub fn icache_remove(ino: Ino) {
    // Ensure cache is initialized
    icache_init();

    let mut cache = ICACHE.lock();
    let inner = cache.as_mut().expect("icache not initialized");

    // Calculate hash value
    let hash = inode_hash(ino);
    let index = (hash as usize) % ICACHE_SIZE;

    // Remove entry
    if let Some(ref _inode) = inner.buckets[index].inode {
        if inner.buckets[index].ino == ino {
            // Remove from cache
            inner.buckets[index].inode = None;
            inner.buckets[index].ino = 0;
            inner.buckets[index].access_time.store(0, Ordering::Relaxed);
            inner.count -= 1;
        }
    }
}

/// Get cache statistics
pub fn icache_stats() -> (usize, usize) {
    // Ensure cache is initialized
    icache_init();

    let cache = ICACHE.lock();
    let cache_inner = cache.as_ref().expect("icache not initialized");

    (cache_inner.count, ICACHE_SIZE)
}

/// Get detailed cache statistics
pub fn icache_stats_detailed() -> (u64, u64, u64, f64) {
    // Ensure cache is initialized
    icache_init();

    let cache = ICACHE.lock();
    let cache_inner = cache.as_ref().expect("icache not initialized");

    (
        cache_inner.stats.hits.load(Ordering::Relaxed),
        cache_inner.stats.misses.load(Ordering::Relaxed),
        cache_inner.stats.evictions.load(Ordering::Relaxed),
        cache_inner.stats.get_hit_rate(),
    )
}

/// Clear Inode cache
///
pub fn icache_flush() {
    // Ensure cache is initialized
    icache_init();

    let mut cache = ICACHE.lock();
    let inner = cache.as_mut().expect("icache not initialized");

    // Clear all buckets
    for bucket in inner.buckets.iter_mut() {
        bucket.inode = None;
        bucket.ino = 0;
        bucket.access_time.store(0, Ordering::Relaxed);
    }

    inner.count = 0;
}
