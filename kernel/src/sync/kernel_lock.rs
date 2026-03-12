//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel Big Lock
//!
//! A simple synchronization mechanism that acquires the lock when entering
//! the kernel and releases it when returning to user mode.
//! This ensures atomic execution of kernel code and simplifies concurrency control.
//!
//! ## Design
//!
//! - When entering kernel (trap/system call): acquire lock
//! - When returning to user mode: release lock
//!
//! ## Note
//!
//! This is a coarse-grained lock suitable for single-core or simple SMP scenarios.

use core::sync::atomic::{AtomicBool, Ordering};

/// Global kernel big lock (simple spinlock)
/// Use #[no_mangle] to make it visible to assembly
#[no_mangle]
pub static mut KERNEL_LOCK: AtomicBool = AtomicBool::new(false);

/// Acquire the kernel big lock
///
/// Note: The actual lock acquisition is implemented in trap.S using inline assembly
/// This function is reserved for scenarios where manual lock acquisition is needed in Rust code
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

/// Release the kernel big lock
///
/// Note: The actual lock release is implemented in trap.S using inline assembly
/// This function is reserved for scenarios where manual lock release is needed in Rust code
#[no_mangle]
pub extern "C" fn kernel_lock_release() {
    unsafe {
        KERNEL_LOCK.store(false, Ordering::Release);
    }
}

/// Check if the kernel big lock is currently held
#[inline]
pub fn is_locked() -> bool {
    unsafe { KERNEL_LOCK.load(Ordering::Acquire) }
}

/// Get the lock recursion depth (simplified version, always returns 1 or 0)
#[inline]
pub fn lock_depth() -> usize {
    if is_locked() { 1 } else { 0 }
}
