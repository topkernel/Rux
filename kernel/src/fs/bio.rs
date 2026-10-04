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

// SAFETY: BufferHead's mutable state (b_data) is protected by BufferState spinlock;
// other fields are atomic or only accessed under lock.
unsafe impl Send for BufferHead {}
// SAFETY: all shared mutable state is protected by the BufferState spinlock;
// b_data is only mutated under lock or from a single thread.
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

    /// Create a buffer head with an UNINITIALIZED data buffer.
    ///
    /// Only for buffers whose every byte is about to be overwritten by the
    /// device (async read DMA target): skips the 4 KiB zero-fill of `new`,
    /// which was pure waste on the read-ahead path (one memset per block
    /// under TCG). Readers must observe BH_Uptodate (bread_wait status 0)
    /// before touching b_data — the same discipline the in-flight BH_Req
    /// state already requires.
    pub fn new_uninit(blocknr: u64, size: u32) -> Self {
        let mut b_data = Vec::with_capacity(size as usize);
        // SAFETY: the allocation holds `size` initialized bytes for the Vec's
        // purposes; no read of b_data happens before the DMA completes and
        // BH_Uptodate is set (documented contract above). The capacity is
        // exactly `size`, so set_len stays in bounds.
        unsafe { b_data.set_len(size as usize); }
        Self {
            b_device: None,
            b_blocknr: blocknr,
            b_size: size,
            b_state: Spinlock::new(BufferState::new()),
            b_data,
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
            // SAFETY: self.b_device is a valid GenDisk pointer set by set_device();
            // b_data is a valid buffer of b_size bytes; blkdev_write handles the I/O.
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
    /// Key: (device_major, device_minor, blocknr) for fast lookup.
    /// Minor included (review 5.3 low): two partitions of the same disk
    /// share a major number — without the minor, block N of partition A
    /// was served as block N of partition B.
    key: (u32, u32, u64),
    /// Next entry in hash chain
    hash_next: Option<*mut CacheEntry>,
    /// Previous entry in LRU list (more recent)
    lru_prev: Option<*mut CacheEntry>,
    /// Next entry in LRU list (less recent)
    lru_next: Option<*mut CacheEntry>,
    /// Eviction in progress (set under the bucket lock before the entry
    /// leaves the hash chain) — get() must skip such entries so no pin can
    /// arrive while the victim is being unlinked and freed (R7-C1).
    evicting: bool,
}

impl CacheEntry {
    fn new(bh: Box<BufferHead>, device_major: u32, device_minor: u32, blocknr: u64) -> Self {
        Self {
            bh: Box::into_raw(bh),
            key: (device_major, device_minor, blocknr),
            hash_next: None,
            lru_prev: None,
            lru_next: None,
            evicting: false,
        }
    }
}

impl Drop for CacheEntry {
    fn drop(&mut self) {
        if !self.bh.is_null() {
            // SAFETY: self.bh was created by Box::into_raw in CacheEntry::new;
            // Drop is called once and bh is non-null (checked above), so this reclaims the Box.
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
/// # Lock hierarchy (MUST be followed to prevent deadlock)
///
/// 1. **Bucket lock** → 2. **LRU lock** (when both needed, always acquire
///    bucket lock first). LRU lock may be acquired alone (e.g. in evict_one).
/// 3. **BufferState lock** — per-buffer state, nested inside bucket lock.
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

// SAFETY: BlockCache is only accessed via &self methods that use internal locking
// (per-bucket Spinlock and LRU Spinlock) and atomic count; raw pointers are
// confined to cache internals and never exposed without locks.
unsafe impl Send for BlockCache {}
// SAFETY: all shared mutable state (hash buckets, LRU list) is protected by
// Spinlocks; raw pointer access is always under the appropriate bucket lock.
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

    /// Acquire LRU lock while already holding a bucket lock.
    ///
    /// # Safety
    /// Caller MUST hold the bucket lock for `bucket_idx` (or any bucket).
    /// This enforces the bucket→LRU lock ordering documented on BlockCache.
    #[inline]
    unsafe fn lru_lock_under_bucket(&self) -> crate::sync::spinlock::SpinlockGuard<'_, LruState> {
        // SAFETY: caller guarantees bucket lock is held (lock hierarchy).
        self.lru.lock()
    }

    #[inline]
    fn hash_index(&self, device_major: u32, device_minor: u32, blocknr: u64) -> usize {
        let hash = (device_major as u64)
            .wrapping_mul(2654435761)
            .wrapping_add((device_minor as u64) << 32)
            .wrapping_add(blocknr);
        (hash as usize) & (self.hash_size - 1)
    }

    /// Move entry to LRU head. Caller must hold the lru lock.
    // SAFETY: VFS callback contract; pointers are valid for the scope of this block
    unsafe fn move_to_lru_head(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        let entry = &mut *entry_ptr;

        // R7-C1: an entry unlinked by evict_one Phase 1 (both links None)
        // but still visible in the hash chain can race here via get().
        // Unlinking it again would drop lru.tail (lru_next None -> tail =
        // lru_prev = None) and leave the real tail orphaned — instead treat
        // it as a fresh insert (same as push_lru_head).
        if entry.lru_prev.is_none() && entry.lru_next.is_none() && lru.head != Some(entry_ptr) {
            Self::push_lru_head(lru, entry_ptr);
            return;
        }
        if lru.head == Some(entry_ptr) {
            return; // already at head
        }

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

    /// Push an entry that is currently UNLINKED onto the LRU head.
    /// Caller must hold the lru lock.
    unsafe fn push_lru_head(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        // R7-C1: idempotent — evict_one Phase 2 re-inserts after a pin
        // recheck; a concurrent get() may have re-linked the entry at head
        // already. Pushing again would self-loop (entry.lru_next = head =
        // entry itself).
        if lru.head == Some(entry_ptr) {
            return;
        }
        let entry = &mut *entry_ptr;
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
    // SAFETY: VFS callback contract; pointers are valid for the scope of this block
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
        // R7-C1 restructuring (supersedes the VFS-H13 two-phase fix, which
        // still raced: a get() landing between the LRU unlink and the
        // bucket lock re-inserted/pinned the victim and Phase 2 then freed
        // a buffer the caller held — or, with the re-insert guard, freed an
        // entry still linked in the LRU). New order:
        //   Phase 1 (LRU lock): PICK a victim (count==0, not already
        //     evicting) but do not unlink anything.
        //   Phase 2 (bucket lock): re-verify count==0, set `evicting`,
        //     unlink from the hash chain. Once the flag is set under this
        //     lock, get() (which takes the same lock) can no longer pin
        //     the entry; with count==0 there are no existing holders.
        //   Phase 3 (LRU lock): unlink from the LRU.
        //   Phase 4 (no locks): sync if dirty, free.
        let victim = {
            let mut lru = self.lru.lock_irqsave();
            let mut current = lru.tail;
            let mut found = None;
            while let Some(entry_ptr) = current {
                // SAFETY: entry_ptr is a valid raw pointer from the LRU list;
                // all CacheEntry pointers in the LRU were created by Box::into_raw
                // and remain valid while in the cache.
                unsafe {
                    let entry = &*entry_ptr;
                    if !entry.evicting && !entry.bh.is_null() && unsafe { (*entry.bh).count() == 0 } {
                        found = Some(entry_ptr);
                        break;
                    }
                    current = entry.lru_prev;
                }
            }
            match found {
                Some(entry_ptr) => entry_ptr,
                None => return false, // all buffers in use (or mid-eviction)
            }
        };
        // lru lock released; victim still fully linked — pinnable ONLY
        // until Phase 2 takes the bucket lock, and the count recheck there
        // aborts the eviction if a pin arrived in this window.

        let victim_key = unsafe { (*victim).key };
        let bucket_idx = self.hash_index(victim_key.0, victim_key.1, victim_key.2);

        unsafe {
            let mut bucket = self.buckets[bucket_idx].lock();
            // R11-1 (S2 root cause): PRESENCE IS AUTHORITATIVE. Every free
            // path unlinks from the hash under this same bucket lock, so
            // "found in chain" is the proof the victim is still alive. The
            // previous order read count/evicting FIRST — a CPU delayed past
            // another CPU's full eviction (unhash + free + heap reuse)
            // then checked flags on freed memory, fell through the walk
            // that no longer found the victim (no abort!), wrote
            // remove_from_lru into the reused block, CAS'd b_state on
            // NULL+0x30 (the observed kernel panic) and double-freed.
            let mut prev: Option<*mut CacheEntry> = None;
            let mut current = bucket.head;
            let mut present = false;
            while let Some(cp) = current {
                if cp == victim {
                    if let Some(pp) = prev {
                        (*pp).hash_next = (*cp).hash_next;
                    } else {
                        bucket.head = (*cp).hash_next;
                    }
                    present = true;
                    break;
                }
                prev = Some(cp);
                current = (*cp).hash_next;
            }
            if !present {
                // Another CPU already evicted (and possibly freed) this
                // victim — do not touch it again.
                return false;
            }
            // Alive (presence under the lock): now the pin/flag rechecks
            // are reads of valid memory.
            if (*(*victim).bh).count() != 0 {
                // Pinned between Phase 1 and Phase 2 — but we already
                // unlinked it! Re-link at the head and abort.
                (*victim).hash_next = bucket.head;
                bucket.head = Some(victim);
                return false;
            }
            (*victim).evicting = true;
        }
        // bucket lock released; entry is unpinnable and unhashed

        {
            let mut lru = self.lru.lock_irqsave();
            // SAFETY: victim is a valid CacheEntry pointer picked from the
            // LRU list in Phase 1; the LRU lock is held.
            unsafe { Self::remove_from_lru(&mut lru, victim); }
        }

        // Phase 4: Sync if dirty (NO locks held — I/O is safe)
        // SAFETY: victim is unhashed, unlinked and unpinnable; its bh field
        // points to a valid BufferHead owned by this entry.
        unsafe {
            if (*(*victim).bh).is_dirty() {
                let _ = (*(*victim).bh).sync();
            }
            let _ = Box::from_raw(victim);
        }
        self.count.fetch_sub(1, Ordering::Release);
        true
    }

    /// Get or create buffer (synchronous read on cache miss).
    fn get(&self, device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
        // SAFETY: device is a valid GenDisk pointer passed from the block device layer;
        // all CacheEntry pointers in the hash chain were created by Box::into_raw.
        unsafe {
            let (device_major, device_minor) = ((*device).major, (*device).first_minor);
            let index = self.hash_index(device_major, device_minor, blocknr);

            // Phase 1: Lookup under bucket lock
            {
                let mut bucket = self.buckets[index].lock();
                let mut prev: Option<*mut CacheEntry> = None;
                let mut current = bucket.head;

                while let Some(entry_ptr) = current {
                    let entry = &*entry_ptr;
                    if entry.evicting {
                        // Being evicted: treat as not found — pinning it
                        // would hand out a buffer that is about to be
                        // freed (R7-C1 resurrection race).
                        prev = Some(entry_ptr);
                        current = entry.hash_next;
                        continue;
                    }
                    if entry.key == (device_major, device_minor, blocknr) {
                        // R9 tripwire: hand out only intact buffers. A len-0
                        // b_data is freed-and-reused memory (BufferHead::new
                        // always allocates block_size bytes); serving it
                        // corrupted ext4 metadata handling (inode.rs:563
                        // slice panic). Report via SBI (safe under any lock)
                        // and treat as a miss so the caller re-reads.
                        if unsafe { entry.bh.is_null() || (*entry.bh).b_data.len() != self.block_size as usize } {
                            let msg = b"bio: dead bh in chain blk=";
                            unsafe {
                                for &b in msg { crate::console::putchar_no_lock(b); }
                                let mut v = blocknr;
                                let mut digs = [0u8; 20];
                                let mut n = 0;
                                if v == 0 { digs[0] = b'0'; n = 1; }
                                while v > 0 { digs[n] = b'0' + (v % 10) as u8; n += 1; v /= 10; }
                                while n > 0 { n -= 1; sbi_rt::legacy::console_putchar(digs[n] as usize); }
                                crate::console::putchar_no_lock(b'\n');
                            }
                            prev = Some(entry_ptr);
                            current = entry.hash_next;
                            continue;
                        }
                        // Found — move to hash chain head
                        if prev.is_some() {
                            let prev_entry = &mut *prev.unwrap();
                            prev_entry.hash_next = entry.hash_next;
                            (*entry_ptr).hash_next = bucket.head;
                            bucket.head = Some(entry_ptr);
                        }
                        // Move to LRU head (bucket lock held — enforces ordering)
                        let mut lru = unsafe { self.lru_lock_under_bucket() };
                        Self::move_to_lru_head(&mut lru, entry_ptr);
                        (*entry.bh).get();
                        return Some(entry.bh);
                    }
                    prev = Some(entry_ptr);
                    current = entry.hash_next;
                }
            }

            // Phase 1.5: Evict if cache is full (soft limit — wedge fix).
            // evict_one() SYNCS dirty victims: one synchronous block write
            // per eviction. Under a dirty-metadata storm (LTP inode02/
            // ftest* filling the disk) the old hard loop spent minutes
            // inside a single bread, all in kernel context — the test
            // runner's SIGKILL could never be delivered (the r2 WEDGE
            // family). Bound the reclaim work; a cache still full of
            // pinned/dirty entries admits the new buffer anyway (reads
            // must not fail on cache pressure — Linux grows its caches).
            let mut evict_tries = 0u32;
            while self.count.load(Ordering::Acquire) as usize >= self.max_entries {
                if !self.evict_one() || evict_tries >= 32 {
                    break;
                }
                evict_tries += 1;
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
            let entry = Box::new(CacheEntry::new(bh, device_major, device_minor, blocknr));
            let entry_ptr = Box::into_raw(entry);

            // Phase 3: Insert into cache
            {
                let mut bucket = self.buckets[index].lock();

                // Double-check for duplicate inserted by another thread
                let mut current = bucket.head;
                while let Some(cp) = current {
                    if (*cp).key == (device_major, device_minor, blocknr) {
                        // R9-8: apply the same integrity guard as Phase 1 —
                        // a dead (freed/reused) duplicate entry must not be
                        // handed out here after the fresh read.
                        if unsafe { (*cp).bh.is_null() || (*(*cp).bh).b_data.len() != self.block_size as usize } {
                            current = (*cp).hash_next;
                            continue;
                        }
                        (*(*cp).bh).get();
                        let mut lru = unsafe { self.lru_lock_under_bucket() };
                        Self::move_to_lru_head(&mut lru, cp);
                        let _ = Box::from_raw(entry_ptr);
                        return Some((*cp).bh);
                    }
                    current = (*cp).hash_next;
                }

                // Insert at hash chain head
                (*entry_ptr).hash_next = bucket.head;
                bucket.head = Some(entry_ptr);

                // Insert at LRU head (bucket lock held — enforces ordering)
                let mut lru = unsafe { self.lru_lock_under_bucket() };
                Self::move_to_lru_head(&mut lru, entry_ptr);
            }

            self.count.fetch_add(1, Ordering::Release);
            Some((*entry_ptr).bh)
        }
    }

    /// getblk-style fetch for FULL-BLOCK overwrites: cache hit returns the
    /// buffer; a miss creates it ZEROED, marked Uptodate+Dirty, WITHOUT the
    /// disk read a bread() would do. Only for blocks whose existing disk
    /// contents are irrelevant (freshly allocated blocks being zeroed) —
    /// reading through this API would fabricate zeros for real data.
    fn get_zero(&self, device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
        // SAFETY: same entry-lifetime guarantees as get(); the BufferHead is
        // either found in the cache (pinned via get()) or freshly created and
        // inserted below, never freed while the caller holds a reference.
        unsafe {
            let (device_major, device_minor) = ((*device).major, (*device).first_minor);
            let index = self.hash_index(device_major, device_minor, blocknr);

            // Phase 1: lookup under bucket lock (hit path identical to get)
            {
                let mut bucket = self.buckets[index].lock();
                let mut current = bucket.head;
                while let Some(entry_ptr) = current {
                    let entry = &*entry_ptr;
                    current = entry.hash_next;
                    if entry.evicting {
                        continue;
                    }
                    if entry.key == (device_major, device_minor, blocknr) {
                        if entry.bh.is_null()
                            || (*entry.bh).b_data.len() != self.block_size as usize
                        {
                            continue; // dead entry — treat as miss
                        }
                        (*entry.bh).get();
                        return Some(entry.bh);
                    }
                }
            }

            // Phase 1.5: evict if full (soft limit — see the read-path
            // Phase 1.5 comment; bounded reclaim, then admit).
            let mut evict_tries = 0u32;
            while self.count.load(Ordering::Acquire) as usize >= self.max_entries {
                if !self.evict_one() || evict_tries >= 32 {
                    break;
                }
                evict_tries += 1;
            }

            // Phase 2: create the buffer zeroed — NO disk read.
            let mut bh = Box::new(BufferHead::new(blocknr, self.block_size));
            bh.set_device(device);
            bh.set_state_bit(BufferState::BH_Uptodate);
            bh.set_state_bit(BufferState::BH_Dirty);

            let entry = Box::new(CacheEntry::new(bh, device_major, device_minor, blocknr));
            let entry_ptr = Box::into_raw(entry);

            // Phase 3: insert (same duplicate double-check as get)
            {
                let mut bucket = self.buckets[index].lock();
                let mut current = bucket.head;
                while let Some(cp) = current {
                    if (*cp).key == (device_major, device_minor, blocknr) {
                        if (*cp).bh.is_null()
                            || (*(*cp).bh).b_data.len() != self.block_size as usize
                        {
                            current = (*cp).hash_next;
                            continue;
                        }
                        (*(*cp).bh).get();
                        let _ = Box::from_raw(entry_ptr);
                        return Some((*cp).bh);
                    }
                    current = (*cp).hash_next;
                }
                (*entry_ptr).hash_next = bucket.head;
                bucket.head = Some(entry_ptr);
                let mut lru = self.lru_lock_under_bucket();
                Self::move_to_lru_head(&mut lru, entry_ptr);
            }

            self.count.fetch_add(1, Ordering::Release);
            Some((*entry_ptr).bh)
        }
    }

    /// Release buffer (decrement refcount)
    fn put(&self, bh: *const BufferHead) {
        // SAFETY: bh is a raw pointer returned by get()/bread(); it points to a
        // valid BufferHead owned by a CacheEntry in the cache.
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

        // No need to disable interrupts globally: no interrupt handler (timer,
        // virtio, softirq) ever acquires a bucket spinlock.  Per-bucket
        // lock() (preempt_disable only) is sufficient and avoids the SMP
        // deadlock where another CPU holds a bucket lock while we spin with
        // IRQs disabled.
        for i in 0..self.hash_size {
            let bucket = self.buckets[i].lock();
            let mut current = bucket.head;
            while let Some(entry_ptr) = current {
                // SAFETY: entry_ptr is from the hash chain (created by Box::into_raw);
                // bucket lock is held so the entry cannot be freed concurrently.
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
            // SAFETY: bh pointers were collected under bucket locks above and had
            // their refcount incremented; they remain valid BufferHead pointers.
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
                // SAFETY: entry_ptr is from the hash chain (created by Box::into_raw);
                // we hold the bucket lock and are draining the entire chain, so
                // Box::from_raw reclaims the CacheEntry and its BufferHead.
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
static CACHE_INIT_LOCK: Spinlock<()> = Spinlock::new(());

fn get_block_cache() -> &'static BlockCache {
    // Double-checked locking: fast path checks without lock,
    // slow path acquires lock then re-checks before initializing.
    if !CACHE_INIT.load(Ordering::Acquire) {
        let _guard = CACHE_INIT_LOCK.lock();
        if !CACHE_INIT.load(Ordering::Acquire) {
            let cache = BlockCache::new(64, 1024, 4096);
            // SAFETY: we hold the lock and CACHE_INIT is false, so no other
            // CPU can access BLOCK_CACHE concurrently.
            unsafe { BLOCK_CACHE = Some(cache); }
            // Release ordering ensures BLOCK_CACHE write is visible before
            // CACHE_INIT becomes true.
            CACHE_INIT.store(true, Ordering::Release);
        }
    }
    // SAFETY: CACHE_INIT is true, BLOCK_CACHE was written before the store.
    unsafe { BLOCK_CACHE.as_ref().unwrap_unchecked() }
}

/// Read a block from cache (or disk if not cached)
///
/// Read a block from cache (or disk if not cached)
pub fn bread(device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
    get_block_cache().get(device, blocknr)
}

/// Fetch a block for a full-block overwrite: no disk read on a miss — the
/// buffer is created zeroed and dirty (see BlockCache::get_zero). Callers
/// must overwrite the whole block (or rely on the zeros).
pub fn getblk_zero(device: *const blkdev::GenDisk, blocknr: u64) -> Option<*mut BufferHead> {
    get_block_cache().get_zero(device, blocknr)
}

/// Async block read: submit I/O without blocking, return buffer head immediately.
///
/// On cache hit: returns buffer with `BH_Uptodate` set (no I/O needed).
/// On cache miss: creates a new BufferHead, submits async I/O via
/// `blkdev::blkdev_read_async`, inserts into cache, and returns the buffer.
/// The buffer data is **not** valid until `bread_wait()` completes.
/// Wait for a buffer whose I/O was submitted by ANOTHER caller to settle.
/// BH_Req is cleared by the owning bread_wait(), so poll it with short
/// sleeps (1 jiffy each). The buffer cache keeps the entry pinned by the
/// owner's reference, so `bh` stays valid. In IRQ/early context (no
/// current task) spin instead of sleeping. Returns 0 when the buffer is
/// uptodate, -EIO otherwise (review 2R.11).
///
/// WEDGE fix (LTP inode02/ftest02/04/06, sendmsg02): this wait had NO
/// timeout. When a buffer's completion was lost (or its owner errored out
/// before its bread_wait), the calling task looped here forever — always
/// in kernel context, so the test runner's SIGKILL could never be
/// delivered, and each iteration churned an add_timer/del_timer heap
/// allocation pair (the observed buddy-allocator livelock; the same
/// churn starved the timer softirq into ALLOCTHROW panics on other
/// CPUs). Bound the wait: after WAIT_DEADLINE_JIFFIES, give up and
/// report -EIO so the syscall unwinds and the task stays killable.
unsafe fn wait_buffer_io_done(bh: *mut BufferHead) -> i32 {
    const WAIT_DEADLINE_JIFFIES: u64 = crate::drivers::timer::msecs_to_jiffies(10_000);
    let mut slept = 0u32;
    let deadline = crate::drivers::timer::get_jiffies().saturating_add(WAIT_DEADLINE_JIFFIES);
    loop {
        let state = (*bh).get_state();
        if !state.test(BufferState::BH_Req) {
            return if state.test(BufferState::BH_Uptodate) { 0 } else { -5 };
        }
        if crate::sched::current().is_some() {
            if crate::drivers::timer::get_jiffies() >= deadline {
                crate::pr_err!(
                    "bio: buffer io wait timeout block={} — reporting EIO (wedge fix)",
                    (*bh).b_blocknr
                );
                return -5;
            }
            let pid = crate::sched::get_current_pid();
            let dl = crate::drivers::timer::get_jiffies().saturating_add(1);
            let id = crate::timer::add_timer_wakeup(dl, pid);
            // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();
            if id != 0 {
                crate::timer::del_timer(id);
            }
            slept += 1;
        } else {
            // Early boot / IRQ context: no task to sleep, no timer budget —
            // spin as before (bounded by the caller's poll budget).
            core::hint::spin_loop();
        }
    }
}

pub fn bread_async(
    device: *const blkdev::GenDisk,
    blocknr: u64,
    completion: &crate::fs::io_completion::IoCompletion,
) -> Option<*mut BufferHead> {
    // SAFETY: device is a valid GenDisk pointer from the block device layer;
    // all CacheEntry pointers in the hash chain were created by Box::into_raw.
    unsafe {
        let (device_major, device_minor) = ((*device).major, (*device).first_minor);
        let cache = get_block_cache();
        let index = cache.hash_index(device_major, device_minor, blocknr);

        // Phase 1: Lookup under bucket lock
        {
            let mut bucket = cache.buckets[index].lock_irqsave();
            let mut prev: Option<*mut CacheEntry> = None;
            let mut current = bucket.head;

            while let Some(entry_ptr) = current {
                let entry = &*entry_ptr;
                // R9-9: same guards as get() — skip entries being evicted
                // and dead (freed/reused) BufferHeads.
                if entry.evicting {
                    prev = Some(entry_ptr);
                    current = entry.hash_next;
                    continue;
                }
                if unsafe { entry.bh.is_null() || (*entry.bh).b_data.len() != cache.block_size as usize } {
                    prev = Some(entry_ptr);
                    current = entry.hash_next;
                    continue;
                }
                if entry.key == (device_major, device_minor, blocknr) {
                    let state = (*entry.bh).get_state();
                    if !state.test(BufferState::BH_Uptodate)
                        && state.test(BufferState::BH_Req)
                    {
                        // In-flight entry owned by another caller: its I/O
                        // has not landed yet. Completing now would hand out
                        // stale buffer contents (review 2R.11) — take a
                        // reference, drop the lock, wait for the owner's
                        // I/O, then complete.
                        let bh = entry.bh;
                        (*bh).get();
                        drop(bucket);
                        let status = wait_buffer_io_done(bh);
                        completion.complete(status);
                        return Some(bh);
                    }
                    // Cache hit (up to date)
                    if prev.is_some() {
                        let prev_entry = &mut *prev.unwrap();
                        prev_entry.hash_next = entry.hash_next;
                        (*entry_ptr).hash_next = bucket.head;
                        bucket.head = Some(entry_ptr);
                    }
                    let mut lru = unsafe { cache.lru_lock_under_bucket() };
                    BlockCache::move_to_lru_head(&mut lru, entry_ptr);
                    (*entry.bh).get();
                    // The caller will bread_wait() on its completion; no I/O
                    // is in flight for a cache hit, so signal it now —
                    // otherwise the caller sleeps forever (review VFS-H7).
                    completion.complete(0);
                    return Some(entry.bh);
                }
                prev = Some(entry_ptr);
                current = entry.hash_next;
            }
        }

        // Phase 1.5: Evict if cache is full (soft limit — see the
        // read-path Phase 1.5 comment; bounded reclaim, then admit).
        let mut evict_tries = 0u32;
        while cache.count.load(Ordering::Acquire) as usize >= cache.max_entries {
            if !cache.evict_one() || evict_tries >= 32 {
                break;
            }
            evict_tries += 1;
        }

        // Phase 2: Cache miss — submit async I/O (no lock held).
        // Uninit buffer: the device DMA fills every byte; zero-filling 4 KiB
        // per block was measurable waste on the batch read-ahead path.
        let block_size = cache.block_size;
        let mut bh = Box::new(BufferHead::new_uninit(blocknr, block_size));
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
        let entry = Box::new(CacheEntry::new(bh, device_major, device_minor, blocknr));
        let entry_ptr = Box::into_raw(entry);

        {
            let mut bucket = cache.buckets[index].lock_irqsave();

            // Double-check for duplicate
            let mut dup_bh: Option<*mut BufferHead> = None;
            let mut current = bucket.head;
            while let Some(cp) = current {
                if (*cp).key == (device_major, device_minor, blocknr) {
                    // R13-2: same dead/evicting guards as get() Phase 3.
                    if (*cp).evicting
                        || unsafe {
                            (*cp).bh.is_null()
                                || (*(*cp).bh).b_data.len() != cache.block_size as usize
                        }
                    {
                        current = (*cp).hash_next;
                        continue;
                    }
                    (*(*cp).bh).get();
                    let mut lru = unsafe { cache.lru_lock_under_bucket() };
                    BlockCache::move_to_lru_head(&mut lru, cp);
                    dup_bh = Some((*cp).bh);
                    break;
                }
                current = (*cp).hash_next;
            }
            drop(bucket);

            if let Some(existing_bh) = dup_bh {
                // A concurrent caller inserted the same block first. OUR
                // redundant I/O (Phase 2) is still going to DMA into the new
                // bh — free it only after that I/O lands (completion is the
                // one we passed to blkdev_read_async), then also wait for
                // the existing buffer to be up to date so the caller never
                // observes a half-filled cache hit (review 2R.11).
                let _ = completion.wait();
                // SAFETY: entry_ptr was created by Box::into_raw above; our
                // I/O has finished and the entry was never published.
                let _ = Box::from_raw(entry_ptr);
                let _ = wait_buffer_io_done(existing_bh);
                // The caller's bread_wait() sees the completion our I/O
                // signaled; the winning buffer is settled either way.
                return Some(existing_bh);
            }

            {
                let mut bucket = cache.buckets[index].lock_irqsave();
                (*entry_ptr).hash_next = bucket.head;
                bucket.head = Some(entry_ptr);

                let mut lru = unsafe { cache.lru_lock_under_bucket() };
                BlockCache::move_to_lru_head(&mut lru, entry_ptr);
            }

            cache.count.fetch_add(1, Ordering::Release);
            Some((*entry_ptr).bh)
        }
    }
}

/// Wait for an async buffer read to complete.
///
/// Blocks until the IoCompletion signals done, then marks the buffer
/// as up-to-date and clears the in-flight flag.
///
/// R20-FS8: returns the I/O status (0 = success, negative errno on failure)
/// so callers can refuse to consume/cache data from a failed read. The
/// buffer stays in the cache marked !Uptodate on error.
pub fn bread_wait(bh: *mut BufferHead, completion: &crate::fs::io_completion::IoCompletion) -> i32 {
    // SAFETY: bh is a raw pointer returned by bread_async(); it points to a
    // valid BufferHead owned by a CacheEntry in the cache.
    unsafe {
        let status = completion.wait();
        if status == 0 {
            (*bh).set_state_bit(BufferState::BH_Uptodate);
        }
        (*bh).clear_state_bit(BufferState::BH_Req);
        status
    }
}

/// Release a buffer
pub fn brelse(bh: *const BufferHead) {
    get_block_cache().put(bh)
}

/// Non-blocking presence probe: does a live block-cache entry exist for
/// (device, blocknr)? No I/O, no pinning, no LRU touch.
///
/// Coherence guard for `bread_async_multi`: that path DMA-reads a whole
/// block run into a private buffer WITHOUT consulting the block cache.
/// If any member block has a live entry here — especially a DIRTY one
/// holding data the disk does not have yet (lazy write-back defers while
/// a journal handle is active) — the private read would fetch stale disk
/// bytes and poison the page cache with them. Callers must split such
/// runs and use per-block `bread_async`, which returns the cached
/// (uptodate, possibly dirty) contents instead.
pub fn block_cached(device: *const blkdev::GenDisk, blocknr: u64) -> bool {
    // SAFETY: device is a valid GenDisk pointer from the block device layer;
    // entry traversal mirrors bread_async Phase 1 under the bucket lock.
    unsafe {
        let (device_major, device_minor) = ((*device).major, (*device).first_minor);
        let cache = get_block_cache();
        let index = cache.hash_index(device_major, device_minor, blocknr);
        let bucket = cache.buckets[index].lock_irqsave();
        let mut current = bucket.head;
        while let Some(entry_ptr) = current {
            let entry = &*entry_ptr;
            // Same validity guards as bread_async Phase 1: skip entries
            // being evicted and dead (freed/reused) BufferHeads.
            if entry.evicting {
                current = entry.hash_next;
                continue;
            }
            if entry.bh.is_null() || (*entry.bh).b_data.len() != cache.block_size as usize {
                current = entry.hash_next;
                continue;
            }
            if entry.key == (device_major, device_minor, blocknr) {
                return true; // SpinlockGuard drops on early return
            }
            current = entry.hash_next;
        }
        false
    }
}

/// Submit ONE async read covering `nblocks` consecutive disk blocks into a
/// single buffer — the coalesced-readahead fast path.
///
/// Every request costs a virtqueue chain (3 descriptors, a header/resp
/// allocation, and one device kick amortized over the batch); under TCG the
/// per-request fixed cost dwarfs the per-byte DMA cost, so a 32-block
/// (128 KiB) request is dramatically cheaper than 32 single-block ones.
///
/// The returned BufferHead is PRIVATE: it is never inserted into the block
/// cache (its odd size would be invisible to `block_size`-guarded lookups
/// anyway). Consume it with `bread_wait`, copy the data out (page-cache
/// inserts), then release it with `bfree_multi` — never `brelse`.
/// A return of None means submission failed (no async support / queue
/// full); the caller falls back to per-block reads.
pub fn bread_async_multi(
    device: *const blkdev::GenDisk,
    start_blocknr: u64,
    nblocks: u32,
    completion: &crate::fs::io_completion::IoCompletion,
) -> Option<*mut BufferHead> {
    if nblocks == 0 {
        return None;
    }
    if nblocks == 1 {
        return bread_async(device, start_blocknr, completion);
    }
    let cache = get_block_cache();
    let block_size = cache.block_size as usize;
    let total = nblocks as usize * block_size;
    let mut bh = Box::new(BufferHead::new_uninit(start_blocknr, total as u32));
    bh.set_device(device);
    bh.set_state_bit(BufferState::BH_Req);
    let sectors_per_block = cache.block_size as u64 / 512;
    // SAFETY-free: device/buffer validity mirrors bread_async Phase 2; the
    // caller keeps the BufferHead (and its Vec) alive until bread_wait.
    if blkdev::blkdev_read_async(
        device,
        start_blocknr * sectors_per_block,
        &mut bh.b_data,
        completion,
    )
    .is_err()
    {
        return None;
    }
    Some(Box::into_raw(bh))
}

/// Free a private multi-block BufferHead from `bread_async_multi`.
///
/// The caller must have waited for its I/O (bread_wait) first — the DMA
/// target must not be freed while the device still writes to it.
///
/// # Safety
/// `bh` must come from `bread_async_multi`, exactly once, and its I/O must
/// have completed.
pub unsafe fn bfree_multi(bh: *mut BufferHead) {
    if bh.is_null() {
        return;
    }
    // SAFETY: caller contract above; Box::into_raw in bread_async_multi.
    unsafe { drop(Box::from_raw(bh)) };
}

/// Sync a dirty buffer to disk
///
/// Lazy write-back: while the current task holds an active ext4 journal
/// handle, the sync is deferred — the buffer simply stays BH_Dirty. This
/// covers ALL metadata writes inside a write(2)/create transaction (data
/// blocks, inode table, bitmaps, group descriptors, extent trees,
/// directory blocks): a synchronous virtio round trip per sync (~10-20ms
/// under TCG) made every small-write workload — each LTP test's setup
/// phase — blow the 30s wall-clock timeout (the r5 "post-exec hang"
/// family). Persistence is restored at the batch points: the journal
/// commit's write-through fast path (registered buffers), fsync/sync
/// (ext4_sync_file flushes the file's data blocks and forces the
/// transaction to commit; sync(2) writes the whole cache), and
/// buffer-cache eviction (dirty victims are synced before reuse).
pub fn sync_dirty_buffer(bh: *const BufferHead) -> Result<(), i32> {
    // SAFETY: get_current_handle reads the current task's handle slot and
    // is null-safe when no task context exists (interrupt/early boot).
    let journal_active = unsafe {
        crate::fs::ext4::namei::get_current_handle().is_some()
    };
    if journal_active {
        return Ok(());
    }
    // SAFETY: bh is a raw pointer returned by bread(); it points to a valid
    // BufferHead owned by a CacheEntry in the cache.
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
