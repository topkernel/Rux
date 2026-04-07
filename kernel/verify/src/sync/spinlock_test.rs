//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RawSpinlock TAS protocol invariant tests.
//!
//! Types copied from: kernel/src/sync/spinlock.rs (simplified — no asm, no deadlock warn)

use std::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Copied types from kernel/src/sync/spinlock.rs
// ============================================================================

pub struct RawSpinlock {
    locked: AtomicU32,
}

impl RawSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self { locked: AtomicU32::new(0) }
    }

    #[inline]
    pub fn lock(&self) {
        while self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_err() {
            std::hint::spin_loop();
        }
    }

    #[inline]
    pub fn try_lock(&self) -> bool {
        self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok()
    }

    #[inline]
    pub fn unlock(&self) {
        self.locked.store(0, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire) != 0
    }
}

// ============================================================================
// Tests
// ============================================================================

/// INV-SPIN-1: try_lock then unlock succeeds
#[test]
fn test_try_lock_unlock() {
    let lock = RawSpinlock::new();
    assert!(!lock.is_locked());
    assert!(lock.try_lock());
    assert!(lock.is_locked());
    lock.unlock();
    assert!(!lock.is_locked());
}

/// INV-SPIN-2: lock then unlock works
#[test]
fn test_lock_unlock() {
    let lock = RawSpinlock::new();
    assert!(!lock.is_locked());
    lock.lock();
    assert!(lock.is_locked());
    lock.unlock();
    assert!(!lock.is_locked());
}

/// INV-SPIN-3: unlock on unlocked lock is safe
#[test]
fn test_unlock_unlocked() {
    let lock = RawSpinlock::new();
    lock.unlock(); // should not panic
    assert!(!lock.is_locked());
}

/// INV-SPIN-4: try_lock fails when already locked
#[test]
fn test_try_lock_fails_when_locked() {
    let lock = RawSpinlock::new();
    lock.lock();
    assert!(!lock.try_lock());
    lock.unlock();
}
