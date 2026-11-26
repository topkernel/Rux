//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RawSpinlock TAS protocol invariant tests.
//!
//! Types copied from: kernel/core/src/sync/types.rs (simplified — no asm, no deadlock warn)

use std::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Copied types from kernel/core/src/sync/types.rs
// ============================================================================

pub struct RawSpinlock {
    locked: AtomicU32,
}

// The kernel's owner-in-word protocol (grq-source-fix): the word holds
// cpu+1, zero means free; holder_cpu/held_by read it, and the terminal
// path force-releases with ownership provable from one load.
const MAX_CPUS: usize = 8;

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
        let ok = self
            .locked
            .compare_exchange(0, Self::held_word(), Ordering::Acquire, Ordering::Acquire)
            .is_ok();
        if ok {
            self.record_owner(caller_return_address());
        }
        ok
    }

    /// Terminal-path force release (grq-source-fix): the caller proves
    /// ownership from the lock word itself (held_by(cpu)); no
    /// preempt_count_sub — the terminal path resets the whole count.
    pub fn force_unlock_terminal(&self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }

    pub fn locked_word(&self) -> u32 {
        self.locked.load(Ordering::Relaxed)
    }

    pub fn holder_cpu(&self) -> Option<usize> {
        let w = self.locked.load(Ordering::Relaxed);
        if w == 0 || w > MAX_CPUS as u32 {
            return None;
        }
        Some((w - 1) as usize)
    }

    pub fn held_by(&self, cpu: usize) -> bool {
        self.locked.load(Ordering::Relaxed) == (cpu as u32).wrapping_add(1)
    }

    #[inline]
    pub fn unlock(&self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }

    /// The held word for THIS cpu — the mirror models a single cpu, so
    /// the constant 1 (the kernel's is cpu_id + 1; the sync-checker
    /// transform canonicalizes that body to this text).
    #[inline]
    fn held_word() -> u32 {
        1u32
    }

    /// The kernel records the lock owner for its deadlock detector; the
    /// copy keeps the shape with an inert stub. (The kernel's
    /// preempt_count_add/sub balance around the critical section is
    /// preemption gating, stripped by the sync-checker normalization.)
    #[inline]
    fn record_owner(&self, _ra: usize) {}

    /// See record_owner().
    #[inline]
    fn clear_owner(&self) {}

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire) != 0
    }
}

/// The kernel captures the call-site return address for deadlock
/// diagnostics; the copy has no frames to capture, so it is constant.
#[inline]
fn caller_return_address() -> usize {
    0
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
