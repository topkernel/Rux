//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RT scheduler entity time_slice lifecycle and bitmap priority scan invariant tests.
//!
//! Types copied from: kernel/src/sched/rt.rs
//! NOTE: AtomicU32/AtomicBool replaced with plain types for std testing.

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/sched/rt.rs
// ============================================================================

pub const MAX_RT_PRIO: usize = 100;
pub const RR_TIMESLICE_MS: u32 = 100;

#[derive(Debug)]
pub struct SchedRtEntity {
    pub time_slice: u32,
    pub on_rq: bool,
}

impl SchedRtEntity {
    pub fn new() -> Self {
        Self {
            time_slice: RR_TIMESLICE_MS,
            on_rq: false,
        }
    }

    pub fn is_on_rq(&self) -> bool {
        self.on_rq
    }

    pub fn set_on_rq(&mut self, on_rq: bool) {
        self.on_rq = on_rq;
    }

    pub fn get_time_slice(&self) -> u32 {
        self.time_slice
    }

    pub fn set_time_slice(&mut self, slice: u32) {
        self.time_slice = slice;
    }

    pub fn dec_time_slice(&mut self) -> u32 {
        if self.time_slice > 0 {
            self.time_slice -= 1;
            self.time_slice
        } else {
            0
        }
    }

    pub fn reset_time_slice(&mut self) {
        self.time_slice = RR_TIMESLICE_MS;
    }
}

impl Default for SchedRtEntity {
    fn default() -> Self {
        Self::new()
    }
}

/// Find highest priority from bitmap (two u64 words).
/// Lower priority number = higher priority.
/// Priority 0-99 maps to: word0[0..63], word1[0..35].
fn find_highest_prio(word0: u64, word1: u64) -> Option<u32> {
    if word0 != 0 {
        return Some(word0.trailing_zeros());
    }
    if word1 != 0 {
        return Some(word1.trailing_zeros() + 64);
    }
    None
}

/// Priority to bitmap word index and bit index.
fn prio_to_bitmap(prio: usize) -> (usize, usize) {
    (prio / 64, prio % 64)
}

/// Set bit in bitmap word.
fn set_bitmap_bit(word0: &mut u64, word1: &mut u64, prio: usize) {
    let (word_idx, bit_idx) = prio_to_bitmap(prio);
    match word_idx {
        0 => *word0 |= 1u64 << bit_idx,
        1 => *word1 |= 1u64 << bit_idx,
        _ => {}
    }
}

