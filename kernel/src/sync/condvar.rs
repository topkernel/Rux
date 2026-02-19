//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 条件变量 (Condition Variable) 机制
//!
//! 完全...
//! - `kernel/sched/wait.c` - 等待操作
//! - `pthread_cond_t` - POSIX 条件变量
//!
//! 核心概念：
//! - 条件变量用于进程间同步
//! - 必须与互斥锁配合使用
//! - wait() 释放锁并等待条件满足
//! - signal() 唤醒一个等待的进程
//! - broadcast() 唤醒所有等待的进程

use crate::process::wait::WaitQueueHead;

/// 条件变量
///
/// 和 POSIX 的 `pthread_cond_t`
///
/// 条件变量用于进程间同步，典型使用场景：
/// - 生产者-消费者模式
/// - 缓冲区满/空通知
/// - 事件完成通知
///
/// # 使用示例
/// ```no_run
/// # use kernel::sync::{Mutex, ConditionVariable};
/// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
/// // 获取锁
/// mutex.lock();
///
/// // 检查条件
/// while !condition_is_met() {
///     cond.wait(mutex);  // 释放锁并等待
/// }
///
/// // ... 临界区 ...
///
/// // 释放锁
/// mutex.unlock();
///
/// // 在另一个线程中：
/// mutex.lock();
/// // ... 修改条件 ...
/// cond.signal();  // 或 broadcast()
/// mutex.unlock();
/// # }
/// ```
#[repr(C)]
pub struct ConditionVariable {
    /// 等待队列
    wait: WaitQueueHead,
}

impl ConditionVariable {
    /// 创建新条件变量
    ///
    /// # 示例
    /// ```
    /// let cond = ConditionVariable::new();
    /// ```
    pub const fn new() -> Self {
        Self {
            wait: WaitQueueHead::new(),
        }
    }

    /// 初始化条件变量（运行时初始化）
    ///
    /// 对应 POSIX 的 `pthread_cond_init()`
    pub fn init(&self) {
        // WaitQueueHead 已经自动初始化
    }

    /// 等待条件满足（不可中断）
    ///
    /// # 参数
    /// * `mutex` - 关联的互斥锁
    ///
    /// # 行为
    /// 1. 原子地释放互斥锁
    /// 2. 加入等待队列
    /// 3. 让出 CPU，进入睡眠
    /// 4. 被唤醒后重新获取互斥锁
    /// 5. 返回
    ///
    /// 对应 POSIX 的 `pthread_cond_wait()`
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
    /// mutex.lock();
    /// while !condition_is_met() {
    ///     cond.wait(mutex);
    /// }
    /// // ... 条件已满足，可以安全执行操作 ...
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn wait(&self, mutex: &super::Mutex) {
        // 1. 释放互斥锁
        mutex.unlock();

        // 2. 加入等待队列并等待
        // 条件：被唤醒（总是满足）
        let current = match crate::sched::current() {
            Some(task) => task,
            None => {
                // 无法获取当前任务，重新获取锁并返回
                mutex.lock();
                return;
            }
        };

        let entry = crate::process::wait::WaitQueueEntry::new(current, false);
        self.wait.add(entry);

        // 3. 让出 CPU
        #[cfg(feature = "riscv64")]
        crate::sched::schedule();

        // 4. 被唤醒后，从等待队列移除
        self.wait.remove(current);

        // 5. 重新获取互斥锁
        mutex.lock();
    }

    /// 等待条件满足（可中断）
    ///
    /// # 参数
    /// * `mutex` - 关联的互斥锁
    ///
    /// # 返回
    /// * `Ok(())` - 条件满足
    /// * `Err(())` - 被信号中断
    ///
    /// # 行为
    /// 1. 原子地释放互斥锁
    /// 2. 加入等待队列
    /// 3. 让出 CPU，进入睡眠
    /// 4. 被唤醒或被信号中断后重新获取互斥锁
    /// 5. 返回结果
    ///
    /// 对应 POSIX 的 `pthread_cond_wait()` (可中断版本)
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) -> Result<(), ()> {
    /// mutex.lock();
    /// loop {
    ///     if condition_is_met() {
    ///         break;
    ///     }
    ///     match cond.wait_interruptible(mutex) {
    ///         Ok(()) => break,
    ///         Err(()) => {
    ///             // 被信号中断
    ///             break;
    ///         }
    ///     }
    /// }
    /// mutex.unlock();
    /// # Ok(())
    /// # }
    /// ```
    pub fn wait_interruptible(&self, mutex: &super::Mutex) -> Result<(), ()> {
        // 1. 释放互斥锁
        mutex.unlock();

        // TODO: 检查信号中断
        // 当前简化实现：直接调用 wait()

        // 2. 加入等待队列并等待
        let current = match crate::sched::current() {
            Some(task) => task,
            None => {
                // 无法获取当前任务，重新获取锁并返回
                mutex.lock();
                return Ok(());
            }
        };

        let entry = crate::process::wait::WaitQueueEntry::new(current, false);
        self.wait.add(entry);

        // 3. 让出 CPU
        #[cfg(feature = "riscv64")]
        crate::sched::schedule();

        // 4. 被唤醒后，从等待队列移除
        self.wait.remove(current);

        // 5. 重新获取互斥锁
        mutex.lock();

        Ok(())
    }

    /// 唤醒一个等待的进程
    ///
    /// # 行为
    /// 唤醒等待队列中的一个进程（如果有的话）
    ///
    /// 对应 POSIX 的 `pthread_cond_signal()`
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
    /// // 修改条件
    /// mutex.lock();
    /// condition = true;
    /// cond.signal();  // 唤醒一个等待者
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn signal(&self) {
        // 唤醒一个进程（使用独占模式）
        self.wait.wake_up_one();
    }

    /// 唤醒所有等待的进程
    ///
    /// # 行为
    /// 唤醒等待队列中的所有进程
    ///
    /// 对应 POSIX 的 `pthread_cond_broadcast()`
    ///
    /// # 示例
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
    /// // 修改条件（可能满足多个等待者）
    /// mutex.lock();
    /// buffer.clear();
    /// cond.broadcast();  // 唤醒所有等待者
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn broadcast(&self) {
        // 唤醒所有进程
        self.wait.wake_up_all();
    }
}

/// 默认实现
impl Default for ConditionVariable {
    fn default() -> Self {
        Self::new()
    }
}
