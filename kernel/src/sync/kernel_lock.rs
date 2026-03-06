//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 内核大锁 (Kernel Big Lock)
//!
//! 一种简单的同步机制，在进入内核时获取锁，返回用户态时释放锁。
//! 这保证了内核代码的原子执行，简化并发控制。
//!
//! ## 设计
//!
//! - 进入内核（trap/系统调用）时：获取锁
//! - 返回用户态时：释放锁
//!
//! ## 注意
//!
//! 这是一个粗粒度的锁，适用于单核或简单的 SMP 场景。
//! Linux 使用更细粒度的锁机制（RCU、per-CPU 变量等）。

use core::sync::atomic::{AtomicBool, Ordering};

/// 全局内核大锁（简单自旋锁）
/// 使用 #[no_mangle] 使其对汇编可见
#[no_mangle]
pub static mut KERNEL_LOCK: AtomicBool = AtomicBool::new(false);

/// 获取内核大锁
///
/// 注意：实际获取锁的操作在 trap.S 中使用内联汇编实现
/// 此函数保留用于 Rust 代码中需要手动获取锁的场景
#[no_mangle]
pub extern "C" fn kernel_lock_acquire() {
    unsafe {
        while KERNEL_LOCK.compare_exchange(
            false,
            true,
            Ordering::Acquire,
            Ordering::Relaxed,
        ).is_err() {
            core::hint::spin_loop();
        }
    }
}

/// 释放内核大锁
///
/// 注意：实际释放锁的操作在 trap.S 中使用内联汇编实现
/// 此函数保留用于 Rust 代码中需要手动释放锁的场景
#[no_mangle]
pub extern "C" fn kernel_lock_release() {
    unsafe {
        KERNEL_LOCK.store(false, Ordering::Release);
    }
}

/// 检查当前是否持有内核大锁
#[inline]
pub fn is_locked() -> bool {
    unsafe { KERNEL_LOCK.load(Ordering::Acquire) }
}

/// 获取锁的递归深度（简化版，总是返回 1 或 0）
#[inline]
pub fn lock_depth() -> usize {
    if is_locked() { 1 } else { 0 }
}
