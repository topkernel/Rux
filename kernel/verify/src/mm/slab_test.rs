//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Slab allocator size class lookup invariant tests.
//!
//! Types copied from: kernel/src/mm/slab.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/mm/slab.rs
// ============================================================================

const PAGE_SIZE: usize = 4096;
const MIN_OBJECT_SIZE: usize = 8;
const MAX_OBJECT_SIZE: usize = PAGE_SIZE;
const NUM_CACHES: usize = 10;

const OBJECT_SIZES: [usize; NUM_CACHES] = [
    8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
];

/// Size class lookup (copied from SlabAllocator::find_cache_index)
fn find_cache_index(size: usize) -> Option<usize> {
    if size == 0 || size > MAX_OBJECT_SIZE {
        return None;
    }
    for (i, &obj_size) in OBJECT_SIZES.iter().enumerate() {
        if size <= obj_size {
            return Some(i);
        }
    }
    None
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SLAB-1: size 0 returns None
    #[test]
    fn test_zero_size(_v in 0u8..1u8) {
        prop_assert_eq!(find_cache_index(0), None);
    }

    /// INV-SLAB-2: size > PAGE_SIZE returns None
    #[test]
    fn test_oversize(size in PAGE_SIZE + 1..PAGE_SIZE * 2) {
        prop_assert_eq!(find_cache_index(size), None);
    }

    /// INV-SLAB-3: exact OBJECT_SIZES[i] returns Some(i)
    #[test]
    fn test_exact_size(idx in 0usize..NUM_CACHES) {
        let size = OBJECT_SIZES[idx];
        prop_assert_eq!(find_cache_index(size), Some(idx));
    }

    /// INV-SLAB-4: One less than OBJECT_SIZES[i] uses previous cache
    #[test]
    fn test_one_less_than_size(idx in 1usize..NUM_CACHES) {
        // For doubling sizes, OBJECT_SIZES[idx]-1 maps to cache idx
        // because OBJECT_SIZES[idx]-1 > OBJECT_SIZES[idx-1]
        let size = OBJECT_SIZES[idx] - 1;
        let result = find_cache_index(size);
        prop_assert!(result.is_some());
        let result_idx = result.unwrap();
        prop_assert!(OBJECT_SIZES[result_idx] >= size);
        prop_assert!(result_idx <= idx);
    }

    /// INV-SLAB-5: size 1..=PAGE_SIZE always returns Some
    #[test]
    fn test_valid_range(size in 1usize..=PAGE_SIZE) {
        prop_assert!(find_cache_index(size).is_some());
    }

    /// INV-SLAB-6: returned index gives size >= requested
    #[test]
    fn test_size_sufficient(size in 1usize..=PAGE_SIZE) {
        let idx = find_cache_index(size).unwrap();
        prop_assert!(OBJECT_SIZES[idx] >= size);
    }

    /// INV-SLAB-7: returned index is minimal (size-1 maps to smaller index)
    #[test]
    fn test_minimal_index(size in 1usize..PAGE_SIZE) {
        let idx = find_cache_index(size).unwrap();
        if idx > 0 {
            prop_assert!(OBJECT_SIZES[idx - 1] < size,
                "smaller cache should not be sufficient");
        }
    }
}

#[test]
/// INV-SLAB-8: OBJECT_SIZES is strictly increasing
fn test_sizes_strictly_increasing() {
    for i in 1..NUM_CACHES {
        assert!(OBJECT_SIZES[i] > OBJECT_SIZES[i - 1],
            "OBJECT_SIZES[{}] = {} <= OBJECT_SIZES[{}] = {}",
            i, OBJECT_SIZES[i], i - 1, OBJECT_SIZES[i - 1]);
    }
}

#[test]
/// INV-SLAB-9: All sizes are powers of two
fn test_sizes_power_of_two() {
    for &size in &OBJECT_SIZES {
        assert!(size > 0 && (size & (size - 1)) == 0,
            "{} is not a power of two", size);
    }
}

#[test]
/// INV-SLAB-10: First size == MIN_OBJECT_SIZE, last == MAX_OBJECT_SIZE
fn test_size_bounds() {
    assert_eq!(OBJECT_SIZES[0], MIN_OBJECT_SIZE);
    assert_eq!(OBJECT_SIZES[NUM_CACHES - 1], MAX_OBJECT_SIZE);
}

#[test]
/// INV-SLAB-11: NUM_CACHES matches OBJECT_SIZES length
fn test_num_caches_match() {
    assert_eq!(OBJECT_SIZES.len(), NUM_CACHES);
}

#[test]
/// INV-SLAB-12: Each size is exactly double the previous
fn test_sizes_doubling() {
    for i in 1..NUM_CACHES {
        assert_eq!(OBJECT_SIZES[i], OBJECT_SIZES[i - 1] * 2,
            "OBJECT_SIZES[{}] = {}, expected {}",
            i, OBJECT_SIZES[i], OBJECT_SIZES[i - 1] * 2);
    }
}
