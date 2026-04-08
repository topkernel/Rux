//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RwSpinlock bit layout invariant tests.
//!
//! Types copied from: kernel/src/sync/rwlock.rs

use proptest::prelude::*;

// ============================================================================
// Copied constants from kernel/src/sync/rwlock.rs
// ============================================================================

const WRITER_BIT: u32 = 1u32 << 31;
const READER_MASK: u32 = !WRITER_BIT;

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

    /// INV-RWLOCK-2: READER_MASK covers bits [30:0]
    #[test]
    fn test_reader_mask_position(_v in 0u8..1u8) {
        prop_assert_eq!(READER_MASK, 0x7FFFFFFF);
        prop_assert_eq!(READER_MASK.count_ones(), 31);
    }

    /// INV-RWLOCK-3: WRITER_BIT and READER_MASK are disjoint
    #[test]
    fn test_disjoint(_v in 0u8..1u8) {
        prop_assert_eq!(WRITER_BIT & READER_MASK, 0);
    }

    /// INV-RWLOCK-4: WRITER_BIT | READER_MASK covers all 32 bits
    #[test]
    fn test_full_coverage(_v in 0u8..1u8) {
        prop_assert_eq!(WRITER_BIT | READER_MASK, u32::MAX);
    }

    /// INV-RWLOCK-5: Setting reader count does not affect writer bit
    #[test]
    fn test_reader_no_writer(reader_count in 0u32..0x80000000u32) {
        let state = reader_count & READER_MASK;
        prop_assert_eq!(state & WRITER_BIT, 0);
        prop_assert_eq!(state & READER_MASK, reader_count);
    }

    /// INV-RWLOCK-6: Setting writer bit preserves reader count zero
    #[test]
    fn test_writer_clears_readers(reader_count in 0u32..0x80000000u32) {
        let state_with_writer = WRITER_BIT;
        prop_assert_eq!(state_with_writer & READER_MASK, 0);
        // Writer state with any reader count
        let state = WRITER_BIT | (reader_count & READER_MASK);
        prop_assert!(state & WRITER_BIT != 0);
    }

    /// INV-RWLOCK-7: Reader count is extractable via mask
    #[test]
    fn test_reader_extract(
        readers in 0u32..0x80000000u32,
        writer in 0u8..2u8,
    ) {
        let rc = readers & READER_MASK;
        let state = rc | (WRITER_BIT * (writer as u32));
        let extracted_readers = state & READER_MASK;
        let has_writer = (state & WRITER_BIT) != 0;
        prop_assert_eq!(extracted_readers, rc);
        prop_assert_eq!(has_writer, writer != 0);
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
        assert_eq!((readers | WRITER_BIT) & READER_MASK, readers);
    }
}

#[test]
/// INV-RWLOCK-10: Maximum reader count (without writer) fits in 31 bits
fn test_max_reader_count() {
    let max_readers = READER_MASK; // 0x7FFFFFFF = 2^31 - 1
    assert_eq!(max_readers.count_ones(), 31);
    assert!(max_readers < WRITER_BIT);
}
