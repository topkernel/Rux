//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! PID 管理
//!
//!
//! - PID 0: swapper/idle 进程
//! - PID 1: init 进程
//! - PID 2: kthreadd (内核线程守护进程)
//! - PID 3+: 普通 PID

use core::sync::atomic::{AtomicU32, Ordering};

pub const PID_MAX_LIMIT: u32 = 4194304; // 4M (默认 32768，最大可到 4M)

pub const PID_SWAPPER: u32 = 0;  // idle 进程
pub const PID_INIT: u32 = 1;     // init 进程

static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT + 1);

pub fn alloc_pid() -> Option<u32> {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    if pid >= PID_MAX_LIMIT {
        // TODO: 实现 PID 复用
        None
    } else {
        Some(pid)
    }
}

pub fn free_pid(_pid: u32) {
    // TODO: 实现 PID bitmap 释放
}
