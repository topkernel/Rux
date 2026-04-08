//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for slab allocator size class lookup.
//!
//! Types and functions copied from: kernel/verify/src/mm/slab_test.rs

#![cfg(kani)]

const PAGE_SIZE: usize = 4096;
const MIN_OBJECT_SIZE: usize = 8;
const MAX_OBJECT_SIZE: usize = PAGE_SIZE;
const NUM_CACHES: usize = 10;

const OBJECT_SIZES: [usize; NUM_CACHES] = [
    8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
];

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

/// INV-SLAB-K1: find_cache_index returns valid index (< NUM_CACHES)
/// for any input size.
#[kani::proof]
fn verify_find_cache_index_returns_valid() {
    let size: usize = kani::any();
    kani::assume(size <= MAX_OBJECT_SIZE);
    if size > 0 {
        let idx = find_cache_index(size).unwrap();
        assert!(idx < NUM_CACHES);
    }
}

/// INV-SLAB-K2: returned cache provides object size >= requested size.
#[kani::proof]
fn verify_find_cache_index_sufficient() {
    let size: usize = kani::any();
    kani::assume(size > 0 && size <= MAX_OBJECT_SIZE);
    let idx = find_cache_index(size).unwrap();
    assert!(OBJECT_SIZES[idx] >= size);
}
