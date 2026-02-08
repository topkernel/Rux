//! 同步原语 (Synchronization Primitives)
//!
//! 完全遵循 Linux 内核的同步机制设计：
//! - `include/linux/semaphore.h` - 信号量
//! - `include/linux/mutex.h` - 互斥锁
//! - `kernel/locking/` - 锁实现
//!
//! 核心概念：
//! - 信号量用于进程同步和互斥
//! - P 操作 (down): 获取信号量
//! - V 操作 (up): 释放信号量

pub mod semaphore;

pub use semaphore::{Semaphore, Mutex, MutexGuard};
