//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for POSIX.1e Capability bitmask operations.
//!
//! Types copied from: kernel/src/security/capability.rs

#![cfg(kani)]

pub const CAP_VALID_MASK: u64 = (1u64 << 41) - 1;
pub const CAP_LAST_CAP: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cap(pub u64);

impl Cap {
    pub const EMPTY: Cap = Cap(0);
    pub const FULL: Cap = Cap(CAP_VALID_MASK);

    pub fn new(mask: u64) -> Self { Cap(mask & CAP_VALID_MASK) }
    pub fn has(&self, cap: u32) -> bool {
        if cap > CAP_LAST_CAP { return false; }
        (self.0 >> cap) & 1 == 1
    }
    pub fn set(&mut self, cap: u32) {
        if cap <= CAP_LAST_CAP { self.0 |= 1u64 << cap; }
    }
    pub fn clear(&mut self, cap: u32) {
        if cap <= CAP_LAST_CAP { self.0 &= !(1u64 << cap); }
    }
    pub fn intersect(&self, other: Cap) -> Cap { Cap(self.0 & other.0) }
    pub fn union(&self, other: Cap) -> Cap { Cap(self.0 | other.0) }
    pub fn complement(&self) -> Cap { Cap(CAP_VALID_MASK & !self.0) }
    pub fn is_subset_of(&self, other: Cap) -> bool { (self.0 & !other.0) == 0 }
    pub fn is_empty(&self) -> bool { self.0 == 0 }
    pub fn bits(&self) -> u64 { self.0 }
}

/// INV-CAP-K1: new(mask) masks to valid 41 bits.
#[kani::proof]
fn verify_new_masks() {
    let mask: u64 = kani::any();
    let c = Cap::new(mask);
    assert_eq!(c.bits(), mask & CAP_VALID_MASK);
}

/// INV-CAP-K2: set(x); has(x) for valid cap 0..=40.
#[kani::proof]
fn verify_set_has() {
    let cap: u32 = kani::any();
    kani::assume(cap <= 40);
    let mut c = Cap::EMPTY;
    c.set(cap);
    assert!(c.has(cap));
}

/// INV-CAP-K3: set(x); clear(x); !has(x) roundtrip.
#[kani::proof]
fn verify_set_clear_roundtrip() {
    let cap: u32 = kani::any();
    kani::assume(cap <= 40);
    let mut c = Cap::EMPTY;
    c.set(cap);
    c.clear(cap);
    assert!(!c.has(cap));
}

/// INV-CAP-K4: intersect = bitwise AND.
#[kani::proof]
fn verify_intersect() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a <= CAP_VALID_MASK && b <= CAP_VALID_MASK);
    assert_eq!(Cap::new(a).intersect(Cap::new(b)).bits(), a & b);
}

/// INV-CAP-K5: union = bitwise OR.
#[kani::proof]
fn verify_union() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a <= CAP_VALID_MASK && b <= CAP_VALID_MASK);
    assert_eq!(Cap::new(a).union(Cap::new(b)).bits(), a | b);
}

/// INV-CAP-K6: complement(complement(c)) == c (involution).
#[kani::proof]
fn verify_complement_involution() {
    let mask: u64 = kani::any();
    kani::assume(mask <= CAP_VALID_MASK);
    let c = Cap::new(mask);
    assert_eq!(c.complement().complement(), c);
}

/// INV-CAP-K7: EMPTY subset of any, FULL subset of FULL.
#[kani::proof]
fn verify_subset_trivial() {
    let mask: u64 = kani::any();
    kani::assume(mask <= CAP_VALID_MASK);
    let full = Cap::FULL;
    let empty = Cap::EMPTY;
    let c = Cap::new(mask);
    assert!(full.is_subset_of(full));
    assert!(empty.is_subset_of(c));
}

/// INV-CAP-K8: has(cap > 40) always false.
#[kani::proof]
fn verify_has_invalid_cap() {
    let cap: u32 = kani::any();
    kani::assume(cap > 40 && cap < 100);
    let mut c = Cap::EMPTY;
    c.set(cap);
    assert!(!c.has(cap));
    assert!(!Cap::FULL.has(cap));
}

/// INV-CAP-K9: De Morgan's law: ~(A & B) == ~A | ~B.
#[kani::proof]
fn verify_de_morgan() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    kani::assume(a <= CAP_VALID_MASK && b <= CAP_VALID_MASK);
    let ca = Cap::new(a);
    let cb = Cap::new(b);
    let lhs = ca.intersect(cb).complement();
    let rhs = ca.complement().union(cb.complement());
    assert_eq!(lhs, rhs);
}
