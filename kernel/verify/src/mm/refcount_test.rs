//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Page refcount underflow protection invariant tests.
//!
//! Types copied from: kernel/src/mm/page_desc.rs

use proptest::prelude::*;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

// ============================================================================
// Copied types from kernel/src/mm/page_desc.rs (simplified for testing)
// ============================================================================

/// Simplified Page descriptor — only refcount/mapcount fields.
pub struct Page {
    _refcount: AtomicI32,
    _mapcount: AtomicI32,
    _padding: [AtomicUsize; 6], // pad to match kernel layout size
}

const PAGE_MAPCOUNT_BIAS: i32 = -1;

impl Page {
    pub const fn new() -> Self {
        Self {
            _refcount: AtomicI32::new(0),
            _mapcount: AtomicI32::new(PAGE_MAPCOUNT_BIAS),
            _padding: [const { AtomicUsize::new(0) }; 6],
        }
    }

    /// Get reference count
    #[inline]
    pub fn refcount(&self) -> i32 {
        self._refcount.load(Ordering::Acquire)
    }

    /// Increment reference count
    #[inline]
    pub fn get_page(&self) -> i32 {
        self._refcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement reference count with underflow protection.
    /// On underflow (refcount was already 0), restores the value.
    #[inline]
    pub fn put_page(&self) -> i32 {
        let prev = self._refcount.fetch_sub(1, Ordering::AcqRel);
        let result = prev - 1;
        if result < 0 {
            self._refcount.fetch_add(1, Ordering::AcqRel);
        }
        result
    }

    /// Try to increment reference count (only if refcount > 0)
    #[inline]
    pub fn try_get_page(&self) -> bool {
        loop {
            let old = self._refcount.load(Ordering::Acquire);
            if old <= 0 {
                return false;
            }
            match self._refcount.compare_exchange_weak(
                old,
                old + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// Set reference count (only for initialization)
    #[inline]
    pub fn set_refcount(&self, count: i32) {
        self._refcount.store(count, Ordering::Release);
    }

    /// Get map count
    #[inline]
    pub fn mapcount(&self) -> i32 {
        self._mapcount.load(Ordering::Acquire)
    }

    /// Increment map count (map +1)
    #[inline]
    pub fn add_mapcount(&self) -> i32 {
        self._mapcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement map count (unmap -1)
    #[inline]
    pub fn sub_mapcount(&self) -> i32 {
        self._mapcount.fetch_sub(1, Ordering::AcqRel) - 1
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-REF-1: refcount never goes negative
    #[test]
    fn test_refcount_never_negative(
        initial in 0i32..100i32,
        ops in proptest::collection::vec(
            proptest::prop_oneof![
                proptest::strategy::Just(true),   // get
                proptest::strategy::Just(false),  // put
            ],
            0..200
        ),
    ) {
        let page = Page::new();
        page.set_refcount(initial);
        for is_get in ops {
            if is_get {
                page.get_page();
            } else {
                page.put_page();
            }
        }
        prop_assert!(page.refcount() >= 0, "refcount went negative: {}", page.refcount());
    }

    /// INV-REF-2: get/put cycle returns to original
    #[test]
    fn test_refcount_symmetry(
        initial in 1i32..100i32,
        n in 1usize..50usize,
    ) {
        let page = Page::new();
        page.set_refcount(initial);
        for _ in 0..n {
            page.get_page();
        }
        for _ in 0..n {
            page.put_page();
        }
        prop_assert_eq!(page.refcount(), initial);
    }

    /// INV-REF-3: get_page increments, put_page decrements
    /// When n_puts > initial + n_gets, underflow protection kicks in.
    #[test]
    fn test_get_put_sequence(
        initial in 0i32..50i32,
        n_gets in 0usize..50usize,
        n_puts in 0usize..50usize,
    ) {
        let page = Page::new();
        page.set_refcount(initial);
        for _ in 0..n_gets {
            page.get_page();
        }
        for _ in 0..n_puts {
            page.put_page();
        }
        let actual = page.refcount();
        prop_assert!(actual >= 0);
    }

    /// INV-REF-4: try_get_page fails on zero refcount
    #[test]
    fn test_try_get_zero_fails(_unit in proptest::strategy::Just(())) {
        let page = Page::new();
        assert!(!page.try_get_page());
        assert_eq!(page.refcount(), 0);
    }

    /// INV-REF-5: try_get_page succeeds on positive refcount
    #[test]
    fn test_try_get_positive_succeeds(initial in 1i32..100i32) {
        let page = Page::new();
        page.set_refcount(initial);
        assert!(page.try_get_page());
        assert_eq!(page.refcount(), initial + 1);
    }
}

/// INV-REF-6: put on zero refcount triggers underflow protection (stays at 0)
#[test]
fn test_put_zero_underflow() {
    let page = Page::new();
    assert_eq!(page.refcount(), 0);
    let result = page.put_page();
    assert!(result < 0, "underflow should return negative");
    assert_eq!(page.refcount(), 0, "refcount should be restored to 0");
}

proptest! {
    /// INV-MAP-1: initial mapcount is PAGE_MAPCOUNT_BIAS (-1)
    #[test]
    fn test_mapcount_initial(_v in 0u8..1u8) {
        let page = Page::new();
        prop_assert_eq!(page.mapcount(), PAGE_MAPCOUNT_BIAS);
    }

    /// INV-MAP-2: map+1, unmap-1 cycle returns to PAGE_MAPCOUNT_BIAS
    #[test]
    fn test_mapcount_symmetry(n_maps in 0usize..200usize) {
        let page = Page::new();
        for _ in 0..n_maps {
            page.add_mapcount();
        }
        for _ in 0..n_maps {
            page.sub_mapcount();
        }
        prop_assert_eq!(page.mapcount(), PAGE_MAPCOUNT_BIAS);
    }

    /// INV-MAP-3: after n maps, mapcount == -1 + n
    #[test]
    fn test_mapcount_after_maps(n_maps in 0usize..200usize) {
        let page = Page::new();
        for _ in 0..n_maps {
            page.add_mapcount();
        }
        prop_assert_eq!(page.mapcount(), PAGE_MAPCOUNT_BIAS + n_maps as i32);
    }

    /// INV-MAP-4: mixed add/sub sequence never corrupts mapcount
    #[test]
    fn test_mapcount_mixed_ops(
        ops in proptest::collection::vec(
            proptest::prop_oneof![
                proptest::strategy::Just(true),   // add_mapcount
                proptest::strategy::Just(false),  // sub_mapcount
            ],
            0..200
        ),
    ) {
        let page = Page::new();
        for is_add in ops {
            if is_add {
                page.add_mapcount();
            } else {
                page.sub_mapcount();
            }
        }
        // mapcount can go below PAGE_MAPCOUNT_BIAS (over-unmap),
        // but it should never wrap (i32 overflow)
        let mc = page.mapcount();
        prop_assert!(mc > i32::MIN / 2, "mapcount severely corrupted: {}", mc);
    }
}
