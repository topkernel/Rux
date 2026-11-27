//! The trace subsystem's ONE audited unsafe file (mainline minimal-first
//! consolidation, 2026-11-27): the Send/Sync commitments and the ring's
//! slot-protocol edges live here; mod.rs and files.rs are
//! `#![forbid(unsafe_code)]` safe code over them.

use super::TraceEvent;
use super::TraceRecord;
use super::TraceRing;

// SAFETY: the event table is read-only after boot except for the
// per-event `enabled` flag, which is an atomic.
unsafe impl Send for TraceEvent {}
unsafe impl Sync for TraceEvent {}

// SAFETY: the ring's mutable state is behind atomics; the slot array's
// interior mutability is behind the slot protocol (INV-TRACERING-1).
unsafe impl Send for TraceRing {}
unsafe impl Sync for TraceRing {}

impl TraceRing {
    /// Shared slot view (INV-TRACERING-1): a reader may trust the slot
    /// only for slots below the published `count` — the same contract
    /// printk's RecordsArray documents for its ring.
    #[inline]
    pub(super) fn slot(&self, idx: usize) -> &TraceRecord {
        // SAFETY: INV-TRACERING-1 — idx is masked into range by the
        // caller's modulo; the array is machine-lifetime static storage.
        unsafe { &(*self.records.get())[idx] }
    }

    /// Exclusive slot view for the writer that owns the slot between
    /// the head hand-out and the count publication (INV-TRACERING-1).
    #[inline]
    pub(super) fn slot_mut(&self, idx: usize) -> &mut TraceRecord {
        // SAFETY: INV-TRACERING-1 — the head counter hands out distinct
        // slots to concurrent writers; the record is written before the
        // count is published. A reader may race with the write and
        // observe a torn record — the documented debug-facility trade.
        unsafe { &mut (*self.records.get())[idx] }
    }
}
