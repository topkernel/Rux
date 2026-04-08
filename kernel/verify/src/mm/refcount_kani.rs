//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for page refcount/mapcount invariants.
//!
//! Types copied from: kernel/verify/src/mm/refcount_test.rs

#![cfg(kani)]

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

const PAGE_MAPCOUNT_BIAS: i32 = -1;

pub struct Page {
    _refcount: AtomicI32,
    _mapcount: AtomicI32,
}

impl Page {
    pub const fn new() -> Self {
        Self {
            _refcount: AtomicI32::new(0),
            _mapcount: AtomicI32::new(PAGE_MAPCOUNT_BIAS),
        }
    }

    pub fn refcount(&self) -> i32 { self._refcount.load(Ordering::Acquire) }
    pub fn get_page(&self) -> i32 { self._refcount.fetch_add(1, Ordering::AcqRel) + 1 }
    pub fn put_page(&self) -> i32 {
        let prev = self._refcount.fetch_sub(1, Ordering::AcqRel);
        let result = prev - 1;
        if result < 0 {
            self._refcount.fetch_add(1, Ordering::AcqRel);
        }
        result
    }
    pub fn set_refcount(&self, count: i32) { self._refcount.store(count, Ordering::Release); }

    pub fn mapcount(&self) -> i32 { self._mapcount.load(Ordering::Acquire) }
    pub fn add_mapcount(&self) -> i32 { self._mapcount.fetch_add(1, Ordering::AcqRel) + 1 }
    pub fn sub_mapcount(&self) -> i32 { self._mapcount.fetch_sub(1, Ordering::AcqRel) - 1 }
}

/// INV-REF-K1: refcount never goes negative after any get/put sequence.
/// Bounded loop: max 10 operations to keep CBMC tractable.
#[kani::proof]
fn verify_refcount_never_negative() {
    let initial: i32 = kani::any();
    kani::assume(initial >= 0 && initial <= 100);
    let page = Page::new();
    page.set_refcount(initial);

    // Symbolic sequence of up to 10 get/put operations
    let n_ops: usize = kani::any();
    kani::assume(n_ops <= 10);
    let is_get: [bool; 10] = kani::any();

    for i in 0..n_ops {
        if is_get[i] {
            page.get_page();
        } else {
            page.put_page();
        }
    }
    assert!(page.refcount() >= 0, "refcount went negative");
}

/// INV-REF-K2: put_page on zero refcount triggers underflow protection.
/// The stored value is restored to 0.
#[kani::proof]
fn verify_refcount_underflow_protection() {
    let page = Page::new(); // refcount starts at 0
    let result = page.put_page();
    assert!(result < 0, "underflow should return negative");
    assert_eq!(page.refcount(), 0, "refcount should be restored to 0");
}

/// INV-MAP-K1: Initial mapcount is PAGE_MAPCOUNT_BIAS (-1).
#[kani::proof]
fn verify_mapcount_initial() {
    let page = Page::new();
    assert_eq!(page.mapcount(), PAGE_MAPCOUNT_BIAS);
}

/// INV-MAP-K2: N maps + N unmaps → mapcount == PAGE_MAPCOUNT_BIAS.
/// Bounded to 20 operations.
#[kani::proof]
fn verify_mapcount_symmetry() {
    let page = Page::new();
    let n: usize = kani::any();
    kani::assume(n <= 20);
    for _ in 0..n {
        page.add_mapcount();
    }
    for _ in 0..n {
        page.sub_mapcount();
    }
    assert_eq!(page.mapcount(), PAGE_MAPCOUNT_BIAS);
}
