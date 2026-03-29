//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Read-Ahead — detect sequential access patterns and prefetch file data.
//!
//! Each open file descriptor gets a `ReadAheadState` stored in `File.private_data`.
//! After detecting consecutive sequential reads, prefetches upcoming blocks
//! into the page cache to reduce VirtIO round-trips.

/// Maximum number of blocks to read ahead per trigger.
pub const MAX_READAHEAD_BLOCKS: u32 = 4;

/// Number of consecutive sequential reads before activating read-ahead.
const ACTIVATION_THRESHOLD: u32 = 2;

/// Read-ahead state for a single open file descriptor.
pub struct ReadAheadState {
    /// Offset where the last read ended.
    pub last_read_end: u64,
    /// Number of consecutive sequential reads detected.
    pub sequential_count: u32,
    /// Whether read-ahead is currently active.
    pub active: bool,
    /// Page index up to which read-ahead has been issued.
    pub ra_until: u64,
    /// Filesystem block size (cached at open time).
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

    /// Update state after a read at `offset` that produced `length` bytes.
    /// Returns `(should_ra, ra_start_page, ra_count)` if read-ahead should be issued.
    pub fn on_read(&mut self, offset: u64, length: u64) -> (bool, u64, u32) {
        if length == 0 {
            return (false, 0, 0);
        }

        // Detect sequential access: this read starts where the last one ended
        let is_sequential = offset == self.last_read_end;

        if is_sequential {
            self.sequential_count += 1;
        } else {
            // Seek or random access — reset
            self.sequential_count = 0;
            self.active = false;
        }

        self.last_read_end = offset + length;

        // Activate after threshold
        if self.sequential_count >= ACTIVATION_THRESHOLD && !self.active {
            self.active = true;
        }

        if !self.active {
            return (false, 0, 0);
        }

        // Check if we need to issue new read-ahead
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
