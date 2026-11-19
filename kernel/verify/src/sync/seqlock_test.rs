//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SeqLock protocol invariant tests.
//!
//! Types copied from: kernel/src/sync/seqlock.rs

use proptest::prelude::*;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

// ============================================================================
// Copied types from kernel/src/sync/seqlock.rs
// ============================================================================

pub struct RawSeqLock {
    sequence: AtomicUsize,
}

impl RawSeqLock {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicUsize::new(0),
        }
    }

    pub fn write_lock(&self) {
        loop {
            let seq = self.sequence.load(Ordering::Relaxed);
            if seq & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            if self.sequence
                .compare_exchange_weak(seq, seq + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            std::hint::spin_loop();
        }
    }

    pub fn write_unlock(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub fn try_write_lock(&self) -> bool {
        let seq = self.sequence.load(Ordering::Relaxed);
        if seq & 1 != 0 {
            return false;
        }
        self.sequence
            .compare_exchange_weak(seq, seq + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    pub fn read_begin(&self) -> usize {
        self.sequence.load(Ordering::Acquire)
    }

    pub fn read_retry(&self, seq1: usize) -> bool {
        let seq2 = self.sequence.load(Ordering::Acquire);
        seq1 != seq2 || seq2 & 1 != 0
    }

    pub fn is_locked(&self) -> bool {
        self.sequence.load(Ordering::Relaxed) & 1 != 0
    }
}

pub struct SeqLock<T: Copy> {
    raw: RawSeqLock,
    data: UnsafeCell<T>,
}

unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
unsafe impl<T: Copy + Send> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            raw: RawSeqLock::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn write(&self) -> SeqLockWriteGuard<'_, T> {
        self.raw.write_lock();
        SeqLockWriteGuard { lock: self }
    }

    pub fn try_write(&self) -> Option<SeqLockWriteGuard<'_, T>> {
        if self.raw.try_write_lock() {
            Some(SeqLockWriteGuard { lock: self })
        } else {
            None
        }
    }

    pub fn read(&self) -> T {
        self.read_inner()
    }

    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    fn read_inner(&self) -> T {
        loop {
            let seq = self.raw.read_begin();
            if seq & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let snapshot = unsafe { core::ptr::read_volatile(self.data.get()) };
            if self.raw.read_retry(seq) {
                std::hint::spin_loop();
                continue;
            }
            return snapshot;
        }
    }
}

pub struct SeqLockWriteGuard<'a, T: Copy> {
    lock: &'a SeqLock<T>,
}

unsafe impl<T: Copy + Send> Send for SeqLockWriteGuard<'_, T> {}

impl<T: Copy> std::ops::Deref for SeqLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: Copy> std::ops::DerefMut for SeqLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: Copy> Drop for SeqLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SEQ-1: initial state is unlocked, sequence is 0
    #[test]
    fn test_initial_state(_v in 0u8..1u8) {
        let lock = SeqLock::new(42u64);
        prop_assert!(!lock.is_locked());
        prop_assert_eq!(lock.read(), 42);
    }

    /// INV-SEQ-2: write returns correct value after mutation
    #[test]
    fn test_write_mutates(val in 0u64..1_000_000u64) {
        let lock = SeqLock::new(0u64);
        {
            let mut guard = lock.write();
            *guard = val;
        }
        prop_assert!(!lock.is_locked());
        prop_assert_eq!(lock.read(), val);
    }

    /// INV-SEQ-3: is_locked reflects write state
    #[test]
    fn test_locked_state(_v in 0u8..1u8) {
        let lock = SeqLock::new(0u32);
        prop_assert!(!lock.is_locked());
        let _guard = lock.write();
        prop_assert!(lock.is_locked());
    }

    /// INV-SEQ-4: try_write returns None when locked
    #[test]
    fn test_try_write_fails_when_locked(_v in 0u8..1u8) {
        let lock = SeqLock::new(0u32);
        let _guard = lock.write();
        prop_assert!(lock.try_write().is_none());
    }

    /// INV-SEQ-5: try_write succeeds when unlocked
    #[test]
    fn test_try_write_succeeds_when_unlocked(_v in 0u8..1u8) {
        let lock = SeqLock::new(99u32);
        let guard = lock.try_write();
        prop_assert!(guard.is_some());
        drop(guard);
        prop_assert!(!lock.is_locked());
    }

    /// INV-SEQ-6: sequence number increments by 2 per write
    #[test]
    fn test_sequence_increments(_v in 0u8..1u8) {
        let lock = SeqLock::new(0u32);
        prop_assert_eq!(lock.raw.sequence.load(Ordering::Relaxed), 0);
        {
            let _g = lock.write();
            // sequence should be 1 (odd = writer active)
            prop_assert_eq!(lock.raw.sequence.load(Ordering::Relaxed), 1);
        }
        // sequence should be 2 (even = no writer)
        prop_assert_eq!(lock.raw.sequence.load(Ordering::Relaxed), 2);
        {
            let _g = lock.write();
            prop_assert_eq!(lock.raw.sequence.load(Ordering::Relaxed), 3);
        }
        prop_assert_eq!(lock.raw.sequence.load(Ordering::Relaxed), 4);
    }

    /// INV-SEQ-7: read after write sees the latest value
    #[test]
    fn test_read_consistency(
        vals in proptest::collection::vec(1u64..1_000_000u64, 1..10),
    ) {
        let lock = SeqLock::new(0u64);
        for &v in &vals {
            {
                let mut guard = lock.write();
                *guard = v;
            }
            // Read after write guard is dropped should see the written value
            let read_val = lock.read();
            prop_assert_eq!(read_val, v, "read saw inconsistent value");
        }
    }

    /// INV-SEQ-8: multiple fields in struct updated atomically
    #[test]
    fn test_struct_atomicity(_v in 0u8..1u8) {
        #[derive(Copy, Clone)]
        struct Point { x: u32, y: u32 }

        let lock = SeqLock::new(Point { x: 0, y: 0 });
        {
            let mut guard = lock.write();
            guard.x = 100;
            guard.y = 200;
        }
        let p = lock.read();
        prop_assert_eq!(p.x, 100);
        prop_assert_eq!(p.y, 200);
    }
}
