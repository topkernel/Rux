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

use core::sync::atomic::{AtomicU32, Ordering};

pub const PID_MAX_LIMIT: u32 = 4194304; // 4M (default 32768, max 4M)

pub const PID_SWAPPER: u32 = 0;  // idle process
pub const PID_INIT: u32 = 1;     // init process

static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT + 1);

pub fn alloc_pid() -> Option<u32> {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    if pid >= PID_MAX_LIMIT {
        // TODO: Implement PID reuse
        None
    } else {
        Some(pid)
    }
}

pub fn free_pid(_pid: u32) {
    // TODO: Implement PID bitmap release
}
