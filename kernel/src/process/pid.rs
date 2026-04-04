//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PID Management — Bitmap-based cyclic allocator
//!
//! - PID 0: swapper/idle process
//! - PID 1: init process
//! - PID 300..32768: normal PIDs (allocated cyclically)
//!
//! PIDs are allocated from a static bitmap covering
//! [RESERVED_PIDS, PID_MAX_DEFAULT).  A cursor scans forward
//! from the last-allocated position and wraps around to
//! RESERVED_PIDS when reaching PID_MAX_DEFAULT, matching Linux's
//! cyclic PID reuse semantics.

use crate::sync::spinlock::Spinlock;

/// Absolute maximum PID value (from config, kept for compatibility).
pub const PID_MAX_LIMIT: u32 = crate::config::PID_MAX_LIMIT as u32;

/// Default maximum PID value for bitmap allocation.
pub const PID_MAX_DEFAULT: u32 = crate::config::PID_MAX_DEFAULT as u32;

/// Reserved low PIDs (0..RESERVED_PIDS are never allocated by alloc_pid).
pub const RESERVED_PIDS: u32 = crate::config::RESERVED_PIDS as u32;

pub const PID_SWAPPER: u32 = 0;  // idle process
pub const PID_INIT: u32 = 1;     // init process

/// Number of u64 words in the PID bitmap.
/// Covers [0, PID_MAX_DEFAULT).
const PID_BITMAP_WORDS: usize = (PID_MAX_DEFAULT as usize + 63) / 64;

/// Compile-time check: bitmap must cover the full PID range.
const _: () = assert!(PID_BITMAP_WORDS * 64 >= PID_MAX_DEFAULT as usize);

/// PID bitmap allocator with cyclic scan.
struct PidAllocator {
    /// Bitmap of allocated PIDs.  Bit N == 1 means PID N is in use.
    bitmap: [u64; PID_BITMAP_WORDS],
    /// Cursor: next PID to try on allocation.
    next: u32,
    /// Number of currently allocated PIDs.
    nr_allocated: u32,
}

impl PidAllocator {
    const fn new() -> Self {
        Self {
            bitmap: [0u64; PID_BITMAP_WORDS],
            next: RESERVED_PIDS,
            nr_allocated: 0,
        }
    }

    /// Scan bitmap words in [lo, hi) for the first zero bit.
    ///
    /// For each word, inverts it and masks off bits below `lo`'s position
    /// within the word, then uses `trailing_zeros()` (compiles to a single
    /// CTZ instruction on RISC-V) to find the first available PID.
    fn scan_range(&self, lo: u32, hi: u32) -> Option<u32> {
        let mut pos = lo;
        while pos < hi {
            let word_idx = pos as usize / 64;
            let bit_in_word = pos as usize % 64;

            // Mask out bits below current position in the first word.
            let mask = if bit_in_word > 0 {
                !((1u64 << bit_in_word) - 1)
            } else {
                u64::MAX
            };

            // Invert: set bits in `inverted` correspond to free PIDs.
            let inverted = (!self.bitmap[word_idx]) & mask;
            if inverted != 0 {
                let bit = inverted.trailing_zeros() as usize;
                let pid = (word_idx * 64 + bit) as u32;
                if pid < hi {
                    return Some(pid);
                }
            }

            // Advance to the next word.
            pos = ((word_idx + 1) * 64) as u32;
        }
        None
    }

    /// Find the next zero bit starting from `start`, wrapping around.
    fn find_next_zero(&self, start: u32) -> Option<u32> {
        // Fast path: all allocatable PIDs in use.
        let allocatable = PID_MAX_DEFAULT - RESERVED_PIDS;
        if self.nr_allocated >= allocatable {
            return None;
        }

        // Scan [start, PID_MAX_DEFAULT).
        if let Some(pid) = self.scan_range(start, PID_MAX_DEFAULT) {
            return Some(pid);
        }

        // Wrap: scan [RESERVED_PIDS, start).
        if start > RESERVED_PIDS {
            self.scan_range(RESERVED_PIDS, start)
        } else {
            None
        }
    }
}

/// Global PID allocator.
static PID_ALLOCATOR: Spinlock<PidAllocator> = Spinlock::new(PidAllocator::new());

/// Allocate a new PID using bitmap-based cyclic allocation.
///
/// Scans forward from the last-allocated position (cursor), wraps
/// around to RESERVED_PIDS when reaching PID_MAX_DEFAULT.
pub fn alloc_pid() -> Option<u32> {
    let mut alloc = PID_ALLOCATOR.lock();

    let pid = alloc.find_next_zero(alloc.next)?;

    // Mark allocated.
    let word_idx = pid as usize / 64;
    let bit_idx = pid as usize % 64;
    alloc.bitmap[word_idx] |= 1u64 << bit_idx;
    alloc.nr_allocated += 1;

    // Advance cursor.
    alloc.next = if pid + 1 >= PID_MAX_DEFAULT {
        RESERVED_PIDS
    } else {
        pid + 1
    };

    Some(pid)
}

/// Free a PID, clearing its bit in the bitmap.
///
/// Reserved PIDs (0..RESERVED_PIDS) and out-of-range PIDs are silently ignored.
pub fn free_pid(pid: u32) {
    if pid < RESERVED_PIDS || pid >= PID_MAX_DEFAULT {
        return;
    }

    let mut alloc = PID_ALLOCATOR.lock();

    let word_idx = pid as usize / 64;
    let bit_idx = pid as usize % 64;
    let mask = 1u64 << bit_idx;

    // Defensive: only decrement if the bit was actually set.
    if alloc.bitmap[word_idx] & mask != 0 {
        alloc.bitmap[word_idx] &= !mask;
        alloc.nr_allocated -= 1;
    }
}
