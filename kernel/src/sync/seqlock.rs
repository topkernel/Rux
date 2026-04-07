//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SeqLock — sequence lock for read-mostly data.
//!
//! Writers are exclusive (serialized like a spinlock).
//! Readers never block: they snapshot the data and retry if a write
//! was in progress during the read window.
//!
//! Constraints:
//!   - T must be Copy (readers snapshot the data).
//!   - Readers disable preemption internally to prevent same-CPU
//!     writer preemption during the read window.
//!   - Suitable for small-to-moderate structs (copy cost is the read cost).
//!
//! Protocol:
//!   - Sequence counter low bit: even = no writer, odd = writer active.
//!   - Writer: increment (odd) → write data → increment (even).
//!   - Reader: read seq → snapshot data → re-read seq → retry if changed.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

// ============================================================================
// RawSeqLock
// ============================================================================

/// Raw sequence lock backend.
///
/// The sequence counter uses the low bit as a "writer active" flag:
///   - Even value  => no writer active, data is consistent
///   - Odd value   => writer active, data may be inconsistent
pub struct RawSeqLock {
    sequence: AtomicUsize,
}

impl RawSeqLock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            sequence: AtomicUsize::new(0),
        }
    }

    /// Acquire writer slot: spin until sequence is even, then CAS to odd.
    #[inline]
    pub fn write_lock(&self) {
        loop {
            let seq = self.sequence.load(Ordering::Relaxed);
            if seq & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            if self.sequence
                .compare_exchange_weak(seq, seq + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Release writer slot: increment sequence to even.
    #[inline]
    pub fn write_unlock(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// Try to acquire writer slot. Returns true on success.
    #[inline]
    pub fn try_write_lock(&self) -> bool {
        let seq = self.sequence.load(Ordering::Relaxed);
        if seq & 1 != 0 {
            return false;
        }
        self.sequence
            .compare_exchange_weak(seq, seq + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Read the current sequence number (Acquire ordering).
    #[inline]
    pub fn read_begin(&self) -> usize {
        self.sequence.load(Ordering::Acquire)
    }

    /// Check if the sequence changed or a writer is active.
    /// Returns true if the read must be retried.
    #[inline]
    pub fn read_retry(&self, seq1: usize) -> bool {
        let seq2 = self.sequence.load(Ordering::Acquire);
        seq1 != seq2 || seq2 & 1 != 0
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.sequence.load(Ordering::Relaxed) & 1 != 0
    }
}

// ============================================================================
// SeqLock<T>
// ============================================================================

/// A sequence lock protecting data of type `T`.
///
/// `T` must be `Copy` because readers snapshot the entire value without
/// holding any lock.
pub struct SeqLock<T: Copy> {
    raw: RawSeqLock,
    data: UnsafeCell<T>,
}

// Safety: writers have exclusive access; readers get a Copy snapshot.
unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
unsafe impl<T: Copy + Send> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            raw: RawSeqLock::new(),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the writer lock. Returns a guard with `DerefMut`.
    #[inline]
    pub fn write(&self) -> SeqLockWriteGuard<'_, T> {
        preempt_disable();
        self.raw.write_lock();
        SeqLockWriteGuard { lock: self }
    }

    /// Try to acquire the writer lock. Returns None if a writer is active.
    #[inline]
    pub fn try_write(&self) -> Option<SeqLockWriteGuard<'_, T>> {
        preempt_disable();
        if self.raw.try_write_lock() {
            Some(SeqLockWriteGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    /// Read a snapshot of the protected data.
    ///
    /// Disables preemption, reads the sequence counter, copies the data,
    /// re-reads the sequence counter, and retries if a write intervened.
    /// Returns the owned snapshot.
    #[inline]
    pub fn read(&self) -> T {
        preempt_disable();
        let result = self.read_inner();
        preempt_enable();
        result
    }

    /// Read without disabling preemption.
    ///
    /// # Safety
    /// Caller must hold preemption disabled for the duration of this call.
    #[inline]
    pub unsafe fn read_preempt_disabled(&self) -> T {
        self.read_inner()
    }

    /// Check if a writer is currently active.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    /// Core read loop (no preemption management).
    #[inline]
    fn read_inner(&self) -> T {
        loop {
            let seq = self.raw.read_begin();
            if seq & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: preemption is disabled and sequence is even (no writer).
            let snapshot = unsafe { core::ptr::read_volatile(self.data.get()) };
            if self.raw.read_retry(seq) {
                core::hint::spin_loop();
                continue;
            }
            return snapshot;
        }
    }
}

// ============================================================================
// SeqLockWriteGuard
// ============================================================================

/// RAII guard for an active SeqLock writer.
///
/// Provides `Deref` and `DerefMut` to the inner data.
/// On drop: releases the writer slot and re-enables preemption.
pub struct SeqLockWriteGuard<'a, T: Copy> {
    lock: &'a SeqLock<T>,
}

unsafe impl<T: Copy + Send> Send for SeqLockWriteGuard<'_, T> {}

impl<T: Copy> Deref for SeqLockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: Copy> DerefMut for SeqLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: Copy> Drop for SeqLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        preempt_enable();
    }
}

// ============================================================================
// Inline helpers
// ============================================================================

#[inline]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
}

#[inline]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
}
