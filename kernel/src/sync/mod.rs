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

// The subsystem's ONE audited unsafe file (mainline minimal-first
// consolidation, 2026-11-27): the former `edges` collection renamed to
// `unsafe.rs` on disk (2026-11-25 edges->unsafe.rs ruling), module
// named `edges` so every existing call site stays stable.
#[path = "unsafe.rs"]
pub mod edges;
pub mod semaphore;
pub mod condvar;
pub mod futex;
pub mod futex_pi;
pub mod mutex;
pub mod spinlock;
pub mod rwlock;
pub mod rwsem;
pub mod seqlock;
pub mod rcu;
pub mod percpu;
pub mod lockkey;

pub use mutex::SleepMutex;
pub use semaphore::Mutex;
pub use futex::{futex_wait, futex_wake, do_futex, sys_futex_handler};
pub use spinlock::{Spinlock, SpinlockGuard, SpinlockIrqGuard, SpinlockBhGuard, RawSpinlock};
pub use rwlock::{
    RwSpinlock, RwSpinlockReadGuard, RwSpinlockWriteGuard,
    RwSpinlockIrqReadGuard, RwSpinlockIrqWriteGuard,
    RwSpinlockBhReadGuard, RwSpinlockBhWriteGuard,
};
pub use rwsem::{RwSemaphore, Rwsem, RwsemReadGuard, RwsemWriteGuard};
pub use rcu::{RcuHead, RcuCallback, rcu_read_lock, rcu_read_unlock, call_rcu, synchronize_rcu, synchronize_rcu_bounded};
pub use percpu::{CpuLocal, CpuLocalRef};
pub use seqlock::{SeqLock, SeqLockWriteGuard, RawSeqLock};
