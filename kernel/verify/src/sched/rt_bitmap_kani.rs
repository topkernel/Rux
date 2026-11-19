//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for RT scheduler bitmap priority scan.
//!
//! Types copied from: kernel/src/sched/rt.rs

#![cfg(kani)]

fn find_highest_prio(bitmap: &[u64; 2]) -> Option<u32> {
    if bitmap[0] != 0 {
        return Some(bitmap[0].trailing_zeros());
    }
    if bitmap[1] != 0 {
        return Some(bitmap[1].trailing_zeros() + 64);
    }
    None
}

/// INV-RT-K1: empty bitmap returns None.
#[kani::proof]
fn verify_empty_bitmap() {
    let bitmap = [0u64; 2];
    assert_eq!(find_highest_prio(&bitmap), None);
}

/// INV-RT-K2: single bit in word0 returns correct index.
#[kani::proof]
fn verify_single_bit_word0() {
    let bit: u32 = kani::any();
    kani::assume(bit < 64);
    let mut bitmap = [0u64; 2];
    bitmap[0] = 1u64 << bit;
    assert_eq!(find_highest_prio(&bitmap), Some(bit));
}

/// INV-RT-K3: single bit in word1 returns correct offset index.
#[kani::proof]
fn verify_single_bit_word1() {
    let bit: u32 = kani::any();
    kani::assume(bit < 36);
    let mut bitmap = [0u64; 2];
    bitmap[1] = 1u64 << bit;
    assert_eq!(find_highest_prio(&bitmap), Some(bit + 64));
}

/// INV-RT-K4: word0 has priority over word1.
#[kani::proof]
fn verify_word0_priority() {
    let w0_bit: u32 = kani::any();
    let w1_bit: u32 = kani::any();
    kani::assume(w0_bit < 64 && w1_bit < 36);
    let mut bitmap = [0u64; 2];
    bitmap[0] = 1u64 << w0_bit;
    bitmap[1] = 1u64 << w1_bit;
    assert_eq!(find_highest_prio(&bitmap), Some(w0_bit));
}

/// INV-RT-K5: all bits set in word0 returns 0 (highest priority).
#[kani::proof]
fn verify_all_set_word0() {
    let bitmap = [u64::MAX, 0u64];
    assert_eq!(find_highest_prio(&bitmap), Some(0));
}

/// INV-RT-K6: random bitmap consistency check.
#[kani::proof]
fn verify_random_bitmap() {
    let w0: u64 = kani::any();
    let w1: u64 = kani::any();
    let bitmap = [w0, w1];
    let result = find_highest_prio(&bitmap);
    if w0 != 0 {
        assert_eq!(result, Some(w0.trailing_zeros()));
    } else if w1 != 0 {
        assert_eq!(result, Some(w1.trailing_zeros() + 64));
    } else {
        assert_eq!(result, None);
    }
}
