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
//! PIDs are allocated from a monotonically increasing counter (fast path).
//! When the counter wraps around PID_MAX_LIMIT, freed PIDs are reused via
//! a free list.

use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::spinlock::Spinlock;

/// Maximum PID value - from config
pub const PID_MAX_LIMIT: u32 = crate::config::PID_MAX_LIMIT as u32;

pub const PID_SWAPPER: u32 = 0;  // idle process
pub const PID_INIT: u32 = 1;     // init process

/// Fast-path counter for PID allocation
static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT + 1);

/// Free list of previously freed PIDs for reuse
static FREED_PIDS: Spinlock<alloc::vec::Vec<u32>> = Spinlock::new(alloc::vec::Vec::new());

/// Allocate a new PID.
///
/// Fast path: monotonically increment NEXT_PID counter.
/// Slow path: when counter wraps, scan freed list for reusable PIDs.
pub fn alloc_pid() -> Option<u32> {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

    if pid < PID_MAX_LIMIT {
        return Some(pid);
    }

    // Counter wrapped: try to reuse a freed PID
    let mut freed = FREED_PIDS.lock();
    if let Some(pid) = freed.pop() {
        Some(pid)
    } else {
        // No PIDs available
        None
    }
}

/// Free a PID, marking it available for reuse.
///
/// Reserved PIDs (0, 1) are never freed.
pub fn free_pid(pid: u32) {
    if pid <= PID_INIT || pid >= PID_MAX_LIMIT {
        return;
    }
    let mut freed = FREED_PIDS.lock();
    freed.push(pid);
}
