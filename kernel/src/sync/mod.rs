//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 同步原语 (Synchronization Primitives)
//!
//! 包含：
//! - 信号量 (Semaphore)
//! - 条件变量 (Condvar)
//! - Futex (Fast Userspace Mutex)
//! - 内核大锁 (Kernel Big Lock)
//!
//! 参考：
//! - Linux kernel/locking/
//! - Linux kernel/futex/

pub mod semaphore;
pub mod condvar;
pub mod futex;
pub mod kernel_lock;

pub use semaphore::Mutex;
pub use futex::{futex_wait, futex_wake, do_futex, sys_futex_handler};
pub use kernel_lock::{kernel_lock_acquire, kernel_lock_release, is_locked, lock_depth};
