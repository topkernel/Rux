//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for semaphore counter arithmetic.
//! Copied from: kernel/src/sync/semaphore.rs

use proptest::prelude::*;

// Simplified semaphore: plain i32 counter instead of AtomicI32 + wait queue
pub struct Semaphore {
    count: i32,
}

impl Semaphore {
    pub fn new(value: i32) -> Self {
        Self { count: value }
    }

    // down: atomic fetch_sub. Returns old value.
    // If old > 0: acquired (return Ok)
    // If old <= 0: would block (return Err for trylock)
    pub fn down(&mut self) -> bool {
        let old = self.count;
        self.count -= 1;
        old > 0
    }

    // down_trylock: decrement, restore if old <= 0
    pub fn down_trylock(&mut self) -> bool {
        let old = self.count;
        self.count -= 1;
        if old > 0 {
            true // acquired
        } else {
            self.count += 1; // restore
            false
        }
    }

    // up: increment
    pub fn up(&mut self) {
        self.count += 1;
    }

    pub fn count(&self) -> i32 {
        self.count
    }
}

proptest! {
    #[test]
    fn test_down_decrements(initial in 1i32..100i32) {
        let mut sem = Semaphore::new(initial);
        let acquired = sem.down();
        assert!(acquired);
        assert_eq!(sem.count(), initial - 1);
    }

    #[test]
    fn test_up_increments(initial in 0i32..100i32) {
        let mut sem = Semaphore::new(initial);
        sem.up();
        assert_eq!(sem.count(), initial + 1);
    }

    #[test]
    fn test_trylock_success(initial in 1i32..100i32) {
        let mut sem = Semaphore::new(initial);
        assert!(sem.down_trylock());
        assert_eq!(sem.count(), initial - 1);
    }

    #[test]
    fn test_trylock_failure_restores(initial in -10i32..1i32) {
        let mut sem = Semaphore::new(initial);
        assert!(!sem.down_trylock());
        assert_eq!(sem.count(), initial, "trylock should restore count on failure");
    }

    #[test]
    fn test_down_up_symmetry(initial in 1i32..100i32) {
        let mut sem = Semaphore::new(initial);
        sem.down();
        sem.up();
        assert_eq!(sem.count(), initial);
    }

    #[test]
    fn test_trylock_up_symmetry(initial in 1i32..100i32) {
        let mut sem = Semaphore::new(initial);
        sem.down_trylock();
        sem.up();
        assert_eq!(sem.count(), initial);
    }

    #[test]
    fn test_multiple_acquires(initial in 1i32..100i32) {
        let mut sem = Semaphore::new(initial);
        // Acquire all available
        for _ in 0..initial {
            assert!(sem.down_trylock());
        }
        assert_eq!(sem.count(), 0);
        // Next trylock should fail
        assert!(!sem.down_trylock());
    }

    #[test]
    fn test_exhaust_and_refill(initial in 1i32..20i32) {
        let mut sem = Semaphore::new(initial);
        // Exhaust
        for _ in 0..initial {
            assert!(sem.down_trylock());
        }
        assert_eq!(sem.count(), 0);
        assert!(!sem.down_trylock());
        // Refill
        for _ in 0..initial {
            sem.up();
        }
        assert_eq!(sem.count(), initial);
        assert!(sem.down_trylock());
    }

    #[test]
    fn test_binary_mutex_semantics(_v in 0u8..1u8) {
        let mut sem = Semaphore::new(1); // mutex
        assert!(sem.down_trylock());
        assert!(!sem.down_trylock()); // already held
        sem.up();
        assert!(sem.down_trylock()); // released
    }

    #[test]
    fn test_counting_semaphore(initial in 2i32..10i32, acquires in 0usize..5usize) {
        let mut sem = Semaphore::new(initial);
        let n = (acquires as i32).min(initial);
        for _ in 0..n {
            sem.down();
        }
        assert_eq!(sem.count(), initial - n);
        for _ in 0..n {
            sem.up();
        }
        assert_eq!(sem.count(), initial);
    }

    #[test]
    fn test_up_on_zero(_v in 0u8..1u8) {
        let mut sem = Semaphore::new(0);
        sem.up();
        assert_eq!(sem.count(), 1);
        assert!(sem.down_trylock());
        assert_eq!(sem.count(), 0);
    }

    #[test]
    fn test_negative_count_allows_up(initial in -5i32..0i32) {
        let mut sem = Semaphore::new(initial);
        sem.up();
        assert_eq!(sem.count(), initial + 1);
    }
}
