//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for SignalMask and signal set operations.
//!
//! Types copied from: kernel/src/signal.rs

#![cfg(kani)]

pub type SigSet = u64;

pub fn sigset_add(set: &mut SigSet, sig: i32) {
    if sig >= 1 && sig <= 64 { *set |= 1u64 << ((sig - 1) as u32); }
}

pub fn sigset_has(set: SigSet, sig: i32) -> bool {
    if sig >= 1 && sig <= 64 { (set & (1u64 << ((sig - 1) as u32))) != 0 } else { false }
}

pub fn sigset_remove(set: &mut SigSet, sig: i32) {
    if sig >= 1 && sig <= 64 { *set &= !(1u64 << ((sig - 1) as u32)); }
}

pub fn sigset_clear(set: &mut SigSet) { *set = 0; }

pub fn sigset_first(set: SigSet) -> Option<i32> {
    if set == 0 { return None; }
    Some(set.trailing_zeros() as i32 + 1)
}

/// INV-SIGSET-K1: add → has returns true for valid signals.
#[kani::proof]
fn verify_sigset_add_has() {
    let sig: i32 = kani::any();
    kani::assume(sig >= 1 && sig <= 64);
    let mut set: SigSet = 0;
    sigset_add(&mut set, sig);
    assert!(sigset_has(set, sig));
}

/// INV-SIGSET-K2: add → remove → has returns false.
#[kani::proof]
fn verify_sigset_add_remove() {
    let sig: i32 = kani::any();
    kani::assume(sig >= 1 && sig <= 64);
    let mut set: SigSet = 0;
    sigset_add(&mut set, sig);
    sigset_remove(&mut set, sig);
    assert!(!sigset_has(set, sig));
}

/// INV-SIGSET-K3: clear makes set empty.
#[kani::proof]
fn verify_sigset_clear() {
    let sig1: i32 = kani::any();
    let sig2: i32 = kani::any();
    let sig3: i32 = kani::any();
    kani::assume(sig1 >= 1 && sig1 <= 64);
    kani::assume(sig2 >= 1 && sig2 <= 64);
    kani::assume(sig3 >= 1 && sig3 <= 64);
    let mut set: SigSet = 0;
    sigset_add(&mut set, sig1);
    sigset_add(&mut set, sig2);
    sigset_add(&mut set, sig3);
    sigset_clear(&mut set);
    assert_eq!(set, 0);
    assert!(sigset_first(set).is_none());
}

/// INV-SIGSET-K4: has returns false for out-of-range signals.
#[kani::proof]
fn verify_sigset_out_of_range() {
    let mut set: SigSet = 0;
    sigset_add(&mut set, 0);
    sigset_add(&mut set, 65);
    sigset_add(&mut set, 100);
    assert!(!sigset_has(set, 0));
    assert!(!sigset_has(set, 65));
    assert!(!sigset_has(set, 100));
    assert_eq!(set, 0);
}

/// INV-SIGSET-K5: SigSet can represent all 64 signals (u64::MAX).
#[kani::proof]
fn verify_sigset_capacity() {
    let mut set: SigSet = 0;
    for sig in 1..=64 {
        sigset_add(&mut set, sig);
    }
    assert_eq!(set, u64::MAX);
    for sig in 1..=64 {
        assert!(sigset_has(set, sig));
    }
}
