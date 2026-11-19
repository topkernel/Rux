//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for RawSpinlock (try_lock variant only).
//!
//! Types copied from: kernel/verify/src/sync/spinlock_test.rs
//!
//! Note: RawSpinlock::lock() has a spin loop unsuitable for Kani.
//! All harnesses use try_lock() instead.

#![cfg(kani)]

use std::sync::atomic::{AtomicU32, Ordering};

pub struct RawSpinlock {
    locked: AtomicU32,
}

impl RawSpinlock {
    pub const fn new() -> Self { Self { locked: AtomicU32::new(0) } }

    pub fn try_lock(&self) -> bool {
        self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok()
    }

    pub fn unlock(&self) { self.locked.store(0, Ordering::Release); }

    pub fn is_locked(&self) -> bool { self.locked.load(Ordering::Acquire) != 0 }
}

/// INV-SPIN-K1: try_lock → unlock returns to unlocked state.
#[kani::proof]
fn verify_spinlock_try_lock_unlock() {
    let lock = RawSpinlock::new();
    assert!(!lock.is_locked());
    assert!(lock.try_lock());
    assert!(lock.is_locked());
    lock.unlock();
    assert!(!lock.is_locked());
}

/// INV-SPIN-K2: try_lock fails when already locked.
#[kani::proof]
fn verify_spinlock_try_lock_fails_when_locked() {
    let lock = RawSpinlock::new();
    assert!(lock.try_lock());
    assert!(!lock.try_lock());  // second try fails
    lock.unlock();
    assert!(lock.try_lock());    // succeeds after unlock
    lock.unlock();
}
