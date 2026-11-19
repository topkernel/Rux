//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RwSpinlock bit layout invariant tests + writer-fairness protocol model.
//!
//! Types copied from: kernel/src/sync/rwlock.rs

use proptest::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

// ============================================================================
// Copied constants from kernel/src/sync/rwlock.rs
// ============================================================================

const WRITER_BIT: u32 = 1u32 << 31;
const WRITER_PENDING_BIT: u32 = 1u32 << 30;
const READER_MASK: u32 = !(WRITER_BIT | WRITER_PENDING_BIT);

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-RWLOCK-1: WRITER_BIT is bit 31 (MSB)
    #[test]
    fn test_writer_bit_position(_v in 0u8..1u8) {
        prop_assert_eq!(WRITER_BIT, 1u32 << 31);
        prop_assert_eq!(WRITER_BIT, 0x80000000);
    }

    /// INV-RWLOCK-1b: WRITER_PENDING_BIT is bit 30
    #[test]
    fn test_writer_pending_bit_position(_v in 0u8..1u8) {
        prop_assert_eq!(WRITER_PENDING_BIT, 1u32 << 30);
        prop_assert_eq!(WRITER_PENDING_BIT, 0x40000000);
    }

    /// INV-RWLOCK-2: READER_MASK covers bits [29:0]
    #[test]
    fn test_reader_mask_position(_v in 0u8..1u8) {
        prop_assert_eq!(READER_MASK, 0x3FFFFFFF);
        prop_assert_eq!(READER_MASK.count_ones(), 30);
    }

    /// INV-RWLOCK-3: the two flag bits and READER_MASK are pairwise disjoint
    #[test]
    fn test_disjoint(_v in 0u8..1u8) {
        prop_assert_eq!(WRITER_BIT & READER_MASK, 0);
        prop_assert_eq!(WRITER_PENDING_BIT & READER_MASK, 0);
        prop_assert_eq!(WRITER_BIT & WRITER_PENDING_BIT, 0);
    }

    /// INV-RWLOCK-4: WRITER_BIT | WRITER_PENDING_BIT | READER_MASK covers all
    /// 32 bits
    #[test]
    fn test_full_coverage(_v in 0u8..1u8) {
        prop_assert_eq!(WRITER_BIT | WRITER_PENDING_BIT | READER_MASK, u32::MAX);
    }

    /// INV-RWLOCK-5: Setting reader count does not affect either flag bit
    #[test]
    fn test_reader_no_writer(reader_count in 0u32..0x4000_0000u32) {
        let state = reader_count & READER_MASK;
        prop_assert_eq!(state & WRITER_BIT, 0);
        prop_assert_eq!(state & WRITER_PENDING_BIT, 0);
        prop_assert_eq!(state & READER_MASK, reader_count);
    }

    /// INV-RWLOCK-7: Reader count is extractable via mask
    #[test]
    fn test_reader_extract(
        readers in 0u32..0x4000_0000u32,
        writer in 0u8..2u8,
        pending in 0u8..2u8,
    ) {
        let rc = readers & READER_MASK;
        let state = rc | (WRITER_BIT * (writer as u32))
            | (WRITER_PENDING_BIT * (pending as u32));
        let extracted_readers = state & READER_MASK;
        let has_writer = (state & WRITER_BIT) != 0;
        let has_pending = (state & WRITER_PENDING_BIT) != 0;
        prop_assert_eq!(extracted_readers, rc);
        prop_assert_eq!(has_writer, writer != 0);
        prop_assert_eq!(has_pending, pending != 0);
    }
}

#[test]
/// INV-RWLOCK-8: READER_MASK has bit 0 as lowest bit
fn test_reader_mask_lowest_bit() {
    assert_eq!(READER_MASK & 1, 1);
    assert_eq!(READER_MASK.trailing_zeros(), 0);
}

#[test]
/// INV-RWLOCK-9: WRITER_BIT has no bits in common with small reader counts
fn test_writer_vs_small_readers() {
    for readers in 0..=100u32 {
        assert_eq!(readers & WRITER_BIT, 0);
        assert_eq!(readers & WRITER_PENDING_BIT, 0);
        assert_eq!((readers | WRITER_BIT) & READER_MASK, readers);
    }
}

#[test]
/// INV-RWLOCK-10: Maximum reader count (without writer) fits in 30 bits
fn test_max_reader_count() {
    let max_readers = READER_MASK; // 0x3FFFFFFF = 2^30 - 1
    assert_eq!(max_readers.count_ones(), 30);
    assert!(max_readers < WRITER_PENDING_BIT);
    assert!(WRITER_PENDING_BIT < WRITER_BIT);
}

// ============================================================================
// Writer-fairness protocol model (mirrors RawRwSpinlock's methods)
// ============================================================================

