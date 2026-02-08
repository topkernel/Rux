//! PID 管理
//!
//! 遵循 Linux 内核的 PID 管理机制 (kernel/pid.c)
//!
//! Linux 的 PID 空间：
//! - PID 0: swapper/idle 进程
//! - PID 1: init 进程
//! - PID 2: kthreadd (内核线程守护进程)
//! - PID 3+: 普通 PID

use core::sync::atomic::{AtomicU32, Ordering};

/// 最大 PID 数值 (与 Linux 一致: /proc/sys/kernel/pid_max)
pub const PID_MAX_LIMIT: u32 = 4194304; // 4M (默认 32768，最大可到 4M)

/// 特殊 PID 定义
pub const PID_SWAPPER: u32 = 0;  // idle 进程
pub const PID_INIT: u32 = 1;     // init 进程

/// 全局 PID 分配器 (简单的原子递增实现)
/// TODO: 实现 PID bitmap 复用机制，遵循 Linux kernel/pid.c
static NEXT_PID: AtomicU32 = AtomicU32::new(PID_INIT + 1);

/// 分配一个新的 PID
///
/// 遵循 Linux 内核的 alloc_pid() 逻辑 (kernel/pid.c)
pub fn alloc_pid() -> Option<u32> {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    if pid >= PID_MAX_LIMIT {
        // TODO: 实现 PID 复用
        None
    } else {
        Some(pid)
    }
}

/// 释放 PID (占位，实际实现需要 bitmap)
///
/// 对应 Linux 内核的 free_pid() (kernel/pid.c)
pub fn free_pid(_pid: u32) {
    // TODO: 实现 PID bitmap 释放
}
