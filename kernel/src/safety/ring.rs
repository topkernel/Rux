//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Static interrupt-ring primitive — the console/trace ring family
//! (unsafe-slim round 4).
//!
//! The kernel's input rings re-implement one shape five times: a
//! `static` slot array behind an `UnsafeCell`, an atomic producer
//! index, an atomic consumer index, and per-access `unsafe` index
//! arithmetic (console.rs UartRxBuf, the trace ring, the printk slot
//! array, the per-CPU expired-token buffers). This module owns the
//! shape once: construction is the one audited edge (the const
//! initializer), and every access is safe code.
//!
//! Discipline encoded here:
//!
//! * **INV-RING-1 (capacity)** — `N` must be a power of two; the mask
//!   is derived from `N`, and a non-power-of-two `N` degenerates to
//!   slot 0 (safe, useless — caught by the in-kernel test, never by a
//!   boot). An index can never leave the array.
//! * **INV-RING-2 (SPSC with a CAS consumer)** — one producer (the
//!   IRQ handler) advances `tail`; consumers CAS `head`, so forked
//!   children sharing the device stay correct (the R27-1 first-byte
//!   loss family). A dropped value beats a wedged producer — `put`
//!   never blocks.
//! * **INV-RING-3 (static liveness)** — the ring lives in a `static`
//!   for the machine's lifetime; slots are `MaybeUninit<T: Copy>`
//!   cells, so no drop glue ever runs on a torn state. Volatile
//!   slot access: producer and consumer run on different harts.
//! * **INV-RING-4 (peek/depth)** — `depth_hint()` is a hint only
//!   (Relaxed indexes); the CAS in `get` is the sole authority on
//!   whether a value is taken.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A machine-lifetime slot ring for `T: Copy`, IRQ-producer /
/// task-consumer. `N` should be a power of two (INV-RING-1).
pub struct SpscRing<T: Copy, const N: usize> {
    slots: UnsafeCell<[UnsafeCell<MaybeUninit<T>>; N]>,
    /// Consumer index (task context; CAS hand-out).
    head: AtomicUsize,
    /// Producer index (IRQ context; the single writer).
    tail: AtomicUsize,
}

// SAFETY (INV-RING-3): the ring lives in a static for the machine's
// lifetime; slot access is bounded (INV-RING-1) and the producer /
// consumer hand-off is the atomics' own synchronization. T: Copy means
// the slots never run drop glue.
// Send/Sync commitments for SpscRing live in the audited unsafe.rs.

impl<T: Copy, const N: usize> SpscRing<T, N> {
    /// Const constructor for `static` use (the one audited edge:
    /// INV-RING-3).
    pub const fn new() -> Self {
        // INV-RING-1 enforced, not just documented: a non-power-of-two
        // N makes MASK 0 and the ring silently drops EVERYTHING (the
        // Miri-audit finding — logical defect, no in-tree tripper).
        // const-evaluated: a bad N fails the build, never a boot.
        assert!(N.is_power_of_two(), "SpscRing: N must be a power of two (INV-RING-1)");
        Self {
            slots: UnsafeCell::new([const {
                UnsafeCell::new(MaybeUninit::zeroed())
            }; N]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    const MASK: usize = if N > 0 && N & (N - 1) == 0 { N - 1 } else { 0 };

    /// Producer: write one value (IRQ context). Drops the value when
    /// the ring is full (INV-RING-2 — a bounded producer never
    /// wedges).
    #[inline]
    pub fn put(&self, val: T) {
        let tail = self.tail.load(Ordering::Relaxed);
        let next = (tail + 1) & Self::MASK;
        if next == self.head.load(Ordering::Acquire) {
            return; // full: drop
        }
        let idx = tail & Self::MASK;
        // Producer slot edge lives in the audited unsafe.rs (INV-RING-1).
        super::edges::ring_slot_write(&self.slots, idx, val);
        self.tail.store(next, Ordering::Release);
    }

    /// Consumer: take one value (task context; CAS so multiple
    /// consumers sharing the device stay correct).
    #[inline]
    pub fn get(&self) -> Option<T> {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            if head == self.tail.load(Ordering::Acquire) {
                return None;
            }
            let idx = head & Self::MASK;
            // Consumer slot edge lives in the audited unsafe.rs
            // (INV-RING-1/3).
            let val = super::edges::ring_slot_read(&self.slots, idx);
            let next = (head + 1) & Self::MASK;
            match self.head.compare_exchange_weak(
                head,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(val),
                Err(h) => head = h,
            }
        }
    }

    /// Hint: how many values are buffered (Relaxed indexes;
    /// INV-RING-4 — never an authority).
    #[inline]
    pub fn depth_hint(&self) -> usize {
        self.tail
            .load(Ordering::Relaxed)
            .wrapping_sub(self.head.load(Ordering::Relaxed))
            & Self::MASK
    }
}
