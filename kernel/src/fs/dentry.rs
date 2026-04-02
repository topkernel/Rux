//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Directory Entry (Dentry) Management
//!
//!
//! Core concepts:
//! - `struct dentry`: Directory entry, representing an entry in a directory
//! - `dcache`: Directory entry cache, speeds up path lookup
//! - `LRU`: Least Recently Used eviction policy

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::borrow::ToOwned;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::fs::inode::Inode;
use crate::fs::mount::MntFlags;

/// Dentry state flags
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DentryFlags(u32);

impl DentryFlags {
    /// Directory entry not connected to dcache
    pub const DCACHE_UNHASHED: u32 = 0x00000001;
    /// Directory entry connected to dcache
    pub const DCACHE_HASHED: u32 = 0x00000002;
    /// Directory entry in use
    pub const DCACHE_REFERENCED: u32 = 0x00000010;
    /// Directory entry deleted
    pub const DCACHE_DENTRY_KILL: u32 = 0x00000040;

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_hashed(&self) -> bool {
        (self.0 & Self::DCACHE_HASHED) != 0
    }

    pub fn is_unhashed(&self) -> bool {
        (self.0 & Self::DCACHE_UNHASHED) != 0
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Dentry state
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DentryState {
    /// Dentry unused
    DUnhashed,
    /// Dentry used
    DHashed,
    /// Dentry being deleted
    DKill,
}

/// VFS mount descriptor — attached to a dentry that is a mount point.
///
/// When `follow_mount()` encounters a dentry with a `VfsMountInternal`,
/// it transparently switches to `root` (the mounted filesystem's root dentry).
pub struct VfsMountInternal {
    /// Root dentry of the mounted filesystem
    pub root: Arc<Dentry>,
    /// Mount flags
    pub flags: MntFlags,
}

unsafe impl Send for VfsMountInternal {}
unsafe impl Sync for VfsMountInternal {}

/// Directory entry
///
#[repr(C)]
pub struct Dentry {
    /// dentry name (last component, e.g. "null" not "/dev/null")
    pub name: Spinlock<String>,
    /// parent directory entry
    pub parent: Spinlock<Option<Arc<Dentry>>>,
    /// child dentries (name -> dentry mapping)
    pub children: Spinlock<BTreeMap<String, Arc<Dentry>>>,
    /// associated inode
    pub inode: Spinlock<Option<Arc<Inode>>>,
    /// If this dentry is a mount point, points to the mount descriptor.
    /// None means this dentry is not a mount point.
    pub vfsmount: Spinlock<Option<Arc<VfsMountInternal>>>,
    /// dentry state
    pub state: Spinlock<DentryState>,
    /// dentry flags
    pub flags: Spinlock<DentryFlags>,
    /// reference count
    ref_count: AtomicU64,
    /// Negative dentry: lookup found no file on disk
    pub negative: AtomicBool,
}

unsafe impl Send for Dentry {}
unsafe impl Sync for Dentry {}

impl Dentry {
    /// Create new dentry
    pub fn new(name: String) -> Self {
        Self {
            name: Spinlock::new(name),
            parent: Spinlock::new(None),
            children: Spinlock::new(BTreeMap::new()),
            inode: Spinlock::new(None),
            vfsmount: Spinlock::new(None),
            state: Spinlock::new(DentryState::DUnhashed),
            flags: Spinlock::new(DentryFlags::new(DentryFlags::DCACHE_UNHASHED)),
            ref_count: AtomicU64::new(1),
            negative: AtomicBool::new(false),
        }
    }

    /// Set parent directory entry
    pub fn set_parent(&self, parent: Arc<Dentry>) {
        *self.parent.lock() = Some(parent);
    }

    /// Set inode
    pub fn set_inode(&self, inode: Arc<Inode>) {
        *self.inode.lock() = Some(inode);
    }

    /// Get inode
    pub fn get_inode(&self) -> Option<Arc<Inode>> {
        self.inode.lock().clone()
    }

    /// Get name
    pub fn get_name(&self) -> String {
        self.name.lock().clone()
    }

    /// Look up a child dentry by name. Returns None if not found.
    pub fn lookup_child(&self, name: &str) -> Option<Arc<Dentry>> {
        self.children.lock().get(name).cloned()
    }

    /// Add a child dentry. If a child with the same name exists, it is replaced.
    pub fn add_child(&self, name: String, child: Arc<Dentry>) {
        self.children.lock().insert(name, child);
    }

    /// Remove a child dentry by name.
    pub fn remove_child(&self, name: &str) {
        self.children.lock().remove(name);
    }

    /// Set the mount descriptor (marks this dentry as a mount point).
    pub fn set_mount(&self, mount: Arc<VfsMountInternal>) {
        *self.vfsmount.lock() = Some(mount);
    }

    /// Get the mount descriptor (if this is a mount point).
    pub fn get_mount(&self) -> Option<Arc<VfsMountInternal>> {
        self.vfsmount.lock().clone()
    }

    /// Set to hashed state
    pub fn set_hashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_HASHED);
        *self.state.lock() = DentryState::DHashed;
    }

