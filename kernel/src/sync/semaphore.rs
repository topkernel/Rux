//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 信号量 (Semaphore) 机制
//!
//! 完全...
//! - `kernel/locking/semaphore.c` - 信号量操作
//!
//! 核心概念：
//! - 信号量用于进程同步和互斥
//! - P 操作 (down/down_interruptible): 获取信号量，可能阻塞
//! - V 操作 (up): 释放信号量，唤醒等待的进程

use core::sync::atomic::{AtomicI32, Ordering};
use crate::process::wait::WaitQueueHead;

/// 信号量
///
///
/// 信号量是一个非负整数，用于进程同步：
/// - 初始化为某个正整数
/// - P 操作 (down): 值减 1，如果为 0 则阻塞等待
/// - V 操作 (up): 值加 1，如果有进程在等待则唤醒
#[repr(C)]
pub struct Semaphore {
    /// 信号量计数值
    /// 使用原子整数保证线程安全
    count: AtomicI32,
    /// 等待队列
    /// 当信号量为 0 时，等待的进程加入此队列
    wait: WaitQueueHead,
}

impl Semaphore {
    /// 创建新信号量
    ///
    /// # 参数
    /// * `value` - 初始值
    ///
    /// # 示例
    /// ```
    /// // 互斥信号量（二值信号量）
    /// let mutex = Semaphore::new(1);
    ///
    /// // 计数信号量（资源池）
    /// let pool = Semaphore::new(10);
    /// ```
    pub const fn new(value: i32) -> Self {
        Self {
            count: AtomicI32::new(value),
            wait: WaitQueueHead::new(),
        }
    }

    /// 初始化信号量（运行时初始化）
    ///
    /// # 参数
    /// * `value` - 初始值
    pub fn init(&self, value: i32) {
        self.count.store(value, Ordering::Release);
        // WaitQueueHead 已经自动初始化
    }

    /// P 操作（不可中断）
    ///
    /// 也称为 down 操作或 wait 操作
    ///
    /// # 行为
    /// - 信号量值减 1
    /// - 如果值 >= 0，立即返回
    /// - 如果值 < 0，阻塞等待直到值变为正数
    ///
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) {
    /// sem.down();  // 获取信号量
    /// // ... 临界区 ...
    /// sem.up();    // 释放信号量
    /// # }
    /// ```
    pub fn down(&self) {
        // 原子减 1
        let old = self.count.fetch_sub(1, Ordering::Acquire);

        if old > 0 {
            // 成功获取信号量
            return;
        }

        // 信号量不足，需要等待
        // 检查条件：信号量值 > 0
        let has_semaphore = || self.count.load(Ordering::Acquire) > 0;

        loop {
            if has_semaphore() {
                // 重新尝试获取
                let old = self.count.fetch_sub(1, Ordering::Acquire);
                if old > 0 {
                    return;
                }
                // 还是失败，继续等待
                self.count.fetch_add(1, Ordering::Release);
            }

            // 添加到等待队列
            let current = match crate::sched::current() {
                Some(task) => task,
                None => return, // 无法获取当前任务，直接返回
            };

            let entry = crate::process::wait::WaitQueueEntry::new(current, false);
            self.wait.add(entry);

            // 让出 CPU
            #[cfg(feature = "riscv64")]
            crate::sched::schedule();

            // 被唤醒后，从等待队列移除
            self.wait.remove(current);
        }
    }

    /// P 操作（可中断）
    ///
    /// 也称为 down_interruptible 操作
    ///
    /// # 行为
    /// - 信号量值减 1
    /// - 如果值 >= 0，立即返回 Ok(())
    /// - 如果值 < 0，阻塞等待直到值变为正数或被信号中断
    ///
    /// # 返回
    /// - `Ok(())` - 成功获取信号量
    /// - `Err(())` - 被信号中断
    ///
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) -> Result<(), ()> {
    /// match sem.down_interruptible() {
    ///     Ok(()) => {
    ///         // 成功获取信号量
    ///         // ... 临界区 ...
    ///         sem.up();
    ///     }
    ///     Err(()) => {
    ///         // 被信号中断
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn down_interruptible(&self) -> Result<(), ()> {
        // 原子减 1
        let old = self.count.fetch_sub(1, Ordering::Acquire);

        if old > 0 {
            // 成功获取信号量
            return Ok(());
        }

        // 信号量不足，需要等待
        // TODO: 实现信号中断检查
        // 当前简化实现：调用 down()
        self.down();
        Ok(())
    }

    /// 尝试 P 操作（非阻塞）
    ///
    /// 也称为 try_down 或 down_trylock 操作
    ///
    /// # 行为
    /// - 信号量值减 1
    /// - 如果值 >= 0，返回 Ok(())
    /// - 如果值 < 0，立即返回 Err(())，不阻塞
    ///
    /// # 返回
    /// - `Ok(())` - 成功获取信号量
    /// - `Err(())` - 信号量不足
    ///
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) -> Result<(), ()> {
    /// match sem.down_trylock() {
    ///     Ok(()) => {
    ///         // 成功获取信号量
    ///         // ... 临界区 ...
    ///         sem.up();
    ///     }
    ///     Err(()) => {
    ///         // 信号量不足
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn down_trylock(&self) -> Result<(), ()> {
        // 原子减 1
        let old = self.count.fetch_sub(1, Ordering::Acquire);

        if old > 0 {
            // 成功获取信号量
            Ok(())
        } else {
            // 信号量不足，恢复值
            self.count.fetch_add(1, Ordering::Release);
            Err(())
        }
    }

    /// V 操作（释放信号量）
    ///
    /// 也称为 up 操作或 signal 操作
    ///
    /// # 行为
    /// - 信号量值加 1
    /// - 如果有进程在等待，唤醒一个进程
    ///
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) {
    /// sem.down();
    /// // ... 临界区 ...
    /// sem.up();  // 释放信号量，唤醒等待的进程
    /// # }
    /// ```
    pub fn up(&self) {
        // 原子加 1
        let old = self.count.fetch_add(1, Ordering::Release);

        if old < 0 {
            // 之前有进程在等待，唤醒一个
            // 使用独占模式，只唤醒一个进程
            self.wait.wake_up_one();
        }
    }

    /// 获取信号量当前值
    ///
    /// # 返回
    /// 当前信号量值
    ///
    /// # 注意
    /// 此值仅供参考，实际值可能在调用后立即改变
    pub fn count(&self) -> i32 {
        self.count.load(Ordering::Acquire)
    }
}

