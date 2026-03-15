//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! PID Management
//!
//! - PID 0: swapper/idle process
//! - PID 1: init process
//! - PID 2: kthreadd (kernel thread daemon)
//! - PID 3+: Normal PIDs
//!
//! Uses bitmap for O(1) allocation and deallocation

use spin::Mutex;

/// Maximum PID value - from config
pub const PID_MAX_LIMIT: u32 = crate::config::PID_MAX_LIMIT as u32;

pub const PID_SWAPPER: u32 = 0;  // idle process
pub const PID_INIT: u32 = 1;     // init process

/// PID bitmap - each bit represents whether a PID is in use
/// PID 0 and 1 are reserved (set to 1 initially)
static PID_BITMAP: Mutex<[u64; (PID_MAX_LIMIT as usize + 63) / 64]> = Mutex::new({
    let mut bitmap = [0u64; (PID_MAX_LIMIT as usize + 63) / 64];
    // PID 0 (idle) and PID 1 (init) are always in use
    bitmap[0] = 0b11;
    bitmap
});

/// Find first zero bit in a u64
fn find_first_zero(word: u64) -> Option<u32> {
    if word == !0 {
        return None;
    }
    // Use trailing ones to find first zero
    Some(word.trailing_ones())
}

pub fn alloc_pid() -> Option<u32> {
    let mut bitmap = PID_BITMAP.lock();

    // Scan bitmap for free PID
    for (word_idx, word) in bitmap.iter().enumerate() {
        if let Some(bit_idx) = find_first_zero(*word) {
            let pid = (word_idx as u32) * 64 + bit_idx;
            if pid >= PID_MAX_LIMIT {
                return None;
            }
            bitmap[word_idx] |= 1u64 << bit_idx;
            return Some(pid);
        }
    }

    None
}

pub fn free_pid(pid: u32) {
    if pid <= PID_INIT {
        // Don't free reserved PIDs (0 and 1)
        return;
    }
    let mut bitmap = PID_BITMAP.lock();
    let word = pid as usize / 64;
    let bit = pid % 64;
    bitmap[word] &= !(1u64 << bit);
}