    /// Set to unhashed state
    pub fn set_unhashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_UNHASHED);
        *self.state.lock() = DentryState::DUnhashed;
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

    /// Mark as negative dentry (lookup found no file)
    pub fn set_negative(&self) {
        self.negative.store(true, Ordering::Release);
    }

    /// Check if this is a negative dentry
    pub fn is_negative(&self) -> bool {
        self.negative.load(Ordering::Acquire)
    }

    /// Clear negative flag (e.g. after file creation)
    pub fn clear_negative(&self) {
        self.negative.store(false, Ordering::Release);
    }
}

/// Create root directory entry
pub fn make_root_dentry() -> Option<Arc<Dentry>> {
    let dentry = Arc::new(Dentry::new("/".to_owned()));
    // Note: Arc returns &T when dereferenced
    // For now, we'll return the Arc directly - the caller can call set_hashed if needed
    Some(dentry)
}

// ============================================================================
// Dentry cache (dcache)
// ============================================================================

/// Dentry cache size - from config
const DCACHE_SIZE: usize = crate::config::DCACHE_SIZE;

/// Dentry cache statistics
#[derive(Debug)]
pub struct DentryCacheStats {
    /// Cache hit count
    pub hits: AtomicU64,
    /// Cache miss count
    pub misses: AtomicU64,
    /// Eviction count
    pub evictions: AtomicU64,
}