/// Single-threaded model of the two-phase write() / blocking read()
/// protocol, exercising the exact transition sequence the kernel lock
/// performs (sync-realign followup: pending writer gates new readers).
struct ModelRw {
    state: AtomicU32,
}

impl ModelRw {
    fn new() -> Self {
        ModelRw { state: AtomicU32::new(0) }
    }

    fn read_may_enter(&self) -> bool {
        let s = self.state.load(Ordering::Acquire);
        s & (WRITER_BIT | WRITER_PENDING_BIT) == 0 && (s & READER_MASK) < READER_MASK
    }

    fn read_enter(&self) -> bool {
        if !self.read_may_enter() {
            return false;
        }
        let s = self.state.load(Ordering::Acquire);
        self.state
            .compare_exchange(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn read_exit(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    fn write_enter(&self) {
        // Phase 1: become the one pending writer.
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & WRITER_BIT != 0 || s & WRITER_PENDING_BIT != 0 {
                continue;
            }
            if self
                .state
                .compare_exchange_weak(s, s | WRITER_PENDING_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        self.write_take_pending();
    }

    /// Phase 2 alone: wait for the reader count to drain, then flip
    /// PENDING → WRITER. The caller must already own the pending bit
    /// (write_enter's phase 1, or a test that registered it directly).
    fn write_take_pending(&self) {
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & READER_MASK == 0 {
                if self
                    .state
                    .compare_exchange_weak(
                        s,
                        (s | WRITER_BIT) & !WRITER_PENDING_BIT,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
            }
        }
    }

    fn write_exit(&self) {
        self.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }

    fn is_write_held(&self) -> bool {
        self.state.load(Ordering::Acquire) & WRITER_BIT != 0
    }
}

#[test]
/// INV-RWLOCK-11 (protocol): mutual exclusion — while the write lock is
/// held, no reader may enter.
fn test_protocol_mutual_exclusion() {
    let rw = ModelRw::new();
    rw.write_enter();
    assert!(rw.is_write_held());
    assert!(!rw.read_may_enter(), "reader must not enter under WRITER_BIT");
    assert!(!rw.read_enter());
    rw.write_exit();
    assert!(rw.read_may_enter());
}

#[test]
/// INV-RWLOCK-12 (protocol): a PENDING writer blocks new readers while
/// the current reader generation still drains — the writer-fairness core
/// (readers cannot starve the queued writer forever).
fn test_protocol_pending_blocks_new_readers() {
    let rw = ModelRw::new();
    assert!(rw.read_enter(), "first reader enters on a free lock");
    // Writer arrives while a reader holds the lock: phase 1 registers
    // PENDING. We simulate the registration directly (phase 1 of
    // write_enter races nothing in the single-threaded model).
    let s = rw.state.load(Ordering::Acquire);
    rw.state
        .store(s | WRITER_PENDING_BIT, Ordering::Release);
    assert!(
        !rw.read_may_enter(),
        "a second reader must be gated by the pending writer"
    );
    assert!(!rw.read_enter());
    // The incumbent reader drains; the writer's phase 2 (it already owns
    // PENDING — write_enter's phase 1 must not be re-run) flips and holds.
    rw.read_exit();
    rw.write_take_pending();
    assert!(rw.is_write_held());
    assert!(!rw.read_may_enter());
    rw.write_exit();
    assert!(rw.read_may_enter());
}

#[test]
/// INV-RWLOCK-13 (protocol): the incumbent reader's read_unlock preserves
/// the pending bit (fetch_sub touches only the reader count field).
fn test_protocol_read_unlock_preserves_pending() {
    let rw = ModelRw::new();
    assert!(rw.read_enter());
    assert!(rw.read_enter());
    let s = rw.state.load(Ordering::Acquire);
    rw.state.store(s | WRITER_PENDING_BIT, Ordering::Release);
    rw.read_exit();
    let after = rw.state.load(Ordering::Acquire);
    assert_eq!(after & READER_MASK, 1);
    assert_eq!(after & WRITER_PENDING_BIT, WRITER_PENDING_BIT);
    rw.read_exit();
    let after2 = rw.state.load(Ordering::Acquire);
    assert_eq!(after2 & READER_MASK, 0);
    assert_eq!(after2, WRITER_PENDING_BIT);
}

#[test]
/// INV-RWLOCK-14 (protocol): write_exit clears only WRITER_BIT, leaving a
/// clean 0 state the next acquire (reader or writer) can take.
fn test_protocol_write_exit_clean() {
    let rw = ModelRw::new();
    rw.write_enter();
    rw.write_exit();
    assert_eq!(rw.state.load(Ordering::Acquire), 0);
    assert!(rw.read_may_enter());
    rw.write_enter(); // immediate re-acquire works
    rw.write_exit();
    assert_eq!(rw.state.load(Ordering::Acquire), 0);
}