/// Clear bit in bitmap word.
fn clear_bitmap_bit(word0: &mut u64, word1: &mut u64, prio: usize) {
    let (word_idx, bit_idx) = prio_to_bitmap(prio);
    match word_idx {
        0 => *word0 &= !(1u64 << bit_idx),
        1 => *word1 &= !(1u64 << bit_idx),
        _ => {}
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-RT-1: new entity has default time_slice and not on_rq
    #[test]
    fn test_new_entity(_v in 0u8..1u8) {
        let entity = SchedRtEntity::new();
        prop_assert_eq!(entity.get_time_slice(), RR_TIMESLICE_MS);
        prop_assert!(!entity.is_on_rq());
    }

    /// INV-RT-2: dec_time_slice decrements by 1
    #[test]
    fn test_dec_time_slice(init in 1u32..200u32) {
        let mut entity = SchedRtEntity::new();
        entity.set_time_slice(init);
        let after = entity.dec_time_slice();
        prop_assert_eq!(after, init - 1);
        prop_assert_eq!(entity.get_time_slice(), init - 1);
    }

    /// INV-RT-3: dec_time_slice at 0 returns 0, stays at 0
    #[test]
    fn test_dec_at_zero(_v in 0u8..1u8) {
        let mut entity = SchedRtEntity::new();
        entity.set_time_slice(0);
        let result = entity.dec_time_slice();
        prop_assert_eq!(result, 0);
        prop_assert_eq!(entity.get_time_slice(), 0);
    }

    /// INV-RT-4: reset_time_slice restores default
    #[test]
    fn test_reset_time_slice(arbitrary in 0u32..500u32) {
        let mut entity = SchedRtEntity::new();
        entity.set_time_slice(arbitrary);
        entity.reset_time_slice();
        prop_assert_eq!(entity.get_time_slice(), RR_TIMESLICE_MS);
    }

    /// INV-RT-5: set_on_rq / is_on_rq roundtrip
    #[test]
    fn test_on_rq_roundtrip(_v in 0u8..1u8) {
        let mut entity = SchedRtEntity::new();
        prop_assert!(!entity.is_on_rq());
        entity.set_on_rq(true);
        prop_assert!(entity.is_on_rq());
        entity.set_on_rq(false);
        prop_assert!(!entity.is_on_rq());
    }

    /// INV-RT-6: dec_time_slice 100 times from default reaches 0
    #[test]
    fn test_exhaust_timeslice(_v in 0u8..1u8) {
        let mut entity = SchedRtEntity::new();
        for _ in 0..RR_TIMESLICE_MS {
            entity.dec_time_slice();
        }
        prop_assert_eq!(entity.get_time_slice(), 0);
        let extra = entity.dec_time_slice();
        prop_assert_eq!(extra, 0);
    }

    /// INV-RT-7: set_time_slice sets exact value
    #[test]
    fn test_set_time_slice(val in 0u32..1000u32) {
        let mut entity = SchedRtEntity::new();
        entity.set_time_slice(val);
        prop_assert_eq!(entity.get_time_slice(), val);
    }

    /// INV-RT-8: find_highest_prio finds lowest set bit in word0
    #[test]
    fn test_find_highest_prio_word0(bit in 0u32..64u32) {
        let word0 = 1u64 << bit;
        let result = find_highest_prio(word0, 0).unwrap();
        prop_assert_eq!(result, bit);
    }

    /// INV-RT-9: find_highest_prio word1 adds 64 offset
    #[test]
    fn test_find_highest_prio_word1(bit in 0u32..36u32) {
        let word1 = 1u64 << bit;
        let result = find_highest_prio(0, word1).unwrap();
        prop_assert_eq!(result, bit + 64);
    }

    /// INV-RT-10: find_highest_prio returns lowest priority number
    #[test]
    fn test_find_highest_prio_lowest_wins(
        bit_low in 0u32..32u32,
        bit_high in 33u32..63u32,
    ) {
        let word0 = (1u64 << bit_low) | (1u64 << bit_high);
        let result = find_highest_prio(word0, 0).unwrap();
        prop_assert_eq!(result, bit_low);
    }

    /// INV-RT-11: find_highest_prio empty bitmap returns None
    #[test]
    fn test_find_highest_prio_empty(_v in 0u8..1u8) {
        prop_assert!(find_highest_prio(0, 0).is_none());
    }

    /// INV-RT-12: set/clear bitmap + find_highest_prio roundtrip
    #[test]
    fn test_bitmap_set_clear_find(
        prio in 0usize..100usize,
    ) {
        let (mut w0, mut w1) = (0u64, 0u64);
        set_bitmap_bit(&mut w0, &mut w1, prio);
        let found = find_highest_prio(w0, w1).unwrap();
        prop_assert_eq!(found as usize, prio);

        clear_bitmap_bit(&mut w0, &mut w1, prio);
        // If this was the only bit, should be None
        if w0 == 0 && w1 == 0 {
            prop_assert!(find_highest_prio(w0, w1).is_none());
        } else {
            prop_assert_ne!(find_highest_prio(w0, w1), Some(prio as u32));
        }
    }

    /// INV-RT-13: prio_to_bitmap correctly maps 0..99 to word/bit
    #[test]
    fn test_prio_to_bitmap(prio in 0usize..100usize) {
        let (word_idx, bit_idx) = prio_to_bitmap(prio);
        if prio < 64 {
            prop_assert_eq!(word_idx, 0);
            prop_assert_eq!(bit_idx, prio);
        } else {
            prop_assert_eq!(word_idx, 1);
            prop_assert_eq!(bit_idx, prio - 64);
        }
    }

    /// INV-RT-14: MAX_RT_PRIO is 100, valid priorities 0..99
    #[test]
    fn test_max_rt_prio(_v in 0u8..1u8) {
        prop_assert_eq!(MAX_RT_PRIO, 100);
    }

    /// INV-RT-15: interleaved set/clear preserves bitmap correctness
    #[test]
    fn test_interleaved_bitmap_ops(
        ops in proptest::collection::vec(
            proptest::bool::ANY,
            0..50
        ),
        seed in 0usize..100usize,
    ) {
        let (mut w0, mut w1) = (0u64, 0u64);
        let mut added: Vec<usize> = Vec::new();

        for (i, do_add) in ops.iter().enumerate() {
            let prio = (seed + i) % MAX_RT_PRIO;
            if *do_add {
                set_bitmap_bit(&mut w0, &mut w1, prio);
                added.push(prio);
            } else if let Some(p) = added.pop() {
                clear_bitmap_bit(&mut w0, &mut w1, p);
            }
        }

        // Verify find_highest_prio matches minimum of remaining
        let found = find_highest_prio(w0, w1);
        if added.is_empty() {
            prop_assert!(found.is_none());
        } else {
            let expected = *added.iter().min().unwrap() as u32;
            prop_assert_eq!(found, Some(expected));
        }
    }

    /// INV-RT-16: time_slice never underflows with repeated dec
    #[test]
    fn test_no_underflow(
        init in 0u32..10u32,
        steps in 10usize..200usize,
    ) {
        let mut entity = SchedRtEntity::new();
        entity.set_time_slice(init);
        for _ in 0..steps {
            entity.dec_time_slice();
        }
        // steps > init, so should reach 0 and stay there (no wrapping)
        prop_assert_eq!(entity.get_time_slice(), 0);
    }
}