impl DentryCacheStats {
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
struct DentryHashBucket {
    /// dentry pointer
    dentry: Option<Arc<Dentry>>,
    /// hash key (for quick comparison)
    key: u64,
    /// LRU timestamp (for eviction)
    access_time: AtomicU64,
}

impl Clone for DentryHashBucket {
    fn clone(&self) -> Self {
        Self {
            dentry: self.dentry.clone(),
            key: self.key,
            access_time: AtomicU64::new(self.access_time.load(Ordering::Relaxed)),
        }
    }
}

/// Dentry hash table
struct DentryCache {
    /// Hash table (heap-allocated to avoid large stack allocation)
    buckets: alloc::boxed::Box<[DentryHashBucket]>,
    /// Number of entries in cache
    count: usize,
    /// Global timestamp (for LRU)
    global_time: AtomicU64,
    /// Statistics
    stats: DentryCacheStats,
}

unsafe impl Send for DentryCache {}
unsafe impl Sync for DentryCache {}

/// Global Dentry cache
static DCACHE: Spinlock<Option<DentryCache>> = Spinlock::new(None);

/// Initialize Dentry cache
fn dcache_init() {
    let mut cache = DCACHE.lock();
    if cache.is_some() {
        return;  // Already initialized
    }

    // Heap-allocate bucket array to avoid large stack allocation
    let buckets: alloc::vec::Vec<DentryHashBucket> = (0..DCACHE_SIZE)
        .map(|_| DentryHashBucket {
            dentry: None,
            key: 0,
            access_time: AtomicU64::new(0),
        })
        .collect();

    *cache = Some(DentryCache {
        buckets: buckets.into_boxed_slice(),
        count: 0,
        global_time: AtomicU64::new(1),
        stats: DentryCacheStats::new(),
    });
}

/// Calculate hash value
///
/// Uses simple FNV-1a hash algorithm
fn dentry_hash(name: &str, parent_ino: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis

    // Mix parent inode number
    hash ^= parent_ino;
    hash = hash.wrapping_mul(0x100000001b3);

    // Mix name
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
}

/// Look up in Dentry cache
///
pub fn dcache_lookup(name: &str, parent_ino: u64) -> Option<Arc<Dentry>> {
    // Ensure cache is initialized
    dcache_init();

    let mut cache = DCACHE.lock();
    let cache_inner = cache.as_mut()?;

    // Calculate hash value
    let hash = dentry_hash(name, parent_ino);
    let index = (hash as usize) % DCACHE_SIZE;

    // Find matching entry
    let bucket = &cache_inner.buckets[index];

    if let Some(ref dentry) = bucket.dentry {
        // Compare hash key
        if bucket.key == hash {
            // Compare name
            if dentry.name.lock().as_str() == name {
                // Update access time (for LRU)
                let current_time = cache_inner.global_time.fetch_add(1, Ordering::Relaxed);
                bucket.access_time.store(current_time, Ordering::Relaxed);

                // Record hit
                cache_inner.stats.record_hit();

                return Some(dentry.clone());
            }
        }
    }

    // Record miss
    cache_inner.stats.record_miss();

    None
}

/// Add Dentry to cache
///
pub fn dcache_add(dentry: Arc<Dentry>, parent_ino: u64) {
    // Ensure cache is initialized
    dcache_init();

    // Calculate hash value (outside cache lock)
    let name = dentry.name.lock();
    let name_str = name.clone();
    drop(name);  // Release name lock
    let hash = dentry_hash(&name_str, parent_ino);

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    let index = (hash as usize) % DCACHE_SIZE;

    // Check if already exists
    if let Some(ref _existing) = inner.buckets[index].dentry {
        if inner.buckets[index].key == hash {
            return;  // Already in cache
        }

        // Use LRU policy: find and evict least recently used entry
        dcache_evict_lru(inner);
    }

    // Get current timestamp
    let current_time = inner.global_time.fetch_add(1, Ordering::Relaxed);

    // Add to cache
    inner.buckets[index] = DentryHashBucket {
        dentry: Some(dentry.clone()),
        key: hash,
        access_time: AtomicU64::new(current_time),
    };
    inner.count += 1;

    // Mark as hashed (outside cache lock)
    drop(cache);  // Release cache lock
    dentry.set_hashed();
}

/// LRU eviction policy: evict least recently used entry
///
fn dcache_evict_lru(cache: &mut DentryCache) {
    // Find least recently used entry (minimum access time)
    let mut lru_index = 0;
    let mut lru_time = u64::MAX;
    let mut found = false;

    for (i, bucket) in cache.buckets.iter().enumerate() {
        if bucket.dentry.is_some() {
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
        if let Some(ref dentry) = cache.buckets[lru_index].dentry {
            // Mark as unhashed
            dentry.set_unhashed();
        }

        cache.buckets[lru_index].dentry = None;
        cache.buckets[lru_index].key = 0;
        cache.buckets[lru_index].access_time.store(0, Ordering::Relaxed);
        cache.count -= 1;

        // Record eviction
        cache.stats.record_eviction();
    }
}

/// Remove from Dentry cache
///
pub fn dcache_remove(name: &str, parent_ino: u64) {
    // Ensure cache is initialized
    dcache_init();

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    // Calculate hash value
    let hash = dentry_hash(name, parent_ino);
    let index = (hash as usize) % DCACHE_SIZE;

    // Remove entry
    if let Some(ref dentry) = inner.buckets[index].dentry {
        if inner.buckets[index].key == hash {
            // Mark as unhashed
            dentry.set_unhashed();

            // Remove from cache
            inner.buckets[index].dentry = None;
            inner.buckets[index].key = 0;
            inner.buckets[index].access_time.store(0, Ordering::Relaxed);
            inner.count -= 1;
        }
    }
}

/// Get cache statistics
pub fn dcache_stats() -> (usize, usize) {
    // Ensure cache is initialized
    dcache_init();

    let cache = DCACHE.lock();
    let cache_inner = cache.as_ref().expect("dcache not initialized");

    (cache_inner.count, DCACHE_SIZE)
}

/// Get detailed cache statistics
pub fn dcache_stats_detailed() -> (u64, u64, u64, f64) {
    // Ensure cache is initialized
    dcache_init();

    let cache = DCACHE.lock();
    let cache_inner = cache.as_ref().expect("dcache not initialized");

    (
        cache_inner.stats.hits.load(Ordering::Relaxed),
        cache_inner.stats.misses.load(Ordering::Relaxed),
        cache_inner.stats.evictions.load(Ordering::Relaxed),
        cache_inner.stats.get_hit_rate(),
    )
}

/// Clear Dentry cache
///
pub fn dcache_flush() {
    // Ensure cache is initialized
    dcache_init();

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    // Clear all buckets
    for bucket in inner.buckets.iter_mut() {
        if let Some(ref dentry) = bucket.dentry {
            // Mark as unhashed
            dentry.set_unhashed();
        }
        bucket.dentry = None;
        bucket.key = 0;
        bucket.access_time.store(0, Ordering::Relaxed);
    }

    inner.count = 0;
}
