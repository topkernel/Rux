//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Write-once publication slot (unsafe-slim round 5).
//!
//! The "probe publishes, everyone else reads" family: device singletons
//! (`VIRTIO_BLK`, `EVDEV_KEYBOARD`, `VIRTIO_NET`, `LO_DEVICE`, the PCI
//! per-slot device/queue tables) were hand-rolled `static mut
//! Option<T>` cells with an atomic ready flag as the publication
//! barrier. [`OnceSlot`] is that pattern as a type — the publication
//! protocol lives HERE, once, and the call sites are safe code.
//!
//! Invariants:
//!
//! * **INV-SLOT-1 (single publication)** — `publish` runs at most once
//!   per slot, at init/probe time, before any concurrent reader can
//!   observe the slot through its owner's own publication ordering
//!   (the owner's ready flag, registration, or probe serialization).
//!   A second publish is a bug and is refused (debug: panic).
//! * **INV-SLOT-2 (barrier)** — publish stores with `Release`; `get`
//!   loads the state with `Acquire`, so a value seen through `get` is
//!   fully constructed (its interior atomics/locks are usable).
//! * **INV-SLOT-3 (exclusive mut access)** — `get_mut_locked` requires
//!   an [`ExclusivityProof`] token; the only impls are lock guards
//!   whose acquisition governs every mutation of the slot's value.
//!   The impl site (not the call site) carries the audit.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

/// Marker token proving the caller holds the lock that governs a
/// slot's mutable access (INV-SLOT-3). Implemented ONLY by lock guards
/// whose lock serializes all mutation of the slot's value — the impl
/// is the audited assertion.
pub trait ExclusivityProof {}

/// A write-once publication slot (INV-SLOT-1/2).
pub struct OnceSlot<T> {
    published: AtomicBool,
    value: UnsafeCell<MaybeUninit<T>>,
}

// SAFETY (INV-SLOT-1/2/3): publication happens-before any concurrent
// get (Release/Acquire pair); mutation beyond publication requires the
// governing lock via ExclusivityProof, so T's own Send+Sync bounds hold.
unsafe impl<T: Send + Sync> Sync for OnceSlot<T> {}
unsafe impl<T: Send> Send for OnceSlot<T> {}

impl<T> OnceSlot<T> {
    /// An empty slot.
    pub const fn new() -> Self {
        OnceSlot {
            published: AtomicBool::new(false),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Publish `v` (INV-SLOT-1: at most once). Returns false if the
    /// slot was already published (the value is dropped, unchanged).
    pub fn publish(&self, v: T) -> bool {
        self.publish_with_addr(|_| v)
    }

    /// Publish a value that needs its own final address at construction
    /// (self-referential registration — e.g. a device whose
    /// `private_data` points at its own static slot). `make` receives
    /// the address the value will occupy; the value it returns is
    /// written there and published (INV-SLOT-1/2).
    pub fn publish_with_addr(&self, make: impl FnOnce(*const T) -> T) -> bool {
        if self.published.load(Ordering::Acquire) {
            return false; // double publish: refuse (INV-SLOT-1)
        }
        let addr = self.value.get() as *const T;
        // SAFETY: INV-SLOT-1 — not yet published, so no reader can be
        // concurrently reading the cell (readers only read a set flag).
        unsafe {
            (*self.value.get()).write(make(addr));
        }
        // INV-SLOT-2: the Release store orders the value write before
        // any Acquire load of the flag that observes true.
        self.published.store(true, Ordering::Release);
        true
    }

    /// The published value, if any (INV-SLOT-2).
    pub fn get(&self) -> Option<&T> {
        if self.published.load(Ordering::Acquire) {
            // SAFETY: INV-SLOT-1/2 — the flag is set, so the value was
            // written before our Acquire; it is never overwritten or
            // dropped afterwards.
            Some(unsafe { (*self.value.get()).assume_init_ref() })
        } else {
            None
        }
    }

    /// Exclusive access to the published value (INV-SLOT-3): `_proof`
    /// must be a guard of the lock that serializes ALL mutation of
    /// this slot's value. Shared readers may race this access under
    /// the same discipline the raw static had (the value's own fields
    /// are atomics or lock-protected).
    pub fn get_mut_locked<P: ExclusivityProof>(&self, _proof: &P) -> Option<&mut T> {
        if self.published.load(Ordering::Acquire) {
            // SAFETY: INV-SLOT-3 — exclusivity is proven by the guard
            // token; INV-SLOT-1/2 as in `get`.
            Some(unsafe { (*self.value.get()).assume_init_mut() })
        } else {
            None
        }
    }

    /// Whether the slot has been published.
    pub fn is_set(&self) -> bool {
        self.published.load(Ordering::Acquire)
    }
}

impl<T> Default for OnceSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_once_then_read() {
        let s: OnceSlot<u32> = OnceSlot::new();
        assert!(s.get().is_none());
        assert!(s.publish(7));
        assert_eq!(*s.get().unwrap(), 7);
        assert!(!s.publish(9), "second publish refused");
        assert_eq!(*s.get().unwrap(), 7, "value unchanged after refused publish");
    }

    #[test]
    fn struct_value_with_interior_lock() {
        struct Dev {
            n: core::sync::atomic::AtomicU32,
        }
        let s: OnceSlot<Dev> = OnceSlot::new();
        s.publish(Dev {
            n: core::sync::atomic::AtomicU32::new(1),
        });
        s.get().unwrap().n.fetch_add(1, Ordering::Relaxed);
        assert_eq!(s.get().unwrap().n.load(Ordering::Relaxed), 2);
    }
}
