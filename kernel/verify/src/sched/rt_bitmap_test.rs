//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RT scheduler bitmap invariant tests.
//!
//! Functions copied from: kernel/src/sched/rt.rs

use proptest::prelude::*;

// ============================================================================
// Copied functions from kernel/src/sched/rt.rs
// ============================================================================

const MAX_RT_PRIO: usize = 100;

fn find_highest_prio(bitmap: &[u64; 2]) -> Option<u32> {
    let word0 = bitmap[0];
    if word0 != 0 {
        return Some(word0.trailing_zeros());
    }

    let word1 = bitmap[1];
    if word1 != 0 {
        return Some(word1.trailing_zeros() + 64);
    }

    None
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-RT-BITMAP-1: empty bitmap returns None
    #[test]
    fn test_empty_bitmap(_v in 0u8..1u8) {
        let bitmap = [0u64; 2];
        prop_assert_eq!(find_highest_prio(&bitmap), None);
    }

    /// INV-RT-BITMAP-2: single bit set in word0
    #[test]
    fn test_single_bit_word0(bit in 0u32..64u32) {
        let mut bitmap = [0u64; 2];
        bitmap[0] = 1u64 << bit;
        prop_assert_eq!(find_highest_prio(&bitmap), Some(bit));
    }

    /// INV-RT-BITMAP-3: single bit set in word1
    #[test]
    fn test_single_bit_word1(bit in 0u32..36u32) {
        let mut bitmap = [0u64; 2];
        bitmap[1] = 1u64 << bit;
        prop_assert_eq!(find_highest_prio(&bitmap), Some(bit + 64));
    }

    /// INV-RT-BITMAP-4: word0 has priority over word1
    #[test]
    fn test_word0_priority(
        w0_bit in 0u32..64u32,
        w1_bit in 0u32..36u32,
    ) {
        let mut bitmap = [0u64; 2];
        bitmap[0] = 1u64 << w0_bit;
        bitmap[1] = 1u64 << w1_bit;
        prop_assert_eq!(find_highest_prio(&bitmap), Some(w0_bit));
    }

    /// INV-RT-BITMAP-5: lowest set bit in word0 wins
    #[test]
    fn test_lowest_bit_wins(
        bits in prop::collection::vec(0u32..64u32, 2..10),
    ) {
        let mut bitmap = [0u64; 2];
        let mut expected = u32::MAX;
        for &bit in &bits {
            bitmap[0] |= 1u64 << bit;
            if bit < expected {
                expected = bit;
            }
        }
        prop_assert_eq!(find_highest_prio(&bitmap), Some(expected));
    }

    /// INV-RT-BITMAP-6: lowest set bit in word1 wins when word0 is empty
    #[test]
    fn test_lowest_bit_w1(
        bits in prop::collection::vec(0u32..36u32, 2..10),
    ) {
        let mut bitmap = [0u64; 2];
        bitmap[0] = 0;
        let mut expected = u32::MAX;
        for &bit in &bits {
            bitmap[1] |= 1u64 << bit;
            if bit < expected {
                expected = bit;
            }
        }
        prop_assert_eq!(find_highest_prio(&bitmap), Some(expected + 64));
    }

    /// INV-RT-BITMAP-7: all bits set in word0 returns 0
    #[test]
    fn test_all_set_word0(_v in 0u8..1u8) {
        let bitmap = [u64::MAX, 0u64];
        prop_assert_eq!(find_highest_prio(&bitmap), Some(0));
    }

    /// INV-RT-BITMAP-8: all bits set in both words returns 0
    #[test]
    fn test_all_set_both(_v in 0u8..1u8) {
        let bitmap = [u64::MAX, u64::MAX];
        prop_assert_eq!(find_highest_prio(&bitmap), Some(0));
    }

    /// INV-RT-BITMAP-9: only highest bit set in word0
    #[test]
    fn test_high_bit_word0(_v in 0u8..1u8) {
        let bitmap = [1u64 << 63, 0u64];
        prop_assert_eq!(find_highest_prio(&bitmap), Some(63));
    }

    /// INV-RT-BITMAP-10: only highest valid bit in word1
    #[test]
    fn test_high_bit_word1(_v in 0u8..1u8) {
        // Only 36 bits valid (100 - 64 = 36)
        let bitmap = [0u64, 1u64 << 35];
        prop_assert_eq!(find_highest_prio(&bitmap), Some(99));
    }

    /// INV-RT-BITMAP-11: random bitmap consistency
    #[test]
    fn test_random_bitmap(
        w0 in 0u64..u64::MAX,
        w1 in 0u64..(1u64 << 36),
    ) {
        let bitmap = [w0, w1];
        let result = find_highest_prio(&bitmap);

        if w0 != 0 {
            let expected = w0.trailing_zeros();
            prop_assert_eq!(result, Some(expected));
        } else if w1 != 0 {
            let expected = w1.trailing_zeros() + 64;
            prop_assert_eq!(result, Some(expected));
        } else {
            prop_assert_eq!(result, None);
        }
    }
}