/// 互斥信号量（Mutex）
///
/// 二值信号量，初始值为 1，用于互斥访问
///
///
/// # 示例
/// ```no_run
/// # use kernel::sync::Mutex;
/// # fn test(mutex: &Mutex) {
/// mutex.lock();
/// // ... 临界区 ...
/// mutex.unlock();
/// # }
/// ```
#[repr(C)]
pub struct Mutex {
    /// 内部信号量
    sem: Semaphore,
}

impl Mutex {
    /// 创建新互斥锁
    ///
    /// # 示例
    /// ```
    /// let mutex = Mutex::new();
    /// ```
    pub const fn new() -> Self {
        Self {
            sem: Semaphore::new(1),
        }
    }

    /// 获取锁
    ///
    /// 如果锁已被占用，则阻塞等待
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// mutex.lock();
    /// // ... 临界区 ...
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn lock(&self) {
        self.sem.down();
    }

    /// 尝试获取锁（非阻塞）
    ///
    /// # 返回
    /// - `Ok(())` - 成功获取锁
    /// - `Err(())` - 锁已被占用
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) -> Result<(), ()> {
    /// match mutex.try_lock() {
    ///     Ok(()) => {
    ///         // 成功获取锁
    ///         // ... 临界区 ...
    ///         mutex.unlock();
    ///     }
    ///     Err(()) => {
    ///         // 锁已被占用
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_lock(&self) -> Result<(), ()> {
        self.sem.down_trylock()
    }

    /// 释放锁
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// mutex.lock();
    /// // ... 临界区 ...
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn unlock(&self) {
        self.sem.up();
    }
}

/// 互斥锁守护（RAII）
///
/// 自动管理锁的生命周期
///
/// # 示例
/// ```no_run
/// # use kernel::sync::Mutex;
/// # fn test(mutex: &Mutex) {
/// {
///     let _guard = mutex.guard();
///     // ... 临界区 ...
/// } // 自动释放锁
/// # }
/// ```
pub struct MutexGuard<'a> {
    mutex: &'a Mutex,
}

impl<'a> MutexGuard<'a> {
    /// 创建锁守护
    ///
    /// # 参数
    /// * `mutex` - 关联的互斥锁
    pub fn new(mutex: &'a Mutex) -> Self {
        mutex.lock();
        Self { mutex }
    }
}

impl<'a> Drop for MutexGuard<'a> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl Mutex {
    /// 获取锁守护（RAII）
    ///
    /// 自动管理锁的生命周期，当守护离开作用域时自动释放锁
    ///
    /// # 返回
    /// MutexGuard 守护对象
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// {
    ///     let _guard = mutex.guard();
    ///     // ... 临界区 ...
    /// } // 自动释放锁
    /// # }
    /// ```
    pub fn guard(&self) -> MutexGuard<'_> {
        MutexGuard::new(self)
    }
}
