//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PID bitmap allocator invariant tests.
//!
//! Types copied from: kernel/src/process/pid.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/process/pid.rs
// ============================================================================

pub const PID_MAX_DEFAULT: u32 = 4096;
pub const RESERVED_PIDS: u32 = 16;
pub const PID_BITMAP_WORDS: usize = ((PID_MAX_DEFAULT + 63) / 64) as usize;

pub struct PidAllocator {
    pub bitmap: [u64; PID_BITMAP_WORDS],
    pub next: u32,
    pub nr_allocated: u32,
}

impl PidAllocator {
    pub fn new() -> Self {
        Self {
            bitmap: [0u64; PID_BITMAP_WORDS],
            next: RESERVED_PIDS,
            nr_allocated: 0,
        }
    }

    pub fn scan_range(&self, lo: u32, hi: u32) -> Option<u32> {
        let mut pos = lo;
        while pos < hi {
            let word_idx = pos as usize / 64;
            let bit_in_word = pos as usize % 64;

            let mask = if bit_in_word > 0 {
                !((1u64 << bit_in_word) - 1)
            } else {
                u64::MAX
            };

            let inverted = (!self.bitmap[word_idx]) & mask;
            if inverted != 0 {
                let bit = inverted.trailing_zeros() as usize;
                let pid = (word_idx * 64 + bit) as u32;
                if pid < hi {
                    return Some(pid);
                }
            }

            pos = ((word_idx + 1) * 64) as u32;
        }
        None
    }

    pub fn find_next_zero(&self, start: u32) -> Option<u32> {
        let allocatable = PID_MAX_DEFAULT - RESERVED_PIDS;
        if self.nr_allocated >= allocatable {
            return None;
        }

        if let Some(pid) = self.scan_range(start, PID_MAX_DEFAULT) {
            return Some(pid);
        }

        if start > RESERVED_PIDS {
            self.scan_range(RESERVED_PIDS, start)
        } else {
            None
        }
    }

    pub fn alloc_pid(&mut self) -> Option<u32> {
        let pid = self.find_next_zero(self.next)?;

        let word_idx = pid as usize / 64;
        let bit_idx = pid as usize % 64;
        self.bitmap[word_idx] |= 1u64 << bit_idx;
        self.nr_allocated += 1;

        self.next = if pid + 1 >= PID_MAX_DEFAULT {
            RESERVED_PIDS
        } else {
            pid + 1
        };

        Some(pid)
    }

    pub fn free_pid(&mut self, pid: u32) {
        if pid < RESERVED_PIDS || pid >= PID_MAX_DEFAULT {
            return;
        }

        let word_idx = pid as usize / 64;
        let bit_idx = pid as usize % 64;
        let mask = 1u64 << bit_idx;

        if self.bitmap[word_idx] & mask != 0 {
            self.bitmap[word_idx] &= !mask;
            self.nr_allocated -= 1;
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-PID-1: Allocated PIDs are >= RESERVED_PIDS
    #[test]
    fn test_pid_reserved(_v in 0u8..1u8) {
        let mut alloc = PidAllocator::new();
        for _ in 0..100 {
            let pid = alloc.alloc_pid().unwrap();
            prop_assert!(pid >= RESERVED_PIDS);
        }
    }

    /// INV-PID-2: Allocated PIDs are unique
    #[test]
    fn test_pid_unique(
        count in 1usize..100usize,
    ) {
        let mut alloc = PidAllocator::new();
        let mut pids = std::collections::HashSet::new();
        for _ in 0..count {
            if let Some(pid) = alloc.alloc_pid() {
                prop_assert!(pids.insert(pid), "duplicate PID: {}", pid);
            }
        }
    }

    /// INV-PID-3: Free makes PID available again
    #[test]
    fn test_pid_free(
        count in 1usize..50usize,
    ) {
        let mut alloc = PidAllocator::new();
        let mut allocated = Vec::new();
        for _ in 0..count {
            if let Some(pid) = alloc.alloc_pid() {
                allocated.push(pid);
            }
        }
        // Free all
        for pid in &allocated {
            alloc.free_pid(*pid);
        }
        prop_assert_eq!(alloc.nr_allocated, 0);
        // Should be able to allocate again
        for &_pid in &allocated {
            let new_pid = alloc.alloc_pid().unwrap();
            prop_assert!(new_pid >= RESERVED_PIDS);
        }
    }

    /// INV-PID-4: Free reserved PIDs is no-op
    #[test]
    fn test_pid_free_reserved(pid in 0u32..RESERVED_PIDS) {
        let mut alloc = PidAllocator::new();
        alloc.free_pid(pid); // should not panic or change state
        prop_assert_eq!(alloc.nr_allocated, 0);
    }

    /// INV-PID-5: Free out-of-range PID is no-op
    #[test]
    fn test_pid_free_oorange(pid in PID_MAX_DEFAULT..(PID_MAX_DEFAULT + 100)) {
        let mut alloc = PidAllocator::new();
        alloc.free_pid(pid);
        prop_assert_eq!(alloc.nr_allocated, 0);
    }

    /// INV-PID-6: nr_allocated matches
    #[test]
    fn test_pid_count(ops in proptest::collection::vec(
        proptest::bool::ANY,
        1..100
    )) {
        let mut alloc = PidAllocator::new();
        let mut allocated = Vec::new();
        for &do_alloc in &ops {
            if do_alloc {
                if let Some(pid) = alloc.alloc_pid() {
                    allocated.push(pid);
                }
            } else if let Some(pid) = allocated.pop() {
                alloc.free_pid(pid);
            }
        }
        prop_assert_eq!(alloc.nr_allocated as usize, allocated.len());
    }

    /// INV-PID-7: scan_range finds first zero in range
    #[test]
    fn test_scan_range(lo in 16u32..100u32, hi in 100u32..200u32) {
        let alloc = PidAllocator::new(); // all zeros
        let result = alloc.scan_range(lo, hi);
        prop_assert!(result.is_some());
        let pid = result.unwrap();
        prop_assert!(pid >= lo);
        prop_assert!(pid < hi);
        prop_assert!(pid >= RESERVED_PIDS);
    }

    /// INV-PID-8: Exhaustion returns None
    #[test]
    fn test_pid_exhaustion(_v in 0u8..1u8) {
        let mut alloc = PidAllocator::new();
        let allocatable = (PID_MAX_DEFAULT - RESERVED_PIDS) as usize;
        for _ in 0..allocatable {
            alloc.alloc_pid().unwrap();
        }
        prop_assert!(alloc.alloc_pid().is_none());
    }

    /// INV-PID-9: Double-free is safe (no panic, no double-decrement)
    #[test]
    fn test_pid_double_free(_v in 0u8..1u8) {
        let mut alloc = PidAllocator::new();
        let pid = alloc.alloc_pid().unwrap();
        alloc.free_pid(pid);
        alloc.free_pid(pid); // should be no-op (bit already cleared)
        prop_assert_eq!(alloc.nr_allocated, 0);
    }
}
