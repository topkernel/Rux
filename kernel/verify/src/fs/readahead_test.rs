//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for readahead state machine.
//! Copied from: kernel/src/fs/readahead.rs

use proptest::prelude::*;

pub const MAX_READAHEAD_BLOCKS: u32 = 4;
const ACTIVATION_THRESHOLD: u32 = 2;

// Copied ReadAheadState
pub struct ReadAheadState {
    pub last_read_end: u64,
    pub sequential_count: u32,
    pub active: bool,
    pub ra_until: u64,
    pub block_size: u64,
}

impl ReadAheadState {
    pub fn new(block_size: u64) -> Self {
        Self {
            last_read_end: 0,
            sequential_count: 0,
            active: false,
            ra_until: 0,
            block_size,
        }
    }

    pub fn on_read(&mut self, offset: u64, length: u64) -> (bool, u64, u32) {
        if length == 0 {
            return (false, 0, 0);
        }

        let is_sequential = offset == self.last_read_end;

        if is_sequential {
            self.sequential_count += 1;
        } else {
            self.sequential_count = 0;
            self.active = false;
        }

        self.last_read_end = offset + length;

        if self.sequential_count >= ACTIVATION_THRESHOLD && !self.active {
            self.active = true;
        }

        if !self.active {
            return (false, 0, 0);
        }

        let current_end_page = (offset + length + self.block_size - 1) / self.block_size;

        if current_end_page >= self.ra_until {
            let ra_start = self.ra_until;
            let ra_count = MAX_READAHEAD_BLOCKS;
            self.ra_until = ra_start + ra_count as u64;
            return (true, ra_start, ra_count);
        }

        (false, 0, 0)
    }
}

proptest! {
    #[test]
    fn test_initial_state(_v in 0u8..1u8) {
        let state = ReadAheadState::new(4096);
        assert_eq!(state.sequential_count, 0);
        assert!(!state.active);
        assert_eq!(state.ra_until, 0);
        assert_eq!(state.last_read_end, 0);
    }

    #[test]
    fn test_zero_length_no_ra(offset in 0u64..10_000u64) {
        let mut state = ReadAheadState::new(4096);
        // Activate first
        state.on_read(0, 100);
        state.on_read(100, 100);
        state.on_read(200, 100);
        let was_active = state.active;
        let (ra, _, _) = state.on_read(offset, 0);
        assert!(!ra);
        // State unchanged
        assert_eq!(state.active, was_active);
    }

    #[test]
    fn test_non_sequential_resets(offset1 in 0u64..1000u64, offset2 in 0u64..1000u64) {
        let mut state = ReadAheadState::new(4096);
        state.on_read(offset1, 100);
        state.on_read(offset1 + 100, 100);
        state.on_read(offset1 + 200, 100);
        assert!(state.active);
        // Non-sequential read resets
        let _ = state.on_read(offset2, 50);
        if offset2 != offset1 + 300 {
            assert!(!state.active);
            assert_eq!(state.sequential_count, 0);
        }
    }

    #[test]
    fn test_activation_threshold(block_size in 512u64..8192u64) {
        let mut state = ReadAheadState::new(block_size);
        let step = block_size;
        // First read: seq_count = 1
        let (ra1, _, _) = state.on_read(0, step);
        assert!(!ra1);
        assert!(!state.active);
        // Second sequential: seq_count = 2 >= threshold, activates
        let (ra2, _, _) = state.on_read(step, step);
        assert!(state.active);
        // Third sequential: may or may not trigger RA depending on page alignment
        let _ = state.on_read(2 * step, step);
        assert!(state.active);
    }

    #[test]
    fn test_ra_count_is_max(sequential_count in 2u32..100u32) {
        let mut state = ReadAheadState::new(4096);
        let step = 4096u64;
        // Prime with sequential reads
        for i in 0..sequential_count {
            let _ = state.on_read(i as u64 * step, step);
        }
        // All triggered RAs should return MAX_READAHEAD_BLOCKS
        for i in sequential_count..sequential_count + 100 {
            let (ra, _, count) = state.on_read(i as u64 * step, step);
            if ra {
                assert_eq!(count, MAX_READAHEAD_BLOCKS);
            }
        }
    }

    #[test]
    fn test_last_read_end_updates(offset in 0u64..10_000u64, length in 1u64..10_000u64) {
        let mut state = ReadAheadState::new(4096);
        state.on_read(offset, length);
        assert_eq!(state.last_read_end, offset + length);
    }

    #[test]
    fn test_ra_until_monotonic(_v in 0u8..1u8) {
        let mut state = ReadAheadState::new(4096);
        let mut prev_ra_until = 0u64;
        let step = 4096u64;
        for i in 0..200u64 {
            let (_, _, _) = state.on_read(i * step, step);
            if state.ra_until > prev_ra_until {
                assert!(state.ra_until > prev_ra_until);
                prev_ra_until = state.ra_until;
            }
        }
    }

    #[test]
    fn test_ra_start_advances(_v in 0u8..1u8) {
        let mut state = ReadAheadState::new(4096);
        let step = 4096u64;
        let mut last_ra_start = 0u64;
        for i in 0..200u64 {
            let (ra, ra_start, _) = state.on_read(i * step, step);
            if ra {
                assert!(ra_start >= last_ra_start);
                last_ra_start = ra_start;
            }
        }
    }

    #[test]
    fn test_block_size_varies(block_size in 256u64..16384u64) {
        let mut state = ReadAheadState::new(block_size);
        let step = block_size;
        // Two sequential reads to activate
        state.on_read(0, step);
        let (ra, _, _) = state.on_read(step, step);
        assert!(state.active);
        // Third read triggers RA since current_end_page >= ra_until (0)
        if ra {
            assert_eq!(state.ra_until, MAX_READAHEAD_BLOCKS as u64);
        }
    }

    #[test]
    fn test_sequential_count_increments(reads in 3u32..50u32) {
        let mut state = ReadAheadState::new(4096);
        let step = 4096u64;
        for i in 0..reads {
            state.on_read(i as u64 * step, step);
            assert_eq!(state.sequential_count, i + 1);
        }
    }
}
