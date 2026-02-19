//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 同步原语 (Synchronization Primitives)
//!
//! 完全...
//! - `kernel/locking/` - 锁实现
//!
//! 核心概念：
//! - 信号量用于进程同步和互斥
//! - P 操作 (down): 获取信号量
//! - V 操作 (up): 释放信号量

pub mod semaphore;
pub mod condvar;

pub use semaphore::Mutex;
