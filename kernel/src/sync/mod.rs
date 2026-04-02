//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Synchronization Primitives
//!
//! Contains:
//! - Semaphore
//! - Condvar (Condition Variable)
//! - Futex (Fast Userspace Mutex)
//! - Kernel Big Lock

pub mod semaphore;
pub mod condvar;
pub mod futex;
pub mod kernel_lock;
pub mod spinlock;
pub mod rwlock;

pub use semaphore::Mutex;
pub use futex::{futex_wait, futex_wake, do_futex, sys_futex_handler};
pub use kernel_lock::{kernel_lock_acquire, kernel_lock_release, is_locked, lock_depth};
pub use spinlock::{Spinlock, SpinlockGuard, RawSpinlock};
pub use rwlock::{RwSpinlock, RwSpinlockReadGuard, RwSpinlockWriteGuard};
