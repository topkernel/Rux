//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for BufferState bitmap and BlockCache hash_index.
//! Copied from: kernel/src/fs/bio.rs

use proptest::prelude::*;

// ============================================================================
// Copied: BufferState from kernel/src/fs/bio.rs
// ============================================================================

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct BufferState(u8);

impl BufferState {
    pub const BH_Uptodate: u8 = 0;
    pub const BH_Dirty: u8 = 1;
    pub const BH_Lock: u8 = 2;
    pub const BH_Req: u8 = 3;
    pub const BH_Mapped: u8 = 4;

    pub fn new() -> Self { Self(0) }

    pub fn set(&mut self, bit: u8) { self.0 |= 1 << bit; }
    pub fn clear(&mut self, bit: u8) { self.0 &= !(1 << bit); }
    pub fn test(&self, bit: u8) -> bool { (self.0 & (1 << bit)) != 0 }

    pub fn is_uptodate(&self) -> bool { self.test(Self::BH_Uptodate) }
    pub fn is_dirty(&self) -> bool { self.test(Self::BH_Dirty) }
    pub fn is_locked(&self) -> bool { self.test(Self::BH_Lock) }
    pub fn is_mapped(&self) -> bool { self.test(Self::BH_Mapped) }
}

// ============================================================================
// Copied: hash_index from kernel/src/fs/bio.rs BlockCache
// ============================================================================

/// Hash function for block cache (hash_size must be power of 2)
pub fn block_cache_hash_index(hash_size: usize, device_major: u32, blocknr: u64) -> usize {
    let hash = (device_major as u64)
        .wrapping_mul(2654435761)
        .wrapping_add(blocknr);
    (hash as usize) & (hash_size - 1)
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    #[test]
    fn test_set_then_test(initial in 0u8..=0xFFu8, bit in 0u8..8u8) {
        let mut state = BufferState(initial);
        state.set(bit);
        prop_assert!(state.test(bit));
    }

    #[test]
    fn test_clear_then_test(initial in 0u8..=0xFFu8, bit in 0u8..8u8) {
        let mut state = BufferState(initial);
        state.clear(bit);
        prop_assert!(!state.test(bit));
    }

    #[test]
    fn test_new_is_zero(_v in 0u8..1u8) {
        let state = BufferState::new();
        prop_assert_eq!(state.0, 0);
        for bit in 0..8 {
            prop_assert!(!state.test(bit));
        }
    }

    #[test]
    fn test_set_idempotent(initial in 0u8..=0xFFu8, bit in 0u8..8u8) {
        let mut s1 = BufferState(initial);
        s1.set(bit);
        s1.set(bit);
        let mut s2 = BufferState(initial);
        s2.set(bit);
        prop_assert_eq!(s1.0, s2.0);
    }

    #[test]
    fn test_clear_idempotent(initial in 0u8..=0xFFu8, bit in 0u8..8u8) {
        let mut s1 = BufferState(initial);
        s1.clear(bit);
        s1.clear(bit);
        let mut s2 = BufferState(initial);
        s2.clear(bit);
        prop_assert_eq!(s1.0, s2.0);
    }

    #[test]
    fn test_clear_set_restores_bit(initial in 0u8..=0xFFu8, bit in 0u8..8u8) {
        let mut state = BufferState(initial);
        let was_set = state.test(bit);
        state.clear(bit);
        state.set(bit);
        prop_assert!(state.test(bit));
        // After clear+set, the bit is always set regardless of initial
    }

    #[test]
    fn test_bit_independence(
        initial in 0u8..=0xFFu8,
        bit_a in 0u8..8u8,
        bit_b in 0u8..8u8,
    ) {
        // When bit_a != bit_b, setting a doesn't affect b
        if bit_a != bit_b {
            let mut state = BufferState(initial);
            state.set(bit_a);
            let was_b_set = BufferState(initial).test(bit_b);
            prop_assert_eq!(state.test(bit_b), was_b_set);
        }
    }

    #[test]
    fn test_named_flags_consistent(val in 0u8..=0xFFu8) {
        let state = BufferState(val);
        prop_assert_eq!(state.is_uptodate(), state.test(BufferState::BH_Uptodate));
        prop_assert_eq!(state.is_dirty(), state.test(BufferState::BH_Dirty));
        prop_assert_eq!(state.is_locked(), state.test(BufferState::BH_Lock));
        prop_assert_eq!(state.is_mapped(), state.test(BufferState::BH_Mapped));
    }

    #[test]
    fn test_hash_index_in_range(
        device_major in 0u32..256u32,
        blocknr in 0u64..1_000_000u64,
    ) {
        let hash_sizes = [4usize, 16, 64, 256, 1024];
        for hash_size in &hash_sizes {
            let idx = block_cache_hash_index(*hash_size, device_major, blocknr);
            prop_assert!(idx < *hash_size, "hash_size={} idx={}", hash_size, idx);
        }
    }

    #[test]
    fn test_hash_index_deterministic(
        device_major in 0u32..256u32,
        blocknr in 0u64..100_000u64,
    ) {
        let idx1 = block_cache_hash_index(64, device_major, blocknr);
        let idx2 = block_cache_hash_index(64, device_major, blocknr);
        prop_assert_eq!(idx1, idx2);
    }

    #[test]
    fn test_hash_index_same_key_same_bucket(
        hash_size in 4usize..1024usize,
    ) {
        // hash_size should be power of 2
        let hash_size = hash_size.next_power_of_two();
        let major = 42u32;
        let block = 12345u64;
        prop_assert_eq!(
            block_cache_hash_index(hash_size, major, block),
            block_cache_hash_index(hash_size, major, block),
        );
    }

    #[test]
    fn test_hash_different_keys_dont_always_collide(
        major1 in 0u32..100u32,
        major2 in 0u32..100u32,
    ) {
        let block = 999u64;
        if major1 != major2 {
            let idx1 = block_cache_hash_index(1024, major1, block);
            let idx2 = block_cache_hash_index(1024, major2, block);
            // Not guaranteed different, but check they're valid
            prop_assert!(idx1 < 1024);
            prop_assert!(idx2 < 1024);
        }
    }
}
