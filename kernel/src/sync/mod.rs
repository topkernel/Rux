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
//! - Spinlock / RwSpinlock

pub mod semaphore;
pub mod condvar;
pub mod futex;
pub mod spinlock;
pub mod rwlock;

pub use semaphore::Mutex;
pub use futex::{futex_wait, futex_wake, do_futex, sys_futex_handler};
pub use spinlock::{Spinlock, SpinlockGuard, SpinlockIrqGuard, SpinlockBhGuard, RawSpinlock};
pub use rwlock::{
    RwSpinlock, RwSpinlockReadGuard, RwSpinlockWriteGuard,
    RwSpinlockIrqReadGuard, RwSpinlockIrqWriteGuard,
    RwSpinlockBhReadGuard, RwSpinlockBhWriteGuard,
};
