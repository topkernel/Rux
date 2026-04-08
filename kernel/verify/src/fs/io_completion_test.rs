//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for I/O completion state machine.
//! Copied from: kernel/src/fs/io_completion.rs

use proptest::prelude::*;

// Simplified IoCompletion: plain bool + i32 instead of AtomicBool + AtomicI32 + WaitQueue
pub struct IoCompletion {
    done: bool,
    status: i32,
}

impl IoCompletion {
    pub fn new() -> Self {
        Self { done: false, status: 0 }
    }

    pub fn complete(&mut self, status: i32) {
        self.status = status;
        self.done = true;
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn try_wait(&self) -> Option<i32> {
        if self.done {
            Some(self.status)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.done = false;
        self.status = 0;
    }

    pub fn status(&self) -> i32 {
        self.status
    }
}

// wait_for_all: returns 0 if all succeeded, first negative error
pub fn wait_for_all(statuses: &[i32]) -> i32 {
    let mut first_error = 0;
    for &s in statuses {
        if s < 0 && first_error == 0 {
            first_error = s;
        }
    }
    first_error
}

proptest! {
    #[test]
    fn test_initial_state(_v in 0u8..1u8) {
        let comp = IoCompletion::new();
        assert!(!comp.is_done());
        assert_eq!(comp.try_wait(), None);
        assert_eq!(comp.status(), 0);
    }

    #[test]
    fn test_complete_success(_v in 0u8..1u8) {
        let mut comp = IoCompletion::new();
        comp.complete(0);
        assert!(comp.is_done());
        assert_eq!(comp.try_wait(), Some(0));
        assert_eq!(comp.status(), 0);
    }

    #[test]
    fn test_complete_error(err in -100i32..0i32) {
        let mut comp = IoCompletion::new();
        comp.complete(err);
        assert!(comp.is_done());
        assert_eq!(comp.try_wait(), Some(err));
        assert_eq!(comp.status(), err);
    }

    #[test]
    fn test_complete_positive(status in 1i32..1000i32) {
        let mut comp = IoCompletion::new();
        comp.complete(status);
        assert!(comp.is_done());
        assert_eq!(comp.try_wait(), Some(status));
    }

    #[test]
    fn test_complete_idempotent(status1 in -100i32..100i32, status2 in -100i32..100i32) {
        let mut comp = IoCompletion::new();
        comp.complete(status1);
        comp.complete(status2);
        // Second complete overwrites (matches kernel: store without check)
        assert_eq!(comp.status(), status2);
        assert_ne!(comp.try_wait(), None);
    }

    #[test]
    fn test_try_wait_before_complete(_v in 0u8..1u8) {
        let comp = IoCompletion::new();
        assert_eq!(comp.try_wait(), None);
    }

    #[test]
    fn test_reset_after_complete(status in -100i32..100i32) {
        let mut comp = IoCompletion::new();
        comp.complete(status);
        comp.reset();
        assert!(!comp.is_done());
        assert_eq!(comp.try_wait(), None);
        assert_eq!(comp.status(), 0);
    }

    #[test]
    fn test_reset_idempotent(_v in 0u8..1u8) {
        let mut comp = IoCompletion::new();
        comp.reset();
        comp.reset();
        assert!(!comp.is_done());
        assert_eq!(comp.status(), 0);
    }

    #[test]
    fn test_wait_for_all_success(count in 1usize..20usize) {
        let statuses = vec![0i32; count];
        assert_eq!(wait_for_all(&statuses), 0);
    }

    #[test]
    fn test_wait_for_all_first_error(count in 1usize..20usize, err_pos in 0usize..20usize, err_val in -100i32..0i32) {
        let mut statuses = vec![0i32; count];
        let pos = err_pos % count;
        // Insert error at pos
        for i in 0..count {
            if i == pos {
                statuses[i] = err_val;
            }
        }
        let result = wait_for_all(&statuses);
        assert_eq!(result, err_val, "first error at pos {} should be returned", pos);
    }

    #[test]
    fn test_wait_for_all_multiple_errors(count in 2usize..20usize) {
        let mut statuses = vec![0i32; count];
        statuses[0] = -5;
        statuses[1] = -10;
        // Should return first error (-5), not second (-10)
        assert_eq!(wait_for_all(&statuses), -5);
    }

    #[test]
    fn test_wait_for_all_empty(_v in 0u8..1u8) {
        assert_eq!(wait_for_all(&[]), 0);
    }

    #[test]
    fn test_wait_for_all_positive_status(status in 1i32..100i32) {
        // Positive status is not an error
        assert_eq!(wait_for_all(&[status]), 0);
        assert_eq!(wait_for_all(&[0, status, 0]), 0);
    }

    #[test]
    fn test_complete_then_reset_cycle(status in -50i32..50i32, cycles in 1usize..5usize) {
        let mut comp = IoCompletion::new();
        for _ in 0..cycles {
            comp.complete(status);
            assert!(comp.is_done());
            comp.reset();
            assert!(!comp.is_done());
        }
        assert!(!comp.is_done());
        assert_eq!(comp.status(), 0);
    }
}
